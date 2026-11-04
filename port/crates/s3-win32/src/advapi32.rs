//! `advapi32.dll` HLE. Registry backed by JSON in data_dir (see `registry_store.rs` for the load order and the
//! `SingleNumber` rule), plus `GetUserNameA`.
//!
//! `NAMES` lists every function we know the game can reach; all are declared (calling one without a handler
//! stops the machine with `UNIMPLEMENTED advapi32.dll!Name`).

use s3_runtime::{KernelObject, LogLevel, Machine, Registry, ThunkResult};

use crate::registry_store::{root_name, split_path, RegStore, RegValue, GENERAL_KEY, REG_SZ};

/// DLL name as registered.
pub const DLL: &str = "advapi32.dll";

/// Known exports (static imports of S3.EXE first).
pub const NAMES: &[&str] = &[
    "RegQueryInfoKeyA", "RegSetValueExA", "RegCloseKey", "GetUserNameA", "RegCreateKeyExA",
    "RegDeleteKeyA", "RegOpenKeyExA", "RegEnumKeyExA", "RegEnumValueA", "RegQueryValueExA",
    "RegOpenKeyA", "RegCreateKeyA", "RegSetValueA", "RegQueryValueA", "RegDeleteValueA",
    "RegFlushKey",
];

const ERROR_SUCCESS: u32 = 0;
const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_INVALID_HANDLE: u32 = 6;
const ERROR_MORE_DATA: u32 = 234;
const ERROR_NO_MORE_ITEMS: u32 = 259;

/// Declare all names and register implemented handlers.
pub fn register(reg: &mut Registry) {
    reg.declare_all(DLL, NAMES);
    reg.register(DLL, "RegOpenKeyExA", reg_open_key_ex_a);
    reg.register(DLL, "RegOpenKeyA", reg_open_key_a);
    reg.register(DLL, "RegCreateKeyExA", reg_create_key_ex_a);
    reg.register(DLL, "RegCreateKeyA", reg_create_key_a);
    reg.register(DLL, "RegCloseKey", reg_close_key);
    reg.register(DLL, "RegQueryValueExA", reg_query_value_ex_a);
    reg.register(DLL, "RegSetValueExA", reg_set_value_ex_a);
    reg.register(DLL, "RegQueryValueA", reg_query_value_a);
    reg.register(DLL, "RegSetValueA", reg_set_value_a);
    reg.register(DLL, "RegDeleteKeyA", reg_delete_key_a);
    reg.register(DLL, "RegDeleteValueA", reg_delete_value_a);
    reg.register(DLL, "RegQueryInfoKeyA", reg_query_info_key_a);
    reg.register(DLL, "RegEnumKeyExA", reg_enum_key_ex_a);
    reg.register(DLL, "RegEnumValueA", reg_enum_value_a);
    reg.register(DLL, "RegFlushKey", reg_flush_key);
    reg.register(DLL, "GetUserNameA", get_user_name_a);
}

#[derive(Default)]
struct AdvState {
    store: Option<RegStore>,
}

/// An open key handle (in the kernel handle table).
struct KeyHandle(Vec<String>);

/// Run `f` on the (lazily loaded) registry store.
pub fn with_store<R>(m: &mut Machine, f: impl FnOnce(&mut RegStore) -> R) -> R {
    let st = m.state::<AdvState>();
    let mut g = st.borrow_mut();
    if g.store.is_none() {
        let (s, notes) = RegStore::load(&m.host.game_dir(), &m.host.data_dir(), s3_runtime::GUEST_GAME_DIR);
        for n in notes {
            m.log(LogLevel::Info, &n);
        }
        g.store = Some(s);
    }
    f(g.store.as_mut().unwrap())
}

fn save(m: &mut Machine) {
    let dd = m.host.data_dir();
    if let Err(e) = with_store(m, |s| s.save(&dd)) {
        m.log(LogLevel::Warn, &format!("registry: save failed: {e}"));
    }
}

/// Path of an HKEY (predefined root or open handle).
fn key_path(m: &Machine, hkey: u32) -> Option<Vec<String>> {
    if let Some(r) = root_name(hkey) {
        return Some(vec![r.to_string()]);
    }
    m.handles.get_other::<KeyHandle>(hkey).map(|k| k.0.clone())
}

fn sub_path(m: &Machine, hkey: u32, sub: Option<String>) -> Option<Vec<String>> {
    let mut p = key_path(m, hkey)?;
    if let Some(s) = sub {
        let extra = split_path(&format!("X\\{s}"));
        p.extend(extra.into_iter().skip(1));
        // Re-run the Wow6432Node folding on the full path.
        p = split_path(&p.join("\\"));
    }
    Some(p)
}

fn open_handle(m: &mut Machine, path: Vec<String>) -> u32 {
    m.handles.insert(KernelObject::Other("regkey", Box::new(KeyHandle(path))))
}

