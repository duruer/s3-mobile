//! Minimal AArch64 encoder for the JIT (hand-encoded; only what `compile.rs` needs).
//!
//! Registers are numbers 0..=31; 31 means `wzr/xzr` or `sp` depending on the instruction (as in the ISA).
//! `w*` methods are 32-bit (sf = 0), `x*` methods 64-bit. Branches to labels are fixed up in [`Asm::finish`].

#![allow(dead_code)]

pub type R = u32;
pub const ZR: R = 31;
pub const SP: R = 31;

/// Condition codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Cond {
    Eq = 0,
    Ne = 1,
    Hs = 2,
    Lo = 3,
    Mi = 4,
    Pl = 5,
    Vs = 6,
    Vc = 7,
    Hi = 8,
    Ls = 9,
    Ge = 10,
    Lt = 11,
    Gt = 12,
    Le = 13,
    Al = 14,
}

impl Cond {
    pub fn invert(self) -> Cond {
        // SAFETY: flipping bit 0 of 0..=13 stays inside the enum.
        unsafe { std::mem::transmute(self as u32 ^ 1) }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Label(pub u32);

#[derive(Clone, Copy)]
enum Fix {
    /// imm26 (b / bl).
    B26,
    /// imm19 at bit 5 (b.cond, cbz, cbnz, ldr literal).
    B19,
    /// imm14 at bit 5 (tbz / tbnz).
    B14,
}

/// Code buffer with labels.
#[derive(Default)]
pub struct Asm {
    /// Address the code will run at (for branches to absolute addresses).
    pub base: usize,
    pub code: Vec<u32>,
    labels: Vec<Option<u32>>,
    fixups: Vec<(usize, Label, Fix)>,
}

/// N:immr:imms field (bits 22..10) of a 32-bit logical immediate, or `None` if `v` is not encodable.
pub fn logic_imm32(v: u32) -> Option<u32> {
    if v == 0 || v == u32::MAX {
        return None;
    }
    let mut size = 32u32;
    while size > 2 {
        let h = size / 2;
        let m = (1u64 << h) - 1;
        if (v as u64 & m) != ((v as u64 >> h) & m) {
            break;
        }
        size = h;
    }
    let m = if size == 32 { u32::MAX } else { (1u32 << size) - 1 };
    let elt = v & m;
    let k = elt.count_ones();
    let ones = if k == 32 { u32::MAX } else { (1u32 << k) - 1 };
    for r in 0..size {
        let rol = if r == 0 { elt } else { ((elt << r) | (elt >> (size - r))) & m };
        if rol == ones {
            let imms = ((!((size << 1) - 1)) & 0x3f) | (k - 1);
            return Some((r << 16) | (imms << 10));
        }
    }
    None
}

impl Asm {
    pub fn new() -> Self {
        Asm::default()
    }

    pub fn new_at(base: usize) -> Self {
        Asm { base, ..Asm::default() }
    }

    /// `b <addr>` (absolute target inside +-128 MiB).
    pub fn b_abs(&mut self, addr: usize) {
        let pc = self.base + self.code.len() * 4;
        let d = (addr as i64 - pc as i64) / 4;
        assert!((-(1 << 25)..(1 << 25)).contains(&d), "b_abs out of range");
        self.emit(0x1400_0000 | ((d as u32) & 0x03FF_FFFF));
    }

    #[inline]
    pub fn emit(&mut self, w: u32) {
        self.code.push(w);
    }

    /// Current position in instructions.
    pub fn pos(&self) -> usize {
        self.code.len()
    }

    pub fn label(&mut self) -> Label {
        self.labels.push(None);
        Label(self.labels.len() as u32 - 1)
    }

    pub fn bind(&mut self, l: Label) {
        debug_assert!(self.labels[l.0 as usize].is_none());
        self.labels[l.0 as usize] = Some(self.code.len() as u32);
    }

    pub fn is_bound(&self, l: Label) -> bool {
        self.labels[l.0 as usize].is_some()
    }

    fn fix(&mut self, l: Label, f: Fix) {
        self.fixups.push((self.code.len(), l, f));
    }

    /// Resolve label fixups. Panics if a label is unbound or out of range (a compiler bug).
    pub fn finish(&mut self) {
        for &(at, l, f) in &self.fixups {
            let target = self.labels[l.0 as usize].expect("unbound JIT label");
            let d = target as i64 - at as i64;
            let w = &mut self.code[at];
            match f {
                Fix::B26 => {
                    assert!((-(1 << 25)..(1 << 25)).contains(&d));
                    *w |= (d as u32) & 0x03FF_FFFF;
                }
                Fix::B19 => {
                    assert!((-(1 << 18)..(1 << 18)).contains(&d));
                    *w |= ((d as u32) & 0x7FFFF) << 5;
                }
                Fix::B14 => {
                    assert!((-(1 << 13)..(1 << 13)).contains(&d));
                    *w |= ((d as u32) & 0x3FFF) << 5;
                }
            }
        }
        self.fixups.clear();
    }

    // ------------------------------------------------------------------------------------------ arithmetic

    fn addsub_imm(&mut self, base: u32, d: R, n: R, imm: u32) {
        if imm < 4096 {
            self.emit(base | (imm << 10) | (n << 5) | d);
        } else {
            assert!(imm & 0xFFF == 0 && imm < (1 << 24), "addsub imm {imm:#x}");
            self.emit(base | (1 << 22) | ((imm >> 12) << 10) | (n << 5) | d);
        }
    }

    /// `add wd, wn, #imm` (imm < 4096 or a multiple of 4096 below 2^24). `n`/`d` 31 = wsp.
    pub fn add_wi(&mut self, d: R, n: R, imm: u32) {
        self.addsub_imm(0x1100_0000, d, n, imm);
    }
    pub fn sub_wi(&mut self, d: R, n: R, imm: u32) {
        self.addsub_imm(0x5100_0000, d, n, imm);
    }
    pub fn subs_wi(&mut self, d: R, n: R, imm: u32) {
        self.addsub_imm(0x7100_0000, d, n, imm);
    }
    pub fn adds_wi(&mut self, d: R, n: R, imm: u32) {
        self.addsub_imm(0x3100_0000, d, n, imm);
    }
    pub fn cmp_wi(&mut self, n: R, imm: u32) {
        self.subs_wi(ZR, n, imm);
    }
    pub fn add_xi(&mut self, d: R, n: R, imm: u32) {
        self.addsub_imm(0x9100_0000, d, n, imm);
    }
    pub fn sub_xi(&mut self, d: R, n: R, imm: u32) {
        self.addsub_imm(0xD100_0000, d, n, imm);
    }
    pub fn cmp_xi(&mut self, n: R, imm: u32) {
        self.addsub_imm(0xF100_0000, ZR, n, imm);
    }

    /// Shifted-register form: shift 0 = lsl, 1 = lsr, 2 = asr.
    fn addsub_reg(&mut self, base: u32, d: R, n: R, m: R, shift: u32, amount: u32) {
        self.emit(base | (shift << 22) | (m << 16) | (amount << 10) | (n << 5) | d);
    }
    pub fn add_w(&mut self, d: R, n: R, m: R) {
        self.addsub_reg(0x0B00_0000, d, n, m, 0, 0);
    }
    pub fn sub_w(&mut self, d: R, n: R, m: R) {
        self.addsub_reg(0x4B00_0000, d, n, m, 0, 0);
    }
    pub fn subs_w(&mut self, d: R, n: R, m: R) {
        self.addsub_reg(0x6B00_0000, d, n, m, 0, 0);
    }
    pub fn adds_w(&mut self, d: R, n: R, m: R) {
        self.addsub_reg(0x2B00_0000, d, n, m, 0, 0);
    }
    /// `add wd, wn, wm, lsl #s`.
    pub fn add_w_lsl(&mut self, d: R, n: R, m: R, s: u32) {
        self.addsub_reg(0x0B00_0000, d, n, m, 0, s);
    }
    pub fn cmp_w(&mut self, n: R, m: R) {
        self.subs_w(ZR, n, m);
    }
    /// `cmp wn, wm, lsl #s`.
    pub fn cmp_w_lsl(&mut self, n: R, m: R, s: u32) {
        self.addsub_reg(0x6B00_0000, ZR, n, m, 0, s);
    }
    /// `cmn wn, wm, lsl #s`.
    pub fn cmn_w_lsl(&mut self, n: R, m: R, s: u32) {
        self.addsub_reg(0x2B00_0000, ZR, n, m, 0, s);
    }
    pub fn neg_w(&mut self, d: R, m: R) {
        self.sub_w(d, ZR, m);
    }
    /// `add xd, xn, xm, lsl #s`.
    pub fn add_x_lsl(&mut self, d: R, n: R, m: R, s: u32) {
        self.addsub_reg(0x8B00_0000, d, n, m, 0, s);
    }
    pub fn sub_x(&mut self, d: R, n: R, m: R) {
        self.addsub_reg(0xCB00_0000, d, n, m, 0, 0);
    }
    /// `cmp xn, wm, sxtw` (extended register).
    pub fn cmp_x_sxtw(&mut self, n: R, m: R) {
        self.emit(0xEB20_0000 | (m << 16) | (6 << 13) | (n << 5) | ZR);
    }

    fn logic_reg(&mut self, base: u32, d: R, n: R, m: R, shift: u32, amount: u32) {
        self.emit(base | (shift << 22) | (m << 16) | (amount << 10) | (n << 5) | d);
    }
    pub fn and_w(&mut self, d: R, n: R, m: R) {
        self.logic_reg(0x0A00_0000, d, n, m, 0, 0);
    }
    pub fn bic_w(&mut self, d: R, n: R, m: R) {
        self.logic_reg(0x0A20_0000, d, n, m, 0, 0);
    }
    pub fn orr_w(&mut self, d: R, n: R, m: R) {
        self.logic_reg(0x2A00_0000, d, n, m, 0, 0);
    }
    /// `orr wd, wn, wm, lsl #s`.
    pub fn orr_w_lsl(&mut self, d: R, n: R, m: R, s: u32) {
        self.logic_reg(0x2A00_0000, d, n, m, 0, s);
    }
    pub fn eor_w(&mut self, d: R, n: R, m: R) {
        self.logic_reg(0x4A00_0000, d, n, m, 0, 0);
    }
    /// `eor wd, wn, wm, lsr #s`.
    pub fn eor_w_lsr(&mut self, d: R, n: R, m: R, s: u32) {
        self.logic_reg(0x4A00_0000, d, n, m, 1, s);
    }
    pub fn ands_w(&mut self, d: R, n: R, m: R) {
        self.logic_reg(0x6A00_0000, d, n, m, 0, 0);
    }
    pub fn tst_w(&mut self, n: R, m: R) {
        self.ands_w(ZR, n, m);
    }
    pub fn mvn_w(&mut self, d: R, m: R) {
        self.logic_reg(0x2A20_0000, d, ZR, m, 0, 0);
    }
    pub fn mov_w(&mut self, d: R, m: R) {
        if d != m {
            self.orr_w(d, ZR, m);
        }
    }
    pub fn mov_x(&mut self, d: R, m: R) {
        if d != m {
            self.logic_reg(0xAA00_0000, d, ZR, m, 0, 0);
        }
    }

    fn logic_imm(&mut self, base: u32, d: R, n: R, v: u32) -> bool {
        match logic_imm32(v) {
            Some(f) => {
                self.emit(base | f | (n << 5) | d);
                true
            }
            None => false,
        }
    }
    /// `and wd, wn, #v` if encodable (returns false and emits nothing otherwise).
    pub fn try_and_wi(&mut self, d: R, n: R, v: u32) -> bool {
        self.logic_imm(0x1200_0000, d, n, v)
    }
    pub fn try_orr_wi(&mut self, d: R, n: R, v: u32) -> bool {
        self.logic_imm(0x3200_0000, d, n, v)
    }
    pub fn try_eor_wi(&mut self, d: R, n: R, v: u32) -> bool {
        self.logic_imm(0x5200_0000, d, n, v)
    }
    pub fn try_tst_wi(&mut self, n: R, v: u32) -> bool {
        self.logic_imm(0x7200_0000, ZR, n, v)
    }

    // ------------------------------------------------------------------------------------------ moves

    pub fn movz_w(&mut self, d: R, imm16: u32, hw: u32) {
        self.emit(0x5280_0000 | (hw << 21) | ((imm16 & 0xFFFF) << 5) | d);
    }
    pub fn movk_w(&mut self, d: R, imm16: u32, hw: u32) {
        self.emit(0x7280_0000 | (hw << 21) | ((imm16 & 0xFFFF) << 5) | d);
    }
    pub fn movn_w(&mut self, d: R, imm16: u32, hw: u32) {
        self.emit(0x1280_0000 | (hw << 21) | ((imm16 & 0xFFFF) << 5) | d);
    }
    /// `wd = v` (1-2 instructions).
    pub fn mov_wi(&mut self, d: R, v: u32) {
        let lo = v & 0xFFFF;
        let hi = v >> 16;
        if hi == 0 {
            self.movz_w(d, lo, 0);
        } else if lo == 0 {
            self.movz_w(d, hi, 1);
        } else if hi == 0xFFFF {
            self.movn_w(d, !v & 0xFFFF, 0);
        } else if lo == 0xFFFF {
            self.movn_w(d, (!v >> 16) & 0xFFFF, 1);
        } else if self.logic_imm(0x3200_0000, d, ZR, v) {
            // orr wd, wzr, #v
        } else {
            self.movz_w(d, lo, 0);
            self.movk_w(d, hi, 1);
        }
    }
    /// `xd = v` (1-4 instructions).
    pub fn mov_xi(&mut self, d: R, v: u64) {
        let mut first = true;
        for hw in 0..4 {
            let part = ((v >> (16 * hw)) & 0xFFFF) as u32;
            if part == 0 {
                continue;
            }
            if first {
                self.emit(0xD280_0000 | (hw << 21) | (part << 5) | d);
                first = false;
            } else {
                self.emit(0xF280_0000 | (hw << 21) | (part << 5) | d);
            }
        }
        if first {
            self.emit(0xD280_0000 | d);
        }
    }

    // ------------------------------------------------------------------------------------------ bitfield

    fn bfm(&mut self, base: u32, d: R, n: R, immr: u32, imms: u32) {
        self.emit(base | (immr << 16) | (imms << 10) | (n << 5) | d);
    }
    /// `ubfx wd, wn, #lsb, #width`.
    pub fn ubfx_w(&mut self, d: R, n: R, lsb: u32, width: u32) {
        self.bfm(0x5300_0000, d, n, lsb, lsb + width - 1);
    }
    /// `sbfx wd, wn, #lsb, #width`.
    pub fn sbfx_w(&mut self, d: R, n: R, lsb: u32, width: u32) {
        self.bfm(0x1300_0000, d, n, lsb, lsb + width - 1);
    }
    /// `bfi wd, wn, #lsb, #width` (insert the low `width` bits of wn at `lsb`).
    pub fn bfi_w(&mut self, d: R, n: R, lsb: u32, width: u32) {
        self.bfm(0x3300_0000, d, n, (32 - lsb) & 31, width - 1);
    }
    /// `lsl wd, wn, #s` (s in 1..=31).
    pub fn lsl_wi(&mut self, d: R, n: R, s: u32) {
        self.bfm(0x5300_0000, d, n, (32 - s) & 31, 31 - s);
    }
    pub fn lsr_wi(&mut self, d: R, n: R, s: u32) {
        self.bfm(0x5300_0000, d, n, s, 31);
    }
    pub fn asr_wi(&mut self, d: R, n: R, s: u32) {
        self.bfm(0x1300_0000, d, n, s, 31);
    }
    /// `lsr xd, xn, #s`.
    pub fn lsr_xi(&mut self, d: R, n: R, s: u32) {
        self.bfm(0xD340_0000, d, n, s, 63);
    }
    pub fn uxtb_w(&mut self, d: R, n: R) {
        self.ubfx_w(d, n, 0, 8);
    }
    pub fn uxth_w(&mut self, d: R, n: R) {
        self.ubfx_w(d, n, 0, 16);
    }
    pub fn sxtb_w(&mut self, d: R, n: R) {
        self.sbfx_w(d, n, 0, 8);
    }
    pub fn sxth_w(&mut self, d: R, n: R) {
        self.sbfx_w(d, n, 0, 16);
    }

    /// Variable shifts (count mod 32): `lslv/lsrv/asrv wd, wn, wm`.
    pub fn lslv_w(&mut self, d: R, n: R, m: R) {
        self.emit(0x1AC0_2000 | (m << 16) | (n << 5) | d);
    }
    pub fn lsrv_w(&mut self, d: R, n: R, m: R) {
        self.emit(0x1AC0_2400 | (m << 16) | (n << 5) | d);
    }
    pub fn asrv_w(&mut self, d: R, n: R, m: R) {
        self.emit(0x1AC0_2800 | (m << 16) | (n << 5) | d);
    }
    pub fn lslv_x(&mut self, d: R, n: R, m: R) {
        self.emit(0x9AC0_2000 | (m << 16) | (n << 5) | d);
    }
    pub fn lsrv_x(&mut self, d: R, n: R, m: R) {
        self.emit(0x9AC0_2400 | (m << 16) | (n << 5) | d);
    }
    pub fn asrv_x(&mut self, d: R, n: R, m: R) {
        self.emit(0x9AC0_2800 | (m << 16) | (n << 5) | d);
    }

    // ------------------------------------------------------------------------------------------ select / mul

    pub fn csel_w(&mut self, d: R, n: R, m: R, c: Cond) {
        self.emit(0x1A80_0000 | (m << 16) | ((c as u32) << 12) | (n << 5) | d);
    }
    pub fn csinc_w(&mut self, d: R, n: R, m: R, c: Cond) {
        self.emit(0x1A80_0400 | (m << 16) | ((c as u32) << 12) | (n << 5) | d);
    }
    /// `cset wd, c` (wd = c ? 1 : 0).
    pub fn cset_w(&mut self, d: R, c: Cond) {
        self.csinc_w(d, ZR, ZR, c.invert());
    }
    pub fn mul_w(&mut self, d: R, n: R, m: R) {
        self.emit(0x1B00_0000 | (m << 16) | (ZR << 10) | (n << 5) | d);
    }
    /// `smull xd, wn, wm`.
    pub fn smull(&mut self, d: R, n: R, m: R) {
        self.emit(0x9B20_0000 | (m << 16) | (ZR << 10) | (n << 5) | d);
    }
    /// `umull xd, wn, wm`.
    pub fn umull(&mut self, d: R, n: R, m: R) {
        self.emit(0x9BA0_0000 | (m << 16) | (ZR << 10) | (n << 5) | d);
    }

    // ------------------------------------------------------------------------------------------ memory

    /// Unsigned-offset load/store; `size` 1/2/4/8 bytes, `off` in bytes (multiple of size, < 4096*size).
    fn ldst_uoff(&mut self, base: u32, t: R, n: R, off: u32, size: u32) {
        assert!(off % size == 0 && off / size < 4096, "ldst offset {off}");
        self.emit(base | ((off / size) << 10) | (n << 5) | t);
    }
    pub fn ldr_w(&mut self, t: R, n: R, off: u32) {
        self.ldst_uoff(0xB940_0000, t, n, off, 4);
    }
    pub fn str_w(&mut self, t: R, n: R, off: u32) {
        self.ldst_uoff(0xB900_0000, t, n, off, 4);
    }
    pub fn ldr_x(&mut self, t: R, n: R, off: u32) {
        self.ldst_uoff(0xF940_0000, t, n, off, 8);
    }
    pub fn str_x(&mut self, t: R, n: R, off: u32) {
        self.ldst_uoff(0xF900_0000, t, n, off, 8);
    }
    pub fn ldrb(&mut self, t: R, n: R, off: u32) {
        self.ldst_uoff(0x3940_0000, t, n, off, 1);
    }
    pub fn strb(&mut self, t: R, n: R, off: u32) {
        self.ldst_uoff(0x3900_0000, t, n, off, 1);
    }
    pub fn strh(&mut self, t: R, n: R, off: u32) {
        self.ldst_uoff(0x7900_0000, t, n, off, 2);
    }

    /// Register-offset load/store `[xn, wm, uxtw]`: `base` selects size/kind.
    fn ldst_reg_uxtw(&mut self, base: u32, t: R, n: R, m: R) {
        self.emit(base | (m << 16) | (2 << 13) | (n << 5) | t);
    }
    /// Guest load of `size` bytes (zero-extended; `signed` sign-extends 1/2-byte loads to 32 bits):
    /// `ldr{b,h,sb,sh} wt, [xbase, wa, uxtw]`.
    pub fn ld_guest(&mut self, t: R, base: R, a: R, size: u32, signed: bool) {
        let op = match (size, signed) {
            (1, false) => 0x3860_0800,
            (1, true) => 0x38E0_0800,
            (2, false) => 0x7860_0800,
            (2, true) => 0x78E0_0800,
            _ => 0xB860_0800,
        };
        self.ldst_reg_uxtw(op, t, base, a);
    }
    /// Guest store of `size` bytes: `str{b,h} wt, [xbase, wa, uxtw]`.
    pub fn st_guest(&mut self, t: R, base: R, a: R, size: u32) {
        let op = match size {
            1 => 0x3820_0800,
            2 => 0x7820_0800,
            _ => 0xB820_0800,
        };
        self.ldst_reg_uxtw(op, t, base, a);
    }
    /// `ldr xt, [xn, xm, lsl #3]`.
    pub fn ldr_x_idx(&mut self, t: R, n: R, m: R) {
        self.emit(0xF860_0800 | (m << 16) | (3 << 13) | (1 << 12) | (n << 5) | t);
    }

    /// `stp/ldp` with signed offset; `x` = 64-bit registers.
    fn ldstp(&mut self, base: u32, t1: R, t2: R, n: R, off: i32, x: bool) {
        let sc = if x { 8 } else { 4 };
        assert!(off % sc == 0 && (-64..64).contains(&(off / sc)), "ldp/stp offset {off}");
        let imm7 = ((off / sc) as u32) & 0x7F;
        self.emit(base | (imm7 << 15) | (t2 << 10) | (n << 5) | t1);
    }
    pub fn stp_w(&mut self, t1: R, t2: R, n: R, off: i32) {
        self.ldstp(0x2900_0000, t1, t2, n, off, false);
    }
    pub fn ldp_w(&mut self, t1: R, t2: R, n: R, off: i32) {
        self.ldstp(0x2940_0000, t1, t2, n, off, false);
    }
    pub fn stp_x(&mut self, t1: R, t2: R, n: R, off: i32) {
        self.ldstp(0xA900_0000, t1, t2, n, off, true);
    }
    pub fn ldp_x(&mut self, t1: R, t2: R, n: R, off: i32) {
        self.ldstp(0xA940_0000, t1, t2, n, off, true);
    }
    /// `stp xt1, xt2, [xn, #off]!`.
    pub fn stp_x_pre(&mut self, t1: R, t2: R, n: R, off: i32) {
        self.ldstp(0xA980_0000, t1, t2, n, off, true);
    }
    /// `ldp xt1, xt2, [xn], #off`.
    pub fn ldp_x_post(&mut self, t1: R, t2: R, n: R, off: i32) {
        self.ldstp(0xA8C0_0000, t1, t2, n, off, true);
    }
    /// `ldr xt, <label>` (PC-relative literal).
    pub fn ldr_x_lit(&mut self, t: R, l: Label) {
        self.fix(l, Fix::B19);
        self.emit(0x5800_0000 | t);
    }

    // ------------------------------------------------------------------------------------------ branches

    pub fn b(&mut self, l: Label) {
        self.fix(l, Fix::B26);
        self.emit(0x1400_0000);
    }
    pub fn b_cond(&mut self, c: Cond, l: Label) {
        self.fix(l, Fix::B19);
        self.emit(0x5400_0000 | c as u32);
    }
    pub fn cbz_w(&mut self, t: R, l: Label) {
        self.fix(l, Fix::B19);
        self.emit(0x3400_0000 | t);
    }
    pub fn cbnz_w(&mut self, t: R, l: Label) {
        self.fix(l, Fix::B19);
        self.emit(0x3500_0000 | t);
    }
    /// `tbz/tbnz wt, #bit, label`.
    pub fn tbz(&mut self, t: R, bit: u32, l: Label) {
        self.fix(l, Fix::B14);
        self.emit(0x3600_0000 | ((bit & 31) << 19) | t);
    }
    pub fn tbnz(&mut self, t: R, bit: u32, l: Label) {
        self.fix(l, Fix::B14);
        self.emit(0x3700_0000 | ((bit & 31) << 19) | t);
    }
    pub fn br(&mut self, n: R) {
        self.emit(0xD61F_0000 | (n << 5));
    }
    pub fn blr(&mut self, n: R) {
        self.emit(0xD63F_0000 | (n << 5));
    }
    pub fn ret(&mut self) {
        self.emit(0xD65F_03C0);
    }
    pub fn brk(&mut self) {
        self.emit(0xD420_0000);
    }

    /// Emit a 64-bit literal (8-byte aligned) at a label.
    pub fn literal64(&mut self, l: Label, v: u64) {
        if self.code.len() % 2 != 0 {
            self.emit(0xD503_201F); // nop (alignment)
        }
        self.bind(l);
        self.emit(v as u32);
        self.emit((v >> 32) as u32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reference encodings from the ARM ARM (checked with clang/llvm-objdump).
    #[test]
    fn encodings() {
        let mut a = Asm::new();
        a.try_and_wi(0, 1, 0xFF); // and w0, w1, #0xff
        a.add_wi(2, 3, 4); // add w2, w3, #4
        a.sub_wi(2, 3, 0x1000); // sub w2, w3, #1, lsl #12
        a.cmp_wi(16, 0x10000); // cmp w16, #16, lsl #12
        a.ldr_w(19, 27, 8); // ldr w19, [x27, #8]
        a.ld_guest(1, 28, 16, 4, false); // ldr w1, [x28, w16, uxtw]
        a.st_guest(1, 28, 16, 1); // strb w1, [x28, w16, uxtw]
        a.bfi_w(19, 11, 8, 8); // bfi w19, w11, #8, #8
        a.ubfx_w(0, 19, 8, 8); // ubfx w0, w19, #8, #8
        a.lsl_wi(0, 1, 24); // lsl w0, w1, #24
        a.cset_w(5, Cond::Lo); // cset w5, lo
        a.mov_wi(3, 0x7e8dec); // mov w3, #0x8dec ; movk w3, #0x7e, lsl #16
        a.stp_x_pre(29, 30, SP, -16); // stp x29, x30, [sp, #-16]!
        a.ldp_x_post(29, 30, SP, 16); // ldp x29, x30, [sp], #16
        assert_eq!(
            a.code,
            vec![
                0x1200_1C20,
                0x1100_1062,
                0x5140_0462,
                0x7140_421F,
                0xB940_0B73,
                0xB870_4B81,
                0x3830_4B81,
                0x3318_1D73,
                0x5308_3E60,
                0x5308_1C20,
                0x1A9F_27E5,
                0x5291_BD83,
                0x72A0_0FC3,
                0xA9BF_7BFD,
                0xA8C1_7BFD,
            ]
        );
    }

    #[test]
    fn logic_immediates() {
        // Decode N:immr:imms back and compare (32-bit, N = 0).
        fn decode(f: u32) -> u32 {
            let immr = (f >> 16) & 0x3f;
            let imms = (f >> 10) & 0x3f;
            let len = 31 - ((!imms) & 0x3f | 0x40).leading_zeros() as i32 + 26; // highest set bit of (~imms & 0x3f)
            let _ = len;
            let mut size = 32u32;
            while size > 1 && (imms & size) != 0 && size != 32 {
                size >>= 1;
            }
            // Element size from the leading ones of imms.
            let esize = if imms & 0x20 == 0 {
                32
            } else if imms & 0x10 == 0 {
                16
            } else if imms & 0x08 == 0 {
                8
            } else if imms & 0x04 == 0 {
                4
            } else {
                2
            };
            let s = imms & (esize - 1);
            let r = immr & (esize - 1);
            let ones: u64 = (1u64 << (s + 1)) - 1;
            let m: u64 = if esize == 32 { 0xFFFF_FFFF } else { (1u64 << esize) - 1 };
            let elt = if r == 0 { ones } else { ((ones >> r) | (ones << (esize - r))) & m };
            let mut v = 0u64;
            let mut p = 0;
            while p < 32 {
                v |= elt << p;
                p += esize;
            }
            v as u32
        }
        for v in
            [0xFFu32, 0xFFFF, 0x10, 0x40, 0x8D5, 0x8000_0000, 0xFFFF_FF00, 0x5555_5555, 0x0F0F_0F0F, 0x7FFF_FFFF, 0xFFF]
        {
            match logic_imm32(v) {
                Some(f) => assert_eq!(decode(f), v, "{v:#x}"),
                None => assert!(v == 0x8D5, "{v:#x} should encode"),
            }
        }
        // Random bit patterns: whenever an encoding is returned it must decode to the value.
        let mut x = 0x1234_5678u32;
        for _ in 0..100_000 {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            if let Some(f) = logic_imm32(x) {
                assert_eq!(decode(f), x);
            }
        }
    }
}
