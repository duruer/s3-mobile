//! kernel32: files, directories, find, file mappings, drives, std handles — all through the VFS
//! (`s3_runtime::Vfs`): the game lives at `C:\S3` (any other drive root maps there too), reads check the
//! overlay (`data_dir/overlay`) first, writes always go to the overlay, the user's game dir is never modified.
//!
//! - Read-only opens use the game file directly; opens with write access copy the file into the overlay first.
//! - `CreateFileMappingA` + `MapViewOfFile` mmap the host file into guest memory (`s3_runtime::mapfile`).
//! - Std handles are the fake values 0x10/0x14/0x18 (FILE_TYPE_CHAR); writes to them go to the log.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

use s3_mem::{align_up, layout, RegionKind};
use s3_runtime::handles::INVALID_HANDLE_VALUE;
use s3_runtime::hosttime::systemtime_to_filetime;
use s3_runtime::vfs::wildcard_match;
use s3_runtime::{KernelObject, LogLevel, Machine, Registry, ThunkResult, Vfs};

use super::DLL;

const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_PATH_NOT_FOUND: u32 = 3;
const ERROR_ACCESS_DENIED: u32 = 5;
const ERROR_INVALID_HANDLE: u32 = 6;
const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
const ERROR_NO_MORE_FILES: u32 = 18;
const ERROR_FILE_EXISTS: u32 = 80;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_NEGATIVE_SEEK: u32 = 131;
const ERROR_ALREADY_EXISTS: u32 = 183;

const GENERIC_WRITE: u32 = 0x4000_0000;
const GENERIC_ALL: u32 = 0x1000_0000;
const FILE_WRITE_DATA: u32 = 0x2;
const FILE_APPEND_DATA: u32 = 0x4;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;

const FILE_ATTRIBUTE_READONLY: u32 = 0x1;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const FILE_ATTRIBUTE_ARCHIVE: u32 = 0x20;

/// Fake std handles (STD_INPUT/OUTPUT/ERROR).
pub const STD_HANDLES: [u32; 3] = [0x10, 0x14, 0x18];

pub fn is_std_handle(h: u32) -> bool {
    STD_HANDLES.contains(&h)
}

/// An open file (`KernelObject::Other("file", ..)`).
pub struct FileObj {
    pub file: File,
    pub guest: String,
    pub host: PathBuf,
    pub writable: bool,
    pub is_dir: bool,
}

struct FindObj {
    entries: Vec<(String, bool, u64, u64)>,
    pos: usize,
}

/// A file mapping object (`KernelObject::Other("mapping", ..)`).
pub struct MappingObj {
    file: Option<File>,
    size: u64,
    /// Pagefile-backed mapping: guest memory shared by all views.
    anon_base: Option<u32>,
}

#[derive(Default)]
struct Views {
    /// view base -> (length, file-backed region we own)
    views: std::collections::BTreeMap<u32, (u32, bool)>,
}

pub(crate) fn register(reg: &mut Registry) {
    reg.register(DLL, "CreateFileA", create_file_a);
    reg.register(DLL, "ReadFile", read_file);
    reg.register(DLL, "WriteFile", write_file);
    reg.register(DLL, "SetFilePointer", set_file_pointer);
    reg.register(DLL, "GetFileSize", get_file_size);
    reg.register(DLL, "SetEndOfFile", set_end_of_file);
    reg.register(DLL, "FlushFileBuffers", flush_file_buffers);
    reg.register(DLL, "GetFileType", get_file_type);
    reg.register(DLL, "GetFileTime", get_file_time);
    reg.register(DLL, "SetFileTime", |_m| ThunkResult::stdcall(1, 4));
    reg.register(DLL, "GetStdHandle", get_std_handle);
    reg.register(DLL, "SetStdHandle", |_m| ThunkResult::stdcall(1, 2));
    reg.register(DLL, "SetHandleCount", |m| ThunkResult::stdcall(m.arg_u32(0), 1));
    reg.register(DLL, "FindFirstFileA", find_first_file_a);
    reg.register(DLL, "FindNextFileA", find_next_file_a);
    reg.register(DLL, "FindClose", find_close);
    reg.register(DLL, "DeleteFileA", delete_file_a);
    reg.register(DLL, "MoveFileA", move_file_a);
    reg.register(DLL, "CopyFileA", copy_file_a);
    reg.register(DLL, "CreateDirectoryA", create_directory_a);
    reg.register(DLL, "RemoveDirectoryA", remove_directory_a);
    reg.register(DLL, "GetFileAttributesA", get_file_attributes_a);
    reg.register(DLL, "SetFileAttributesA", set_file_attributes_a);
    reg.register(DLL, "GetFullPathNameA", get_full_path_name_a);
    reg.register(DLL, "GetCurrentDirectoryA", get_current_directory_a);
    reg.register(DLL, "SetCurrentDirectoryA", set_current_directory_a);
    reg.register(DLL, "GetDriveTypeA", |_m| ThunkResult::stdcall(3, 1)); // DRIVE_FIXED (see wine-trace.md)
    reg.register(DLL, "GetLogicalDrives", |_m| ThunkResult::stdcall(1 << 2, 0)); // C: only
    reg.register(DLL, "GetVolumeInformationA", get_volume_information_a);
    reg.register(DLL, "GetDiskFreeSpaceA", get_disk_free_space_a);
    reg.register(DLL, "GetTempPathA", get_temp_path_a);
    reg.register(DLL, "GetTempFileNameA", get_temp_file_name_a);
    reg.register(DLL, "CreateFileMappingA", create_file_mapping_a);
    reg.register(DLL, "OpenFileMappingA", open_file_mapping_a);
    reg.register(DLL, "MapViewOfFile", map_view_of_file);
    reg.register(DLL, "UnmapViewOfFile", unmap_view_of_file);
    reg.register(DLL, "FlushViewOfFile", |_m| ThunkResult::stdcall(1, 2));
}

