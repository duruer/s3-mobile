//! Memory-mapped files for `CreateFileMappingA` / `MapViewOfFile`: the host file is `mmap`ed (MAP_PRIVATE |
//! MAP_FIXED) straight into the guest window, so the ~230 MB of GFX `.dat` files the game maps cost no RAM
//! until touched. Guest writes to a view stay private (copy-on-write; HYPOTHESIS: the game never writes
//! through a file view — wine-trace shows read-only mappings only).
//!
//! Pages wholly beyond EOF are left as the window's anonymous zero pages (an mmap there would SIGBUS).
//! If `mmap` fails (or on non-Unix hosts) the bytes are copied instead.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

use s3_mem::GuestMemory;

fn host_page() -> u64 {
    #[cfg(unix)]
    {
        // SAFETY: sysconf is always safe.
        let v = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        if v > 0 {
            return v as u64;
        }
    }
    4096
}

/// Map `len` bytes of `file` starting at `offset` to guest `addr` (which must be 64 KiB aligned, inside a
/// region at least `align_up(len, 64 KiB)` long). Returns the number of bytes backed by the file.
pub fn map_file_into(mem: &mut GuestMemory, addr: u32, file: &File, offset: u64, len: u32) -> std::io::Result<u32> {
    let flen = file.metadata()?.len();
    let avail = flen.saturating_sub(offset).min(len as u64);
    if avail == 0 {
        return Ok(0);
    }
    let hp = host_page();
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        if offset % hp == 0 && (mem.host_ptr(addr) as u64) % hp == 0 {
            let map_len = avail.div_ceil(hp) * hp;
            // SAFETY: the target range lies inside our own guest window reservation (MAP_FIXED replaces the
            // anonymous pages there); fd is a valid open file.
            let p = unsafe {
                libc::mmap(
                    mem.host_ptr(addr) as *mut libc::c_void,
                    map_len as usize,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_PRIVATE | libc::MAP_FIXED,
                    file.as_raw_fd(),
                    offset as libc::off_t,
                )
            };
            if p != libc::MAP_FAILED {
                // Zero the tail of the last page beyond `len` is not needed: the guest only sees `len` + the
                // file's own bytes up to the page end (Windows views also expose the rest of the page).
                return Ok(avail as u32);
            }
        }
    }
    let _ = hp;
    let mut f = file.try_clone()?;
    f.seek(SeekFrom::Start(offset))?;
    let mut buf = vec![0u8; avail as usize];
    f.read_exact(&mut buf)?;
    mem.write_bytes(addr, &buf);
    Ok(avail as u32)
}

/// Undo [`map_file_into`]: put anonymous zero pages back over `[addr, addr+len)` (rounded up to host pages).
pub fn unmap_file(mem: &mut GuestMemory, addr: u32, len: u32) {
    if len == 0 {
        return;
    }
    let hp = host_page();
    let map_len = (len as u64).div_ceil(hp) * hp;
    #[cfg(unix)]
    {
        // SAFETY: range inside our guest window; replaces whatever is mapped there with fresh anonymous memory.
        let p = unsafe {
            libc::mmap(
                mem.host_ptr(addr) as *mut libc::c_void,
                map_len as usize,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_NORESERVE | libc::MAP_FIXED,
                -1,
                0,
            )
        };
        if p != libc::MAP_FAILED {
            return;
        }
    }
    mem.fill(addr, map_len as usize, 0);
}
