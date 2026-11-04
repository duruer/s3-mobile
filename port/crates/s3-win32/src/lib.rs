//! High-level emulation of the Win32 DLLs used by S3.EXE. One module per DLL; each has `NAMES` (declared
//! as UNIMPLEMENTED stubs) and `register()` which installs the implemented handlers.

pub mod advapi32;
pub mod audio_music;
pub mod registry_store;
pub mod comctl32;
pub mod comdlg32;
pub mod font;
pub mod gdi32;
pub mod kernel32;
pub mod ole32;
pub mod rpcrt4;
pub mod shell32;
pub mod user32;
pub mod wininet;
pub mod winmm;
pub mod wsock32;

use s3_runtime::Registry;

/// Register every Win32 module.
pub fn register_all(reg: &mut Registry) {
    kernel32::register(reg);
    user32::register(reg);
    gdi32::register(reg);
    advapi32::register(reg);
    shell32::register(reg);
    rpcrt4::register(reg);
    ole32::register(reg);
    winmm::register(reg);
    comctl32::register(reg);
    comdlg32::register(reg);
    wsock32::register(reg);
    wininet::register(reg);
}

#[cfg(test)]
mod user_gdi_tests;
