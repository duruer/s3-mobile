//! Threaded dispatch: at decode time every instruction gets a handler function pointer plus a compact
//! pre-decoded operand record ([`Op`]).
//!
//! The hottest instruction forms of MSVC6 code (per `docs/reverse-engineering/static-isa.md` and the in-game
//! profile in `docs/porting/performance.md`: 32-bit `mov`, `push`, `pop`, `lea`, `call`, `ret`, `jmp`, `jcc`,
//! ALU ops, `inc`/`dec`, `movzx`, and the 8/16-bit `mov`/`cmp`/`test`/`and` forms of the blitters) get
//! specialised handlers. They read register numbers, displacement, immediate and branch target from the `Op`
//! (no iced accessor decoding at run time) and compute the effective address with a form fixed at decode time
//! (const generic `E`: `[disp]`, `[base+disp]`, `[base+index*s+disp]`, `[index*s+disp]`).
//! Everything else uses [`exec::step`] through [`generic`].
//!
//! Handler contract: `cpu.eip` already holds the next instruction's address (the run loop stores it before
//! every call). Fast handlers do not maintain `cpu.cur_ip`; on failure they store the exit in
//! `cpu.pending_exit` and return `false`; the run loop rewinds EIP and fixes the fault EIP.
//! [`generic`] sets `cur_ip` (the x87 code uses it as the last instruction pointer) and calls `step`.
//!
//! Correctness rule: a specialised handler must behave exactly like `step` for the forms `select` routes to
//! it. The unit tests run every snippet through both paths (`Cpu::set_generic_only`), and the differential
//! tests against the host CPU cover both.

use iced_x86::{Code, ConditionCode, FlowControl, Instruction, Mnemonic, OpKind, Register};

use crate::exec;
use crate::{fl, Cpu, GuestMemory, ESP};

/// Instruction handler; `false` = exit stored in `cpu.pending_exit`.
pub(crate) type Handler = fn(&mut Cpu, &mut GuestMemory, &Op) -> bool;

/// Pre-decoded operands of one instruction (filled by [`select`]; unused fields are 0).
#[derive(Clone, Copy)]
pub(crate) struct Op {
    pub h: Handler,
    /// Address of the next instruction.
    pub next: u32,
    /// Memory displacement.
    pub disp: u32,
    /// Immediate (already sign/zero-extended to the operand size) or branch target.
    pub imm: u32,
    /// Register numbers of operand 0 / operand 1 (0..7 in encoding order; for 8-bit registers 0..3 = AL..BL,
    /// 4..7 = AH..BH).
    pub r0: u8,
    pub r1: u8,
    /// Memory base / index register (0..7).
    pub base: u8,
    pub index: u8,
    /// log2 of the index scale.
    pub shift: u8,
    /// The decoded instruction (for the generic path). Points into the owning `Block::insns`, whose heap
    /// buffer is never reallocated after `decode_block` set this; null until then.
    pub ins: *const Instruction,
}

// SAFETY: `ins` only points into the block that owns this `Op`; the block (and its CPU) moves between host
// threads as a whole and is never shared.
unsafe impl Send for Op {}
unsafe impl Sync for Op {}

