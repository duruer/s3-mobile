//! CPU tests: hand-assembled snippets (iced-x86 `code_asm`), no game bytes.

use iced_x86::code_asm::*;

use crate::*;

const CODE: u32 = 0x0040_1000;
const STACK_TOP: u32 = 0x0090_0000;
const DATA: u32 = 0x0060_0000;

fn setup(build: impl FnOnce(&mut CodeAssembler)) -> (Cpu, GuestMemory) {
    let mut a = CodeAssembler::new(32).unwrap();
    build(&mut a);
    a.ret().unwrap();
    let bytes = a.assemble(CODE as u64).unwrap();
    let mut mem = GuestMemory::new().unwrap();
    mem.write_bytes(CODE, &bytes);
    let mut cpu = Cpu::new();
    cpu.regs[ESP] = STACK_TOP;
    cpu.push32(&mut mem, layout::MAGIC_RETURN);
    cpu.eip = CODE;
    (cpu, mem)
}

fn run(build: impl FnOnce(&mut CodeAssembler)) -> (Cpu, GuestMemory) {
    let (mut cpu, mut mem) = setup(build);
    let exit = cpu.run(&mut mem, 1_000_000);
    assert_eq!(exit, CpuExit::ReturnMagic, "eip={:#x}", cpu.eip);
    assert_eq!(cpu.regs[ESP], STACK_TOP);
    (cpu, mem)
}

#[test]
fn mov_add_sub_flags() {
    let (cpu, _) = run(|a| {
        a.mov(eax, 0xFFFF_FFFFu32).unwrap();
        a.add(eax, 1).unwrap(); // CF=1 ZF=1
        a.mov(ebx, 5).unwrap();
        a.sub(ebx, 7).unwrap(); // -2, CF=1 SF=1
        a.mov(ecx, 0x7FFF_FFFF).unwrap();
        a.inc(ecx).unwrap(); // OF=1, CF preserved (1 from sub)
    });
    assert_eq!(cpu.regs[EAX], 0);
    assert_eq!(cpu.regs[EBX], (-2i32) as u32);
    assert_eq!(cpu.regs[ECX], 0x8000_0000);
    assert!(cpu.flag(fl::OF));
    assert!(cpu.flag(fl::CF));
    assert!(cpu.flag(fl::SF));
    assert!(!cpu.flag(fl::ZF));
}

#[test]
fn partial_registers_and_extend() {
    let (cpu, _) = run(|a| {
        a.mov(eax, 0x1234_5678).unwrap();
        a.mov(ah, 0xAB).unwrap();
        a.mov(al, 0xCD).unwrap();
        a.mov(bx, 0xFFFF).unwrap();
        a.movsx(ecx, bx).unwrap();
        a.movzx(edx, ah).unwrap();
        a.mov(esi, 0x80).unwrap();
        a.movsx(edi, si).unwrap(); // 0x80 as i16 is positive
    });
    assert_eq!(cpu.regs[EAX], 0x1234_ABCD);
    assert_eq!(cpu.regs[EBX] & 0xFFFF, 0xFFFF);
    assert_eq!(cpu.regs[ECX], 0xFFFF_FFFF);
    assert_eq!(cpu.regs[EDX], 0xAB);
    assert_eq!(cpu.regs[EDI], 0x80);
}

#[test]
fn memory_lea_push_pop_call() {
    let (cpu, mem) = run(|a| {
        let mut func = a.create_label();
        let mut done = a.create_label();
        a.mov(ebx, DATA).unwrap();
        a.mov(dword_ptr(ebx + 8), 41).unwrap();
        a.lea(esi, ptr(ebx + ecx * 4 + 8)).unwrap(); // ecx = 0
        a.push(dword_ptr(esi)).unwrap();
        a.call(func).unwrap();
        a.add(esp, 4).unwrap();
        a.jmp(done).unwrap();
        a.set_label(&mut func).unwrap();
        a.mov(eax, dword_ptr(esp + 4)).unwrap();
        a.inc(eax).unwrap();
        a.mov(dword_ptr(ebx + 12), eax).unwrap();
        a.ret().unwrap();
        a.set_label(&mut done).unwrap();
        a.nop().unwrap();
    });
    assert_eq!(cpu.regs[ESI], DATA + 8);
    assert_eq!(cpu.regs[EAX], 42);
    assert_eq!(mem.read_u32(DATA + 12), 42);
}

#[test]
fn loop_with_jcc_sum() {
    // sum 1..=100 with cmp/jle
    let (cpu, _) = run(|a| {
        let mut top = a.create_label();
        a.xor(eax, eax).unwrap();
        a.mov(ecx, 1).unwrap();
        a.set_label(&mut top).unwrap();
        a.add(eax, ecx).unwrap();
        a.inc(ecx).unwrap();
        a.cmp(ecx, 100).unwrap();
        a.jle(top).unwrap();
    });
    assert_eq!(cpu.regs[EAX], 5050);
}

#[test]
fn logic_and_test_and_signed_conditions() {
    let (cpu, _) = run(|a| {
        a.mov(eax, 0xF0F0).unwrap();
        a.and(eax, 0x0FF0).unwrap();
        a.or(eax, 1).unwrap();
        a.xor(eax, 0x10).unwrap();
        a.mov(ebx, -5i32).unwrap();
        a.cmp(ebx, 3).unwrap();
        a.setl(cl).unwrap(); // -5 < 3 signed
        a.setb(dl).unwrap(); // unsigned 0xFFFFFFFB < 3 false
        a.test(eax, eax).unwrap();
        a.setne(dh).unwrap();
    });
    assert_eq!(cpu.regs[EAX], 0x00E1);
    assert_eq!(cpu.regs[ECX] & 0xFF, 1);
    assert_eq!(cpu.regs[EDX] & 0xFF, 0);
    assert_eq!((cpu.regs[EDX] >> 8) & 0xFF, 1);
}

#[test]
fn shifts_mul_div() {
    let (cpu, _) = run(|a| {
        a.mov(eax, 1).unwrap();
        a.shl(eax, 4).unwrap(); // 16
        a.mov(ebx, -64i32).unwrap();
        a.sar(ebx, 2).unwrap(); // -16
        a.mov(ecx, 0x8000_0001u32).unwrap();
        a.rol(ecx, 1).unwrap(); // 3, CF=1
        a.mov(esi, 7).unwrap();
        a.imul_3(edi, esi, 6).unwrap(); // 42
        a.mul(esi).unwrap(); // edx:eax = 16*7 = 112
        a.mov(esi, 10).unwrap();
        a.xor(edx, edx).unwrap();
        a.div(esi).unwrap(); // eax = 11, edx = 2
    });
    assert_eq!(cpu.regs[EAX], 11);
    assert_eq!(cpu.regs[EDX], 2);
    assert_eq!(cpu.regs[EBX], (-16i32) as u32);
    assert_eq!(cpu.regs[ECX], 3);
    assert_eq!(cpu.regs[EDI], 42);
}

#[test]
fn idiv_negative_and_cdq() {
    let (cpu, _) = run(|a| {
        a.mov(eax, -7i32).unwrap();
        a.cdq().unwrap();
        a.mov(ecx, 2).unwrap();
        a.idiv(ecx).unwrap();
    });
    assert_eq!(cpu.regs[EAX] as i32, -3);
    assert_eq!(cpu.regs[EDX] as i32, -1);
}

#[test]
fn divide_by_zero_faults_at_instruction() {
    let (mut cpu, mut mem) = setup(|a| {
        a.xor(ecx, ecx).unwrap();
        a.div(ecx).unwrap();
    });
    match cpu.run(&mut mem, 100) {
        CpuExit::Fault { eip, kind: FaultKind::DivideError, .. } => {
            assert_eq!(eip, CODE + 2);
            assert_eq!(cpu.eip, CODE + 2);
        }
        e => panic!("unexpected {e:?}"),
    }
}