fn open_common(m: &mut Machine, hkey: u32, sub: Option<String>, out: u32, create: bool) -> (u32, u32) {
    let Some(path) = sub_path(m, hkey, sub) else { return (ERROR_INVALID_HANDLE, 0) };
    let disp;
    if create {
        let created = with_store(m, |s| s.create_key(&path));
        disp = if created { 1 } else { 2 };
        if created {
            save(m);
        }
    } else {
        if !with_store(m, |s| s.key(&path).is_some()) {
            m.log(LogLevel::Debug, &format!("RegOpenKey {} -> not found", path.join("\\")));
            if out != 0 {
                m.mem.write_u32(out, 0);
            }
            return (ERROR_FILE_NOT_FOUND, 0);
        }
        disp = 2;
    }
    let h = open_handle(m, path);
    if out != 0 {
        m.mem.write_u32(out, h);
    }
    (ERROR_SUCCESS, disp)
}

/// RegOpenKeyExA(hKey, lpSubKey, ulOptions, samDesired, phkResult)
fn reg_open_key_ex_a(m: &mut Machine) -> ThunkResult {
    let (hkey, sub, out) = (m.arg_u32(0), m.arg_str(1), m.arg_u32(4));
    ThunkResult::stdcall(open_common(m, hkey, sub, out, false).0, 5)
}

/// RegOpenKeyA(hKey, lpSubKey, phkResult)
fn reg_open_key_a(m: &mut Machine) -> ThunkResult {
    let (hkey, sub, out) = (m.arg_u32(0), m.arg_str(1), m.arg_u32(2));
    ThunkResult::stdcall(open_common(m, hkey, sub, out, false).0, 3)
}

/// RegCreateKeyExA(hKey, lpSubKey, Reserved, lpClass, dwOptions, samDesired, lpSA, phkResult, lpdwDisposition)
fn reg_create_key_ex_a(m: &mut Machine) -> ThunkResult {
    let (hkey, sub, out, disp) = (m.arg_u32(0), m.arg_str(1), m.arg_u32(7), m.arg_u32(8));
    let (r, d) = open_common(m, hkey, sub, out, true);
    if r == 0 && disp != 0 {
        m.mem.write_u32(disp, d);
    }
    ThunkResult::stdcall(r, 9)
}

/// RegCreateKeyA(hKey, lpSubKey, phkResult)
fn reg_create_key_a(m: &mut Machine) -> ThunkResult {
    let (hkey, sub, out) = (m.arg_u32(0), m.arg_str(1), m.arg_u32(2));
    ThunkResult::stdcall(open_common(m, hkey, sub, out, true).0, 3)
}

fn reg_close_key(m: &mut Machine) -> ThunkResult {
    let h = m.arg_u32(0);
    if root_name(h).is_some() {
        return ThunkResult::stdcall(0, 1);
    }
    let r = if m.handles.get_other::<KeyHandle>(h).is_some() {
        let _ = m.handles.close(h);
        0
    } else {
        ERROR_INVALID_HANDLE
    };
    ThunkResult::stdcall(r, 1)
}

/// Copy a value out with the usual size protocol. Returns the error code.
fn write_value_out(m: &mut Machine, v: &RegValue, lp_type: u32, lp_data: u32, lpcb: u32) -> u32 {
    if lp_type != 0 {
        m.mem.write_u32(lp_type, v.kind);
    }
    let need = v.data.len() as u32;
    if lpcb == 0 {
        return if lp_data == 0 { 0 } else { 87 }; // ERROR_INVALID_PARAMETER
    }
    let have = m.mem.read_u32(lpcb);
    m.mem.write_u32(lpcb, need);
    if lp_data == 0 {
        return 0;
    }
    if have < need {
        return ERROR_MORE_DATA;
    }
    m.mem.write_bytes(lp_data, &v.data);
    0
}

/// RegQueryValueExA(hKey, lpValueName, lpReserved, lpType, lpData, lpcbData)
fn reg_query_value_ex_a(m: &mut Machine) -> ThunkResult {
    let (hkey, name) = (m.arg_u32(0), m.arg_str(1).unwrap_or_default());
    let (lp_type, lp_data, lpcb) = (m.arg_u32(3), m.arg_u32(4), m.arg_u32(5));
    let Some(path) = key_path(m, hkey) else { return ThunkResult::stdcall(ERROR_INVALID_HANDLE, 6) };
    let v = with_store(m, |s| s.get_value(&path, &name).cloned());
    let r = match v {
        Some(v) => write_value_out(m, &v, lp_type, lp_data, lpcb),
        None => {
            m.log(LogLevel::Debug, &format!("RegQueryValueEx {}\\{name} -> not found", path.join("\\")));
            ERROR_FILE_NOT_FOUND
        }
    };
    ThunkResult::stdcall(r, 6)
}

