//! `kernel32.dll` HLE. Processes, threads, sync, memory, files, time, locale, SEH.
//!
//! `NAMES` lists every function we know the game can reach; all are declared (calling one without a handler
//! stops the machine with `UNIMPLEMENTED kernel32.dll!Name`). Implement a function by writing a handler and
//! registering it in `register` (see docs/porting/architecture.md, "Adding an HLE function").
//!
//! Split by area (all `pub(crate)` submodules register into the same DLL):
//! - this file: process, modules, memory (Virtual*/Heap*/Local*/Global*), errors/SEH, debug output, resources
//! - `kernel32_sync.rs`: threads, events/mutexes/semaphores/waitable timers, waits, critical sections, TLS,
//!   Sleep, CloseHandle/DuplicateHandle, Interlocked*
//! - `kernel32_file.rs`: files, find, directories, mappings, drives, std handles (via the VFS)
//! - `kernel32_time.rs`: tick count, performance counter, system/local time, FILETIME conversions
//! - `kernel32_locale.rs`: code pages, locale info, MultiByteToWideChar & co., lstr*
//!
//! See docs/porting/kernel32.md.

use s3_runtime::resources::{find_resource_entry, ResId};
use s3_runtime::{LogLevel, Machine, Registry, ThunkResult};

#[path = "kernel32_file.rs"]
pub mod kernel32_file;
#[path = "kernel32_locale.rs"]
pub mod kernel32_locale;
#[path = "kernel32_sync.rs"]
pub mod kernel32_sync;
#[path = "kernel32_time.rs"]
pub mod kernel32_time;
#[cfg(test)]
#[path = "kernel32_tests.rs"]
mod kernel32_tests;

/// DLL name as registered.
pub const DLL: &str = "kernel32.dll";

/// Known exports (static imports of S3.EXE first).
pub const NAMES: &[&str] = &[
    "FindFirstFileA", "GetCurrentProcess", "SetFileAttributesA", "FindClose", "DuplicateHandle",
    "DeleteFileA", "GetDriveTypeA", "VirtualProtect", "GetModuleFileNameA", "OutputDebugStringA",
    "GetTickCount", "GetCurrentDirectoryA", "ResetEvent", "MoveFileA", "WaitForMultipleObjects",
    "SetCurrentDirectoryA", "CreateDirectoryA", "ReleaseSemaphore", "PulseEvent", "GetSystemTime",
    "Sleep", "SetEvent", "InterlockedExchange", "GetLastError", "CopyFileA", "CreateSemaphoreA",
    "IsBadReadPtr", "DeleteCriticalSection", "EnterCriticalSection", "VirtualFree", "HeapCreate",
    "HeapDestroy", "GetUserDefaultLCID", "EnumSystemLocalesA", "GetLocaleInfoA", "IsValidCodePage",
    "IsValidLocale", "SetEnvironmentVariableA", "CompareStringW", "LeaveCriticalSection",
    "ResumeThread", "GetStringTypeW", "GetACP", "GetStringTypeA", "GetCPInfo",
    "SetUnhandledExceptionFilter", "CompareStringA", "GetLocaleInfoW", "CreateProcessA",
    "GetExitCodeProcess", "IsBadCodePtr", "SetEndOfFile", "SetStdHandle", "FlushFileBuffers",
    "GetEnvironmentStringsW", "GetEnvironmentStrings", "FreeEnvironmentStringsW",
    "FreeEnvironmentStringsA", "UnhandledExceptionFilter", "IsBadWritePtr", "SetErrorMode",
    "CloseHandle", "UnmapViewOfFile", "MapViewOfFile", "CreateFileMappingA", "GetFileSize",
    "CreateFileA", "FreeLibrary", "LoadLibraryA", "WriteFile", "GetTempFileNameA", "GetTempPathA",
    "GetModuleHandleA", "GetVersionExA", "GetProcAddress", "QueryPerformanceCounter",
    "QueryPerformanceFrequency", "WaitForSingleObject", "CreateEventA", "VirtualAlloc",
    "CreateMutexA", "InitializeCriticalSection", "LCMapStringW", "GetFileAttributesA",
    "LCMapStringA", "SetThreadPriority", "GetCurrentThread", "LocalFree", "FormatMessageA",
    "SetLastError", "ReadFile", "SetFilePointer", "VirtualQuery", "GetSystemInfo", "lstrcpyA",
    "lstrlenA", "GetFileTime", "FileTimeToDosDateTime", "FileTimeToLocalFileTime",
    "GlobalMemoryStatus", "GetSystemTimeAsFileTime", "RtlUnwind", "RaiseException", "ExitProcess",
    "TerminateProcess", "GetTimeZoneInformation", "GetLocalTime", "WideCharToMultiByte", "GetOEMCP",
    "InterlockedDecrement", "InterlockedIncrement", "MultiByteToWideChar", "FindNextFileA",
    "FileTimeToSystemTime", "HeapAlloc", "HeapFree", "GetStartupInfoA", "GetCommandLineA",
    "GetVersion", "CreateThread", "GetCurrentThreadId", "TlsSetValue", "ExitThread", "TlsAlloc",
    "TlsGetValue", "SetHandleCount", "GetStdHandle", "GetFileType", "HeapReAlloc", "HeapSize",
    // Resolved with GetProcAddress by the game (wine-trace.md / static-startup.md) or cheap extras.
    "CreateWaitableTimerA", "OpenWaitableTimerA", "SetWaitableTimer", "CancelWaitableTimer",
    "GetFullPathNameA", "GetVolumeInformationA", "GetLogicalDrives", "GetDiskFreeSpaceA", "RemoveDirectoryA",
    "FindResourceA", "LoadResource", "LockResource", "SizeofResource", "FreeResource",
    "GetProcessHeap", "GetCurrentProcessId", "LocalAlloc", "LocalReAlloc", "LocalSize", "LocalLock", "LocalUnlock",
    "GlobalAlloc", "GlobalFree", "GlobalLock", "GlobalUnlock", "GlobalSize", "GlobalReAlloc", "GlobalHandle",
    "GetModuleFileNameW", "GetCommandLineW", "GetEnvironmentVariableA", "ExpandEnvironmentStringsA",
    "SuspendThread", "GetExitCodeThread", "TerminateThread", "GetThreadPriority", "SleepEx",
    "OpenEventA", "OpenMutexA", "OpenSemaphoreA", "WaitForSingleObjectEx", "WaitForMultipleObjectsEx",
    "InitializeCriticalSectionAndSpinCount", "TryEnterCriticalSection", "TlsFree",
    "InterlockedExchangeAdd", "InterlockedCompareExchange", "SystemTimeToFileTime", "LocalFileTimeToFileTime",
    "DosDateTimeToFileTime", "CompareFileTime", "SetFileTime", "OpenFileMappingA", "FlushViewOfFile",
    "GetSystemDefaultLCID", "GetUserDefaultLangID", "GetSystemDefaultLangID", "GetThreadLocale",
    "lstrlenW", "lstrcpynA", "lstrcatA", "lstrcmpA", "lstrcmpiA", "GetVersionExW", "GetComputerNameA",
    "GetWindowsDirectoryA", "GetSystemDirectoryA", "GetPrivateProfileStringA", "GetPrivateProfileIntA",
    "WritePrivateProfileStringA", "IsBadStringPtrA", "Beep", "GetProcessVersion",
];

