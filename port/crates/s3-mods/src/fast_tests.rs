//! Differential tests of the native fast paths (`fast_*.rs`) against the original guest code.
//!
//! The original code comes from the user's local S3.EXE (never from our sources): the test maps its sections
//! into guest memory, prepares random inputs, then
//! 1. calls the native fast path with a random budget: it returns `k` guest instructions (or declines), and
//! 2. runs the interpreter (no fast paths) for exactly `k` instructions from the same state,
//! and requires identical registers, EFLAGS, EIP and memory (all buffers the code touches, byte for byte).
//! Skipped (passes with a note) when the exe is missing. Exe path: env `S3_EXE`, else
//! `game-files/gog-s3-ultimate/S3.EXE` in the repository.

use s3_cpu::{Cpu, GuestMemory, NativeFn, EAX, EBP, ECX, EDI, EDX, ESI, ESP};

use crate::{fast_blit, fast_land, fast_obj, fast_snap, fast_sprite};

fn exe_bytes() -> Option<Vec<u8>> {
    let p = std::env::var_os("S3_EXE").map(std::path::PathBuf::from).unwrap_or_else(|| {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../game-files/gog-s3-ultimate/S3.EXE")
    });
    std::fs::read(p).ok()
}

/// Map the PE sections at their virtual addresses.
fn load_image(mem: &mut GuestMemory, b: &[u8]) {
    let u16_at = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]) as usize;
    let u32_at = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    let pe = u32_at(0x3c) as usize;
    let n = u16_at(pe + 6);
    let opt = u16_at(pe + 20);
    let base = u32_at(pe + 24 + 28);
    for i in 0..n {
        let s = pe + 24 + opt + 40 * i;
        let (va, rsize, raw) = (u32_at(s + 12), u32_at(s + 16) as usize, u32_at(s + 20) as usize);
        if rsize > 0 && raw + rsize <= b.len() {
            mem.write_bytes(base + va, &b[raw..raw + rsize]);
        }
    }
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 16) as u32
    }
    fn below(&mut self, n: u32) -> u32 {
        self.next() % n.max(1)
    }
}

/// Guest buffers used by the cases.
const STACK: u32 = 0x3000_0000;
const SRC: u32 = 0x3100_0000;
const DST: u32 = 0x3200_0000;
const PAL: u32 = 0x3300_0000;
const BUF_LEN: u32 = 0x10_0000;

/// One case: initial registers, memory writes, native function, start EIP. Compares both executions.
/// Returns false if the native path declined.
fn check(
    exe: &[u8],
    f: NativeFn,
    start: u32,
    regs: [u32; 8],
    eflags: u32,
    writes: &[(u32, Vec<u8>)],
    budget: u64,
    what: &str,
) -> bool {
    let regions = [STACK, SRC, DST, PAL, 0x007e_8dec & !0xFFF].map(|a| (a, BUF_LEN));
    check_in(exe, f, start, regs, eflags, writes, budget, what, &regions)
}

#[allow(clippy::too_many_arguments)]
fn check_in(
    exe: &[u8],
    f: NativeFn,
    start: u32,
    regs: [u32; 8],
    eflags: u32,
    writes: &[(u32, Vec<u8>)],
    budget: u64,
    what: &str,
    regions: &[(u32, u32)],
) -> bool {
    let mk = || {
        let mut mem = GuestMemory::new().unwrap();
        load_image(&mut mem, exe);
        for (a, d) in writes {
            mem.write_bytes(*a, d);
        }
        let mut cpu = Cpu::new();
        cpu.regs = regs;
        cpu.set_eflags(eflags);
        cpu.eip = start;
        (cpu, mem)
    };
    let (mut nc, mut nm) = mk();
    let Some(k) = f(&mut nc, &mut nm, budget) else { return false };
    assert!(k <= budget, "{what}: k {k} > budget {budget}");
    let (mut ic, mut im) = mk();
    let mut left = k;
    while left > 0 {
        let before = ic.instructions;
        let e = ic.run(&mut im, left);
        assert!(matches!(e, s3_cpu::CpuExit::Budget), "{what}: interpreter exit {e:?} at {:#x}", ic.eip);
        left -= ic.instructions - before;
    }
    assert_eq!(nc.eip, ic.eip, "{what}: eip");
    assert_eq!(nc.regs, ic.regs, "{what}: regs (native vs interpreter)");
    assert_eq!(nc.eflags(), ic.eflags(), "{what}: eflags");
    for &(a, len) in regions {
        assert!(nm.slice(a, len as usize) == im.slice(a, len as usize), "{what}: memory at {a:#x} differs");
    }
    true
}