#[test]
fn null_pointer_faults() {
    let (mut cpu, mut mem) = setup(|a| {
        a.xor(eax, eax).unwrap();
        a.mov(ebx, dword_ptr(eax + 4)).unwrap();
    });
    match cpu.run(&mut mem, 100) {
        CpuExit::Fault { kind: FaultKind::AccessViolation { addr: 4, write: false }, .. } => {}
        e => panic!("unexpected {e:?}"),
    }
}

#[test]
fn rep_string_ops() {
    let (cpu, mem) = run(|a| {
        a.cld().unwrap();
        a.mov(edi, DATA).unwrap();
        a.mov(eax, 0x4141_4141).unwrap();
        a.mov(ecx, 16).unwrap();
        a.rep().stosd().unwrap();
        a.mov(esi, DATA).unwrap();
        a.mov(edi, DATA + 0x100).unwrap();
        a.mov(ecx, 64).unwrap();
        a.rep().movsb().unwrap();
        // strlen-style scan
        a.mov(byte_ptr(DATA + 0x100 + 10), 0).unwrap();
        a.mov(edi, DATA + 0x100).unwrap();
        a.xor(eax, eax).unwrap();
        a.mov(ecx, 0xFFFF_FFFFu32).unwrap();
        a.repne().scasb().unwrap();
        a.not(ecx).unwrap();
        a.dec(ecx).unwrap(); // length
    });
    assert_eq!(mem.read_u32(DATA + 60), 0x4141_4141);
    assert_eq!(mem.read_u8(DATA + 0x100 + 63), 0x41);
    assert_eq!(cpu.regs[ECX], 10);
}

#[test]
fn overlapping_movsb_replicates() {
    let (_, mem) = run(|a| {
        a.mov(byte_ptr(DATA), 0x5A).unwrap();
        a.mov(esi, DATA).unwrap();
        a.mov(edi, DATA + 1).unwrap();
        a.mov(ecx, 15).unwrap();
        a.rep().movsb().unwrap();
    });
    assert!(mem.slice(DATA, 16).iter().all(|&b| b == 0x5A));
}

#[test]
fn enter_leave_pushad_popad_xchg_bswap() {
    let (cpu, _) = run(|a| {
        a.mov(ebp, 0x1234).unwrap();
        a.enter(16, 0).unwrap();
        a.mov(dword_ptr(ebp - 4), 99).unwrap();
        a.mov(eax, 7).unwrap();
        a.pushad().unwrap();
        a.mov(eax, 0).unwrap();
        a.popad().unwrap();
        a.mov(edx, dword_ptr(ebp - 4)).unwrap();
        a.leave().unwrap();
        a.mov(ecx, 0x1122_3344).unwrap();
        a.bswap(ecx).unwrap();
        a.xchg(eax, ecx).unwrap();
    });
    assert_eq!(cpu.regs[ECX], 7);
    assert_eq!(cpu.regs[EAX], 0x4433_2211);
    assert_eq!(cpu.regs[EDX], 99);
    assert_eq!(cpu.regs[EBP], 0x1234);
}

#[test]
fn thunk_exit_and_fs_segment() {
    let (mut cpu, mut mem) = setup(|a| {
        a.mov(eax, dword_ptr(0x18).fs()).unwrap();
        a.push(5).unwrap();
        a.mov(ecx, layout::THUNK_BASE + 0x10).unwrap();
        a.call(ecx).unwrap();
    });
    cpu.fs_base = 0x7FFD_E000;
    mem.write_u32(0x7FFD_E018, 0x7FFD_E000);
    let exit = cpu.run(&mut mem, 100);
    assert_eq!(exit, CpuExit::Thunk(layout::THUNK_BASE + 0x10));
    assert_eq!(cpu.regs[EAX], 0x7FFD_E000);
    // Return address on top, argument after it.
    assert_eq!(mem.read_u32(cpu.regs[ESP] + 4), 5);
}

#[test]
fn budget_and_cmpxchg_xadd_bt() {
    let (cpu, mem) = run(|a| {
        a.mov(dword_ptr(DATA), 10).unwrap();
        a.mov(eax, 10).unwrap();
        a.mov(ecx, 20).unwrap();
        a.cmpxchg(dword_ptr(DATA), ecx).unwrap(); // equal -> [DATA]=20
        a.mov(edx, 5).unwrap();
        a.xadd(dword_ptr(DATA), edx).unwrap(); // [DATA]=25, edx=20
        a.mov(ebx, 0b1000).unwrap();
        a.bt(ebx, 3).unwrap(); // CF=1
        a.setb(al).unwrap();
        a.bts(ebx, 0).unwrap();
        a.bsr(esi, ebx).unwrap();
    });
    assert_eq!(mem.read_u32(DATA), 25);
    assert_eq!(cpu.regs[EDX], 20);
    assert_eq!(cpu.regs[EAX] & 0xFF, 1);
    assert_eq!(cpu.regs[EBX], 0b1001);
    assert_eq!(cpu.regs[ESI], 3);

    let (mut cpu, mut mem) = setup(|a| {
        let mut l = a.create_label();
        a.set_label(&mut l).unwrap();
        a.jmp(l).unwrap();
    });
    assert_eq!(cpu.run(&mut mem, 1000), CpuExit::Budget);
    assert_eq!(cpu.instructions, 1000);
}

#[test]
fn unimplemented_reports_mnemonic() {
    let (mut cpu, mut mem) = setup(|a| {
        a.addps(xmm0, xmm1).unwrap(); // SSE: out of scope (S3.EXE has none)
    });
    match cpu.run(&mut mem, 10) {
        CpuExit::Fault { kind: FaultKind::Unimplemented, detail, .. } => assert!(detail.contains("Addps"), "{detail}"),
        e => panic!("unexpected {e:?}"),
    }
}

#[test]
fn cache_invalidation_sees_new_code() {
    let (mut cpu, mut mem) = setup(|a| {
        a.mov(eax, 1).unwrap();
    });
    assert_eq!(cpu.run(&mut mem, 100), CpuExit::ReturnMagic);
    assert_eq!(cpu.regs[EAX], 1);
    // Patch the immediate and rerun.
    mem.write_u32(CODE + 1, 2);
    cpu.invalidate_range(CODE, 5);
    cpu.regs[ESP] = STACK_TOP;
    cpu.push32(&mut mem, layout::MAGIC_RETURN);
    cpu.eip = CODE;
    assert_eq!(cpu.run(&mut mem, 100), CpuExit::ReturnMagic);
    assert_eq!(cpu.regs[EAX], 2);
}

/// Micro benchmark: `cargo test --release -p s3-cpu -- --ignored --nocapture bench_mips`.
/// Mix typical of MSVC6 code: memory loads/stores, ALU, compare+branch, call/ret, push/pop.
#[test]
#[ignore]
fn bench_mips() {
    let (mut cpu, mut mem) = setup(|a| {
        let mut top = a.create_label();
        let mut func = a.create_label();
        let mut done = a.create_label();
        let mut skip = a.create_label();
        a.mov(ecx, 20_000_000).unwrap();
        a.mov(esi, DATA).unwrap();
        a.xor(eax, eax).unwrap();
        a.set_label(&mut top).unwrap();
        a.mov(edx, dword_ptr(esi + 8)).unwrap();
        a.add(edx, ecx).unwrap();
        a.mov(dword_ptr(esi + 8), edx).unwrap();
        a.test(ecx, 7).unwrap();
        a.jne(skip).unwrap();
        a.push(ecx).unwrap();
        a.call(func).unwrap();
        a.add(esp, 4).unwrap();
        a.set_label(&mut skip).unwrap();
        a.lea(edi, ptr(esi + ecx * 4)).unwrap();
        a.and(edi, 0xFFF).unwrap();
        a.movzx(ebx, byte_ptr(esi + edi)).unwrap();
        a.add(eax, ebx).unwrap();
        a.dec(ecx).unwrap();
        a.jne(top).unwrap();
        a.jmp(done).unwrap();
        a.set_label(&mut func).unwrap();
        a.push(ebp).unwrap();
        a.mov(ebp, esp).unwrap();
        a.mov(eax, dword_ptr(ebp + 8)).unwrap();
        a.shl(eax, 2).unwrap();
        a.pop(ebp).unwrap();
        a.ret().unwrap();
        a.set_label(&mut done).unwrap();
        a.nop().unwrap();
    });
    let t = std::time::Instant::now();
    let mut total = 0u64;
    loop {
        let before = cpu.instructions;
        let e = cpu.run(&mut mem, 50_000_000);
        total += cpu.instructions - before;
        match e {
            CpuExit::Budget => continue,
            CpuExit::ReturnMagic => break,
            e => panic!("{e:?}"),
        }
    }
    let s = t.elapsed().as_secs_f64();
    println!("bench_mips: {total} instructions in {s:.3}s = {:.1} MIPS", total as f64 / s / 1e6);
}