fn is_general(path: &[String]) -> bool {
    let g = split_path(GENERAL_KEY);
    path.len() == g.len() && path.iter().zip(g.iter()).all(|(a, b)| a.eq_ignore_ascii_case(b))
}

fn set_value(m: &mut Machine, path: Vec<String>, v: RegValue) {
    if v.name.eq_ignore_ascii_case("SingleNumber") && is_general(&path) {
        let exists = with_store(m, |s| s.get_value(&path, "SingleNumber").is_some());
        if exists {
            m.log(LogLevel::Info, "registry: ignoring (encrypted) rewrite of SingleNumber, keeping the plain value");
            return;
        }
    }
    m.log(LogLevel::Debug, &format!("RegSetValue {}\\{} type {} len {}", path.join("\\"), v.name, v.kind, v.data.len()));
    with_store(m, |s| s.set_value(&path, v));
    save(m);
}

/// RegSetValueExA(hKey, lpValueName, Reserved, dwType, lpData, cbData)
fn reg_set_value_ex_a(m: &mut Machine) -> ThunkResult {
    let (hkey, name) = (m.arg_u32(0), m.arg_str(1).unwrap_or_default());
    let (kind, data, cb) = (m.arg_u32(3), m.arg_u32(4), m.arg_u32(5));
    let Some(path) = key_path(m, hkey) else { return ThunkResult::stdcall(ERROR_INVALID_HANDLE, 6) };
    let bytes = if data == 0 { Vec::new() } else { m.mem.read_bytes(data, cb.min(1 << 20) as usize) };
    set_value(m, path, RegValue { name, kind, data: bytes });
    ThunkResult::stdcall(0, 6)
}

/// RegQueryValueA(hKey, lpSubKey, lpValue, lpcbValue): default value of a subkey (string).
fn reg_query_value_a(m: &mut Machine) -> ThunkResult {
    let (hkey, sub, data, lpcb) = (m.arg_u32(0), m.arg_str(1), m.arg_u32(2), m.arg_u32(3));
    let Some(path) = sub_path(m, hkey, sub) else { return ThunkResult::stdcall(ERROR_INVALID_HANDLE, 4) };
    let r = match with_store(m, |s| s.key(&path).map(|k| k.values.get("").cloned())) {
        None => ERROR_FILE_NOT_FOUND,
        Some(v) => {
            let v = v.unwrap_or_else(|| RegValue::sz("", ""));
            write_value_out(m, &v, 0, data, lpcb)
        }
    };
    ThunkResult::stdcall(r, 4)
}

/// RegSetValueA(hKey, lpSubKey, dwType, lpData, cbData)
fn reg_set_value_a(m: &mut Machine) -> ThunkResult {
    let (hkey, sub, data) = (m.arg_u32(0), m.arg_str(1), m.arg_u32(3));
    let Some(path) = sub_path(m, hkey, sub) else { return ThunkResult::stdcall(ERROR_INVALID_HANDLE, 5) };
    let s = if data == 0 { String::new() } else { m.mem.read_cstr(data) };
    set_value(m, path, RegValue::sz("", &s));
    ThunkResult::stdcall(0, 5)
}

fn reg_delete_key_a(m: &mut Machine) -> ThunkResult {
    let (hkey, sub) = (m.arg_u32(0), m.arg_str(1));
    let Some(path) = sub_path(m, hkey, sub) else { return ThunkResult::stdcall(ERROR_INVALID_HANDLE, 2) };
    let ok = with_store(m, |s| s.delete_key(&path));
    save(m);
    ThunkResult::stdcall(if ok { 0 } else { ERROR_FILE_NOT_FOUND }, 2)
}

fn reg_delete_value_a(m: &mut Machine) -> ThunkResult {
    let (hkey, name) = (m.arg_u32(0), m.arg_str(1).unwrap_or_default());
    let Some(path) = key_path(m, hkey) else { return ThunkResult::stdcall(ERROR_INVALID_HANDLE, 2) };
    let ok = with_store(m, |s| s.delete_value(&path, &name));
    save(m);
    ThunkResult::stdcall(if ok { 0 } else { ERROR_FILE_NOT_FOUND }, 2)
}

