//! Threaded dispatch: at decode time every instruction gets a handler function pointer.
//!
//! The hottest instruction forms of MSVC6 code (per `docs/reverse-engineering/static-isa.md`: 32-bit `mov`,
//! `push`, `pop`, `lea`, `call`, `ret`, `jmp`, `jcc`, ALU ops, `inc`/`dec`, `movzx`) get specialised handlers
//! that skip the mnemonic `match` and the operand-kind / operand-size checks of the generic path.
//! Everything else uses [`exec::step`]. Handlers follow the same contract as `step`: `cpu.eip` already points
//! at the next instruction, faults are returned as `Err` and `run` rewinds EIP.
//!
//! Correctness rule: a specialised handler must behave exactly like `step` for the forms `select` routes to
//! it. The unit tests run every snippet through both paths (`Cpu::generic_only`).

use iced_x86::{Code, ConditionCode, FlowControl, Instruction, Mnemonic, OpKind, Register};

use crate::exec::{self, R};
use crate::{Cpu, GuestMemory, ESP};

/// Instruction handler.
pub(crate) type Handler = fn(&mut Cpu, &mut GuestMemory, &Instruction) -> R;

const R_EAX: usize = Register::EAX as usize;

/// Index (EAX..EDI) of a 32-bit register operand.
#[inline(always)]
fn r32(ins: &Instruction, i: u32) -> usize {
    (ins.op_register(i) as usize).wrapping_sub(R_EAX) & 7
}

#[inline(always)]
fn is_r32(ins: &Instruction, i: u32) -> bool {
    ins.op_kind(i) == OpKind::Register && ins.op_register(i).is_gpr32()
}

#[inline(always)]
fn is_m32(ins: &Instruction, i: u32) -> bool {
    ins.op_kind(i) == OpKind::Memory && ins.memory_size().size() == 4 && !is_addr16(ins)
}

/// 16-bit addressing (0x67 prefix) is left to the generic path.
#[inline(always)]
fn is_addr16(ins: &Instruction) -> bool {
    ins.memory_base().is_gpr16() || ins.memory_index().is_gpr16()
}

#[inline(always)]
fn is_imm32(ins: &Instruction, i: u32) -> bool {
    ins.op_kind(i) == OpKind::Immediate32
}

#[inline(always)]
fn is_imm8s(ins: &Instruction, i: u32) -> bool {
    ins.op_kind(i) == OpKind::Immediate8to32
}

/// Immediate operand: `S8` = sign-extended imm8 form, else imm32.
#[inline(always)]
fn imm<const S8: bool>(ins: &Instruction) -> u32 {
    if S8 {
        ins.immediate8to32() as u32
    } else {
        ins.immediate32()
    }
}

// ---------------------------------------------------------------------------------------------------------
// Handlers

fn mov_rr(cpu: &mut Cpu, _: &mut GuestMemory, ins: &Instruction) -> R {
    cpu.regs[r32(ins, 0)] = cpu.regs[r32(ins, 1)];
    Ok(())
}

fn mov_rm(cpu: &mut Cpu, mem: &mut GuestMemory, ins: &Instruction) -> R {
    let v = cpu.rd(mem, cpu.ea(ins), 4)?;
    cpu.regs[r32(ins, 0)] = v;
    Ok(())
}

fn mov_mr(cpu: &mut Cpu, mem: &mut GuestMemory, ins: &Instruction) -> R {
    let v = cpu.regs[r32(ins, 1)];
    cpu.wr(mem, cpu.ea(ins), 4, v)
}

fn mov_ri(cpu: &mut Cpu, _: &mut GuestMemory, ins: &Instruction) -> R {
    cpu.regs[r32(ins, 0)] = ins.immediate32();
    Ok(())
}

fn mov_mi(cpu: &mut Cpu, mem: &mut GuestMemory, ins: &Instruction) -> R {
    cpu.wr(mem, cpu.ea(ins), 4, ins.immediate32())
}