/// Declare all names and register implemented handlers.
pub fn register(reg: &mut Registry) {
    reg.declare_all(DLL, NAMES);
    kernel32_sync::register(reg);
    kernel32_file::register(reg);
    kernel32_time::register(reg);
    kernel32_locale::register(reg);
    // process / modules
    reg.register(DLL, "GetVersion", get_version);
    reg.register(DLL, "IsProcessorFeaturePresent", is_processor_feature_present);
    reg.register(DLL, "GetVersionExA", get_version_ex_a);
    reg.register(DLL, "GetCurrentProcess", get_current_process);
    reg.register(DLL, "GetCurrentProcessId", |_m| ThunkResult::stdcall(s3_runtime::machine::PROCESS_ID, 0));
    reg.register(DLL, "GetLastError", get_last_error);
    reg.register(DLL, "SetLastError", set_last_error);
    reg.register(DLL, "ExitProcess", exit_process);
    reg.register(DLL, "TerminateProcess", terminate_process);
    reg.register(DLL, "CreateProcessA", create_process_a);
    reg.register(DLL, "GetExitCodeProcess", get_exit_code_process);
    reg.register(DLL, "GetCommandLineA", get_command_line_a);
    reg.register(DLL, "GetCommandLineW", |m| ThunkResult::stdcall(m.process.cmdline_w, 0));
    reg.register(DLL, "GetStartupInfoA", get_startup_info_a);
    reg.register(DLL, "GetEnvironmentStrings", |m| ThunkResult::stdcall(m.process.env_a, 0));
    reg.register(DLL, "GetEnvironmentStringsW", |m| ThunkResult::stdcall(m.process.env_w, 0));
    reg.register(DLL, "FreeEnvironmentStringsA", |_m| ThunkResult::stdcall(1, 1));
    reg.register(DLL, "FreeEnvironmentStringsW", |_m| ThunkResult::stdcall(1, 1));
    reg.register(DLL, "GetEnvironmentVariableA", get_environment_variable_a);
    reg.register(DLL, "SetEnvironmentVariableA", set_environment_variable_a);
    reg.register(DLL, "GetModuleHandleA", get_module_handle_a);
    reg.register(DLL, "GetModuleFileNameA", get_module_file_name_a);
    reg.register(DLL, "LoadLibraryA", load_library_a);
    reg.register(DLL, "GetProcAddress", get_proc_address);
    reg.register(DLL, "FreeLibrary", free_library);
    reg.register(DLL, "GetSystemInfo", get_system_info);
    reg.register(DLL, "GetWindowsDirectoryA", |m| sys_dir(m, "C:\\WINDOWS"));
    reg.register(DLL, "GetSystemDirectoryA", |m| sys_dir(m, "C:\\WINDOWS\\SYSTEM"));
    reg.register(DLL, "GetComputerNameA", get_computer_name_a);
    // memory
    reg.register(DLL, "GetProcessHeap", |m| ThunkResult::stdcall(m.mem.process_heap(), 0));
    reg.register(DLL, "HeapCreate", heap_create);
    reg.register(DLL, "HeapDestroy", heap_destroy);
    reg.register(DLL, "HeapAlloc", heap_alloc);
    reg.register(DLL, "HeapFree", heap_free);
    reg.register(DLL, "HeapReAlloc", heap_realloc);
    reg.register(DLL, "HeapSize", heap_size);
    reg.register(DLL, "VirtualAlloc", virtual_alloc);
    reg.register(DLL, "VirtualFree", virtual_free);
    reg.register(DLL, "VirtualQuery", virtual_query);
    reg.register(DLL, "VirtualProtect", virtual_protect);
    reg.register(DLL, "GlobalMemoryStatus", global_memory_status);
    reg.register(DLL, "LocalAlloc", local_alloc);
    reg.register(DLL, "GlobalAlloc", local_alloc);
    reg.register(DLL, "LocalFree", local_free);
    reg.register(DLL, "GlobalFree", local_free);
    reg.register(DLL, "LocalReAlloc", local_realloc);
    reg.register(DLL, "GlobalReAlloc", local_realloc);
    reg.register(DLL, "LocalSize", local_size);
    reg.register(DLL, "GlobalSize", local_size);
    reg.register(DLL, "LocalLock", |m| ThunkResult::stdcall(m.arg_u32(0), 1));
    reg.register(DLL, "GlobalLock", |m| ThunkResult::stdcall(m.arg_u32(0), 1));
    reg.register(DLL, "GlobalHandle", |m| ThunkResult::stdcall(m.arg_u32(0), 1));
    reg.register(DLL, "LocalUnlock", |_m| ThunkResult::stdcall(1, 1));
    reg.register(DLL, "GlobalUnlock", |_m| ThunkResult::stdcall(1, 1));
    reg.register(DLL, "IsBadReadPtr", is_bad_read_ptr);
    reg.register(DLL, "IsBadWritePtr", is_bad_read_ptr);
    reg.register(DLL, "IsBadCodePtr", is_bad_code_ptr);
    reg.register(DLL, "IsBadStringPtrA", is_bad_string_ptr_a);
    // errors / SEH / debug
    reg.register(DLL, "RaiseException", raise_exception);
    reg.register(DLL, "RtlUnwind", rtl_unwind);
    reg.register(DLL, "SetUnhandledExceptionFilter", set_unhandled_exception_filter);
    reg.register(DLL, "UnhandledExceptionFilter", unhandled_exception_filter);
    reg.register(DLL, "SetErrorMode", set_error_mode);
    reg.register(DLL, "OutputDebugStringA", output_debug_string_a);
    reg.register(DLL, "FormatMessageA", format_message_a);
    reg.register(DLL, "Beep", |_m| ThunkResult::stdcall(1, 2));
    // resources
    reg.register(DLL, "FindResourceA", find_resource_a);
    reg.register(DLL, "LoadResource", load_resource);
    reg.register(DLL, "LockResource", |m| ThunkResult::stdcall(m.arg_u32(0), 1));
    reg.register(DLL, "SizeofResource", sizeof_resource);
    reg.register(DLL, "FreeResource", |_m| ThunkResult::stdcall(0, 1));
}

