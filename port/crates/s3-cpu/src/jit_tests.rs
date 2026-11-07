//! JIT differential tests (perf-jit): random guest programs run through the interpreter and through the JIT;
//! registers, EFLAGS, lazy-flag state, EIP, exits, retired counts and memory must be identical after every
//! budget slice. Only built on aarch64 with the `jit` feature.

use iced_x86::code_asm::*;

use crate::*;

const CODE: u32 = 0x0040_1000;
const SUBS: u32 = 0x0048_0000;
const STACK_TOP: u32 = 0x0090_0000;
const STACK_LO: u32 = 0x008F_0000;
const DATA: u32 = 0x0100_0000;
const DATA_LEN: u32 = 0x10_0000;
const MID: u32 = DATA + DATA_LEN / 2;
const CNT: u32 = DATA + DATA_LEN + 0x10;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn n(&mut self, k: u64) -> u64 {
        self.next() % k
    }
    fn u32(&mut self) -> u32 {
        // Edge values often.
        const E: [u32; 12] = [0, 1, 2, 0x7F, 0x80, 0xFF, 0x7FFF, 0x8000, 0xFFFF, 0x7FFF_FFFF, 0x8000_0000, 0xFFFF_FFFF];
        if self.n(3) == 0 {
            E[self.n(E.len() as u64) as usize]
        } else {
            self.next() as u32
        }
    }
    fn small(&mut self) -> i32 {
        self.n(256) as i32 - 128
    }
}

const DREG: [AsmRegister32; 3] = [eax, ecx, edx];
const PREG: [AsmRegister32; 4] = [ebx, esi, edi, ebp];
const AREG: [AsmRegister32; 7] = [eax, ecx, edx, ebx, esi, edi, ebp];
const R8W: [AsmRegister8; 7] = [al, cl, dl, ah, ch, dh, bl];
const R8R: [AsmRegister8; 8] = [al, cl, dl, ah, ch, dh, bl, bh];
const R16: [AsmRegister16; 3] = [ax, cx, dx];

fn pick<T: Copy>(r: &mut Rng, v: &[T]) -> T {
    v[r.n(v.len() as u64) as usize]
}

/// A memory operand that stays inside the DATA region (pointer registers stay near MID).
fn mem(r: &mut Rng) -> AsmMemoryOperand {
    let p = pick(r, &PREG);
    match r.n(10) {
        0..=5 => p + r.small(),
        6 => {
            let q = pick(r, &PREG);
            let s = [1u32, 2, 4, 8][r.n(4) as usize];
            let d = (s.wrapping_mul(MID)).wrapping_neg().wrapping_add(r.small() as u32) as i32;
            p + q * s + d
        }
        7 => {
            let q = pick(r, &PREG);
            let s = [1u32, 2, 4, 8][r.n(4) as usize];
            let d = MID.wrapping_sub(s.wrapping_mul(MID)).wrapping_add(r.small() as u32) as i32;
            q * s + d
        }
        _ => ptr((MID as i64 + r.small() as i64) as u64),
    }
    .into()
}

struct Gen {
    r: Rng,
    pushes: u32,
    subs: Vec<CodeLabel>,
    ops: u32,
}

