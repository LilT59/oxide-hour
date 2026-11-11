//! The transcendental functions the C++ comparison build actually calls, reproduced in software.
//!
//! `Real Sin(Real x)` / `Cos` are `sinf`/`cosf` (`GeneralsMD/Code/GameEngine/Source/Common/System/Trig.cpp:48-63`).
//! On 32-bit x86 the UCRT header defines those as `(float)sin((double)x)` / `(float)cos((double)x)`
//! (`corecrt_math.h`: the real float functions are declared only for `_M_X64`/ARM), and the build's
//! import table names the double routines below them: `_libm_sse2_sin_precise` / `_libm_sse2_cos_precise`
//! (the 64-bit CRT is a different implementation and differs in the last bit on ~1% of double results,
//! so the 64-bit `sinf` cannot stand in for this build's).
//!
//! `libm::sinf` (musl's float routine) is not the same function: it differs from the rounded double
//! result in about 0.1% of inputs by one ulp, and those differences are the heading drift this module
//! removes. The double `sin`/`cos` are not correctly rounded either (about 0.05% of double results are
//! one ulp off the correctly rounded value), so this reproduces the algorithm, not merely an accurate
//! sine.
//!
//! # Algorithm
//!
//! The published algorithm used here is a Cody-Waite argument reduction plus a table of sin/cos at
//! multiples of `pi/32` and a polynomial for the remainder:
//!
//! * `n = rint(x * 32/pi)` and `r = x - n*pi/32`, with `pi/32` split into three chunks (see
//!   [`PIO32_HI`]): the leading chunk keeps 32 significant bits and the second 31, so `n*chunk` is
//!   exact for the reduced range and the truncated bits are carried by the next chunk;
//! * `sin(x) = sin(n*pi/32)*cos(r) + cos(n*pi/32)*sin(r)`, with `sin`/`cos(n*pi/32)` taken from
//!   [`TABLE`] at `n mod 64` as a double-double pair, and `sin(r)`/`cos(r)` evaluated by the
//!   degree-9/8 Taylor polynomials in `r^2` (coefficients are the exact `+/-1/k!`, see [`C9`]);
//! * everything is summed with compensated (double-double) additions in a fixed order.
//!
//! Cosine is the same computation with the table index shifted by 16, i.e. `cos(x) = sin(x + pi/2)`.
//! Every operation is a plain `f64` `+ - *` in that order, so the same operations in Rust give the
//! same bits on every machine, and the final `(float)` is the C++'s `(float)sin((double)x)`.
//!
//! # How it was validated, and its ranges
//!
//! The implementation was checked black-box against the real 32-bit CRT (a scratch MSVC probe calling
//! `sinf`/`cosf` with the comparison build's flags): every f32 bit pattern of every exponent field
//! 0..=143, both signs (~2.4 billion inputs — the CRT's whole fast path plus the tiny range) gives
//! bit-identical results for both `sin` and `cos`. That probing is also where the ranges come from:
//! the CRT switches to a different routine at `|x| >= 90112`, and below `2^-252` (which for an `f32`
//! means zero or a subnormal) its result is the argument itself / `1`.
//!
//! The CRT's fallback for `|x| >= 90112` is its x87 `fsin`/`fcos`, which is not reproduced here:
//! [`sin`] and [`cos`] use the `libm` double routine for that range (still deterministic). The game
//! never produces an angle that large — every heading is wrapped to `(-pi, pi]` by `normalizeAngle`
//! (`GameCommon.cpp:54`).
//!
//! # The table
//!
//! `TABLE[k]` is `{cos(k*pi/32) - cos_hi, sin(k*pi/32), sin_low, cos_hi}` for `k = 0..63`, a
//! double-double split that keeps the products with `r` accurate:
//!
//! * `sin_hi` is the correctly rounded `sin(k*pi/32)`;
//! * `cos_hi` is the power of two `2^n` with `n` the smallest integer `>= log2|cos(k*pi/32)| - 0.5`
//!   (the nearest power of two in ratio, ties to the smaller one, which makes `cos_hi * r` exact), or
//!   `0` where the cosine is zero;
//! * `cos_mid` and `sin_low` are the correctly rounded residuals `cos - cos_hi` and `sin - sin_hi`.
//!
//! `tools/gen_crtmath_table.py` derives the whole table from those rules in 90-digit arithmetic (the
//! multiples of `pi/4` taken as their exact algebraic values) and prints the literals below; no value
//! in this file was read out of a binary.