const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_MOD_NOT_FOUND: u32 = 126;
const ERROR_PROC_NOT_FOUND: u32 = 127;
const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;

/// Windows 98 (4.10.1998): the game was built for Win9x; `GetVersion` high bit set = not NT.
fn get_version(_m: &mut Machine) -> ThunkResult {
    ThunkResult::stdcall(0xC0000A04, 0)
}

fn get_current_process(_m: &mut Machine) -> ThunkResult {
    ThunkResult::stdcall(s3_runtime::handles::CURRENT_PROCESS, 0)
}




fn get_last_error(m: &mut Machine) -> ThunkResult {
    ThunkResult::stdcall(m.last_error(), 0)
}

fn set_last_error(m: &mut Machine) -> ThunkResult {
    let v = m.arg_u32(0);
    m.set_last_error(v);
    ThunkResult::stdcall(0, 1)
}

fn exit_process(m: &mut Machine) -> ThunkResult {
    ThunkResult::Exit(m.arg_u32(0))
}

fn get_command_line_a(m: &mut Machine) -> ThunkResult {
    ThunkResult::stdcall(m.process.cmdline_a, 0)
}

fn get_module_handle_a(m: &mut Machine) -> ThunkResult {
    let r = match m.arg_str(0) {
        None => m.process.image_base,
        Some(name) => {
            let exe = m.process.module_path.rsplit('\\').next().unwrap_or("").to_ascii_lowercase();
            let n = s3_runtime::normalize_dll(&name);
            if n == exe || n.trim_end_matches(".dll") == exe.trim_end_matches(".exe") {
                m.process.image_base
            } else {
                m.registry.module_handle(&n).unwrap_or(0)
            }
        }
    };
    if r == 0 {
        m.set_last_error(ERROR_MOD_NOT_FOUND);
    }
    ThunkResult::stdcall(r, 1)
}