fn random_bytes(r: &mut Rng, n: usize) -> Vec<u8> {
    (0..n).map(|_| r.next() as u8).collect()
}

#[test]
fn differential_fast_paths() {
    let Some(exe) = exe_bytes() else {
        eprintln!("differential_fast_paths: S3.EXE not found, skipped");
        return;
    };
    let mut r = Rng(0x1234_5678_9abc_def1);
    let mut ran = [0u32; 4];
    for case in 0..120 {
        let budget = if case % 4 == 0 { 1 + r.below(400) as u64 } else { 1 << 30 };
        let eflags = 0x202 | (r.next() & 0x8D5);
        let base_regs = |r: &mut Rng| -> [u32; 8] { std::array::from_fn(|_| r.next()) };

        // --- sprite rows: random RLE rows, pitch, row counter.
        {
            let mut regs = base_regs(&mut r);
            let esp = STACK + 0x1000;
            let rows = 1 + r.below(12);
            let pitch = 2 * (100 + r.below(600)) + r.below(2); // odd pitch too (shr 1 drops bit 0)
            let mut src = Vec::new();
            for _ in 0..rows {
                let runs = 1 + r.below(5);
                for k in 0..runs {
                    let cnt = r.below(60) as u8;
                    let skip = r.below(40) as u8 | if k + 1 == runs { 0x80 } else { 0 };
                    src.push(cnt);
                    src.push(skip);
                    src.extend(random_bytes(&mut r, cnt as usize * 2));
                }
            }
            let mut stack = random_bytes(&mut r, 0x200);
            let put = |s: &mut Vec<u8>, off: usize, v: u32| s[off..off + 4].copy_from_slice(&v.to_le_bytes());
            let dst0 = DST + 2 * r.below(64);
            put(&mut stack, 0x10, dst0);
            put(&mut stack, 0x30, pitch);
            put(&mut stack, 0x98, rows + r.below(3));
            regs[EAX] = SRC;
            regs[ESI] = dst0;
            regs[EDI] = r.below(2);
            regs[ESP] = esp;
            let writes = vec![(SRC, src), (esp, stack), (DST, random_bytes(&mut r, 0x4000))];
            if check(&exe, fast_sprite::sprite_rows, fast_sprite::START, regs, eflags, &writes, budget, "sprite_rows") {
                ran[0] += 1;
            }
        }
        // --- palette row.
        {
            let mut regs = base_regs(&mut r);
            let n = 1 + r.below(300);
            regs[EAX] = DST + 2 * r.below(32);
            regs[ESI] = regs[EAX] + 2 * n;
            regs[ECX] = SRC + r.below(16);
            regs[EBP] = PAL + 2 * r.below(8);
            let writes = vec![(SRC, random_bytes(&mut r, 400)), (PAL, random_bytes(&mut r, 600))];
            if check(&exe, fast_blit::palette_row, fast_blit::PAL_START, regs, eflags, &writes, budget, "palette_row") {
                ran[1] += 1;
            }
        }
        // --- darken.
        {
            let mut regs = base_regs(&mut r);
            let n = 1 + r.below(300);
            regs[EAX] = DST + 4 * r.below(32);
            regs[ESI] = regs[EAX] + 4 * n;
            let writes = vec![(DST, random_bytes(&mut r, 2000)), (0x007e_8dec, random_bytes(&mut r, 4))];
            if check(&exe, fast_blit::darken, fast_blit::DARK_START, regs, eflags, &writes, budget, "darken") {
                ran[2] += 1;
            }
        }
        // --- dither.
        {
            let mut regs = base_regs(&mut r);
            let n = 1 + r.below(300);
            regs[EAX] = DST + 2 * r.below(32);
            regs[EDX] = regs[EAX] + 2 * n;
            regs[ESI] = r.below(32);
            let writes = vec![(DST, random_bytes(&mut r, 1000))];
            if check(&exe, fast_blit::dither, fast_blit::DITHER_START, regs, eflags, &writes, budget, "dither") {
                ran[3] += 1;
            }
        }
    }
    eprintln!("differential_fast_paths: native ran (not declined) sprite/palette/darken/dither = {ran:?} of 120 each");
    assert!(ran.iter().all(|&n| n > 60), "too many declines: {ran:?}");
}

