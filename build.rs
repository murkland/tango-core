extern crate bindgen;
extern crate winres;

use std::env;
use std::path::PathBuf;

fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();

    let mgba_dst = cmake::Config::new("external/mgba")
        .define("LIBMGBA_ONLY", "on")
        .build();

    println!(
        "cargo:rustc-link-search=native={}/build",
        mgba_dst.display()
    );
    println!("cargo:rustc-link-lib=static=mgba");
    match target_os.as_str() {
        "macos" => {
            println!("cargo:rustc-link-lib=framework=Cocoa");
        }
        "windows" => {
            println!("cargo:rustc-link-lib=shlwapi");
            println!("cargo:rustc-link-lib=ole32");
            println!("cargo:rustc-link-lib=uuid");
        }
        tos => panic!("unknown target os {:?}!", tos),
    }
    println!("cargo:rerun-if-changed=mgba_wrapper.h");
    let bindings = bindgen::Builder::default()
        .header("mgba_wrapper.h")
        .clang_args(&["-Iexternal/mgba/include", "-D__STDC_NO_THREADS__=1"])
        .parse_callbacks(Box::new(bindgen::CargoCallbacks))
        .generate()
        .expect("Unable to generate bindings");
    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("mgba_bindings.rs"))
        .expect("Couldn't write bindings!");

    if target_os == "windows" {
        let mut res = winres::WindowsResource::new();
        res.set_icon("tango.ico")
            .set_ar_path("x86_64-w64-mingw32-ar")
            .set_windres_path("x86_64-w64-mingw32-windres")
            .compile()
            .unwrap();
    }
}