fn movzx_r_m8(cpu: &mut Cpu, mem: &mut GuestMemory, ins: &Instruction) -> R {
    let v = cpu.rd(mem, cpu.ea(ins), 1)?;
    cpu.regs[r32(ins, 0)] = v;
    Ok(())
}

/// movzx/movsx r32 from memory: SIZE = 1 or 2, SIGNED.
fn movx_rm<const SIZE: u32, const SIGNED: bool>(cpu: &mut Cpu, mem: &mut GuestMemory, ins: &Instruction) -> R {
    let v = cpu.rd(mem, cpu.ea(ins), SIZE)?;
    cpu.regs[r32(ins, 0)] = if SIGNED { crate::flags::sext(v, SIZE) as u32 } else { v };
    Ok(())
}

/// movzx/movsx r32 from an 8/16-bit register.
fn movx_rr<const SIGNED: bool>(cpu: &mut Cpu, _: &mut GuestMemory, ins: &Instruction) -> R {
    let r = ins.op1_register();
    let size = r.size() as u32;
    let v = cpu.get_reg(r);
    cpu.regs[r32(ins, 0)] = if SIGNED { crate::flags::sext(v, size) as u32 } else { v };
    Ok(())
}

/// shl/shr/sar r32, imm8 (KIND: 0 shl, 1 shr, 2 sar).
fn shift_ri<const KIND: u8>(cpu: &mut Cpu, _: &mut GuestMemory, ins: &Instruction) -> R {
    let m = match KIND {
        0 => Mnemonic::Shl,
        1 => Mnemonic::Shr,
        _ => Mnemonic::Sar,
    };
    let d = r32(ins, 0);
    if let Some(r) = cpu.shift(m, cpu.regs[d], ins.immediate8() as u32, 4) {
        cpu.regs[d] = r;
    }
    Ok(())
}

fn lea(cpu: &mut Cpu, _: &mut GuestMemory, ins: &Instruction) -> R {
    cpu.regs[r32(ins, 0)] = cpu.ea_noseg(ins);
    Ok(())
}

#[inline(always)]
fn push32(cpu: &mut Cpu, mem: &mut GuestMemory, v: u32) -> R {
    let sp = cpu.regs[ESP].wrapping_sub(4);
    cpu.wr(mem, sp, 4, v)?;
    cpu.regs[ESP] = sp;
    Ok(())
}

fn push_r(cpu: &mut Cpu, mem: &mut GuestMemory, ins: &Instruction) -> R {
    let v = cpu.regs[r32(ins, 0)];
    push32(cpu, mem, v)
}

fn push_i<const S8: bool>(cpu: &mut Cpu, mem: &mut GuestMemory, ins: &Instruction) -> R {
    push32(cpu, mem, imm::<S8>(ins))
}

fn push_m(cpu: &mut Cpu, mem: &mut GuestMemory, ins: &Instruction) -> R {
    let v = cpu.rd(mem, cpu.ea(ins), 4)?;
    push32(cpu, mem, v)
}

fn pop_r(cpu: &mut Cpu, mem: &mut GuestMemory, ins: &Instruction) -> R {
    let v = cpu.rd(mem, cpu.regs[ESP], 4)?;
    cpu.regs[ESP] = cpu.regs[ESP].wrapping_add(4);
    // `pop esp` loads the popped value (the increment is overwritten).
    cpu.regs[r32(ins, 0)] = v;
    Ok(())
}

fn call_rel(cpu: &mut Cpu, mem: &mut GuestMemory, ins: &Instruction) -> R {
    push32(cpu, mem, cpu.eip)?;
    cpu.eip = ins.near_branch32();
    Ok(())
}

fn call_r(cpu: &mut Cpu, mem: &mut GuestMemory, ins: &Instruction) -> R {
    let t = cpu.regs[r32(ins, 0)];
    push32(cpu, mem, cpu.eip)?;
    cpu.eip = t;
    Ok(())
}