macro_rules! alu_all {
    ($a:expr, $g:expr, $op:ident) => {{
        let g: &mut Gen = $g;
        let a: &mut CodeAssembler = $a;
        match g.r.n(13) {
            0 => a.$op(pick(&mut g.r, &DREG), pick(&mut g.r, &AREG)).unwrap(),
            1 => a.$op(pick(&mut g.r, &DREG), g.r.u32() as i32).unwrap(),
            2 => a.$op(pick(&mut g.r, &DREG), dword_ptr(mem(&mut g.r))).unwrap(),
            3 => a.$op(dword_ptr(mem(&mut g.r)), pick(&mut g.r, &AREG)).unwrap(),
            4 => a.$op(dword_ptr(mem(&mut g.r)), g.r.u32() as i32).unwrap(),
            5 => a.$op(pick(&mut g.r, &R8W), pick(&mut g.r, &R8R)).unwrap(),
            6 => a.$op(pick(&mut g.r, &R8W), g.r.u32() as i32 & 0xFF).unwrap(),
            7 => a.$op(pick(&mut g.r, &R8W), byte_ptr(mem(&mut g.r))).unwrap(),
            8 => a.$op(byte_ptr(mem(&mut g.r)), pick(&mut g.r, &R8R)).unwrap(),
            9 => a.$op(byte_ptr(mem(&mut g.r)), g.r.u32() as i32 & 0xFF).unwrap(),
            10 => a.$op(pick(&mut g.r, &R16), pick(&mut g.r, &R16)).unwrap(),
            11 => a.$op(pick(&mut g.r, &R16), g.r.u32() as i32 & 0xFFFF).unwrap(),
            _ => a.$op(word_ptr(mem(&mut g.r)), pick(&mut g.r, &R16)).unwrap(),
        }
    }};
}

macro_rules! cmp_all {
    ($a:expr, $g:expr, $op:ident) => {{
        let g: &mut Gen = $g;
        let a: &mut CodeAssembler = $a;
        match g.r.n(8) {
            0 => a.$op(pick(&mut g.r, &AREG), pick(&mut g.r, &AREG)).unwrap(),
            1 => a.$op(pick(&mut g.r, &AREG), g.r.u32() as i32).unwrap(),
            2 => a.$op(dword_ptr(mem(&mut g.r)), pick(&mut g.r, &AREG)).unwrap(),
            3 => a.$op(dword_ptr(mem(&mut g.r)), g.r.u32() as i32).unwrap(),
            4 => a.$op(pick(&mut g.r, &R8R), pick(&mut g.r, &R8R)).unwrap(),
            5 => a.$op(byte_ptr(mem(&mut g.r)), g.r.u32() as i32 & 0xFF).unwrap(),
            6 => a.$op(pick(&mut g.r, &R16), g.r.u32() as i32 & 0xFFFF).unwrap(),
            _ => a.$op(pick(&mut g.r, &R8R), g.r.u32() as i32 & 0xFF).unwrap(),
        }
    }};
}

