use super::*;

#[test]
fn read_write_numbers_and_strings() {
    let mut m = GuestMemory::new().unwrap();
    m.write_u32(0x40_0000, 0xDEAD_BEEF);
    assert_eq!(m.read_u32(0x40_0000), 0xDEAD_BEEF);
    assert_eq!(m.read_u8(0x40_0000), 0xEF);
    assert_eq!(m.read_u16(0x40_0002), 0xDEAD);
    m.write_u64(0x40_0011, 0x0102_0304_0506_0708); // unaligned
    assert_eq!(m.read_u64(0x40_0011), 0x0102_0304_0506_0708);
    m.write_f64(0x40_0100, 1.5);
    assert_eq!(m.read_f64(0x40_0100), 1.5);
    m.write_f32(0x40_0108, -2.25);
    assert_eq!(m.read_f32(0x40_0108), -2.25);
    // Top of the window: reading 8 bytes at the last address must not crash (guard).
    m.write_u8(0xFFFF_FFFF, 7);
    assert_eq!(m.read_u8(0xFFFF_FFFF), 7);
    let _ = m.read_u64(0xFFFF_FFFF);

    assert_eq!(m.write_cstr(0x50_0000, "S3.EXE\u{e9}"), 8);
    assert_eq!(m.read_cstr(0x50_0000), "S3.EXE\u{e9}");
    assert_eq!(m.strlen(0x50_0000, 100), 7);
    assert_eq!(m.write_wstr(0x50_1000, "Grüße"), 6);
    assert_eq!(m.read_wstr(0x50_1000), "Grüße");
    m.write_bytes(0x50_2000, b"abcdef");
    m.copy_within(0x50_2000, 0x50_2002, 4);
    assert_eq!(m.slice(0x50_2000, 6), b"ababcd");
}

#[test]
fn virtual_alloc_reserve_commit_query_free() {
    let mut m = GuestMemory::new().unwrap();
    let a = m.virtual_alloc(0, 0x3000, MEM_RESERVE, PAGE_READWRITE).unwrap();
    assert_eq!(a % layout::ALLOC_GRANULARITY, 0);
    assert!(a >= layout::ALLOC_ARENA_START);
    let q = m.virtual_query(a + 0x10);
    assert_eq!(q.state, MEM_RESERVE);
    assert_eq!(q.allocation_base, a);
    assert_eq!(q.base_address, a);
    assert_eq!(q.region_size, 0x3000);

    let c = m.virtual_alloc(a + 0x1004, 0x10, MEM_COMMIT, PAGE_READWRITE).unwrap();
    assert_eq!(c, a + 0x1000);
    let q = m.virtual_query(a + 0x1000);
    assert_eq!((q.state, q.protect, q.region_size, q.type_), (MEM_COMMIT, PAGE_READWRITE, 0x1000, MEM_PRIVATE));
    assert_eq!(m.virtual_query(a).state, MEM_RESERVE);
    assert_eq!(m.virtual_query(a + 0x2000).state, MEM_RESERVE);

    assert_eq!(m.virtual_protect(a + 0x1000, 0x1000, PAGE_READONLY).unwrap(), PAGE_READWRITE);
    assert!(m.virtual_protect(a, 0x1000, PAGE_READONLY).is_err()); // not committed

    m.write_u32(a + 0x1000, 42);
    m.virtual_free(a + 0x1000, 0x1000, MEM_DECOMMIT).unwrap();
    assert_eq!(m.read_u32(a + 0x1000), 0);
    assert_eq!(m.virtual_query(a + 0x1000).state, MEM_RESERVE);

    // Second allocation does not overlap.
    let b = m.virtual_alloc(0, 0x1_0000, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE).unwrap();
    assert!(b >= a + 0x1_0000);
    // Fixed reserve over an existing allocation fails.
    assert!(m.virtual_alloc(a, 0x1000, MEM_RESERVE, PAGE_READWRITE).is_err());

    assert!(m.virtual_free(a + 0x1000, 0, MEM_RELEASE).is_err());
    m.virtual_free(a, 0, MEM_RELEASE).unwrap();
    assert_eq!(m.virtual_query(a).state, MEM_FREE);
    // Freed space is reused.
    let c = m.virtual_alloc(0, 0x1000, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE).unwrap();
    assert_eq!(c, a);

    // Fixed address reservation.
    let f = m.virtual_alloc(0x2000_1234, 0x100, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE).unwrap();
    assert_eq!(f, 0x2000_0000);
    assert_eq!(m.virtual_query(0x2000_1000).state, MEM_COMMIT);
}

#[test]
fn query_free_space_and_bytes() {
    let m = GuestMemory::new().unwrap();
    let q = m.virtual_query(0x0050_0000);
    assert_eq!(q.state, MEM_FREE);
    let b = q.to_bytes();
    assert_eq!(&b[0..4], &0x0050_0000u32.to_le_bytes());
    assert_eq!(&b[16..20], &MEM_FREE.to_le_bytes());
}

#[test]
fn heap_alloc_free_realloc() {
    let mut m = GuestMemory::new().unwrap();
    let h = m.process_heap();
    let a = m.heap_alloc(h, 10, 0).unwrap();
    let b = m.heap_alloc(h, 100, HEAP_ZERO_MEMORY).unwrap();
    assert_eq!(a % 8, 0);
    assert_eq!(b % 8, 0);
    assert!(b >= a + 16 || a >= b + 100);
    assert_eq!(m.heap_size(h, a), Some(10));
    assert!(m.slice(b, 100).iter().all(|&x| x == 0));

    m.write_bytes(a, b"0123456789");
    // Grow: must keep contents.
    let a2 = m.heap_realloc(h, a, 5000, HEAP_ZERO_MEMORY).unwrap();
    assert_eq!(m.slice(a2, 10), b"0123456789");
    assert!(m.slice(a2 + 10, 4990).iter().all(|&x| x == 0));
    assert_eq!(m.heap_size(h, a2), Some(5000));
    // Shrink in place.
    assert_eq!(m.heap_realloc(h, a2, 4, 0), Some(a2));
    assert_eq!(m.heap_size(h, a2), Some(4));

    assert!(m.heap_free(h, b));
    assert!(!m.heap_free(h, b)); // double free detected
    assert!(m.heap_free(h, a2));
    assert_eq!(m.heaps.get(h).unwrap().live_blocks(), 0);
    // All free space coalesced back into the single segment.
    let seg = m.heaps.get(h).unwrap().segments[0];
    assert_eq!(m.heaps.get(h).unwrap().free_bytes(), seg.1 as u64);
}

#[test]
fn heap_grows_and_destroys() {
    let mut m = GuestMemory::new().unwrap();
    let h = m.heap_create(0, 0).unwrap();
    // Bigger than one segment.
    let big = m.heap_alloc(h, 3 * 1024 * 1024, 0).unwrap();
    m.write_u32(big + 3 * 1024 * 1024 - 4, 1);
    let mut v = Vec::new();
    for i in 0..2000u32 {
        v.push(m.heap_alloc(h, 1 + i % 300, 0).unwrap());
    }
    for p in v {
        assert!(m.heap_free(h, p));
    }
    m.heap_destroy(h).unwrap();
    assert_eq!(m.virtual_query(h).state, MEM_FREE);
    assert!(m.heap_alloc(h, 4, 0).is_none());
}
