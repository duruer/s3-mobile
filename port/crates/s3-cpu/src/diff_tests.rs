//! Differential tests: every integer ALU / shift / multiply / divide / bit instruction is executed natively on
//! the x86_64 host (inline asm; these instructions behave the same in 64-bit mode for 8/16/32-bit operands)
//! and in the interpreter, over edge-case and pseudo-random operands and incoming flag states. Results and
//! EFLAGS must match, except for flags Intel documents as *undefined* for that instruction.
//! Both the specialised fast handlers and the generic path are checked.
//! Only compiled on x86_64 hosts.

#![allow(clippy::type_complexity)]

use core::arch::asm;

use iced_x86::code_asm::*;

use crate::*;

const CODE: u32 = 0x0040_1000;
const STACK_TOP: u32 = 0x0090_0000;
#[allow(dead_code)]
const DATA: u32 = 0x0060_0000;

const CF: u32 = fl::CF;
const PF: u32 = fl::PF;
const AF: u32 = fl::AF;
const ZF: u32 = fl::ZF;
const SF: u32 = fl::SF;
const OF: u32 = fl::OF;
const ALL: u32 = fl::STATUS;

/// Interpreter harness: one assembled snippet (`op` + `ret`), run many times with different inputs.
struct Harness {
    cpu: Cpu,
    mem: GuestMemory,
}

impl Harness {
    fn new(generic: bool, build: &dyn Fn(&mut CodeAssembler)) -> Self {
        let mut a = CodeAssembler::new(32).unwrap();
        build(&mut a);
        a.ret().unwrap();
        let bytes = a.assemble(CODE as u64).unwrap();
        let mut mem = GuestMemory::new().unwrap();
        mem.write_bytes(CODE, &bytes);
        let mut cpu = Cpu::new();
        cpu.set_generic_only(generic);
        Harness { cpu, mem }
    }

    /// Run with EAX, ECX, EDX, EBX and incoming flags. Returns (exit, regs, eflags).
    fn run(&mut self, v_eax: u32, v_ecx: u32, v_edx: u32, v_ebx: u32, fin: u32) -> (CpuExit, [u32; 8], u32) {
        let c = &mut self.cpu;
        c.regs = [v_eax, v_ecx, v_edx, v_ebx, STACK_TOP, 0x1111, 0x2222, 0x3333];
        c.push32(&mut self.mem, layout::MAGIC_RETURN);
        c.eip = CODE;
        c.set_eflags(fin | 0x202);
        let e = c.run(&mut self.mem, 100);
        (e, c.regs, c.eflags())
    }
}

fn values() -> Vec<u32> {
    let mut v = vec![
        0, 1, 2, 3, 7, 8, 9, 15, 16, 17, 31, 32, 33, 0x7F, 0x80, 0x81, 0xFE, 0xFF, 0x100, 0x7FFF, 0x8000, 0x8001,
        0xFFFE, 0xFFFF, 0x1_0000, 0x7FFF_FFFF, 0x8000_0000, 0x8000_0001, 0xFFFF_FFFE, 0xFFFF_FFFF, 0x1234_5678,
        0xDEAD_BEEF, 0x0000_FF80, 0x00FF_FF7F,
    ];
    let mut x: u32 = 0x9E37_79B9;
    for _ in 0..30 {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        v.push(x);
    }
    v
}

/// Incoming flag states (CF matters for adc/sbb/rcl/rcr and must survive inc/dec).
const FLAG_INS: [u32; 3] = [0, CF, ALL];

fn sz_mask(size: u32) -> u32 {
    match size {
        1 => 0xFF,
        2 => 0xFFFF,
        _ => 0xFFFF_FFFF,
    }
}

// ---------------------------------------------------------------------------------------------------------
// Native helpers. `fin` = incoming flags (loaded with popfq), returns (rax/eax, flags) etc.

macro_rules! nat_rr {
    ($tmpl:literal) => {{
        fn f(a: u32, b: u32, fin: u32) -> (u32, u32) {
            let mut a = a as u64;
            let fo: u64;
            unsafe {
                asm!("push {fi}", "popfq", $tmpl, "pushfq", "pop {fo}",
                    a = inout(reg) a, b = in(reg) b as u64, fi = in(reg) fin as u64, fo = lateout(reg) fo);
            }
            (a as u32, fo as u32)
        }
        f as fn(u32, u32, u32) -> (u32, u32)
    }};
}

