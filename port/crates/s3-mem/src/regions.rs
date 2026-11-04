//! VirtualAlloc-style bookkeeping: allocations (reservations) with per-page commit state and protection.
//!
//! Memory itself is always accessible in the host window; this only answers the questions Win32 code asks
//! (`VirtualQuery`, "is this already reserved?", "where is free space?").

use std::collections::BTreeMap;

use crate::layout::{ALLOC_ARENA_END, ALLOC_ARENA_START, ALLOC_GRANULARITY, PAGE_SIZE};
use crate::{align_down, align_up, MemError};

pub const MEM_COMMIT: u32 = 0x1000;
pub const MEM_RESERVE: u32 = 0x2000;
pub const MEM_DECOMMIT: u32 = 0x4000;
pub const MEM_RELEASE: u32 = 0x8000;
pub const MEM_FREE: u32 = 0x1_0000;
pub const MEM_PRIVATE: u32 = 0x2_0000;
pub const MEM_MAPPED: u32 = 0x4_0000;
pub const MEM_IMAGE: u32 = 0x100_0000;

pub const PAGE_NOACCESS: u32 = 0x01;
pub const PAGE_READONLY: u32 = 0x02;
pub const PAGE_READWRITE: u32 = 0x04;
pub const PAGE_EXECUTE: u32 = 0x10;
pub const PAGE_EXECUTE_READ: u32 = 0x20;
pub const PAGE_EXECUTE_READWRITE: u32 = 0x40;

/// What an allocation is (the `Type` field of `MEMORY_BASIC_INFORMATION`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionKind {
    Private,
    Image,
    Mapped,
}

impl RegionKind {
    fn mem_type(self) -> u32 {
        match self {
            RegionKind::Private => MEM_PRIVATE,
            RegionKind::Image => MEM_IMAGE,
            RegionKind::Mapped => MEM_MAPPED,
        }
    }
}

/// One reservation.
#[derive(Debug, Clone)]
pub struct Allocation {
    pub base: u32,
    /// Size in bytes, multiple of the page size.
    pub size: u32,
    pub alloc_protect: u32,
    pub kind: RegionKind,
    /// Per page: 0 = reserved only, otherwise the committed page's protection.
    pub pages: Vec<u32>,
}

impl Allocation {
    fn end(&self) -> u64 {
        self.base as u64 + self.size as u64
    }
}

/// `MEMORY_BASIC_INFORMATION` contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryInfo {
    pub base_address: u32,
    pub allocation_base: u32,
    pub allocation_protect: u32,
    pub region_size: u32,
    /// MEM_COMMIT / MEM_RESERVE / MEM_FREE.
    pub state: u32,
    pub protect: u32,
    /// MEM_PRIVATE / MEM_IMAGE / MEM_MAPPED (0 for free).
    pub type_: u32,
}