/// Only HLE modules can be "loaded": returns their fake HMODULE.
fn load_library_a(m: &mut Machine) -> ThunkResult {
    let name = m.arg_str(0).unwrap_or_default();
    let h = m.registry.module_handle(&name).unwrap_or(0);
    if h == 0 {
        m.log(s3_runtime::LogLevel::Warn, &format!("LoadLibraryA({name}): no HLE module"));
        m.set_last_error(ERROR_MOD_NOT_FOUND);
    }
    ThunkResult::stdcall(h, 1)
}

/// Returns 0 for functions nobody declared (logged), so optional-feature probes take their fallback path.
fn get_proc_address(m: &mut Machine) -> ThunkResult {
    let hmod = m.arg_u32(0);
    let p = m.arg_u32(1);
    let Some(dll) = m.registry.module_name(hmod).map(|s| s.to_string()) else {
        m.set_last_error(ERROR_MOD_NOT_FOUND);
        return ThunkResult::stdcall(0, 2);
    };
    let r = if p < 0x1_0000 {
        m.registry.lookup_ordinal(&dll, p as u16)
    } else {
        let name = m.mem.read_cstr(p);
        let r = m.registry.lookup(&dll, &name);
        if r.is_none() {
            m.log(s3_runtime::LogLevel::Warn, &format!("GetProcAddress({dll}, {name}): not declared"));
        }
        r
    };
    if r.is_none() {
        m.set_last_error(ERROR_PROC_NOT_FOUND);
    }
    ThunkResult::stdcall(r.unwrap_or(0), 2)
}

/// `IsProcessorFeaturePresent(feature)` (looked up by the CRT via GetProcAddress). Matches what our CPUID reports
/// (Pentium P54C: FPU, TSC, CMPXCHG8B; no MMX/SSE/3DNow, no FDIV errata, FPU not emulated).
pub fn processor_feature_present(feature: u32) -> bool {
    const PF_COMPARE_EXCHANGE_DOUBLE: u32 = 2;
    const PF_RDTSC_INSTRUCTION_AVAILABLE: u32 = 8;
    matches!(feature, PF_COMPARE_EXCHANGE_DOUBLE | PF_RDTSC_INSTRUCTION_AVAILABLE)
}

fn is_processor_feature_present(m: &mut Machine) -> ThunkResult {
    ThunkResult::stdcall(processor_feature_present(m.arg_u32(0)) as u32, 1)
}

fn free_library(_m: &mut Machine) -> ThunkResult {
    ThunkResult::stdcall(1, 1)
}

fn heap_create(m: &mut Machine) -> ThunkResult {
    let (initial, max) = (m.arg_u32(1), m.arg_u32(2));
    let h = m.mem.heap_create(initial, max).unwrap_or(0);
    ThunkResult::stdcall(h, 3)
}

fn heap_destroy(m: &mut Machine) -> ThunkResult {
    let h = m.arg_u32(0);
    let ok = m.mem.heap_destroy(h).is_ok();
    ThunkResult::stdcall(ok as u32, 1)
}

fn heap_alloc(m: &mut Machine) -> ThunkResult {
    let (h, flags, size) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2));
    let p = m.mem.heap_alloc(h, size, flags).unwrap_or(0);
    if p == 0 {
        m.set_last_error(ERROR_NOT_ENOUGH_MEMORY);
    }
    ThunkResult::stdcall(p, 3)
}

fn heap_free(m: &mut Machine) -> ThunkResult {
    let (h, p) = (m.arg_u32(0), m.arg_u32(2));
    let ok = p == 0 || m.mem.heap_free(h, p);
    ThunkResult::stdcall(ok as u32, 3)
}

fn heap_realloc(m: &mut Machine) -> ThunkResult {
    let (h, flags, p, size) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2), m.arg_u32(3));
    let r = m.mem.heap_realloc(h, p, size, flags).unwrap_or(0);
    ThunkResult::stdcall(r, 4)
}

fn heap_size(m: &mut Machine) -> ThunkResult {
    let (h, p) = (m.arg_u32(0), m.arg_u32(2));
    ThunkResult::stdcall(m.mem.heap_size(h, p).unwrap_or(u32::MAX), 3)
}

