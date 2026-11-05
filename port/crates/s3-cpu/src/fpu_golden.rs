//! Golden-vector determinism test for the x87 emulation.
//!
//! Runs every non-trivial x87 operation over a fixed pseudo-random input set (normal values in several ranges,
//! plus random bit patterns that hit NaN/SNaN/inf/subnormal/zero), through the real instruction path
//! (assembled snippets executed by the interpreter), and hashes the result bits + status words. The hashes are
//! stored below. They must be identical on every host (x86_64, aarch64, i686-SSE2, ...): lockstep multiplayer
//! peers compute the same game state only if this holds. See `docs/porting/determinism.md`.
//!
//! If a hash changes:
//! - on a new host only -> a real cross-platform determinism bug (do NOT just update the constant);
//! - after an intended semantic change in fpu*.rs (or a `libm` bump) -> update the constant in the same commit and
//!   note it in determinism.md (it changes multiplayer compatibility between app versions).
//!
//! Print the current values with `cargo test -p s3-cpu golden -- --nocapture`.

use iced_x86::code_asm::*;

use crate::*;

const CODE: u32 = 0x0040_1000;
const STACK_TOP: u32 = 0x0090_0000;
const IN_A: u32 = 0x0060_0000;
const IN_B: u32 = 0x0068_0000;
const OUT: u32 = 0x0070_0000;
const OUT_SW: u32 = 0x0078_0000;
const N: usize = 2048;

/// xorshift64* (fixed seed): deterministic input generator.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Input class mix: 0..4 = uniform in [-r, r] for r = 1, 10, 1e4, 1e15; 5 = raw bits; 6 = special values.
fn gen(rng: &mut Rng) -> f64 {
    const SPECIAL: [u64; 12] = [
        0,
        0x8000_0000_0000_0000,
        0x7FF0_0000_0000_0000,
        0xFFF0_0000_0000_0000,
        0x7FF8_0000_0000_0000,
        0xFFF8_0000_0000_0000,
        0x7FF0_0000_0000_0001, // SNaN
        0x7FF8_0000_DEAD_BEEF,
        0x0000_0000_0000_0001, // min subnormal
        0x000F_FFFF_FFFF_FFFF, // max subnormal
        0x0010_0000_0000_0000, // min normal
        0x7FEF_FFFF_FFFF_FFFF, // max
    ];
    let c = rng.next() % 8;
    let s = rng.unit() * 2.0 - 1.0;
    match c {
        0 => s,
        1 => s * 10.0,
        2 => s * 1e4,
        3 => s * 1e15,
        4 => s * std::f64::consts::PI * 2.0,
        5 | 6 => f64::from_bits(rng.next()),
        _ => f64::from_bits(SPECIAL[(rng.next() % SPECIAL.len() as u64) as usize]),
    }
}

fn inputs(seed: u64) -> (Vec<f64>, Vec<f64>) {
    let mut r = Rng(seed);
    let a = (0..N).map(|_| gen(&mut r)).collect();
    let b = (0..N).map(|_| gen(&mut r)).collect();
    (a, b)
}

/// Run `body` once per input pair: ST0 = a[i], ST1 = b[i] on entry; `body` must leave exactly `pops` values,
/// ST0 is stored as a qword (or, if `store32`, as a dword), the status word recorded, then the stack emptied.
fn run_op(
    a_in: &[f64],
    b_in: &[f64],
    control: u16,
    store32: bool,
    body: impl Fn(&mut CodeAssembler),
    left: usize,
) -> u64 {
    let mut a = CodeAssembler::new(32).unwrap();
    let mut lp = a.create_label();
    let cw_slot = OUT_SW - 4;
    a.mov(dword_ptr(cw_slot as u64), control as u32).unwrap();
    a.fldcw(word_ptr(cw_slot as u64)).unwrap();
    a.mov(esi, 0u32).unwrap();
    a.set_label(&mut lp).unwrap();
    a.fld(qword_ptr(esi * 8 + IN_B as i32)).unwrap();
    a.fld(qword_ptr(esi * 8 + IN_A as i32)).unwrap();
    body(&mut a);
    if store32 {
        a.fst(dword_ptr(esi * 8 + OUT as i32)).unwrap();
    } else {
        a.fst(qword_ptr(esi * 8 + OUT as i32)).unwrap();
    }
    a.fnstsw(ax).unwrap();
    a.mov(word_ptr(esi * 2 + OUT_SW as i32), ax).unwrap();
    for _ in 0..left {
        a.fstp(st0).unwrap();
    }
    a.fnclex().unwrap();
    a.inc(esi).unwrap();
    a.cmp(esi, N as u32).unwrap();
    a.jne(lp).unwrap();
    a.ret().unwrap();
    let bytes = a.assemble(CODE as u64).unwrap();

    let mut mem = GuestMemory::new().unwrap();
    mem.write_bytes(CODE, &bytes);
    for i in 0..N {
        mem.write_u64(IN_A + 8 * i as u32, a_in[i].to_bits());
        mem.write_u64(IN_B + 8 * i as u32, b_in[i].to_bits());
    }
    let mut cpu = Cpu::new();
    cpu.regs[ESP] = STACK_TOP;
    cpu.push32(&mut mem, layout::MAGIC_RETURN);
    cpu.eip = CODE;
    let exit = cpu.run(&mut mem, 100_000_000);
    assert_eq!(exit, CpuExit::ReturnMagic, "eip={:#x}", cpu.eip);

    // FNV-1a over result bits and status words.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |x: u64, n: usize| {
        for k in 0..n {
            h ^= (x >> (8 * k)) & 0xFF;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    };
    for i in 0..N {
        eat(mem.read_u64(OUT + 8 * i as u32), 8);
        eat(mem.read_u16(OUT_SW + 2 * i as u32) as u64, 2);
    }
    h
}