fn fail(m: &mut Machine, err: u32, value: u32, nargs: u32) -> ThunkResult {
    m.set_last_error(err);
    ThunkResult::stdcall(value, nargs)
}

/// Does the parent directory of `guest` exist?
fn parent_exists(vfs: &Vfs, guest: &str) -> bool {
    let parts = vfs.components(guest);
    if parts.len() <= 1 {
        return true;
    }
    vfs.is_dir(&Vfs::guest_path(&parts[..parts.len() - 1]))
}

fn not_found_err(vfs: &Vfs, guest: &str) -> u32 {
    if parent_exists(vfs, guest) {
        ERROR_FILE_NOT_FOUND
    } else {
        ERROR_PATH_NOT_FOUND
    }
}

/// `CreateFileA(name, access, share, sa, disposition, flags, template)`.
fn create_file_a(m: &mut Machine) -> ThunkResult {
    let Some(name) = m.arg_str(0) else { return fail(m, ERROR_PATH_NOT_FOUND, INVALID_HANDLE_VALUE, 7) };
    let (access, disp, flags) = (m.arg_u32(1), m.arg_u32(4), m.arg_u32(5));
    if name.is_empty() {
        return fail(m, ERROR_PATH_NOT_FOUND, INVALID_HANDLE_VALUE, 7);
    }
    if name.starts_with("\\\\.\\") || name.eq_ignore_ascii_case("CON") || name.eq_ignore_ascii_case("NUL") {
        m.log(LogLevel::Warn, &format!("CreateFileA({name}): device names not supported"));
        return fail(m, ERROR_FILE_NOT_FOUND, INVALID_HANDLE_VALUE, 7);
    }
    let write = access & (GENERIC_WRITE | GENERIC_ALL | FILE_WRITE_DATA | FILE_APPEND_DATA) != 0;
    let existing = m.vfs.resolve_read(&name);
    let exists = existing.is_some();

    if let Some(p) = &existing {
        if p.is_dir() {
            if flags & FILE_FLAG_BACKUP_SEMANTICS != 0 && matches!(disp, 3 | 4) {
                if let Ok(f) = File::open(p) {
                    let obj = FileObj { file: f, guest: name.clone(), host: p.clone(), writable: false, is_dir: true };
                    let h = m.handles.insert(KernelObject::Other("file", Box::new(obj)));
                    return ThunkResult::stdcall(h, 7);
                }
            }
            return fail(m, ERROR_ACCESS_DENIED, INVALID_HANDLE_VALUE, 7);
        }
    }

    // (create/truncate?, must exist?, fail if exists?)
    let (truncate, must_exist, fail_exists) = match disp {
        1 => (true, false, true),  // CREATE_NEW
        2 => (true, false, false), // CREATE_ALWAYS
        3 => (false, true, false), // OPEN_EXISTING
        4 => (false, false, false), // OPEN_ALWAYS
        5 => (true, true, false),  // TRUNCATE_EXISTING
        _ => return fail(m, ERROR_INVALID_PARAMETER, INVALID_HANDLE_VALUE, 7),
    };
    if fail_exists && exists {
        return fail(m, ERROR_FILE_EXISTS, INVALID_HANDLE_VALUE, 7);
    }
    if must_exist && !exists {
        let e = not_found_err(&m.vfs, &name);
        return fail(m, e, INVALID_HANDLE_VALUE, 7);
    }
    if !exists && !parent_exists(&m.vfs, &name) {
        return fail(m, ERROR_PATH_NOT_FOUND, INVALID_HANDLE_VALUE, 7);
    }

    let result: std::io::Result<(File, PathBuf, bool)> = (|| {
        if truncate || !exists {
            let p = m.vfs.create_path(&name)?;
            let f = OpenOptions::new().read(true).write(true).create(true).truncate(truncate).open(&p)?;
            Ok((f, p, true))
        } else if write {
            let p = m.vfs.copy_up(&name)?;
            let f = OpenOptions::new().read(true).write(true).open(&p)?;
            Ok((f, p, true))
        } else {
            let p = existing.clone().unwrap();
            Ok((File::open(&p)?, p, false))
        }
    })();
    match result {
        Ok((file, host, writable)) => {
            if m.trace.thunks {
                m.log(LogLevel::Debug, &format!("CreateFileA({name}) -> {}", host.display()));
            }
            let obj = FileObj { file, guest: name, host, writable, is_dir: false };
            let h = m.handles.insert(KernelObject::Other("file", Box::new(obj)));
            m.set_last_error(if exists && matches!(disp, 2 | 4) { ERROR_ALREADY_EXISTS } else { 0 });
            ThunkResult::stdcall(h, 7)
        }
        Err(e) => {
            m.log(LogLevel::Warn, &format!("CreateFileA: host error {e}"));
            fail(m, ERROR_ACCESS_DENIED, INVALID_HANDLE_VALUE, 7)
        }
    }
}

