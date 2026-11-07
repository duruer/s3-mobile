//! Executable memory for the JIT: one fixed-size region per code arena, W^X (pages are writable only while code
//! is copied in, read+execute otherwise), instruction-cache maintenance after writes.
//!
//! - Windows: `VirtualAlloc(PAGE_READWRITE)`, `VirtualProtect` to `PAGE_EXECUTE_READ`, `FlushInstructionCache`.
//! - Unix (Linux / Android): anonymous `mmap`, `mprotect`, `dc cvau` / `ic ivau` (user-space cache maintenance).

pub(crate) struct ExecMem {
    ptr: *mut u8,
    len: usize,
    page: usize,
}

// SAFETY: the region is owned exclusively by one code arena, which moves between host threads with its CPU.
unsafe impl Send for ExecMem {}

impl ExecMem {
    /// Reserve `len` bytes (rounded up to host pages), read+execute.
    pub fn new(len: usize) -> Option<ExecMem> {
        let page = imp::page_size();
        let len = (len + page - 1) & !(page - 1);
        let ptr = imp::alloc(len)?;
        imp::protect(ptr, len, false);
        Some(ExecMem { ptr, len, page })
    }

    pub fn ptr(&self) -> *mut u8 {
        self.ptr
    }

    pub fn len(&self) -> usize {
        self.len
    }

    /// Copy `data` to offset `off`: the touched pages are made writable (not executable) for the copy, then
    /// read+execute again, then the I-cache is synchronised (W^X: code is never writable and executable at once).
    pub fn write(&mut self, off: usize, data: &[u8]) {
        assert!(off + data.len() <= self.len);
        if data.is_empty() {
            return;
        }
        let a = off & !(self.page - 1);
        let b = (off + data.len() + self.page - 1) & !(self.page - 1);
        // SAFETY: inside our region; the pages are writable during the copy.
        unsafe {
            imp::protect(self.ptr.add(a), b - a, true);
            std::ptr::copy_nonoverlapping(data.as_ptr(), self.ptr.add(off), data.len());
            imp::protect(self.ptr.add(a), b - a, false);
            imp::flush_icache(self.ptr.add(off), data.len());
        }
    }
}

impl Drop for ExecMem {
    fn drop(&mut self) {
        imp::free(self.ptr, self.len);
    }
}

#[cfg(windows)]
mod imp {
    use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
    use windows_sys::Win32::System::Memory::{
        VirtualAlloc, VirtualFree, VirtualProtect, MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_EXECUTE_READ,
        PAGE_PROTECTION_FLAGS, PAGE_READWRITE,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    pub fn page_size() -> usize {
        4096
    }

    pub fn alloc(len: usize) -> Option<*mut u8> {
        // SAFETY: new private allocation.
        let p = unsafe { VirtualAlloc(std::ptr::null(), len, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE) };
        (!p.is_null()).then_some(p as *mut u8)
    }

    pub fn protect(p: *mut u8, len: usize, writable: bool) {
        let mut old: PAGE_PROTECTION_FLAGS = 0;
        let prot = if writable { PAGE_READWRITE } else { PAGE_EXECUTE_READ };
        // SAFETY: our own allocation.
        let ok = unsafe { VirtualProtect(p as *const _, len, prot, &mut old) };
        assert!(ok != 0, "jit: VirtualProtect failed");
    }

    pub fn flush_icache(p: *mut u8, len: usize) {
        // SAFETY: our own allocation.
        unsafe {
            FlushInstructionCache(GetCurrentProcess(), p as *const _, len);
        }
    }

    pub fn free(p: *mut u8, _len: usize) {
        // SAFETY: from alloc.
        unsafe {
            VirtualFree(p as *mut _, 0, MEM_RELEASE);
        }
    }
}

#[cfg(unix)]
mod imp {
    pub fn page_size() -> usize {
        // SAFETY: sysconf is always safe to call.
        let v = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        if v <= 0 {
            4096
        } else {
            v as usize
        }
    }

    pub fn alloc(len: usize) -> Option<*mut u8> {
        // SAFETY: anonymous private mapping.
        let p = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        (p != libc::MAP_FAILED).then_some(p as *mut u8)
    }

    pub fn protect(p: *mut u8, len: usize, writable: bool) {
        let prot = if writable { libc::PROT_READ | libc::PROT_WRITE } else { libc::PROT_READ | libc::PROT_EXEC };
        // SAFETY: our own mapping.
        let r = unsafe { libc::mprotect(p as *mut _, len, prot) };
        assert!(r == 0, "jit: mprotect failed (errno {})", std::io::Error::last_os_error());
    }

    /// Clean the D-cache and invalidate the I-cache over `[p, p+len)` (what `__clear_cache` does on AArch64).
    pub fn flush_icache(p: *mut u8, len: usize) {
        let ctr: u64;
        // SAFETY: CTR_EL0 is readable from EL0 on Linux/Android; the cache maintenance instructions are allowed at
        // EL0 (SCTLR_EL1.UCI) and only touch our own mapping.
        unsafe {
            std::arch::asm!("mrs {}, ctr_el0", out(reg) ctr, options(nomem, nostack));
            let dline = 4usize << ((ctr >> 16) & 0xF);
            let iline = 4usize << (ctr & 0xF);
            let start = p as usize;
            let end = start + len;
            // IDC (bit 28): D-cache clean to PoU not required.
            if (ctr >> 28) & 1 == 0 {
                let mut a = start & !(dline - 1);
                while a < end {
                    std::arch::asm!("dc cvau, {}", in(reg) a, options(nostack));
                    a += dline;
                }
            }
            std::arch::asm!("dsb ish", options(nostack));
            // DIC (bit 29): I-cache invalidation to PoU not required.
            if (ctr >> 29) & 1 == 0 {
                let mut a = start & !(iline - 1);
                while a < end {
                    std::arch::asm!("ic ivau, {}", in(reg) a, options(nostack));
                    a += iline;
                }
                std::arch::asm!("dsb ish", options(nostack));
            }
            std::arch::asm!("isb", options(nostack));
        }
    }

    pub fn free(p: *mut u8, len: usize) {
        // SAFETY: from alloc.
        unsafe {
            libc::munmap(p as *mut _, len);
        }
    }
}
