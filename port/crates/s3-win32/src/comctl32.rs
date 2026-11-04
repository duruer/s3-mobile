//! `comctl32.dll` HLE. Common controls: initialization succeeds, nothing else is needed (HYPOTHESIS).
//!
//! `NAMES` lists every function we know the game can reach; all are declared (calling one without a handler
//! stops the machine with `UNIMPLEMENTED comctl32.dll!Name`).

use s3_runtime::{LogLevel, Machine, Registry, ThunkResult};

/// DLL name as registered.
pub const DLL: &str = "comctl32.dll";

/// Known exports.
pub const NAMES: &[&str] = &["InitCommonControls", "InitCommonControlsEx"];

/// Declare all names and register implemented handlers.
pub fn register(reg: &mut Registry) {
    reg.declare_all(DLL, NAMES);
    reg.register(DLL, "InitCommonControls", init_common_controls);
    reg.register(DLL, "InitCommonControlsEx", init_common_controls_ex);
}

fn init_common_controls(_m: &mut Machine) -> ThunkResult {
    ThunkResult::stdcall(0, 0)
}

fn init_common_controls_ex(m: &mut Machine) -> ThunkResult {
    m.log(LogLevel::Debug, "InitCommonControlsEx");
    ThunkResult::stdcall(1, 1)
}