/// `ReadFile(h, buf, n, *read, overlapped)`.
fn read_file(m: &mut Machine) -> ThunkResult {
    let (h, buf, n, pread) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2), m.arg_u32(3));
    if pread != 0 {
        m.mem.write_u32(pread, 0);
    }
    if is_std_handle(h) {
        return ThunkResult::stdcall(1, 5);
    }
    let Some(f) = m.handles.get_other_mut::<FileObj>(h) else { return fail(m, ERROR_INVALID_HANDLE, 0, 5) };
    if f.is_dir {
        return fail(m, ERROR_ACCESS_DENIED, 0, 5);
    }
    let dst = m.mem.slice_mut(buf, n as usize);
    let mut total = 0usize;
    while total < dst.len() {
        match f.file.read(&mut dst[total..]) {
            Ok(0) => break,
            Ok(k) => total += k,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return fail(m, ERROR_ACCESS_DENIED, 0, 5),
        }
    }
    if pread != 0 {
        m.mem.write_u32(pread, total as u32);
    }
    ThunkResult::stdcall(1, 5)
}

/// `WriteFile(h, buf, n, *written, overlapped)`.
fn write_file(m: &mut Machine) -> ThunkResult {
    let (h, buf, n, pw) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2), m.arg_u32(3));
    if is_std_handle(h) {
        let text = String::from_utf8_lossy(m.mem.slice(buf, n.min(4096) as usize)).to_string();
        m.log(LogLevel::Info, &format!("[guest stdio] {}", text.trim_end()));
        if pw != 0 {
            m.mem.write_u32(pw, n);
        }
        return ThunkResult::stdcall(1, 5);
    }
    let Some(f) = m.handles.get_other_mut::<FileObj>(h) else { return fail(m, ERROR_INVALID_HANDLE, 0, 5) };
    if !f.writable {
        return fail(m, ERROR_ACCESS_DENIED, 0, 5);
    }
    let src = m.mem.slice(buf, n as usize);
    if f.file.write_all(src).is_err() {
        return fail(m, ERROR_ACCESS_DENIED, 0, 5);
    }
    if pw != 0 {
        m.mem.write_u32(pw, n);
    }
    ThunkResult::stdcall(1, 5)
}

