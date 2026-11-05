//! Deterministic x87 math: every result here is a pure function of the input bits and is bit-identical on every
//! host (x86_64, aarch64, i686 with SSE2, wasm32). Needed for lockstep multiplayer (see
//! `docs/porting/determinism.md`).
//!
//! Rules:
//! - Never call the host libm (`f64::sin`, `f64::ln_1p`, `%`, ...): its results differ between glibc, Bionic,
//!   musl and macOS. Transcendentals use the pinned pure-Rust `libm` crate (a port of musl/FreeBSD fdlibm,
//!   built with `default-features = false`, i.e. no arch intrinsics). fdlibm is not correctly rounded (< 1 ulp),
//!   but it is a fixed algorithm made of IEEE basic operations only, so its output is reproducible.
//! - Only IEEE-754 basic operations (`+ - * /`, `sqrt`, conversions) are used directly: they are correctly
//!   rounded on all hosts. Rust never contracts `a*b+c` into an FMA, so no fusion differences.
//! - NaNs are never produced by the host: x86 SSE creates `0xFFF8..` (negative), aarch64 creates `0x7FF8..`
//!   (positive), and two-NaN propagation depends on operand order (which LLVM may commute). We decide every NaN
//!   result ourselves with the x87 rules (see [`propagate_nan`]).
//! - `fmod` is done in integer arithmetic ([`fprem`]), which also gives the exact quotient bits.

use super::conv::INDEFINITE;

/// Is `x` a signalling NaN (quiet bit clear)?
#[inline]
pub fn is_snan(x: f64) -> bool {
    x.is_nan() && (x.to_bits() >> 51) & 1 == 0
}

/// Quiet a NaN (set the quiet bit), keeping sign and payload.
#[inline]
pub fn quiet(x: f64) -> f64 {
    f64::from_bits(x.to_bits() | (1u64 << 51))
}

/// x87 NaN propagation for a two-operand instruction where at least one operand is a NaN:
/// one NaN -> that NaN (quieted); two NaNs -> the one with the larger significand (quieted); equal significands
/// -> `a` (the destination operand). The caller raises IE when [`is_snan`] is true for either operand.
/// Deterministic: does not depend on host NaN rules or operand order chosen by the compiler.
pub fn propagate_nan(a: f64, b: f64) -> f64 {
    match (a.is_nan(), b.is_nan()) {
        (true, false) => quiet(a),
        (false, true) => quiet(b),
        _ => {
            let sig = |x: f64| x.to_bits() & ((1u64 << 51) - 1);
            if sig(b) > sig(a) {
                quiet(b)
            } else {
                quiet(a)
            }
        }
    }
}

/// Canonicalise a host result of a one-operand operation: NaN input -> quieted input; NaN output from a
/// non-NaN input -> x87 indefinite. Returns (value, invalid).
#[inline]
pub fn canon1(input: f64, r: f64) -> (f64, bool) {
    if input.is_nan() {
        (quiet(input), is_snan(input))
    } else if r.is_nan() {
        (INDEFINITE, true)
    } else {
        (r, false)
    }
}

// ------------------------------------------------------------------------------------------- transcendentals

/// `fsin` for |x| < 2^63 (caller handles the out-of-range case). Infinity -> indefinite.
pub fn sin(x: f64) -> f64 {
    if x.is_nan() {
        return quiet(x);
    }
    if x.is_infinite() {
        return INDEFINITE;
    }
    libm::sin(x)
}

/// `fcos`.
pub fn cos(x: f64) -> f64 {
    if x.is_nan() {
        return quiet(x);
    }
    if x.is_infinite() {
        return INDEFINITE;
    }
    libm::cos(x)
}

/// `fptan` (the pushed 1.0 is the caller's job).
pub fn tan(x: f64) -> f64 {
    if x.is_nan() {
        return quiet(x);
    }
    if x.is_infinite() {
        return INDEFINITE;
    }
    libm::tan(x)
}