// ---------------------------------------------------------------------------------------------------------
// cpu-int additions

/// Run raw bytes at CODE (they must end in `ret`).
fn run_bytes(bytes: &[u8], init: impl FnOnce(&mut Cpu, &mut GuestMemory)) -> (Cpu, GuestMemory, CpuExit) {
    let mut mem = GuestMemory::new().unwrap();
    mem.write_bytes(CODE, bytes);
    let mut cpu = Cpu::new();
    cpu.regs[ESP] = STACK_TOP;
    cpu.push32(&mut mem, layout::MAGIC_RETURN);
    cpu.eip = CODE;
    init(&mut cpu, &mut mem);
    let e = cpu.run(&mut mem, 100_000);
    (cpu, mem, e)
}

/// Run a snippet through both the fast handlers and the generic path; both must agree on registers,
/// flags and the DATA page.
fn run_both(build: impl Fn(&mut CodeAssembler)) -> (Cpu, GuestMemory) {
    let mut out = Vec::new();
    for generic in [false, true] {
        let mut a = CodeAssembler::new(32).unwrap();
        build(&mut a);
        a.ret().unwrap();
        let bytes = a.assemble(CODE as u64).unwrap();
        let mut mem = GuestMemory::new().unwrap();
        mem.write_bytes(CODE, &bytes);
        let mut cpu = Cpu::new();
        cpu.set_generic_only(generic);
        cpu.regs[ESP] = STACK_TOP;
        cpu.push32(&mut mem, layout::MAGIC_RETURN);
        cpu.eip = CODE;
        let e = cpu.run(&mut mem, 1_000_000);
        assert_eq!(e, CpuExit::ReturnMagic, "generic={generic} eip={:#x}", cpu.eip);
        out.push((cpu, mem));
    }
    let (g, gm) = out.pop().unwrap();
    let (f, fm) = out.pop().unwrap();
    assert_eq!(f.regs, g.regs, "fast vs generic registers");
    assert_eq!(f.eflags(), g.eflags(), "fast vs generic eflags");
    assert_eq!(fm.slice(DATA, 0x1000), gm.slice(DATA, 0x1000), "fast vs generic memory");
    (f, fm)
}

#[test]
fn fast_forms_imm8_sign_extension_and_memory_alu() {
    let (cpu, mem) = run_both(|a| {
        a.mov(eax, 10).unwrap();
        a.add(eax, -1).unwrap(); // 83 /0 ib (sign-extended)
        a.mov(ebx, 0x100).unwrap();
        a.sub(ebx, 0x7F).unwrap();
        a.and(ebx, -2i32).unwrap();
        a.mov(dword_ptr(DATA), 5).unwrap();
        a.sub(dword_ptr(DATA), -3).unwrap(); // 8
        a.add(dword_ptr(DATA), 0x1000).unwrap(); // imm32
        a.mov(ecx, 2).unwrap();
        a.xor(dword_ptr(DATA + 4), ecx).unwrap();
        a.or(ecx, dword_ptr(DATA)).unwrap();
        a.cmp(dword_ptr(DATA), 0x1008).unwrap();
        a.sete(dl).unwrap();
        a.push(-5).unwrap(); // push imm8 sign-extended
        a.pop(esi).unwrap();
        a.push(0x12345).unwrap();
        a.pop(edi).unwrap();
        a.mov(dword_ptr(DATA + 8), ecx).unwrap();
        a.inc(dword_ptr(DATA + 8)).unwrap();
        a.test(dword_ptr(DATA + 8), 1).unwrap();
        a.setne(dh).unwrap();
        a.adc(eax, 0).unwrap();
    });
    assert_eq!(cpu.regs[EAX], 9);
    assert_eq!(cpu.regs[EBX], 0x80);
    assert_eq!(mem.read_u32(DATA), 0x1008);
    assert_eq!(mem.read_u32(DATA + 4), 2);
    assert_eq!(cpu.regs[ECX], 0x100A);
    assert_eq!(cpu.regs[EDX] & 0xFFFF, 0x0101);
    assert_eq!(cpu.regs[ESI], (-5i32) as u32);
    assert_eq!(cpu.regs[EDI], 0x12345);
    assert_eq!(mem.read_u32(DATA + 8), 0x100B);
}

#[test]
fn sbb_reg_reg_idiom_and_neg_carry() {
    let (cpu, _) = run_both(|a| {
        a.mov(eax, 5).unwrap();
        a.neg(eax).unwrap(); // CF=1
        a.sbb(ecx, ecx).unwrap(); // -1
        a.xor(edx, edx).unwrap();
        a.neg(edx).unwrap(); // CF=0
        a.sbb(ebx, ebx).unwrap(); // 0
    });
    assert_eq!(cpu.regs[ECX], 0xFFFF_FFFF);
    assert_eq!(cpu.regs[EBX], 0);
}

#[test]
fn string_ops_backward_and_compare() {
    let (cpu, mem) = run_both(|a| {
        // memmove backwards (CRT uses std + rep movsb/movsd for overlapping dst > src)
        a.mov(edi, DATA).unwrap();
        a.mov(ecx, 8).unwrap();
        a.mov(eax, 0x0403_0201).unwrap();
        a.rep().stosd().unwrap(); // 32 bytes of 01 02 03 04
        a.std().unwrap();
        a.mov(esi, DATA + 31).unwrap();
        a.mov(edi, DATA + 35).unwrap();
        a.mov(ecx, 32).unwrap();
        a.rep().movsb().unwrap();
        a.cld().unwrap();
        // repe cmpsb finds the first difference
        a.mov(esi, DATA + 4).unwrap();
        a.mov(edi, DATA + 8).unwrap();
        a.mov(byte_ptr(DATA + 8 + 5), 0xEE).unwrap();
        a.mov(ecx, 20).unwrap();
        a.repe().cmpsb().unwrap();
        a.mov(ebx, ecx).unwrap(); // 20 - 6 = 14
        a.setb(dl).unwrap(); // [esi]=02 < EE ? -> CF=1
        // lodsb / scasd / stosw singles
        a.mov(esi, DATA).unwrap();
        a.lodsb().unwrap();
        a.mov(dword_ptr(DATA + 0x100), 0x1234_5678).unwrap();
        a.mov(edi, DATA + 0x100).unwrap();
        a.push(eax).unwrap();
        a.mov(eax, 0x1234_5678).unwrap();
        a.scasd().unwrap();
        a.sete(dh).unwrap();
        a.pop(eax).unwrap();
        a.mov(edi, DATA + 0x200).unwrap();
        a.stosw().unwrap();
        // rep with ECX=0 does nothing
        a.xor(ecx, ecx).unwrap();
        a.mov(edi, DATA + 0x300).unwrap();
        a.rep().stosb().unwrap();
    });
    // Backward copy moved [DATA..DATA+32) to [DATA+4..DATA+36) without corruption.
    for i in (0..32u32).filter(|&i| i != 9) {
        assert_eq!(mem.read_u8(DATA + 4 + i), (i % 4 + 1) as u8, "byte {i}");
    }
    assert_eq!(cpu.regs[EBX], 14);
    assert_eq!(cpu.regs[EDX] & 0xFF, 1);
    assert_eq!((cpu.regs[EDX] >> 8) & 0xFF, 1);
    assert_eq!(cpu.regs[EAX] & 0xFF, 1);
    assert_eq!(mem.read_u16(DATA + 0x200), 0x0201);
    assert_eq!(cpu.regs[EDI], DATA + 0x300);
    assert!(!cpu.flag(fl::DF));
}