impl Gen {
    /// One random instruction (or a small group that keeps the invariants).
    fn insn(&mut self, a: &mut CodeAssembler, depth: u32) {
        self.ops += 1;
        let r = &mut self.r;
        match r.n(60) {
            0..=3 => alu_all!(a, self, add),
            4..=6 => alu_all!(a, self, sub),
            7 => alu_all!(a, self, and),
            8 => alu_all!(a, self, or),
            9 => alu_all!(a, self, xor),
            10 => alu_all!(a, self, adc),
            11 => alu_all!(a, self, sbb),
            12..=15 => cmp_all!(a, self, cmp),
            16..=17 => cmp_all!(a, self, test),
            18..=23 => match r.n(9) {
                0 => a.mov(pick(r, &DREG), pick(r, &AREG)).unwrap(),
                1 => a.mov(pick(r, &DREG), r.u32()).unwrap(),
                2 => a.mov(pick(r, &DREG), dword_ptr(mem(r))).unwrap(),
                3 => a.mov(dword_ptr(mem(r)), pick(r, &AREG)).unwrap(),
                4 => a.mov(dword_ptr(mem(r)), r.u32()).unwrap(),
                5 => a.mov(pick(r, &R8W), byte_ptr(mem(r))).unwrap(),
                6 => a.mov(byte_ptr(mem(r)), pick(r, &R8R)).unwrap(),
                7 => a.mov(word_ptr(mem(r)), pick(r, &R16)).unwrap(),
                _ => a.mov(pick(r, &R16), word_ptr(mem(r))).unwrap(),
            },
            24 => match r.n(6) {
                0 => a.movzx(pick(r, &DREG), byte_ptr(mem(r))).unwrap(),
                1 => a.movsx(pick(r, &DREG), word_ptr(mem(r))).unwrap(),
                2 => a.movzx(pick(r, &DREG), pick(r, &R8R)).unwrap(),
                3 => a.movsx(pick(r, &DREG), pick(r, &R8R)).unwrap(),
                4 => a.movzx(pick(r, &DREG), pick(r, &R16)).unwrap(),
                _ => a.movsx(pick(r, &DREG), byte_ptr(mem(r))).unwrap(),
            },
            25 => match r.n(3) {
                0 => a.lea(pick(r, &DREG), mem(r)).unwrap(),
                1 => {
                    let p = pick(r, &PREG);
                    a.lea(p, p + r.small()).unwrap()
                }
                _ => {
                    let p = pick(r, &PREG);
                    a.lea(pick(r, &DREG), p + pick(r, &DREG) * 4 + r.small()).unwrap()
                }
            },
            26..=27 => match r.n(6) {
                0 => a.inc(pick(r, &DREG)).unwrap(),
                1 => a.dec(pick(r, &DREG)).unwrap(),
                2 => a.inc(dword_ptr(mem(r))).unwrap(),
                3 => a.dec(dword_ptr(mem(r))).unwrap(),
                4 => a.dec(pick(r, &PREG)).unwrap(),
                _ => a.inc(byte_ptr(mem(r))).unwrap(),
            },
            28 => match r.n(3) {
                0 => a.neg(pick(r, &DREG)).unwrap(),
                1 => a.not(pick(r, &DREG)).unwrap(),
                _ => a.neg(byte_ptr(mem(r))).unwrap(),
            },
            29..=31 => {
                let d = pick(r, &DREG);
                let c = [0u32, 1, 2, 7, 8, 15, 16, 24, 31, 32, 33][r.n(11) as usize];
                match r.n(9) {
                    0 => a.shl(d, c).unwrap(),
                    1 => a.shr(d, c).unwrap(),
                    2 => a.sar(d, c).unwrap(),
                    3 => a.shl(d, cl).unwrap(),
                    4 => a.shr(pick(r, &R8W), c).unwrap(),
                    5 => a.rol(d, c).unwrap(),
                    6 => a.rcr(d, 1).unwrap(),
                    7 => a.sar(pick(r, &R16), cl).unwrap(),
                    _ => a.shl(d, 1).unwrap(),
                }
            }
            32..=33 => match r.n(6) {
                0 => a.imul_2(pick(r, &DREG), pick(r, &AREG)).unwrap(),
                1 => a.imul_2(pick(r, &DREG), dword_ptr(mem(r))).unwrap(),
                2 => a.imul_3(pick(r, &DREG), pick(r, &AREG), r.u32() as i32).unwrap(),
                3 => a.imul_3(pick(r, &DREG), dword_ptr(mem(r)), r.small()).unwrap(),
                4 => a.mul(pick(r, &AREG)).unwrap(),
                _ => a.imul(dword_ptr(mem(r))).unwrap(),
            },
            34 => match r.n(3) {
                0 => a.cdq().unwrap(),
                1 => a.cwde().unwrap(),
                _ => a.nop().unwrap(),
            },
            35..=36 => {
                let d = pick(r, &R8W);
                match r.n(16) {
                    0 => a.seto(d),
                    1 => a.setno(d),
                    2 => a.setb(d),
                    3 => a.setae(d),
                    4 => a.sete(d),
                    5 => a.setne(d),
                    6 => a.setbe(d),
                    7 => a.seta(d),
                    8 => a.sets(d),
                    9 => a.setns(d),
                    10 => a.setp(d),
                    11 => a.setnp(d),
                    12 => a.setl(d),
                    13 => a.setge(d),
                    14 => a.setle(d),
                    _ => a.setg(d),
                }
                .unwrap()
            }
            37..=39 if depth == 0 => {
                // push (balanced by later pops)
                match r.n(3) {
                    0 => a.push(pick(r, &AREG)).unwrap(),
                    1 => a.push(r.u32() as i32).unwrap(),
                    _ => a.push(dword_ptr(mem(r))).unwrap(),
                }
                self.pushes += 1;
            }
            40..=41 if depth == 0 => {
                if self.pushes > 0 {
                    a.pop(pick(r, &DREG)).unwrap();
                    self.pushes -= 1;
                } else {
                    a.cmc().unwrap();
                }
            }
            42 => match r.n(8) {
                0 => {
                    a.pushfd().unwrap();
                    a.pop(eax).unwrap()
                }
                1 => a.lahf().unwrap(),
                2 => a.clc().unwrap(),
                3 => a.stc().unwrap(),
                4 => a.bt(pick(r, &DREG), r.n(32) as i32).unwrap(),
                5 => a.bswap(pick(r, &DREG)).unwrap(),
                6 => a.xchg(pick(r, &DREG), pick(r, &DREG)).unwrap(),
                _ => a.cmove(pick(r, &DREG), pick(r, &AREG)).unwrap(),
            },
            43 => {
                // Possible #DE.
                if r.n(4) == 0 {
                    a.div(ecx).unwrap();
                } else {
                    a.xor(edx, edx).unwrap();
                    a.or(ecx, 1).unwrap();
                    a.div(ecx).unwrap();
                }
            }
            44 => {
                if r.n(8) == 0 {
                    // Possible NULL fault (memory access below 64 KiB).
                    a.mov(eax, (r.n(0xF000) as u32) & !3).unwrap();
                    match r.n(3) {
                        0 => a.mov(ecx, dword_ptr(eax + 4)).unwrap(),
                        1 => a.add(dword_ptr(eax), ecx).unwrap(),
                        _ => a.push(dword_ptr(eax)).unwrap(),
                    }
                    if r.n(2) == 0 {
                        self.pushes += 0;
                    }
                } else {
                    a.cld().unwrap();
                    a.and(ecx, 7).unwrap();
                    a.rep().movsb().unwrap();
                }
            }
            45..=48 if depth < 2 => {
                // Forward conditional jump over a few instructions (new blocks, flags across blocks).
                let mut l = a.create_label();
                match r.n(10) {
                    0 => a.je(l),
                    1 => a.jne(l),
                    2 => a.jl(l),
                    3 => a.jge(l),
                    4 => a.jb(l),
                    5 => a.ja(l),
                    6 => a.jbe(l),
                    7 => a.js(l),
                    8 => a.jo(l),
                    _ => a.jmp(l),
                }
                .unwrap();
                let k = 1 + r.n(4);
                for _ in 0..k {
                    self.insn(a, depth + 1);
                }
                a.set_label(&mut l).unwrap();
                a.nop().unwrap();
            }
            49 if !self.subs.is_empty() && depth < 2 => {
                let l = self.subs[self.r.n(self.subs.len() as u64) as usize];
                a.call(l).unwrap();
            }
            _ => alu_all!(a, self, add),
        }
    }
}