/// `SetFilePointer(h, lo, *hi, method)`.
fn set_file_pointer(m: &mut Machine) -> ThunkResult {
    let (h, lo, phi, method) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2), m.arg_u32(3));
    let dist: i64 = if phi != 0 { ((m.mem.read_u32(phi) as u64) << 32 | lo as u64) as i64 } else { lo as i32 as i64 };
    let Some(f) = m.handles.get_other_mut::<FileObj>(h) else { return fail(m, ERROR_INVALID_HANDLE, u32::MAX, 4) };
    let base: i64 = match method {
        0 => 0,
        1 => f.file.stream_position().unwrap_or(0) as i64,
        2 => f.file.metadata().map(|md| md.len()).unwrap_or(0) as i64,
        _ => return fail(m, ERROR_INVALID_PARAMETER, u32::MAX, 4),
    };
    let pos = base + dist;
    if pos < 0 {
        return fail(m, ERROR_NEGATIVE_SEEK, u32::MAX, 4);
    }
    if f.file.seek(SeekFrom::Start(pos as u64)).is_err() {
        return fail(m, ERROR_INVALID_PARAMETER, u32::MAX, 4);
    }
    if phi != 0 {
        m.mem.write_u32(phi, (pos as u64 >> 32) as u32);
    }
    m.set_last_error(0);
    ThunkResult::stdcall(pos as u32, 4)
}

fn get_file_size(m: &mut Machine) -> ThunkResult {
    let (h, phi) = (m.arg_u32(0), m.arg_u32(1));
    let Some(f) = m.handles.get_other::<FileObj>(h) else { return fail(m, ERROR_INVALID_HANDLE, u32::MAX, 2) };
    let len = f.file.metadata().map(|md| md.len()).unwrap_or(0);
    if phi != 0 {
        m.mem.write_u32(phi, (len >> 32) as u32);
    }
    m.set_last_error(0);
    ThunkResult::stdcall(len as u32, 2)
}

fn set_end_of_file(m: &mut Machine) -> ThunkResult {
    let h = m.arg_u32(0);
    let Some(f) = m.handles.get_other_mut::<FileObj>(h) else { return fail(m, ERROR_INVALID_HANDLE, 0, 1) };
    if !f.writable {
        return fail(m, ERROR_ACCESS_DENIED, 0, 1);
    }
    let pos = f.file.stream_position().unwrap_or(0);
    let ok = f.file.set_len(pos).is_ok();
    ThunkResult::stdcall(ok as u32, 1)
}

fn flush_file_buffers(m: &mut Machine) -> ThunkResult {
    let h = m.arg_u32(0);
    if let Some(f) = m.handles.get_other_mut::<FileObj>(h) {
        let _ = f.file.flush();
    }
    ThunkResult::stdcall(1, 1)
}

fn get_file_type(m: &mut Machine) -> ThunkResult {
    let h = m.arg_u32(0);
    if is_std_handle(h) {
        return ThunkResult::stdcall(2, 1); // FILE_TYPE_CHAR
    }
    if m.handles.get_other::<FileObj>(h).is_some() {
        return ThunkResult::stdcall(1, 1); // FILE_TYPE_DISK
    }
    fail(m, ERROR_INVALID_HANDLE, 0, 1)
}

fn get_file_time(m: &mut Machine) -> ThunkResult {
    let (h, pc, pa, pw) = (m.arg_u32(0), m.arg_u32(1), m.arg_u32(2), m.arg_u32(3));
    let Some(f) = m.handles.get_other::<FileObj>(h) else { return fail(m, ERROR_INVALID_HANDLE, 0, 4) };
    let md = f.file.metadata().ok();
    let ft = |t: Option<std::time::SystemTime>| t.map(systemtime_to_filetime).unwrap_or(0);
    let mt = ft(md.as_ref().and_then(|d| d.modified().ok()));
    let at = ft(md.as_ref().and_then(|d| d.accessed().ok()));
    for (p, v) in [(pc, mt), (pa, at), (pw, mt)] {
        if p != 0 {
            m.mem.write_u64(p, v);
        }
    }
    ThunkResult::stdcall(1, 4)
}

fn get_std_handle(m: &mut Machine) -> ThunkResult {
    let n = m.arg_u32(0) as i32;
    let h = match n {
        -10 => STD_HANDLES[0],
        -11 => STD_HANDLES[1],
        -12 => STD_HANDLES[2],
        _ => return fail(m, ERROR_INVALID_HANDLE, INVALID_HANDLE_VALUE, 1),
    };
    ThunkResult::stdcall(h, 1)
}

// ---------------------------------------------------------------------------------------------- find

/// Split a guest path into (directory, last component).
fn split_dir(path: &str) -> (String, String) {
    let p = path.replace('/', "\\");
    match p.rfind('\\') {
        Some(i) => {
            let dir = if i == 0 || (i == 2 && p.as_bytes()[1] == b':') { p[..=i].to_string() } else { p[..i].to_string() };
            (dir, p[i + 1..].to_string())
        }
        None if p.len() >= 2 && p.as_bytes()[1] == b':' => (p[..2].to_string(), p[2..].to_string()),
        None => (".".to_string(), p),
    }
}

