//! x87 tests: hand-assembled snippets (iced-x86 `code_asm`), no game bytes.

use iced_x86::code_asm::*;

use crate::fpu::sw;
use crate::*;

const CODE: u32 = 0x0040_1000;
const STACK_TOP: u32 = 0x0090_0000;
const D: u32 = 0x0060_0000;

fn run_with(init: impl FnOnce(&mut GuestMemory), build: impl FnOnce(&mut CodeAssembler)) -> (Cpu, GuestMemory) {
    let mut a = CodeAssembler::new(32).unwrap();
    build(&mut a);
    a.ret().unwrap();
    let bytes = a.assemble(CODE as u64).unwrap();
    let mut mem = GuestMemory::new().unwrap();
    mem.write_bytes(CODE, &bytes);
    init(&mut mem);
    let mut cpu = Cpu::new();
    cpu.regs[ESP] = STACK_TOP;
    cpu.push32(&mut mem, layout::MAGIC_RETURN);
    cpu.eip = CODE;
    let exit = cpu.run(&mut mem, 1_000_000);
    assert_eq!(exit, CpuExit::ReturnMagic, "eip={:#x}", cpu.eip);
    (cpu, mem)
}

fn d(off: u32) -> AsmMemoryOperand {
    ptr(D as u64 + off as u64)
}

fn put_f64(m: &mut GuestMemory, off: u32, v: f64) {
    m.write_u64(D + off, v.to_bits());
}

#[test]
fn load_add_store_and_stack() {
    let (cpu, mem) = run_with(
        |m| {
            put_f64(m, 0, 1.5);
            m.write_u32(D + 8, 2.25f32.to_bits());
            m.write_u32(D + 12, (-7i32) as u32);
        },
        |a| {
            a.fld(qword_ptr(d(0))).unwrap(); // 1.5
            a.fadd(dword_ptr(d(8))).unwrap(); // 3.75
            a.fild(dword_ptr(d(12))).unwrap(); // -7, 3.75
            a.fmulp(st1, st0).unwrap(); // -26.25
            a.fld1().unwrap();
            a.fsubrp(st1, st0).unwrap(); // st1 = st0 - st1 -> 1 - (-26.25) = 27.25
            a.fst(qword_ptr(d(16))).unwrap();
            a.fstp(tword_ptr(d(32))).unwrap();
        },
    );
    assert_eq!(f64::from_bits(mem.read_u64(D + 16)), 27.25);
    assert_eq!(mem.read_u16(D + 40), 0x4003);
    assert_eq!(mem.read_u64(D + 32), 0xDA00_0000_0000_0000);
    assert_eq!(cpu.fpu.top, 0);
    assert_eq!(cpu.fpu.tags, 0xFFFF);
}

#[test]
fn ftol_truncation_and_rounding_modes() {
    // MSVC6 _ftol: fnstcw; or ah,0x0c; fldcw; fistp qword; fldcw
    let (_, mem) = run_with(
        |m| {
            put_f64(m, 0, -2.7);
            put_f64(m, 8, 2.5);
        },
        |a| {
            a.fld(qword_ptr(d(0))).unwrap();
            a.fnstcw(word_ptr(d(0x40))).unwrap();
            a.mov(ax, word_ptr(d(0x40))).unwrap();
            a.or(ah, 0x0c).unwrap();
            a.mov(word_ptr(d(0x42)), ax).unwrap();
            a.fldcw(word_ptr(d(0x42))).unwrap();
            a.fistp(qword_ptr(d(0x50))).unwrap();
            a.fldcw(word_ptr(d(0x40))).unwrap();
            // Default round-to-nearest-even: 2.5 -> 2
            a.fld(qword_ptr(d(8))).unwrap();
            a.fist(dword_ptr(d(0x60))).unwrap();
            a.frndint().unwrap();
            a.fistp(word_ptr(d(0x64))).unwrap();
            // Overflow -> integer indefinite
            a.fld(qword_ptr(d(0x70))).unwrap();
            a.fistp(word_ptr(d(0x66))).unwrap();
        },
    );
    assert_eq!(mem.read_u64(D + 0x50) as i64, -2);
    assert_eq!(mem.read_u32(D + 0x60), 2);
    assert_eq!(mem.read_u16(D + 0x64), 2);
    // d(0x70) is 0.0 (fresh memory) -> 0, not overflow; check a real overflow separately below.
    assert_eq!(mem.read_u16(D + 0x66), 0);
    let (cpu, mem) = run_with(
        |m| put_f64(m, 0, 70000.0),
        |a| {
            a.fld(qword_ptr(d(0))).unwrap();
            a.fistp(word_ptr(d(8))).unwrap();
        },
    );
    assert_eq!(mem.read_u16(D + 8), 0x8000);
    assert!(cpu.fpu.status & sw::IE != 0);
}