fn virtual_alloc(m: &mut Machine) -> ThunkResult {
    let (a, size, ty, prot) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2), m.arg_u32(3));
    let r = m.mem.virtual_alloc(a, size, ty, prot).unwrap_or(0);
    if r == 0 {
        m.set_last_error(ERROR_INVALID_PARAMETER);
    }
    ThunkResult::stdcall(r, 4)
}

fn virtual_free(m: &mut Machine) -> ThunkResult {
    let (a, size, ty) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2));
    let ok = m.mem.virtual_free(a, size, ty).is_ok();
    ThunkResult::stdcall(ok as u32, 3)
}

fn virtual_query(m: &mut Machine) -> ThunkResult {
    let (a, buf, len) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2));
    if len < 28 {
        return ThunkResult::stdcall(0, 3);
    }
    let info = m.mem.virtual_query(a).to_bytes();
    m.mem.write_bytes(buf, &info);
    ThunkResult::stdcall(28, 3)
}

fn virtual_protect(m: &mut Machine) -> ThunkResult {
    let (a, size, prot, old) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2), m.arg_u32(3));
    match m.mem.virtual_protect(a, size, prot) {
        Ok(o) => {
            if old != 0 {
                m.mem.write_u32(old, o);
            }
            ThunkResult::stdcall(1, 4)
        }
        Err(_) => {
            m.set_last_error(ERROR_INVALID_PARAMETER);
            ThunkResult::stdcall(0, 4)
        }
    }
}




/// `RaiseException(code, flags, nargs, args)`: dispatch through the guest SEH chain. If a handler returns
/// ExceptionContinueExecution the call returns normally (CONTEXT.Eip = return address).
fn raise_exception(m: &mut Machine) -> ThunkResult {
    use s3_cpu::ESP;
    let (code, flags, n, args) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2), m.arg_u32(3));
    let params: Vec<u32> = (0..n.min(15)).map(|i| m.mem.read_u32(args + 4 * i)).collect();
    let ret = m.return_address();
    let mut c = m.cpu().context();
    c.eip = ret;
    c.regs[ESP] = c.regs[ESP].wrapping_add(4 + 16);
    s3_runtime::seh::raise_exception(m, code, flags, &params, c, ret);
    ThunkResult::Done
}

/// `RtlUnwind(target_frame, target_ip, exception_record, return_value)`.
fn rtl_unwind(m: &mut Machine) -> ThunkResult {
    let (frame, _ip, rec, rv) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2), m.arg_u32(3));
    s3_runtime::seh::rtl_unwind(m, frame, rec);
    ThunkResult::stdcall(rv, 4)
}

// ================================================================================================ process

/// `GetVersionExA(OSVERSIONINFOA*)`: Windows 98 (4.10.1998, platform VER_PLATFORM_WIN32_WINDOWS), matching
/// `GetVersion`.
fn get_version_ex_a(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(0);
    let size = m.mem.read_u32(p);
    if size < 148 {
        m.set_last_error(122); // ERROR_INSUFFICIENT_BUFFER
        return ThunkResult::stdcall(0, 1);
    }
    m.mem.fill(p + 4, (size - 4) as usize, 0);
    m.mem.write_u32(p + 4, 4);
    m.mem.write_u32(p + 8, 10);
    m.mem.write_u32(p + 12, 0x040A_07CE);
    m.mem.write_u32(p + 16, 1);
    m.mem.write_cstr(p + 20, " A");
    ThunkResult::stdcall(1, 1)
}

fn terminate_process(m: &mut Machine) -> ThunkResult {
    let (h, code) = (m.arg_u32(0), m.arg_u32(1));
    if h == s3_runtime::handles::CURRENT_PROCESS || matches!(m.handles.get(h), Some(s3_runtime::KernelObject::Process)) {
        m.log(LogLevel::Info, &format!("TerminateProcess({code})"));
        return ThunkResult::Exit(code);
    }
    m.set_last_error(6);
    ThunkResult::stdcall(0, 2)
}

/// `CreateProcessA`: child processes are not supported (the game only uses it for the 565->555 patch
/// converter and external tools); fails with ERROR_FILE_NOT_FOUND.
fn create_process_a(m: &mut Machine) -> ThunkResult {
    let app = m.arg_str(0).unwrap_or_default();
    let cmd = m.arg_str(1).unwrap_or_default();
    m.log(LogLevel::Warn, &format!("CreateProcessA({app:?}, {cmd:?}): not supported"));
    m.set_last_error(2);
    ThunkResult::stdcall(0, 10)
}

fn get_exit_code_process(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(1);
    if p != 0 {
        m.mem.write_u32(p, 259); // STILL_ACTIVE
    }
    ThunkResult::stdcall(1, 2)
}

/// `GetStartupInfoA(STARTUPINFOA*)` (68 bytes, nothing inherited).
fn get_startup_info_a(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(0);
    m.mem.fill(p, 68, 0);
    m.mem.write_u32(p, 68);
    ThunkResult::stdcall(0, 1)
}