/// WIN32_FIND_DATAA (320 bytes).
fn write_find_data(m: &mut Machine, p: u32, name: &str, is_dir: bool, size: u64, mtime: u64) {
    m.mem.fill(p, 320, 0);
    m.mem.write_u32(p, if is_dir { FILE_ATTRIBUTE_DIRECTORY } else { FILE_ATTRIBUTE_ARCHIVE });
    for off in [4, 12, 20] {
        m.mem.write_u64(p + off, mtime);
    }
    m.mem.write_u32(p + 28, (size >> 32) as u32);
    m.mem.write_u32(p + 32, size as u32);
    let n: String = name.chars().take(259).collect();
    m.mem.write_cstr(p + 44, &n);
}

/// `FindFirstFileA(pattern, WIN32_FIND_DATAA*)`. Like Windows, "." and ".." are listed first for
/// subdirectories when the pattern matches them.
fn find_first_file_a(m: &mut Machine) -> ThunkResult {
    let (pp, data) = (m.arg_u32(0), m.arg_u32(1));
    let pattern_full = m.mem.read_cstr(pp);
    let (dir, pat) = split_dir(&pattern_full);
    if pat.is_empty() || !m.vfs.is_dir(&dir) && !m.vfs.components(&dir).is_empty() {
        return fail(m, if pat.is_empty() { ERROR_FILE_NOT_FOUND } else { ERROR_PATH_NOT_FOUND }, INVALID_HANDLE_VALUE, 2);
    }
    let mut entries = Vec::new();
    for dot in [".", ".."] {
        if wildcard_match(&pat, dot) && pat.contains(['*', '?']) {
            entries.push((dot.to_string(), true, 0, 0));
        }
    }
    for e in m.vfs.list_dir(&dir) {
        if wildcard_match(&pat, &e.name) {
            let mt = std::fs::metadata(&e.host_path).and_then(|md| md.modified()).map(systemtime_to_filetime).unwrap_or(0);
            entries.push((e.name, e.is_dir, e.size, mt));
        }
    }
    if entries.is_empty() {
        return fail(m, ERROR_FILE_NOT_FOUND, INVALID_HANDLE_VALUE, 2);
    }
    let (n, d, s, t) = entries[0].clone();
    write_find_data(m, data, &n, d, s, t);
    let h = m.handles.insert(KernelObject::Other("find", Box::new(FindObj { entries, pos: 1 })));
    ThunkResult::stdcall(h, 2)
}

fn find_next_file_a(m: &mut Machine) -> ThunkResult {
    let (h, data) = (m.arg_u32(0), m.arg_u32(1));
    let Some(f) = m.handles.get_other_mut::<FindObj>(h) else { return fail(m, ERROR_INVALID_HANDLE, 0, 2) };
    let Some(e) = f.entries.get(f.pos).cloned() else { return fail(m, ERROR_NO_MORE_FILES, 0, 2) };
    f.pos += 1;
    write_find_data(m, data, &e.0, e.1, e.2, e.3);
    ThunkResult::stdcall(1, 2)
}

fn find_close(m: &mut Machine) -> ThunkResult {
    let h = m.arg_u32(0);
    if m.handles.get_other::<FindObj>(h).is_none() {
        return fail(m, ERROR_INVALID_HANDLE, 0, 1);
    }
    let _ = m.handles.close(h);
    ThunkResult::stdcall(1, 1)
}

// ---------------------------------------------------------------------------------------------- file system ops

fn vfs_result(m: &mut Machine, r: Result<(), u32>, nargs: u32) -> ThunkResult {
    match r {
        Ok(()) => ThunkResult::stdcall(1, nargs),
        Err(e) => fail(m, e, 0, nargs),
    }
}

fn delete_file_a(m: &mut Machine) -> ThunkResult {
    let name = m.arg_str(0).unwrap_or_default();
    let r = m.vfs.remove_file(&name);
    vfs_result(m, r, 1)
}

fn move_file_a(m: &mut Machine) -> ThunkResult {
    let (a, b) = (m.arg_str(0).unwrap_or_default(), m.arg_str(1).unwrap_or_default());
    let r = m.vfs.rename(&a, &b);
    vfs_result(m, r, 2)
}

