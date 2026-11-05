//! Eager EFLAGS computation helpers. `size` is the operand size in bytes (1, 2 or 4).

use iced_x86::ConditionCode;

use crate::{fl, Cpu};

#[inline(always)]
pub fn mask(size: u32) -> u32 {
    match size {
        1 => 0xFF,
        2 => 0xFFFF,
        _ => 0xFFFF_FFFF,
    }
}

#[inline(always)]
pub fn sign_bit(size: u32) -> u32 {
    match size {
        1 => 0x80,
        2 => 0x8000,
        _ => 0x8000_0000,
    }
}

/// Sign-extend the low `size` bytes of `v` to i32.
#[inline(always)]
pub fn sext(v: u32, size: u32) -> i32 {
    match size {
        1 => v as u8 as i8 as i32,
        2 => v as u16 as i16 as i32,
        _ => v as i32,
    }
}

/// PF value (0 or `fl::PF`) for every low result byte.
static PARITY: [u8; 256] = {
    let mut t = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        if (i as u8).count_ones() & 1 == 0 {
            t[i] = fl::PF as u8;
        }
        i += 1;
    }
    t
};

/// SF | ZF | PF of a masked result.
#[inline(always)]
pub fn szp(r: u32, size: u32) -> u32 {
    let sf = if r & sign_bit(size) != 0 { fl::SF } else { 0 };
    let zf = if r == 0 { fl::ZF } else { 0 };
    sf | zf | PARITY[(r & 0xFF) as usize] as u32
}

/// Lazy-flag kinds ([`Cpu::lk`]): which operation produced the status flags (`NONE` = `ef` holds them).
pub(crate) mod lk {
    pub const NONE: u8 = 0;
    /// `lr = la + lb` (no carry in).
    pub const ADD: u8 = 1;
    /// `lr = la - lb` (no borrow in; also CMP).
    pub const SUB: u8 = 2;
    /// AND/OR/XOR/TEST: CF=OF=AF=0.
    pub const LOGIC: u8 = 3;
    /// INC: like ADD with lb = 1, CF = `lcf`.
    pub const INC: u8 = 4;
    /// DEC: like SUB with lb = 1, CF = `lcf`.
    pub const DEC: u8 = 5;
}

impl Cpu {
    /// Full EFLAGS (status bits computed from the lazy state if needed).
    #[inline(always)]
    pub fn eflags(&self) -> u32 {
        if self.lk == lk::NONE {
            self.ef
        } else {
            (self.ef & !fl::STATUS) | self.lazy_status()
        }
    }

    /// Replace EFLAGS.
    #[inline(always)]
    pub fn set_eflags(&mut self, v: u32) {
        self.ef = v;
        self.lk = lk::NONE;
    }

    /// Fold the lazy state into `ef` (before a partial flag update).
    #[inline(always)]
    pub(crate) fn materialize(&mut self) {
        if self.lk != lk::NONE {
            self.ef = self.eflags();
            self.lk = lk::NONE;
        }
    }

    /// Status flags (CF PF AF ZF SF OF) of the lazy state (`lk != NONE`). Must give exactly what the eager
    /// helpers `alu_add`/`alu_sub`/`alu_logic` (+ CF restore for inc/dec) would have stored.
    #[inline(never)]
    fn lazy_status(&self) -> u32 {
        let size = self.lsz as u32;
        let m = mask(size);
        let sb = sign_bit(size);
        let (a, b, r) = (self.la & m, self.lb & m, self.lr & m);
        match self.lk {
            lk::LOGIC => szp(r, size),
            lk::ADD | lk::INC => {
                let cf = if self.lk == lk::ADD { (r < a) as u32 } else { self.lcf };
                let af = (a ^ b ^ r) & fl::AF;
                let of = if (a ^ r) & (b ^ r) & sb != 0 { fl::OF } else { 0 };
                cf | af | of | szp(r, size)
            }
            _ => {
                let cf = if self.lk == lk::SUB { (a < b) as u32 } else { self.lcf };
                let af = (a ^ b ^ r) & fl::AF;
                let of = if (a ^ b) & (a ^ r) & sb != 0 { fl::OF } else { 0 };
                cf | af | of | szp(r, size)
            }
        }
    }

    /// AF (0 or `fl::AF`) without materialising the other flags.
    #[inline(always)]
    pub(crate) fn af(&self) -> u32 {
        match self.lk {
            lk::NONE => self.ef & fl::AF,
            lk::LOGIC => 0,
            _ => (self.la ^ self.lb ^ self.lr) & fl::AF,
        }
    }

    /// OF (0/1) without materialising the other flags.
    #[inline(always)]
    pub(crate) fn of(&self) -> u32 {
        let sh = 32 - 8 * self.lsz as u32;
        let (a, b, r) = (self.la << sh, self.lb << sh, self.lr << sh);
        match self.lk {
            lk::NONE => (self.ef >> 11) & 1,
            lk::LOGIC => 0,
            lk::ADD | lk::INC => ((a ^ r) & (b ^ r)) >> 31,
            _ => ((a ^ b) & (a ^ r)) >> 31,
        }
    }

