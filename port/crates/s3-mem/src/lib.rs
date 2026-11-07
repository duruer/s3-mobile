//! Guest 32-bit address space for the Settlers III port.
//!
//! One 4 GiB (+ guard) host reservation backs the whole guest address space: guest address `g` lives at host
//! address `base + g`. The reservation is `PROT_READ|PROT_WRITE`, `MAP_PRIVATE|MAP_ANONYMOUS|MAP_NORESERVE`,
//! so untouched pages cost nothing and every guest access is a plain host load/store (no fault handling).
//!
//! On top of the raw window this crate keeps *bookkeeping* for Win32 semantics:
//! - [`Regions`]: VirtualAlloc/VirtualFree/VirtualProtect/VirtualQuery (64 KiB allocation granularity).
//! - [`Heaps`]: HeapCreate/HeapAlloc/HeapFree/HeapReAlloc/HeapSize with metadata kept on the host side
//!   (guest buffer overruns cannot corrupt the allocator).
//!
//! The address map is in [`layout`]; see `docs/porting/architecture.md`.

mod heap;
mod regions;
mod sys;

pub use heap::{GuestHeap, Heaps, HEAP_REALLOC_IN_PLACE_ONLY, HEAP_ZERO_MEMORY};
pub use regions::{
    Allocation, MemoryInfo, RegionKind, Regions, MEM_COMMIT, MEM_DECOMMIT, MEM_FREE, MEM_IMAGE, MEM_MAPPED,
    MEM_PRIVATE, MEM_RELEASE, MEM_RESERVE, PAGE_EXECUTE, PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE, PAGE_NOACCESS,
    PAGE_READONLY, PAGE_READWRITE,
};

use std::fmt;

/// Guest address map. All values are guest virtual addresses.
pub mod layout {
    /// Size of the guest window.
    pub const WINDOW_SIZE: u64 = 1 << 32;
    /// Extra host bytes mapped after the window so that `addr + 16` never leaves the mapping.
    pub const GUARD_SIZE: u64 = 1 << 16;
    /// Page size.
    pub const PAGE_SIZE: u32 = 0x1000;
    /// VirtualAlloc allocation granularity.
    pub const ALLOC_GRANULARITY: u32 = 0x1_0000;
    /// Accesses below this address are treated as NULL-pointer faults by the CPU.
    pub const NULL_GUARD_END: u32 = 0x0001_0000;
    /// Low arena: process parameters, environment block, misc runtime data.
    pub const LOW_ARENA_START: u32 = 0x0001_0000;
    pub const LOW_ARENA_END: u32 = 0x0040_0000;
    /// S3.EXE image (fixed base, no relocations).
    pub const IMAGE_BASE: u32 = 0x0040_0000;
    pub const IMAGE_END: u32 = 0x007E_D000;
    /// VirtualAlloc / heap arena (anything without a fixed address).
    pub const ALLOC_ARENA_START: u32 = 0x1000_0000;
    pub const ALLOC_ARENA_END: u32 = 0x7000_0000;
    /// Thread stacks.
    pub const STACK_ARENA_START: u32 = 0x7000_0000;
    pub const STACK_ARENA_END: u32 = 0x7F00_0000;
    /// System area: TEBs (one page each, growing down from `TEB_TOP`) and the PEB.
    pub const SYSTEM_AREA_START: u32 = 0x7F00_0000;
    pub const SYSTEM_AREA_END: u32 = 0x7FFF_0000;
    /// PEB address (Windows-like).
    pub const PEB_ADDR: u32 = 0x7FFD_F000;
    /// TEB of thread slot 0; slot `i` is at `TEB_TOP - i * PAGE_SIZE`.
    pub const TEB_TOP: u32 = 0x7FFD_E000;
    /// Fake module handles for HLE DLLs (`ddraw.dll`, `kernel32.dll`, ...): `FAKE_MODULE_BASE + i * 0x10000`.
    pub const FAKE_MODULE_BASE: u32 = 0x7800_0000;
    /// Every HLE function is one address in `[THUNK_BASE, THUNK_END)`. The CPU exits when EIP enters it.
    pub const THUNK_BASE: u32 = 0xFFF0_0000;
    pub const THUNK_END: u32 = 0xFFFF_F000;
    /// Return address pushed by host->guest calls; the CPU exits with `ReturnMagic` when EIP reaches it.
    pub const MAGIC_RETURN: u32 = 0xFFFF_F000;
}