#[test]
fn rep_movsd_overlapping_forward_replicates_dwords() {
    let (_, mem) = run_both(|a| {
        a.mov(dword_ptr(DATA), 0xAABB_CCDDu32).unwrap();
        a.mov(esi, DATA).unwrap();
        a.mov(edi, DATA + 4).unwrap();
        a.mov(ecx, 7).unwrap();
        a.rep().movsd().unwrap();
    });
    for i in 0..8 {
        assert_eq!(mem.read_u32(DATA + 4 * i), 0xAABB_CCDD);
    }
}

#[test]
fn repne_scasb_with_fault_midway_rewinds_and_keeps_ecx() {
    // Scan backwards from 0x10003 into the NULL guard: must fault with ECX/EDI reflecting progress.
    let (mut cpu, mut mem) = setup(|a| {
        a.std().unwrap();
        a.mov(edi, 0x0001_0003).unwrap();
        a.mov(al, 0x77).unwrap();
        a.mov(ecx, 100).unwrap();
        a.repne().scasb().unwrap();
    });
    match cpu.run(&mut mem, 1000) {
        CpuExit::Fault { kind: FaultKind::AccessViolation { addr: 0xFFFF, write: false }, eip, .. } => {
            assert_eq!(cpu.eip, eip);
            assert_eq!(cpu.regs[ECX], 96);
            assert_eq!(cpu.regs[EDI], 0xFFFF);
        }
        e => panic!("{e:?}"),
    }
}

#[test]
fn jcxz_with_address_size_prefix_tests_cx_only() {
    // mov ecx, 0x10000 ; 67 e3 05 (jcxz +5) ; mov eax, 1 ; ret ; mov eax, 2 ; ret
    let code = [
        0xB9, 0x00, 0x00, 0x01, 0x00, // mov ecx, 0x10000
        0x67, 0xE3, 0x06, // jcxz +6
        0xB8, 0x01, 0, 0, 0, // mov eax,1
        0xC3, // ret
        0xB8, 0x02, 0, 0, 0, // mov eax,2
        0xC3,
    ];
    let (cpu, _, e) = run_bytes(&code, |_, _| {});
    assert_eq!(e, CpuExit::ReturnMagic);
    assert_eq!(cpu.regs[EAX], 2, "CX == 0 even though ECX != 0");
    // jecxz (no prefix) must not jump for ECX = 0x10000.
    let mut code2 = code;
    code2[5] = 0x90; // nop instead of 0x67
    let (cpu, _, _) = run_bytes(&code2, |_, _| {});
    assert_eq!(cpu.regs[EAX], 1);
}

#[test]
fn loop_variants() {
    let (cpu, _) = run_both(|a| {
        let mut l1 = a.create_label();
        let mut l2 = a.create_label();
        a.xor(eax, eax).unwrap();
        a.mov(ecx, 5).unwrap();
        a.set_label(&mut l1).unwrap();
        a.add(eax, 2).unwrap();
        a.loop_(l1).unwrap();
        // loope stops when ZF=0
        a.mov(ecx, 10).unwrap();
        a.xor(ebx, ebx).unwrap();
        a.set_label(&mut l2).unwrap();
        a.inc(ebx).unwrap();
        a.cmp(ebx, 3).unwrap(); // ZF=0 once ebx != 3 ... first iteration ebx=1 -> ZF=0 -> exit
        a.loope(l2).unwrap();
    });
    assert_eq!(cpu.regs[EAX], 10);
    assert_eq!(cpu.regs[EBX], 1);
    assert_eq!(cpu.regs[ECX], 9);
}

#[test]
fn bcd_adjust() {
    let (cpu, _) = run_both(|a| {
        a.mov(al, 0x79).unwrap();
        a.add(al, 0x35).unwrap();
        a.daa().unwrap(); // 0x14, CF=1
        a.setb(bl).unwrap();
        a.mov(bh, al).unwrap();
        a.mov(al, 0x35).unwrap();
        a.sub(al, 0x47).unwrap();
        a.das().unwrap(); // 0x88, CF=1
        a.setb(cl).unwrap();
        a.mov(ch, al).unwrap();
        a.mov(eax, 0).unwrap();
        a.mov(al, 8).unwrap();
        a.add(al, 6).unwrap();
        a.aaa().unwrap(); // AX = 0x0104
        a.mov(edx, eax).unwrap();
        a.mov(eax, 0x0103).unwrap();
        a.sub(al, 8).unwrap();
        a.aas().unwrap(); // AX = 0x0005
        a.mov(esi, eax).unwrap();
        a.mov(eax, 0x3F).unwrap();
        a.aam(10).unwrap(); // AX = 0x0603
        a.mov(edi, eax).unwrap();
        a.aad(10).unwrap(); // AX = 0x003F
    });
    assert_eq!(cpu.regs[EBX] & 0xFFFF, 0x1401);
    assert_eq!(cpu.regs[ECX] & 0xFFFF, 0x8801);
    assert_eq!(cpu.regs[EDX] & 0xFFFF, 0x0104);
    assert_eq!(cpu.regs[ESI] & 0xFFFF, 0x0005);
    assert_eq!(cpu.regs[EDI] & 0xFFFF, 0x0603);
    assert_eq!(cpu.regs[EAX] & 0xFFFF, 0x003F);
}

#[test]
fn aam_zero_is_divide_error() {
    let (cpu, _, e) = run_bytes(&[0xD4, 0x00, 0xC3], |_, _| {});
    assert!(matches!(e, CpuExit::Fault { kind: FaultKind::DivideError, .. }), "{e:?}");
    assert_eq!(cpu.eip, CODE);
}

#[test]
fn flags_push_pop_lahf_sahf_and_cpu_probes() {
    let (cpu, mem) = run_both(|a| {
        // 486 / CPUID probe: toggle AC (bit 18) and ID (bit 21) through pushfd/popfd.
        a.pushfd().unwrap();
        a.pop(eax).unwrap();
        a.mov(ecx, eax).unwrap();
        a.xor(eax, 0x0024_0000).unwrap();
        a.push(eax).unwrap();
        a.popfd().unwrap();
        a.pushfd().unwrap();
        a.pop(eax).unwrap();
        a.xor(eax, ecx).unwrap(); // bits that toggled
        a.mov(dword_ptr(DATA), eax).unwrap();
        a.push(ecx).unwrap();
        a.popfd().unwrap();
        // sahf after "fnstsw ax" pattern: AH = C3..C0 -> ZF, PF, CF
        a.mov(eax, 0x4500).unwrap(); // C3 | C2 | C0 -> ZF PF CF
        a.sahf().unwrap();
        a.setp(bl).unwrap();
        a.sete(bh).unwrap();
        a.setb(dl).unwrap();
        a.lahf().unwrap();
        a.mov(dword_ptr(DATA + 4), eax).unwrap();
        a.stc().unwrap();
        a.cmc().unwrap();
        a.setb(dh).unwrap();
        a.mov(dword_ptr(DATA + 16), ebx).unwrap();
        a.mov(dword_ptr(DATA + 20), edx).unwrap();
        // cpuid leaf 1: no MMX
        a.mov(eax, 1).unwrap();
        a.cpuid().unwrap();
        a.mov(dword_ptr(DATA + 8), edx).unwrap();
        a.mov(dword_ptr(DATA + 12), eax).unwrap();
    });
    assert_eq!(mem.read_u32(DATA), 0x0024_0000, "AC and ID toggle");
    let _ = &cpu;
    assert_eq!(mem.read_u32(DATA + 16) & 0xFFFF, 0x0101);
    assert_eq!(mem.read_u32(DATA + 20) & 0xFFFF, 0x0001);
    assert_eq!((mem.read_u32(DATA + 4) >> 8) & 0xD5, 0x45);
    assert_eq!(mem.read_u32(DATA + 8) & (1 << 23), 0, "MMX must not be reported");
    assert_eq!(mem.read_u32(DATA + 8) & 1, 1, "FPU present");
    assert_eq!((mem.read_u32(DATA + 12) >> 8) & 0xF, 5, "family 5");
}