struct Prog {
    bytes: Vec<u8>,
    sub_bytes: Vec<u8>,
}

fn gen_prog(seed: u64) -> Prog {
    let mut g = Gen { r: Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1), pushes: 0, subs: Vec::new(), ops: 0 };
    // Subroutines (own assembler at SUBS): random body + ret, no pushes left.
    let mut sa = CodeAssembler::new(32).unwrap();
    let nsub = g.r.n(3) as usize;
    let mut starts = Vec::new();
    for _ in 0..nsub {
        let mut l = sa.create_label();
        sa.set_label(&mut l).unwrap();
        starts.push(l);
        let saved = g.pushes;
        g.pushes = 0;
        for _ in 0..(1 + g.r.n(12)) {
            g.insn(&mut sa, 1);
        }
        for _ in 0..g.pushes {
            sa.pop(edx).unwrap();
        }
        g.pushes = saved;
        if g.r.n(2) == 0 {
            sa.ret().unwrap();
        } else {
            sa.ret_1(0).unwrap();
        }
    }
    let sub_res = if nsub > 0 {
        sa.assemble_options(SUBS as u64, iced_x86::BlockEncoderOptions::RETURN_NEW_INSTRUCTION_OFFSETS).unwrap()
    } else {
        sa.nop().unwrap();
        sa.assemble_options(SUBS as u64, iced_x86::BlockEncoderOptions::RETURN_NEW_INSTRUCTION_OFFSETS).unwrap()
    };
    let sub_addrs: Vec<u64> = starts.iter().map(|l| sub_res.label_ip(l).unwrap()).collect();
    let sub_bytes = sub_res.inner.code_buffer.clone();

    let mut a = CodeAssembler::new(32).unwrap();
    // Calls to the subroutines use their absolute addresses (`call rel32`).
    let iters = 1 + g.r.n(12) as u32;
    a.mov(dword_ptr(CNT as u64), iters).unwrap();
    let mut top = a.create_label();
    a.set_label(&mut top).unwrap();
    let body = 1 + g.r.n(48);
    for _ in 0..body {
        if !sub_addrs.is_empty() && g.r.n(16) == 0 {
            let t = sub_addrs[g.r.n(sub_addrs.len() as u64) as usize];
            a.call(t).unwrap();
        } else {
            g.insn(&mut a, 0);
        }
    }
    for _ in 0..g.pushes {
        a.pop(edx).unwrap();
    }
    g.pushes = 0;
    match g.r.n(3) {
        0 => {
            a.dec(dword_ptr(CNT as u64)).unwrap();
            a.jne(top).unwrap();
        }
        1 => {
            a.sub(dword_ptr(CNT as u64), 1).unwrap();
            a.jg(top).unwrap();
        }
        _ => {
            a.mov(eax, dword_ptr(CNT as u64)).unwrap();
            a.dec(eax).unwrap();
            a.mov(dword_ptr(CNT as u64), eax).unwrap();
            a.test(eax, eax).unwrap();
            a.jne(top).unwrap();
        }
    }
    a.ret().unwrap();
    let bytes = a.assemble(CODE as u64).unwrap();
    Prog { bytes, sub_bytes }
}

