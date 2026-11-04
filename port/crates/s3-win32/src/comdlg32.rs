//! `comdlg32.dll` HLE. Common dialogs (editor paths): every dialog is \"cancelled\".
//!
//! `NAMES` lists every function we know the game can reach; all are declared (calling one without a handler
//! stops the machine with `UNIMPLEMENTED comdlg32.dll!Name`).

use s3_runtime::{LogLevel, Machine, Registry, ThunkResult};

/// DLL name as registered.
pub const DLL: &str = "comdlg32.dll";

/// Known exports.
pub const NAMES: &[&str] = &["GetOpenFileNameA", "GetSaveFileNameA", "CommDlgExtendedError"];

/// Declare all names and register implemented handlers.
pub fn register(reg: &mut Registry) {
    reg.declare_all(DLL, NAMES);
    reg.register(DLL, "GetOpenFileNameA", cancelled);
    reg.register(DLL, "GetSaveFileNameA", cancelled);
    reg.register(DLL, "CommDlgExtendedError", no_error);
}

fn cancelled(m: &mut Machine) -> ThunkResult {
    m.log(LogLevel::Warn, "comdlg32: file dialog requested; returning cancel");
    ThunkResult::stdcall(0, 1)
}

fn no_error(_m: &mut Machine) -> ThunkResult {
    ThunkResult::stdcall(0, 0)
}