#[test]
fn interrupts_and_invalid_opcodes() {
    let (cpu, _, e) = run_bytes(&[0x90, 0xCC, 0xC3], |_, _| {});
    assert_eq!(e, CpuExit::Interrupt(3));
    assert_eq!(cpu.eip, CODE + 2, "EIP after int3");
    let (_, _, e) = run_bytes(&[0xCD, 0x2E, 0xC3], |_, _| {});
    assert_eq!(e, CpuExit::Interrupt(0x2E));
    let (cpu, _, e) = run_bytes(&[0x90, 0x0F, 0x0B], |_, _| {});
    assert!(matches!(e, CpuExit::Fault { kind: FaultKind::InvalidOpcode, eip, .. } if eip == CODE + 1), "{e:?}");
    assert_eq!(cpu.eip, CODE + 1);
    // Undecodable bytes at the block start.
    let (_, _, e) = run_bytes(&[0x0F, 0xFF, 0xFF, 0xFF], |_, _| {});
    assert!(matches!(e, CpuExit::Fault { kind: FaultKind::InvalidOpcode, .. }), "{e:?}");
    // in al, dx / out: privileged
    let (_, _, e) = run_bytes(&[0xEC, 0xC3], |_, _| {});
    assert!(matches!(e, CpuExit::Fault { kind: FaultKind::GeneralProtection, .. }), "{e:?}");
    let (_, _, e) = run_bytes(&[0xF4], |_, _| {});
    assert_eq!(e, CpuExit::Halt);
}

#[test]
fn misc_xlat_enter_nested_xchg_mem_cmpxchg_fail_bt_mem() {
    let (cpu, mem) = run_both(|a| {
        a.mov(ebx, DATA + 0x100).unwrap();
        a.mov(byte_ptr(DATA + 0x100 + 7), 0x42).unwrap();
        a.mov(eax, 0xFFFF_FF07u32).unwrap();
        a.xlatb().unwrap(); // AL = [EBX + 7]
        a.mov(esi, eax).unwrap();
        // xchg with memory
        a.mov(dword_ptr(DATA), 11).unwrap();
        a.mov(ecx, 22).unwrap();
        a.xchg(dword_ptr(DATA), ecx).unwrap();
        // cmpxchg failing path loads the accumulator
        a.mov(eax, 99).unwrap();
        a.mov(edx, 5).unwrap();
        a.cmpxchg(dword_ptr(DATA), edx).unwrap(); // [DATA]=22 != 99 -> EAX=22, ZF=0
        a.setne(dl).unwrap();
        // bt with memory base and a register offset beyond the dword (bit 37 = byte 4, bit 5)
        a.mov(dword_ptr(DATA + 0x200), 0).unwrap();
        a.mov(dword_ptr(DATA + 0x204), 0x20).unwrap();
        a.mov(edi, 37).unwrap();
        a.bt(dword_ptr(DATA + 0x200), edi).unwrap();
        a.setb(dh).unwrap();
        a.mov(edi, -1i32).unwrap(); // bit -1 = byte -4 .. bit 31 of the previous dword
        a.bts(dword_ptr(DATA + 0x200), edi).unwrap();
        // lock inc on memory
        a.lock().inc(dword_ptr(DATA + 0x300)).unwrap();
        a.lock().dec(dword_ptr(DATA + 0x300)).unwrap();
        a.lock().dec(dword_ptr(DATA + 0x300)).unwrap();
        // movsx/movzx 16-bit forms
        a.mov(word_ptr(DATA + 0x400), 0x8001).unwrap();
        a.movsx(ebp, word_ptr(DATA + 0x400)).unwrap();
        a.mov(dword_ptr(DATA + 0x404), ebp).unwrap();
        a.movzx(ebp, word_ptr(DATA + 0x400)).unwrap();
        a.mov(dword_ptr(DATA + 0x408), ebp).unwrap();
        a.mov(ebp, 0).unwrap();
    });
    assert_eq!(cpu.regs[ESI], 0xFFFF_FF42);
    assert_eq!(cpu.regs[ECX], 11);
    assert_eq!(cpu.regs[EAX], 22);
    assert_eq!(mem.read_u32(DATA), 22);
    assert_eq!(cpu.regs[EDX] & 0xFFFF, 0x0101);
    assert_eq!(mem.read_u32(DATA + 0x1FC), 0x8000_0000);
    assert_eq!(mem.read_u32(DATA + 0x300), 0xFFFF_FFFF);
    assert_eq!(mem.read_u32(DATA + 0x404), 0xFFFF_8001);
    assert_eq!(mem.read_u32(DATA + 0x408), 0x8001);
}

#[test]
fn enter_nested_levels() {
    let (cpu, mem) = run_both(|a| {
        a.mov(ebp, STACK_TOP - 0x100).unwrap();
        a.mov(dword_ptr(STACK_TOP - 0x104), 0xAAAA).unwrap(); // outer frame's saved display entry
        a.enter(8, 2).unwrap();
        a.mov(eax, ebp).unwrap();
        a.mov(ebx, dword_ptr(ebp - 4)).unwrap(); // copied display entry
        a.mov(ecx, dword_ptr(ebp - 8)).unwrap(); // the new frame pointer itself
        a.mov(edx, esp).unwrap();
        a.leave().unwrap();
    });
    let frame = STACK_TOP - 4 - 4; // after `ret addr` push and `push ebp`
    assert_eq!(cpu.regs[EAX], frame);
    assert_eq!(cpu.regs[EBX], 0xAAAA);
    assert_eq!(cpu.regs[ECX], frame);
    assert_eq!(cpu.regs[EDX], frame - 8 - 8);
    assert_eq!(cpu.regs[EBP], STACK_TOP - 0x100);
    let _ = mem;
}

#[test]
fn fs_override_reads_and_writes_teb() {
    let (mut cpu, mut mem) = setup(|a| {
        // Typical SEH prologue: push handler; push fs:[0]; mov fs:[0], esp
        a.push(0x0040_2000).unwrap();
        a.push(dword_ptr(0).fs()).unwrap();
        a.mov(dword_ptr(0).fs(), esp).unwrap();
        a.mov(eax, dword_ptr(4).fs()).unwrap();
        // epilogue
        a.mov(ecx, dword_ptr(esp)).unwrap();
        a.mov(dword_ptr(0).fs(), ecx).unwrap();
        a.add(esp, 8).unwrap();
    });
    cpu.fs_base = 0x7FFD_E000;
    mem.write_u32(0x7FFD_E000, 0xFFFF_FFFF);
    mem.write_u32(0x7FFD_E004, 0x0090_0000);
    assert_eq!(cpu.run(&mut mem, 100), CpuExit::ReturnMagic);
    assert_eq!(cpu.regs[EAX], 0x0090_0000);
    assert_eq!(mem.read_u32(0x7FFD_E000), 0xFFFF_FFFF);
    assert_eq!(cpu.regs[ECX], 0xFFFF_FFFF);
}

