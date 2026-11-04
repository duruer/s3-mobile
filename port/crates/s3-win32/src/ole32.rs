//! `ole32.dll` HLE. COM: CoCreateInstance dispatches to factories registered in Machine::com (s3-dx registers DirectDraw/DirectPlay).
//!
//! `NAMES` lists every function we know the game can reach; all are declared (calling one without a handler
//! stops the machine with `UNIMPLEMENTED ole32.dll!Name`). Implement a function by writing a handler and
//! registering it in `register` (see docs/porting/architecture.md, "Adding an HLE function").

use s3_runtime::Registry;

#[allow(unused_imports)]
use s3_runtime::{Machine, ThunkResult};

/// DLL name as registered.
pub const DLL: &str = "ole32.dll";

/// Known exports (static imports of S3.EXE first).
pub const NAMES: &[&str] = &[
    "CoCreateGuid", "CoInitialize", "CoUninitialize", "CoCreateInstance", "CoInitializeEx",
    "CoTaskMemAlloc", "CoTaskMemFree", "OleInitialize", "OleUninitialize",
];

/// Declare all names and register implemented handlers.
pub fn register(reg: &mut Registry) {
    reg.declare_all(DLL, NAMES);
    reg.register(DLL, "CoInitialize", co_initialize);
    reg.register(DLL, "CoUninitialize", co_uninitialize);
    reg.register(DLL, "CoCreateInstance", co_create_instance);
    reg.register(DLL, "CoInitializeEx", co_initialize_ex);
    reg.register(DLL, "OleInitialize", co_initialize);
    reg.register(DLL, "OleUninitialize", co_uninitialize);
    reg.register(DLL, "CoCreateGuid", co_create_guid);
    reg.register(DLL, "CoTaskMemAlloc", co_task_mem_alloc);
    reg.register(DLL, "CoTaskMemFree", co_task_mem_free);
}

fn co_initialize_ex(_m: &mut Machine) -> ThunkResult {
    ThunkResult::stdcall(0, 2) // S_OK
}

/// CoCreateGuid(GUID*): random v4 GUID.
fn co_create_guid(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(0);
    if p == 0 {
        return ThunkResult::stdcall(0x8007_0057, 1); // E_INVALIDARG
    }
    let g = crate::rpcrt4::random_guid(m);
    m.mem.write_bytes(p, &g);
    ThunkResult::stdcall(0, 1)
}

fn co_task_mem_alloc(m: &mut Machine) -> ThunkResult {
    let n = m.arg_u32(0);
    let p = m.mem.alloc(n.max(1));
    ThunkResult::stdcall(p, 1)
}

fn co_task_mem_free(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(0);
    if p != 0 {
        m.mem.free(p);
    }
    ThunkResult::stdcall(0, 1)
}

fn co_initialize(_m: &mut Machine) -> ThunkResult {
    ThunkResult::stdcall(0, 1) // S_OK
}

fn co_uninitialize(_m: &mut Machine) -> ThunkResult {
    ThunkResult::stdcall(0, 0)
}

/// `CoCreateInstance(rclsid, pUnkOuter, dwClsContext, riid, ppv)`: looks the CLSID up in `Machine::com`
/// (factories registered by s3-dx etc.).
fn co_create_instance(m: &mut Machine) -> ThunkResult {
    use s3_runtime::com::{Guid, REGDB_E_CLASSNOTREG};
    let clsid = Guid::read(m, m.arg_u32(0));
    let iid = Guid::read(m, m.arg_u32(3));
    let ppv = m.arg_u32(4);
    let factory = m.com.factories.get(&clsid).map(|(n, f)| (n.clone(), *f));
    let hr = match factory {
        Some((name, f)) => match f(m, iid) {
            Ok(p) => {
                if ppv != 0 {
                    m.mem.write_u32(ppv, p);
                }
                m.log(s3_runtime::LogLevel::Debug, &format!("CoCreateInstance({name}, {iid}) -> {p:#x}"));
                0
            }
            Err(hr) => hr,
        },
        None => {
            m.log(s3_runtime::LogLevel::Warn, &format!("CoCreateInstance: class {clsid} not registered"));
            REGDB_E_CLASSNOTREG
        }
    };
    if hr != 0 && ppv != 0 {
        m.mem.write_u32(ppv, 0);
    }
    ThunkResult::stdcall(hr, 5)
}