#[test]
fn fcomp_fnstsw_sahf_pattern() {
    // if (a < b) ebx = 1 ; if (a == a) ecx = 1 ; unordered -> PF
    let (cpu, _) = run_with(
        |m| {
            put_f64(m, 0, 1.0);
            put_f64(m, 8, 2.0);
            put_f64(m, 16, f64::NAN);
        },
        |a| {
            let mut l1 = a.create_label();
            let mut l2 = a.create_label();
            a.xor(ebx, ebx).unwrap();
            a.xor(ecx, ecx).unwrap();
            a.xor(edx, edx).unwrap();
            a.fld(qword_ptr(d(0))).unwrap();
            a.fcomp(qword_ptr(d(8))).unwrap();
            a.fnstsw(ax).unwrap();
            a.sahf().unwrap();
            a.jae(l1).unwrap();
            a.mov(ebx, 1).unwrap();
            a.set_label(&mut l1).unwrap();
            a.fld(qword_ptr(d(8))).unwrap();
            a.fld(qword_ptr(d(8))).unwrap();
            a.fcompp().unwrap();
            a.fnstsw(ax).unwrap();
            a.test(ah, 0x40).unwrap();
            a.jz(l2).unwrap();
            a.mov(ecx, 1).unwrap();
            a.set_label(&mut l2).unwrap();
            a.fld(qword_ptr(d(16))).unwrap();
            a.fld(qword_ptr(d(0))).unwrap();
            a.fxch(st0, st1).unwrap();
            a.fucompp().unwrap();
            a.fnstsw(ax).unwrap();
            a.sahf().unwrap();
            a.setp(dl).unwrap();
        },
    );
    assert_eq!(cpu.regs[EBX], 1);
    assert_eq!(cpu.regs[ECX], 1);
    assert_eq!(cpu.regs[EDX], 1);
    assert_eq!(cpu.fpu.top, 0);
    assert_eq!(cpu.fpu.status & sw::IE, 0, "fucom with QNaN must not raise IE");
}

#[test]
fn fcomi_sets_eflags() {
    let (cpu, _) = run_with(
        |m| {
            put_f64(m, 0, 3.0);
            put_f64(m, 8, 2.0);
        },
        |a| {
            a.fld(qword_ptr(d(8))).unwrap(); // st1 = 2
            a.fld(qword_ptr(d(0))).unwrap(); // st0 = 3
            a.fcomip(st0, st1).unwrap();
            a.seta(bl).unwrap();
            a.fstp(st0).unwrap();
        },
    );
    assert_eq!(cpu.regs[EBX] & 0xFF, 1);
    assert!(!cpu.flag(fl::ZF) && !cpu.flag(fl::CF) && !cpu.flag(fl::PF));
    assert_eq!(cpu.fpu.top, 0);
}

#[test]
fn constants_and_transcendentals() {
    let (_, mem) = run_with(
        |m| {
            put_f64(m, 0, 0.5);
            put_f64(m, 8, 8.0);
        },
        |a| {
            a.fldpi().unwrap();
            a.fstp(qword_ptr(d(0x20))).unwrap();
            a.fld(qword_ptr(d(0))).unwrap();
            a.fsin().unwrap();
            a.fstp(qword_ptr(d(0x28))).unwrap();
            // log2(8) * 1 via fyl2x
            a.fld1().unwrap();
            a.fld(qword_ptr(d(8))).unwrap();
            a.fyl2x().unwrap();
            a.fstp(qword_ptr(d(0x30))).unwrap();
            // 2^0.5 - 1 via f2xm1
            a.fld(qword_ptr(d(0))).unwrap();
            a.f2xm1().unwrap();
            a.fstp(qword_ptr(d(0x38))).unwrap();
            // fscale: 3 * 2^4
            a.fld(qword_ptr(d(8))).unwrap();
            a.fsqrt().unwrap(); // sqrt(8)
            a.fstp(qword_ptr(d(0x40))).unwrap();
            a.mov(dword_ptr(d(0x48)), 4).unwrap();
            a.fild(dword_ptr(d(0x48))).unwrap();
            a.mov(dword_ptr(d(0x48)), 3).unwrap();
            a.fild(dword_ptr(d(0x48))).unwrap();
            a.fscale().unwrap();
            a.fstp(qword_ptr(d(0x50))).unwrap();
            a.fstp(st0).unwrap();
            // fpatan: atan2(1, 1)
            a.fld1().unwrap();
            a.fld1().unwrap();
            a.fpatan().unwrap();
            a.fstp(qword_ptr(d(0x58))).unwrap();
        },
    );
    let f = |o: u32| f64::from_bits(mem.read_u64(D + o));
    assert_eq!(f(0x20), std::f64::consts::PI);
    assert_eq!(f(0x28), 0.5f64.sin());
    assert_eq!(f(0x30), 3.0);
    assert!((f(0x38) - (2f64.sqrt() - 1.0)).abs() < 1e-15);
    assert_eq!(f(0x40), 8f64.sqrt());
    assert_eq!(f(0x50), 48.0);
    assert_eq!(f(0x58), std::f64::consts::FRAC_PI_4);
}

