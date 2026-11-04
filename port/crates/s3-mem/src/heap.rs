//! Guest heaps (`HeapCreate`/`HeapAlloc`/...). Metadata lives on the host side.
//!
//! Each heap owns one or more segments (VirtualAlloc'd, committed). Free space is tracked in two maps
//! (by address for coalescing, by (size, address) for best-fit). Blocks are 8-byte aligned (like Windows).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::layout::{ALLOC_ARENA_END, ALLOC_ARENA_START, ALLOC_GRANULARITY};
use crate::regions::{RegionKind, Regions, PAGE_READWRITE};
use crate::{align_up, MemError};

pub const HEAP_ZERO_MEMORY: u32 = 0x08;
pub const HEAP_REALLOC_IN_PLACE_ONLY: u32 = 0x10;

const MIN_SEGMENT: u32 = 0x10_0000; // 1 MiB
const ALIGN: u32 = 8;

/// One guest heap.
#[derive(Debug, Default)]
pub struct GuestHeap {
    /// (base, size) of each segment; the first segment's base is the heap handle.
    pub segments: Vec<(u32, u32)>,
    free_by_addr: BTreeMap<u32, u32>,
    free_by_size: BTreeSet<(u32, u32)>,
    /// ptr -> (block size, requested size)
    used: HashMap<u32, (u32, u32)>,
    /// Bytes currently handed out (block sizes).
    pub used_bytes: u64,
}

impl GuestHeap {
    fn insert_free(&mut self, mut addr: u32, mut size: u32) {
        // Coalesce with predecessor.
        if let Some((&pa, &ps)) = self.free_by_addr.range(..addr).next_back() {
            if pa as u64 + ps as u64 == addr as u64 {
                self.free_by_addr.remove(&pa);
                self.free_by_size.remove(&(ps, pa));
                addr = pa;
                size += ps;
            }
        }
        // Coalesce with successor.
        let end = addr as u64 + size as u64;
        if end <= u32::MAX as u64 {
            if let Some(&ns) = self.free_by_addr.get(&(end as u32)) {
                self.free_by_addr.remove(&(end as u32));
                self.free_by_size.remove(&(ns, end as u32));
                size += ns;
            }
        }
        self.free_by_addr.insert(addr, size);
        self.free_by_size.insert((size, addr));
    }

    fn take_free(&mut self, size: u32) -> Option<u32> {
        let &(fs, fa) = self.free_by_size.range((size, 0)..).next()?;
        self.free_by_size.remove(&(fs, fa));
        self.free_by_addr.remove(&fa);
        if fs > size {
            let rest = fa + size;
            let rs = fs - size;
            self.free_by_addr.insert(rest, rs);
            self.free_by_size.insert((rs, rest));
        }
        Some(fa)
    }

    /// Number of live blocks.
    pub fn live_blocks(&self) -> usize {
        self.used.len()
    }

    /// Total free bytes in all segments.
    pub fn free_bytes(&self) -> u64 {
        self.free_by_addr.values().map(|&s| s as u64).sum()
    }
}

/// All guest heaps by handle.
#[derive(Debug, Default)]
pub struct Heaps {
    heaps: BTreeMap<u32, GuestHeap>,
}

fn block_size(size: u32) -> u32 {
    align_up(size.max(1), ALIGN)
}

impl Heaps {
    pub fn new() -> Self {
        Self::default()
    }

    /// Heap by handle.
    pub fn get(&self, heap: u32) -> Option<&GuestHeap> {
        self.heaps.get(&heap)
    }

