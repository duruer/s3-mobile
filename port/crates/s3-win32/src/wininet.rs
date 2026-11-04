//! `wininet.dll` HLE. WinINet stubs: no internet access in the port; every call fails cleanly
//! (ERROR_INTERNET_CANNOT_CONNECT).
//!
//! `NAMES` lists every function we know the game can reach; all are declared (calling one without a handler
//! stops the machine with `UNIMPLEMENTED wininet.dll!Name`).

use s3_runtime::{LogLevel, Machine, Registry, ThunkResult};

/// DLL name as registered.
pub const DLL: &str = "wininet.dll";

/// Known exports.
pub const NAMES: &[&str] = &[
    "InternetOpenA", "InternetOpenUrlA", "InternetReadFile", "InternetCloseHandle",
    "InternetGetConnectedState", "InternetConnectA", "HttpOpenRequestA", "HttpSendRequestA",
];

const ERROR_INTERNET_CANNOT_CONNECT: u32 = 12029;

/// Declare all names and register implemented handlers.
pub fn register(reg: &mut Registry) {
    reg.declare_all(DLL, NAMES);
    let h: &[(&str, s3_runtime::Handler)] = &[
        ("InternetOpenA", fail_5),
        ("InternetOpenUrlA", fail_6),
        ("InternetReadFile", fail_4),
        ("InternetCloseHandle", ok_1),
        ("InternetGetConnectedState", get_connected_state),
        ("InternetConnectA", fail_8),
        ("HttpOpenRequestA", fail_8),
        ("HttpSendRequestA", fail_5),
    ];
    for (n, f) in h {
        reg.register(DLL, n, *f);
    }
}

macro_rules! fail {
    ($name:ident, $n:expr) => {
        fn $name(m: &mut Machine) -> ThunkResult {
            m.set_last_error(ERROR_INTERNET_CANNOT_CONNECT);
            ThunkResult::stdcall(0, $n)
        }
    };
}
fail!(fail_4, 4);
fail!(fail_5, 5);
fail!(fail_6, 6);
fail!(fail_8, 8);

fn ok_1(_m: &mut Machine) -> ThunkResult {
    ThunkResult::stdcall(1, 1)
}

fn get_connected_state(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(0);
    if p != 0 {
        m.mem.write_u32(p, 0);
    }
    m.log(LogLevel::Debug, "InternetGetConnectedState -> offline");
    ThunkResult::stdcall(0, 2)
}