#[derive(Default)]
struct EnvState {
    vars: Option<Vec<(String, String)>>,
}

fn env_vars(m: &mut Machine) -> s3_runtime::Shared<EnvState> {
    let st = m.state::<EnvState>();
    {
        let mut s = st.borrow_mut();
        if s.vars.is_none() {
            s.vars = Some(
                s3_runtime::process::DEFAULT_ENV
                    .iter()
                    .filter_map(|v| v.split_once('='))
                    .map(|(a, b)| (a.to_string(), b.to_string()))
                    .collect(),
            );
        }
    }
    st
}

fn get_environment_variable_a(m: &mut Machine) -> ThunkResult {
    let (name, buf, size) = (m.arg_str(0).unwrap_or_default(), m.arg_u32(1), m.arg_u32(2));
    let st = env_vars(m);
    let v = st.borrow_mut().vars.as_ref().unwrap().iter().find(|(k, _)| k.eq_ignore_ascii_case(&name)).map(|(_, v)| v.clone());
    match v {
        Some(v) => {
            let len = v.len() as u32;
            if buf != 0 && size > len {
                m.mem.write_cstr(buf, &v);
                ThunkResult::stdcall(len, 3)
            } else {
                ThunkResult::stdcall(len + 1, 3)
            }
        }
        None => {
            m.set_last_error(203); // ERROR_ENVVAR_NOT_FOUND
            ThunkResult::stdcall(0, 3)
        }
    }
}

/// Stored host-side only (the environment block returned by GetEnvironmentStrings is not rebuilt).
fn set_environment_variable_a(m: &mut Machine) -> ThunkResult {
    let (name, value) = (m.arg_str(0).unwrap_or_default(), m.arg_str(1));
    let st = env_vars(m);
    let mut s = st.borrow_mut();
    let vars = s.vars.as_mut().unwrap();
    vars.retain(|(k, _)| !k.eq_ignore_ascii_case(&name));
    if let Some(v) = value {
        vars.push((name, v));
    }
    ThunkResult::stdcall(1, 2)
}

/// Guest path of a module handle: the exe, or `C:\WINDOWS\SYSTEM\<DLL>` for HLE modules.
fn module_path(m: &Machine, hmod: u32) -> Option<String> {
    if hmod == 0 || hmod == m.process.image_base {
        return Some(m.process.module_path.clone());
    }
    m.registry.module_name(hmod).map(|n| format!("C:\\WINDOWS\\SYSTEM\\{}", n.to_ascii_uppercase()))
}

/// `GetModuleFileNameA(hmod, buf, size)`: returns the number of chars copied (truncated, NUL-terminated).
fn get_module_file_name_a(m: &mut Machine) -> ThunkResult {
    let (hmod, buf, size) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2));
    let Some(path) = module_path(m, hmod) else {
        m.set_last_error(ERROR_MOD_NOT_FOUND);
        return ThunkResult::stdcall(0, 3);
    };
    if size == 0 {
        return ThunkResult::stdcall(0, 3);
    }
    let n = path.len().min(size as usize - 1);
    m.mem.write_cstr(buf, &path[..n]);
    ThunkResult::stdcall(n as u32, 3)
}

/// `GetSystemInfo(SYSTEM_INFO*)`: one Pentium (586), 4 KiB pages, 64 KiB granularity.
fn get_system_info(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(0);
    m.mem.fill(p, 36, 0);
    m.mem.write_u32(p + 4, 0x1000);
    m.mem.write_u32(p + 8, 0x1_0000);
    m.mem.write_u32(p + 12, 0x7FFE_FFFF);
    m.mem.write_u32(p + 16, 1);
    m.mem.write_u32(p + 20, 1);
    m.mem.write_u32(p + 24, 586);
    m.mem.write_u32(p + 28, 0x1_0000);
    m.mem.write_u16(p + 32, 5);
    ThunkResult::stdcall(0, 1)
}

fn sys_dir(m: &mut Machine, dir: &str) -> ThunkResult {
    let (buf, size) = (m.arg_u32(0), m.arg_u32(1));
    let len = dir.len() as u32;
    if buf != 0 && size > len {
        m.mem.write_cstr(buf, dir);
        ThunkResult::stdcall(len, 2)
    } else {
        ThunkResult::stdcall(len + 1, 2)
    }
}

fn get_computer_name_a(m: &mut Machine) -> ThunkResult {
    let (buf, psize) = (m.arg_u32(0), m.arg_u32(1));
    let name = "S3PORT";
    let size = m.mem.read_u32(psize);
    if size <= name.len() as u32 {
        m.mem.write_u32(psize, name.len() as u32 + 1);
        m.set_last_error(111); // ERROR_BUFFER_OVERFLOW
        return ThunkResult::stdcall(0, 2);
    }
    m.mem.write_cstr(buf, name);
    m.mem.write_u32(psize, name.len() as u32);
    ThunkResult::stdcall(1, 2)
}

// ================================================================================================ memory