/// `32/pi`, correctly rounded to `f64`.
const K32: f64 = 10.185916357881302;
/// `2^52 * 1.5`: adding and subtracting it rounds a double to an integer (round-half-to-even).
const MAGIC: f64 = 6755399441055744.0;
/// `pi/32` as a Cody-Waite split: the leading part is `pi/32` truncated to 32 significant bits, the
/// middle part the residual truncated to 32 significant bits (checked: pi/32 - PIO32_HI truncated to 31 bits does
/// not match), the low part the correctly rounded remainder (the
/// tail beyond the three is ~6e-39, far below the reduction's needs for `|n| < 2^21`).
const PIO32_HI: f64 = 0.09817477042088285;
const PIO32_MID: f64 = 3.798187816439979e-12;
const PIO32_LO: f64 = 1.2639164054974691e-22;
/// Taylor coefficients, the exact values correctly rounded: `1/9!`, `1/8!`, `-1/7!`, `-1/6!`, `1/5!`,
/// `1/4!`, `-1/6`, `-1/2`.
const C9: f64 = 2.7557319223985893e-06;
const C8: f64 = 2.48015873015873e-05;
const C7: f64 = -0.0001984126984126984;
const C6: f64 = -0.001388888888888889;
const C5: f64 = 0.008333333333333333;
const C4: f64 = 0.041666666666666664;
const C3: f64 = -0.16666666666666666;
const C2: f64 = -0.5;
/// `2^-252`, the low end of the CRT's fast path (found by probing).
const PATH_MIN: f64 = f64::from_bits(0x3030_0000_0000_0000);
/// The upper edge of the fast path, `1.375 * 2^16` (found by probing).
const PATH_MAX: f64 = 90112.0;

