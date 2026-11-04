//! Host mapping primitives (Linux / Android; any Unix with MAP_NORESERVE semantics).

use crate::MemError;

fn errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

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
    let hp = host_page_size();
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
        if libc::madvise(a as *mut libc::c_void, b - a, libc::MADV_DONTNEED) != 0 {
            let e = errno();
            std::ptr::write_bytes(a as *mut u8, 0, b - a);
            return Err(MemError::Os(e));
        }
    }
    Ok(())
}