#[test]
fn operand_size_prefix_16bit_ops() {
    let (cpu, mem) = run_both(|a| {
        a.mov(eax, 0x1234_FFFF).unwrap();
        a.add(ax, 1).unwrap(); // AX wraps, upper half kept, CF=1
        a.setb(bl).unwrap();
        a.mov(word_ptr(DATA), 0x7FFF).unwrap();
        a.inc(word_ptr(DATA)).unwrap(); // OF
        a.seto(bh).unwrap();
        a.push(ax).unwrap(); // 66 50: 2-byte push
        a.pop(cx).unwrap();
        a.mov(edx, esp).unwrap();
        a.shl(word_ptr(DATA), 1).unwrap(); // 0x8000 << 1 = 0, CF=1
        a.rcl(word_ptr(DATA), 1).unwrap(); // = 1
        a.imul_3(si, word_ptr(DATA), -3).unwrap();
    });
    assert_eq!(cpu.regs[EAX], 0x1234_0000);
    assert_eq!(cpu.regs[EBX] & 0xFFFF, 0x0101);
    assert_eq!(cpu.regs[ECX] & 0xFFFF, 0);
    assert_eq!(cpu.regs[EDX], STACK_TOP - 4);
    assert_eq!(mem.read_u16(DATA), 1);
    assert_eq!(cpu.regs[ESI] & 0xFFFF, 0xFFFD);
}

#[test]
fn indirect_calls_jumps_and_ret_imm() {
    const TARGET: u32 = CODE + 0x800;
    let (cpu, _) = run_both(|a| {
        let mut f = a.create_label();
        // Jump target: `mov esi, 77; ret` written as data first (not yet decoded, so no invalidation needed).
        a.mov(dword_ptr(TARGET), 0x0000_4DBE).unwrap();
        a.mov(word_ptr(TARGET + 4), 0xC300).unwrap();
        a.push(1).unwrap();
        a.push(2).unwrap();
        a.call(f).unwrap(); // stdcall, ret 8
        a.mov(ebx, eax).unwrap();
        a.mov(dword_ptr(DATA + 8), TARGET).unwrap();
        a.mov(edx, 2).unwrap();
        a.jmp(dword_ptr(edx * 4 + DATA)).unwrap(); // switch-table style; target `ret`s to MAGIC
        a.int3().unwrap();
        a.set_label(&mut f).unwrap();
        a.mov(eax, dword_ptr(esp + 4)).unwrap();
        a.add(eax, dword_ptr(esp + 8)).unwrap();
        a.ret_1(8).unwrap();
    });
    assert_eq!(cpu.regs[EBX], 3);
    assert_eq!(cpu.regs[ESI], 77);
}

#[test]
fn sixteen_bit_addressing_wraps_and_string_addr16_is_refused() {
    // 67 8B 00 = mov eax, [bx+si]; bx=0xFFFF si=0x0011 -> address 0x0010 (wraps) -> NULL guard fault at 0x10.
    let (_, _, e) = run_bytes(&[0x67, 0x8B, 0x00, 0xC3], |c, _| {
        c.regs[EBX] = 0x0001_FFFF;
        c.regs[ESI] = 0x0011;
    });
    assert!(matches!(e, CpuExit::Fault { kind: FaultKind::AccessViolation { addr: 0x10, .. }, .. }), "{e:?}");
    // 67 A4 = movsb with SI/DI
    let (_, _, e) = run_bytes(&[0x67, 0xA4, 0xC3], |_, _| {});
    assert!(matches!(e, CpuExit::Fault { kind: FaultKind::Unimplemented, .. }), "{e:?}");
}

#[test]
fn movzx_movsx_all_forms_fast_vs_generic() {
    let (cpu, _) = run_both(|a| {
        a.mov(dword_ptr(DATA), 0x8180_FF7Fu32).unwrap();
        a.movzx(eax, byte_ptr(DATA + 1)).unwrap(); // 0xFF
        a.movsx(ebx, byte_ptr(DATA + 1)).unwrap(); // -1
        a.movzx(ecx, word_ptr(DATA + 2)).unwrap(); // 0x8180
        a.movsx(edx, word_ptr(DATA + 2)).unwrap(); // 0xFFFF8180
        a.mov(esi, 0x1234_80F0).unwrap();
        a.movsx(edi, si).unwrap(); // 0xFFFF80F0
        a.movzx(ebp, si).unwrap();
        a.mov(dword_ptr(DATA + 4), ebp).unwrap();
        a.movsx(ebp, al).unwrap(); // al = 0xFF -> -1
        a.mov(dword_ptr(DATA + 8), ebp).unwrap();
        a.movzx(esi, ch).unwrap(); // 0x81
        a.mov(ebp, 0).unwrap();
    });
    assert_eq!(cpu.regs[EAX], 0xFF);
    assert_eq!(cpu.regs[EBX], 0xFFFF_FFFF);
    assert_eq!(cpu.regs[ECX], 0x8180);
    assert_eq!(cpu.regs[EDX], 0xFFFF_8180);
    assert_eq!(cpu.regs[EDI], 0xFFFF_80F0);
    assert_eq!(cpu.regs[ESI], 0x81);
}

#[test]
fn hook_exits_mid_block_and_skip_once() {
    // mov eax,1 (5 bytes) ; mov ebx,2 (5 bytes, hooked at CODE+5) ; ret
    let (mut cpu, mut mem) = setup(|a| {
        a.mov(eax, 1u32).unwrap();
        a.mov(ebx, 2u32).unwrap();
    });
    // Warm the cache without hooks first: set_hooks must drop the old unsplit block.
    let mut warm = Cpu::new();
    warm.eip = CODE;
    let _ = warm.run(&mut mem, 1);
    let mut set = HookAddrs::new();
    set.insert(CODE + 5);
    cpu.set_hooks(Some(std::sync::Arc::new(set)));
    assert_eq!(cpu.run(&mut mem, 100), CpuExit::Hook(CODE + 5));
    assert_eq!(cpu.regs[EAX], 1);
    assert_eq!(cpu.regs[EBX], 0, "hooked instruction must not have run");
    assert_eq!(cpu.eip, CODE + 5);
    // Running again without skip exits again (no progress).
    assert_eq!(cpu.run(&mut mem, 100), CpuExit::Hook(CODE + 5));
    cpu.skip_hook_once(CODE + 5);
    assert_eq!(cpu.run(&mut mem, 100), CpuExit::ReturnMagic);
    assert_eq!(cpu.regs[EBX], 2);
    // Removing hooks restores plain execution.
    cpu.set_hooks(None);
    cpu.eip = CODE;
    cpu.push32(&mut mem, layout::MAGIC_RETURN);
    assert_eq!(cpu.run(&mut mem, 100), CpuExit::ReturnMagic);
}

// ---------------------------------------------------------------------------------------------------------
// perf-interp: threaded dispatch must keep exits and retired-instruction counts exact.

/// Loop with loads, stores, ALU, a call and a conditional skip (like `bench_mips`, 200 iterations).
fn budget_program(a: &mut CodeAssembler) {
    let mut top = a.create_label();
    let mut func = a.create_label();
    let mut done = a.create_label();
    let mut skip = a.create_label();
    a.mov(ecx, 200u32).unwrap();
    a.mov(esi, DATA).unwrap();
    a.xor(eax, eax).unwrap();
    a.set_label(&mut top).unwrap();
    a.mov(edx, dword_ptr(esi + 8)).unwrap();
    a.add(edx, ecx).unwrap();
    a.mov(dword_ptr(esi + 8), edx).unwrap();
    a.imul_3(edx, edx, 37).unwrap();
    a.test(ecx, 3).unwrap();
    a.jne(skip).unwrap();
    a.push(ecx).unwrap();
    a.call(func).unwrap();
    a.add(esp, 4).unwrap();
    a.set_label(&mut skip).unwrap();
    a.lea(edi, ptr(esi + ecx * 4)).unwrap();
    a.and(edi, 0xFFF).unwrap();
    a.movzx(ebx, byte_ptr(esi + edi)).unwrap();
    a.add(eax, ebx).unwrap();
    a.adc(eax, edx).unwrap();
    a.dec(ecx).unwrap();
    a.jne(top).unwrap();
    a.jmp(done).unwrap();
    a.set_label(&mut func).unwrap();
    a.push(ebp).unwrap();
    a.mov(ebp, esp).unwrap();
    a.mov(eax, dword_ptr(ebp + 8)).unwrap();
    a.shl(eax, 2).unwrap();
    a.cdq().unwrap();
    a.pop(ebp).unwrap();
    a.ret().unwrap();
    a.set_label(&mut done).unwrap();
    a.nop().unwrap();
}