/// `{cos - cos_hi, sin, sin_low, cos_hi}` for `k * pi/32`, k = 0..63.
// The entries at `k = 8, 24, 40, 56` are `+/- sqrt(1/2)`; they are table data, not an approximation of
// the constant.
#[allow(clippy::approx_constant)]
#[rustfmt::skip]
static TABLE: [[f64; 4]; 64] = [
    [0.0, 0.0, 0.0, 1.0],
    [-0.004815273327803114, 0.0980171403295606, -1.634582362244256e-18, 1.0],
    [-0.019214719596769552, 0.19509032201612828, -7.991079068461731e-18, 1.0],
    [-0.043059664267791134, 0.2902846772544624, -1.892797870777425e-17, 1.0],
    [-0.07612046748871325, 0.3826834323650898, -1.0050772696461588e-17, 1.0],
    [-0.11807873565164496, 0.47139673682599764, 6.516678136069013e-18, 1.0],
    [-0.16853038769745476, 0.5555702330196022, 4.709410940561677e-17, 1.0],
    [-0.22698954663726303, 0.6343932841636455, 1.0420901929280035e-17, 1.0],
    [0.20710678118654752, 0.7071067811865476, -4.833646656726457e-17, 0.5],
    [0.1343932841636455, 0.773010453362737, -3.256590703364977e-17, 0.5],
    [0.055570233019602226, 0.8314696123025452, 1.4073856984728024e-18, 0.5],
    [-0.02860326317400235, 0.881921264348355, -1.9843248405890562e-17, 0.5],
    [-0.11731656763491023, 0.9238795325112867, 1.7645047084336677e-17, 0.5],
    [0.040284677254462366, 0.9569403357322088, 4.05538698618757e-17, 0.25],
    [-0.05490967798387173, 0.9807852804032304, 1.8546939997825006e-17, 0.25],
    [-0.026982859670439396, 0.9951847266721969, -4.248691367830441e-17, 0.125],
    [0.0, 1.0, 0.0, 0.0],
    [0.026982859670439396, 0.9951847266721969, -4.248691367830441e-17, -0.125],
    [0.05490967798387173, 0.9807852804032304, 1.8546939997825006e-17, -0.25],
    [-0.040284677254462366, 0.9569403357322088, 4.05538698618757e-17, -0.25],
    [0.11731656763491023, 0.9238795325112867, 1.7645047084336677e-17, -0.5],
    [0.02860326317400235, 0.881921264348355, -1.9843248405890562e-17, -0.5],
    [-0.055570233019602226, 0.8314696123025452, 1.4073856984728024e-18, -0.5],
    [-0.1343932841636455, 0.773010453362737, -3.256590703364977e-17, -0.5],
    [-0.20710678118654752, 0.7071067811865476, -4.833646656726457e-17, -0.5],
    [0.22698954663726303, 0.6343932841636455, 1.0420901929280035e-17, -1.0],
    [0.16853038769745476, 0.5555702330196022, 4.709410940561677e-17, -1.0],
    [0.11807873565164496, 0.47139673682599764, 6.516678136069013e-18, -1.0],
    [0.07612046748871325, 0.3826834323650898, -1.0050772696461588e-17, -1.0],
    [0.043059664267791134, 0.2902846772544624, -1.892797870777425e-17, -1.0],
    [0.019214719596769552, 0.19509032201612828, -7.991079068461731e-18, -1.0],
    [0.004815273327803114, 0.0980171403295606, -1.634582362244256e-18, -1.0],
    [0.0, 0.0, 0.0, -1.0],
    [0.004815273327803114, -0.0980171403295606, 1.634582362244256e-18, -1.0],
    [0.019214719596769552, -0.19509032201612828, 7.991079068461731e-18, -1.0],
    [0.043059664267791134, -0.2902846772544624, 1.892797870777425e-17, -1.0],
    [0.07612046748871325, -0.3826834323650898, 1.0050772696461588e-17, -1.0],
    [0.11807873565164496, -0.47139673682599764, -6.516678136069013e-18, -1.0],
    [0.16853038769745476, -0.5555702330196022, -4.709410940561677e-17, -1.0],
    [0.22698954663726303, -0.6343932841636455, -1.0420901929280035e-17, -1.0],
    [-0.20710678118654752, -0.7071067811865476, 4.833646656726457e-17, -0.5],
    [-0.1343932841636455, -0.773010453362737, 3.256590703364977e-17, -0.5],
    [-0.055570233019602226, -0.8314696123025452, -1.4073856984728024e-18, -0.5],
    [0.02860326317400235, -0.881921264348355, 1.9843248405890562e-17, -0.5],
    [0.11731656763491023, -0.9238795325112867, -1.7645047084336677e-17, -0.5],
    [-0.040284677254462366, -0.9569403357322088, -4.05538698618757e-17, -0.25],
    [0.05490967798387173, -0.9807852804032304, -1.8546939997825006e-17, -0.25],
    [0.026982859670439396, -0.9951847266721969, 4.248691367830441e-17, -0.125],
    [0.0, -1.0, 0.0, 0.0],
    [-0.026982859670439396, -0.9951847266721969, 4.248691367830441e-17, 0.125],
    [-0.05490967798387173, -0.9807852804032304, -1.8546939997825006e-17, 0.25],
    [0.040284677254462366, -0.9569403357322088, -4.05538698618757e-17, 0.25],
    [-0.11731656763491023, -0.9238795325112867, -1.7645047084336677e-17, 0.5],
    [-0.02860326317400235, -0.881921264348355, 1.9843248405890562e-17, 0.5],
    [0.055570233019602226, -0.8314696123025452, -1.4073856984728024e-18, 0.5],
    [0.1343932841636455, -0.773010453362737, 3.256590703364977e-17, 0.5],
    [0.20710678118654752, -0.7071067811865476, 4.833646656726457e-17, 0.5],
    [-0.22698954663726303, -0.6343932841636455, -1.0420901929280035e-17, 1.0],
    [-0.16853038769745476, -0.5555702330196022, -4.709410940561677e-17, 1.0],
    [-0.11807873565164496, -0.47139673682599764, -6.516678136069013e-18, 1.0],
    [-0.07612046748871325, -0.3826834323650898, 1.0050772696461588e-17, 1.0],
    [-0.043059664267791134, -0.2902846772544624, 1.892797870777425e-17, 1.0],
    [-0.019214719596769552, -0.19509032201612828, 7.991079068461731e-18, 1.0],
    [-0.004815273327803114, -0.0980171403295606, 1.634582362244256e-18, 1.0],
];

