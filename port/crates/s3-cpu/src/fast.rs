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
//! Handler contract (tail-call threading, 2026-11-07): the run loop calls the first `Op`'s handler; every
//! non-branch handler finishes with [`next`] (a tail call into the next `Op`'s handler), the last one reaches the
//! [`block_end`] sentinel or is a control transfer that returns `true` itself. `cpu.eip` holds the address after
//! the block's *last* instruction while the block runs (no other instruction reads EIP); branches overwrite it.
//! Fast handlers do not maintain `cpu.cur_ip`; on failure they store the exit in `cpu.pending_exit` and their
//! `Op` in `cpu.fault_op` and return `false`; the run loop rewinds EIP and fixes the fault EIP and the count.
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

/// Pre-decoded operands of one instruction (filled by [`select`]; unused fields are 0). 32 bytes, so two
/// share a cache line.
#[derive(Clone, Copy)]
pub(crate) struct Op {
    pub h: Handler,
    /// Address of the next instruction.
    pub next: u32,
    /// Memory displacement. For [`generic`] ops: low half of the `Instruction` pointer (see [`Op::set_ins`]).
    pub disp: u32,
    /// Immediate (already sign/zero-extended to the operand size) or branch target. For [`generic`] ops: high
    /// half of the `Instruction` pointer.
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
}

const _: () = assert!(std::mem::size_of::<Op>() <= 32);

impl std::fmt::Debug for Op {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Op(next {:#x})", self.next)
    }
}

impl Op {
    pub(crate) fn new(h: Handler, ins: &Instruction) -> Op {
        Op { h, next: ins.next_ip32(), disp: 0, imm: 0, r0: 0, r1: 0, base: 0, index: 0, shift: 0 }
    }

    /// For a [`generic`] op: remember its decoded instruction (in `disp`/`imm`, which the generic path does not
    /// use). It points into the owning `Block::insns`, whose heap buffer is never reallocated afterwards.
    pub(crate) fn set_ins(&mut self, ins: *const Instruction) {
        debug_assert!(self.h as usize == generic as Handler as usize);
        let p = ins as usize as u64;
        self.disp = p as u32;
        self.imm = (p >> 32) as u32;
    }

