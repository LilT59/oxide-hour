//! The engine's own float helpers, bit for bit (lane 194, wave 23).
//!
//! The retail build is 32-bit x86 MSVC and runs `GameLogic::setFPMode()` (GameLogic.cpp:196-215), which sets the x87
//! precision control to 24 bits with round-to-nearest. Every x87 operation then rounds its result to an IEEE f32, so plain
//! f32 arithmetic in the same order is the engine's arithmetic (the exponent range is wider, irrelevant for these values).
//!
//! [`inv_sqrt`] is `WWMath::Inv_Sqrt` for `_MSC_VER && _M_IX86` (Core/Libraries/Source/WWVegas/WWMath/wwmath.h:614-660), an
//! inline-asm integer guess and three Newton steps, not `1/sqrt`. [`vec3_normalize`] is `Vector3::Normalize`
//! (vector3.h:417-426), and [`z_rotation_transform`] is `Matrix3D::Transform_Vector` (matrix3d.h:1741-1756) applied to the
//! matrix that `Thing::setOrientation` builds for an angle (Thing.cpp:204-245: a Z rotation with the position as the
//! translation).

/// `WWMath::Inv_Sqrt` (wwmath.h:614-660, `_MSC_VER && _M_IX86`). The asm, register by register:
///
/// ```text
/// eax = 0xbe6eb508 - bits(a)       ; mov eax, ..; sub eax, [a]
/// [a] = bits(a) - 0x800000         ; sub [a], 800000h   -> y0 = a / 2 (the exponent drops by one)
/// r0  = bits(eax >> 1)             ; shr eax, 1; [esp-8] = eax, loaded as the float r
/// y1  = (r * r) * y0               ; fmul st,st; fmul [a]
/// r1  = 1.5 - y1                   ; fsub st, st(2)
/// y2  = (y1 * r1) * r1             ; fmul st(3), st; fmul st(3), st
/// x2  = r0 * r1                    ; fmulp st(4), st
/// r2  = 1.5 - y2                   ; fsub st, st(2)
/// y3  = (y2 * r2) * r2             ; fmul st(2), st; fmulp st(2), st
/// x3  = x2 * r2                    ; fmul st(3), st
/// r3  = 1.5 - y3                   ; fsubp st(1), st
/// result = x3 * r3                 ; fmulp st(1), st; fstp retval
/// ```
///
/// Every step is one f32 operation (PC_24). The result is within one ulp or so of `1/sqrt(a)` but not the same function:
/// `inv_sqrt(4.0)` is `0x3effffff` (0.49999997), where `1.0 / 2.0` is `0.5`. Only the normal, positive domain the engine
/// passes (squared lengths) is meant; the asm's special cases are not modelled.
pub fn inv_sqrt(a: f32) -> f32 {
    let bits = a.to_bits();
    let r0 = f32::from_bits(0xbe6e_b508u32.wrapping_sub(bits) >> 1);
    let y0 = f32::from_bits(bits.wrapping_sub(0x0080_0000));
    let y1 = (r0 * r0) * y0;
    let r1 = 1.5 - y1;
    let y2 = (y1 * r1) * r1;
    let x2 = r0 * r1;
    let r2 = 1.5 - y2;
    let y3 = (y2 * r2) * r2;
    let x3 = x2 * r2;
    let r3 = 1.5 - y3;
    x3 * r3
}

/// `Vector3::Length2` (vector3.h:469-472): `X*X + Y*Y + Z*Z`, in that order.
pub fn length2(v: (f32, f32, f32)) -> f32 {
    v.0 * v.0 + v.1 * v.1 + v.2 * v.2
}

/// `Vector3::Normalize` (vector3.h:417-426): when the squared length is not zero, each component is multiplied by
/// [`inv_sqrt`] of it. A zero vector is returned unchanged.
pub fn vec3_normalize(v: (f32, f32, f32)) -> (f32, f32, f32) {
    let len2 = length2(v);
    if len2 != 0.0 {
        let oolen = inv_sqrt(len2);
        (v.0 * oolen, v.1 * oolen, v.2 * oolen)
    } else {
        v
    }
}

/// `Vector3::Normalized_Cross_Product` (vector3.h:382-389): `a x b` in the engine's order, then `Vector3::Normalize`, i.e.
/// multiplied by `Inv_Sqrt`, not divided by `sqrt`. No cut-off: a parallel pair gives a zero cross, which `Normalize` leaves zero
/// (vector3.h:417-426), and the callers use it as C++ does (Locomotor.cpp:159 and NeutronMissileUpdate.cpp:317 take it unguarded).
pub fn normalized_cross(a: (f32, f32, f32), b: (f32, f32, f32)) -> (f32, f32, f32) {
    let c = (a.1 * b.2 - a.2 * b.1, a.2 * b.0 - a.0 * b.2, a.0 * b.1 - a.1 * b.0);
    vec3_normalize(c)
}

