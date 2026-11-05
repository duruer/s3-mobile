//! Pure numeric helpers for the x87 emulation: 80-bit extended <-> f64, packed BCD, rounding control,
//! precision control, `ldexp`/`frexp`-style helpers. No CPU state here.

/// The x87 "real indefinite" (negative quiet NaN) as f64.
pub const INDEFINITE: f64 = f64::from_bits(0xFFF8_0000_0000_0000);

/// Round `m / 2^s` to nearest, ties to even (s may be 0..=127).
fn shr_round_ne(m: u64, s: u32) -> u64 {
    if s == 0 {
        return m;
    }
    if s >= 128 {
        return 0;
    }
    let m = m as u128;
    let q = m >> s;
    let rem = m & ((1u128 << s) - 1);
    let half = 1u128 << (s - 1);
    let q = if rem > half || (rem == half && (q & 1) == 1) { q + 1 } else { q };
    q as u64
}

/// f64 -> 80-bit extended `(significand with explicit integer bit, sign|exponent)`. Exact.
pub fn f64_to_f80(v: f64) -> (u64, u16) {
    let bits = v.to_bits();
    let sign = ((bits >> 63) as u16) << 15;
    let e = ((bits >> 52) & 0x7FF) as i32;
    let f = bits & ((1u64 << 52) - 1);
    if e == 0 {
        if f == 0 {
            return (0, sign);
        }
        // Denormal: normalise (80-bit has the exponent range to represent it as a normal).
        let p = 63 - f.leading_zeros() as i32; // MSB position (< 52)
        let mant = f << (63 - p);
        let exp = p - 1074 + 16383;
        return (mant, sign | exp as u16);
    }
    if e == 0x7FF {
        if f == 0 {
            return (1u64 << 63, sign | 0x7FFF);
        }
        return ((1u64 << 63) | (f << 11), sign | 0x7FFF);
    }
    ((1u64 << 63) | (f << 11), sign | (e - 1023 + 16383) as u16)
}

/// 80-bit extended -> f64, rounding to nearest-even (overflow -> inf, underflow -> denormal/0).
pub fn f80_to_f64(mant: u64, se: u16) -> f64 {
    let sign = ((se >> 15) as u64) << 63;
    let e = (se & 0x7FFF) as i32;
    if e == 0x7FFF {
        let frac = (mant << 1) >> 12;
        if mant << 1 == 0 {
            return f64::from_bits(sign | 0x7FF0_0000_0000_0000);
        }
        // NaN: keep the top fraction bits, force quiet.
        return f64::from_bits(sign | 0x7FF8_0000_0000_0000 | frac);
    }
    if mant == 0 {
        return f64::from_bits(sign);
    }
    let lz = mant.leading_zeros() as i32;
    let m = mant << lz;
    // value = m / 2^63 * 2^unb
    let unb = if e == 0 { 1 } else { e } - 16383 - lz;
    let mut ef = unb + 1023;
    if ef >= 1 {
        let mut r = shr_round_ne(m, 11);
        if r == 1u64 << 53 {
            r >>= 1;
            ef += 1;
        }
        if ef >= 0x7FF {
            return f64::from_bits(sign | 0x7FF0_0000_0000_0000);
        }
        f64::from_bits(sign | ((ef as u64) << 52) | (r & ((1u64 << 52) - 1)))
    } else {
        let s = (11 + (1 - ef)) as u32;
        let r = shr_round_ne(m, s);
        f64::from_bits(sign | r)
    }
}

/// f32 -> f64 as `fld m32` does it. Non-NaN values convert exactly; NaNs are quieted with the payload kept
/// (bit-level, so the host's NaN conversion rules never matter).
pub fn f32_to_f64(x: f32) -> f64 {
    if x.is_nan() {
        let b = x.to_bits() as u64;
        let sign = (b >> 31) << 63;
        let payload = (b & 0x003F_FFFF) << 29;
        return f64::from_bits(sign | 0x7FF8_0000_0000_0000 | payload);
    }
    x as f64
}