/// Snapshot fill tile loop (`fast_snap`): random map tiles, mostly on the common path, random exits.
#[test]
fn differential_snap_tiles() {
    let Some(exe) = exe_bytes() else {
        eprintln!("differential_snap_tiles: S3.EXE not found, skipped");
        return;
    };
    const OBJ: u32 = 0x3500_0000;
    const GD: u32 = 0x3400_0000;
    const SNAP: u32 = 0x3600_0000;
    let mut r = Rng(0x0bad_cafe_1234_5679);
    let mut ran = 0;
    for case in 0..200 {
        let budget = if case % 4 == 0 { 1 + r.below(2000) as u64 } else { 1 << 30 };
        let eflags = 0x202 | (r.next() & 0x8D5);
        let mut regs: [u32; 8] = std::array::from_fn(|_| r.next());
        let esp = STACK + 0x1000;
        regs[ESP] = esp;
        regs[EBP] = OBJ;
        let n = 8 + r.below(24); // map size
        let row0 = r.below(n);
        let col = if r.below(20) == 0 { n + r.below(3) } else { r.below(n) };
        let k0 = r.below(40);
        let kmax = k0 + r.below(30);
        let base = r.below(100);
        let mut stack = random_bytes(&mut r, 0x100);
        let put = |s: &mut Vec<u8>, off: usize, v: u32| s[off..off + 4].copy_from_slice(&v.to_le_bytes());
        put(&mut stack, 0x28, row0);
        put(&mut stack, 0x44, col);
        put(&mut stack, 0x34, k0);
        put(&mut stack, 0x3c, kmax);
        put(&mut stack, 0x20, base);
        let mut obj = vec![0u8; 0x400];
        obj[0x3a0..0x3a4].copy_from_slice(&GD.to_le_bytes());
        obj[0x3d0..0x3d4].copy_from_slice(&SNAP.to_le_bytes());
        let put_h = |s: &mut Vec<u8>, off: usize, v: u32| s[off..off + 4].copy_from_slice(&v.to_le_bytes());
        let mut gdh = vec![0u8; 0x80];
        put_h(&mut gdh, 0x2d2d0 - 0x2d284, n);
        put_h(&mut gdh, 0, if r.below(30) == 0 { 1 } else { r.next() & !1 });
        put_h(&mut gdh, 0x2d2e4 - 0x2d284, if r.below(10) == 0 { row0 + r.below(8) } else { 0xffff });
        gdh[0x2d300 - 0x2d284] = 0x55;
        // tiles: rows row0.. (at most n rows) of column col
        let tiles_len = (n as usize + 2) * 768 * 24;
        let mut tiles = random_bytes(&mut r, tiles_len.min(0x10_0000));
        for row in 0..n + 1 {
            let o = ((row * 768 + col) * 24) as usize;
            if o + 0x12 > tiles.len() {
                continue;
            }
            let rare = r.below(25) == 0;
            let t = &mut tiles[o..o + 0x12];
            if !rare {
                t[0] = 0;
                t[1] = 0;
                t[2] = 0;
                t[3] = 0;
                t[0xa] &= !0x20;
                if t[0xb] == 0x55 {
                    t[0xb] = 0;
                }
                let a = (r.below(8)) as u8;
                t[9] = a | (a << 3) | (r.below(2) as u8) << 6;
            }
        }
        let writes = vec![
            (esp, stack),
            (OBJ, obj),
            (GD + 0x2d284, gdh),
            (GD + 0x67444, tiles),
            (SNAP, random_bytes(&mut r, 0x10_0000)),
        ];
        let regions = [(STACK, 0x2000), (OBJ, 0x1000), (GD + 0x2d000, 0x1000), (GD + 0x67000, 0x11_0000), (SNAP, 0x11_0000)];
        if check_in(&exe, fast_snap::snap_tiles, fast_snap::START, regs, eflags, &writes, budget, "snap_tiles", &regions) {
            ran += 1;
        }
    }
    eprintln!("differential_snap_tiles: native ran {ran} of 200");
    assert!(ran > 100, "too many declines: {ran}");
}

