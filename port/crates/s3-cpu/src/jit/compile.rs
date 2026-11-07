//! Block compiler: one decoded [`Block`] -> AArch64 code (see `mod.rs` for the model).
//!
//! Every native translation mirrors the interpreter's fast handler for the same form (`fast.rs`), which by its
//! contract equals `exec::step`. Only instructions whose `Op` uses a fast handler are translated natively (so
//! `cur_ip`, which only the generic path maintains, stays identical); everything else calls the interpreter.
//!
//! Register use: x19..x26 guest EAX..EDI, x27 `Cpu`, x28 guest memory base, x13 remaining budget,
//! x9/x10/x11 lazy-flag operands la/lb/lr (x9 = status bits in the `Eager` state), x12 lcf,
//! x14/x15/x17 temporaries, x16 address temporary, x0..x8 more temporaries.

use iced_x86::{Code, ConditionCode, FlowControl, Instruction, Mnemonic, OpKind};

use super::a64::{Asm, Cond, Label, R, SP, ZR};
use super::*;
use crate::fast::{self, Op, E_BASE, E_BI, E_DISP};
use crate::flags::lk;
use crate::{fl, layout, Block};

const CPU: R = 27;
const MEM: R = 28;
const LEFT: R = 13;
const FA: R = 9;
const FB: R = 10;
const FR: R = 11;
const FC: R = 12;
const T0: R = 14;
const T1: R = 15;
const AD: R = 16;
const T3: R = 17;
/// Saved branch target / helper temporaries.
const W5: R = 5;

#[inline]
fn g(r: u8) -> R {
    19 + (r & 7) as R
}

/// Compile-time location of the status flags.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Fl {
    /// In the `Cpu` (lazy or materialised), as the interpreter left them.
    Cpu,
    /// Lazy kind + operand size; operands in FA/FB/FR (+ FC for inc/dec), not yet stored.
    Lazy(u8, u32),
    /// Materialised status bits (CF PF AF ZF SF OF) in FA (`lk = NONE` once stored).
    Eager,
}

/// A condition after its compare was emitted.
enum Cc {
    Always,
    Never,
    If(Cond),
}

/// Memory operand (EA form, base, index, shift, displacement), as `fast::mem_op` returns it.
type M = (u8, u8, u8, u8, u32);

struct C<'a> {
    a: Asm,
    jit: &'a mut Jit,
    blk: &'a Block,
    fl: Fl,
    /// Index of the instruction being compiled.
    i: u32,
    /// Side-exit label of the current instruction (created on first check).
    cur_side: Option<Label>,
    side: Vec<(Label, u32, Fl)>,
    fails: Vec<(Label, u32)>,
    lits: Vec<(Label, u64)>,
    miss: Option<Label>,
    ended: bool,
}

/// Compile `blk` for execution at address `base`. Returns the code and the offset (in instructions) of the
/// block body; for a native-fast-path block the code starts with a stub that calls the native function first
/// (what the run loop does at a block entry) and continues in the body when it declines.
pub(super) fn compile(jit: &mut Jit, blk: &Block, base: usize) -> (Vec<u32>, usize) {
    let mut c = C {
        a: Asm::new_at(base),
        jit,
        blk,
        fl: Fl::Cpu,
        i: 0,
        cur_side: None,
        side: Vec::new(),
        fails: Vec::new(),
        lits: Vec::new(),
        miss: None,
        ended: false,
    };
    let n = blk.n;
    let budget = c.a.label();
    if blk.native.is_some() {
        c.native_stub(budget);
    }
    let body = c.a.pos();
    // Budget: the whole block must fit, else the run loop runs it partially through the interpreter.
    c.a.cmp_xi(LEFT, n);
    c.a.b_cond(Cond::Lo, budget);
    for i in 0..n as usize {
        c.i = i as u32;
        c.cur_side = None;
        let ins = &blk.insns[i];
        let op = &blk.ops[i];
        let last = i + 1 == n as usize;
        if !c.native(ins, op) {
            c.fallback(ins, last);
        }
        if c.ended {
            break;
        }
    }
    if !c.ended {
        c.end_block();
        c.link(blk.end);
    }
    // Out-of-line stubs.
    c.a.bind(budget);
    c.a.mov_wi(0, blk.start);
    c.a.b_abs(c.jit.exit_eip);
    if let Some(l) = c.miss {
        c.a.bind(l);
        c.a.b_abs(c.jit.exit_eip);
    }
    let blk_lit = c.lit(blk as *const Block as u64);
    for (l, i, f) in std::mem::take(&mut c.side) {
        c.a.bind(l);
        c.flush(f);
        c.exit_at(blk_lit, i, EXIT_SIDE);
    }
    for (l, i) in std::mem::take(&mut c.fails) {
        c.a.bind(l);
        c.exit_at(blk_lit, i, EXIT_FAIL);
    }
    for (l, v) in std::mem::take(&mut c.lits) {
        c.a.literal64(l, v);
    }
    c.a.finish();
    (c.a.code, body)
}