fn machine(p: &Prog, seed: u64) -> (Cpu, GuestMemory) {
    let mut mem = GuestMemory::new().unwrap();
    mem.write_bytes(CODE, &p.bytes);
    mem.write_bytes(SUBS, &p.sub_bytes);
    let mut r = Rng(seed ^ 0x5555_AAAA_1234_5678);
    let mut fill = vec![0u8; DATA_LEN as usize];
    for c in fill.chunks_mut(8) {
        let v = r.next().to_le_bytes();
        c.copy_from_slice(&v[..c.len()]);
    }
    mem.write_bytes(DATA, &fill);
    let mut cpu = Cpu::new();
    for (i, reg) in cpu.regs.iter_mut().enumerate() {
        *reg = match i {
            EBX | ESI | EDI | EBP => MID.wrapping_add(r.n(512) as u32).wrapping_sub(256),
            _ => r.u32(),
        };
    }
    cpu.regs[ESP] = STACK_TOP;
    cpu.push32(&mut mem, layout::MAGIC_RETURN);
    cpu.eip = CODE;
    cpu.set_eflags(0x202 | (r.next() as u32 & fl::STATUS));
    (cpu, mem)
}

/// Observable CPU state (lazy fields only where they are observable).
fn state(c: &Cpu) -> String {
    let lazy = if c.lk == flags::lk::NONE || c.lk == flags::lk::LOGIC {
        String::new()
    } else {
        format!(
            " lk={} lsz={} la={:#x} lb={:#x} lr={:#x} lcf={}",
            c.lk,
            c.lsz,
            c.la,
            c.lb,
            c.lr,
            if c.lk >= 4 { c.lcf } else { 0 }
        )
    };
    format!(
        "regs={:08x?} eip={:#x} eflags={:#x} insns={} fs={:#x}{lazy}",
        c.regs,
        c.eip,
        c.eflags(),
        c.instructions,
        c.fs_base
    )
}

