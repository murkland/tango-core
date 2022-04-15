use byteorder::ReadBytesExt;
use clap::Parser;
use cpal::traits::{HostTrait, StreamTrait};
use std::io::Read;

#[derive(clap::Parser)]
struct Cli {
    #[clap(long)]
    dump: bool,

    #[clap(parse(from_os_str))]
    path: std::path::PathBuf,
}

#[derive(Debug)]
struct InputPair {
    local_tick: u32,
    remote_tick: u32,
    p1_input: Input,
    p2_input: Input,
}

#[derive(Debug)]
struct Input {
    joyflags: u16,
    custom_screen_state: u8,
    turn: Vec<u8>,
}

struct Replay {
    local_player_index: u8,
    state: mgba::state::State,
    input_pairs: Vec<InputPair>,
}

const HEADER: &[u8] = b"TOOT";
const VERSION: u8 = 0x09;

impl Replay {
    fn decode(mut r: impl std::io::Read) -> std::io::Result<Self> {
        let mut header = [0u8; 4];
        r.read(&mut header)?;
        if &header != HEADER {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid header",
            ));
        }

        if r.read_u8()? != VERSION {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid version",
            ));
        }

        let mut zr = zstd::stream::read::Decoder::new(r)?;

        let local_player_index = zr.read_u8()?;

        let mut state = vec![0u8; zr.read_u32::<byteorder::LittleEndian>()? as usize];
        zr.read_exact(&mut state)?;
        let state = mgba::state::State::from_slice(&state);

        let mut input_pairs = vec![];

        loop {
            let local_tick = match zr.read_u32::<byteorder::LittleEndian>() {
                Ok(local_tick) => local_tick,
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::UnexpectedEof {
                        break;
                    }
                    return Err(e);
                }
            };
            let remote_tick = zr.read_u32::<byteorder::LittleEndian>()?;

            let p1_joyflags = zr.read_u16::<byteorder::LittleEndian>()?;
            let p2_joyflags = zr.read_u16::<byteorder::LittleEndian>()?;

            let p1_custom_screen_state = zr.read_u8()?;
            let p2_custom_screen_state = zr.read_u8()?;

            let mut p1_turn = vec![0u8; zr.read_u32::<byteorder::LittleEndian>()? as usize];
            zr.read_exact(&mut p1_turn)?;

            let mut p2_turn = vec![0u8; zr.read_u32::<byteorder::LittleEndian>()? as usize];
            zr.read_exact(&mut p2_turn)?;

            input_pairs.push(InputPair {
                local_tick,
                remote_tick,
                p1_input: Input {
                    joyflags: p1_joyflags,
                    custom_screen_state: p1_custom_screen_state,
                    turn: p1_turn,
                },
                p2_input: Input {
                    joyflags: p2_joyflags,
                    custom_screen_state: p2_custom_screen_state,
                    turn: p2_turn,
                },
            });
        }

        Ok(Replay {
            local_player_index,
            state,
            input_pairs,
        })
    }
}

fn main() -> Result<(), anyhow::Error> {
    env_logger::Builder::from_default_env()
        .filter(Some("tango"), log::LevelFilter::Info)
        .filter(Some("replayview"), log::LevelFilter::Info)
        .init();

    let args = Cli::parse();

    let mut f = zstd::stream::read::Decoder::new(std::fs::File::open(args.path)?)?;

    let replay = Replay::decode(&mut f)?;

    let rom_path = std::fs::read_dir("roms")?
        .flat_map(|dirent| {
            let dirent = dirent.as_ref().expect("dirent");
            let mut core = mgba::core::Core::new_gba("tango").expect("new_gba");
            let vf = match mgba::vfile::VFile::open(&dirent.path(), mgba::vfile::flags::O_RDONLY) {
                Ok(vf) => vf,
                Err(e) => {
                    log::warn!(
                        "failed to open {} for probing: {}",
                        dirent.path().display(),
                        e
                    );
                    return vec![];
                }
            };

            if let Err(e) = core.as_mut().load_rom(vf) {
                log::warn!(
                    "failed to load {} for probing: {}",
                    dirent.path().display(),
                    e
                );
                return vec![];
            }

            if core.as_ref().game_title() != replay.state.rom_title() {
                return vec![];
            }

            if core.as_ref().crc32() != replay.state.rom_crc32() {
                return vec![];
            }

            return vec![dirent.path()];
        })
        .next()
        .ok_or_else(|| anyhow::format_err!("could not find eligible rom"))?;

    log::info!("found rom: {}", rom_path.display());

    let core = {
        let mut core = mgba::core::Core::new_gba("tango")?;
        let vf = mgba::vfile::VFile::open(&rom_path, mgba::vfile::flags::O_RDONLY)?;
        core.as_mut().load_rom(vf)?;
        core.as_mut().load_state(&replay.state)?;
        core.enable_video_buffer();
        std::sync::Arc::new(parking_lot::Mutex::new(core))
    };

    let vbuf = std::sync::Arc::new(parking_lot::Mutex::new(vec![
        0u8;
        (mgba::gba::SCREEN_WIDTH * mgba::gba::SCREEN_HEIGHT * 4)
            as usize
    ]));

    let audio_device = cpal::default_host()
        .default_output_device()
        .ok_or_else(|| anyhow::format_err!("could not open audio device"))?;

    let stream = {
        let core = core.clone();
        mgba::audio::open_stream(core, &audio_device)?
    };
    stream.play()?;

    let mut thread = {
        let core = core.clone();
        mgba::thread::Thread::new(core)
    };
    {
        let core = core.clone();
        let vbuf = vbuf.clone();
        thread.set_frame_callback(Some(Box::new(move || {
            // TODO: This sometimes causes segfaults when the game gets unloaded.
            let core = core.lock();
            let mut vbuf = vbuf.lock();
            vbuf.copy_from_slice(core.video_buffer().unwrap());
            for i in (0..vbuf.len()).step_by(4) {
                vbuf[i + 3] = 0xff;
            }
        })));
    }
    thread.start();

    if args.dump {
        for ip in &replay.input_pairs {
            println!("{:?}", ip);
        }
    }

    let event_loop = winit::event_loop::EventLoop::new();

    let window = {
        let size =
            winit::dpi::LogicalSize::new(mgba::gba::SCREEN_WIDTH * 3, mgba::gba::SCREEN_HEIGHT * 3);
        winit::window::WindowBuilder::new()
            .with_title("tango replayview")
            .with_inner_size(size)
            .with_min_inner_size(size)
            .build(&event_loop)?
    };

    let mut pixels = {
        let window_size = window.inner_size();
        let surface_texture =
            pixels::SurfaceTexture::new(window_size.width, window_size.height, &window);
        pixels::PixelsBuilder::new(
            mgba::gba::SCREEN_WIDTH,
            mgba::gba::SCREEN_HEIGHT,
            surface_texture,
        )
        .build()?
    };

    {
        let vbuf = vbuf.clone();
        event_loop.run(move |event, _, control_flow| {
            *control_flow = winit::event_loop::ControlFlow::Poll;

            match event {
                winit::event::Event::MainEventsCleared => {
                    let vbuf = vbuf.lock().clone();
                    pixels.get_frame().copy_from_slice(&vbuf);
                    pixels.render().expect("render pixels");
                }
                _ => {}
            }
        });
    }
}