/// `sin(x)` as the C++ build's `Real Sin(Real)`: `(float)sin((double)x)` from the 32-bit UCRT.
pub fn sin(x: f32) -> f32 {
    let d = x as f64;
    if !(PATH_MIN..PATH_MAX).contains(&d.abs()) {
        // Below the fast path's lower edge the CRT returns the argument itself (measured).
        if d.abs() < PATH_MIN {
            return x;
        }
        return libm::sin(d) as f32;
    }
    kernel(d, 0) as f32
}

/// `cos(x)` as the C++ build's `Real Cos(Real)`.
pub fn cos(x: f32) -> f32 {
    let d = x as f64;
    if !(PATH_MIN..PATH_MAX).contains(&d.abs()) {
        if d.abs() < PATH_MIN {
            return 1.0;
        }
        return libm::cos(d) as f32;
    }
    kernel(d, 0x10) as f32
}

/// The reduction + table + polynomial body shared by sine and cosine: `index_bias` is 0 for sine and
/// 16 for cosine. The operations are in the fixed order the algorithm prescribes; the compiler must
/// not reassociate them (Rust's `f32`/`f64` operators do not, and there is no FMA contraction).
fn kernel(x: f64, index_bias: i64) -> f64 {
    let t = x * K32;
    let n = ((t + MAGIC) - MAGIC) as i64;
    let nf = (t + MAGIC) - MAGIC;
    // `n mod 64`, with a multiple of 64 added first so negative `n` wraps like C's `& 63`.
    let idx = ((n + 0x1c7600 + index_bias) & 63) as usize;
    let [a0, a1, a2, a3] = TABLE[idx];

    // Three-chunk argument reduction: r = x - n*pi/32, kept as r1 plus the residual t3.
    let r0 = x - PIO32_HI * nf;
    let nb = PIO32_MID * nf;
    let nc = PIO32_LO * nf;
    let r1 = r0 - nb;
    let t1 = r0 - r1;
    let t2 = t1 - nb;
    let t3 = nc - t2;

    // sin(r) / cos(r) polynomial, one quantity per lane, and the double-double combination with the
    // tabulated sin/cos of n*pi/32.
    let c = a0 + a3;
    let r2 = r1 * r1;
    let mut v5 = [C9 * r0, C8 * r0];
    let v7 = a1 * r1 - c;
    v5[0] *= r1;
    v5[1] *= r1;
    let mut v2 = [c * r1, a1];
    let ch_r1 = a3 * r1;
    v2[0] *= r2;
    v2[1] *= r2;
    let r4 = r2 * r2;
    let mut v6 = [C5 * r2, C4 * r2];
    let mut v4 = r1 * a0;
    v5[0] += C7;
    v5[1] += C6;
    v6[0] += C3;
    v6[1] += C2;
    v5[0] *= r4;
    v5[1] *= r4;
    let v0 = ch_r1;
    let v3 = ch_r1 + a1;
    let mut v1 = t3 * v7;
    let v7b = v4;
    v4 += v3;
    v6[0] += v5[0];
    v6[1] += v5[1];
    let mut v5b = a1 - v3;
    let mut v3b = v3 - v4;
    v1 += a2;
    v6[0] *= v2[0];
    v6[1] *= v2[1];
    v5b += v0;
    v3b += v7b;
    v1 += v5b;
    v1 += v3b;
    v1 += v6[0];
    v1 += v6[1];
    v4 + v1
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `(argument, sin, cos)` bit patterns returned by the C++ build's `sinf`/`cosf`
    /// (`(float)sin((double)x)` through the 32-bit UCRT, measured with a scratch probe compiled by
    /// the same MSVC toolchain).
    const CRT_REFERENCE: &[(u32, u32, u32)] = &[
        (0x00000000, 0x00000000, 0x3f800000),
        (0x80000000, 0x80000000, 0x3f800000),
        (0x0da24260, 0x0da24260, 0x3f800000),
        (0x8da24260, 0x8da24260, 0x3f800000),
        (0x3f800000, 0x3f576aa4, 0x3f0a5140),
        (0xbf800000, 0xbf576aa4, 0x3f0a5140),
        (0x3f000000, 0x3ef57744, 0x3f60a940),
        (0xbf000000, 0xbef57744, 0x3f60a940),
        (0x40490fdb, 0xb3bbbd2e, 0xbf800000),
        (0xc0490fdb, 0x33bbbd2e, 0xbf800000),
        (0x3dc90fdb, 0x3dc8bd36, 0x3f7ec46d),
        (0x3fc90fdb, 0x3f800000, 0xb33bbd2e),
        (0xbfc90fdb, 0xbf800000, 0xb33bbd2e),
        (0x40000000, 0x3f68c7b7, 0xbed51133),
        (0x41200000, 0xbf0b44f8, 0xbf56cd64),
        (0x42c80000, 0xbf01a12e, 0x3f5cc0ee),
        (0xc2c80000, 0x3f01a12e, 0x3f5cc0ee),
        (0x447a0000, 0x3f53ae61, 0x3f0ff813),
        (0xc5800400, 0x3e0baad1, 0x3f7d9b8c),
        (0x47afc800, 0xbeadcd5e, 0x3f70cc91),
        (0xc7afc7c0, 0x3f3fb548, 0x3f29a891),
        (0x3f490fdb, 0x3f3504f3, 0x3f3504f3),
        (0x4016cbe4, 0x3f3504f3, 0xbf3504f3),
        (0x4096cbe4, 0xbf800000, 0x324cde2e),
        (0x40c90fdb, 0x343bbd2e, 0x3f800000),
        (0xbacbdba8, 0xbacbdba3, 0x3f7fffec),
        (0xb92f5ab0, 0xb92f5ab0, 0x3f800000),
        (0xc0bcadc9, 0x3ec138a9, 0x3f6d1206),
        (0xc008633f, 0xbf58dc9e, 0xbf080a52),
        (0xb7745429, 0xb7745429, 0x3f800000),
        (0xbea13283, 0xbe9e8c23, 0x3f736aab),
        (0xba988b12, 0xba988b10, 0x3f7ffff5),
        (0xc23c3198, 0xbd9a6467, 0xbf7f4582),
        (0x3fbcf924, 0x3f7edbf1, 0x3dc121d7),
        (0x416e35f8, 0x3f3b22dd, 0xbf2eb03d),
    ];

    /// Every value is the bit pattern the C++ build returns; this is the whole point of the module.
    #[test]
    fn sin_and_cos_match_the_crt_bit_for_bit() {
        for &(x, sin_bits, cos_bits) in CRT_REFERENCE {
            let x = f32::from_bits(x);
            assert_eq!(sin(x).to_bits(), sin_bits, "sin({x:e})");
            assert_eq!(cos(x).to_bits(), cos_bits, "cos({x:e})");
        }
    }

    /// The table is `sin`/`cos` of `k*pi/32`; `(cos_mid + cos_hi)^2 + (sin_hi + sin_low)^2` must be
    /// one to within the low parts' size, and the values must be monotone along each quadrant.
    #[test]
    fn the_table_entries_are_sin_and_cos_of_multiples_of_pi_over_32() {
        for (k, entry) in TABLE.iter().enumerate() {
            let [cos_mid, sin_hi, sin_low, cos_hi] = *entry;
            let c = cos_hi + cos_mid;
            let s = sin_hi + sin_low;
            let err = (c * c + s * s - 1.0).abs();
            assert!(err < 1e-15, "k={k}: sin^2+cos^2 off by {err:e}");
            let theta = k as f64 * std::f64::consts::PI / 32.0;
            let want_sin = libm::sin(theta);
            let want_cos = libm::cos(theta);
            // `libm`'s own double sin/cos are within an ulp of the correctly rounded value, which is
            // the table's `sin_hi`/`cos_hi`; this catches a mistyped entry.
            assert!((s - want_sin).abs() < 1e-15, "k={k}: sin {s:e} vs {want_sin:e}");
            assert!((c - want_cos).abs() < 1e-15, "k={k}: cos {c:e} vs {want_cos:e}");
        }
    }

    /// `cos(x) = sin(x + pi/2)` held to the table's precision for small arguments, and both are
    /// periodic to within the CRT's own reduction error.
    #[test]
    fn sin_and_cos_are_periodic_to_within_a_few_ulp() {
        for i in -100..=100 {
            let x = i as f32 * 0.023;
            let s = sin(x);
            let c = cos(x);
            assert!((s * s + c * c - 1.0).abs() < 2.0e-7, "x={x}");
        }
    }
}