fn compare_mem(a: &GuestMemory, b: &GuestMemory) -> Option<String> {
    for (lo, len) in [(DATA, DATA_LEN + 0x100), (STACK_LO, STACK_TOP - STACK_LO)] {
        let x = a.slice(lo, len as usize);
        let y = b.slice(lo, len as usize);
        if x != y {
            let i = x.iter().zip(y).position(|(p, q)| p != q).unwrap();
            return Some(format!("memory differs at {:#x}: {:#x} vs {:#x}", lo as usize + i, x[i], y[i]));
        }
    }
    None
}

/// Run program `seed` on the interpreter and on the JIT with the same budget slices; panic on any difference.
fn diff_one(seed: u64, thr: u64, slices: bool) -> (u64, usize) {
    let p = gen_prog(seed);
    let (mut ci, mut mi) = machine(&p, seed);
    let (mut cj, mut mj) = machine(&p, seed);
    ci.set_jit(Some(false));
    cj.set_jit(Some(true));
    cj.set_jit_threshold(Some(thr));
    let mut r = Rng(seed ^ 0xABCD);
    for step in 0..100_000 {
        let big = r.n(4) == 0;
        let mut budget = if slices { 1 + r.n(if big { 400 } else { 40 }) } else { 5_000_000 };
        if std::env::var_os("JIT_STEP1").is_some() {
            budget = 1;
        }
        if std::env::var_os("JIT_VERBOSE").is_some_and(|v| v == "2") {
            eprintln!("  step {step} budget {budget} eip {:#x} {}", ci.eip, state(&ci));
        }
        let ei = ci.run(&mut mi, budget);
        if std::env::var_os("JIT_VERBOSE").is_some_and(|v| v == "2") {
            eprintln!("  interp done: {ei:?}");
        }
        let ej = cj.run(&mut mj, budget);
        let (s_i, s_j) = (state(&ci), state(&cj));
        if ei != ej || s_i != s_j {
            panic!("seed {seed} thr {thr} step {step} budget {budget}:\n interp {ei:?} {s_i}\n jit    {ej:?} {s_j}");
        }
        if let Some(d) = compare_mem(&mi, &mj) {
            panic!("seed {seed} thr {thr} step {step}: {d}");
        }
        if ei != CpuExit::Budget {
            break;
        }
    }
    (ci.instructions, cj.cache.jit_blocks())
}

#[test]
fn jit_matches_interpreter_on_random_programs() {
    let mut insns = 0;
    let mut compiled = 0;
    // `JIT_SEEDS=a-b` picks the seed range (debugging), `JIT_VERBOSE=1` prints each seed first.
    let (lo, hi) = std::env::var("JIT_SEEDS")
        .ok()
        .and_then(|v| v.split_once('-').map(|(a, b)| (a.parse().unwrap(), b.parse().unwrap())))
        .unwrap_or((1u64, 1000u64));
    let verbose = std::env::var_os("JIT_VERBOSE").is_some();
    for seed in lo..=hi {
        for (thr, slices) in [(0, false), (0, true), (2, true)] {
            if verbose {
                eprintln!("seed {seed} thr {thr} slices {slices}");
            }
            let (n, c) = diff_one(seed, thr, slices);
            insns += n;
            compiled += c;
        }
    }
    assert!(compiled > 1000, "too few compiled blocks: {compiled}");
    eprintln!("jit diff: {insns} instructions, {compiled} compiled blocks");
}