impl std::fmt::Debug for Op {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Op(next {:#x})", self.next)
    }
}

impl Op {
    pub(crate) fn new(h: Handler, ins: &Instruction) -> Op {
        Op { h, next: ins.next_ip32(), disp: 0, imm: 0, r0: 0, r1: 0, base: 0, index: 0, shift: 0, ins: std::ptr::null() }
    }
}

// EA forms.
const E_DISP: u8 = 0;
const E_BASE: u8 = 1;
const E_BI: u8 = 2;
const E_IDX: u8 = 3;

/// Effective address (DS/ES/SS: segment base 0).
#[inline(always)]
fn ea<const E: u8>(cpu: &Cpu, op: &Op) -> u32 {
    match E {
        E_DISP => op.disp,
        E_BASE => op.disp.wrapping_add(cpu.regs[op.base as usize & 7]),
        E_BI => op
            .disp
            .wrapping_add(cpu.regs[op.base as usize & 7])
            .wrapping_add(cpu.regs[op.index as usize & 7] << op.shift),
        _ => op.disp.wrapping_add(cpu.regs[op.index as usize & 7] << op.shift),
    }
}

/// Unwrap an `Option` from [`ld`]/[`st`]/[`push32`]: `None` (exit already stored) returns `false`.
macro_rules! tri {
    ($cpu:expr, $e:expr) => {
        match $e {
            Some(v) => v,
            None => return false,
        }
    };
}

/// Store an access-violation exit (kept out of line so the handlers' fast paths need no stack frame).
#[cold]
#[inline(never)]
fn av(cpu: &mut Cpu, addr: u32, write: bool) {
    cpu.pending_exit = Some(cpu.av_exit(addr, write));
}

#[inline(always)]
fn ld<const SZ: u32>(cpu: &mut Cpu, mem: &GuestMemory, a: u32) -> Option<u32> {
    if a < crate::layout::NULL_GUARD_END {
        av(cpu, a, false);
        return None;
    }
    Some(match SZ {
        1 => mem.read_u8(a) as u32,
        2 => mem.read_u16(a) as u32,
        _ => mem.read_u32(a),
    })
}

#[inline(always)]
fn st<const SZ: u32>(cpu: &mut Cpu, mem: &mut GuestMemory, a: u32, v: u32) -> Option<()> {
    if a < crate::layout::NULL_GUARD_END {
        av(cpu, a, true);
        return None;
    }
    match SZ {
        1 => mem.write_u8(a, v as u8),
        2 => mem.write_u16(a, v as u16),
        _ => mem.write_u32(a, v),
    }
    Some(())
}

/// Read an 8/16/32-bit register by `Op` register number.
#[inline(always)]
fn getr<const SZ: u32>(cpu: &Cpu, r: u8) -> u32 {
    match SZ {
        1 => {
            if r < 4 {
                cpu.regs[r as usize] & 0xFF
            } else {
                (cpu.regs[(r & 3) as usize] >> 8) & 0xFF
            }
        }
        2 => cpu.regs[r as usize & 7] & 0xFFFF,
        _ => cpu.regs[r as usize & 7],
    }
}

/// Write an 8/16/32-bit register (partial registers merge).
#[inline(always)]
fn setr<const SZ: u32>(cpu: &mut Cpu, r: u8, v: u32) {
    match SZ {
        1 => {
            if r < 4 {
                let x = &mut cpu.regs[r as usize];
                *x = (*x & !0xFF) | (v & 0xFF);
            } else {
                let x = &mut cpu.regs[(r & 3) as usize];
                *x = (*x & !0xFF00) | ((v & 0xFF) << 8);
            }
        }
        2 => {
            let x = &mut cpu.regs[r as usize & 7];
            *x = (*x & !0xFFFF) | (v & 0xFFFF);
        }
        _ => cpu.regs[r as usize & 7] = v,
    }
}

// ---------------------------------------------------------------------------------------------------------
// Handlers

fn mov_rr<const SZ: u32>(cpu: &mut Cpu, _: &mut GuestMemory, op: &Op) -> bool {
    let v = getr::<SZ>(cpu, op.r1);
    setr::<SZ>(cpu, op.r0, v);
    true
}

fn mov_rm<const SZ: u32, const E: u8>(cpu: &mut Cpu, mem: &mut GuestMemory, op: &Op) -> bool {
    let v = tri!(cpu, ld::<SZ>(cpu, mem, ea::<E>(cpu, op)));
    setr::<SZ>(cpu, op.r0, v);
    true
}

fn mov_mr<const SZ: u32, const E: u8>(cpu: &mut Cpu, mem: &mut GuestMemory, op: &Op) -> bool {
    let v = getr::<SZ>(cpu, op.r1);
    tri!(cpu, st::<SZ>(cpu, mem, ea::<E>(cpu, op), v));
    true
}

fn mov_ri<const SZ: u32>(cpu: &mut Cpu, _: &mut GuestMemory, op: &Op) -> bool {
    setr::<SZ>(cpu, op.r0, op.imm);
    true
}

fn mov_mi<const SZ: u32, const E: u8>(cpu: &mut Cpu, mem: &mut GuestMemory, op: &Op) -> bool {
    tri!(cpu, st::<SZ>(cpu, mem, ea::<E>(cpu, op), op.imm));
    true
}

/// movzx/movsx r32 from memory: SZ = 1 or 2, SIGNED.
fn movx_rm<const SZ: u32, const SIGNED: bool, const E: u8>(
    cpu: &mut Cpu,
    mem: &mut GuestMemory,
    op: &Op,
) -> bool {
    let v = tri!(cpu, ld::<SZ>(cpu, mem, ea::<E>(cpu, op)));
    cpu.regs[op.r0 as usize & 7] = if SIGNED { crate::flags::sext(v, SZ) as u32 } else { v };
    true
}

/// movzx/movsx r32 from an 8/16-bit register.
fn movx_rr<const SZ: u32, const SIGNED: bool>(cpu: &mut Cpu, _: &mut GuestMemory, op: &Op) -> bool {
    let v = getr::<SZ>(cpu, op.r1);
    cpu.regs[op.r0 as usize & 7] = if SIGNED { crate::flags::sext(v, SZ) as u32 } else { v };
    true
}

/// shl/shr/sar r32, imm8 (KIND: 0 shl, 1 shr, 2 sar). Count in `op.imm`.
fn shift_ri<const KIND: u8>(cpu: &mut Cpu, _: &mut GuestMemory, op: &Op) -> bool {
    let d = op.r0 as usize & 7;
    let a = cpu.regs[d];
    let c = op.imm & 0x1F;
    if c == 0 {
        return true;
    }
    let (r, cf, of) = match KIND {
        0 => {
            let r = a << c;
            let cf = (a >> (32 - c)) & 1;
            (r, cf, ((r >> 31) ^ cf) & 1)
        }
        1 => {
            let r = a >> c;
            let cf = (a >> (c - 1)) & 1;
            (r, cf, (a >> 31) & 1)
        }
        _ => {
            let r = ((a as i32) >> c) as u32;
            let cf = ((a as i32) >> (c - 1)) as u32 & 1;
            (r, cf, 0)
        }
    };
    cpu.regs[d] = r;
    // Same flags as `Cpu::shift` (exec.rs): CF = last bit out, OF as for a 1-bit shift (also for c > 1, where
    // it is undefined), AF unchanged, SZP from the result.
    let af = cpu.af();
    cpu.set_eflags((cpu.ef & !fl::STATUS) | af | cf | if of != 0 { fl::OF } else { 0 } | crate::flags::szp(r, 4));
    true
}

fn lea<const E: u8>(cpu: &mut Cpu, _: &mut GuestMemory, op: &Op) -> bool {
    cpu.regs[op.r0 as usize & 7] = ea::<E>(cpu, op);
    true
}

#[inline(always)]
fn push32(cpu: &mut Cpu, mem: &mut GuestMemory, v: u32) -> Option<()> {
    let sp = cpu.regs[ESP].wrapping_sub(4);
    st::<4>(cpu, mem, sp, v)?;
    cpu.regs[ESP] = sp;
    Some(())
}

fn push_r(cpu: &mut Cpu, mem: &mut GuestMemory, op: &Op) -> bool {
    let v = cpu.regs[op.r0 as usize & 7];
    tri!(cpu, push32(cpu, mem, v));
    true
}

fn push_i(cpu: &mut Cpu, mem: &mut GuestMemory, op: &Op) -> bool {
    tri!(cpu, push32(cpu, mem, op.imm));
    true
}

fn push_m<const E: u8>(cpu: &mut Cpu, mem: &mut GuestMemory, op: &Op) -> bool {
    let v = tri!(cpu, ld::<4>(cpu, mem, ea::<E>(cpu, op)));
    tri!(cpu, push32(cpu, mem, v));
    true
}

fn pop_r(cpu: &mut Cpu, mem: &mut GuestMemory, op: &Op) -> bool {
    let v = tri!(cpu, ld::<4>(cpu, mem, cpu.regs[ESP]));
    cpu.regs[ESP] = cpu.regs[ESP].wrapping_add(4);
    // `pop esp` loads the popped value (the increment is overwritten).
    cpu.regs[op.r0 as usize & 7] = v;
    true
}

fn call_rel(cpu: &mut Cpu, mem: &mut GuestMemory, op: &Op) -> bool {
    tri!(cpu, push32(cpu, mem, op.next));
    cpu.eip = op.imm;
    true
}

fn call_r(cpu: &mut Cpu, mem: &mut GuestMemory, op: &Op) -> bool {
    let t = cpu.regs[op.r0 as usize & 7];
    tri!(cpu, push32(cpu, mem, op.next));
    cpu.eip = t;
    true
}

fn call_m<const E: u8>(cpu: &mut Cpu, mem: &mut GuestMemory, op: &Op) -> bool {
    let t = tri!(cpu, ld::<4>(cpu, mem, ea::<E>(cpu, op)));
    tri!(cpu, push32(cpu, mem, op.next));
    cpu.eip = t;
    true
}

fn ret(cpu: &mut Cpu, mem: &mut GuestMemory, op: &Op) -> bool {
    let v = tri!(cpu, ld::<4>(cpu, mem, cpu.regs[ESP]));
    cpu.regs[ESP] = cpu.regs[ESP].wrapping_add(4 + op.imm);
    cpu.eip = v;
    true
}

fn jmp_rel(cpu: &mut Cpu, _: &mut GuestMemory, op: &Op) -> bool {
    cpu.eip = op.imm;
    true
}

fn jmp_m<const E: u8>(cpu: &mut Cpu, mem: &mut GuestMemory, op: &Op) -> bool {
    cpu.eip = tri!(cpu, ld::<4>(cpu, mem, ea::<E>(cpu, op)));
    true
}

fn jmp_r(cpu: &mut Cpu, _: &mut GuestMemory, op: &Op) -> bool {
    cpu.eip = cpu.regs[op.r0 as usize & 7];
    true
}

/// Condition code (iced `ConditionCode as u8`) against EFLAGS.
#[inline(always)]
fn cc_eval<const CC: u8>(f: u32) -> bool {
    let cf = f & fl::CF != 0;
    let zf = f & fl::ZF != 0;
    let sf = f & fl::SF != 0;
    let of = f & fl::OF != 0;
    let pf = f & fl::PF != 0;
    const O: u8 = ConditionCode::o as u8;
    const NO: u8 = ConditionCode::no as u8;
    const B: u8 = ConditionCode::b as u8;
    const AE: u8 = ConditionCode::ae as u8;
    const EQ: u8 = ConditionCode::e as u8;
    const NE: u8 = ConditionCode::ne as u8;
    const BE: u8 = ConditionCode::be as u8;
    const A: u8 = ConditionCode::a as u8;
    const S: u8 = ConditionCode::s as u8;
    const NS: u8 = ConditionCode::ns as u8;
    const P: u8 = ConditionCode::p as u8;
    const NP: u8 = ConditionCode::np as u8;
    const L: u8 = ConditionCode::l as u8;
    const GE: u8 = ConditionCode::ge as u8;
    const LE: u8 = ConditionCode::le as u8;
    const G: u8 = ConditionCode::g as u8;
    match CC {
        O => of,
        NO => !of,
        B => cf,
        AE => !cf,
        EQ => zf,
        NE => !zf,
        BE => cf || zf,
        A => !cf && !zf,
        S => sf,
        NS => !sf,
        P => pf,
        NP => !pf,
        L => sf != of,
        GE => sf == of,
        LE => zf || sf != of,
        G => !zf && sf == of,
        _ => true,
    }
}

/// Condition from the lazy flag state where it is cheap; otherwise from the materialised EFLAGS. Same results
/// as `cc_eval(cpu.eflags())` (test `lazy_conditions_match_materialised_flags`).
///
/// For every lazy kind `lr` is the masked result, so ZF = `lr == 0` and SF = its top bit; CMP/SUB keep the
/// masked operands, so the unsigned/signed orderings compare them directly (signed: shifted to bit 31).
#[inline(always)]
fn cond_fast<const CC: u8>(cpu: &Cpu) -> bool {
    use crate::flags::lk;
    const EQ: u8 = ConditionCode::e as u8;
    const NE: u8 = ConditionCode::ne as u8;
    const S: u8 = ConditionCode::s as u8;
    const NS: u8 = ConditionCode::ns as u8;
    const B: u8 = ConditionCode::b as u8;
    const AE: u8 = ConditionCode::ae as u8;
    const BE: u8 = ConditionCode::be as u8;
    const A: u8 = ConditionCode::a as u8;
    const L: u8 = ConditionCode::l as u8;
    const GE: u8 = ConditionCode::ge as u8;
    const LE: u8 = ConditionCode::le as u8;
    const G: u8 = ConditionCode::g as u8;
    let kind = cpu.lk;
    if kind == lk::NONE {
        return cc_eval::<CC>(cpu.ef);
    }
    let r = cpu.lr;
    // Shift that moves the operand's sign bit to bit 31.
    let sh = 32 - 8 * cpu.lsz as u32;
    match CC {
        EQ => return r == 0,
        NE => return r != 0,
        S => return ((r << sh) as i32) < 0,
        NS => return ((r << sh) as i32) >= 0,
        _ => {}
    }
    if kind == lk::SUB {
        let (a, b) = (cpu.la, cpu.lb);
        let (sa, sb) = (((a << sh) as i32), ((b << sh) as i32));
        match CC {
            B => return a < b,
            AE => return a >= b,
            BE => return a <= b,
            A => return a > b,
            L => return sa < sb,
            GE => return sa >= sb,
            LE => return sa <= sb,
            G => return sa > sb,
            _ => {}
        }
    } else if kind != lk::LOGIC {
        // ADD / INC / DEC: SF and OF from the result and operands.
        let sf = (r << sh) >> 31;
        let of = cpu.of();
        match CC {
            L => return sf != of,
            GE => return sf == of,
            LE => return r == 0 || sf != of,
            G => return r != 0 && sf == of,
            B | AE | BE | A => {
                let cf = cpu.cf() != 0;
                let zf = r == 0;
                match CC {
                    B => return cf,
                    AE => return !cf,
                    BE => return cf || zf,
                    _ => return !cf && !zf,
                }
            }
            _ => {}
        }
    } else {
        let sr = (r << sh) as i32;
        // CF = OF = 0.
        match CC {
            B => return false,
            AE => return true,
            BE => return r == 0,
            A => return r != 0,
            L => return sr < 0,
            GE => return sr >= 0,
            LE => return sr <= 0,
            G => return sr > 0,
            _ => {}
        }
    }
    cc_eval::<CC>(cpu.eflags())
}

fn jcc<const CC: u8>(cpu: &mut Cpu, _: &mut GuestMemory, op: &Op) -> bool {
    if cond_fast::<CC>(cpu) {
        cpu.eip = op.imm;
    }
    true
}

fn jcc_handler(cc: ConditionCode) -> Handler {
    use ConditionCode as C;
    macro_rules! j {
        ($c:ident) => {
            jcc::<{ C::$c as u8 }> as Handler
        };
    }
    match cc {
        C::o => j!(o),
        C::no => j!(no),
        C::b => j!(b),
        C::ae => j!(ae),
        C::e => j!(e),
        C::ne => j!(ne),
        C::be => j!(be),
        C::a => j!(a),
        C::s => j!(s),
        C::ns => j!(ns),
        C::p => j!(p),
        C::np => j!(np),
        C::l => j!(l),
        C::ge => j!(ge),
        C::le => j!(le),
        C::g => j!(g),
        C::None => j!(None),
    }
}

// ALU ops. OP: 0 add, 1 or, 2 adc, 3 sbb, 4 and, 5 sub, 6 xor, 7 cmp, 8 test.
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
fn alu<const OP: u8, const SZ: u32>(cpu: &mut Cpu, a: u32, b: u32) -> u32 {
    match OP {
        ADD => cpu.alu_add(a, b, 0, SZ),
        ADC => {
            let c = cpu.cf();
            cpu.alu_add(a, b, c, SZ)
        }
        SUB | CMP => cpu.alu_sub(a, b, 0, SZ),
        SBB => {
            let c = cpu.cf();
            cpu.alu_sub(a, b, c, SZ)
        }
        AND | TEST => cpu.alu_logic(a & b, SZ),
        OR => cpu.alu_logic(a | b, SZ),
        _ => cpu.alu_logic(a ^ b, SZ),
    }
}

const fn writes(op: u8) -> bool {
    op != CMP && op != TEST
}

fn alu_rr<const OP: u8, const SZ: u32>(cpu: &mut Cpu, _: &mut GuestMemory, op: &Op) -> bool {
    let a = getr::<SZ>(cpu, op.r0);
    let b = getr::<SZ>(cpu, op.r1);
    let r = alu::<OP, SZ>(cpu, a, b);
    if writes(OP) {
        setr::<SZ>(cpu, op.r0, r);
    }
    true
}

fn alu_ri<const OP: u8, const SZ: u32>(cpu: &mut Cpu, _: &mut GuestMemory, op: &Op) -> bool {
    let a = getr::<SZ>(cpu, op.r0);
    let r = alu::<OP, SZ>(cpu, a, op.imm);
    if writes(OP) {
        setr::<SZ>(cpu, op.r0, r);
    }
    true
}

fn alu_rm<const OP: u8, const SZ: u32, const E: u8>(
    cpu: &mut Cpu,
    mem: &mut GuestMemory,
    op: &Op,
) -> bool {
    let b = tri!(cpu, ld::<SZ>(cpu, mem, ea::<E>(cpu, op)));
    let a = getr::<SZ>(cpu, op.r0);
    let r = alu::<OP, SZ>(cpu, a, b);
    if writes(OP) {
        setr::<SZ>(cpu, op.r0, r);
    }
    true
}

fn alu_mr<const OP: u8, const SZ: u32, const E: u8>(
    cpu: &mut Cpu,
    mem: &mut GuestMemory,
    op: &Op,
) -> bool {
    let a = ea::<E>(cpu, op);
    let v = tri!(cpu, ld::<SZ>(cpu, mem, a));
    let b = getr::<SZ>(cpu, op.r1);
    let r = alu::<OP, SZ>(cpu, v, b);
    if writes(OP) {
        tri!(cpu, st::<SZ>(cpu, mem, a, r));
    }
    true
}

fn alu_mi<const OP: u8, const SZ: u32, const E: u8>(
    cpu: &mut Cpu,
    mem: &mut GuestMemory,
    op: &Op,
) -> bool {
    let a = ea::<E>(cpu, op);
    let v = tri!(cpu, ld::<SZ>(cpu, mem, a));
    let r = alu::<OP, SZ>(cpu, v, op.imm);
    if writes(OP) {
        tri!(cpu, st::<SZ>(cpu, mem, a, r));
    }
    true
}

fn inc_r(cpu: &mut Cpu, _: &mut GuestMemory, op: &Op) -> bool {
    let d = op.r0 as usize & 7;
    cpu.regs[d] = cpu.alu_inc(cpu.regs[d], 4);
    true
}

fn dec_r(cpu: &mut Cpu, _: &mut GuestMemory, op: &Op) -> bool {
    let d = op.r0 as usize & 7;
    cpu.regs[d] = cpu.alu_dec(cpu.regs[d], 4);
    true
}

fn nop(_: &mut Cpu, _: &mut GuestMemory, _: &Op) -> bool {
    true
}

/// Generic path: the full interpreter (`exec::step`).
pub(crate) fn generic(cpu: &mut Cpu, mem: &mut GuestMemory, op: &Op) -> bool {
    // SAFETY: `op.ins` points into the owning block's instruction array (see `Op::ins`).
    let ins = unsafe { &*op.ins };
    cpu.cur_ip = ins.ip32();
    match exec::step(cpu, mem, ins) {
        Ok(()) => true,
        Err(e) => {
            cpu.pending_exit = Some(e);
            false
        }
    }
}

// ---------------------------------------------------------------------------------------------------------
// Selection

/// Register number for `Op` (32/16/8-bit GPRs), `None` for anything else.
fn reg_no(r: Register) -> Option<u8> {
    let n = r as u32;
    let (lo, hi) = if r.is_gpr32() {
        (Register::EAX as u32, Register::EDI as u32)
    } else if r.is_gpr16() {
        (Register::AX as u32, Register::DI as u32)
    } else if r.is_gpr8() {
        (Register::AL as u32, Register::BH as u32)
    } else {
        return None;
    };
    (lo..=hi).contains(&n).then(|| (n - lo) as u8)
}

/// Register operand `i` of size `sz` bytes.
fn reg_op(ins: &Instruction, i: u32, sz: u32) -> Option<u8> {
    if ins.op_kind(i) != OpKind::Register {
        return None;
    }
    let r = ins.op_register(i);
    if r.size() as u32 != sz {
        return None;
    }
    reg_no(r)
}

/// Memory operand description: (EA form, base, index, shift, disp). `None` for FS/GS overrides, 16-bit
/// addressing and anything unusual (those go generic).
fn mem_form(ins: &Instruction) -> Option<(u8, u8, u8, u8, u32)> {
    match ins.memory_segment() {
        Register::DS | Register::ES | Register::SS | Register::CS | Register::None => {}
        _ => return None,
    }
    let b = ins.memory_base();
    let x = ins.memory_index();
    let disp = ins.memory_displacement32();
    let bno = if b == Register::None {
        None
    } else if b.is_gpr32() {
        Some(reg_no(b)?)
    } else {
        return None;
    };
    let xno = if x == Register::None {
        None
    } else if x.is_gpr32() {
        Some(reg_no(x)?)
    } else {
        return None;
    };
    let shift = ins.memory_index_scale().trailing_zeros() as u8;
    Some(match (bno, xno) {
        (None, None) => (E_DISP, 0, 0, 0, disp),
        (Some(b), None) => (E_BASE, b, 0, 0, disp),
        (Some(b), Some(x)) => (E_BI, b, x, shift, disp),
        (None, Some(x)) => (E_IDX, 0, x, shift, disp),
    })
}

/// Pick a handler variant by EA form.
macro_rules! by_e {
    ($e:expr, $f:ident $(< $($g:tt),* >)?) => {
        match $e {
            E_DISP => $f::<$($($g,)*)? E_DISP> as Handler,
            E_BASE => $f::<$($($g,)*)? E_BASE> as Handler,
            E_BI => $f::<$($($g,)*)? E_BI> as Handler,
            _ => $f::<$($($g,)*)? E_IDX> as Handler,
        }
    };
}

/// Memory operand `i` of exactly `sz` bytes (0 = any, for LEA): EA form etc.
fn mem_op(ins: &Instruction, i: u32, sz: u32) -> Option<(u8, u8, u8, u8, u32)> {
    if ins.op_kind(i) != OpKind::Memory {
        return None;
    }
    if sz != 0 && ins.memory_size().size() as u32 != sz {
        return None;
    }
    mem_form(ins)
}

/// Immediate operand `i` as a value of the operand size.
fn imm_op(ins: &Instruction, i: u32) -> Option<u32> {
    Some(match ins.op_kind(i) {
        OpKind::Immediate8 => ins.immediate8() as u32,
        OpKind::Immediate16 => ins.immediate16() as u32,
        OpKind::Immediate32 => ins.immediate32(),
        OpKind::Immediate8to16 => ins.immediate8to16() as u16 as u32,
        OpKind::Immediate8to32 => ins.immediate8to32() as u32,
        _ => return None,
    })
}

/// Pick the handler and operands for a decoded instruction.
pub(crate) fn select(ins: &Instruction) -> Op {
    if ins.has_lock_prefix() && ins.op0_kind() != OpKind::Memory {
        // `lock` on a register destination is #UD; let the generic path decide.
        return Op::new(generic, ins);
    }
    specialised(ins).unwrap_or_else(|| Op::new(generic, ins))
}

/// Generic-only `Op` (tests / debugging).
pub(crate) fn select_generic(ins: &Instruction) -> Op {
    Op::new(generic, ins)
}

/// Operand size in bytes of a two-operand integer instruction (from operand 0).
fn size0(ins: &Instruction) -> u32 {
    match ins.op0_kind() {
        OpKind::Register => ins.op0_register().size() as u32,
        OpKind::Memory => ins.memory_size().size() as u32,
        _ => 0,
    }
}

/// ALU forms of one operation for sizes 1/2/4.
fn alu_select<const OP: u8>(ins: &Instruction, o: &mut Op) -> Option<Handler> {
    let sz = size0(ins);
    if ins.op_count() != 2 || !matches!(sz, 1 | 2 | 4) {
        return None;
    }
    macro_rules! forms {
        ($sz:literal) => {{
            if let (Some(a), Some(b)) = (reg_op(ins, 0, $sz), reg_op(ins, 1, $sz)) {
                o.r0 = a;
                o.r1 = b;
                Some(alu_rr::<OP, $sz> as Handler)
            } else if let (Some(a), Some(v)) = (reg_op(ins, 0, $sz), imm_op(ins, 1)) {
                o.r0 = a;
                o.imm = v;
                Some(alu_ri::<OP, $sz> as Handler)
            } else if let (Some(a), Some((e, b, x, s, d))) = (reg_op(ins, 0, $sz), mem_op(ins, 1, $sz)) {
                o.r0 = a;
                (o.base, o.index, o.shift, o.disp) = (b, x, s, d);
                Some(by_e!(e, alu_rm<OP, $sz>))
            } else if let (Some((e, b, x, s, d)), Some(r)) = (mem_op(ins, 0, $sz), reg_op(ins, 1, $sz)) {
                o.r1 = r;
                (o.base, o.index, o.shift, o.disp) = (b, x, s, d);
                Some(by_e!(e, alu_mr<OP, $sz>))
            } else if let (Some((e, b, x, s, d)), Some(v)) = (mem_op(ins, 0, $sz), imm_op(ins, 1)) {
                o.imm = v;
                (o.base, o.index, o.shift, o.disp) = (b, x, s, d);
                Some(by_e!(e, alu_mi<OP, $sz>))
            } else {
                None
            }
        }};
    }
    match sz {
        1 => forms!(1),
        2 => forms!(2),
        _ => forms!(4),
    }
}

fn mov_select(ins: &Instruction, o: &mut Op) -> Option<Handler> {
    let sz = size0(ins);
    if ins.op_count() != 2 || !matches!(sz, 1 | 2 | 4) {
        return None;
    }
    // Segment-register moves (`mov ax, ds` etc.) fail reg_op and go generic.
    macro_rules! forms {
        ($sz:literal) => {{
            if let (Some(a), Some(b)) = (reg_op(ins, 0, $sz), reg_op(ins, 1, $sz)) {
                o.r0 = a;
                o.r1 = b;
                Some(mov_rr::<$sz> as Handler)
            } else if let (Some(a), Some((e, b, x, s, d))) = (reg_op(ins, 0, $sz), mem_op(ins, 1, $sz)) {
                o.r0 = a;
                (o.base, o.index, o.shift, o.disp) = (b, x, s, d);
                Some(by_e!(e, mov_rm<$sz>))
            } else if let (Some((e, b, x, s, d)), Some(r)) = (mem_op(ins, 0, $sz), reg_op(ins, 1, $sz)) {
                o.r1 = r;
                (o.base, o.index, o.shift, o.disp) = (b, x, s, d);
                Some(by_e!(e, mov_mr<$sz>))
            } else if let (Some(a), Some(v)) = (reg_op(ins, 0, $sz), imm_op(ins, 1)) {
                o.r0 = a;
                o.imm = v;
                Some(mov_ri::<$sz> as Handler)
            } else if let (Some((e, b, x, s, d)), Some(v)) = (mem_op(ins, 0, $sz), imm_op(ins, 1)) {
                o.imm = v;
                (o.base, o.index, o.shift, o.disp) = (b, x, s, d);
                Some(by_e!(e, mov_mi<$sz>))
            } else {
                None
            }
        }};
    }
    match sz {
        1 => forms!(1),
        2 => forms!(2),
        _ => forms!(4),
    }
}

fn specialised(ins: &Instruction) -> Option<Op> {
    use Mnemonic as M;
    let mut o = Op::new(generic, ins);
    let m = ins.mnemonic();
    let set_mem = |o: &mut Op, f: (u8, u8, u8, u8, u32)| -> u8 {
        (o.base, o.index, o.shift, o.disp) = (f.1, f.2, f.3, f.4);
        f.0
    };
    let h: Handler = match m {
        M::Mov => mov_select(ins, &mut o)?,
        M::Movzx | M::Movsx => {
            o.r0 = reg_op(ins, 0, 4)?;
            let signed = m == M::Movsx;
            if let Some(f) = mem_op(ins, 1, 0) {
                let e = set_mem(&mut o, f);
                match (signed, ins.memory_size().size()) {
                    (false, 1) => by_e!(e, movx_rm<1, false>),
                    (false, 2) => by_e!(e, movx_rm<2, false>),
                    (true, 1) => by_e!(e, movx_rm<1, true>),
                    (true, 2) => by_e!(e, movx_rm<2, true>),
                    _ => return None,
                }
            } else if let Some(r) = reg_op(ins, 1, 1) {
                o.r1 = r;
                if signed {
                    movx_rr::<1, true>
                } else {
                    movx_rr::<1, false>
                }
            } else if let Some(r) = reg_op(ins, 1, 2) {
                o.r1 = r;
                if signed {
                    movx_rr::<2, true>
                } else {
                    movx_rr::<2, false>
                }
            } else {
                return None;
            }
        }
        M::Shl | M::Sal | M::Shr | M::Sar if ins.op1_kind() == OpKind::Immediate8 => {
            o.r0 = reg_op(ins, 0, 4)?;
            o.imm = ins.immediate8() as u32;
            match m {
                M::Shr => shift_ri::<1>,
                M::Sar => shift_ri::<2>,
                _ => shift_ri::<0>,
            }
        }
        M::Lea => {
            o.r0 = reg_op(ins, 0, 4)?;
            let f = mem_op(ins, 1, 0)?;
            let e = set_mem(&mut o, f);
            by_e!(e, lea)
        }
        M::Push => match ins.code() {
            Code::Push_r32 => {
                o.r0 = reg_op(ins, 0, 4)?;
                push_r
            }
            Code::Pushd_imm8 | Code::Pushd_imm32 => {
                o.imm = imm_op(ins, 0)?;
                push_i
            }
            Code::Push_rm32 => {
                if let Some(r) = reg_op(ins, 0, 4) {
                    o.r0 = r;
                    push_r
                } else {
                    let f = mem_op(ins, 0, 4)?;
                    let e = set_mem(&mut o, f);
                    by_e!(e, push_m)
                }
            }
            _ => return None,
        },
        M::Pop => match ins.code() {
            Code::Pop_r32 | Code::Pop_rm32 => {
                o.r0 = reg_op(ins, 0, 4)?;
                pop_r
            }
            _ => return None,
        },
        M::Call => match ins.code() {
            Code::Call_rel32_32 => {
                o.imm = ins.near_branch32();
                call_rel
            }
            Code::Call_rm32 => {
                if let Some(r) = reg_op(ins, 0, 4) {
                    o.r0 = r;
                    call_r
                } else {
                    let f = mem_op(ins, 0, 4)?;
                    let e = set_mem(&mut o, f);
                    by_e!(e, call_m)
                }
            }
            _ => return None,
        },
        M::Ret => match ins.code() {
            Code::Retnd => ret,
            Code::Retnd_imm16 => {
                o.imm = ins.immediate16() as u32;
                ret
            }
            _ => return None,
        },
        M::Jmp => match ins.code() {
            Code::Jmp_rel8_32 | Code::Jmp_rel32_32 => {
                o.imm = ins.near_branch32();
                jmp_rel
            }
            Code::Jmp_rm32 => {
                if let Some(r) = reg_op(ins, 0, 4) {
                    o.r0 = r;
                    jmp_r
                } else {
                    let f = mem_op(ins, 0, 4)?;
                    let e = set_mem(&mut o, f);
                    by_e!(e, jmp_m)
                }
            }
            _ => return None,
        },
        _ if ins.flow_control() == FlowControl::ConditionalBranch
            && ins.condition_code() != ConditionCode::None
            && ins.op0_kind() == OpKind::NearBranch32
            && ins.is_jcc_short_or_near() =>
        {
            o.imm = ins.near_branch32();
            jcc_handler(ins.condition_code())
        }
        M::Add => alu_select::<ADD>(ins, &mut o)?,
        M::Or => alu_select::<OR>(ins, &mut o)?,
        M::Adc => alu_select::<ADC>(ins, &mut o)?,
        M::Sbb => alu_select::<SBB>(ins, &mut o)?,
        M::And => alu_select::<AND>(ins, &mut o)?,
        M::Sub => alu_select::<SUB>(ins, &mut o)?,
        M::Xor => alu_select::<XOR>(ins, &mut o)?,
        M::Cmp => alu_select::<CMP>(ins, &mut o)?,
        M::Test => alu_select::<TEST>(ins, &mut o)?,
        M::Inc => {
            o.r0 = reg_op(ins, 0, 4)?;
            inc_r
        }
        M::Dec => {
            o.r0 = reg_op(ins, 0, 4)?;
            dec_r
        }
        M::Nop => nop,
        _ => return None,
    };
    o.h = h;
    Some(o)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `cond_fast` (lazy shortcuts) must agree with evaluating the materialised EFLAGS, for every lazy kind,
    /// operand size and condition code.
    #[test]
    fn lazy_conditions_match_materialised_flags() {
        let vals = [
            0u32, 1, 2, 0x7F, 0x80, 0xFF, 0x100, 0x7FFF, 0x8000, 0xFFFF, 0x1_0000, 0x7FFF_FFFF, 0x8000_0000,
            0x8000_0001, 0xFFFF_FFFE, 0xFFFF_FFFF, 0x1234_5678, 0xDEAD_BEEF,
        ];
        macro_rules! check_all {
            ($cpu:expr) => {{
                let c = $cpu;
                let f = c.eflags();
                assert_eq!(c.cf(), f & fl::CF, "cf");
                assert_eq!(c.af(), f & fl::AF, "af");
                assert_eq!(c.of(), (f >> 11) & 1, "of");
                for bit in [fl::CF, fl::PF, fl::AF, fl::ZF, fl::SF, fl::OF] {
                    assert_eq!(c.flag(bit), f & bit != 0, "flag {bit:#x}");
                }
                macro_rules! one {
                    ($cc:ident) => {
                        assert_eq!(
                            cond_fast::<{ ConditionCode::$cc as u8 }>(c),
                            cc_eval::<{ ConditionCode::$cc as u8 }>(f),
                            "cc {} lk {} sz {} a {:#x} b {:#x} r {:#x}",
                            stringify!($cc),
                            c.lk,
                            c.lsz,
                            c.la,
                            c.lb,
                            c.lr
                        );
                    };
                }
                one!(o);
                one!(no);
                one!(b);
                one!(ae);
                one!(e);
                one!(ne);
                one!(be);
                one!(a);
                one!(s);
                one!(ns);
                one!(p);
                one!(np);
                one!(l);
                one!(ge);
                one!(le);
                one!(g);
            }};
        }
        for &sz in &[1u32, 2, 4] {
            for &a in &vals {
                for &b in &vals {
                    for cf_in in [0u32, 1] {
                        let mut c = Cpu::new();
                        c.set_eflags(0x202 | cf_in);
                        c.alu_sub(a, b, 0, sz);
                        check_all!(&c);
                        c.alu_add(a, b, 0, sz);
                        check_all!(&c);
                        c.alu_logic(a & b, sz);
                        check_all!(&c);
                        c.alu_logic(a ^ b, sz);
                        check_all!(&c);
                        c.set_eflags(0x202 | cf_in);
                        c.alu_inc(a, sz);
                        check_all!(&c);
                        assert_eq!(c.cf(), cf_in);
                        c.set_eflags(0x202 | cf_in);
                        c.alu_dec(a, sz);
                        check_all!(&c);
                        assert_eq!(c.cf(), cf_in);
                    }
                }
            }
        }
    }
}