    #[inline(always)]
    fn ins(&self) -> *const Instruction {
        (((self.imm as u64) << 32) | self.disp as u64) as usize as *const Instruction
    }
}

// EA forms.
pub(crate) const E_DISP: u8 = 0;
pub(crate) const E_BASE: u8 = 1;
pub(crate) const E_BI: u8 = 2;
pub(crate) const E_IDX: u8 = 3;

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

/// Continue with the next `Op` of the block (threaded code: a tail call, so the dispatch is one indirect
/// branch per instruction and every handler has its own branch-prediction site). Every block's `ops` ends
/// with the [`block_end`] sentinel, so `op + 1` exists for every real instruction.
#[inline(always)]
fn next(cpu: &mut Cpu, mem: &mut GuestMemory, op: &Op) -> bool {
    // SAFETY: see above; `op` points into a block's `ops` array, which is not the sentinel.
    let n = unsafe { &*(op as *const Op).add(1) };
    (n.h)(cpu, mem, n)
}

/// Sentinel after the last instruction of every block: the block completed.
pub(crate) fn block_end(_: &mut Cpu, _: &mut GuestMemory, _: &Op) -> bool {
    true
}

/// Store an access-violation exit and the faulting `Op` (out of line and reached by a tail call, so the
/// handlers' fast paths need no stack frame).
#[cold]
#[inline(never)]
fn av(cpu: &mut Cpu, op: &Op, addr: u32, write: bool) -> bool {
    cpu.pending_exit = Some(cpu.av_exit(addr, write));
    cpu.fault_op = op as *const Op as usize;
    false
}

#[inline(always)]
fn rd<const SZ: u32>(mem: &GuestMemory, a: u32) -> u32 {
    match SZ {
        1 => mem.read_u8(a) as u32,
        2 => mem.read_u16(a) as u32,
        _ => mem.read_u32(a),
    }
}

#[inline(always)]
fn wr<const SZ: u32>(mem: &mut GuestMemory, a: u32, v: u32) {
    match SZ {
        1 => mem.write_u8(a, v as u8),
        2 => mem.write_u16(a, v as u16),
        _ => mem.write_u32(a, v),
    }
}

/// Guest load inside a handler: an address in the NULL guard returns `false` from the handler via [`av`].
macro_rules! ld {
    ($cpu:ident, $mem:ident, $op:ident, $sz:expr, $a:expr) => {{
        let a: u32 = $a;
        if a < crate::layout::NULL_GUARD_END {
            return av($cpu, $op, a, false);
        }
        rd::<{ $sz }>($mem, a)
    }};
}

/// Guest store inside a handler (see [`ld!`]).
macro_rules! st {
    ($cpu:ident, $mem:ident, $op:ident, $sz:expr, $a:expr, $v:expr) => {{
        let a: u32 = $a;
        let v: u32 = $v;
        if a < crate::layout::NULL_GUARD_END {
            return av($cpu, $op, a, true);
        }
        wr::<{ $sz }>($mem, a, v)
    }};
}

/// Push a dword inside a handler (ESP changes only if the store succeeds).
macro_rules! push {
    ($cpu:ident, $mem:ident, $op:ident, $v:expr) => {{
        let v: u32 = $v;
        let sp = $cpu.regs[ESP].wrapping_sub(4);
        st!($cpu, $mem, $op, 4, sp, v);
        $cpu.regs[ESP] = sp;
    }};
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

fn mov_rr<const SZ: u32>(cpu: &mut Cpu, mem: &mut GuestMemory, op: &Op) -> bool {
    let v = getr::<SZ>(cpu, op.r1);
    setr::<SZ>(cpu, op.r0, v);
    next(cpu, mem, op)
}

fn mov_rm<const SZ: u32, const E: u8>(cpu: &mut Cpu, mem: &mut GuestMemory, op: &Op) -> bool {
    let v = ld!(cpu, mem, op, SZ, ea::<E>(cpu, op));
    setr::<SZ>(cpu, op.r0, v);
    next(cpu, mem, op)
}

fn mov_mr<const SZ: u32, const E: u8>(cpu: &mut Cpu, mem: &mut GuestMemory, op: &Op) -> bool {
    let v = getr::<SZ>(cpu, op.r1);
    st!(cpu, mem, op, SZ, ea::<E>(cpu, op), v);
    next(cpu, mem, op)
}

fn mov_ri<const SZ: u32>(cpu: &mut Cpu, mem: &mut GuestMemory, op: &Op) -> bool {
    setr::<SZ>(cpu, op.r0, op.imm);
    next(cpu, mem, op)
}

fn mov_mi<const SZ: u32, const E: u8>(cpu: &mut Cpu, mem: &mut GuestMemory, op: &Op) -> bool {
    st!(cpu, mem, op, SZ, ea::<E>(cpu, op), op.imm);
    next(cpu, mem, op)
}

/// movzx/movsx r32 from memory: SZ = 1 or 2, SIGNED.
fn movx_rm<const SZ: u32, const SIGNED: bool, const E: u8>(
    cpu: &mut Cpu,
    mem: &mut GuestMemory,
    op: &Op,
) -> bool {
    let v = ld!(cpu, mem, op, SZ, ea::<E>(cpu, op));
    cpu.regs[op.r0 as usize & 7] = if SIGNED { crate::flags::sext(v, SZ) as u32 } else { v };
    next(cpu, mem, op)
}

/// movzx/movsx r32 from an 8/16-bit register.
fn movx_rr<const SZ: u32, const SIGNED: bool>(cpu: &mut Cpu, mem: &mut GuestMemory, op: &Op) -> bool {
    let v = getr::<SZ>(cpu, op.r1);
    cpu.regs[op.r0 as usize & 7] = if SIGNED { crate::flags::sext(v, SZ) as u32 } else { v };
    next(cpu, mem, op)
}

/// shl/shr/sar r32, imm8 (KIND: 0 shl, 1 shr, 2 sar). Count in `op.imm`.
fn shift_ri<const KIND: u8>(cpu: &mut Cpu, mem: &mut GuestMemory, op: &Op) -> bool {
    let d = op.r0 as usize & 7;
    let a = cpu.regs[d];
    let c = op.imm & 0x1F;
    if c == 0 {
        return next(cpu, mem, op);
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
    next(cpu, mem, op)
}

fn lea<const E: u8>(cpu: &mut Cpu, mem: &mut GuestMemory, op: &Op) -> bool {
    cpu.regs[op.r0 as usize & 7] = ea::<E>(cpu, op);
    next(cpu, mem, op)
}

fn push_r(cpu: &mut Cpu, mem: &mut GuestMemory, op: &Op) -> bool {
    push!(cpu, mem, op, cpu.regs[op.r0 as usize & 7]);
    next(cpu, mem, op)
}

fn push_i(cpu: &mut Cpu, mem: &mut GuestMemory, op: &Op) -> bool {
    push!(cpu, mem, op, op.imm);
    next(cpu, mem, op)
}

fn push_m<const E: u8>(cpu: &mut Cpu, mem: &mut GuestMemory, op: &Op) -> bool {
    let v = ld!(cpu, mem, op, 4, ea::<E>(cpu, op));
    push!(cpu, mem, op, v);
    next(cpu, mem, op)
}

fn pop_r(cpu: &mut Cpu, mem: &mut GuestMemory, op: &Op) -> bool {
    let v = ld!(cpu, mem, op, 4, cpu.regs[ESP]);
    cpu.regs[ESP] = cpu.regs[ESP].wrapping_add(4);
    // `pop esp` loads the popped value (the increment is overwritten).
    cpu.regs[op.r0 as usize & 7] = v;
    next(cpu, mem, op)
}

// Control transfers are always the last instruction of their block (`decode_block`), so they return to the
// run loop directly instead of dispatching the `block_end` sentinel.

fn call_rel(cpu: &mut Cpu, mem: &mut GuestMemory, op: &Op) -> bool {
    push!(cpu, mem, op, op.next);
    cpu.eip = op.imm;
    true
}

fn call_r(cpu: &mut Cpu, mem: &mut GuestMemory, op: &Op) -> bool {
    let t = cpu.regs[op.r0 as usize & 7];
    push!(cpu, mem, op, op.next);
    cpu.eip = t;
    true
}

fn call_m<const E: u8>(cpu: &mut Cpu, mem: &mut GuestMemory, op: &Op) -> bool {
    let t = ld!(cpu, mem, op, 4, ea::<E>(cpu, op));
    push!(cpu, mem, op, op.next);
    cpu.eip = t;
    true
}

fn ret(cpu: &mut Cpu, mem: &mut GuestMemory, op: &Op) -> bool {
    let v = ld!(cpu, mem, op, 4, cpu.regs[ESP]);
    cpu.regs[ESP] = cpu.regs[ESP].wrapping_add(4 + op.imm);
    cpu.eip = v;
    true
}

fn jmp_rel(cpu: &mut Cpu, _: &mut GuestMemory, op: &Op) -> bool {
    cpu.eip = op.imm;
    true
}

fn jmp_m<const E: u8>(cpu: &mut Cpu, mem: &mut GuestMemory, op: &Op) -> bool {
    cpu.eip = ld!(cpu, mem, op, 4, ea::<E>(cpu, op));
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

/// [`cc_eval`] with a run-time condition code (iced `ConditionCode as u8`; JIT helper).
#[cfg_attr(not(all(feature = "jit", target_arch = "aarch64")), allow(dead_code))]
pub(crate) fn cc_eval_rt(f: u32, cc: u8) -> bool {
    use ConditionCode as C;
    macro_rules! m {
        ($($c:ident),*) => {
            match cc {
                $(x if x == C::$c as u8 => cc_eval::<{ C::$c as u8 }>(f),)*
                _ => true,
            }
        };
    }
    m!(o, no, b, ae, e, ne, be, a, s, ns, p, np, l, ge, le, g)
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

fn alu_rr<const OP: u8, const SZ: u32>(cpu: &mut Cpu, mem: &mut GuestMemory, op: &Op) -> bool {
    let a = getr::<SZ>(cpu, op.r0);
    let b = getr::<SZ>(cpu, op.r1);
    let r = alu::<OP, SZ>(cpu, a, b);
    if writes(OP) {
        setr::<SZ>(cpu, op.r0, r);
    }
    next(cpu, mem, op)
}

fn alu_ri<const OP: u8, const SZ: u32>(cpu: &mut Cpu, mem: &mut GuestMemory, op: &Op) -> bool {
    let a = getr::<SZ>(cpu, op.r0);
    let r = alu::<OP, SZ>(cpu, a, op.imm);
    if writes(OP) {
        setr::<SZ>(cpu, op.r0, r);
    }
    next(cpu, mem, op)
}

fn alu_rm<const OP: u8, const SZ: u32, const E: u8>(
    cpu: &mut Cpu,
    mem: &mut GuestMemory,
    op: &Op,
) -> bool {
    let b = ld!(cpu, mem, op, SZ, ea::<E>(cpu, op));
    let a = getr::<SZ>(cpu, op.r0);
    let r = alu::<OP, SZ>(cpu, a, b);
    if writes(OP) {
        setr::<SZ>(cpu, op.r0, r);
    }
    next(cpu, mem, op)
}

fn alu_mr<const OP: u8, const SZ: u32, const E: u8>(
    cpu: &mut Cpu,
    mem: &mut GuestMemory,
    op: &Op,
) -> bool {
    let a = ea::<E>(cpu, op);
    let v = ld!(cpu, mem, op, SZ, a);
    let b = getr::<SZ>(cpu, op.r1);
    let r = alu::<OP, SZ>(cpu, v, b);
    if writes(OP) {
        st!(cpu, mem, op, SZ, a, r);
    }
    next(cpu, mem, op)
}

fn alu_mi<const OP: u8, const SZ: u32, const E: u8>(
    cpu: &mut Cpu,
    mem: &mut GuestMemory,
    op: &Op,
) -> bool {
    let a = ea::<E>(cpu, op);
    let v = ld!(cpu, mem, op, SZ, a);
    let r = alu::<OP, SZ>(cpu, v, op.imm);
    if writes(OP) {
        st!(cpu, mem, op, SZ, a, r);
    }
    next(cpu, mem, op)
}

fn inc_r(cpu: &mut Cpu, mem: &mut GuestMemory, op: &Op) -> bool {
    let d = op.r0 as usize & 7;
    cpu.regs[d] = cpu.alu_inc(cpu.regs[d], 4);
    next(cpu, mem, op)
}

fn dec_r(cpu: &mut Cpu, mem: &mut GuestMemory, op: &Op) -> bool {
    let d = op.r0 as usize & 7;
    cpu.regs[d] = cpu.alu_dec(cpu.regs[d], 4);
    next(cpu, mem, op)
}

fn nop(cpu: &mut Cpu, mem: &mut GuestMemory, op: &Op) -> bool {
    next(cpu, mem, op)
}

// perf-interp (2026-11-07): fast forms for what the in-game profile still sent to the generic path (imul/mul,
// cdq/cwde, 8/16-bit shifts and shifts by CL, setcc, neg/not, inc/dec m32). Each mirrors `exec::step`.

/// shl/shr/sar (KIND 0/1/2) of a `SZ`-byte value by `c` (already masked to 5 bits, != 0): result and flags
/// exactly as `Cpu::shift` (exec.rs): CF = last bit out (0 when the count exceeds the width), OF as for a 1-bit
/// shift, SZP from the result, AF unchanged.
#[inline(always)]
fn shift_val<const KIND: u8, const SZ: u32>(cpu: &mut Cpu, a: u32, c: u32) -> u32 {
    use crate::flags::{mask, sext, sign_bit, szp};
    let bits = SZ * 8;
    let msk = mask(SZ);
    let sb = sign_bit(SZ);
    let a = a & msk;
    let (r, cf, of) = match KIND {
        0 => {
            let r = ((a as u64) << c) as u32 & msk;
            let cf = c <= bits && ((a as u64) >> (bits - c)) & 1 != 0;
            (r, cf, ((r & sb) != 0) != cf)
        }
        1 => {
            let r = if c >= bits { 0 } else { a >> c };
            let cf = c <= bits && ((a as u64) >> (c - 1)) & 1 != 0;
            (r, cf, a & sb != 0)
        }
        _ => {
            let s = sext(a, SZ);
            let r = (s >> c.min(31)) as u32 & msk;
            let cf = (s >> (c.min(bits) - 1).min(31)) & 1 != 0;
            (r, cf, false)
        }
    };
    let af = cpu.af();
    cpu.set_eflags((cpu.ef & !fl::STATUS) | af | cf as u32 | if of { fl::OF } else { 0 } | szp(r, SZ));
    r
}

/// shl/shr/sar r8/r16 (and r32 via `shift_ri`), imm8 in `op.imm`.
fn shift_ri_n<const KIND: u8, const SZ: u32>(cpu: &mut Cpu, mem: &mut GuestMemory, op: &Op) -> bool {
    let c = op.imm & 0x1F;
    if c != 0 {
        let r = shift_val::<KIND, SZ>(cpu, getr::<SZ>(cpu, op.r0), c);
        setr::<SZ>(cpu, op.r0, r);
    }
    next(cpu, mem, op)
}

/// shl/shr/sar r8/r16/r32, cl.
fn shift_rc<const KIND: u8, const SZ: u32>(cpu: &mut Cpu, mem: &mut GuestMemory, op: &Op) -> bool {
    let c = cpu.regs[crate::ECX] & 0x1F;
    if c != 0 {
        let r = shift_val::<KIND, SZ>(cpu, getr::<SZ>(cpu, op.r0), c);
        setr::<SZ>(cpu, op.r0, r);
    }
    next(cpu, mem, op)
}

/// Flags of a 32-bit multiply as `exec::step` sets them: SZP from the low result, CF = OF = overflow, AF
/// unchanged.
#[inline(always)]
fn mul_flags(cpu: &mut Cpu, lo: u32, of: bool) {
    let f = cpu.eflags();
    let keep = f & !(fl::SF | fl::ZF | fl::PF | fl::CF | fl::OF);
    cpu.set_eflags(keep | crate::flags::szp(lo, 4) | if of { fl::CF | fl::OF } else { 0 });
}

#[inline(always)]
fn imul32(cpu: &mut Cpu, a: u32, b: u32) -> u32 {
    let r = a as i32 as i64 * b as i32 as i64;
    let lo = r as u32;
    mul_flags(cpu, lo, lo as i32 as i64 != r);
    lo
}

/// `imul r32, r32` (SRC_MEM false) / `imul r32, m32`.
fn imul_r<const SRC_MEM: bool, const E: u8>(cpu: &mut Cpu, mem: &mut GuestMemory, op: &Op) -> bool {
    let b = if SRC_MEM { ld!(cpu, mem, op, 4, ea::<E>(cpu, op)) } else { cpu.regs[op.r1 as usize & 7] };
    let a = cpu.regs[op.r0 as usize & 7];
    cpu.regs[op.r0 as usize & 7] = imul32(cpu, a, b);
    next(cpu, mem, op)
}

/// `imul r32, r32, imm` / `imul r32, m32, imm` (imm in `op.imm`).
fn imul_ri<const SRC_MEM: bool, const E: u8>(cpu: &mut Cpu, mem: &mut GuestMemory, op: &Op) -> bool {
    let a = if SRC_MEM { ld!(cpu, mem, op, 4, ea::<E>(cpu, op)) } else { cpu.regs[op.r1 as usize & 7] };
    cpu.regs[op.r0 as usize & 7] = imul32(cpu, a, op.imm);
    next(cpu, mem, op)
}

/// One-operand 32-bit `mul` (SIGNED false) / `imul`: EDX:EAX = EAX * src.
fn mul1<const SIGNED: bool, const SRC_MEM: bool, const E: u8>(
    cpu: &mut Cpu,
    mem: &mut GuestMemory,
    op: &Op,
) -> bool {
    let b = if SRC_MEM { ld!(cpu, mem, op, 4, ea::<E>(cpu, op)) } else { cpu.regs[op.r0 as usize & 7] };
    let a = cpu.regs[crate::EAX];
    let (lo, hi, of) = if SIGNED {
        let r = a as i32 as i64 * b as i32 as i64;
        (r as u32, (r >> 32) as u32, r != r as i32 as i64)
    } else {
        let r = a as u64 * b as u64;
        (r as u32, (r >> 32) as u32, r > 0xFFFF_FFFF)
    };
    cpu.regs[crate::EAX] = lo;
    cpu.regs[crate::EDX] = hi;
    mul_flags(cpu, lo, of);
    next(cpu, mem, op)
}

fn cdq(cpu: &mut Cpu, mem: &mut GuestMemory, op: &Op) -> bool {
    cpu.regs[crate::EDX] = ((cpu.regs[crate::EAX] as i32) >> 31) as u32;
    next(cpu, mem, op)
}

fn cwde(cpu: &mut Cpu, mem: &mut GuestMemory, op: &Op) -> bool {
    cpu.regs[crate::EAX] = cpu.regs[crate::EAX] as u16 as i16 as i32 as u32;
    next(cpu, mem, op)
}

/// `setcc r8`.
fn setcc_r<const CC: u8>(cpu: &mut Cpu, mem: &mut GuestMemory, op: &Op) -> bool {
    let v = cond_fast::<CC>(cpu) as u32;
    setr::<1>(cpu, op.r0, v);
    next(cpu, mem, op)
}

fn setcc_handler(cc: ConditionCode) -> Option<Handler> {
    use ConditionCode as C;
    macro_rules! s {
        ($c:ident) => {
            setcc_r::<{ C::$c as u8 }> as Handler
        };
    }
    Some(match cc {
        C::o => s!(o),
        C::no => s!(no),
        C::b => s!(b),
        C::ae => s!(ae),
        C::e => s!(e),
        C::ne => s!(ne),
        C::be => s!(be),
        C::a => s!(a),
        C::s => s!(s),
        C::ns => s!(ns),
        C::p => s!(p),
        C::np => s!(np),
        C::l => s!(l),
        C::ge => s!(ge),
        C::le => s!(le),
        C::g => s!(g),
        C::None => return None,
    })
}

fn neg_r(cpu: &mut Cpu, mem: &mut GuestMemory, op: &Op) -> bool {
    let d = op.r0 as usize & 7;
    cpu.regs[d] = cpu.alu_sub(0, cpu.regs[d], 0, 4);
    next(cpu, mem, op)
}

fn not_r(cpu: &mut Cpu, mem: &mut GuestMemory, op: &Op) -> bool {
    let d = op.r0 as usize & 7;
    cpu.regs[d] = !cpu.regs[d];
    next(cpu, mem, op)
}

/// `inc m32` (DEC false) / `dec m32`.
fn incdec_m<const DEC: bool, const E: u8>(cpu: &mut Cpu, mem: &mut GuestMemory, op: &Op) -> bool {
    let a = ea::<E>(cpu, op);
    let v = ld!(cpu, mem, op, 4, a);
    let r = if DEC { cpu.alu_dec(v, 4) } else { cpu.alu_inc(v, 4) };
    st!(cpu, mem, op, 4, a, r);
    next(cpu, mem, op)
}

// ---------------------------------------------------------------------------------------------------------
// Superinstructions: a flag-setting compare followed by the `jcc` that ends the block (perf-interp, 2026-11-07).
//
// The fused handler is installed on the compare's `Op` (decode-time peephole, [`fuse_cmp_jcc`]); the `jcc` keeps
// its own `Op` right behind it (branch target in `imm`), which the fused handler reads and then skips by
// returning to the run loop. The body is literally the compare handler's code followed by the `jcc` handler's
// code, so results, lazy flags, faults (reported at the compare's `Op`) and the retired count (the block's
// instruction count) are those of the two separate instructions; the compiler only folds the condition test
// into the freshly computed operands.

const F_RR: u8 = 0;
const F_RI: u8 = 1;
const F_MI: u8 = 2;
const F_DEC: u8 = 3;

/// `cmp`/`test` (OP) in form F (`r,r` / `r,imm` / `m,imm`), or `dec r32` (F_DEC), then `jcc` (CC).
fn cmp_jcc<const OP: u8, const SZ: u32, const F: u8, const CC: u8, const E: u8>(
    cpu: &mut Cpu,
    mem: &mut GuestMemory,
    op: &Op,
) -> bool {
    match F {
        F_RR => {
            let (a, b) = (getr::<SZ>(cpu, op.r0), getr::<SZ>(cpu, op.r1));
            alu::<OP, SZ>(cpu, a, b);
        }
        F_RI => {
            let a = getr::<SZ>(cpu, op.r0);
            alu::<OP, SZ>(cpu, a, op.imm);
        }
        F_MI => {
            let v = ld!(cpu, mem, op, SZ, ea::<E>(cpu, op));
            alu::<OP, SZ>(cpu, v, op.imm);
        }
        _ => {
            let d = op.r0 as usize & 7;
            cpu.regs[d] = cpu.alu_dec(cpu.regs[d], 4);
        }
    }
    // SAFETY: the `jcc` is the next (and last) instruction of the block (see `fuse_cmp_jcc`).
    let j = unsafe { &*(op as *const Op).add(1) };
    if cond_fast::<CC>(cpu) {
        cpu.eip = j.imm;
    }
    true
}

/// Fused handler for the compare `ins` (already selected into a fast `Op` with its operands) followed by a
/// fast `jcc` with condition `cc`, or `None` if this pair is not fused.
pub(crate) fn fuse_cmp_jcc(ins: &Instruction, cc: ConditionCode) -> Option<Handler> {
    let sz = size0(ins);
    let op = match ins.mnemonic() {
        Mnemonic::Cmp => CMP,
        Mnemonic::Test => TEST,
        Mnemonic::Dec => {
            reg_op(ins, 0, 4)?;
            return fuse_cc::<CMP, 4, F_DEC, E_DISP>(cc);
        }
        _ => return None,
    };
    // Same form detection (and therefore the same `Op` operand fields) as `alu_select`.
    if ins.op_count() != 2 || !matches!(sz, 1 | 2 | 4) {
        return None;
    }
    let form = if reg_op(ins, 0, sz).is_some() && reg_op(ins, 1, sz).is_some() {
        F_RR
    } else if reg_op(ins, 0, sz).is_some() && imm_op(ins, 1).is_some() {
        F_RI
    } else if let (Some((e, ..)), Some(_)) = (mem_op(ins, 0, sz), imm_op(ins, 1)) {
        return match (op, sz, e) {
            (CMP, 1, E_BASE) => fuse_cc::<CMP, 1, F_MI, E_BASE>(cc),
            (CMP, 1, E_DISP) => fuse_cc::<CMP, 1, F_MI, E_DISP>(cc),
            (CMP, 4, E_BASE) => fuse_cc::<CMP, 4, F_MI, E_BASE>(cc),
            (CMP, 4, E_DISP) => fuse_cc::<CMP, 4, F_MI, E_DISP>(cc),
            (CMP, 2, E_BASE) => fuse_cc::<CMP, 2, F_MI, E_BASE>(cc),
            (TEST, 1, E_BASE) => fuse_cc::<TEST, 1, F_MI, E_BASE>(cc),
            (TEST, 1, E_DISP) => fuse_cc::<TEST, 1, F_MI, E_DISP>(cc),
            (TEST, 4, E_BASE) => fuse_cc::<TEST, 4, F_MI, E_BASE>(cc),
            _ => None,
        };
    } else {
        return None;
    };
    match (op, sz, form) {
        (CMP, 4, F_RR) => fuse_cc::<CMP, 4, F_RR, E_DISP>(cc),
        (CMP, 2, F_RR) => fuse_cc::<CMP, 2, F_RR, E_DISP>(cc),
        (CMP, 1, F_RR) => fuse_cc::<CMP, 1, F_RR, E_DISP>(cc),
        (CMP, 4, F_RI) => fuse_cc::<CMP, 4, F_RI, E_DISP>(cc),
        (CMP, 2, F_RI) => fuse_cc::<CMP, 2, F_RI, E_DISP>(cc),
        (CMP, 1, F_RI) => fuse_cc::<CMP, 1, F_RI, E_DISP>(cc),
        (TEST, 4, F_RR) => fuse_cc::<TEST, 4, F_RR, E_DISP>(cc),
        (TEST, 2, F_RR) => fuse_cc::<TEST, 2, F_RR, E_DISP>(cc),
        (TEST, 1, F_RR) => fuse_cc::<TEST, 1, F_RR, E_DISP>(cc),
        (TEST, 4, F_RI) => fuse_cc::<TEST, 4, F_RI, E_DISP>(cc),
        (TEST, 1, F_RI) => fuse_cc::<TEST, 1, F_RI, E_DISP>(cc),
        _ => None,
    }
}

/// [`cmp_jcc`] instance for a condition (the 12 conditions `cond_fast` evaluates from the lazy state directly).
fn fuse_cc<const OP: u8, const SZ: u32, const F: u8, const E: u8>(cc: ConditionCode) -> Option<Handler> {
    use ConditionCode as C;
    macro_rules! h {
        ($c:ident) => {
            cmp_jcc::<OP, SZ, F, { C::$c as u8 }, E> as Handler
        };
    }
    Some(match cc {
        C::e => h!(e),
        C::ne => h!(ne),
        C::b => h!(b),
        C::ae => h!(ae),
        C::be => h!(be),
        C::a => h!(a),
        C::s => h!(s),
        C::ns => h!(ns),
        C::l => h!(l),
        C::ge => h!(ge),
        C::le => h!(le),
        C::g => h!(g),
        _ => return None,
    })
}

/// Whether `ins` gets the fast `jcc` handler (same test as in [`specialised`]).
pub(crate) fn is_fast_jcc(ins: &Instruction) -> bool {
    ins.flow_control() == FlowControl::ConditionalBranch
        && ins.condition_code() != ConditionCode::None
        && ins.op0_kind() == OpKind::NearBranch32
        && ins.is_jcc_short_or_near()
}

/// Generic path: the full interpreter (`exec::step`).
pub(crate) fn generic(cpu: &mut Cpu, mem: &mut GuestMemory, op: &Op) -> bool {
    // SAFETY: a generic op holds a pointer into the owning block's instruction array (see `Op::set_ins`).
    let ins = unsafe { &*op.ins() };
    cpu.cur_ip = ins.ip32();
    match exec::step(cpu, mem, ins) {
        Ok(()) => next(cpu, mem, op),
        Err(e) => generic_exit(cpu, op, e),
    }
}

#[cold]
#[inline(never)]
fn generic_exit(cpu: &mut Cpu, op: &Op, e: crate::CpuExit) -> bool {
    cpu.pending_exit = Some(e);
    cpu.fault_op = op as *const Op as usize;
    false
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
pub(crate) fn reg_op(ins: &Instruction, i: u32, sz: u32) -> Option<u8> {
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
pub(crate) fn mem_op(ins: &Instruction, i: u32, sz: u32) -> Option<(u8, u8, u8, u8, u32)> {
    if ins.op_kind(i) != OpKind::Memory {
        return None;
    }
    if sz != 0 && ins.memory_size().size() as u32 != sz {
        return None;
    }
    mem_form(ins)
}

/// Immediate operand `i` as a value of the operand size.
pub(crate) fn imm_op(ins: &Instruction, i: u32) -> Option<u32> {
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
            if let Some(r) = reg_op(ins, 0, 4) {
                o.r0 = r;
                o.imm = ins.immediate8() as u32;
                match m {
                    M::Shr => shift_ri::<1>,
                    M::Sar => shift_ri::<2>,
                    _ => shift_ri::<0>,
                }
            } else {
                let sz = size0(ins);
                o.r0 = reg_op(ins, 0, sz)?;
                o.imm = ins.immediate8() as u32;
                match (m, sz) {
                    (M::Shr, 1) => shift_ri_n::<1, 1>,
                    (M::Sar, 1) => shift_ri_n::<2, 1>,
                    (_, 1) => shift_ri_n::<0, 1>,
                    (M::Shr, 2) => shift_ri_n::<1, 2>,
                    (M::Sar, 2) => shift_ri_n::<2, 2>,
                    (_, 2) => shift_ri_n::<0, 2>,
                    _ => return None,
                }
            }
        }
        M::Shl | M::Sal | M::Shr | M::Sar
            if ins.op1_kind() == OpKind::Register && ins.op1_register() == Register::CL =>
        {
            let sz = size0(ins);
            o.r0 = reg_op(ins, 0, sz)?;
            match (m, sz) {
                (M::Shr, 1) => shift_rc::<1, 1>,
                (M::Sar, 1) => shift_rc::<2, 1>,
                (_, 1) => shift_rc::<0, 1>,
                (M::Shr, 2) => shift_rc::<1, 2>,
                (M::Sar, 2) => shift_rc::<2, 2>,
                (_, 2) => shift_rc::<0, 2>,
                (M::Shr, 4) => shift_rc::<1, 4>,
                (M::Sar, 4) => shift_rc::<2, 4>,
                (_, 4) => shift_rc::<0, 4>,
                _ => return None,
            }
        }
        M::Imul | M::Mul => match ins.code() {
            Code::Imul_r32_rm32 | Code::Imul_r32_rm32_imm8 | Code::Imul_r32_rm32_imm32 => {
                o.r0 = reg_op(ins, 0, 4)?;
                let three = ins.code() != Code::Imul_r32_rm32;
                if three {
                    o.imm = ins.immediate(2) as u32;
                }
                if let Some(r) = reg_op(ins, 1, 4) {
                    o.r1 = r;
                    if three {
                        imul_ri::<false, E_DISP>
                    } else {
                        imul_r::<false, E_DISP>
                    }
                } else {
                    let f = mem_op(ins, 1, 4)?;
                    let e = set_mem(&mut o, f);
                    if three {
                        by_e!(e, imul_ri<true>)
                    } else {
                        by_e!(e, imul_r<true>)
                    }
                }
            }
            Code::Imul_rm32 | Code::Mul_rm32 => {
                let signed = ins.code() == Code::Imul_rm32;
                if let Some(r) = reg_op(ins, 0, 4) {
                    o.r0 = r;
                    if signed {
                        mul1::<true, false, E_DISP>
                    } else {
                        mul1::<false, false, E_DISP>
                    }
                } else {
                    let f = mem_op(ins, 0, 4)?;
                    let e = set_mem(&mut o, f);
                    if signed {
                        by_e!(e, mul1<true, true>)
                    } else {
                        by_e!(e, mul1<false, true>)
                    }
                }
            }
            _ => return None,
        },
        M::Cdq => cdq,
        M::Cwde => cwde,
        _ if ins.op_count() == 1
            && matches!(
                m,
                M::Seto
                    | M::Setno
                    | M::Setb
                    | M::Setae
                    | M::Sete
                    | M::Setne
                    | M::Setbe
                    | M::Seta
                    | M::Sets
                    | M::Setns
                    | M::Setp
                    | M::Setnp
                    | M::Setl
                    | M::Setge
                    | M::Setle
                    | M::Setg
            ) =>
        {
            o.r0 = reg_op(ins, 0, 1)?;
            setcc_handler(ins.condition_code())?
        }
        M::Neg => {
            o.r0 = reg_op(ins, 0, 4)?;
            neg_r
        }
        M::Not => {
            o.r0 = reg_op(ins, 0, 4)?;
            not_r
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
        M::Inc | M::Dec => {
            let dec = m == M::Dec;
            if let Some(r) = reg_op(ins, 0, 4) {
                o.r0 = r;
                if dec {
                    dec_r
                } else {
                    inc_r
                }
            } else {
                let f = mem_op(ins, 0, 4)?;
                let e = set_mem(&mut o, f);
                if dec {
                    by_e!(e, incdec_m<true>)
                } else {
                    by_e!(e, incdec_m<false>)
                }
            }
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