fn call_m(cpu: &mut Cpu, mem: &mut GuestMemory, ins: &Instruction) -> R {
    let t = cpu.rd(mem, cpu.ea(ins), 4)?;
    push32(cpu, mem, cpu.eip)?;
    cpu.eip = t;
    Ok(())
}

fn ret(cpu: &mut Cpu, mem: &mut GuestMemory, _: &Instruction) -> R {
    let v = cpu.rd(mem, cpu.regs[ESP], 4)?;
    cpu.regs[ESP] = cpu.regs[ESP].wrapping_add(4);
    cpu.eip = v;
    Ok(())
}

fn ret_imm(cpu: &mut Cpu, mem: &mut GuestMemory, ins: &Instruction) -> R {
    let v = cpu.rd(mem, cpu.regs[ESP], 4)?;
    cpu.regs[ESP] = cpu.regs[ESP].wrapping_add(4 + ins.immediate16() as u32);
    cpu.eip = v;
    Ok(())
}

fn jmp_rel(cpu: &mut Cpu, _: &mut GuestMemory, ins: &Instruction) -> R {
    cpu.eip = ins.near_branch32();
    Ok(())
}

fn jmp_m(cpu: &mut Cpu, mem: &mut GuestMemory, ins: &Instruction) -> R {
    cpu.eip = cpu.rd(mem, cpu.ea(ins), 4)?;
    Ok(())
}

fn jmp_r(cpu: &mut Cpu, _: &mut GuestMemory, ins: &Instruction) -> R {
    cpu.eip = cpu.regs[r32(ins, 0)];
    Ok(())
}

fn jcc(cpu: &mut Cpu, _: &mut GuestMemory, ins: &Instruction) -> R {
    if cpu.cond(ins.condition_code()) {
        cpu.eip = ins.near_branch32();
    }
    Ok(())
}

// ALU ops (32-bit). OP: 0 add, 1 or, 2 adc, 3 sbb, 4 and, 5 sub, 6 xor, 7 cmp, 8 test.
const ADD: u8 = 0;
const OR: u8 = 1;
const ADC: u8 = 2;
const SBB: u8 = 3;
const AND: u8 = 4;
const SUB: u8 = 5;
const XOR: u8 = 6;
const CMP: u8 = 7;
const TEST: u8 = 8;

#[inline(always)]
fn alu<const OP: u8>(cpu: &mut Cpu, a: u32, b: u32) -> u32 {
    match OP {
        ADD => cpu.alu_add(a, b, 0, 4),
        ADC => {
            let c = cpu.eflags & 1;
            cpu.alu_add(a, b, c, 4)
        }
        SUB | CMP => cpu.alu_sub(a, b, 0, 4),
        SBB => {
            let c = cpu.eflags & 1;
            cpu.alu_sub(a, b, c, 4)
        }
        AND | TEST => cpu.alu_logic(a & b, 4),
        OR => cpu.alu_logic(a | b, 4),
        _ => cpu.alu_logic(a ^ b, 4),
    }
}

const fn writes(op: u8) -> bool {
    op != CMP && op != TEST
}

fn alu_rr<const OP: u8>(cpu: &mut Cpu, _: &mut GuestMemory, ins: &Instruction) -> R {
    let d = r32(ins, 0);
    let r = alu::<OP>(cpu, cpu.regs[d], cpu.regs[r32(ins, 1)]);
    if writes(OP) {
        cpu.regs[d] = r;
    }
    Ok(())
}

fn alu_ri<const OP: u8, const S8: bool>(cpu: &mut Cpu, _: &mut GuestMemory, ins: &Instruction) -> R {
    let d = r32(ins, 0);
    let r = alu::<OP>(cpu, cpu.regs[d], imm::<S8>(ins));
    if writes(OP) {
        cpu.regs[d] = r;
    }
    Ok(())
}

fn alu_rm<const OP: u8>(cpu: &mut Cpu, mem: &mut GuestMemory, ins: &Instruction) -> R {
    let d = r32(ins, 0);
    let b = cpu.rd(mem, cpu.ea(ins), 4)?;
    let r = alu::<OP>(cpu, cpu.regs[d], b);
    if writes(OP) {
        cpu.regs[d] = r;
    }
    Ok(())
}