/// f64 -> f32 as `fst m32` does it: round to nearest even (IEEE conversion, identical on all hosts);
/// NaNs are quieted and the payload truncated (bit-level).
pub fn f64_to_f32(x: f64) -> f32 {
    if x.is_nan() {
        let b = x.to_bits();
        let sign = ((b >> 63) as u32) << 31;
        let payload = ((b >> 29) & 0x003F_FFFF) as u32;
        return f32::from_bits(sign | 0x7FC0_0000 | payload);
    }
    x as f32
}

/// Round to an integral value using x87 rounding control (0 nearest-even, 1 down, 2 up, 3 truncate).
/// NaN is returned quieted (never through the host's rounding functions, which may be libm calls).
pub fn round_rc(v: f64, rc: u16) -> f64 {
    if v.is_nan() {
        return f64::from_bits(v.to_bits() | (1u64 << 51));
    }
    match rc & 3 {
        0 => v.round_ties_even(),
        1 => v.floor(),
        2 => v.ceil(),
        _ => v.trunc(),
    }
}

/// Apply x87 precision control to an arithmetic result: PC=0 (24-bit significand) rounds the significand to
/// 24 bits (keeping the wide exponent range); PC=2/3 are left at f64's 53 bits.
pub fn apply_pc(v: f64, control: u16) -> f64 {
    if (control >> 8) & 3 != 0 || !v.is_finite() || v == 0.0 {
        return v;
    }
    // Round the 52-bit fraction to 23 bits (drop 29) with ties-to-even. The carry may ripple into the exponent,
    // which is the correct behaviour (including overflow to infinity).
    let bits = v.to_bits();
    let keep_lsb = (bits >> 29) & 1;
    let r = bits.wrapping_add((1u64 << 28) - 1 + keep_lsb) & !((1u64 << 29) - 1);
    f64::from_bits(r)
}

/// `x * 2^n` without intermediate overflow/underflow for large `n`.
pub fn ldexp(mut x: f64, mut n: i64) -> f64 {
    if x == 0.0 || !x.is_finite() {
        return x;
    }
    n = n.clamp(-5000, 5000);
    let p1000 = f64::from_bits(((1023 + 1000) as u64) << 52);
    let m1000 = f64::from_bits(((1023 - 1000) as u64) << 52);
    while n > 1000 {
        x *= p1000;
        n -= 1000;
        if x.is_infinite() {
            return x;
        }
    }
    while n < -1000 {
        x *= m1000;
        n += 1000;
        if x == 0.0 {
            return x;
        }
    }
    // Now |n| <= 1000: split so that each factor is a normal f64.
    let a = n / 2;
    let b = n - a;
    x * f64::from_bits(((1023 + a) as u64) << 52) * f64::from_bits(((1023 + b) as u64) << 52)
}

/// Unbiased binary exponent of a finite non-zero `x` (floor(log2|x|)), as `fxtract` reports it.
pub fn exponent_of(x: f64) -> i64 {
    let bits = x.to_bits();
    let e = ((bits >> 52) & 0x7FF) as i64;
    if e == 0 {
        let f = bits & ((1u64 << 52) - 1);
        let p = 63 - f.leading_zeros() as i64;
        p - 1074
    } else {
        e - 1023
    }
}

/// Decode 10-byte packed BCD (18 digits + sign byte).
pub fn bcd_to_f64(b: &[u8; 10]) -> f64 {
    let mut v: u64 = 0;
    for i in (0..9).rev() {
        let hi = (b[i] >> 4) as u64;
        let lo = (b[i] & 0xF) as u64;
        v = v * 100 + hi * 10 + lo;
    }
    let f = v as f64;
    if b[9] & 0x80 != 0 {
        -f
    } else {
        f
    }
}

/// Encode an integral value as 10-byte packed BCD. Returns `None` if out of range (|v| >= 10^18) or NaN.
pub fn f64_to_bcd(v: f64) -> Option<[u8; 10]> {
    if v.is_nan() || v.abs() >= 1e18 {
        return None;
    }
    let mut out = [0u8; 10];
    let mut n = v.abs() as u64;
    for byte in out.iter_mut().take(9) {
        let lo = (n % 10) as u8;
        n /= 10;
        let hi = (n % 10) as u8;
        n /= 10;
        *byte = (hi << 4) | lo;
    }
    if v.is_sign_negative() {
        out[9] = 0x80;
    }
    Some(out)
}