#[test]
fn fprem_loop_and_fxam() {
    // CRT fmod pattern: loop fprem until C2 clear.
    let (cpu, mem) = run_with(
        |m| {
            put_f64(m, 0, 3.0);
            put_f64(m, 8, 17.5);
        },
        |a| {
            let mut again = a.create_label();
            a.fld(qword_ptr(d(0))).unwrap(); // divisor
            a.fld(qword_ptr(d(8))).unwrap(); // dividend
            a.set_label(&mut again).unwrap();
            a.fprem().unwrap();
            a.fnstsw(ax).unwrap();
            a.sahf().unwrap();
            a.jp(again).unwrap();
            a.fstp(qword_ptr(d(0x10))).unwrap();
            a.fstp(st0).unwrap();
            a.fldz().unwrap();
            a.fchs().unwrap();
            a.fxam().unwrap();
            a.fnstsw(ax).unwrap();
            a.mov(ebx, eax).unwrap();
            a.fstp(st0).unwrap();
            a.fxam().unwrap(); // empty
            a.fnstsw(ax).unwrap();
        },
    );
    assert_eq!(f64::from_bits(mem.read_u64(D + 0x10)), 2.5);
    // -0: C3=1, C1=1 (sign), C2=0, C0=0
    assert_eq!(cpu.regs[EBX] as u16 & (sw::CC), sw::C3 | sw::C1);
    // empty: C3=1, C0=1
    assert_eq!(cpu.regs[EAX] as u16 & (sw::C3 | sw::C2 | sw::C0), sw::C3 | sw::C0);
    // 17.5 / 3 -> q = 5 = 0b101: C0=Q2=1, C3=Q1=0, C1=Q0=1 checked via fprem1 below.
    let (cpu, mem) = run_with(
        |m| {
            put_f64(m, 0, 3.0);
            put_f64(m, 8, 17.5);
        },
        |a| {
            a.fld(qword_ptr(d(0))).unwrap();
            a.fld(qword_ptr(d(8))).unwrap();
            a.fprem1().unwrap(); // 17.5 - 6*3 = -0.5, q = 6 = 0b110
            a.fnstsw(ax).unwrap();
            a.fstp(qword_ptr(d(0x10))).unwrap();
            a.fstp(st0).unwrap();
        },
    );
    assert_eq!(f64::from_bits(mem.read_u64(D + 0x10)), -0.5);
    let s = cpu.regs[EAX] as u16;
    assert_eq!(s & (sw::C0 | sw::C3 | sw::C1 | sw::C2), sw::C0 | sw::C3);
}

#[test]
fn fnsave_frstor_and_fnstenv() {
    let (cpu, mem) = run_with(
        |m| put_f64(m, 0, 6.5),
        |a| {
            a.fld(qword_ptr(d(0))).unwrap();
            a.fld1().unwrap();
            a.fnsave(ptr(d(0x100))).unwrap();
            // After fnsave: fninit state
            a.fnstsw(ax).unwrap();
            a.mov(ebx, eax).unwrap();
            a.frstor(ptr(d(0x100))).unwrap();
            a.faddp(st1, st0).unwrap(); // 7.5
            a.fnstenv(ptr(d(0x200))).unwrap();
            a.fstp(qword_ptr(d(8))).unwrap();
        },
    );
    assert_eq!(cpu.regs[EBX] & 0xFFFF, 0);
    assert_eq!(f64::from_bits(mem.read_u64(D + 8)), 7.5);
    // Saved image: CW 0x37F, TOP=6, st0 = 1.0 in 80-bit at +28.
    assert_eq!(mem.read_u16(D + 0x100), 0x037F);
    assert_eq!((mem.read_u16(D + 0x104) >> 11) & 7, 6);
    assert_eq!(mem.read_u64(D + 0x100 + 28), 0x8000_0000_0000_0000);
    assert_eq!(mem.read_u16(D + 0x100 + 36), 0x3FFF);
    // Tag word in the save image: R6 = valid (1.0), R7 = valid (6.5), others empty.
    assert_eq!(mem.read_u16(D + 0x108), 0x0FFF);
    // fnstenv: TOP = 7, one valid reg (R7), then exceptions masked.
    assert_eq!((mem.read_u16(D + 0x204) >> 11) & 7, 7);
    assert_eq!(mem.read_u16(D + 0x208), 0x3FFF);
    assert_eq!(cpu.fpu.control & 0x3F, 0x3F);
}