type Body = fn(&mut CodeAssembler);

/// (name, control word, store as f32, body, values left on the stack after the body)
fn cases() -> Vec<(&'static str, u16, bool, Body, usize)> {
    vec![
        ("fadd", 0x027F, false, |a| a.fadd_2(st0, st1).unwrap(), 2),
        ("fsub", 0x027F, false, |a| a.fsub_2(st0, st1).unwrap(), 2),
        ("fmul", 0x027F, false, |a| a.fmul_2(st0, st1).unwrap(), 2),
        ("fdiv", 0x027F, false, |a| a.fdiv_2(st0, st1).unwrap(), 2),
        ("fdivr", 0x027F, false, |a| a.fdivr_2(st0, st1).unwrap(), 2),
        ("fmul_pc24", 0x007F, false, |a| a.fmul_2(st0, st1).unwrap(), 2),
        ("fdiv_pc24", 0x007F, false, |a| a.fdiv_2(st0, st1).unwrap(), 2),
        ("fst_m32", 0x027F, true, |_| {}, 2),
        ("fsqrt", 0x027F, false, |a| a.fsqrt().unwrap(), 2),
        ("fsin", 0x027F, false, |a| a.fsin().unwrap(), 2),
        ("fcos", 0x027F, false, |a| a.fcos().unwrap(), 2),
        ("fptan", 0x027F, false, |a| a.fptan().unwrap(), 3),
        ("fsincos", 0x027F, false, |a| a.fsincos().unwrap(), 3),
        ("fpatan", 0x027F, false, |a| a.fpatan().unwrap(), 1),
        ("f2xm1", 0x027F, false, |a| a.f2xm1().unwrap(), 2),
        ("fyl2x", 0x027F, false, |a| a.fyl2x().unwrap(), 1),
        ("fyl2xp1", 0x027F, false, |a| a.fyl2xp1().unwrap(), 1),
        ("fprem", 0x027F, false, |a| a.fprem().unwrap(), 2),
        ("fprem1", 0x027F, false, |a| a.fprem1().unwrap(), 2),
        ("fscale", 0x027F, false, |a| a.fscale().unwrap(), 2),
        ("fxtract", 0x027F, false, |a| a.fxtract().unwrap(), 3),
        ("frndint_rn", 0x027F, false, |a| a.frndint().unwrap(), 2),
        ("frndint_dn", 0x067F, false, |a| a.frndint().unwrap(), 2),
        ("frndint_up", 0x0A7F, false, |a| a.frndint().unwrap(), 2),
        ("frndint_tz", 0x0E7F, false, |a| a.frndint().unwrap(), 2),
        ("fcom", 0x027F, false, |a| a.fcom_2(st0, st1).unwrap(), 2),
        ("fxam", 0x027F, false, |a| a.fxam().unwrap(), 2),
    ]
}

/// Expected hashes, computed on x86_64 Linux (2026-11-05). Same order as `cases()`.
const EXPECTED: &[(&str, u64)] = &[
        ("fadd", 0xcf1e5681047d1330),
        ("fsub", 0x640f558f18c32ec7),
        ("fmul", 0xbff0127509b429ae),
        ("fdiv", 0xcac4d41055740c76),
        ("fdivr", 0x23a35eeeb7361984),
        ("fmul_pc24", 0x66cf9514ef25a5c2),
        ("fdiv_pc24", 0xc55ce7b288894c8b),
        ("fst_m32", 0x09f1ff274a173e85),
        ("fsqrt", 0x38d9b215e0b49c61),
        ("fsin", 0x9b73acd2a25084ec),
        ("fcos", 0x6c41217bd0604f68),
        ("fptan", 0x9c467a7da8fc0d58),
        ("fsincos", 0x9832ccad46c1a260),
        ("fpatan", 0xae99780e55e61de7),
        ("f2xm1", 0x2ce25a48d4d6f03e),
        ("fyl2x", 0xc9fcbaeb2ba61c83),
        ("fyl2xp1", 0x31bfe269cb389b73),
        ("fprem", 0x68bcc74088abcc41),
        ("fprem1", 0xf5f0fc080f75a847),
        ("fscale", 0x339a2fba0424b613),
        ("fxtract", 0x617e7f08d8d0ae03),
        ("frndint_rn", 0xcba2957a4bfb790c),
        ("frndint_dn", 0xeebdb5d9857e45f2),
        ("frndint_up", 0xc0bf4f09783ceccf),
        ("frndint_tz", 0x970906176fa513d1),
        ("fcom", 0xbcc82bea3de0c145),
        ("fxam", 0x25050d5931b1002f),
];