/// RegQueryInfoKeyA(hKey, lpClass, lpcClass, lpReserved, lpcSubKeys, lpcMaxSubKeyLen, lpcMaxClassLen,
/// lpcValues, lpcMaxValueNameLen, lpcMaxValueLen, lpcbSecurityDescriptor, lpftLastWriteTime)
fn reg_query_info_key_a(m: &mut Machine) -> ThunkResult {
    let hkey = m.arg_u32(0);
    let Some(path) = key_path(m, hkey) else { return ThunkResult::stdcall(ERROR_INVALID_HANDLE, 12) };
    let info = with_store(m, |s| {
        s.key(&path).map(|k| {
            (
                k.subkeys.len() as u32,
                k.subkeys.values().map(|s| s.name.len()).max().unwrap_or(0) as u32,
                k.values.len() as u32,
                k.values.values().map(|v| v.name.len()).max().unwrap_or(0) as u32,
                k.values.values().map(|v| v.data.len()).max().unwrap_or(0) as u32,
            )
        })
    });
    let Some((nsub, maxsub, nval, maxname, maxval)) = info else { return ThunkResult::stdcall(ERROR_FILE_NOT_FOUND, 12) };
    let (cls, lpc_cls) = (m.arg_u32(1), m.arg_u32(2));
    if cls != 0 {
        m.mem.write_u8(cls, 0);
    }
    if lpc_cls != 0 {
        m.mem.write_u32(lpc_cls, 0);
    }
    for (i, v) in [(4, nsub), (5, maxsub), (6, 0), (7, nval), (8, maxname), (9, maxval), (10, 0)] {
        let p = m.arg_u32(i);
        if p != 0 {
            m.mem.write_u32(p, v);
        }
    }
    let ft = m.arg_u32(11);
    if ft != 0 {
        m.mem.write_u64(ft, 0x01D0_0000_0000_0000);
    }
    ThunkResult::stdcall(0, 12)
}

/// RegEnumKeyExA(hKey, dwIndex, lpName, lpcName, lpReserved, lpClass, lpcClass, lpftLastWriteTime)
fn reg_enum_key_ex_a(m: &mut Machine) -> ThunkResult {
    let (hkey, idx, name, lpc) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2), m.arg_u32(3));
    let Some(path) = key_path(m, hkey) else { return ThunkResult::stdcall(ERROR_INVALID_HANDLE, 8) };
    let n = with_store(m, |s| s.key(&path).and_then(|k| k.subkeys.values().nth(idx as usize).map(|s| s.name.clone())));
    let Some(n) = n else { return ThunkResult::stdcall(ERROR_NO_MORE_ITEMS, 8) };
    let have = if lpc != 0 { m.mem.read_u32(lpc) } else { 0 };
    if (n.len() as u32) + 1 > have {
        return ThunkResult::stdcall(ERROR_MORE_DATA, 8);
    }
    m.mem.write_cstr(name, &n);
    m.mem.write_u32(lpc, n.len() as u32);
    ThunkResult::stdcall(0, 8)
}

/// RegEnumValueA(hKey, dwIndex, lpValueName, lpcValueName, lpReserved, lpType, lpData, lpcbData)
fn reg_enum_value_a(m: &mut Machine) -> ThunkResult {
    let (hkey, idx, name, lpc) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2), m.arg_u32(3));
    let (lp_type, lp_data, lpcb) = (m.arg_u32(5), m.arg_u32(6), m.arg_u32(7));
    let Some(path) = key_path(m, hkey) else { return ThunkResult::stdcall(ERROR_INVALID_HANDLE, 8) };
    let v = with_store(m, |s| s.key(&path).and_then(|k| k.values.values().nth(idx as usize).cloned()));
    let Some(v) = v else { return ThunkResult::stdcall(ERROR_NO_MORE_ITEMS, 8) };
    let have = if lpc != 0 { m.mem.read_u32(lpc) } else { 0 };
    if (v.name.len() as u32) + 1 > have {
        return ThunkResult::stdcall(ERROR_MORE_DATA, 8);
    }
    m.mem.write_cstr(name, &v.name);
    m.mem.write_u32(lpc, v.name.len() as u32);
    let r = write_value_out(m, &v, lp_type, lp_data, lpcb);
    ThunkResult::stdcall(r, 8)
}

fn reg_flush_key(m: &mut Machine) -> ThunkResult {
    save(m);
    ThunkResult::stdcall(0, 1)
}

/// GetUserNameA(lpBuffer, pcbBuffer): the registry `Playername` (default "Player").
fn get_user_name_a(m: &mut Machine) -> ThunkResult {
    let (buf, pcb) = (m.arg_u32(0), m.arg_u32(1));
    let g = split_path(GENERAL_KEY);
    let name = with_store(m, |s| s.get_value(&g, "Playername").filter(|v| v.kind == REG_SZ).map(|v| v.as_str()))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Player".into());
    let have = if pcb != 0 { m.mem.read_u32(pcb) } else { 0 };
    let need = name.len() as u32 + 1;
    if pcb != 0 {
        m.mem.write_u32(pcb, need);
    }
    if buf == 0 || have < need {
        m.set_last_error(122); // ERROR_INSUFFICIENT_BUFFER
        return ThunkResult::stdcall(0, 2);
    }
    m.mem.write_cstr(buf, &name);
    ThunkResult::stdcall(1, 2)
}