macro_rules! nat_r {
    ($tmpl:literal) => {{
        fn f(a: u32, _b: u32, fin: u32) -> (u32, u32) {
            let mut a = a as u64;
            let fo: u64;
            unsafe {
                asm!("push {fi}", "popfq", $tmpl, "pushfq", "pop {fo}",
                    a = inout(reg) a, fi = in(reg) fin as u64, fo = lateout(reg) fo);
            }
            (a as u32, fo as u32)
        }
        f as fn(u32, u32, u32) -> (u32, u32)
    }};
}

/// `op a, cl` (b = count).
macro_rules! nat_cl {
    ($tmpl:literal) => {{
        fn f(a: u32, b: u32, fin: u32) -> (u32, u32) {
            let mut a = a as u64;
            let fo: u64;
            unsafe {
                asm!("push {fi}", "popfq", $tmpl, "pushfq", "pop {fo}",
                    a = inout(reg) a, in("ecx") b, fi = in(reg) fin as u64, fo = lateout(reg) fo);
            }
            (a as u32, fo as u32)
        }
        f as fn(u32, u32, u32) -> (u32, u32)
    }};
}

/// `shld/shrd a, b, cl` (c = count).
macro_rules! nat_rrcl {
    ($tmpl:literal) => {{
        fn f(a: u32, b: u32, c: u32, fin: u32) -> (u32, u32) {
            let mut a = a as u64;
            let fo: u64;
            unsafe {
                asm!("push {fi}", "popfq", $tmpl, "pushfq", "pop {fo}",
                    a = inout(reg) a, b = in(reg) b as u64, in("ecx") c, fi = in(reg) fin as u64,
                    fo = lateout(reg) fo);
            }
            (a as u32, fo as u32)
        }
        f as fn(u32, u32, u32, u32) -> (u32, u32)
    }};
}

/// One-operand mul/imul/div/idiv with EAX/EDX implicit; operand in EBX-like register `b`.
macro_rules! nat_md {
    ($tmpl:literal) => {{
        fn f(v_eax: u32, v_edx: u32, b: u32, fin: u32) -> (u32, u32, u32) {
            let mut va = v_eax as u64;
            let mut vd = v_edx as u64;
            let fo: u64;
            unsafe {
                asm!("push {fi}", "popfq", $tmpl, "pushfq", "pop {fo}",
                    inout("rax") va, inout("rdx") vd, b = in(reg) b as u64, fi = in(reg) fin as u64,
                    fo = lateout(reg) fo);
            }
            (va as u32, vd as u32, fo as u32)
        }
        f as fn(u32, u32, u32, u32) -> (u32, u32, u32)
    }};
}

// ---------------------------------------------------------------------------------------------------------
// Checkers

type Nat2 = fn(u32, u32, u32) -> (u32, u32);

/// Binary op `op eax-ish, ebx-ish` (or unary / `op eax, cl` when `cl` is set, then b goes to ECX).
fn check_bin(
    name: &str,
    size: u32,
    build: &dyn Fn(&mut CodeAssembler),
    native: Nat2,
    by_cl: bool,
    undef: &dyn Fn(u32, u32) -> u32,
) {
    let vals = values();
    let counts: Vec<u32> = (0..=40).chain([63, 64, 255, 0x101]).collect();
    let bs: &[u32] = if by_cl { &counts } else { &vals };
    for generic in [false, true] {
        let mut h = Harness::new(generic, build);
        for &a in &vals {
            for &b in bs {
                for &fin in &FLAG_INS {
                    let (na, nf) = native(a, b, fin);
                    let (exit, regs, f) =
                        if by_cl { h.run(a, b, 0, 0, fin) } else { h.run(a, 0, 0, b, fin) };
                    assert_eq!(exit, CpuExit::ReturnMagic, "{name}/{size} a={a:#x} b={b:#x}");
                    let u = undef(a, b);
                    let res_undef = u & 0x8000_0000 != 0;
                    let fm = ALL & !u;
                    let ctx = format!(
                        "{name} size={size} generic={generic} a={a:#x} b={b:#x} fin={fin:#x}: \
                         interp eax={:#x} fl={:#x}, native eax={na:#x} fl={nf:#x} (compared flags {fm:#x})",
                        regs[EAX], f
                    );
                    if !res_undef {
                        assert_eq!(regs[EAX], na, "result: {ctx}");
                    }
                    assert_eq!(f & fm, nf & fm, "flags: {ctx}");
                }
            }
        }
    }
}