impl C<'_> {
    // ------------------------------------------------------------------------------------- infrastructure

    fn lit(&mut self, v: u64) -> Label {
        if let Some(&(l, _)) = self.lits.iter().find(|e| e.1 == v) {
            return l;
        }
        let l = self.a.label();
        self.lits.push((l, v));
        l
    }

    /// Exit to the run loop at instruction `i` of this block (`code` = EXIT_SIDE / EXIT_FAIL).
    fn exit_at(&mut self, blk_lit: Label, i: u32, code: u32) {
        self.a.ldr_x_lit(T3, blk_lit);
        self.a.str_x(T3, CPU, OFF_BLK);
        self.a.mov_wi(T3, i);
        self.a.str_w(T3, CPU, OFF_IDX);
        self.a.mov_wi(0, code);
        self.a.b_abs(self.jit.exit_common);
    }

    /// Side-exit label of the current instruction (flags as at its start).
    fn side(&mut self) -> Label {
        if let Some(l) = self.cur_side {
            return l;
        }
        let l = self.a.label();
        self.side.push((l, self.i, self.fl));
        self.cur_side = Some(l);
        l
    }

    /// Fault check: guest address in `r` below the NULL guard -> side exit.
    fn check(&mut self, r: R) {
        let l = self.side();
        self.a.cmp_wi(r, layout::NULL_GUARD_END);
        self.a.b_cond(Cond::Lo, l);
    }

    /// `dst = src + v` (32-bit, any v).
    fn add_imm(&mut self, dst: R, src: R, v: u32) {
        if v == 0 {
            self.a.mov_w(dst, src);
        } else if v < 4096 {
            self.a.add_wi(dst, src, v);
        } else if v.wrapping_neg() < 4096 {
            self.a.sub_wi(dst, src, v.wrapping_neg());
        } else if v & 0xFFF == 0 && v < (1 << 24) {
            self.a.add_wi(dst, src, v);
        } else {
            self.a.mov_wi(T3, v);
            self.a.add_w(dst, src, T3);
        }
    }

    /// Effective address into `dst` (or the base register itself for `[reg]`). `None` = a constant address in
    /// the NULL guard: an unconditional side exit was emitted and the block ends here.
    fn ea(&mut self, m: M, dst: R) -> Option<R> {
        let (e, b, x, s, d) = m;
        match e {
            E_DISP => {
                if d < layout::NULL_GUARD_END {
                    let l = self.side();
                    self.a.b(l);
                    self.ended = true;
                    return None;
                }
                self.a.mov_wi(dst, d);
                return Some(dst);
            }
            E_BASE => {
                if d == 0 && dst == AD {
                    return Some(g(b));
                }
                self.add_imm(dst, g(b), d);
            }
            E_BI => {
                if d != 0 && !(d < 4096 || d.wrapping_neg() < 4096) {
                    self.a.mov_wi(T3, d);
                    self.a.add_w_lsl(dst, g(b), g(x), s as u32);
                    self.a.add_w(dst, dst, T3);
                } else {
                    self.a.add_w_lsl(dst, g(b), g(x), s as u32);
                    self.add_imm(dst, dst, d);
                }
            }
            _ => {
                self.a.mov_wi(T3, d);
                self.a.add_w_lsl(dst, T3, g(x), s as u32);
            }
        }
        Some(dst)
    }

    /// Effective address of a memory access, with its fault check (E_DISP constants need none).
    fn addr(&mut self, m: M) -> Option<R> {
        let r = self.ea(m, AD)?;
        if m.0 != E_DISP {
            self.check(r);
        }
        Some(r)
    }

    /// Zero-extended value of an 8/16/32-bit guest register (32-bit: the register itself, else into `tmp`).
    fn read(&mut self, sz: u32, r: u8, tmp: R) -> R {
        match sz {
            4 => g(r),
            2 => {
                self.a.uxth_w(tmp, g(r));
                tmp
            }
            _ => {
                if r < 4 {
                    self.a.uxtb_w(tmp, g(r));
                } else {
                    self.a.ubfx_w(tmp, g(r & 3), 8, 8);
                }
                tmp
            }
        }
    }

    /// Write the low `sz` bytes of `src` to a guest register (partial registers merge).
    fn write(&mut self, sz: u32, r: u8, src: R) {
        match sz {
            4 => self.a.mov_w(g(r), src),
            2 => self.a.bfi_w(g(r), src, 0, 16),
            _ => {
                if r < 4 {
                    self.a.bfi_w(g(r), src, 0, 8);
                } else {
                    self.a.bfi_w(g(r & 3), src, 8, 8);
                }
            }
        }
    }

    /// Call `f(cpu, x1)` (an `extern "C"` helper). Clobbers x0..x17 except x13 (saved).
    fn call(&mut self, f: usize, x1: Option<u64>) {
        self.a.str_x(LEFT, SP, SAVE_LEFT);
        self.a.mov_x(0, CPU);
        if let Some(v) = x1 {
            let l = self.lit(v);
            self.a.ldr_x_lit(1, l);
        }
        let l = self.lit(f as u64);
        self.a.ldr_x_lit(AD, l);
        self.a.blr(AD);
        self.a.ldr_x(LEFT, SP, SAVE_LEFT);
    }

    fn store_regs(&mut self) {
        for k in 0..4 {
            self.a.stp_w(19 + 2 * k, 20 + 2 * k, CPU, (OFF_REGS + 8 * k) as i32);
        }
    }

    fn load_regs(&mut self) {
        for k in 0..4 {
            self.a.ldp_w(19 + 2 * k, 20 + 2 * k, CPU, (OFF_REGS + 8 * k) as i32);
        }
    }

    // ------------------------------------------------------------------------------------------ flags

    /// Store the flag state `f` into the `Cpu` (interpreter format). Does not change NZCV.
    fn flush(&mut self, f: Fl) {
        match f {
            Fl::Cpu => {}
            Fl::Lazy(k, sz) => {
                self.a.mov_wi(T0, k as u32 | (sz << 8));
                self.a.strh(T0, CPU, OFF_LK);
                self.a.stp_w(FA, FB, CPU, OFF_LA as i32);
                self.a.str_w(FR, CPU, OFF_LR);
                if k == lk::INC || k == lk::DEC {
                    self.a.str_w(FC, CPU, OFF_LCF);
                }
            }
            Fl::Eager => {
                self.a.ldr_w(T0, CPU, OFF_EF);
                self.a.mov_wi(T1, fl::STATUS);
                self.a.bic_w(T0, T0, T1);
                self.a.orr_w(T0, T0, FA);
                self.a.str_w(T0, CPU, OFF_EF);
                self.a.strb(ZR, CPU, OFF_LK);
            }
        }
    }

    /// FC = CF of the current flag state (before an inc/dec). Clobbers T0, T1, T3.
    fn cf_into_fc(&mut self) {
        match self.fl {
            Fl::Lazy(lk::ADD, _) => {
                self.a.cmp_w(FR, FA);
                self.a.cset_w(FC, Cond::Lo);
            }
            Fl::Lazy(lk::SUB, _) => {
                self.a.cmp_w(FA, FB);
                self.a.cset_w(FC, Cond::Lo);
            }
            Fl::Lazy(lk::LOGIC, _) => self.a.movz_w(FC, 0, 0),
            Fl::Lazy(..) => {}
            Fl::Eager => {
                self.a.try_and_wi(FC, FA, 1);
            }
            Fl::Cpu => {
                let (l_ef, l_zero, l_lcf, done) = (self.a.label(), self.a.label(), self.a.label(), self.a.label());
                self.a.ldrb(T0, CPU, OFF_LK);
                self.a.cbz_w(T0, l_ef);
                self.a.cmp_wi(T0, lk::LOGIC as u32);
                self.a.b_cond(Cond::Hi, l_lcf);
                self.a.b_cond(Cond::Eq, l_zero);
                self.a.ldr_w(T1, CPU, OFF_LA);
                self.a.ldr_w(T3, CPU, OFF_LB);
                self.a.ldr_w(FC, CPU, OFF_LR);
                self.a.cmp_wi(T0, lk::ADD as u32);
                self.a.csel_w(T0, FC, T1, Cond::Eq); // ADD: lr < la, SUB: la < lb
                self.a.csel_w(T1, T1, T3, Cond::Eq);
                self.a.cmp_w(T0, T1);
                self.a.cset_w(FC, Cond::Lo);
                self.a.b(done);
                self.a.bind(l_ef);
                self.a.ldr_w(FC, CPU, OFF_EF);
                self.a.try_and_wi(FC, FC, fl::CF);
                self.a.b(done);
                self.a.bind(l_zero);
                self.a.movz_w(FC, 0, 0);
                self.a.b(done);
                self.a.bind(l_lcf);
                self.a.ldr_w(FC, CPU, OFF_LCF);
                self.a.bind(done);
            }
        }
    }

    /// `dst` = AF (0 or 0x10) of the current flag state. Clobbers T3 (dst must not be T3).
    fn af_into(&mut self, dst: R) {
        match self.fl {
            Fl::Lazy(lk::LOGIC, _) => self.a.movz_w(dst, 0, 0),
            Fl::Lazy(..) => {
                self.a.eor_w(dst, FA, FB);
                self.a.eor_w(dst, dst, FR);
                self.a.try_and_wi(dst, dst, fl::AF);
            }
            Fl::Eager => {
                self.a.try_and_wi(dst, FA, fl::AF);
            }
            Fl::Cpu => {
                let (l_none, l_zero, done) = (self.a.label(), self.a.label(), self.a.label());
                self.a.ldrb(T3, CPU, OFF_LK);
                self.a.cbz_w(T3, l_none);
                self.a.cmp_wi(T3, lk::LOGIC as u32);
                self.a.b_cond(Cond::Eq, l_zero);
                self.a.ldr_w(dst, CPU, OFF_LA);
                self.a.ldr_w(T3, CPU, OFF_LB);
                self.a.eor_w(dst, dst, T3);
                self.a.ldr_w(T3, CPU, OFF_LR);
                self.a.eor_w(dst, dst, T3);
                self.a.try_and_wi(dst, dst, fl::AF);
                self.a.b(done);
                self.a.bind(l_none);
                self.a.ldr_w(dst, CPU, OFF_EF);
                self.a.try_and_wi(dst, dst, fl::AF);
                self.a.b(done);
                self.a.bind(l_zero);
                self.a.movz_w(dst, 0, 0);
                self.a.bind(done);
            }
        }
    }

    /// `acc |= SF | ZF | PF` of the 32-bit result `r`. Clobbers T0.
    fn szp_or(&mut self, acc: R, r: R) {
        self.a.ubfx_w(T0, r, 31, 1);
        self.a.orr_w_lsl(acc, acc, T0, 7);
        self.a.cmp_wi(r, 0);
        self.a.cset_w(T0, Cond::Eq);
        self.a.orr_w_lsl(acc, acc, T0, 6);
        self.parity_odd(T0, r);
        self.a.mvn_w(T0, T0);
        self.a.try_and_wi(T0, T0, 1);
        self.a.orr_w_lsl(acc, acc, T0, 2);
    }

    /// Bit 0 of `dst` = 1 if the low byte of `r` has an odd number of ones.
    fn parity_odd(&mut self, dst: R, r: R) {
        self.a.eor_w_lsr(dst, r, r, 4);
        self.a.eor_w_lsr(dst, dst, dst, 2);
        self.a.eor_w_lsr(dst, dst, dst, 1);
    }

    /// Emit the compare for condition `cc` on the current flag state.
    fn cond(&mut self, cc: ConditionCode) -> Cc {
        use ConditionCode as K;
        match self.fl {
            Fl::Cpu => {
                self.call(jit_cond as *const () as usize, Some(cc as u8 as u64));
                self.a.cmp_wi(0, 0);
                Cc::If(Cond::Ne)
            }
            Fl::Eager => {
                let bit = |c: K| match c {
                    K::o | K::no => Some(fl::OF),
                    K::b | K::ae => Some(fl::CF),
                    K::e | K::ne => Some(fl::ZF),
                    K::s | K::ns => Some(fl::SF),
                    K::p | K::np => Some(fl::PF),
                    _ => None,
                };
                let set = matches!(cc, K::o | K::b | K::e | K::s | K::p | K::be | K::l | K::le);
                if let Some(b) = bit(cc) {
                    self.a.try_tst_wi(FA, b);
                } else {
                    match cc {
                        K::be | K::a => {
                            self.a.mov_wi(T1, fl::CF | fl::ZF);
                            self.a.tst_w(FA, T1);
                        }
                        K::l | K::ge => {
                            self.a.ubfx_w(T0, FA, 7, 1);
                            self.a.ubfx_w(T1, FA, 11, 1);
                            self.a.cmp_w(T0, T1);
                        }
                        _ => {
                            // le / g: ZF || SF != OF
                            self.a.ubfx_w(T0, FA, 7, 1);
                            self.a.ubfx_w(T1, FA, 11, 1);
                            self.a.eor_w(T0, T0, T1);
                            self.a.ubfx_w(T1, FA, 6, 1);
                            self.a.orr_w(T0, T0, T1);
                            self.a.cmp_wi(T0, 0);
                        }
                    }
                }
                Cc::If(if set { Cond::Ne } else { Cond::Eq })
            }
            Fl::Lazy(k, sz) => self.cond_lazy(k, sz, cc),
        }
    }

    fn cond_lazy(&mut self, k: u8, sz: u32, cc: ConditionCode) -> Cc {
        use ConditionCode as K;
        let sh = 32 - 8 * sz;
        let sb = 1u32 << (8 * sz - 1);
        match cc {
            K::e | K::ne => {
                self.a.cmp_wi(FR, 0);
                return Cc::If(if cc == K::e { Cond::Eq } else { Cond::Ne });
            }
            K::s | K::ns => {
                self.a.try_tst_wi(FR, sb);
                return Cc::If(if cc == K::s { Cond::Ne } else { Cond::Eq });
            }
            K::p | K::np => {
                self.parity_odd(T0, FR);
                self.a.try_tst_wi(T0, 1);
                return Cc::If(if cc == K::p { Cond::Eq } else { Cond::Ne });
            }
            _ => {}
        }
        // Signed / overflow conditions map 1:1 once N, Z, V describe the operation (shifted to bit 31).
        let signed = |c: K| match c {
            K::l => Some(Cond::Lt),
            K::ge => Some(Cond::Ge),
            K::le => Some(Cond::Le),
            K::g => Some(Cond::Gt),
            K::o => Some(Cond::Vs),
            K::no => Some(Cond::Vc),
            _ => None,
        };
        match k {
            lk::SUB => {
                if let Some(c) = signed(cc) {
                    if sh == 0 {
                        self.a.cmp_w(FA, FB);
                    } else {
                        self.a.lsl_wi(T0, FA, sh);
                        self.a.cmp_w_lsl(T0, FB, sh);
                    }
                    return Cc::If(c);
                }
                self.a.cmp_w(FA, FB);
                Cc::If(match cc {
                    K::b => Cond::Lo,
                    K::ae => Cond::Hs,
                    K::be => Cond::Ls,
                    _ => Cond::Hi,
                })
            }
            lk::ADD => {
                if sh == 0 {
                    self.a.cmn_w_lsl(FA, FB, 0);
                } else {
                    self.a.lsl_wi(T0, FA, sh);
                    self.a.cmn_w_lsl(T0, FB, sh);
                }
                if let Some(c) = signed(cc) {
                    return Cc::If(c);
                }
                match cc {
                    K::b => Cc::If(Cond::Hs),
                    K::ae => Cc::If(Cond::Lo),
                    _ => {
                        // be / a: CF || ZF
                        self.a.cset_w(T0, Cond::Hs);
                        self.a.csinc_w(T0, T0, ZR, Cond::Ne);
                        self.a.cmp_wi(T0, 0);
                        Cc::If(if cc == K::be { Cond::Ne } else { Cond::Eq })
                    }
                }
            }
            lk::INC | lk::DEC => {
                if let Some(c) = signed(cc) {
                    let a = if sh == 0 {
                        FA
                    } else {
                        self.a.lsl_wi(T0, FA, sh);
                        T0
                    };
                    self.a.mov_wi(T1, 1 << sh);
                    if k == lk::INC {
                        self.a.cmn_w_lsl(a, T1, 0);
                    } else {
                        self.a.cmp_w_lsl(a, T1, 0);
                    }
                    return Cc::If(c);
                }
                match cc {
                    K::b | K::ae => {
                        self.a.cmp_wi(FC, 0);
                        Cc::If(if cc == K::b { Cond::Ne } else { Cond::Eq })
                    }
                    _ => {
                        self.a.cmp_wi(FR, 0);
                        self.a.csinc_w(T0, FC, ZR, Cond::Ne);
                        self.a.cmp_wi(T0, 0);
                        Cc::If(if cc == K::be { Cond::Ne } else { Cond::Eq })
                    }
                }
            }
            _ => {
                // LOGIC: CF = OF = 0.
                match cc {
                    K::b | K::o => Cc::Never,
                    K::ae | K::no => Cc::Always,
                    K::be | K::a => {
                        self.a.cmp_wi(FR, 0);
                        Cc::If(if cc == K::be { Cond::Eq } else { Cond::Ne })
                    }
                    K::l | K::ge => {
                        self.a.try_tst_wi(FR, sb);
                        Cc::If(if cc == K::l { Cond::Ne } else { Cond::Eq })
                    }
                    _ => {
                        if sh == 0 {
                            self.a.cmp_wi(FR, 0);
                        } else {
                            self.a.lsl_wi(T0, FR, sh);
                            self.a.cmp_wi(T0, 0);
                        }
                        Cc::If(if cc == K::le { Cond::Le } else { Cond::Gt })
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------------------------------- block ends

    /// Flags to the `Cpu`, count the block.
    fn end_block(&mut self) {
        let f = self.fl;
        self.flush(f);
        self.fl = Fl::Cpu;
        self.a.sub_xi(LEFT, LEFT, self.blk.n);
        self.ended = true;
    }

    /// Native fast path at a block entry (as in the run loop): exit if the budget is used up, else call it;
    /// declined -> fall into the block body, done -> continue at the EIP it left.
    fn native_stub(&mut self, budget: Label) {
        let body = self.a.label();
        self.a.cbz_x(LEFT, budget);
        // The run loop has EIP = block start when it calls a native function.
        self.a.mov_wi(T0, self.blk.start);
        self.a.str_w(T0, CPU, OFF_EIP);
        self.store_regs();
        self.a.str_x(LEFT, SP, SAVE_LEFT);
        self.a.mov_x(0, CPU);
        self.a.mov_x(1, LEFT);
        let lb = self.lit(self.blk as *const Block as u64);
        self.a.ldr_x_lit(2, lb);
        let lf = self.lit(jit_native as *const () as usize as u64);
        self.a.ldr_x_lit(AD, lf);
        self.a.blr(AD);
        self.a.ldr_x(LEFT, SP, SAVE_LEFT);
        self.load_regs();
        self.a.cbz_w(0, body);
        self.a.ldr_x(LEFT, CPU, OFF_LEFT);
        self.a.ldr_w(0, CPU, OFF_EIP);
        self.indirect();
        self.a.bind(body);
    }

    /// Continue at the static guest address `eip`: a direct branch if its block is compiled already, else
    /// through its slot.
    fn link(&mut self, eip: u32) {
        if eip < layout::THUNK_BASE {
            let s = self.jit.slot(eip);
            // SAFETY: slots point into the JIT's chunks.
            let v = unsafe { *s } as usize;
            if v != self.jit.exit_eip {
                self.a.b_abs(v);
                return;
            }
        }
        self.a.mov_wi(0, eip);
        if eip >= layout::THUNK_BASE {
            self.a.b_abs(self.jit.exit_eip);
            return;
        }
        let s = self.jit.slot(eip) as u64;
        let l = self.lit(s);
        self.a.ldr_x_lit(AD, l);
        self.a.ldr_x(AD, AD, 0);
        self.a.br(AD);
    }

    /// Continue at the guest address in w0 (indirect-branch table, else exit).
    fn indirect(&mut self) {
        let t = self.jit.jtab.as_ptr() as u64;
        let l = self.lit(t);
        let miss = match self.miss {
            Some(m) => m,
            None => {
                let m = self.a.label();
                self.miss = Some(m);
                m
            }
        };
        self.a.ldr_x_lit(AD, l);
        self.a.eor_w_lsr(T3, 0, 0, JTAB_BITS);
        self.a.try_and_wi(T3, T3, (JTAB - 1) as u32);
        self.a.add_x_lsl(AD, AD, T3, 4);
        self.a.ldr_w(T3, AD, 0);
        self.a.cmp_w(T3, 0);
        self.a.b_cond(Cond::Ne, miss);
        self.a.ldr_x(AD, AD, 8);
        self.a.br(AD);
    }

    /// Push the 32-bit value in `v` (fault check first; ESP changes only after the store).
    fn push(&mut self, v: R) {
        self.a.sub_wi(T1, g(4), 4);
        self.check(T1);
        self.a.st_guest(v, MEM, T1, 4);
        self.a.mov_w(g(4), T1);
    }

    // ------------------------------------------------------------------------------------- fallback

    /// Run instruction `ins` through the interpreter (one handler call).
    fn fallback(&mut self, ins: &Instruction, last: bool) {
        let f = self.fl;
        self.flush(f);
        self.fl = Fl::Cpu;
        self.store_regs();
        self.a.mov_wi(T0, self.blk.end);
        self.a.str_w(T0, CPU, OFF_EIP);
        let op = self.jit.fallback_op(ins) as u64;
        self.call(jit_op as *const () as usize, Some(op));
        self.load_regs();
        let l = self.a.label();
        self.fails.push((l, self.i));
        self.a.cbz_w(0, l);
        if last && ins.flow_control() != FlowControl::Next {
            self.end_block();
            self.a.ldr_w(0, CPU, OFF_EIP);
            self.indirect();
        }
    }

    // ------------------------------------------------------------------------------------- natives

    /// Native translation of `ins` (mirrors its fast handler). `false` = not translated (nothing emitted).
    fn native(&mut self, ins: &Instruction, op: &Op) -> bool {
        use Mnemonic as Mn;
        if op.h as usize == fast::generic as fast::Handler as usize {
            return false;
        }
        let m = ins.mnemonic();
        match m {
            Mn::Mov => self.mov(ins),
            Mn::Movzx | Mn::Movsx => self.movx(ins, m == Mn::Movsx),
            Mn::Lea => {
                let (Some(d), Some(mo)) = (fast::reg_op(ins, 0, 4), fast::mem_op(ins, 1, 0)) else {
                    return false;
                };
                let r = self.ea_lea(mo, g(d));
                self.a.mov_w(g(d), r);
                true
            }
            Mn::Push => match ins.code() {
                Code::Push_r32 | Code::Push_rm32 if fast::reg_op(ins, 0, 4).is_some() => {
                    let r = fast::reg_op(ins, 0, 4).unwrap();
                    self.push(g(r));
                    true
                }
                Code::Pushd_imm8 | Code::Pushd_imm32 => {
                    let Some(v) = fast::imm_op(ins, 0) else {
                        return false;
                    };
                    let src = if v == 0 {
                        ZR
                    } else {
                        self.a.mov_wi(T0, v);
                        T0
                    };
                    self.push(src);
                    true
                }
                Code::Push_rm32 => {
                    let Some(mo) = fast::mem_op(ins, 0, 4) else {
                        return false;
                    };
                    let Some(a) = self.addr(mo) else { return true };
                    self.a.sub_wi(T1, g(4), 4);
                    self.check(T1);
                    self.a.ld_guest(T0, MEM, a, 4, false);
                    self.a.st_guest(T0, MEM, T1, 4);
                    self.a.mov_w(g(4), T1);
                    true
                }
                _ => false,
            },
            Mn::Pop => match ins.code() {
                Code::Pop_r32 | Code::Pop_rm32 => {
                    let Some(r) = fast::reg_op(ins, 0, 4) else {
                        return false;
                    };
                    self.check(g(4));
                    self.a.ld_guest(T0, MEM, g(4), 4, false);
                    self.a.add_wi(g(4), g(4), 4);
                    self.a.mov_w(g(r), T0);
                    true
                }
                _ => false,
            },
            Mn::Add => self.alu(ins, lk::ADD, 0),
            Mn::Sub => self.alu(ins, lk::SUB, 0),
            Mn::Cmp => self.alu(ins, lk::SUB, 1),
            Mn::And => self.alu(ins, lk::LOGIC, 0),
            Mn::Test => self.alu(ins, lk::LOGIC, 1),
            Mn::Or => self.alu(ins, lk::LOGIC, 2),
            Mn::Xor => self.alu(ins, lk::LOGIC, 3),
            Mn::Inc | Mn::Dec => self.incdec(ins, m == Mn::Dec),
            Mn::Neg => {
                let Some(r) = fast::reg_op(ins, 0, 4) else {
                    return false;
                };
                self.a.movz_w(FA, 0, 0);
                self.a.mov_w(FB, g(r));
                self.a.neg_w(FR, g(r));
                self.a.mov_w(g(r), FR);
                self.fl = Fl::Lazy(lk::SUB, 4);
                true
            }
            Mn::Not => {
                let Some(r) = fast::reg_op(ins, 0, 4) else {
                    return false;
                };
                self.a.mvn_w(g(r), g(r));
                true
            }
            Mn::Cdq => {
                self.a.asr_wi(g(2), g(0), 31);
                true
            }
            Mn::Cwde => {
                self.a.sxth_w(g(0), g(0));
                true
            }
            Mn::Nop => true,
            Mn::Shl | Mn::Sal | Mn::Shr | Mn::Sar if ins.op1_kind() == OpKind::Immediate8 => {
                let Some(r) = fast::reg_op(ins, 0, 4) else {
                    return false;
                };
                let kind = match m {
                    Mn::Shr => 1,
                    Mn::Sar => 2,
                    _ => 0,
                };
                self.shift_ri(r, kind, ins.immediate8() as u32 & 31);
                true
            }
            Mn::Imul | Mn::Mul => self.mul(ins),
            Mn::Call => match ins.code() {
                Code::Call_rel32_32 => {
                    self.a.mov_wi(T0, ins.next_ip32());
                    self.push(T0);
                    self.end_block();
                    self.link(ins.near_branch32());
                    true
                }
                Code::Call_rm32 => {
                    if let Some(r) = fast::reg_op(ins, 0, 4) {
                        self.a.mov_w(W5, g(r));
                    } else {
                        let Some(mo) = fast::mem_op(ins, 0, 4) else {
                            return false;
                        };
                        let Some(a) = self.addr(mo) else { return true };
                        self.a.ld_guest(W5, MEM, a, 4, false);
                    }
                    self.a.mov_wi(T0, ins.next_ip32());
                    self.push(T0);
                    self.end_block();
                    self.a.mov_w(0, W5);
                    self.indirect();
                    true
                }
                _ => false,
            },
            Mn::Ret => {
                let imm = match ins.code() {
                    Code::Retnd => 0,
                    Code::Retnd_imm16 => ins.immediate16() as u32,
                    _ => return false,
                };
                self.check(g(4));
                self.a.ld_guest(W5, MEM, g(4), 4, false);
                self.add_imm(g(4), g(4), 4 + imm);
                self.end_block();
                self.a.mov_w(0, W5);
                self.indirect();
                true
            }
            Mn::Jmp => match ins.code() {
                Code::Jmp_rel8_32 | Code::Jmp_rel32_32 => {
                    self.end_block();
                    self.link(ins.near_branch32());
                    true
                }
                Code::Jmp_rm32 => {
                    if let Some(r) = fast::reg_op(ins, 0, 4) {
                        self.a.mov_w(W5, g(r));
                    } else {
                        let Some(mo) = fast::mem_op(ins, 0, 4) else {
                            return false;
                        };
                        let Some(a) = self.addr(mo) else { return true };
                        self.a.ld_guest(W5, MEM, a, 4, false);
                    }
                    self.end_block();
                    self.a.mov_w(0, W5);
                    self.indirect();
                    true
                }
                _ => false,
            },
            _ if fast::is_fast_jcc(ins) => {
                let c = self.cond(ins.condition_code());
                self.end_block();
                let (taken, next) = (ins.near_branch32(), ins.next_ip32());
                match c {
                    Cc::Always => self.link(taken),
                    Cc::Never => self.link(next),
                    Cc::If(c) => {
                        let l = self.a.label();
                        self.a.b_cond(c, l);
                        self.link(next);
                        self.a.bind(l);
                        self.link(taken);
                    }
                }
                true
            }
            _ if ins.op_count() == 1 && is_setcc(m) => {
                let Some(r) = fast::reg_op(ins, 0, 1) else {
                    return false;
                };
                match self.cond(ins.condition_code()) {
                    Cc::Always => self.a.movz_w(T0, 1, 0),
                    Cc::Never => self.a.movz_w(T0, 0, 0),
                    Cc::If(c) => self.a.cset_w(T0, c),
                }
                self.write(1, r, T0);
                true
            }
            _ => false,
        }
    }

    /// LEA: effective address into `dst` (no memory access, no check).
    fn ea_lea(&mut self, m: M, dst: R) -> R {
        let (e, ..) = m;
        if e == E_DISP {
            self.a.mov_wi(dst, m.4);
            return dst;
        }
        self.ea(m, dst).unwrap_or(dst)
    }

    fn mov(&mut self, ins: &Instruction) -> bool {
        let sz = size0(ins);
        if ins.op_count() != 2 || !matches!(sz, 1 | 2 | 4) {
            return false;
        }
        if let (Some(d), Some(s)) = (fast::reg_op(ins, 0, sz), fast::reg_op(ins, 1, sz)) {
            let v = self.read(sz, s, T0);
            self.write(sz, d, v);
        } else if let (Some(d), Some(mo)) = (fast::reg_op(ins, 0, sz), fast::mem_op(ins, 1, sz)) {
            let Some(a) = self.addr(mo) else { return true };
            if sz == 4 {
                self.a.ld_guest(g(d), MEM, a, 4, false);
            } else {
                self.a.ld_guest(T0, MEM, a, sz, false);
                self.write(sz, d, T0);
            }
        } else if let (Some(mo), Some(s)) = (fast::mem_op(ins, 0, sz), fast::reg_op(ins, 1, sz)) {
            let Some(a) = self.addr(mo) else { return true };
            let v = self.read(sz, s, T0);
            self.a.st_guest(v, MEM, a, sz);
        } else if let (Some(d), Some(v)) = (fast::reg_op(ins, 0, sz), fast::imm_op(ins, 1)) {
            if sz == 4 {
                self.a.mov_wi(g(d), v);
            } else {
                self.a.mov_wi(T0, v);
                self.write(sz, d, T0);
            }
        } else if let (Some(mo), Some(v)) = (fast::mem_op(ins, 0, sz), fast::imm_op(ins, 1)) {
            let Some(a) = self.addr(mo) else { return true };
            let v = v & crate::flags::mask(sz);
            let src = if v == 0 {
                ZR
            } else {
                self.a.mov_wi(T0, v);
                T0
            };
            self.a.st_guest(src, MEM, a, sz);
        } else {
            return false;
        }
        true
    }

    fn movx(&mut self, ins: &Instruction, signed: bool) -> bool {
        let Some(d) = fast::reg_op(ins, 0, 4) else {
            return false;
        };
        if let Some(mo) = fast::mem_op(ins, 1, 0) {
            let sz = ins.memory_size().size() as u32;
            if !matches!(sz, 1 | 2) {
                return false;
            }
            let Some(a) = self.addr(mo) else { return true };
            self.a.ld_guest(g(d), MEM, a, sz, signed);
            return true;
        }
        for sz in [1u32, 2] {
            if let Some(s) = fast::reg_op(ins, 1, sz) {
                match (sz, signed, s < 4) {
                    (2, false, _) => self.a.uxth_w(g(d), g(s)),
                    (2, true, _) => self.a.sxth_w(g(d), g(s)),
                    (_, false, true) => self.a.uxtb_w(g(d), g(s)),
                    (_, true, true) => self.a.sxtb_w(g(d), g(s)),
                    (_, false, false) => self.a.ubfx_w(g(d), g(s & 3), 8, 8),
                    (_, true, false) => self.a.sbfx_w(g(d), g(s & 3), 8, 8),
                }
                return true;
            }
        }
        false
    }

    /// ALU op: `kind` = lazy kind (ADD / SUB / LOGIC); `sub`: for SUB 0 = sub, 1 = cmp; for LOGIC 0 = and,
    /// 1 = test, 2 = or, 3 = xor.
    fn alu(&mut self, ins: &Instruction, kind: u8, sub: u8) -> bool {
        let sz = size0(ins);
        if ins.op_count() != 2 || !matches!(sz, 1 | 2 | 4) {
            return false;
        }
        let msk = crate::flags::mask(sz);
        let writes = !((kind == lk::SUB && sub == 1) || (kind == lk::LOGIC && sub == 1));
        // Sources into FA (destination value) and FB (source value), both masked; memory forms check first.
        enum Dst {
            Reg(u8),
            Mem(R),
        }
        let dst;
        if let (Some(d), Some(s)) = (fast::reg_op(ins, 0, sz), fast::reg_op(ins, 1, sz)) {
            // Read both before writing FA/FB (FA may alias nothing guest-visible, but keep the order simple).
            let a = self.read(sz, d, T0);
            let b = self.read(sz, s, T1);
            self.a.mov_w(FA, a);
            self.a.mov_w(FB, b);
            dst = Dst::Reg(d);
        } else if let (Some(d), Some(v)) = (fast::reg_op(ins, 0, sz), fast::imm_op(ins, 1)) {
            let a = self.read(sz, d, T0);
            self.a.mov_w(FA, a);
            self.a.mov_wi(FB, v & msk);
            dst = Dst::Reg(d);
        } else if let (Some(d), Some(mo)) = (fast::reg_op(ins, 0, sz), fast::mem_op(ins, 1, sz)) {
            let Some(ad) = self.addr(mo) else { return true };
            self.a.ld_guest(FB, MEM, ad, sz, false);
            let a = self.read(sz, d, T0);
            self.a.mov_w(FA, a);
            dst = Dst::Reg(d);
        } else if let (Some(mo), Some(s)) = (fast::mem_op(ins, 0, sz), fast::reg_op(ins, 1, sz)) {
            let Some(ad) = self.addr(mo) else { return true };
            let b = self.read(sz, s, T1);
            self.a.ld_guest(FA, MEM, ad, sz, false);
            self.a.mov_w(FB, b);
            dst = Dst::Mem(ad);
        } else if let (Some(mo), Some(v)) = (fast::mem_op(ins, 0, sz), fast::imm_op(ins, 1)) {
            let Some(ad) = self.addr(mo) else { return true };
            self.a.ld_guest(FA, MEM, ad, sz, false);
            self.a.mov_wi(FB, v & msk);
            dst = Dst::Mem(ad);
        } else {
            return false;
        }
        match kind {
            lk::ADD => self.a.add_w(FR, FA, FB),
            lk::SUB => self.a.sub_w(FR, FA, FB),
            _ => match sub {
                0 | 1 => self.a.and_w(FR, FA, FB),
                2 => self.a.orr_w(FR, FA, FB),
                _ => self.a.eor_w(FR, FA, FB),
            },
        }
        if sz != 4 && kind != lk::LOGIC {
            self.a.try_and_wi(FR, FR, msk);
        }
        self.fl = Fl::Lazy(kind, sz);
        if writes {
            match dst {
                Dst::Reg(d) => self.write(sz, d, FR),
                Dst::Mem(ad) => self.a.st_guest(FR, MEM, ad, sz),
            }
        }
        true
    }

    fn incdec(&mut self, ins: &Instruction, dec: bool) -> bool {
        let k = if dec { lk::DEC } else { lk::INC };
        if let Some(r) = fast::reg_op(ins, 0, 4) {
            self.cf_into_fc();
            self.a.mov_w(FA, g(r));
            self.a.movz_w(FB, 1, 0);
            if dec {
                self.a.sub_wi(FR, FA, 1);
            } else {
                self.a.add_wi(FR, FA, 1);
            }
            self.a.mov_w(g(r), FR);
        } else {
            let Some(mo) = fast::mem_op(ins, 0, 4) else {
                return false;
            };
            let Some(ad) = self.addr(mo) else { return true };
            // `cf_into_fc` clobbers T0/T1/T3 only; the address is in AD or a guest register.
            self.cf_into_fc();
            self.a.ld_guest(FA, MEM, ad, 4, false);
            self.a.movz_w(FB, 1, 0);
            if dec {
                self.a.sub_wi(FR, FA, 1);
            } else {
                self.a.add_wi(FR, FA, 1);
            }
            self.a.st_guest(FR, MEM, ad, 4);
        }
        self.fl = Fl::Lazy(k, 4);
        true
    }

    /// shl/shr/sar r32, imm (KIND 0/1/2), count already masked; flags as `fast::shift_ri`.
    fn shift_ri(&mut self, r: u8, kind: u8, c: u32) {
        if c == 0 {
            return;
        }
        let x = g(r);
        self.af_into(T1);
        // CF = last bit shifted out.
        let cf_bit = if kind == 0 { 32 - c } else { c - 1 };
        self.a.ubfx_w(1, x, cf_bit, 1);
        if kind == 1 {
            self.a.ubfx_w(2, x, 31, 1); // OF (shr) = old sign
        }
        match kind {
            0 => self.a.lsl_wi(x, x, c),
            1 => self.a.lsr_wi(x, x, c),
            _ => self.a.asr_wi(x, x, c),
        }
        if kind == 0 {
            self.a.ubfx_w(2, x, 31, 1);
            self.a.eor_w(2, 2, 1); // OF (shl) = new sign ^ CF
        }
        self.a.orr_w(FA, T1, 1);
        if kind != 2 {
            self.a.orr_w_lsl(FA, FA, 2, 11);
        }
        self.szp_or(FA, x);
        self.fl = Fl::Eager;
    }

    /// imul (2/3 operand, 1 operand) and mul r/m32; flags as `fast::mul_flags` (AF kept).
    fn mul(&mut self, ins: &Instruction) -> bool {
        // Operand fetch first (fault checks before anything changes).
        let src = |c: &mut Self, i: u32| -> Option<Option<R>> {
            if let Some(r) = fast::reg_op(ins, i, 4) {
                return Some(Some(g(r)));
            }
            let mo = fast::mem_op(ins, i, 4)?;
            match c.addr(mo) {
                Some(a) => {
                    c.a.ld_guest(3, MEM, a, 4, false);
                    Some(Some(3))
                }
                None => Some(None),
            }
        };
        match ins.code() {
            Code::Imul_r32_rm32 | Code::Imul_r32_rm32_imm8 | Code::Imul_r32_rm32_imm32 => {
                let Some(d) = fast::reg_op(ins, 0, 4) else {
                    return false;
                };
                let three = ins.code() != Code::Imul_r32_rm32;
                let Some(s) = src(self, 1) else { return false };
                let Some(s) = s else { return true };
                self.af_into(T1);
                if three {
                    self.a.mov_wi(4, ins.immediate(2) as u32);
                    self.a.smull(0, s, 4);
                } else {
                    self.a.smull(0, g(d), s);
                }
                self.a.cmp_x_sxtw(0, 0);
                self.a.cset_w(1, Cond::Ne);
                self.a.mov_w(g(d), 0);
            }
            Code::Imul_rm32 | Code::Mul_rm32 => {
                let signed = ins.code() == Code::Imul_rm32;
                let Some(s) = src(self, 0) else { return false };
                let Some(s) = s else { return true };
                self.af_into(T1);
                if signed {
                    self.a.smull(0, g(0), s);
                    self.a.cmp_x_sxtw(0, 0);
                } else {
                    self.a.umull(0, g(0), s);
                    self.a.lsr_xi(2, 0, 32);
                    self.a.cmp_wi(2, 0);
                }
                self.a.cset_w(1, Cond::Ne);
                self.a.lsr_xi(2, 0, 32);
                self.a.mov_w(g(0), 0);
                self.a.mov_w(g(2), 2);
            }
            _ => return false,
        }
        // FA = AF | szp(lo) | (of ? CF|OF : 0); lo is in w0 (also the guest destination).
        self.a.orr_w(FA, T1, 1);
        self.a.orr_w_lsl(FA, FA, 1, 11);
        self.szp_or(FA, 0);
        self.fl = Fl::Eager;
        true
    }
}

fn is_setcc(m: Mnemonic) -> bool {
    use Mnemonic as M;
    matches!(
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
    )
}

/// Operand size in bytes from operand 0 (as `fast::size0`).
fn size0(ins: &Instruction) -> u32 {
    match ins.op0_kind() {
        OpKind::Register => ins.op0_register().size() as u32,
        OpKind::Memory => ins.memory_size().size() as u32,
        _ => 0,
    }
}