fn copy_file_a(m: &mut Machine) -> ThunkResult {
    let (a, b, fx) = (m.arg_str(0).unwrap_or_default(), m.arg_str(1).unwrap_or_default(), m.arg_u32(2) != 0);
    let r = m.vfs.copy(&a, &b, fx);
    vfs_result(m, r, 3)
}

fn create_directory_a(m: &mut Machine) -> ThunkResult {
    let a = m.arg_str(0).unwrap_or_default();
    let r = m.vfs.create_dir(&a);
    vfs_result(m, r, 2)
}

fn remove_directory_a(m: &mut Machine) -> ThunkResult {
    let a = m.arg_str(0).unwrap_or_default();
    let r = m.vfs.remove_dir(&a);
    vfs_result(m, r, 1)
}

fn get_file_attributes_a(m: &mut Machine) -> ThunkResult {
    let name = m.arg_str(0).unwrap_or_default();
    if m.vfs.components(&name).is_empty() {
        return ThunkResult::stdcall(FILE_ATTRIBUTE_DIRECTORY, 1);
    }
    match m.vfs.resolve_read(&name) {
        Some(p) => {
            let a = if p.is_dir() { FILE_ATTRIBUTE_DIRECTORY } else { FILE_ATTRIBUTE_ARCHIVE };
            ThunkResult::stdcall(a, 1)
        }
        None => {
            let e = not_found_err(&m.vfs, &name);
            fail(m, e, u32::MAX, 1)
        }
    }
}

/// Attributes are not stored (READONLY etc. are accepted and ignored).
fn set_file_attributes_a(m: &mut Machine) -> ThunkResult {
    let name = m.arg_str(0).unwrap_or_default();
    let attrs = m.arg_u32(1);
    if m.vfs.resolve_read(&name).is_none() {
        let e = not_found_err(&m.vfs, &name);
        return fail(m, e, 0, 2);
    }
    if attrs & FILE_ATTRIBUTE_READONLY != 0 {
        m.log(LogLevel::Debug, &format!("SetFileAttributesA({name}, {attrs:#x}): attributes not stored"));
    }
    ThunkResult::stdcall(1, 2)
}

/// Write `s` + NUL into a buffer of `size` chars; returns the Win32 "length or required size" value.
fn put_path(m: &mut Machine, buf: u32, size: u32, s: &str) -> u32 {
    let len = s.len() as u32;
    if buf != 0 && size > len {
        m.mem.write_cstr(buf, s);
        len
    } else {
        len + 1
    }
}

/// `GetFullPathNameA(name, size, buf, *filepart)`: normalized absolute guest path (always under `C:\S3`).
fn get_full_path_name_a(m: &mut Machine) -> ThunkResult {
    let (size, buf, pfile) = (m.arg_u32(1), m.arg_u32(2), m.arg_u32(3));
    let name = m.arg_str(0).unwrap_or_default();
    let parts = m.vfs.components(&name);
    let mut full = Vfs::guest_path(&parts);
    if (name.ends_with('\\') || name.ends_with('/')) && !full.ends_with('\\') {
        full.push('\\');
    }
    let r = put_path(m, buf, size, &full);
    if pfile != 0 && buf != 0 && r < size {
        let file_off = full.rfind('\\').map(|i| i as u32 + 1).unwrap_or(0);
        m.mem.write_u32(pfile, if file_off as usize >= full.len() { 0 } else { buf + file_off });
    }
    ThunkResult::stdcall(r, 4)
}

fn get_current_directory_a(m: &mut Machine) -> ThunkResult {
    let (size, buf) = (m.arg_u32(0), m.arg_u32(1));
    let cwd = m.vfs.cwd.clone();
    let r = put_path(m, buf, size, &cwd);
    ThunkResult::stdcall(r, 2)
}

fn set_current_directory_a(m: &mut Machine) -> ThunkResult {
    let name = m.arg_str(0).unwrap_or_default();
    if m.vfs.set_cwd(&name) {
        ThunkResult::stdcall(1, 1)
    } else {
        let e = not_found_err(&m.vfs, &name);
        fail(m, e, 0, 1)
    }
}