/// Bit for "result undefined" in the undef mask.
const RES: u32 = 0x8000_0000;

macro_rules! bin3 {
    ($name:literal, $m:ident, $t8:literal, $t16:literal, $t32:literal, $undef:expr) => {{
        check_bin($name, 1, &|a| { a.$m(al, bl).unwrap(); }, nat_rr!($t8), false, &$undef);
        check_bin($name, 2, &|a| { a.$m(ax, bx).unwrap(); }, nat_rr!($t16), false, &$undef);
        check_bin($name, 4, &|a| { a.$m(eax, ebx).unwrap(); }, nat_rr!($t32), false, &$undef);
    }};
}

#[test]
fn diff_alu_rr() {
    let none = |_: u32, _: u32| 0;
    let logic = |_: u32, _: u32| AF;
    bin3!("add", add, "add {a:l}, {b:l}", "add {a:x}, {b:x}", "add {a:e}, {b:e}", none);
    bin3!("adc", adc, "adc {a:l}, {b:l}", "adc {a:x}, {b:x}", "adc {a:e}, {b:e}", none);
    bin3!("sub", sub, "sub {a:l}, {b:l}", "sub {a:x}, {b:x}", "sub {a:e}, {b:e}", none);
    bin3!("sbb", sbb, "sbb {a:l}, {b:l}", "sbb {a:x}, {b:x}", "sbb {a:e}, {b:e}", none);
    bin3!("cmp", cmp, "cmp {a:l}, {b:l}", "cmp {a:x}, {b:x}", "cmp {a:e}, {b:e}", none);
    bin3!("and", and, "and {a:l}, {b:l}", "and {a:x}, {b:x}", "and {a:e}, {b:e}", logic);
    bin3!("or", or, "or {a:l}, {b:l}", "or {a:x}, {b:x}", "or {a:e}, {b:e}", logic);
    bin3!("xor", xor, "xor {a:l}, {b:l}", "xor {a:x}, {b:x}", "xor {a:e}, {b:e}", logic);
    bin3!("test", test, "test {a:l}, {b:l}", "test {a:x}, {b:x}", "test {a:e}, {b:e}", logic);
}

#[test]
fn diff_unary() {
    let none = |_: u32, _: u32| 0;
    macro_rules! un3 {
        ($name:literal, $m:ident, $t8:literal, $t16:literal, $t32:literal) => {{
            check_bin($name, 1, &|a| { a.$m(al).unwrap(); }, nat_r!($t8), false, &none);
            check_bin($name, 2, &|a| { a.$m(ax).unwrap(); }, nat_r!($t16), false, &none);
            check_bin($name, 4, &|a| { a.$m(eax).unwrap(); }, nat_r!($t32), false, &none);
        }};
    }
    un3!("neg", neg, "neg {a:l}", "neg {a:x}", "neg {a:e}");
    un3!("not", not, "not {a:l}", "not {a:x}", "not {a:e}");
    un3!("inc", inc, "inc {a:l}", "inc {a:x}", "inc {a:e}");
    un3!("dec", dec, "dec {a:l}", "dec {a:x}", "dec {a:e}");
}

/// Undefined flags of shifts by `count` for operand `size`.
fn shift_undef(size: u32, kind: &'static str) -> impl Fn(u32, u32) -> u32 {
    move |_a, count| {
        let c = count & 31;
        if c == 0 {
            return 0;
        }
        let bits = size * 8;
        let mut u = 0;
        match kind {
            "shift" => {
                u |= AF;
                if c != 1 {
                    u |= OF;
                }
            }
            "shlr" => {
                u |= AF;
                if c != 1 {
                    u |= OF;
                }
                if c >= bits {
                    u |= CF;
                }
            }
            _ => {
                // rotates
                if c != 1 {
                    u |= OF;
                }
            }
        }
        u
    }
}

