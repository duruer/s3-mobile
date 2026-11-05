//! DirectX HLE: DirectDraw (`ddraw.rs`), DirectSound (`dsound.rs`), DirectInput 3 (`dinput.rs`),
//! DirectPlay 6 + lobby stubs (`dplay.rs`). Pixel-level blit code is in `blit.rs`, the audio mixer in
//! `mixer.rs`. Documentation: docs/porting/directx-hle.md.
//!
//! COM objects use the runtime helpers (`Machine::com_vtable`, `Machine::com_new_object`): vtables are arrays
//! of thunks named `Interface::Method` in the implementing DLL, `this` is `m.arg_u32(0)`. Method lists are in
//! vtable order from the DirectX SDK headers (ddraw.h, dsound.h, dinput.h, dplay.h, dplobby.h). Host-side
//! object state lives in per-module `Machine::state` tables keyed by the guest object pointer.

pub mod blit;
pub mod ddraw;
pub mod dinput;
pub mod dplay;
pub mod dplay_net;
pub mod dsound;
pub mod mixer;

#[cfg(test)]
mod tests;

use s3_runtime::{Machine, Registry, ThunkResult};

/// Declare all DirectX exports and interface methods and register their handlers.
pub fn register_all(reg: &mut Registry) {
    ddraw::register(reg);
    dsound::register(reg);
    dinput::register(reg);
    dplay::register(reg);
}

/// Register COM class factories (CoCreateInstance) and the host-side services (audio mixer, DirectInput
/// event pump) in the machine.
pub fn register_factories(m: &mut Machine) {
    ddraw::register_factories(m);
    dplay::register_factories(m);
    m.add_service("dsound-mixer", dsound::mixer_service);
    m.add_service("dinput-pump", dinput::pump_service);
}

/// Declare every `Iface::Method` of an interface (so they show up in the registry before first use).
pub(crate) fn declare_interface(reg: &mut Registry, dll: &str, iface: &str, methods: &[&str]) {
    for m in methods {
        reg.declare(dll, &format!("{iface}::{m}"));
    }
}

/// Register `iface::name` in `dll`.
pub(crate) fn reg_method(reg: &mut Registry, dll: &str, iface: &str, name: &str, h: s3_runtime::Handler) {
    reg.register(dll, &format!("{iface}::{name}"), h);
}

/// IUnknown methods (first three slots of every interface).
pub const IUNKNOWN: [&str; 3] = ["QueryInterface", "AddRef", "Release"];

/// stdcall return helper.
#[inline]
pub(crate) fn ret(eax: u32, nargs: u32) -> ThunkResult {
    ThunkResult::stdcall(eax, nargs)
}

/// Write `v` to `*ptr` if ptr is not NULL.
#[inline]
pub(crate) fn put_u32(m: &mut Machine, ptr: u32, v: u32) {
    if ptr != 0 {
        m.mem.write_u32(ptr, v);
    }
}

/// Generate trivial stdcall handlers returning a constant: `stub!(name, nargs, result);`.
#[macro_export]
#[doc(hidden)]
macro_rules! stubs {
    ($($name:ident = ($nargs:expr, $res:expr);)*) => {
        $(
            fn $name(_m: &mut s3_runtime::Machine) -> s3_runtime::ThunkResult {
                s3_runtime::ThunkResult::stdcall($res, $nargs)
            }
        )*
    };
}

/// HRESULTs shared by the modules.
pub mod hr {
    pub const S_OK: u32 = 0;
    pub const S_FALSE: u32 = 1;
    pub const E_NOTIMPL: u32 = 0x8000_4001;
    pub const E_NOINTERFACE: u32 = 0x8000_4002;
    pub const E_POINTER: u32 = 0x8000_4003;
    pub const E_FAIL: u32 = 0x8000_4005;
    pub const E_INVALIDARG: u32 = 0x8007_0057;
    pub const E_OUTOFMEMORY: u32 = 0x8007_000E;
    pub const CLASS_E_NOAGGREGATION: u32 = 0x8004_0110;
}