/// Errors from guest memory operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemError {
    /// Range overlaps an existing allocation.
    Overlap { addr: u32, len: u32 },
    /// Range is not (fully) inside an allocation / not committed.
    NotAllocated { addr: u32, len: u32 },
    /// Bad parameters (zero size, wrap-around, misaligned base for release, ...).
    BadRange { addr: u32, len: u32 },
    /// No free address space left in the arena.
    OutOfSpace,
    /// Unknown heap handle.
    BadHeap(u32),
    /// Host OS error (errno).
    Os(i32),
}

impl fmt::Display for MemError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MemError::Overlap { addr, len } => write!(f, "guest range {addr:#x}+{len:#x} overlaps an allocation"),
            MemError::NotAllocated { addr, len } => write!(f, "guest range {addr:#x}+{len:#x} is not allocated"),
            MemError::BadRange { addr, len } => write!(f, "invalid guest range {addr:#x}+{len:#x}"),
            MemError::OutOfSpace => write!(f, "guest address space exhausted"),
            MemError::BadHeap(h) => write!(f, "unknown heap handle {h:#x}"),
            MemError::Os(e) => write!(f, "host error {e}"),
        }
    }
}

impl std::error::Error for MemError {}

/// Round `v` up to a multiple of `align` (power of two). Saturates at `u32::MAX & !(align-1)`.
#[inline]
pub fn align_up(v: u32, align: u32) -> u32 {
    let a = align - 1;
    match v.checked_add(a) {
        Some(x) => x & !a,
        None => u32::MAX & !a,
    }
}

/// Round `v` down to a multiple of `align` (power of two).
#[inline]
pub fn align_down(v: u32, align: u32) -> u32 {
    v & !(align - 1)
}

/// The guest address space.
///
/// Reads take `&self`, writes take `&mut self`. All accessors accept any `u32` address: the whole window is
/// mapped, so there is no failure path (unallocated memory reads as zero). Wrap-around (`addr + len > 2^32`)
/// panics for slices.
pub struct GuestMemory {
    base: *mut u8,
    /// VirtualAlloc bookkeeping.
    pub regions: Regions,
    /// Guest heaps (HeapCreate & co).
    pub heaps: Heaps,
    process_heap: u32,
    /// False for an alias view ([`GuestMemory::alias_view`]): it does not release the window on drop.
    owns_window: bool,
}

// The raw pointer is only an address; `GuestMemory` owns its mapping exclusively.
unsafe impl Send for GuestMemory {}

impl fmt::Debug for GuestMemory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GuestMemory").field("base", &self.base).finish()
    }
}

macro_rules! rw_num {
    ($read:ident, $write:ident, $t:ty) => {
        #[inline(always)]
        pub fn $read(&self, addr: u32) -> $t {
            // SAFETY: the window is 4 GiB + guard and addr < 2^32, so addr + size_of::<$t>() is in bounds.
            unsafe { <$t>::from_le_bytes(std::ptr::read_unaligned(self.base.add(addr as usize) as *const _)) }
        }
        #[inline(always)]
        pub fn $write(&mut self, addr: u32, v: $t) {
            // SAFETY: see read.
            unsafe { std::ptr::write_unaligned(self.base.add(addr as usize) as *mut _, v.to_le_bytes()) }
        }
    };
}