#[test]
fn diff_shifts() {
    macro_rules! sh3 {
        ($name:literal, $m:ident, $kind:literal, $t8:literal, $t16:literal, $t32:literal) => {{
            check_bin($name, 1, &|a| { a.$m(al, cl).unwrap(); }, nat_cl!($t8), true, &shift_undef(1, $kind));
            check_bin($name, 2, &|a| { a.$m(ax, cl).unwrap(); }, nat_cl!($t16), true, &shift_undef(2, $kind));
            check_bin($name, 4, &|a| { a.$m(eax, cl).unwrap(); }, nat_cl!($t32), true, &shift_undef(4, $kind));
        }};
    }
    sh3!("shl", shl, "shlr", "shl {a:l}, cl", "shl {a:x}, cl", "shl {a:e}, cl");
    sh3!("shr", shr, "shlr", "shr {a:l}, cl", "shr {a:x}, cl", "shr {a:e}, cl");
    sh3!("sar", sar, "shift", "sar {a:l}, cl", "sar {a:x}, cl", "sar {a:e}, cl");
    sh3!("rol", rol, "rot", "rol {a:l}, cl", "rol {a:x}, cl", "rol {a:e}, cl");
    sh3!("ror", ror, "rot", "ror {a:l}, cl", "ror {a:x}, cl", "ror {a:e}, cl");
    sh3!("rcl", rcl, "rot", "rcl {a:l}, cl", "rcl {a:x}, cl", "rcl {a:e}, cl");
    sh3!("rcr", rcr, "rot", "rcr {a:l}, cl", "rcr {a:x}, cl", "rcr {a:e}, cl");
}

#[test]
fn diff_shifts_by_immediate() {
    // `shl eax, 1` (D1 /4) and `shl eax, imm8` (C1 /4) encodings.
    for n in [1u32, 3, 31] {
        for (name, kind) in [("shl", "shlr"), ("shr", "shlr"), ("sar", "shift"), ("rol", "rot"), ("ror", "rot")] {
            let build = move |a: &mut CodeAssembler| {
                match name {
                    "shl" => a.shl(eax, n).unwrap(),
                    "shr" => a.shr(eax, n).unwrap(),
                    "sar" => a.sar(eax, n).unwrap(),
                    "rol" => a.rol(eax, n).unwrap(),
                    _ => a.ror(eax, n).unwrap(),
                }
            };
            let native: Nat2 = match name {
                "shl" => nat_cl!("shl {a:e}, cl"),
                "shr" => nat_cl!("shr {a:e}, cl"),
                "sar" => nat_cl!("sar {a:e}, cl"),
                "rol" => nat_cl!("rol {a:e}, cl"),
                _ => nat_cl!("ror {a:e}, cl"),
            };
            let u = shift_undef(4, kind);
            for generic in [false, true] {
                let mut h = Harness::new(generic, &build);
                for &a in &values() {
                    for &fin in &FLAG_INS {
                        let (na, nf) = native(a, n, fin);
                        let (_, regs, f) = h.run(a, 0, 0, 0, fin);
                        let fm = ALL & !u(a, n);
                        assert_eq!(regs[EAX], na, "{name} {n} a={a:#x}");
                        assert_eq!(f & fm, nf & fm, "{name} {n} a={a:#x} fin={fin:#x}");
                    }
                }
            }
        }
    }
}