/// `fpatan`: atan2(y, x) with x87 NaN propagation (ST(1)=y is the "first" operand).
pub fn atan2(y: f64, x: f64) -> f64 {
    if y.is_nan() || x.is_nan() {
        return propagate_nan(y, x);
    }
    libm::atan2(y, x)
}

/// `f2xm1`: 2^x - 1 (x87 domain -1 <= x <= 1; other finite inputs are still computed).
/// Computed as expm1(x * ln2) — the product is rounded once, so the result is within ~1 ulp of the true value.
pub fn exp2m1(x: f64) -> f64 {
    if x.is_nan() {
        return quiet(x);
    }
    if x == f64::NEG_INFINITY {
        return -1.0;
    }
    if x == f64::INFINITY {
        return x;
    }
    if x == 0.0 {
        return x; // keeps the sign of zero
    }
    // t = x*ln2 as a double-double (p + e): Dekker's exact product (no FMA, so identical everywhere) plus the
    // low part of ln2. Then 2^x - 1 = expm1(p + e) ~= expm1(p) + e * (1 + expm1(p)).
    let p = x * LN2_HI_D;
    let e = two_prod_err(x, LN2_HI_D, p) + x * LN2_LO_D;
    let m = libm::expm1(p);
    m + e * (1.0 + m)
}

/// ln 2 = LN2_HI_D + LN2_LO_D (LN2_HI_D = the f64 nearest ln 2).
const LN2_HI_D: f64 = std::f64::consts::LN_2;
const LN2_LO_D: f64 = f64::from_bits(0x3C7A_BC9E_3B39_803F); // 2.3190468138462996e-17

/// Exact error of the product `a*b` rounded to `p` (Dekker/Veltkamp split; valid for |a|,|b| well inside the
/// f64 range, which holds for the f2xm1 domain).
fn two_prod_err(a: f64, b: f64, p: f64) -> f64 {
    fn split(x: f64) -> (f64, f64) {
        let c = 134_217_729.0 * x; // 2^27 + 1
        let hi = c - (c - x);
        (hi, x - hi)
    }
    if !a.is_finite() || a.abs() > 1e300 {
        return 0.0;
    }
    let (ah, al) = split(a);
    let (bh, bl) = split(b);
    ((ah * bh - p) + ah * bl + al * bh) + al * bl
}

/// log2(x) for `fyl2x` (x <= 0 handled by the caller's invalid/zero-divide logic via the returned value:
/// log2(0) = -inf, log2(<0) = NaN which the caller turns into indefinite).
pub fn log2(x: f64) -> f64 {
    if x.is_nan() {
        return quiet(x);
    }
    if x < 0.0 {
        return INDEFINITE;
    }
    libm::log2(x)
}

/// log2(1 + x) for `fyl2xp1`.
pub fn log2_1p(x: f64) -> f64 {
    if x.is_nan() {
        return quiet(x);
    }
    if x < -1.0 {
        return INDEFINITE;
    }
    if x == 0.0 {
        return x;
    }
    // log1p(x) / ln2: two roundings, deterministic.
    libm::log1p(x) / std::f64::consts::LN_2
}

/// `fsqrt` core: IEEE sqrt is correctly rounded on all hosts; only the NaN result needs canonicalising.
pub fn sqrt(x: f64) -> f64 {
    if x.is_nan() {
        return quiet(x);
    }
    if x < 0.0 {
        return INDEFINITE;
    }
    x.sqrt()
}

// ------------------------------------------------------------------------------------------------ remainder

/// Decompose a finite non-zero f64 into (integer significand < 2^53, exponent) with |x| = m * 2^e.
fn decompose(x: f64) -> (u64, i32) {
    let bits = x.to_bits();
    let e = ((bits >> 52) & 0x7FF) as i32;
    let f = bits & ((1u64 << 52) - 1);
    if e == 0 {
        (f, -1074)
    } else {
        (f | (1u64 << 52), e - 1075)
    }
}