/// Running with small budgets (stops inside blocks) must retire exactly the budget per run and end in the same
/// state, with the same total count, as one run (fast and generic paths).
#[test]
fn budget_stops_are_exact() {
    let mut results = Vec::new();
    for generic in [false, true] {
        for chunk in [1_000_000u64, 1, 2, 3, 5, 7, 13] {
            let (mut cpu, mut mem) = setup(budget_program);
            cpu.set_generic_only(generic);
            let mut runs = 0u32;
            loop {
                let before = cpu.instructions;
                let e = cpu.run(&mut mem, chunk);
                runs += 1;
                match e {
                    CpuExit::Budget => assert_eq!(cpu.instructions - before, chunk, "chunk {chunk}"),
                    CpuExit::ReturnMagic => break,
                    e => panic!("{e:?}"),
                }
                assert!(runs < 1_000_000);
            }
            results.push((generic, chunk, cpu.regs, cpu.eflags(), cpu.instructions, mem.slice(DATA, 0x1000).to_vec()));
        }
    }
    let r0 = &results[0];
    for r in &results {
        assert_eq!((r.2, r.3, r.4), (r0.2, r0.3, r0.4), "generic {} chunk {}", r.0, r.1);
        assert_eq!(r.5, r0.5, "memory, generic {} chunk {}", r.0, r.1);
    }
}

/// Exits in the middle of a block: retired count and EIP for faults (instruction not retired, EIP at it) and
/// for `int3` / `hlt` (retired, EIP after it), from fast and generic handlers.
#[test]
fn mid_block_exits_count_exactly() {
    // nop; mov eax,[0x10]; ret  -> AV at CODE+1 after 1 instruction.
    let (cpu, _, e) = run_bytes(&[0x90, 0xA1, 0x10, 0, 0, 0, 0xC3], |_, _| {});
    assert!(
        matches!(e, CpuExit::Fault { kind: FaultKind::AccessViolation { addr: 0x10, write: false }, eip, .. } if eip == CODE + 1),
        "{e:?}"
    );
    assert_eq!((cpu.eip, cpu.instructions), (CODE + 1, 1));
    // nop; nop; mov [ecx],eax (ecx = 0); ret
    let (cpu, _, e) = run_bytes(&[0x90, 0x90, 0x89, 0x01, 0xC3], |_, _| {});
    assert!(
        matches!(e, CpuExit::Fault { kind: FaultKind::AccessViolation { addr: 0, write: true }, eip, .. } if eip == CODE + 2),
        "{e:?}"
    );
    assert_eq!((cpu.eip, cpu.instructions), (CODE + 2, 2));
    // nop; push eax with ESP in the guard -> fault, ESP unchanged.
    let (cpu, _, e) = run_bytes(&[0x90, 0x50, 0xC3], |c, _| c.regs[ESP] = 0x100);
    assert!(matches!(e, CpuExit::Fault { eip, .. } if eip == CODE + 1), "{e:?}");
    assert_eq!((cpu.eip, cpu.instructions, cpu.regs[ESP]), (CODE + 1, 1, 0x100));
    // nop; cdq; idiv ecx (ecx = 0) -> #DE after 2 instructions.
    let (cpu, _, e) = run_bytes(&[0x90, 0x99, 0xF7, 0xF9, 0xC3], |_, _| {});
    assert!(matches!(e, CpuExit::Fault { kind: FaultKind::DivideError, eip, .. } if eip == CODE + 2), "{e:?}");
    assert_eq!((cpu.eip, cpu.instructions), (CODE + 2, 2));
    // nop; int3 -> retired, EIP after it.
    let (cpu, _, e) = run_bytes(&[0x90, 0xCC, 0xC3], |_, _| {});
    assert_eq!(e, CpuExit::Interrupt(3));
    assert_eq!((cpu.eip, cpu.instructions), (CODE + 2, 2));
    // nop; nop; hlt; nop -> Halt, EIP after hlt.
    let (cpu, _, e) = run_bytes(&[0x90, 0x90, 0xF4, 0x90, 0xC3], |_, _| {});
    assert_eq!(e, CpuExit::Halt);
    assert_eq!((cpu.eip, cpu.instructions), (CODE + 3, 3));
    // Partial-block path: budget 2, then a run that faults at the 4th instruction of the block.
    let mut mem = GuestMemory::new().unwrap();
    mem.write_bytes(CODE, &[0x90, 0x90, 0x90, 0xA1, 0x10, 0, 0, 0, 0x90, 0xC3]);
    let mut cpu = Cpu::new();
    cpu.regs[ESP] = STACK_TOP;
    cpu.eip = CODE;
    assert_eq!(cpu.run(&mut mem, 2), CpuExit::Budget);
    assert_eq!((cpu.eip, cpu.instructions), (CODE + 2, 2));
    let e = cpu.run(&mut mem, 3);
    assert!(matches!(e, CpuExit::Fault { eip, .. } if eip == CODE + 3), "{e:?}");
    assert_eq!((cpu.eip, cpu.instructions), (CODE + 3, 3));
    // Budget stop exactly before the faulting instruction, then the fault from the full-block path.
    cpu.eip = CODE;
    cpu.instructions = 0;
    assert_eq!(cpu.run(&mut mem, 3), CpuExit::Budget);
    assert_eq!((cpu.eip, cpu.instructions), (CODE + 3, 3));
}

/// Edge-case and pseudo-random operand values for the fast-vs-generic form tests.
fn form_values() -> Vec<u32> {
    let mut v = vec![
        0u32, 1, 2, 3, 7, 8, 9, 15, 16, 17, 31, 32, 33, 0x7F, 0x80, 0x81, 0xFF, 0x100, 0x7FFF, 0x8000, 0xFFFF,
        0x1_0000, 0x7FFF_FFFF, 0x8000_0000, 0x8000_0001, 0xFFFF_FFFE, 0xFFFF_FFFF, 0x1234_5678, 0xDEAD_BEEF,
    ];
    let mut x = 0x9E37_79B9u32;
    for _ in 0..40 {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        v.push(x);
    }
    v
}

/// Run `emit` for operand pairs `(a, b)` (EAX = a, EBX = b, ECX = b, EDX = !a, [DATA+0xF00] = b, incoming flags
/// alternating 0 / all status flags) and store EAX, EBX, ECX, EDX and EFLAGS of every case into DATA; the
/// fast and the generic path must agree on everything (`run_both`).
fn forms_fast_vs_generic(emit: &dyn Fn(&mut CodeAssembler)) {
    let vals = form_values();
    let pairs: Vec<(u32, u32)> = vals.iter().flat_map(|&a| vals.iter().map(move |&b| (a, b))).collect();
    for chunk in pairs.chunks(150) {
        run_both(|a| {
            for (k, &(x, y)) in chunk.iter().enumerate() {
                let o = DATA + 20 * k as u32;
                a.mov(eax, x).unwrap();
                a.mov(ebx, y).unwrap();
                a.mov(ecx, y).unwrap();
                a.mov(edx, !x).unwrap();
                a.mov(dword_ptr(DATA + 0xF00), y).unwrap();
                a.push(if k % 2 == 0 { 0x202u32 } else { 0x202 | 0x8D5 }).unwrap();
                a.popfd().unwrap();
                emit(a);
                a.pushfd().unwrap();
                a.pop(dword_ptr(o + 16)).unwrap();
                a.mov(dword_ptr(o), eax).unwrap();
                a.mov(dword_ptr(o + 4), ebx).unwrap();
                a.mov(dword_ptr(o + 8), ecx).unwrap();
                a.mov(dword_ptr(o + 12), edx).unwrap();
            }
        });
    }
}