/// `GetVolumeInformationA(root, name, nsize, *serial, *maxcomp, *flags, fsname, fssize)`.
fn get_volume_information_a(m: &mut Machine) -> ThunkResult {
    let (name, nsize, ser, maxc, fl, fs, fssize) =
        (m.arg_u32(1), m.arg_u32(2), m.arg_u32(3), m.arg_u32(4), m.arg_u32(5), m.arg_u32(6), m.arg_u32(7));
    if name != 0 && nsize > 2 {
        m.mem.write_cstr(name, "S3");
    }
    if ser != 0 {
        m.mem.write_u32(ser, 0x5333_0001);
    }
    if maxc != 0 {
        m.mem.write_u32(maxc, 255);
    }
    if fl != 0 {
        m.mem.write_u32(fl, 0x3); // case-preserved, case-sensitive search
    }
    if fs != 0 && fssize > 4 {
        m.mem.write_cstr(fs, "FAT32");
    }
    ThunkResult::stdcall(1, 8)
}

/// `GetDiskFreeSpaceA(root, *spc, *bps, *free, *total)`: 1 GiB free of 2 GiB (fits 32-bit math).
fn get_disk_free_space_a(m: &mut Machine) -> ThunkResult {
    let vals = [8u32, 512, 262_144, 524_288];
    for (i, v) in vals.iter().enumerate() {
        let p = m.arg_u32(1 + i as u32);
        if p != 0 {
            m.mem.write_u32(p, *v);
        }
    }
    ThunkResult::stdcall(1, 5)
}

const TEMP_DIR: &str = "C:\\S3\\TEMP\\";

fn get_temp_path_a(m: &mut Machine) -> ThunkResult {
    let (size, buf) = (m.arg_u32(0), m.arg_u32(1));
    let _ = m.vfs.resolve_write(&format!("{TEMP_DIR}x"));
    let r = put_path(m, buf, size, TEMP_DIR);
    ThunkResult::stdcall(r, 2)
}

/// `GetTempFileNameA(dir, prefix, unique, buf)`.
fn get_temp_file_name_a(m: &mut Machine) -> ThunkResult {
    let (unique, buf) = (m.arg_u32(2), m.arg_u32(3));
    let dir = m.arg_str(0).unwrap_or_else(|| ".".into());
    let prefix: String = m.arg_str(1).unwrap_or_default().chars().take(3).collect();
    let dir = dir.trim_end_matches(['\\', '/']).to_string();
    let mut n = if unique != 0 { unique & 0xFFFF } else { (m.now_ms() as u32 & 0xFFFF).max(1) };
    let path = loop {
        let p = format!("{dir}\\{prefix}{n:04X}.TMP");
        if unique != 0 || m.vfs.resolve_read(&p).is_none() {
            break p;
        }
        n = (n + 1) & 0xFFFF;
    };
    if unique == 0 {
        if let Ok(hp) = m.vfs.create_path(&path) {
            let _ = File::create(hp);
        }
    }
    m.mem.write_cstr(buf, &path);
    ThunkResult::stdcall(n, 4)
}

// ---------------------------------------------------------------------------------------------- mappings

/// `CreateFileMappingA(hFile, sa, protect, max_hi, max_lo, name)`.
fn create_file_mapping_a(m: &mut Machine) -> ThunkResult {
    let (hf, hi, lo, name) = (m.arg_u32(0), m.arg_u32(3), m.arg_u32(4), m.arg_str(5));
    if let Some(n) = &name {
        if let Some(h) = m.handles.open_named(n) {
            m.set_last_error(ERROR_ALREADY_EXISTS);
            return ThunkResult::stdcall(h, 6);
        }
    }
    let max = (hi as u64) << 32 | lo as u64;
    let obj = if hf == INVALID_HANDLE_VALUE {
        if max == 0 {
            return fail(m, ERROR_INVALID_PARAMETER, 0, 6);
        }
        MappingObj { file: None, size: max, anon_base: None }
    } else {
        let Some(f) = m.handles.get_other::<FileObj>(hf) else { return fail(m, ERROR_INVALID_HANDLE, 0, 6) };
        let len = f.file.metadata().map(|md| md.len()).unwrap_or(0);
        let Ok(file) = f.file.try_clone() else { return fail(m, ERROR_ACCESS_DENIED, 0, 6) };
        if max > len {
            // Windows would grow the file; we do not (HYPOTHESIS: the game only maps existing data files).
            m.log(LogLevel::Warn, &format!("CreateFileMappingA: size {max} > file size {len}; not growing"));
        }
        let size = if max == 0 { len } else { max };
        if size == 0 {
            return fail(m, 1006, 0, 6); // ERROR_FILE_INVALID (empty file)
        }
        MappingObj { file: Some(file), size, anon_base: None }
    };
    m.set_last_error(0);
    let h = m.handles.insert_named(KernelObject::Other("mapping", Box::new(obj)), name);
    ThunkResult::stdcall(h, 6)
}