/// `m * 2^e` for m < 2^53 and a result known to be representable exactly (used for remainders: they are
/// multiples of 2^e(b) and smaller than |b|).
fn compose(m: u64, e: i32) -> f64 {
    if m == 0 {
        return 0.0;
    }
    // Normalise m to 53 bits, then build the bits directly (handles subnormal results too).
    let lz = m.leading_zeros() as i32 - 11; // shift that puts the MSB at bit 52
    let (mut mm, mut ee) = (m << lz, e - lz);
    // value = mm * 2^ee, mm in [2^52, 2^53)
    let biased = ee + 1075;
    if biased >= 1 {
        f64::from_bits(((biased as u64) << 52) | (mm & ((1u64 << 52) - 1)))
    } else {
        // Subnormal: shift right; exact by construction (the low bits are zero).
        let s = (1 - biased) as u32;
        mm >>= s.min(63);
        ee += s as i32;
        let _ = ee;
        f64::from_bits(mm)
    }
}

/// Result of [`fprem`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rem {
    /// Remainder (sign of the dividend; zero keeps the dividend's sign).
    pub r: f64,
    /// Low 3 bits of the integer quotient |a/b| (for C0/C3/C1).
    pub q: u8,
}

/// Exact `fprem` (truncating quotient, `ieee = false`) or `fprem1` (round-to-nearest-even quotient,
/// `ieee = true`) for finite `a` and finite non-zero `b`. Always the complete remainder (C2 = 0): real hardware
/// may need several iterations for exponent differences >= 64, which the CRT loops over anyway.
pub fn fprem(a: f64, b: f64, ieee: bool) -> Rem {
    debug_assert!(a.is_finite() && b.is_finite() && b != 0.0);
    if a == 0.0 {
        return Rem { r: a, q: 0 };
    }
    let (ma, ea) = decompose(a);
    let (mb, eb) = decompose(b);
    // |a| = ma*2^ea, |b| = mb*2^eb. Long division of ma*2^(ea-eb) by mb, keeping the remainder and q mod 8.
    let (mut r, mut q): (u64, u64);
    if ea >= eb {
        r = ma % mb;
        q = ma / mb;
        let mut d = ea - eb;
        // Shift in chunks of up to 10 bits: r < mb < 2^53, so r << 10 < 2^63 fits.
        while d > 0 {
            let k = d.min(10) as u32;
            let x = r << k;
            q = (q << k) | (x / mb); // only the low bits of q matter (wrapping shift is fine)
            r = x % mb;
            d -= k as i32;
        }
        // remainder value = r * 2^eb
    } else {
        // ea < eb only happens for a normal b (eb > -1074, mb >= 2^52); then |a| < 2^53 * 2^ea <= 2^52 * 2^eb
        // <= |b|, so the truncated quotient is 0 and the remainder is a itself.
        if !ieee {
            return Rem { r: a, q: 0 };
        }
        // fprem1 still needs the rounding step: compare |a| with |b|/2 exactly at exponent ea.
        let d = (eb - ea) as u32;
        if d >= 2 {
            // |a| < 2^53*2^ea <= 2^51*2^eb <= |b|/2: no rounding.
            return Rem { r: a, q: 0 };
        }
        // d == 1: |b| = (2*mb) * 2^ea, 2*mb < 2^54 fits in u64.
        return finish_rem(a, b, ma, ea, 0, ieee, Some((mb << 1, ea)));
    }
    finish_rem(a, b, r, eb, q, ieee, Some((mb, eb)))
}

