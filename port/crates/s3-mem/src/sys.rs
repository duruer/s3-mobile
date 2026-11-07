//! Host mapping primitives.
//!
//! - Unix (Linux / Android; any Unix with MAP_NORESERVE semantics): anonymous private `mmap`.
//! - Windows: `VirtualAlloc`. Windows has no MAP_NORESERVE, so the whole window is committed up front. Pages are still
//!   demand-zero (no physical memory until touched), but the full size counts against the system commit limit
//!   (RAM + page file).

pub(crate) use imp::{discard, release_window, reserve_window};

/// Zero `[p, p+len)`: whole host pages in the interior go through `give_back` (returns them to the OS and leaves them
/// zero), the unaligned edges are zeroed by hand. Shared by both platforms.
fn discard_with(
    p: *mut u8,
    len: usize,
    hp: usize,
    give_back: impl FnOnce(usize, usize) -> Result<(), crate::MemError>,
) -> Result<(), crate::MemError> {
    let start = p as usize;
    let end = start + len;
    let a = (start + hp - 1) & !(hp - 1);
    let b = end & !(hp - 1);
    if a >= b {
        // SAFETY: inside our mapping.
        unsafe { std::ptr::write_bytes(p, 0, len) };
        return Ok(());
    }
    // SAFETY: all ranges are inside our private anonymous mapping.
    unsafe {
        std::ptr::write_bytes(p, 0, a - start);
        std::ptr::write_bytes(b as *mut u8, 0, end - b);
    }
    give_back(a, b - a).inspect_err(|_| {
        // SAFETY: inside our mapping; fall back to zeroing by hand.
        unsafe { std::ptr::write_bytes(a as *mut u8, 0, b - a) };
    })
}

fn errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

#[cfg(unix)]
mod imp {
    use super::errno;
    use crate::MemError;

    /// Reserve `len` bytes readable/writable, lazily backed, not counted against commit limits.
    pub(crate) fn reserve_window(len: usize) -> Result<*mut u8, MemError> {
        // SAFETY: anonymous private mapping at a kernel-chosen address.
        let p = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE,
                -1,
                0,
            )
        };
        if p == libc::MAP_FAILED {
            return Err(MemError::Os(errno()));
        }
        Ok(p as *mut u8)
    }

    pub(crate) fn release_window(p: *mut u8, len: usize) {
        // SAFETY: p/len come from reserve_window.
        unsafe {
            libc::munmap(p as *mut libc::c_void, len);
        }
    }

    fn host_page_size() -> usize {
        // SAFETY: sysconf is always safe to call.
        let v = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        if v <= 0 {
            4096
        } else {
            v as usize
        }
    }

    /// Zero a range, giving whole host pages back to the kernel (host pages may be 16 KiB on Android, so only the
    /// host-page-aligned interior is madvised; the edges are zeroed by hand).
    pub(crate) fn discard(p: *mut u8, len: usize) -> Result<(), MemError> {
        super::discard_with(p, len, host_page_size(), |a, n| {
            // SAFETY: page-aligned range inside our private anonymous mapping.
            if unsafe { libc::madvise(a as *mut libc::c_void, n, libc::MADV_DONTNEED) } != 0 {
                return Err(MemError::Os(errno()));
            }
            Ok(())
        })
    }
}

#[cfg(windows)]
mod imp {
    use super::errno;
    use crate::MemError;
    use windows_sys::Win32::System::Memory::{
        VirtualAlloc, VirtualFree, MEM_COMMIT, MEM_DECOMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE,
    };
    use windows_sys::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};

    /// Reserve and commit `len` bytes readable/writable. Pages are demand-zero, but (unlike MAP_NORESERVE on Unix)
    /// the full size is charged against the commit limit.
    pub(crate) fn reserve_window(len: usize) -> Result<*mut u8, MemError> {
        // SAFETY: new private allocation at a system-chosen address.
        let p = unsafe { VirtualAlloc(std::ptr::null(), len, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE) };
        if p.is_null() {
            return Err(MemError::Os(errno()));
        }
        Ok(p as *mut u8)
    }

    pub(crate) fn release_window(p: *mut u8, _len: usize) {
        // SAFETY: p comes from reserve_window; MEM_RELEASE requires size 0 and frees the whole allocation.
        unsafe {
            VirtualFree(p as *mut core::ffi::c_void, 0, MEM_RELEASE);
        }
    }

    fn host_page_size() -> usize {
        // SAFETY: GetSystemInfo only writes the struct we pass.
        let mut si: SYSTEM_INFO = unsafe { std::mem::zeroed() };
        unsafe { GetSystemInfo(&mut si) };
        if si.dwPageSize == 0 {
            4096
        } else {
            si.dwPageSize as usize
        }
    }

    /// Zero a range. The page-aligned interior is decommitted (physical pages go back to the OS) and committed again,
    /// which leaves it demand-zero, the same effect as `madvise(MADV_DONTNEED)` on a private anonymous mapping.
    pub(crate) fn discard(p: *mut u8, len: usize) -> Result<(), MemError> {
        super::discard_with(p, len, host_page_size(), |a, n| {
            // SAFETY: page-aligned range inside our committed allocation.
            unsafe {
                if VirtualFree(a as *mut core::ffi::c_void, n, MEM_DECOMMIT) == 0 {
                    return Err(MemError::Os(errno()));
                }
                if VirtualAlloc(a as *const core::ffi::c_void, n, MEM_COMMIT, PAGE_READWRITE).is_null() {
                    // The range is decommitted now; this must not be left that way. Commit failure here means the
                    // system ran out of commit, which the caller cannot recover from either.
                    panic!("s3-mem: re-commit after discard failed (os error {})", errno());
                }
            }
            Ok(())
        })
    }
}