fn alu_mr<const OP: u8>(cpu: &mut Cpu, mem: &mut GuestMemory, ins: &Instruction) -> R {
    let a = cpu.ea(ins);
    let v = cpu.rd(mem, a, 4)?;
    let r = alu::<OP>(cpu, v, cpu.regs[r32(ins, 1)]);
    if writes(OP) {
        cpu.wr(mem, a, 4, r)?;
    }
    Ok(())
}

fn alu_mi<const OP: u8, const S8: bool>(cpu: &mut Cpu, mem: &mut GuestMemory, ins: &Instruction) -> R {
    let a = cpu.ea(ins);
    let v = cpu.rd(mem, a, 4)?;
    let r = alu::<OP>(cpu, v, imm::<S8>(ins));
    if writes(OP) {
        cpu.wr(mem, a, 4, r)?;
    }
    Ok(())
}

fn inc_r(cpu: &mut Cpu, _: &mut GuestMemory, ins: &Instruction) -> R {
    let d = r32(ins, 0);
    let cf = cpu.eflags & 1;
    cpu.regs[d] = cpu.alu_add(cpu.regs[d], 1, 0, 4);
    cpu.eflags = (cpu.eflags & !1) | cf;
    Ok(())
}

fn dec_r(cpu: &mut Cpu, _: &mut GuestMemory, ins: &Instruction) -> R {
    let d = r32(ins, 0);
    let cf = cpu.eflags & 1;
    cpu.regs[d] = cpu.alu_sub(cpu.regs[d], 1, 0, 4);
    cpu.eflags = (cpu.eflags & !1) | cf;
    Ok(())
}

fn nop(_: &mut Cpu, _: &mut GuestMemory, _: &Instruction) -> R {
    Ok(())
}

fn generic(cpu: &mut Cpu, mem: &mut GuestMemory, ins: &Instruction) -> R {
    exec::step(cpu, mem, ins)
}

// ---------------------------------------------------------------------------------------------------------
// Selection

macro_rules! alu_forms {
    ($op:expr, $ins:expr) => {{
        let ins = $ins;
        if is_r32(ins, 0) && is_r32(ins, 1) {
            Some(alu_rr::<{ $op }> as Handler)
        } else if is_r32(ins, 0) && is_imm32(ins, 1) {
            Some(alu_ri::<{ $op }, false> as Handler)
        } else if is_r32(ins, 0) && is_imm8s(ins, 1) {
            Some(alu_ri::<{ $op }, true> as Handler)
        } else if is_r32(ins, 0) && is_m32(ins, 1) {
            Some(alu_rm::<{ $op }> as Handler)
        } else if is_m32(ins, 0) && is_r32(ins, 1) {
            Some(alu_mr::<{ $op }> as Handler)
        } else if is_m32(ins, 0) && is_imm32(ins, 1) {
            Some(alu_mi::<{ $op }, false> as Handler)
        } else if is_m32(ins, 0) && is_imm8s(ins, 1) {
            Some(alu_mi::<{ $op }, true> as Handler)
        } else {
            None
        }
    }};
}

/// Pick the handler for a decoded instruction.
pub(crate) fn select(ins: &Instruction) -> Handler {
    if ins.has_lock_prefix() && ins.op0_kind() != OpKind::Memory {
        // `lock` on a register destination is #UD; let the generic path decide.
        return generic;
    }
    specialised(ins).unwrap_or(generic)
}