/// Shared tail of [`fprem`]: r (at exponent `e`) is the truncating remainder, q the truncated quotient.
/// `div` = (divisor significand, exponent) at the same exponent `e`.
fn finish_rem(a: f64, _b: f64, mut r: u64, e: i32, mut q: u64, ieee: bool, div: Option<(u64, i32)>) -> Rem {
    let neg = a.is_sign_negative();
    let mut flip = false;
    if ieee {
        if let Some((mb, _)) = div {
            // Round the quotient to nearest even: if 2r > mb, or 2r == mb and q odd -> q+1, r = mb - r (sign flips).
            let two_r = (r as u128) * 2;
            let mb128 = mb as u128;
            if two_r > mb128 || (two_r == mb128 && q & 1 == 1) {
                q = q.wrapping_add(1);
                r = mb - r;
                flip = true;
            }
        }
    }
    let mag = compose(r, e);
    let rv = if r == 0 {
        0.0f64.copysign(a)
    } else if neg != flip {
        -mag
    } else {
        mag
    };
    Rem { r: rv, q: (q & 7) as u8 }
}

// ------------------------------------------------------------------------------------- host environment check

/// Verify that the host FPU is in the IEEE mode determinism relies on: subnormals are neither flushed to zero
/// (FTZ) nor treated as zero on input (DAZ), round-to-nearest is active, and basic ops are correctly rounded.
/// Some Android audio/graphics libraries are known to set FTZ on their own threads (HYPOTHESIS for ours); call
/// this on the emulation thread at startup. Values go through `black_box` so the checks run at runtime.
pub fn host_float_env_check() -> Result<(), &'static str> {
    use std::hint::black_box as bb;
    let tiny = bb(f64::from_bits(1)); // smallest subnormal
    if bb(tiny * 1.0) != tiny || bb(tiny + tiny).to_bits() != 2 {
        return Err("subnormal results flushed to zero (FTZ)");
    }
    if bb(f64::MIN_POSITIVE) / bb(2.0) == 0.0 {
        return Err("subnormal results flushed to zero (FTZ)");
    }
    if bb(tiny) == 0.0 {
        return Err("subnormal inputs treated as zero (DAZ)");
    }
    // Round to nearest even: 1 + 2^-53 -> 1, 1 + 3*2^-53 -> 1 + 2^-51.
    let e53 = f64::from_bits(0x3CA0_0000_0000_0000);
    if bb(1.0f64) + bb(e53) != 1.0 || bb(1.0f64) + bb(3.0 * e53) != 1.0 + 4.0 * e53 {
        return Err("rounding mode is not round-to-nearest-even");
    }
    if bb(-1.0f64) - bb(e53) != -1.0 {
        return Err("rounding mode is not round-to-nearest-even");
    }
    // Correctly rounded division and sqrt.
    if bb(1.0f64) / bb(3.0) != f64::from_bits(0x3FD5_5555_5555_5555) || bb(2.0f64).sqrt() != std::f64::consts::SQRT_2 {
        return Err("division/sqrt not correctly rounded");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host_fmod(a: f64, b: f64) -> f64 {
        a % b
    }

    #[test]
    fn fprem_matches_exact_fmod() {
        let vals = [
            1.0, -1.0, 3.5, -7.25, 1e10, -1e-10, 123456789.123, 2.0f64.powi(60), 5e-324, 1e-310, 1e300, -1e308,
            std::f64::consts::PI, 0.1, 6.0, 7.0,
        ];
        for &a in &vals {
            for &b in &vals {
                let r = fprem(a, b, false);
                // fmod is exact in every conforming libm; compare against it here (test only).
                assert_eq!(r.r.to_bits(), host_fmod(a, b).to_bits(), "fprem({a:e}, {b:e})");
            }
        }
        // Quotient bits: 7 / 2 = 3 rem 1 -> q = 3
        assert_eq!(fprem(7.0, 2.0, false), Rem { r: 1.0, q: 3 });
        assert_eq!(fprem(-7.0, 2.0, false), Rem { r: -1.0, q: 3 });
        assert_eq!(fprem(13.0, 1.0, false), Rem { r: 0.0, q: 5 });
        assert_eq!(fprem(1.0, 2.0, false), Rem { r: 1.0, q: 0 });
        // Large quotient: 2^60 / 3 -> q mod 8 exact.
        let q = (1u64 << 60) / 3;
        assert_eq!(fprem(2.0f64.powi(60), 3.0, false).q, (q & 7) as u8);
    }

    #[test]
    fn fprem1_is_ieee_remainder() {
        assert_eq!(fprem(7.0, 2.0, true), Rem { r: -1.0, q: 4 });
        assert_eq!(fprem(5.0, 2.0, true), Rem { r: 1.0, q: 2 }); // 2.5 -> 2 (even)
        assert_eq!(fprem(3.0, 2.0, true), Rem { r: -1.0, q: 2 }); // 1.5 -> 2
        assert_eq!(fprem(1.0, 4.0, true), Rem { r: 1.0, q: 0 });
        assert_eq!(fprem(3.0, 4.0, true), Rem { r: -1.0, q: 1 });
        for &(a, b) in &[(10.3, 3.0), (-10.3, 3.0), (1e20, 7.0), (5.5, -2.0)] {
            assert_eq!(fprem(a, b, true).r, libm::remainder(a, b), "{a} {b}");
        }
    }

    #[test]
    fn nan_rules_are_host_independent() {
        let qa = f64::from_bits(0x7FF8_0000_0000_0005);
        let qb = f64::from_bits(0xFFF8_0000_0000_0009);
        let sa = f64::from_bits(0x7FF0_0000_0000_0007);
        assert_eq!(propagate_nan(qa, 1.0).to_bits(), qa.to_bits());
        assert_eq!(propagate_nan(1.0, qb).to_bits(), qb.to_bits());
        assert_eq!(propagate_nan(qa, qb).to_bits(), qb.to_bits()); // larger significand
        assert_eq!(propagate_nan(qb, qa).to_bits(), qb.to_bits());
        assert_eq!(propagate_nan(sa, 1.0).to_bits(), 0x7FF8_0000_0000_0007);
        assert_eq!(sqrt(-1.0).to_bits(), INDEFINITE.to_bits());
        assert_eq!(sin(f64::INFINITY).to_bits(), INDEFINITE.to_bits());
        assert_eq!(log2(-1.0).to_bits(), INDEFINITE.to_bits());
        assert_eq!(sin(qa).to_bits(), qa.to_bits());
    }

    #[test]
    fn host_env_is_ieee() {
        host_float_env_check().unwrap();
    }

    #[test]
    fn accuracy_against_known_values() {
        // fdlibm is < 1 ulp; these are exact or correctly rounded reference values.
        assert_eq!(sin(0.0).to_bits(), 0);
        assert_eq!(cos(0.0), 1.0);
        assert_eq!(atan2(1.0, 1.0), std::f64::consts::FRAC_PI_4);
        assert_eq!(log2(8.0), 3.0);
        assert_eq!(exp2m1(1.0), 1.0);
        assert_eq!(exp2m1(-1.0), -0.5);
        assert_eq!(log2_1p(1.0), 1.0);
        let ulp = |a: f64, b: f64| (a.to_bits() as i64 - b.to_bits() as i64).abs();
        // The f64 constants are rounded, so the exact results are themselves up to ~1 ulp away from 0.5.
        assert!(ulp(sin(std::f64::consts::FRAC_PI_6), 0.5) <= 2);
        assert!(ulp(cos(std::f64::consts::FRAC_PI_3), 0.5) <= 2);
        assert!(ulp(tan(std::f64::consts::FRAC_PI_4), 1.0) <= 1);
        // sqrt(2) - 1 correctly rounded (the f64 expression SQRT_2 - 1.0 is 2 ulp off).
        assert!(ulp(exp2m1(0.5), f64::from_bits(0x3FDA_8279_99FC_EF32)) <= 1);
        assert!(ulp(exp2m1(-0.25), f64::from_bits(0xBFC4_5D81_9A94_B14B)) <= 1);
    }
}