/// Hooks and native blocks are still entered at their addresses (never inside JIT-linked code).
#[test]
fn jit_respects_hooks_natives_and_invalidation() {
    use std::sync::atomic::{AtomicU64, Ordering};
    static CALLS: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];
    fn decline0(_: &mut Cpu, _: &mut GuestMemory, _: u64) -> Option<u64> {
        CALLS[0].fetch_add(1, Ordering::Relaxed);
        None
    }
    fn decline1(_: &mut Cpu, _: &mut GuestMemory, _: u64) -> Option<u64> {
        CALLS[1].fetch_add(1, Ordering::Relaxed);
        None
    }
    // Loop: top: add eax, ecx ; [mid:] xor edx, eax ; dec ecx ; jnz top ; ret
    let mut a = CodeAssembler::new(32).unwrap();
    let mut top = a.create_label();
    let mut mid = a.create_label();
    a.mov(ecx, 1000).unwrap();
    a.set_label(&mut top).unwrap();
    a.add(eax, ecx).unwrap();
    a.set_label(&mut mid).unwrap();
    a.xor(edx, eax).unwrap();
    a.dec(ecx).unwrap();
    a.jne(top).unwrap();
    a.ret().unwrap();
    let res = a.assemble_options(CODE as u64, iced_x86::BlockEncoderOptions::RETURN_NEW_INSTRUCTION_OFFSETS).unwrap();
    let mid_ip = res.label_ip(&mid).unwrap() as u32;
    let top_ip = res.label_ip(&top).unwrap() as u32;
    let bytes = res.inner.code_buffer.clone();
    let p = Prog { bytes, sub_bytes: vec![0x90] };
    for (k, jit) in [false, true].into_iter().enumerate() {
        let (mut c, mut m) = machine(&p, 7);
        c.set_jit(Some(jit));
        c.set_jit_threshold(Some(0));
        let mut nb = NativeBlocks::new();
        let f: NativeFn = if k == 0 { decline0 } else { decline1 };
        nb.insert(top_ip, NativeBlock { name: "t", len: 2, hash: code_hash(m.slice(top_ip, 2)), f });
        c.set_native_blocks(Some(std::sync::Arc::new(nb)));
        let mut hooks = HookAddrs::new();
        hooks.insert(mid_ip);
        c.set_hooks(Some(std::sync::Arc::new(hooks)));
        let run_all = |c: &mut Cpu, m: &mut GuestMemory| {
            let mut hits = 0;
            loop {
                match c.run(m, 1_000_000) {
                    CpuExit::Hook(e) => {
                        assert_eq!(e, mid_ip);
                        hits += 1;
                        c.skip_hook_once(e);
                    }
                    CpuExit::ReturnMagic => break hits,
                    e => panic!("{e:?}"),
                }
            }
        };
        assert_eq!(run_all(&mut c, &mut m), 1000, "jit={jit}");
        assert_eq!(CALLS[k].load(Ordering::Relaxed), 999, "jit={jit}"); // the first pass enters `top` mid-block
                                                                        // Self-modifying code: patch `add eax, ecx` (01 c8) into `sub eax, ecx` (29 c8), invalidate, rerun.
        let before = c.regs[EAX];
        m.write_bytes(top_ip, &[0x29]);
        c.invalidate_range(top_ip, 1);
        c.eip = CODE;
        c.regs[ESP] = STACK_TOP - 4;
        m.write_u32(STACK_TOP - 4, layout::MAGIC_RETURN);
        assert_eq!(run_all(&mut c, &mut m), 1000, "jit={jit}");
        if jit {
            assert!(c.cache.jit_blocks() > 0);
        }
        let mut expect = before;
        for i in (1..=1000u32).rev() {
            expect = expect.wrapping_sub(i);
        }
        assert_eq!(c.regs[EAX], expect, "jit={jit}");
    }
}