#[test]
fn imul_mul_cdq_forms_fast_vs_generic() {
    forms_fast_vs_generic(&|a| a.imul_2(eax, ebx).unwrap());
    forms_fast_vs_generic(&|a| a.imul_2(eax, dword_ptr(DATA + 0xF00)).unwrap());
    forms_fast_vs_generic(&|a| a.imul_3(edx, ebx, 37).unwrap());
    forms_fast_vs_generic(&|a| a.imul_3(edx, ebx, -3).unwrap());
    forms_fast_vs_generic(&|a| a.imul_3(eax, ebx, 0x1234_5678).unwrap());
    forms_fast_vs_generic(&|a| a.imul_3(ecx, dword_ptr(DATA + 0xF00), -100_000).unwrap());
    forms_fast_vs_generic(&|a| a.imul(ebx).unwrap());
    forms_fast_vs_generic(&|a| a.imul(dword_ptr(DATA + 0xF00)).unwrap());
    forms_fast_vs_generic(&|a| a.mul(ebx).unwrap());
    forms_fast_vs_generic(&|a| a.mul(dword_ptr(DATA + 0xF00)).unwrap());
    forms_fast_vs_generic(&|a| a.cdq().unwrap());
    forms_fast_vs_generic(&|a| a.cwde().unwrap());
    forms_fast_vs_generic(&|a| a.neg(ebx).unwrap());
    forms_fast_vs_generic(&|a| a.not(ebx).unwrap());
    forms_fast_vs_generic(&|a| a.inc(dword_ptr(DATA + 0xF00)).unwrap());
    forms_fast_vs_generic(&|a| a.dec(dword_ptr(DATA + 0xF00)).unwrap());
}

#[test]
fn shift_forms_fast_vs_generic() {
    for c in [0u32, 1, 2, 7, 8, 9, 15, 16, 17, 24, 31] {
        forms_fast_vs_generic(&|a| a.shl(bl, c).unwrap());
        forms_fast_vs_generic(&|a| a.shr(bh, c).unwrap());
        forms_fast_vs_generic(&|a| a.sar(al, c).unwrap());
        forms_fast_vs_generic(&|a| a.shl(bx, c).unwrap());
        forms_fast_vs_generic(&|a| a.shr(ax, c).unwrap());
        forms_fast_vs_generic(&|a| a.sar(dx, c).unwrap());
    }
    // By CL (ECX = b: all counts incl. >= 32, masked to 5 bits).
    forms_fast_vs_generic(&|a| a.shl(eax, cl).unwrap());
    forms_fast_vs_generic(&|a| a.shr(edx, cl).unwrap());
    forms_fast_vs_generic(&|a| a.sar(eax, cl).unwrap());
    forms_fast_vs_generic(&|a| a.shl(ax, cl).unwrap());
    forms_fast_vs_generic(&|a| a.shr(dx, cl).unwrap());
    forms_fast_vs_generic(&|a| a.sar(bx, cl).unwrap());
    forms_fast_vs_generic(&|a| a.shl(al, cl).unwrap());
    forms_fast_vs_generic(&|a| a.shr(ah, cl).unwrap());
    forms_fast_vs_generic(&|a| a.sar(bl, cl).unwrap());
}

#[test]
fn setcc_forms_fast_vs_generic() {
    // After cmp (lazy SUB), test (lazy LOGIC), add (lazy ADD) and the popfd'd flags (eager).
    forms_fast_vs_generic(&|a| {
        a.cmp(eax, ebx).unwrap();
        a.setb(al).unwrap();
        a.setl(ah).unwrap();
        a.setbe(bl).unwrap();
        a.setg(bh).unwrap();
        a.seto(cl).unwrap();
        a.setp(ch).unwrap();
        a.sets(dl).unwrap();
        a.sete(dh).unwrap();
    });
    forms_fast_vs_generic(&|a| {
        a.add(ebx, eax).unwrap();
        a.setae(al).unwrap();
        a.setge(ah).unwrap();
        a.seta(bl).unwrap();
        a.setle(bh).unwrap();
        a.setno(cl).unwrap();
        a.setnp(ch).unwrap();
        a.setns(dl).unwrap();
        a.setne(dh).unwrap();
    });
    forms_fast_vs_generic(&|a| {
        a.test(al, bl).unwrap();
        a.setb(al).unwrap();
        a.setle(ah).unwrap();
        a.seta(bl).unwrap();
        a.setg(bh).unwrap();
    });
    forms_fast_vs_generic(&|a| {
        a.setb(al).unwrap();
        a.setl(ah).unwrap();
        a.setbe(bl).unwrap();
        a.setg(bh).unwrap();
        a.seto(cl).unwrap();
        a.setp(ch).unwrap();
        a.sets(dl).unwrap();
        a.sete(dh).unwrap();
    });
}

/// Superinstructions (`cmp`/`test`/`dec` + block-ending `jcc`, `fast::cmp_jcc`): every fused form and condition
/// against the generic path, both branch outcomes.
#[test]
fn cmp_jcc_fusion_fast_vs_generic() {
    type Cmp = fn(&mut CodeAssembler);
    let cmps: [Cmp; 21] = [
        |a| a.cmp(eax, ebx).unwrap(),
        |a| a.cmp(ax, bx).unwrap(),
        |a| a.cmp(al, bl).unwrap(),
        |a| a.cmp(eax, 0x7F).unwrap(),
        |a| a.cmp(ebx, 0x8000_0000u32).unwrap(),
        |a| a.cmp(ax, 0x8000).unwrap(),
        |a| a.cmp(bl, 0x80).unwrap(),
        |a| a.test(eax, ebx).unwrap(),
        |a| a.test(ax, bx).unwrap(),
        |a| a.test(al, bl).unwrap(),
        |a| a.test(ebx, 0x8000_0001u32).unwrap(),
        |a| a.test(bl, 0x81).unwrap(),
        |a| a.cmp(byte_ptr(DATA + 0xF00), 0x80).unwrap(),
        |a| a.cmp(byte_ptr(esi + 0xF00), 0x7F).unwrap(),
        |a| a.cmp(dword_ptr(DATA + 0xF00), 9).unwrap(),
        |a| a.cmp(dword_ptr(esi + 0xF00), -1).unwrap(),
        |a| a.cmp(word_ptr(esi + 0xF00), 0x7FFF).unwrap(),
        |a| a.test(byte_ptr(DATA + 0xF00), 0x81).unwrap(),
        |a| a.test(byte_ptr(esi + 0xF00), 1).unwrap(),
        |a| a.test(dword_ptr(esi + 0xF00), 0x8000_0000u32).unwrap(),
        |a| a.dec(ebx).unwrap(),
    ];
    type Jcc = fn(&mut CodeAssembler, CodeLabel);
    let jccs: [Jcc; 16] = [
        |a, l| a.je(l).unwrap(),
        |a, l| a.jne(l).unwrap(),
        |a, l| a.jb(l).unwrap(),
        |a, l| a.jae(l).unwrap(),
        |a, l| a.jbe(l).unwrap(),
        |a, l| a.ja(l).unwrap(),
        |a, l| a.js(l).unwrap(),
        |a, l| a.jns(l).unwrap(),
        |a, l| a.jl(l).unwrap(),
        |a, l| a.jge(l).unwrap(),
        |a, l| a.jle(l).unwrap(),
        |a, l| a.jg(l).unwrap(),
        |a, l| a.jo(l).unwrap(),
        |a, l| a.jno(l).unwrap(),
        |a, l| a.jp(l).unwrap(),
        |a, l| a.jnp(l).unwrap(),
    ];
    for c in cmps {
        for j in jccs {
            forms_fast_vs_generic(&|a| {
                let mut l = a.create_label();
                a.mov(esi, DATA).unwrap();
                c(a);
                j(a, l);
                a.lea(edx, ptr(edx + 1)).unwrap();
                a.set_label(&mut l).unwrap();
                a.nop().unwrap();
            });
        }
    }
}