// Needs the 4 GiB guest window, so 64-bit hosts only; `golden_pure_functions` covers 32-bit hosts.
#[cfg(target_pointer_width = "64")]
#[test]
fn golden_x87_vectors() {
    let (a, b) = inputs(0x5E77_1E55_3000_0001);
    let mut got = Vec::new();
    for (name, cw, f32s, body, left) in cases() {
        got.push((name, run_op(&a, &b, cw, f32s, body, left)));
    }
    for (n, h) in &got {
        println!("        (\"{n}\", {h:#018x}),");
    }
    if EXPECTED.is_empty() {
        panic!("EXPECTED is empty: paste the lines printed above into fpu_golden.rs");
    }
    let mut bad = Vec::new();
    for ((n, h), (en, eh)) in got.iter().zip(EXPECTED) {
        assert_eq!(n, en, "case list changed: update EXPECTED");
        if h != eh {
            bad.push(*n);
        }
    }
    assert_eq!(got.len(), EXPECTED.len(), "case list changed: update EXPECTED");
    assert!(bad.is_empty(), "x87 results differ from the golden vectors for: {bad:?} (host determinism bug?)");
}

/// Integer conversions (fistp with every rounding mode, m16/m32/m64) — separate because they store integers.
#[cfg(target_pointer_width = "64")]
#[test]
fn golden_x87_int_conversions() {
    let (a, b) = inputs(0x1A7E_6E55_0000_0002);
    let mut hs = Vec::new();
    for cw in [0x027Fu16, 0x067F, 0x0A7F, 0x0E7F] {
        hs.push(run_op(&a, &b, cw, false, |a| {
            a.fist(dword_ptr(OUT_SW as u64 - 16)).unwrap();
            a.fild(dword_ptr(OUT_SW as u64 - 16)).unwrap();
            a.fist(word_ptr(OUT_SW as u64 - 16)).unwrap();
            a.fild(word_ptr(OUT_SW as u64 - 16)).unwrap();
            a.faddp(st1, st0).unwrap();
            a.fld(st1).unwrap();
            a.fistp(qword_ptr(OUT_SW as u64 - 16)).unwrap();
            a.fild(qword_ptr(OUT_SW as u64 - 16)).unwrap();
            a.faddp(st1, st0).unwrap();
        }, 3));
    }
    for h in &hs {
        println!("int conv {h:#018x}");
    }
    assert_eq!(hs, EXPECTED_INT, "integer conversion results differ from the golden vectors");
}

const EXPECTED_INT: [u64; 4] = [0x3311e2bfe7852cfd, 0xfe87a3178c7cf7cb, 0x5bbe1d8cf98b909b, 0x74ea2482ea929baf];

/// The numeric layer alone (no interpreter, no guest memory), so it also runs on 32-bit hosts
/// (`cargo test -p s3-cpu --target i686-unknown-linux-gnu golden_pure`) and wasm.
#[test]
fn golden_pure_functions() {
    use crate::fpu::{conv, math};
    let (a, b) = inputs(0x9E3A_11FE_0000_0003);
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |x: u64| {
        for k in 0..8 {
            h ^= (x >> (8 * k)) & 0xFF;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    };
    for i in 0..N {
        let (x, y) = (a[i], b[i]);
        for v in [
            math::sin(x),
            math::cos(x),
            math::tan(x),
            math::atan2(x, y),
            math::exp2m1(x),
            math::log2(x),
            math::log2_1p(x),
            math::sqrt(x),
            math::propagate_nan(x, y),
            conv::round_rc(x, 0),
            conv::round_rc(x, 1),
            conv::round_rc(x, 2),
            conv::round_rc(x, 3),
            conv::apply_pc(x * y, 0x007F),
            conv::ldexp(x, (y.to_bits() % 2200) as i64 - 1100),
            conv::f32_to_f64(conv::f64_to_f32(x)),
        ] {
            eat(v.to_bits());
        }
        if x.is_finite() && y.is_finite() && y != 0.0 {
            let r = math::fprem(x, y, false);
            let r1 = math::fprem(x, y, true);
            eat(r.r.to_bits() ^ r.q as u64);
            eat(r1.r.to_bits() ^ r1.q as u64);
        }
        let (m, se) = conv::f64_to_f80(x);
        eat(m ^ se as u64);
        eat(conv::f80_to_f64(y.to_bits(), (x.to_bits() >> 48) as u16).to_bits());
    }
    println!("golden_pure_functions {h:#018x}");
    assert_eq!(h, EXPECTED_PURE, "numeric-layer results differ from the golden vectors (host determinism bug?)");
}

const EXPECTED_PURE: u64 = 0x7d0c9df6e90f4fca;