impl GuestMemory {
    /// Reserve the 4 GiB window and create the process heap.
    pub fn new() -> Result<Self, MemError> {
        let base = sys::reserve_window((layout::WINDOW_SIZE + layout::GUARD_SIZE) as usize)?;
        let mut m =
            GuestMemory { base, regions: Regions::new(), heaps: Heaps::new(), process_heap: 0, owns_window: true };
        m.process_heap = m.heap_create(0x10_0000, 0)?;
        Ok(m)
    }

    /// A second handle on the same guest window for a CPU running on another host thread (multi-core mode,
    /// docs/porting/multicore.md): same base pointer, **empty** `regions`/`heaps` (the bookkeeping stays with the
    /// owner), and it never unmaps the window.
    ///
    /// # Safety
    /// The view must not outlive `self`, and it may only be used for plain guest loads/stores (what `Cpu::run`
    /// does). Concurrent accesses through both handles race exactly like guest threads on a real multi-core PC.
    pub unsafe fn alias_view(&self) -> GuestMemory {
        GuestMemory { base: self.base, regions: Regions::new(), heaps: Heaps::new(), process_heap: 0, owns_window: false }
    }

    /// Host pointer of guest address 0.
    pub fn base_ptr(&self) -> *mut u8 {
        self.base
    }

    /// Host pointer for a guest address.
    #[inline]
    pub fn host_ptr(&self, addr: u32) -> *mut u8 {
        // SAFETY: in bounds of the window.
        unsafe { self.base.add(addr as usize) }
    }

    rw_num!(read_u8, write_u8, u8);
    rw_num!(read_u16, write_u16, u16);
    rw_num!(read_u32, write_u32, u32);
    rw_num!(read_u64, write_u64, u64);
    rw_num!(read_i8, write_i8, i8);
    rw_num!(read_i16, write_i16, i16);
    rw_num!(read_i32, write_i32, i32);
    rw_num!(read_f32, write_f32, f32);
    rw_num!(read_f64, write_f64, f64);

    #[inline]
    fn check(addr: u32, len: usize) {
        assert!(
            addr as u64 + len as u64 <= layout::WINDOW_SIZE,
            "guest slice {addr:#x}+{len:#x} wraps the 4 GiB window"
        );
    }

    /// Borrow guest bytes.
    pub fn slice(&self, addr: u32, len: usize) -> &[u8] {
        Self::check(addr, len);
        // SAFETY: bounds checked; the mapping lives as long as self.
        unsafe { std::slice::from_raw_parts(self.base.add(addr as usize), len) }
    }

    /// Mutably borrow guest bytes.
    pub fn slice_mut(&mut self, addr: u32, len: usize) -> &mut [u8] {
        Self::check(addr, len);
        // SAFETY: bounds checked; &mut self guarantees exclusivity.
        unsafe { std::slice::from_raw_parts_mut(self.base.add(addr as usize), len) }
    }

    /// Copy bytes out of guest memory.
    pub fn read_bytes(&self, addr: u32, len: usize) -> Vec<u8> {
        self.slice(addr, len).to_vec()
    }

    /// Copy bytes into guest memory.
    pub fn write_bytes(&mut self, addr: u32, data: &[u8]) {
        self.slice_mut(addr, data.len()).copy_from_slice(data);
    }

    /// Fill guest memory with a byte.
    pub fn fill(&mut self, addr: u32, len: usize, value: u8) {
        self.slice_mut(addr, len).fill(value);
    }

    /// Copy within guest memory (memmove semantics).
    pub fn copy_within(&mut self, src: u32, dst: u32, len: usize) {
        Self::check(src, len);
        Self::check(dst, len);
        // SAFETY: both ranges in bounds; ptr::copy handles overlap.
        unsafe { std::ptr::copy(self.base.add(src as usize), self.base.add(dst as usize), len) }
    }