#[test]
fn diff_shld_shrd() {
    type Nat3 = fn(u32, u32, u32, u32) -> (u32, u32);
    let cases: [(&str, u32, &dyn Fn(&mut CodeAssembler), Nat3); 4] = [
        ("shld32", 4, &|a| a.shld(eax, ebx, cl).unwrap(), nat_rrcl!("shld {a:e}, {b:e}, cl")),
        ("shrd32", 4, &|a| a.shrd(eax, ebx, cl).unwrap(), nat_rrcl!("shrd {a:e}, {b:e}, cl")),
        ("shld16", 2, &|a| a.shld(ax, bx, cl).unwrap(), nat_rrcl!("shld {a:x}, {b:x}, cl")),
        ("shrd16", 2, &|a| a.shrd(ax, bx, cl).unwrap(), nat_rrcl!("shrd {a:x}, {b:x}, cl")),
    ];
    let vals = values();
    for (name, size, build, native) in cases {
        for generic in [false, true] {
            let mut h = Harness::new(generic, build);
            for &a in vals.iter().step_by(3) {
                for &b in vals.iter().step_by(2) {
                    for c in 0..=33u32 {
                        let mc = c & 31;
                        if mc > size * 8 {
                            continue; // result undefined
                        }
                        let fin = CF;
                        let (na, nf) = native(a, b, c, fin);
                        let (_, regs, f) = h.run(a, c, 0, b, fin);
                        let mut u = 0;
                        if mc != 0 {
                            u |= AF;
                            if mc != 1 {
                                u |= OF;
                            }
                            if mc == size * 8 {
                                u |= CF | OF; // count == size (16-bit): CF/OF undefined in practice
                            }
                        }
                        let fm = ALL & !u;
                        assert_eq!(regs[EAX], na, "{name} a={a:#x} b={b:#x} c={c}");
                        assert_eq!(f & fm, nf & fm, "{name} flags a={a:#x} b={b:#x} c={c}");
                    }
                }
            }
        }
    }
}

#[test]
fn diff_imul2_bsf_bsr_bt() {
    let mul_undef = |_: u32, _: u32| SF | ZF | AF | PF;
    check_bin("imul16", 2, &|a| { a.imul_2(ax, bx).unwrap(); }, nat_rr!("imul {a:x}, {b:x}"), false, &mul_undef);
    check_bin("imul32", 4, &|a| { a.imul_2(eax, ebx).unwrap(); }, nat_rr!("imul {a:e}, {b:e}"), false, &mul_undef);
    let bs_undef = |_: u32, b: u32| (ALL & !ZF) | if b == 0 { RES } else { 0 };
    let bs16_undef = |_: u32, b: u32| (ALL & !ZF) | if b & 0xFFFF == 0 { RES } else { 0 };
    check_bin("bsf32", 4, &|a| { a.bsf(eax, ebx).unwrap(); }, nat_rr!("bsf {a:e}, {b:e}"), false, &bs_undef);
    check_bin("bsr32", 4, &|a| { a.bsr(eax, ebx).unwrap(); }, nat_rr!("bsr {a:e}, {b:e}"), false, &bs_undef);
    check_bin("bsf16", 2, &|a| { a.bsf(ax, bx).unwrap(); }, nat_rr!("bsf {a:x}, {b:x}"), false, &bs16_undef);
    check_bin("bsr16", 2, &|a| { a.bsr(ax, bx).unwrap(); }, nat_rr!("bsr {a:x}, {b:x}"), false, &bs16_undef);
    let bt_undef = |_: u32, _: u32| OF | SF | AF | PF | ZF;
    check_bin("bt32", 4, &|a| { a.bt(eax, ebx).unwrap(); }, nat_rr!("bt {a:e}, {b:e}"), false, &bt_undef);
    check_bin("bts32", 4, &|a| { a.bts(eax, ebx).unwrap(); }, nat_rr!("bts {a:e}, {b:e}"), false, &bt_undef);
    check_bin("btr32", 4, &|a| { a.btr(eax, ebx).unwrap(); }, nat_rr!("btr {a:e}, {b:e}"), false, &bt_undef);
    check_bin("btc32", 4, &|a| { a.btc(eax, ebx).unwrap(); }, nat_rr!("btc {a:e}, {b:e}"), false, &bt_undef);
    check_bin("bt16", 2, &|a| { a.bt(ax, bx).unwrap(); }, nat_rr!("bt {a:x}, {b:x}"), false, &bt_undef);
    check_bin("bts16", 2, &|a| { a.bts(ax, bx).unwrap(); }, nat_rr!("bts {a:x}, {b:x}"), false, &bt_undef);
}