#[test]
fn stack_overflow_and_underflow_flags() {
    let (cpu, _) = run_with(
        |_| {},
        |a| {
            for _ in 0..9 {
                a.fld1().unwrap();
            }
        },
    );
    assert!(cpu.fpu.status & sw::SF != 0 && cpu.fpu.status & sw::IE != 0 && cpu.fpu.status & sw::C1 != 0);
    assert!(cpu.fpu.st(0).is_nan());
    let (cpu, _) = run_with(
        |_| {},
        |a| {
            a.fninit().unwrap();
            a.fchs().unwrap();
            a.fnclex().unwrap();
            a.fldz().unwrap();
            a.fld1().unwrap();
            a.fdivrp(st1, st0).unwrap(); // st1 = 1 / 0 => ZE
        },
    );
    assert_eq!(cpu.fpu.status & sw::SF, 0);
    assert!(cpu.fpu.status & sw::ZE != 0);
    assert_eq!(cpu.fpu.st(0), f64::INFINITY);
}

#[test]
fn precision_control_24_bits() {
    let (_, mem) = run_with(
        |m| {
            m.write_u16(D, 0x007F); // PC = 24 bits
            put_f64(m, 8, 1.0);
            put_f64(m, 16, 2f64.powi(-30));
        },
        |a| {
            a.fldcw(word_ptr(d(0))).unwrap();
            a.fld(qword_ptr(d(8))).unwrap();
            a.fadd(qword_ptr(d(16))).unwrap();
            a.fstp(qword_ptr(d(24))).unwrap();
        },
    );
    assert_eq!(f64::from_bits(mem.read_u64(D + 24)), 1.0);
}

#[test]
fn integer_ops_bcd_fxtract() {
    let (cpu, mem) = run_with(
        |m| {
            m.write_u16(D, 10);
            m.write_u32(D + 4, 4);
            put_f64(m, 8, 100.0);
        },
        |a| {
            a.fld(qword_ptr(d(8))).unwrap();
            a.fidiv(dword_ptr(d(4))).unwrap(); // 25
            a.fisub(word_ptr(d(0))).unwrap(); // 15
            a.ficom(word_ptr(d(0))).unwrap(); // 15 > 10 -> CC 000
            a.fnstsw(ax).unwrap();
            a.fbstp(ptr(d(0x10))).unwrap();
            a.fbld(ptr(d(0x10))).unwrap();
            a.fxtract().unwrap(); // st0 = 1.875, st1 = 3
            a.fstp(qword_ptr(d(0x20))).unwrap();
            a.fistp(dword_ptr(d(0x28))).unwrap();
        },
    );
    assert_eq!(cpu.regs[EAX] as u16 & sw::CC, 0);
    assert_eq!(mem.read_u8(D + 0x10), 0x15);
    assert_eq!(f64::from_bits(mem.read_u64(D + 0x20)), 1.875);
    assert_eq!(mem.read_u32(D + 0x28), 3);
    assert_eq!(cpu.fpu.top, 0);
}

#[test]
fn mmx_movq_copy() {
    let (cpu, mem) = run_with(
        |m| m.write_u64(D, 0x1122_3344_5566_7788),
        |a| {
            a.movq(mm0, qword_ptr(d(0))).unwrap();
            a.movq(mm1, mm0).unwrap();
            a.movq(qword_ptr(d(8)), mm1).unwrap();
            a.movd(eax, mm1).unwrap();
            a.emms().unwrap();
        },
    );
    assert_eq!(mem.read_u64(D + 8), 0x1122_3344_5566_7788);
    assert_eq!(cpu.regs[EAX], 0x5566_7788);
    assert_eq!(cpu.fpu.tags, 0xFFFF);
}