    /// All heap handles.
    pub fn handles(&self) -> impl Iterator<Item = u32> + '_ {
        self.heaps.keys().copied()
    }

    fn add_segment(regions: &mut Regions, min: u32) -> Result<(u32, u32), MemError> {
        let size = align_up(min.max(MIN_SEGMENT), ALLOC_GRANULARITY);
        let base = regions.reserve_in(None, size, RegionKind::Private, PAGE_READWRITE, (ALLOC_ARENA_START, ALLOC_ARENA_END))?;
        regions.commit(base, size, PAGE_READWRITE)?;
        Ok((base, size))
    }

    /// Create a heap. `max` is ignored (heaps are growable).
    pub fn create(&mut self, regions: &mut Regions, initial: u32, _max: u32) -> Result<u32, MemError> {
        let (base, size) = Self::add_segment(regions, initial)?;
        let mut h = GuestHeap::default();
        h.segments.push((base, size));
        h.insert_free(base, size);
        self.heaps.insert(base, h);
        Ok(base)
    }

    /// Destroy a heap; returns its segments so the caller can release them.
    pub fn destroy(&mut self, heap: u32) -> Result<Vec<(u32, u32)>, MemError> {
        let h = self.heaps.remove(&heap).ok_or(MemError::BadHeap(heap))?;
        Ok(h.segments)
    }

    /// Allocate `size` bytes. The memory is NOT zeroed here (see `GuestMemory::heap_alloc`).
    pub fn alloc(&mut self, regions: &mut Regions, heap: u32, size: u32) -> Result<u32, MemError> {
        let bs = block_size(size);
        let h = self.heaps.get_mut(&heap).ok_or(MemError::BadHeap(heap))?;
        let p = match h.take_free(bs) {
            Some(p) => p,
            None => {
                let (base, ssize) = Self::add_segment(regions, bs)?;
                h.segments.push((base, ssize));
                h.insert_free(base, ssize);
                h.take_free(bs).ok_or(MemError::OutOfSpace)?
            }
        };
        h.used.insert(p, (bs, size));
        h.used_bytes += bs as u64;
        Ok(p)
    }

    /// Free a block.
    pub fn free(&mut self, heap: u32, ptr: u32) -> Result<(), MemError> {
        let h = self.heaps.get_mut(&heap).ok_or(MemError::BadHeap(heap))?;
        let (bs, _) = h.used.remove(&ptr).ok_or(MemError::NotAllocated { addr: ptr, len: 0 })?;
        h.used_bytes -= bs as u64;
        h.insert_free(ptr, bs);
        Ok(())
    }

    /// Requested size of a live block.
    pub fn size(&self, heap: u32, ptr: u32) -> Option<u32> {
        self.heaps.get(&heap)?.used.get(&ptr).map(|&(_, s)| s)
    }

    /// Which heap owns `ptr` (any heap), if it is a live block.
    pub fn owner_of(&self, ptr: u32) -> Option<u32> {
        self.heaps.iter().find(|(_, h)| h.used.contains_key(&ptr)).map(|(k, _)| *k)
    }

    /// Shrink or grow a block without moving it. Returns false if it cannot grow in place.
    pub fn try_resize_in_place(&mut self, heap: u32, ptr: u32, size: u32) -> bool {
        let Some(h) = self.heaps.get_mut(&heap) else { return false };
        let Some(&(bs, _)) = h.used.get(&ptr) else { return false };
        let nbs = block_size(size);
        if nbs <= bs {
            if nbs < bs {
                h.insert_free(ptr + nbs, bs - nbs);
                h.used_bytes -= (bs - nbs) as u64;
            }
            h.used.insert(ptr, (nbs, size));
            return true;
        }
        let next = ptr + bs;
        let need = nbs - bs;
        match h.free_by_addr.get(&next).copied() {
            Some(fs) if fs >= need => {
                h.free_by_addr.remove(&next);
                h.free_by_size.remove(&(fs, next));
                if fs > need {
                    h.free_by_addr.insert(next + need, fs - need);
                    h.free_by_size.insert((fs - need, next + need));
                }
                h.used.insert(ptr, (nbs, size));
                h.used_bytes += need as u64;
                true
            }
            _ => false,
        }
    }
}