/// `GlobalMemoryStatus(MEMORYSTATUS*)`: 256 MiB RAM machine (values chosen to fit 32-bit math).
fn global_memory_status(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(0);
    let v = [32u32, 10, 256 << 20, 192 << 20, 512 << 20, 448 << 20, 0x7FFE_0000, 0x7000_0000];
    for (i, x) in v.iter().enumerate() {
        m.mem.write_u32(p + 4 * i as u32, *x);
    }
    ThunkResult::stdcall(0, 1)
}

const LMEM_ZEROINIT: u32 = 0x40;

/// `LocalAlloc/GlobalAlloc(flags, size)`: fixed blocks from the process heap (handle == pointer, also for
/// MOVEABLE; Lock returns the same value).
fn local_alloc(m: &mut Machine) -> ThunkResult {
    let (flags, size) = (m.arg_u32(0), m.arg_u32(1));
    let h = m.mem.process_heap();
    let zero = if flags & LMEM_ZEROINIT != 0 { s3_mem::HEAP_ZERO_MEMORY } else { 0 };
    let p = m.mem.heap_alloc(h, size.max(1), zero).unwrap_or(0);
    if p == 0 {
        m.set_last_error(ERROR_NOT_ENOUGH_MEMORY);
    }
    ThunkResult::stdcall(p, 2)
}

fn local_free(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(0);
    if p == 0 {
        return ThunkResult::stdcall(0, 1);
    }
    let h = m.mem.process_heap();
    if m.mem.heap_free(h, p) {
        ThunkResult::stdcall(0, 1)
    } else {
        m.set_last_error(6);
        ThunkResult::stdcall(p, 1)
    }
}

fn local_realloc(m: &mut Machine) -> ThunkResult {
    let (p, size, flags) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2));
    let h = m.mem.process_heap();
    let zero = if flags & LMEM_ZEROINIT != 0 { s3_mem::HEAP_ZERO_MEMORY } else { 0 };
    let r = m.mem.heap_realloc(h, p, size.max(1), zero).unwrap_or(0);
    if r == 0 {
        m.set_last_error(ERROR_NOT_ENOUGH_MEMORY);
    }
    ThunkResult::stdcall(r, 3)
}

fn local_size(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(0);
    let h = m.mem.process_heap();
    ThunkResult::stdcall(m.mem.heap_size(h, p).unwrap_or(0), 1)
}

/// True if every page of `[p, p+n)` is committed.
fn range_ok(m: &Machine, p: u32, n: u32) -> bool {
    if n == 0 {
        return true;
    }
    if p < layout_null_end() {
        return false;
    }
    let Some(end) = p.checked_add(n - 1) else { return false };
    let mut a = p & !0xFFF;
    loop {
        if m.mem.virtual_query(a).state != s3_mem::MEM_COMMIT {
            return false;
        }
        match a.checked_add(0x1000) {
            Some(next) if next <= end => a = next,
            _ => return true,
        }
    }
}

fn layout_null_end() -> u32 {
    s3_mem::layout::NULL_GUARD_END
}

fn is_bad_read_ptr(m: &mut Machine) -> ThunkResult {
    let (p, n) = (m.arg_u32(0), m.arg_u32(1));
    ThunkResult::stdcall(!range_ok(m, p, n) as u32, 2)
}

fn is_bad_code_ptr(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(0);
    ThunkResult::stdcall(!range_ok(m, p, 1) as u32, 1)
}

fn is_bad_string_ptr_a(m: &mut Machine) -> ThunkResult {
    let (p, max) = (m.arg_u32(0), m.arg_u32(1));
    if max == 0 {
        return ThunkResult::stdcall(0, 2);
    }
    if !range_ok(m, p, 1) {
        return ThunkResult::stdcall(1, 2);
    }
    let n = m.mem.strlen(p, max.min(0x100_0000) as usize) as u32;
    ThunkResult::stdcall(!range_ok(m, p, (n + 1).min(max)) as u32, 2)
}

// ================================================================================================ errors / debug

#[derive(Default)]
struct ErrState {
    filter: u32,
    error_mode: u32,
    debug_lines: u32,
}

fn set_unhandled_exception_filter(m: &mut Machine) -> ThunkResult {
    let f = m.arg_u32(0);
    let st = m.state::<ErrState>();
    let prev = std::mem::replace(&mut st.borrow_mut().filter, f);
    ThunkResult::stdcall(prev, 1)
}

/// `UnhandledExceptionFilter(EXCEPTION_POINTERS*)`: called by the CRT's top-level `__except` filter. Logs the
/// exception, runs the filter set with SetUnhandledExceptionFilter (if any) and otherwise returns
/// EXCEPTION_EXECUTE_HANDLER (1), after which the CRT terminates the process.
fn unhandled_exception_filter(m: &mut Machine) -> ThunkResult {
    let ptrs = m.arg_u32(0);
    let rec = m.mem.read_u32(ptrs);
    let (code, addr) = (m.mem.read_u32(rec), m.mem.read_u32(rec + 12));
    m.log(LogLevel::Error, &format!("UnhandledExceptionFilter: exception {code:#x} at {addr:#x} (thread {})", m.cur));
    let filter = m.state::<ErrState>().borrow_mut().filter;
    if filter != 0 {
        let r = m.call_guest(filter, &[ptrs]);
        if r != 0 {
            return ThunkResult::stdcall(r, 1);
        }
    }
    ThunkResult::stdcall(1, 1)
}