    /// Length of the NUL-terminated byte string at `addr` (scans at most `max` bytes).
    pub fn strlen(&self, addr: u32, max: usize) -> usize {
        let max = max.min((layout::WINDOW_SIZE - addr as u64) as usize);
        let s = self.slice(addr, max);
        s.iter().position(|&b| b == 0).unwrap_or(max)
    }

    /// NUL-terminated ANSI string bytes (without the NUL), at most 64 KiB.
    pub fn read_cstr_bytes(&self, addr: u32) -> Vec<u8> {
        let n = self.strlen(addr, 0x1_0000);
        self.read_bytes(addr, n)
    }

    /// NUL-terminated ANSI string decoded as Windows-1252/Latin-1 (each byte -> one char).
    pub fn read_cstr(&self, addr: u32) -> String {
        self.read_cstr_bytes(addr).iter().map(|&b| b as char).collect()
    }

    /// Write `s` (chars > 0xFF become '?') plus a NUL. Returns bytes written including the NUL.
    pub fn write_cstr(&mut self, addr: u32, s: &str) -> usize {
        let bytes: Vec<u8> = s.chars().map(|c| if (c as u32) < 0x100 { c as u8 } else { b'?' }).collect();
        self.write_bytes(addr, &bytes);
        self.write_u8(addr + bytes.len() as u32, 0);
        bytes.len() + 1
    }

    /// Write raw bytes plus a NUL.
    pub fn write_cstr_bytes(&mut self, addr: u32, s: &[u8]) -> usize {
        self.write_bytes(addr, s);
        self.write_u8(addr + s.len() as u32, 0);
        s.len() + 1
    }

    /// NUL-terminated UTF-16LE string (at most 32 Ki units).
    pub fn read_wstr(&self, addr: u32) -> String {
        let mut units = Vec::new();
        let mut a = addr;
        while units.len() < 0x8000 {
            let u = self.read_u16(a);
            if u == 0 {
                break;
            }
            units.push(u);
            a = a.wrapping_add(2);
        }
        String::from_utf16_lossy(&units)
    }

    /// Write `s` as UTF-16LE plus a NUL unit. Returns units written including the NUL.
    pub fn write_wstr(&mut self, addr: u32, s: &str) -> usize {
        let mut a = addr;
        let mut n = 0;
        for u in s.encode_utf16() {
            self.write_u16(a, u);
            a += 2;
            n += 1;
        }
        self.write_u16(a, 0);
        n + 1
    }

    /// Zero a range and give whole host pages back to the kernel (used on release/decommit).
    pub fn discard(&mut self, addr: u32, len: u32) {
        if len == 0 {
            return;
        }
        Self::check(addr, len as usize);
        // Errors already fell back to zeroing by hand.
        let _ = sys::discard(self.host_ptr(addr), len as usize);
    }

    // ---------------------------------------------------------------- VirtualAlloc family

    /// Win32 `VirtualAlloc` semantics (`alloc_type` = MEM_COMMIT/MEM_RESERVE bits). Returns the base address.
    pub fn virtual_alloc(&mut self, addr: u32, size: u32, alloc_type: u32, protect: u32) -> Result<u32, MemError> {
        self.regions.virtual_alloc(addr, size, alloc_type, protect)
    }

    /// Win32 `VirtualFree` semantics. Freed / decommitted memory reads back as zero.
    pub fn virtual_free(&mut self, addr: u32, size: u32, free_type: u32) -> Result<(), MemError> {
        let (a, len) = self.regions.virtual_free(addr, size, free_type)?;
        self.discard(a, len);
        Ok(())
    }

    /// Win32 `VirtualProtect`; returns the old protection of the first page. Protection is bookkeeping only.
    pub fn virtual_protect(&mut self, addr: u32, size: u32, protect: u32) -> Result<u32, MemError> {
        self.regions.protect(addr, size, protect)
    }

    /// Win32 `VirtualQuery`.
    pub fn virtual_query(&self, addr: u32) -> MemoryInfo {
        self.regions.query(addr)
    }