fn open_file_mapping_a(m: &mut Machine) -> ThunkResult {
    let h = m.arg_str(2).and_then(|n| m.handles.open_named(&n)).unwrap_or(0);
    if h == 0 {
        m.set_last_error(ERROR_FILE_NOT_FOUND);
    }
    ThunkResult::stdcall(h, 3)
}

/// `MapViewOfFile(hMap, access, off_hi, off_lo, size)`.
fn map_view_of_file(m: &mut Machine) -> ThunkResult {
    let (hm, ohi, olo, n) = (m.arg_u32(0), m.arg_u32(2), m.arg_u32(3), m.arg_u32(4));
    let off = (ohi as u64) << 32 | olo as u64;
    if off % layout::ALLOC_GRANULARITY as u64 != 0 {
        return fail(m, ERROR_INVALID_PARAMETER /* MAPPED_ALIGNMENT */, 0, 5);
    }
    let Some(mo) = m.handles.get_other::<MappingObj>(hm) else { return fail(m, ERROR_INVALID_HANDLE, 0, 5) };
    if off >= mo.size {
        return fail(m, ERROR_ACCESS_DENIED, 0, 5);
    }
    let len = if n == 0 { (mo.size - off).min(u32::MAX as u64) as u32 } else { n };
    if off + len as u64 > mo.size {
        return fail(m, ERROR_ACCESS_DENIED, 0, 5);
    }
    let anon = mo.file.is_none();
    if anon {
        // Pagefile-backed: one shared block; views alias it.
        let size = mo.size;
        let base = match mo.anon_base {
            Some(b) => b,
            None => {
                let Ok(b) = m.mem.alloc_region(
                    None,
                    align_up(size as u32, layout::ALLOC_GRANULARITY),
                    RegionKind::Mapped,
                    (layout::ALLOC_ARENA_START, layout::ALLOC_ARENA_END),
                ) else {
                    return fail(m, ERROR_NOT_ENOUGH_MEMORY, 0, 5);
                };
                if let Some(mo) = m.handles.get_other_mut::<MappingObj>(hm) {
                    mo.anon_base = Some(b);
                }
                b
            }
        };
        let v = base + off as u32;
        let st = m.state::<Views>();
        st.borrow_mut().views.insert(v, (len, false));
        return ThunkResult::stdcall(v, 5);
    }
    let Ok(file) = mo.file.as_ref().unwrap().try_clone() else { return fail(m, ERROR_ACCESS_DENIED, 0, 5) };
    let Ok(base) = m.mem.alloc_region(
        None,
        align_up(len, layout::ALLOC_GRANULARITY),
        RegionKind::Mapped,
        (layout::ALLOC_ARENA_START, layout::ALLOC_ARENA_END),
    ) else {
        return fail(m, ERROR_NOT_ENOUGH_MEMORY, 0, 5);
    };
    if let Err(e) = s3_runtime::mapfile::map_file_into(&mut m.mem, base, &file, off, len) {
        m.log(LogLevel::Warn, &format!("MapViewOfFile: {e}"));
        let _ = m.mem.virtual_free(base, 0, s3_mem::MEM_RELEASE);
        return fail(m, ERROR_ACCESS_DENIED, 0, 5);
    }
    let st = m.state::<Views>();
    st.borrow_mut().views.insert(base, (len, true));
    ThunkResult::stdcall(base, 5)
}

fn unmap_view_of_file(m: &mut Machine) -> ThunkResult {
    let p = m.arg_u32(0);
    let st = m.state::<Views>();
    let v = st.borrow_mut().views.remove(&p);
    match v {
        Some((len, true)) => {
            s3_runtime::mapfile::unmap_file(&mut m.mem, p, len);
            let _ = m.mem.virtual_free(p, 0, s3_mem::MEM_RELEASE);
            ThunkResult::stdcall(1, 1)
        }
        Some((_, false)) => ThunkResult::stdcall(1, 1),
        None => fail(m, 487 /* ERROR_INVALID_ADDRESS */, 0, 1),
    }
}

#[cfg(test)]
mod tests {
    use super::split_dir;

    #[test]
    fn split() {
        assert_eq!(split_dir("C:\\S3\\Map\\*.map"), ("C:\\S3\\Map".into(), "*.map".into()));
        assert_eq!(split_dir("*.*"), (".".into(), "*.*".into()));
        assert_eq!(split_dir("C:\\*.*"), ("C:\\".into(), "*.*".into()));
        assert_eq!(split_dir("\\x"), ("\\".into(), "x".into()));
    }
}