fn set_error_mode(m: &mut Machine) -> ThunkResult {
    let mode = m.arg_u32(0);
    let st = m.state::<ErrState>();
    let prev = std::mem::replace(&mut st.borrow_mut().error_mode, mode);
    ThunkResult::stdcall(prev, 1)
}

/// Guest debug output -> host log (first 2000 lines at Info, then Debug).
fn output_debug_string_a(m: &mut Machine) -> ThunkResult {
    let s = m.arg_str(0).unwrap_or_default();
    let st = m.state::<ErrState>();
    let n = {
        let mut g = st.borrow_mut();
        g.debug_lines += 1;
        g.debug_lines
    };
    let level = if n <= 2000 { LogLevel::Info } else { LogLevel::Debug };
    m.log(level, &format!("[guest t{}] {}", m.cur, s.trim_end()));
    ThunkResult::stdcall(0, 1)
}

/// `FormatMessageA(flags, src, id, lang, buf, size, args)`. System messages are a generic
/// "Error N." text; `FORMAT_MESSAGE_FROM_STRING` copies the string without processing inserts (HYPOTHESIS:
/// the game only formats system error codes for its error dialogs/logs).
fn format_message_a(m: &mut Machine) -> ThunkResult {
    let (flags, src, id, buf, size) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2), m.arg_u32(4), m.arg_u32(5));
    let text = if flags & 0x400 != 0 && src != 0 {
        m.mem.read_cstr(src)
    } else {
        let t = match id {
            0 => "The operation completed successfully.",
            2 => "The system cannot find the file specified.",
            3 => "The system cannot find the path specified.",
            5 => "Access is denied.",
            6 => "The handle is invalid.",
            8 => "Not enough storage is available to process this command.",
            87 => "The parameter is incorrect.",
            _ => "",
        };
        if t.is_empty() {
            format!("Error {id} ({id:#x}).\r\n")
        } else {
            format!("{t}\r\n")
        }
    };
    let len = text.len() as u32;
    if flags & 0x100 != 0 {
        // FORMAT_MESSAGE_ALLOCATE_BUFFER: *buf = LocalAlloc'ed string
        let h = m.mem.process_heap();
        let p = m.mem.heap_alloc(h, (len + 1).max(size), 0).unwrap_or(0);
        if p == 0 {
            return ThunkResult::stdcall(0, 7);
        }
        m.mem.write_cstr(p, &text);
        m.mem.write_u32(buf, p);
        return ThunkResult::stdcall(len, 7);
    }
    if size <= len {
        m.set_last_error(122);
        return ThunkResult::stdcall(0, 7);
    }
    m.mem.write_cstr(buf, &text);
    ThunkResult::stdcall(len, 7)
}

// ================================================================================================ resources

fn res_module(m: &Machine, hmod: u32) -> u32 {
    if hmod == 0 {
        m.process.image_base
    } else {
        hmod
    }
}

/// `FindResourceA(hmod, name, type)` -> HRSRC (address of the IMAGE_RESOURCE_DATA_ENTRY).
fn find_resource_a(m: &mut Machine) -> ThunkResult {
    let base = res_module(m, m.arg_u32(0));
    let name = ResId::from_guest_a(m, m.arg_u32(1));
    let ty = ResId::from_guest_a(m, m.arg_u32(2));
    if base != m.process.image_base {
        m.set_last_error(1813); // ERROR_RESOURCE_TYPE_NOT_FOUND (HLE modules have no resources)
        return ThunkResult::stdcall(0, 3);
    }
    match find_resource_entry(m, base, &ty, &name, None) {
        Some(e) => ThunkResult::stdcall(e, 3),
        None => {
            m.set_last_error(1814); // ERROR_RESOURCE_NAME_NOT_FOUND
            ThunkResult::stdcall(0, 3)
        }
    }
}

fn load_resource(m: &mut Machine) -> ThunkResult {
    let (hmod, hres) = (m.arg_u32(0), m.arg_u32(1));
    if hres == 0 {
        return ThunkResult::stdcall(0, 2);
    }
    let base = res_module(m, hmod);
    ThunkResult::stdcall(base + m.mem.read_u32(hres), 2)
}

fn sizeof_resource(m: &mut Machine) -> ThunkResult {
    let hres = m.arg_u32(1);
    if hres == 0 {
        return ThunkResult::stdcall(0, 2);
    }
    ThunkResult::stdcall(m.mem.read_u32(hres + 4), 2)
}
