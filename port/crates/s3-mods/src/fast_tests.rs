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

use crate::{fast_blit, fast_sprite};

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
    for a in [STACK, SRC, DST, PAL, 0x007e_8dec & !0xFFF] {
        assert!(nm.slice(a, BUF_LEN as usize) == im.slice(a, BUF_LEN as usize), "{what}: memory at {a:#x} differs");
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