/// Per-tile object drawer `0x4d3050`, empty-tile case (`fast_obj`): random cells/args, the native path runs the
/// whole call up to the return address; the interpreter must arrive at the same state.
#[test]
fn differential_obj_empty() {
    let Some(exe) = exe_bytes() else {
        eprintln!("differential_obj_empty: S3.EXE not found, skipped");
        return;
    };
    const SNAP: u32 = 0x3600_0000;
    let mut r = Rng(0x5eed_0000_4d30_50aa);
    let mut ran = 0;
    for case in 0..300 {
        let budget = if case % 5 == 0 { 100 + r.below(20) as u64 } else { 1 << 30 };
        let eflags = 0x202 | (r.next() & 0x8D5);
        let mut regs: [u32; 8] = std::array::from_fn(|_| r.next());
        let e = STACK + 0x1000;
        regs[ESP] = e;
        regs[ECX] = r.next();
        let (a2, a3) = (r.below(200), r.below(130));
        let mut stack = random_bytes(&mut r, 0x1400);
        let base = 0xc00; // the stack buffer starts at STACK = e - 0x1000; base + 0x400 = offset of e
        let put = |s: &mut Vec<u8>, off: usize, v: u32| s[off..off + 4].copy_from_slice(&v.to_le_bytes());
        put(&mut stack, base + 0x400, 0x0040_1000 + r.below(0x1000)); // return address
        put(&mut stack, base + 0x404, SNAP);
        put(&mut stack, base + 0x408, a2);
        put(&mut stack, base + 0x40c, a3);
        put(&mut stack, base + 0x418, if r.below(15) == 0 { 1 + r.below(300) } else { 0 });
        let mut snap = random_bytes(&mut r, 0x10_0000.min(0x60 + 8));
        put(&mut snap, 0x60, [0, 1, 2, 3, 4, 5, 0x10][r.below(7) as usize]);
        let cell = ((a3 + a2 * 130) * 48) as usize;
        let mut cellb = random_bytes(&mut r, 0x10);
        cellb[2] = [0, 1, 2, 5, 7, 8, 0x50, 0x51, 0xff, 3, 4, 6][r.below(12) as usize];
        if r.below(2) == 0 {
            cellb[0xa] = 0;
            cellb[0xb] = 0;
        }
        if r.below(2) == 0 {
            cellb[5] &= 0x13;
        }
        let writes = vec![(STACK, stack), (SNAP, snap), (SNAP + cell as u32 + 0x500, cellb)];
        let regions = [(STACK, 0x3000), (SNAP, 0x20_0000)];
        if check_in(&exe, fast_obj::obj_empty, fast_obj::START, regs, eflags, &writes, budget, "obj_empty", &regions) {
            ran += 1;
        }
    }
    eprintln!("differential_obj_empty: native ran {ran} of 300");
    assert!(ran > 60, "too many declines: {ran}");
}

/// Landscape triangle loop (`fast_land`): random heights and dirty bits, random frame variables.
#[test]
fn differential_land_clean() {
    let Some(exe) = exe_bytes() else {
        eprintln!("differential_land_clean: S3.EXE not found, skipped");
        return;
    };
    const SNAP: u32 = 0x3600_0000;
    let mut r = Rng(0x1a2d_c1ea_0054_ce53);
    let mut ran = 0;
    for case in 0..300 {
        let budget = if case % 4 == 0 { 1 + r.below(800) as u64 } else { 1 << 30 };
        let eflags = 0x202 | (r.next() & 0x8D5);
        let mut regs: [u32; 8] = std::array::from_fn(|_| r.next());
        let ebp = STACK + 0x1000;
        regs[EBP] = ebp;
        regs[ESP] = ebp - 0x200;
        let mut frame = random_bytes(&mut r, 0x200);
        let o = |d: i32| (0x100 + d) as usize; // frame buffer starts at ebp - 0x100
        let put = |f: &mut Vec<u8>, d: i32, v: u32| f[o(d)..o(d) + 4].copy_from_slice(&v.to_le_bytes());
        let v74 = r.below(50);
        put(&mut frame, -0x74, v74);
        put(&mut frame, -0xc4, v74 + r.below(60));
        put(&mut frame, -0x5c, r.below(2));
        put(&mut frame, -0x44, r.below(20));
        put(&mut frame, -0x48, r.below(40));
        put(&mut frame, -0x98, r.below(1000));
        put(&mut frame, 8, SNAP);
        for d in [-0x68, -0xe4, -0xe8] {
            put(&mut frame, d, if r.below(40) == 0 { 1 } else { r.below(3) * 2 });
        }
        let mut cells = random_bytes(&mut r, 0x8_0000);
        for (i, c) in cells.iter_mut().enumerate() {
            if i % 48 == 0x505 % 48 && r.below(30) != 0 {
                *c &= 0x7f;
            }
        }
        let writes = vec![(ebp - 0x100, frame), (SNAP, cells)];
        let regions = [(STACK, 0x2000), (SNAP, 0x8_0000)];
        if check_in(&exe, fast_land::land_clean, fast_land::START, regs, eflags, &writes, budget, "land_clean", &regions) {
            ran += 1;
        }
    }
    eprintln!("differential_land_clean: native ran {ran} of 300");
    assert!(ran > 60, "too many declines: {ran}");
}