    /// Reserve + commit a range for runtime use. `addr = None` picks a free spot in `[lo, hi)`.
    pub fn alloc_region(
        &mut self,
        addr: Option<u32>,
        size: u32,
        kind: RegionKind,
        arena: (u32, u32),
    ) -> Result<u32, MemError> {
        let base = self.regions.reserve_in(addr, size, kind, PAGE_READWRITE, arena)?;
        self.regions.commit(base, size, PAGE_READWRITE)?;
        Ok(base)
    }

    // ---------------------------------------------------------------- heaps

    /// The default process heap (`GetProcessHeap`).
    pub fn process_heap(&self) -> u32 {
        self.process_heap
    }

    /// `HeapCreate`. Returns the heap handle (= guest address of its first segment).
    pub fn heap_create(&mut self, initial: u32, max: u32) -> Result<u32, MemError> {
        self.heaps.create(&mut self.regions, initial, max)
    }

    /// `HeapDestroy`.
    pub fn heap_destroy(&mut self, heap: u32) -> Result<(), MemError> {
        let segs = self.heaps.destroy(heap)?;
        for (base, size) in segs {
            let _ = self.regions.virtual_free(base, 0, MEM_RELEASE);
            self.discard(base, size);
        }
        Ok(())
    }

    /// `HeapAlloc`. `flags & HEAP_ZERO_MEMORY` zeroes the block.
    pub fn heap_alloc(&mut self, heap: u32, size: u32, flags: u32) -> Option<u32> {
        let p = self.heaps.alloc(&mut self.regions, heap, size).ok()?;
        if flags & HEAP_ZERO_MEMORY != 0 {
            self.fill(p, size as usize, 0);
        }
        Some(p)
    }

    /// `HeapFree`.
    pub fn heap_free(&mut self, heap: u32, ptr: u32) -> bool {
        self.heaps.free(heap, ptr).is_ok()
    }

    /// `HeapSize` (the size requested by the guest).
    pub fn heap_size(&self, heap: u32, ptr: u32) -> Option<u32> {
        self.heaps.size(heap, ptr)
    }

    /// `HeapReAlloc`. Keeps contents; zeroes the grown part when `HEAP_ZERO_MEMORY` is set.
    pub fn heap_realloc(&mut self, heap: u32, ptr: u32, size: u32, flags: u32) -> Option<u32> {
        let old = self.heaps.size(heap, ptr)?;
        if self.heaps.try_resize_in_place(heap, ptr, size) {
            if flags & HEAP_ZERO_MEMORY != 0 && size > old {
                self.fill(ptr + old, (size - old) as usize, 0);
            }
            return Some(ptr);
        }
        if flags & HEAP_REALLOC_IN_PLACE_ONLY != 0 {
            return None;
        }
        let new = self.heaps.alloc(&mut self.regions, heap, size).ok()?;
        self.copy_within(ptr, new, old.min(size) as usize);
        if flags & HEAP_ZERO_MEMORY != 0 && size > old {
            self.fill(new + old, (size - old) as usize, 0);
        }
        self.heaps.free(heap, ptr).ok()?;
        Some(new)
    }

    /// Convenience: allocate from the process heap (runtime-internal strings, COM objects, ...). Zeroed.
    pub fn alloc(&mut self, size: u32) -> u32 {
        let h = self.process_heap;
        self.heap_alloc(h, size.max(1), HEAP_ZERO_MEMORY).expect("process heap exhausted")
    }

    /// Convenience: free a block from [`GuestMemory::alloc`].
    pub fn free(&mut self, ptr: u32) -> bool {
        let h = self.process_heap;
        self.heap_free(h, ptr)
    }
}

impl Drop for GuestMemory {
    fn drop(&mut self) {
        if !self.owns_window {
            return;
        }
        sys::release_window(self.base, (layout::WINDOW_SIZE + layout::GUARD_SIZE) as usize);
    }
}

#[cfg(test)]
mod tests;