fn specialised(ins: &Instruction) -> Option<Handler> {
    use Mnemonic as M;
    let m = ins.mnemonic();
    // Operand-size override / 16-bit stack ops go generic.
    let h: Handler = match m {
        M::Mov if ins.op_count() == 2 => {
            if is_r32(ins, 0) && is_r32(ins, 1) {
                mov_rr
            } else if is_r32(ins, 0) && is_m32(ins, 1) {
                mov_rm
            } else if is_m32(ins, 0) && is_r32(ins, 1) {
                mov_mr
            } else if is_r32(ins, 0) && is_imm32(ins, 1) {
                mov_ri
            } else if is_m32(ins, 0) && is_imm32(ins, 1) {
                mov_mi
            } else {
                return None;
            }
        }
        M::Movzx | M::Movsx if is_r32(ins, 0) && ins.op1_kind() == OpKind::Memory && !is_addr16(ins) => {
            match (m == M::Movsx, ins.memory_size().size()) {
                (false, 1) => movzx_r_m8,
                (false, 2) => movx_rm::<2, false>,
                (true, 1) => movx_rm::<1, true>,
                (true, 2) => movx_rm::<2, true>,
                _ => return None,
            }
        }
        M::Movzx if is_r32(ins, 0) && ins.op1_kind() == OpKind::Register => movx_rr::<false>,
        M::Movsx if is_r32(ins, 0) && ins.op1_kind() == OpKind::Register => movx_rr::<true>,
        M::Shl | M::Sal if is_r32(ins, 0) && ins.op1_kind() == OpKind::Immediate8 => shift_ri::<0>,
        M::Shr if is_r32(ins, 0) && ins.op1_kind() == OpKind::Immediate8 => shift_ri::<1>,
        M::Sar if is_r32(ins, 0) && ins.op1_kind() == OpKind::Immediate8 => shift_ri::<2>,
        M::Lea if is_r32(ins, 0) && !is_addr16(ins) => lea,
        M::Push => match ins.code() {
            Code::Push_r32 => push_r,
            Code::Pushd_imm8 => push_i::<true>,
            Code::Pushd_imm32 => push_i::<false>,
            Code::Push_rm32 if ins.op0_kind() == OpKind::Memory && !is_addr16(ins) => push_m,
            Code::Push_rm32 => push_r,
            _ => return None,
        },
        M::Pop => match ins.code() {
            Code::Pop_r32 => pop_r,
            Code::Pop_rm32 if ins.op0_kind() == OpKind::Register => pop_r,
            _ => return None,
        },
        M::Call => match ins.code() {
            Code::Call_rel32_32 => call_rel,
            Code::Call_rm32 if ins.op0_kind() == OpKind::Register => call_r,
            Code::Call_rm32 if !is_addr16(ins) => call_m,
            _ => return None,
        },
        M::Ret => match ins.code() {
            Code::Retnd => ret,
            Code::Retnd_imm16 => ret_imm,
            _ => return None,
        },
        M::Jmp => match ins.code() {
            Code::Jmp_rel8_32 | Code::Jmp_rel32_32 => jmp_rel,
            Code::Jmp_rm32 if ins.op0_kind() == OpKind::Register => jmp_r,
            Code::Jmp_rm32 if !is_addr16(ins) => jmp_m,
            _ => return None,
        },
        _ if ins.flow_control() == FlowControl::ConditionalBranch
            && ins.condition_code() != ConditionCode::None
            && ins.op0_kind() == OpKind::NearBranch32
            && ins.is_jcc_short_or_near() =>
        {
            jcc
        }
        M::Add => alu_forms!(ADD, ins)?,
        M::Or => alu_forms!(OR, ins)?,
        M::Adc => alu_forms!(ADC, ins)?,
        M::Sbb => alu_forms!(SBB, ins)?,
        M::And => alu_forms!(AND, ins)?,
        M::Sub => alu_forms!(SUB, ins)?,
        M::Xor => alu_forms!(XOR, ins)?,
        M::Cmp => alu_forms!(CMP, ins)?,
        M::Test => alu_forms!(TEST, ins)?,
        M::Inc if is_r32(ins, 0) => inc_r,
        M::Dec if is_r32(ins, 0) => dec_r,
        M::Nop => nop,
        _ => return None,
    };
    Some(h)
}

/// Handler that always runs the generic path (tests compare it against the specialised handlers).
pub(crate) fn generic_handler() -> Handler {
    generic
}
