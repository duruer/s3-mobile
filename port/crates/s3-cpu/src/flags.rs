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

impl Cpu {
    #[inline(always)]
    pub fn flag(&self, f: u32) -> bool {
        self.eflags & f != 0
    }

    #[inline(always)]
    pub fn set_flag(&mut self, f: u32, on: bool) {
        if on {
            self.eflags |= f;
        } else {
            self.eflags &= !f;
        }
    }

    /// Set SF, ZF, PF from a result (other flags unchanged).
    #[inline(always)]
    pub fn set_szp(&mut self, res: u32, size: u32) {
        let r = res & mask(size);
        self.eflags = (self.eflags & !(fl::SF | fl::ZF | fl::PF)) | szp(r, size);
    }

    /// `a + b + carry`, all six status flags. Returns the masked result.
    #[inline(always)]
    pub fn alu_add(&mut self, a: u32, b: u32, carry: u32, size: u32) -> u32 {
        let m = mask(size);
        let (a, b) = (a & m, b & m);
        let wide = a as u64 + b as u64 + carry as u64;
        let r = (wide as u32) & m;
        let cf = (wide > m as u64) as u32;
        let af = (a ^ b ^ r) & fl::AF;
        let of = if (a ^ r) & (b ^ r) & sign_bit(size) != 0 { fl::OF } else { 0 };
        self.eflags = (self.eflags & !fl::STATUS) | cf | af | of | szp(r, size);
        r
    }

    /// `a - b - borrow`, all six status flags (CMP uses this and discards the result).
    #[inline(always)]
    pub fn alu_sub(&mut self, a: u32, b: u32, borrow: u32, size: u32) -> u32 {
        let m = mask(size);
        let (a, b) = (a & m, b & m);
        let r = a.wrapping_sub(b).wrapping_sub(borrow) & m;
        let cf = ((a as u64) < b as u64 + borrow as u64) as u32;
        let af = (a ^ b ^ r) & fl::AF;
        let of = if (a ^ b) & (a ^ r) & sign_bit(size) != 0 { fl::OF } else { 0 };
        self.eflags = (self.eflags & !fl::STATUS) | cf | af | of | szp(r, size);
        r
    }

    /// Flags of AND/OR/XOR/TEST: CF=OF=0, AF=0 (undefined on hardware), SZP from result.
    #[inline(always)]
    pub fn alu_logic(&mut self, r: u32, size: u32) -> u32 {
        let r = r & mask(size);
        self.eflags = (self.eflags & !fl::STATUS) | szp(r, size);
        r
    }

    /// Evaluate a condition code against EFLAGS.
    #[inline(always)]
    pub fn cond(&self, cc: ConditionCode) -> bool {
        let f = self.eflags;
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
