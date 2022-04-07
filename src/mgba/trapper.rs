use super::c;
use super::core;
use super::gba;

#[repr(C)]
pub struct Trapper {
    cpu_component: c::mCPUComponent,
    real_bkpt16: Option<unsafe extern "C" fn(*mut c::ARMCore, i32)>,
    r#impl: Impl,
    _pin: std::marker::PhantomPinned,
}

struct Trap {
    handler: Box<dyn Fn()>,
    original: u16,
}

struct Impl {
    core: std::sync::Arc<std::sync::Mutex<core::Core>>,
    traps: std::collections::HashMap<u32, Trap>,
}

const TRAPPER_IMM: i32 = 0xef;

unsafe extern "C" fn c_trapper_init(
    _cpu: *mut std::os::raw::c_void,
    _cpu_component: *mut c::mCPUComponent,
) {
}

unsafe extern "C" fn c_trapper_deinit(_cpu_component: *mut c::mCPUComponent) {}

unsafe extern "C" fn c_trapper_bkpt16(arm_core: *mut c::ARMCore, imm: i32) {
    let mut gba = gba::GBA((*arm_core).master as *mut _ as *mut c::GBA);
    let mut arm_core = gba.get_cpu();
    let components = arm_core.get_components_mut();
    let trapper =
        components[c::mCPUComponentType_CPU_COMPONENT_MISC_1 as usize] as *mut _ as *mut Trapper;
    if imm == TRAPPER_IMM {
        let caller = arm_core.get_gpr(15) as u32 - 4;
        let trap = (*trapper).r#impl.traps.get(&caller).unwrap();
        c::ARMRunFake(arm_core.0, trap.original as u32);
        (trap.handler)();
    }
    (*trapper).real_bkpt16.unwrap()(arm_core.0, imm)
}

impl Trapper {
    pub fn new(core: std::sync::Arc<std::sync::Mutex<core::Core>>) -> Self {
        let mut cpu_component = unsafe { std::mem::zeroed::<c::mCPUComponent>() };
        cpu_component.init = Some(c_trapper_init);
        cpu_component.deinit = Some(c_trapper_deinit);
        Trapper {
            cpu_component,
            real_bkpt16: None,
            r#impl: Impl {
                core,
                traps: std::collections::HashMap::new(),
            },
            _pin: std::marker::PhantomPinned,
        }
    }

    pub fn add(&mut self, addr: u32, handler: Box<dyn Fn()>) {
        let core = self.r#impl.core.lock().unwrap();
        let original = core.raw_read_16(addr, -1);
        core.raw_write_16(addr, -1, (0xbe00 | TRAPPER_IMM) as u16);
        self.r#impl.traps.insert(addr, Trap { original, handler });
    }

    pub fn attach(&mut self) {
        let mut arm_core = {
            let mut core = self.r#impl.core.lock().unwrap();
            core.get_gba().get_cpu().0
        };
        self.real_bkpt16 = unsafe { *arm_core }.irqh.bkpt16;
        unsafe {
            let components = std::slice::from_raw_parts_mut(
                (*arm_core).components,
                c::mCPUComponentType_CPU_COMPONENT_MAX as usize,
            );
            components[c::mCPUComponentType_CPU_COMPONENT_MISC_1 as usize] =
                self as *mut _ as *mut c::mCPUComponent;
            c::ARMHotplugAttach(arm_core, c::mCPUComponentType_CPU_COMPONENT_MISC_1 as u64);
            (*arm_core).irqh.bkpt16 = Some(c_trapper_bkpt16);
        }
    }
}