/// `Matrix3D::Transform_Vector` (matrix3d.h:1741-1756) of the matrix `Thing::setOrientation(angle)` builds at position
/// `origin` (Thing.cpp:204-245, the `m_transform.Set(x, y, z, pos)` with `u = (cos, sin, 0)`, `z = (0, 0, 1)`). That matrix is
/// `[c -s 0 ox; s c 0 oy; 0 0 1 oz]`, and each output is `a*x + b*y + c*z + t`, summed left to right with the translation last.
///
/// `c` and `s` must be the engine's `Cos(angle)` and `Sin(angle)` (`Trig.cpp`, the same functions as [`crate::math::cos`]).
pub fn z_rotation_transform(origin: (f32, f32, f32), c: f32, s: f32, p: (f32, f32, f32)) -> (f32, f32, f32) {
    let x = c * p.0 + (-s) * p.1 + 0.0 * p.2 + origin.0;
    let y = s * p.0 + c * p.1 + 0.0 * p.2 + origin.1;
    let z = 0.0 * p.0 + 0.0 * p.1 + 1.0 * p.2 + origin.2;
    (x, y, z)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pinned bit patterns of the asm dataflow above (each step one f32 operation). They come from evaluating those steps in
    /// f32 (a scratch script, `invsqrt_ref.py`, with each product and sum rounded to f32), not from a hand-run of 80-bit code.
    /// Revert check: a `1.0 / a.sqrt()` implementation gives `0x3f000000` for 4.0 and fails the first assertion.
    #[test]
    fn inv_sqrt_matches_the_asm_dataflow_bit_for_bit() {
        assert_eq!(inv_sqrt(4.0).to_bits(), 0x3eff_ffff);
        assert_eq!(inv_sqrt(2.0).to_bits(), 0x3f35_04f2);
        assert_eq!(inv_sqrt(1.0).to_bits(), 0x3f7f_ffff);
        assert_eq!(inv_sqrt(100.0).to_bits(), 0x3dcc_cccc);
    }

    /// The asm is an approximation, not `1/sqrt`: some inputs differ by an ulp, and the error stays near 1e-7.
    #[test]
    fn inv_sqrt_is_not_one_over_sqrt_but_close() {
        assert_ne!(inv_sqrt(4.0), 1.0 / 4.0f32.sqrt());
        let mut differs = 0;
        for k in 1..=2000u32 {
            let a = k as f32 * 0.37;
            let exact = 1.0 / a.sqrt();
            let v = inv_sqrt(a);
            assert!(((v - exact) / exact).abs() < 2e-6, "a={a}: {v} vs {exact}");
            if v != exact {
                differs += 1;
            }
        }
        assert!(differs > 1000, "most inputs differ from 1/sqrt by rounding ({differs})");
    }

    /// `Vector3::Normalize` multiplies by the approximate inverse length, so a normalised vector is close to unit length
    /// but its components are not `x / len` (the engine's formula is the product, not the quotient).
    #[test]
    fn normalize_multiplies_by_inv_sqrt_of_the_squared_length() {
        let v = (3.0f32, 4.0, 0.0);
        let n = vec3_normalize(v);
        let oolen = inv_sqrt(25.0);
        assert_eq!(n, (3.0 * oolen, 4.0 * oolen, 0.0));
        assert!((length2(n) - 1.0).abs() < 1e-6);
        assert_eq!(vec3_normalize((0.0, 0.0, 0.0)), (0.0, 0.0, 0.0));
    }

    /// `Vector3::Normalized_Cross_Product` of (1,2,3) and (4,5,6.5) is (-2, 5.5, -3), squared length 43.25. Multiplying by
    /// `Inv_Sqrt` (the engine) gives the bits pinned here; dividing by `sqrt` (the port's old code) gives one ulp more in x, y and z.
    /// Revert check: the division form fails the bit assertions.
    #[test]
    fn normalized_cross_multiplies_by_inv_sqrt() {
        let n = normalized_cross((1.0, 2.0, 3.0), (4.0, 5.0, 6.5));
        assert_eq!(n.0.to_bits(), 0xbe9b_b4df);
        assert_eq!(n.1.to_bits(), 0x3f56_18b3);
        assert_eq!(n.2.to_bits(), 0xbee9_8f4e);
    }

    /// `Normalized_Cross_Product` has no cut-off (vector3.h:382-389): a tiny non-zero cross is still normalised to unit length,
    /// and a parallel pair (exactly zero) stays zero. Revert check: restore a `|cross| <= 1e-6` cut-off; the tiny case is zero.
    #[test]
    fn normalized_cross_has_no_cut_off() {
        let tiny = normalized_cross((1.0, 0.0, 0.0), (1.0, 1.0e-7, 0.0));
        assert!((length2(tiny) - 1.0).abs() < 1e-5, "a tiny cross is normalised: {tiny:?}");
        assert_eq!(normalized_cross((1.0, 0.0, 0.0), (2.0, 0.0, 0.0)), (0.0, 0.0, 0.0), "parallel vectors give a zero axis");
    }

    /// The Z-rotation transform with exact cos and sin (c = 1, s = 0 and c = 0, s = 1): a translation moves the point by the
    /// origin, and a quarter turn maps x to y. These inputs are exact, so they do NOT pin the summation order (any order
    /// passes); the order is a reading of `Matrix3D::Transform_Vector` (matrix3d.h:1741-1756), not a tested fact. Pinning
    /// it needs a non-trivial angle with the C++ sum order computed bit by bit (open: the review's test gap, item 13).
    #[test]
    fn z_rotation_transform_matches_exact_cos_and_sin() {
        assert_eq!(z_rotation_transform((10.0, 20.0, 30.0), 1.0, 0.0, (1.0, 2.0, 3.0)), (11.0, 22.0, 33.0));
        let (c, s) = (0.0f32, 1.0f32);
        assert_eq!(z_rotation_transform((0.0, 0.0, 0.0), c, s, (1.0, 0.0, 0.0)), (0.0, 1.0, 0.0));
    }
}