    /// CF (0/1) without materialising the other flags.
    #[inline(always)]
    pub(crate) fn cf(&self) -> u32 {
        match self.lk {
            lk::NONE => self.ef & fl::CF,
            lk::ADD => ((self.lr & mask(self.lsz as u32)) < (self.la & mask(self.lsz as u32))) as u32,
            lk::SUB => ((self.la & mask(self.lsz as u32)) < (self.lb & mask(self.lsz as u32))) as u32,
            lk::LOGIC => 0,
            _ => self.lcf,
        }
    }

    #[inline(always)]
    pub fn flag(&self, f: u32) -> bool {
        match f {
            fl::CF => self.cf() != 0,
            fl::AF => self.af() != 0,
            fl::OF => self.of() != 0,
            fl::ZF if self.lk != lk::NONE => self.lr == 0,
            _ => self.eflags() & f != 0,
        }
    }

    #[inline(always)]
    pub fn set_flag(&mut self, f: u32, on: bool) {
        self.materialize();
        if on {
            self.ef |= f;
        } else {
            self.ef &= !f;
        }
    }

    /// Set SF, ZF, PF from a result (other flags unchanged).
    #[inline(always)]
    pub fn set_szp(&mut self, res: u32, size: u32) {
        self.materialize();
        let r = res & mask(size);
        self.ef = (self.ef & !(fl::SF | fl::ZF | fl::PF)) | szp(r, size);
    }

    #[inline(always)]
    fn set_lazy(&mut self, kind: u8, a: u32, b: u32, r: u32, size: u32) {
        self.lk = kind;
        self.la = a;
        self.lb = b;
        self.lr = r;
        self.lsz = size as u8;
    }

    /// `a + b + carry`, all six status flags. Returns the masked result.
    #[inline(always)]
    pub fn alu_add(&mut self, a: u32, b: u32, carry: u32, size: u32) -> u32 {
        let m = mask(size);
        let (a, b) = (a & m, b & m);
        if carry == 0 {
            let r = a.wrapping_add(b) & m;
            self.set_lazy(lk::ADD, a, b, r, size);
            return r;
        }
        let wide = a as u64 + b as u64 + carry as u64;
        let r = (wide as u32) & m;
        let cf = (wide > m as u64) as u32;
        let af = (a ^ b ^ r) & fl::AF;
        let of = if (a ^ r) & (b ^ r) & sign_bit(size) != 0 { fl::OF } else { 0 };
        self.set_eflags((self.ef & !fl::STATUS) | cf | af | of | szp(r, size));
        r
    }

    /// `a - b - borrow`, all six status flags (CMP uses this and discards the result).
    #[inline(always)]
    pub fn alu_sub(&mut self, a: u32, b: u32, borrow: u32, size: u32) -> u32 {
        let m = mask(size);
        let (a, b) = (a & m, b & m);
        if borrow == 0 {
            let r = a.wrapping_sub(b) & m;
            self.set_lazy(lk::SUB, a, b, r, size);
            return r;
        }
        let r = a.wrapping_sub(b).wrapping_sub(borrow) & m;
        let cf = ((a as u64) < b as u64 + borrow as u64) as u32;
        let af = (a ^ b ^ r) & fl::AF;
        let of = if (a ^ b) & (a ^ r) & sign_bit(size) != 0 { fl::OF } else { 0 };
        self.set_eflags((self.ef & !fl::STATUS) | cf | af | of | szp(r, size));
        r
    }

    /// Flags of AND/OR/XOR/TEST: CF=OF=0, AF=0 (undefined on hardware), SZP from result.
    #[inline(always)]
    pub fn alu_logic(&mut self, r: u32, size: u32) -> u32 {
        let r = r & mask(size);
        self.set_lazy(lk::LOGIC, 0, 0, r, size);
        r
    }

    /// `inc` (CF unchanged).
    #[inline(always)]
    pub fn alu_inc(&mut self, a: u32, size: u32) -> u32 {
        let cf = self.cf();
        let m = mask(size);
        let a = a & m;
        let r = a.wrapping_add(1) & m;
        self.set_lazy(lk::INC, a, 1, r, size);
        self.lcf = cf;
        r
    }

    /// `dec` (CF unchanged).
    #[inline(always)]
    pub fn alu_dec(&mut self, a: u32, size: u32) -> u32 {
        let cf = self.cf();
        let m = mask(size);
        let a = a & m;
        let r = a.wrapping_sub(1) & m;
        self.set_lazy(lk::DEC, a, 1, r, size);
        self.lcf = cf;
        r
    }

    /// Evaluate a condition code against EFLAGS.
    #[inline(always)]
    pub fn cond(&self, cc: ConditionCode) -> bool {
        let f = self.eflags();
        let cf = f & fl::CF != 0;
        let zf = f & fl::ZF != 0;
        let sf = f & fl::SF != 0;
        let of = f & fl::OF != 0;
        let pf = f & fl::PF != 0;
        match cc {
            ConditionCode::None => true,
            ConditionCode::o => of,
            ConditionCode::no => !of,
            ConditionCode::b => cf,
            ConditionCode::ae => !cf,
            ConditionCode::e => zf,
            ConditionCode::ne => !zf,
            ConditionCode::be => cf || zf,
            ConditionCode::a => !cf && !zf,
            ConditionCode::s => sf,
            ConditionCode::ns => !sf,
            ConditionCode::p => pf,
            ConditionCode::np => !pf,
            ConditionCode::l => sf != of,
            ConditionCode::ge => sf == of,
            ConditionCode::le => zf || sf != of,
            ConditionCode::g => !zf && sf == of,
        }
    }
}
