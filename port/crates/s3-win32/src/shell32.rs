//! `shell32.dll` HLE. ShellExecute (help pages, the 565to555 converter, web links): logged and refused.
//!
//! `NAMES` lists every function we know the game can reach; all are declared (calling one without a handler
//! stops the machine with `UNIMPLEMENTED shell32.dll!Name`).

use s3_runtime::{LogLevel, Machine, Registry, ThunkResult};

/// DLL name as registered.
pub const DLL: &str = "shell32.dll";

/// Known exports.
pub const NAMES: &[&str] = &["ShellExecuteExA", "ShellExecuteA", "SHGetSpecialFolderPathA"];

const SE_ERR_NOASSOC: u32 = 31;

/// Declare all names and register implemented handlers.
pub fn register(reg: &mut Registry) {
    reg.declare_all(DLL, NAMES);
    reg.register(DLL, "ShellExecuteExA", shell_execute_ex_a);
    reg.register(DLL, "ShellExecuteA", shell_execute_a);
    reg.register(DLL, "SHGetSpecialFolderPathA", sh_get_special_folder_path_a);
}

/// ShellExecuteExA(SHELLEXECUTEINFOA*): fails with SE_ERR_NOASSOC (hInstApp at +0x20).
fn shell_execute_ex_a(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(0);
    let (verb, file, params) = if p != 0 {
        let rd = |m: &Machine, o: u32| {
            let s = m.mem.read_u32(p + o);
            if s == 0 { String::new() } else { m.mem.read_cstr(s) }
        };
        (rd(m, 0x0C), rd(m, 0x10), rd(m, 0x14))
    } else {
        Default::default()
    };
    m.log(LogLevel::Warn, &format!("ShellExecuteExA({verb:?}, {file:?}, {params:?}) refused"));
    if p != 0 {
        m.mem.write_u32(p + 0x20, SE_ERR_NOASSOC);
    }
    m.set_last_error(1155); // ERROR_NO_ASSOCIATION
    ThunkResult::stdcall(0, 1)
}

/// ShellExecuteA(hwnd, verb, file, params, dir, show)
fn shell_execute_a(m: &mut Machine) -> ThunkResult {
    let file = m.arg_str(2).unwrap_or_default();
    m.log(LogLevel::Warn, &format!("ShellExecuteA({file:?}) refused"));
    ThunkResult::stdcall(SE_ERR_NOASSOC, 6)
}

/// SHGetSpecialFolderPathA(hwnd, path, csidl, create): every folder is the game dir.
fn sh_get_special_folder_path_a(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(1);
    if p != 0 {
        m.mem.write_cstr(p, s3_runtime::GUEST_GAME_DIR);
    }
    ThunkResult::stdcall(1, 4)
}