#[test]
fn diff_mul_div() {
    type NatMD = fn(u32, u32, u32, u32) -> (u32, u32, u32);
    let vals = values();
    let cases: [(&str, u32, &dyn Fn(&mut CodeAssembler), NatMD); 12] = [
        ("mul8", 1, &|a| a.mul(bl).unwrap(), nat_md!("mul {b:l}")),
        ("mul16", 2, &|a| a.mul(bx).unwrap(), nat_md!("mul {b:x}")),
        ("mul32", 4, &|a| a.mul(ebx).unwrap(), nat_md!("mul {b:e}")),
        ("imul8", 1, &|a| a.imul(bl).unwrap(), nat_md!("imul {b:l}")),
        ("imul16", 2, &|a| a.imul(bx).unwrap(), nat_md!("imul {b:x}")),
        ("imul32", 4, &|a| a.imul(ebx).unwrap(), nat_md!("imul {b:e}")),
        ("div8", 1, &|a| a.div(bl).unwrap(), nat_md!("div {b:l}")),
        ("div16", 2, &|a| a.div(bx).unwrap(), nat_md!("div {b:x}")),
        ("div32", 4, &|a| a.div(ebx).unwrap(), nat_md!("div {b:e}")),
        ("idiv8", 1, &|a| a.idiv(bl).unwrap(), nat_md!("idiv {b:l}")),
        ("idiv16", 2, &|a| a.idiv(bx).unwrap(), nat_md!("idiv {b:x}")),
        ("idiv32", 4, &|a| a.idiv(ebx).unwrap(), nat_md!("idiv {b:e}")),
    ];
    for (name, size, build, native) in cases {
        let is_div = name.contains("div");
        let signed = name.starts_with('i');
        for generic in [false, true] {
            let mut h = Harness::new(generic, build);
            for &a in &vals {
                for &d in vals.iter().step_by(4) {
                    for &b in &vals {
                        // Would the native division fault? Then the interpreter must raise #DE.
                        if is_div {
                            let m = sz_mask(size);
                            let bad = if b & m == 0 {
                                true
                            } else if !signed {
                                let n: u64 = match size {
                                    1 => (a & 0xFFFF) as u64,
                                    2 => (((d & 0xFFFF) << 16) | (a & 0xFFFF)) as u64,
                                    _ => ((d as u64) << 32) | a as u64,
                                };
                                n / (b & m) as u64 > m as u64
                            } else {
                                let n: i64 = match size {
                                    1 => a as u16 as i16 as i64,
                                    2 => ((((d & 0xFFFF) << 16) | (a & 0xFFFF)) as i32) as i64,
                                    _ => (((d as u64) << 32) | a as u64) as i64,
                                };
                                let dv: i64 = match size {
                                    1 => b as u8 as i8 as i64,
                                    2 => b as u16 as i16 as i64,
                                    _ => b as i32 as i64,
                                };
                                let q = n.checked_div(dv);
                                let (lo, hi) = match size {
                                    1 => (-128i64, 127i64),
                                    2 => (-32768, 32767),
                                    _ => (i32::MIN as i64, i32::MAX as i64),
                                };
                                !matches!(q, Some(q) if q >= lo && q <= hi)
                            };
                            if bad {
                                let (exit, _, _) = h.run(a, 0, d, b, 0);
                                assert!(
                                    matches!(exit, CpuExit::Fault { kind: FaultKind::DivideError, .. }),
                                    "{name} a={a:#x} d={d:#x} b={b:#x}: {exit:?}"
                                );
                                continue;
                            }
                        }
                        let (na, nd, nf) = native(a, d, b, 0);
                        let (exit, regs, f) = h.run(a, 0, d, b, 0);
                        assert_eq!(exit, CpuExit::ReturnMagic);
                        let ctx = format!("{name} generic={generic} a={a:#x} d={d:#x} b={b:#x}");
                        assert_eq!(regs[EAX], na, "eax {ctx}");
                        assert_eq!(regs[EDX], nd, "edx {ctx}");
                        if !is_div {
                            let fm = CF | OF;
                            assert_eq!(f & fm, nf & fm, "flags {ctx}");
                        }
                    }
                }
            }
        }
    }
}