/// The BCD indefinite encoding stored by `fbstp` on invalid operation.
pub const BCD_INDEFINITE: [u8; 10] = [0, 0, 0, 0, 0, 0, 0, 0xC0, 0xFF, 0xFF];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f80_roundtrip_and_known_values() {
        for &v in &[0.0, -0.0, 1.0, -2.5, std::f64::consts::PI, 1e300, -1e-300, 5e-324, f64::MAX, f64::MIN_POSITIVE] {
            let (m, se) = f64_to_f80(v);
            assert_eq!(f80_to_f64(m, se).to_bits(), v.to_bits(), "{v}");
        }
        // 1.0 = 3FFF 8000000000000000
        assert_eq!(f64_to_f80(1.0), (0x8000_0000_0000_0000, 0x3FFF));
        // pi as stored by fldpi (rounded to 64 bits): C90FDAA22168C235 -> nearest f64 pi
        assert_eq!(f80_to_f64(0xC90F_DAA2_2168_C235, 0x4000), std::f64::consts::PI);
        // Overflow / underflow
        assert!(f80_to_f64(0x8000_0000_0000_0000, 0x7000).is_infinite());
        assert_eq!(f80_to_f64(0x8000_0000_0000_0000, 0x0001), 0.0);
        // Infinity, NaN
        assert_eq!(f80_to_f64(0x8000_0000_0000_0000, 0xFFFF), f64::NEG_INFINITY);
        assert!(f80_to_f64(0xC000_0000_0000_0000, 0x7FFF).is_nan());
        // Ties-to-even on the 11 dropped bits: 1 + 2^-53 -> 1.0, 1 + 3*2^-53 -> 1 + 2^-51
        assert_eq!(f80_to_f64(0x8000_0000_0000_0400, 0x3FFF), 1.0);
        assert_eq!(f80_to_f64(0x8000_0000_0000_0C00, 0x3FFF), 1.0 + 2f64.powi(-51));
    }

    #[test]
    fn rounding_and_bcd() {
        assert_eq!(round_rc(2.5, 0), 2.0);
        assert_eq!(round_rc(-2.5, 1), -3.0);
        assert_eq!(round_rc(2.1, 2), 3.0);
        assert_eq!(round_rc(-2.9, 3), -2.0);
        assert_eq!(apply_pc(1.0 + 2f64.powi(-30), 0x007F), 1.0);
        assert_eq!(apply_pc(1.0 + 2f64.powi(-30), 0x027F), 1.0 + 2f64.powi(-30));
        let b = f64_to_bcd(-1234567.0).unwrap();
        assert_eq!(&b[..4], &[0x67, 0x45, 0x23, 0x01]);
        assert_eq!(bcd_to_f64(&b), -1234567.0);
        assert_eq!(ldexp(1.0, -1074), 5e-324);
        assert_eq!(ldexp(3.0, 4), 48.0);
        assert_eq!(exponent_of(48.0), 5);
    }

    #[test]
    fn f32_nan_conversion_is_bit_exact() {
        // SNaN f32 0x7F800001 -> quiet f64 with payload << 29
        assert_eq!(f32_to_f64(f32::from_bits(0x7F80_0001)).to_bits(), 0x7FF8_0000_2000_0000);
        assert_eq!(f32_to_f64(f32::from_bits(0xFFC0_0000)).to_bits(), 0xFFF8_0000_0000_0000);
        assert_eq!(f64_to_f32(f64::from_bits(0xFFF8_0000_0000_0000)).to_bits(), 0xFFC0_0000);
        assert_eq!(f64_to_f32(f64::from_bits(0x7FF0_0000_2000_0000)).to_bits(), 0x7FC0_0001);
        assert_eq!(f64_to_f32(0.1), 0.1f32);
        assert_eq!(f32_to_f64(0.1f32), 0.1f32 as f64);
    }
}