impl MemoryInfo {
    /// Serialize as the 28-byte Win32 `MEMORY_BASIC_INFORMATION`.
    pub fn to_bytes(&self) -> [u8; 28] {
        let mut b = [0u8; 28];
        let f = [
            self.base_address,
            self.allocation_base,
            self.allocation_protect,
            self.region_size,
            self.state,
            self.protect,
            self.type_,
        ];
        for (i, v) in f.iter().enumerate() {
            b[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        b
    }
}

/// All reservations, keyed by base address.
#[derive(Debug, Default)]
pub struct Regions {
    allocs: BTreeMap<u32, Allocation>,
}

impl Regions {
    pub fn new() -> Self {
        Self::default()
    }

    /// Iterate allocations in address order.
    pub fn iter(&self) -> impl Iterator<Item = &Allocation> {
        self.allocs.values()
    }

    /// The allocation containing `addr`, if any.
    pub fn find(&self, addr: u32) -> Option<&Allocation> {
        let (_, a) = self.allocs.range(..=addr).next_back()?;
        if (addr as u64) < a.end() {
            Some(a)
        } else {
            None
        }
    }

    fn find_mut(&mut self, addr: u32) -> Option<&mut Allocation> {
        let (_, a) = self.allocs.range_mut(..=addr).next_back()?;
        if (addr as u64) < a.end() {
            Some(a)
        } else {
            None
        }
    }

    fn overlaps(&self, addr: u32, size: u32) -> bool {
        let end = addr as u64 + size as u64;
        if let Some((_, a)) = self.allocs.range(..end.min(u32::MAX as u64 + 1) as u32).next_back() {
            if a.end() > addr as u64 && (a.base as u64) < end {
                return true;
            }
        }
        // The range query above uses `..end` which excludes an allocation starting exactly at `end` (fine) but
        // when end == 2^32 the cast saturates; double check the last allocation.
        if end > u32::MAX as u64 {
            if let Some((_, a)) = self.allocs.iter().next_back() {
                return a.end() > addr as u64;
            }
        }
        false
    }

    /// Lowest 64 KiB aligned free gap of `size` bytes inside `[lo, hi)`.
    fn find_free(&self, size: u32, lo: u32, hi: u32) -> Option<u32> {
        let mut cand = align_up(lo, ALLOC_GRANULARITY) as u64;
        let size = size as u64;
        for a in self.allocs.values() {
            if a.end() <= cand {
                continue;
            }
            if a.base as u64 >= hi as u64 {
                break;
            }
            if cand + size <= a.base as u64 {
                break;
            }
            cand = align_up(a.end().min(u32::MAX as u64) as u32, ALLOC_GRANULARITY) as u64;
            if a.end() > u32::MAX as u64 {
                return None;
            }
        }
        if cand + size <= hi as u64 {
            Some(cand as u32)
        } else {
            None
        }
    }

    /// Reserve `size` bytes at `addr` (rounded down to 64 KiB) or anywhere in the default arena.
    pub fn reserve(&mut self, addr: Option<u32>, size: u32, kind: RegionKind, protect: u32) -> Result<u32, MemError> {
        self.reserve_in(addr, size, kind, protect, (ALLOC_ARENA_START, ALLOC_ARENA_END))
    }

    /// Like [`Regions::reserve`] but searching `arena = [lo, hi)` when `addr` is `None`.
    /// A fixed `addr` is only rounded down to the page size (runtime-internal fixed placements like TEBs);
    /// Win32 `VirtualAlloc` rounds to 64 KiB itself.
    pub fn reserve_in(
        &mut self,
        addr: Option<u32>,
        size: u32,
        kind: RegionKind,
        protect: u32,
        arena: (u32, u32),
    ) -> Result<u32, MemError> {
        if size == 0 {
            return Err(MemError::BadRange { addr: addr.unwrap_or(0), len: size });
        }
        let (base, size) = match addr {
            Some(a) => {
                let base = align_down(a, PAGE_SIZE);
                let end = a as u64 + size as u64;
                let end = (end + PAGE_SIZE as u64 - 1) & !(PAGE_SIZE as u64 - 1);
                if end > 1u64 << 32 {
                    return Err(MemError::BadRange { addr: a, len: size });
                }
                (base, (end - base as u64) as u32)
            }
            None => {
                let size = align_up(size, PAGE_SIZE);
                (self.find_free(size, arena.0, arena.1).ok_or(MemError::OutOfSpace)?, size)
            }
        };
        if self.overlaps(base, size) {
            return Err(MemError::Overlap { addr: base, len: size });
        }
        let pages = vec![0u32; (size / PAGE_SIZE) as usize];
        self.allocs.insert(base, Allocation { base, size, alloc_protect: protect, kind, pages });
        Ok(base)
    }

    /// Commit pages of an existing allocation.
    pub fn commit(&mut self, addr: u32, size: u32, protect: u32) -> Result<(), MemError> {
        let start = align_down(addr, PAGE_SIZE);
        let end = addr as u64 + size.max(1) as u64;
        let a = self.find_mut(start).ok_or(MemError::NotAllocated { addr, len: size })?;
        if end > a.end() {
            return Err(MemError::NotAllocated { addr, len: size });
        }
        let p0 = ((start - a.base) / PAGE_SIZE) as usize;
        let p1 = ((end - a.base as u64).div_ceil(PAGE_SIZE as u64)) as usize;
        for p in &mut a.pages[p0..p1] {
            *p = if protect == 0 { PAGE_READWRITE } else { protect };
        }
        Ok(())
    }

    /// `VirtualAlloc` semantics.
    pub fn virtual_alloc(&mut self, addr: u32, size: u32, alloc_type: u32, protect: u32) -> Result<u32, MemError> {
        if size == 0 {
            return Err(MemError::BadRange { addr, len: size });
        }
        let reserve = alloc_type & MEM_RESERVE != 0;
        let commit = alloc_type & MEM_COMMIT != 0;
        if addr == 0 || reserve {
            let fixed = if addr == 0 { None } else { Some(align_down(addr, ALLOC_GRANULARITY)) };
            let size = match fixed {
                Some(b) => (addr - b) + size,
                None => size,
            };
            let base = self.reserve(fixed, size, RegionKind::Private, protect)?;
            if commit {
                let sz = self.find(base).map(|a| a.size).unwrap_or(size);
                self.commit(base, sz, protect)?;
            }
            return Ok(base);
        }
        if commit {
            self.commit(addr, size, protect)?;
            return Ok(align_down(addr, PAGE_SIZE));
        }
        Err(MemError::BadRange { addr, len: size })
    }

    /// `VirtualFree` semantics. Returns the guest range whose contents must be discarded.
    pub fn virtual_free(&mut self, addr: u32, size: u32, free_type: u32) -> Result<(u32, u32), MemError> {
        if free_type & MEM_RELEASE != 0 {
            if size != 0 {
                return Err(MemError::BadRange { addr, len: size });
            }
            let a = self.allocs.remove(&addr).ok_or(MemError::NotAllocated { addr, len: 0 })?;
            return Ok((a.base, a.size));
        }
        if free_type & MEM_DECOMMIT != 0 {
            let a = self.find_mut(addr).ok_or(MemError::NotAllocated { addr, len: size })?;
            let (start, end) = if size == 0 {
                // size 0 with DECOMMIT: from addr to end of the allocation.
                (align_down(addr, PAGE_SIZE), a.end())
            } else {
                let s = align_down(addr, PAGE_SIZE);
                let e = (addr as u64 + size as u64 + PAGE_SIZE as u64 - 1) & !(PAGE_SIZE as u64 - 1);
                (s, e)
            };
            if end > a.end() {
                return Err(MemError::NotAllocated { addr, len: size });
            }
            let p0 = ((start - a.base) / PAGE_SIZE) as usize;
            let p1 = ((end - a.base as u64) / PAGE_SIZE as u64) as usize;
            for p in &mut a.pages[p0..p1] {
                *p = 0;
            }
            return Ok((start, (end - start as u64) as u32));
        }
        Err(MemError::BadRange { addr, len: size })
    }

    /// `VirtualProtect` bookkeeping. Returns the previous protection of the first page.
    pub fn protect(&mut self, addr: u32, size: u32, protect: u32) -> Result<u32, MemError> {
        let start = align_down(addr, PAGE_SIZE);
        let end = addr as u64 + size.max(1) as u64;
        let a = self.find_mut(start).ok_or(MemError::NotAllocated { addr, len: size })?;
        if end > a.end() {
            return Err(MemError::NotAllocated { addr, len: size });
        }
        let p0 = ((start - a.base) / PAGE_SIZE) as usize;
        let p1 = ((end - a.base as u64).div_ceil(PAGE_SIZE as u64)) as usize;
        if a.pages[p0..p1].iter().any(|&p| p == 0) {
            return Err(MemError::NotAllocated { addr, len: size });
        }
        let old = a.pages[p0];
        for p in &mut a.pages[p0..p1] {
            *p = protect;
        }
        Ok(old)
    }

    /// `VirtualQuery` semantics.
    pub fn query(&self, addr: u32) -> MemoryInfo {
        let page = align_down(addr, PAGE_SIZE);
        if let Some(a) = self.find(page) {
            let idx = ((page - a.base) / PAGE_SIZE) as usize;
            let st = a.pages[idx];
            let mut n = 1;
            while idx + n < a.pages.len() && a.pages[idx + n] == st {
                n += 1;
            }
            return MemoryInfo {
                base_address: page,
                allocation_base: a.base,
                allocation_protect: a.alloc_protect,
                region_size: n as u32 * PAGE_SIZE,
                state: if st == 0 { MEM_RESERVE } else { MEM_COMMIT },
                protect: st,
                type_: a.kind.mem_type(),
            };
        }
        // Free: extend up to the next allocation.
        let next = self.allocs.range(page..).next().map(|(b, _)| *b as u64).unwrap_or(1u64 << 32);
        MemoryInfo {
            base_address: page,
            allocation_base: 0,
            allocation_protect: 0,
            region_size: (next - page as u64).min(u32::MAX as u64 & !(PAGE_SIZE as u64 - 1)) as u32,
            state: MEM_FREE,
            protect: PAGE_NOACCESS,
            type_: 0,
        }
    }
}
