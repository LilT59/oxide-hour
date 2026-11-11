//! Small `Real`/`Coord3D` helpers. All transcendental functions go through `libm` so results
//! are bit-identical on every platform (AGENTS.md determinism rules).

/// Logic frames per second (`LOGICFRAMES_PER_SECOND`, Core/GameEngine/Include/Common/GameCommon.h).
pub const LOGICFRAMES_PER_SECOND: u32 = 30;
/// Width of a height map cell in world units (`MAP_XY_FACTOR`, Common/MapObject.h).
pub const MAP_XY_FACTOR: f32 = 10.0;
/// World units per height sample step (`MAP_XY_FACTOR / 16`).
pub const MAP_HEIGHT_SCALE: f32 = MAP_XY_FACTOR / 16.0;
pub const PI: f32 = std::f32::consts::PI;
pub const TWO_PI: f32 = 2.0 * PI;

/// `Coord3D`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Coord3D {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Coord3D {
    pub const ZERO: Coord3D = Coord3D { x: 0.0, y: 0.0, z: 0.0 };
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }
    /// 2D distance (`sqrt(dx*dx + dy*dy)`).
    pub fn dist_2d(&self, o: &Coord3D) -> f32 {
        sqrt(self.dist_sqr_2d(o))
    }
    pub fn dist_sqr_2d(&self, o: &Coord3D) -> f32 {
        let (dx, dy) = (self.x - o.x, self.y - o.y);
        dx * dx + dy * dy
    }
}

pub fn sqrt(v: f32) -> f32 {
    libm::sqrtf(v)
}
/// `Sin` (`Trig.cpp:48`): `sinf` in the C++ source, which on the 32-bit UCRT is
/// `(float)sin((double)x)`. See [`crate::crtmath`] for the CRT routine this reproduces.
pub fn sin(v: f32) -> f32 {
    crate::crtmath::sin(v)
}
/// `Cos` (`Trig.cpp:52`), same `(float)cos((double)x)` path as [`sin`].
pub fn cos(v: f32) -> f32 {
    crate::crtmath::cos(v)
}
/// `atan2`: the engine calls the double `atan2` and stores the result in a `Real` (float), i.e.
/// `(float)atan2((double)y, (double)x)`. The float `libm::atan2f` is a different routine and differs
/// from that in about 16% of inputs by one ulp (measured against the 32-bit CRT, scratch probe); the
/// double one rounded to `f32` matched the CRT in 400k random pairs.
pub fn atan2(y: f32, x: f32) -> f32 {
    libm::atan2(y as f64, x as f64) as f32
}
/// `ASin` (`Trig.cpp:65-68`, `TurretAI::friend_turnTowardsPitch`'s target pitch): the source calls
/// `asinf`, but on the 32-bit x86 build `corecrt_math.h` maps `asinf` to `(float)asin((double)x)` (the
/// real float entry points are only declared for `_M_X64`/ARM), so this is the double routine rounded
/// to `f32`. The import table names `_libm_sse2_asin_precise`.
pub fn asin(v: f32) -> f32 {
    libm::asin(v as f64) as f32
}
/// `ACos` (`Trig.cpp:57-60`): `acosf` in the source, `(float)acos((double)x)` on the 32-bit build for
/// the same header reason as [`asin`]; the import table names `_libm_sse2_acos_precise`.
pub fn acos(v: f32) -> f32 {
    libm::acos(v as f64) as f32
}
pub fn floor(v: f32) -> f32 {
    libm::floorf(v)
}
pub fn ceil(v: f32) -> f32 {
    libm::ceilf(v)
}

/// `REAL_TO_INT_FLOOR` in the non-`RTS_GENERALS` build (Core/Libraries/Include/Lib/BaseFunctions.h:98, the
/// `#else` of :93-99; `RTS_ZEROHOUR` takes it): `fast_float2long_round(fast_float_floor(x))`. `fast_float_floor`
/// (:75-80) subtracts `0.99999994` from a negative input before it truncates, so an exact negative integer `n`
/// gives `n - 1` (`REAL_TO_INT_FLOOR(-2.0) == -3`), while `-2.5 -> -3` and `1e-8 -> 0`. The truncated value is
/// already whole, so the final round changes nothing. Lane 180 (R21-10): this replaces the exact `floor`.
pub fn real_to_int_floor(v: f32) -> i32 {
    const ALMOST_1: f32 = f32::from_bits(0x3F7F_FFFF); // `0.99999994`, `fast_float_floor`'s subtrahend (:77)
    let f = if v.to_bits() & 0x8000_0000 != 0 { v - ALMOST_1 } else { v };
    f.trunc() as i32
}
/// `REAL_TO_INT_CEIL` in the non-`RTS_GENERALS` build (Core/Libraries/Include/Lib/BaseFunctions.h:97, the
/// `#else` of :93-99): `fast_float2long_round(fast_float_ceil(x))`. `fast_float_ceil` (:84-90) adds
/// `0.99999994` to a non-negative input before truncating, so an exact positive integer `k` gives `k + 1`
/// (`REAL_TO_INT_CEIL(24.0) == 25`) and `0.0` gives 0. Negative inputs are only truncated (`-2.5 -> -2`).
/// Callers: the engine's `worldToCellDist` (PartitionManager.h:1546-1549), the grid sizing
/// (PartitionManager.cpp:2693-2694), the ring sample count (PartitionManager.cpp:4024) and the shroud
/// radii (`doShroudReveal` etc., PartitionManager.cpp:4060).
pub fn real_to_int_ceil(v: f32) -> i32 {
    const ALMOST_1: f32 = f32::from_bits(0x3F7F_FFFF); // `0.99999994`, `fast_float_ceil`'s addend (:86)
    let f = if v.to_bits() & 0x8000_0000 == 0 { v + ALMOST_1 } else { v };
    f.trunc() as i32
}

/// `normalizeAngle`: wraps into `(-PI, PI]`.
///
/// The literal port of `normalizeAngle` (`GameCommon.cpp:54`) is a `while (a > PI) a -= 2*PI` loop,
/// which is unbounded for a huge finite angle. A command carries an attacker-controlled angle
/// (`MSG_DOZER_CONSTRUCT`), so a large value would spin for ~1e38 iterations and hang the sim. The
/// loop runs for the magnitudes where it terminates in a few turns -- exactly as before, so every
/// angle the engine actually produces is bit-identical -- then falls back to the O(1)
/// `WWMath::Normalize_Angle` wrap (`wwmath.h:671`) for the pathological rest. TheSuperHackers mark
/// the loop version "DO NOT USE THIS FUNCTION!".
pub fn normalize_angle(mut a: f32) -> f32 {
    if !a.is_finite() {
        return 0.0;
    }
    const MAX_TURNS: u32 = 1024;
    let mut turns = 0;
    while a > PI && turns < MAX_TURNS {
        a -= TWO_PI;
        turns += 1;
    }
    while a <= -PI && turns < MAX_TURNS {
        a += TWO_PI;
        turns += 1;
    }
    if a > PI || a <= -PI {
        // `WWMath::Normalize_Angle` (`wwmath.h:671`) is a single expression, but `2*PI * floor(..)`
        // overflows `f32` for a value as large as `f32::MAX`; `fmod` reduces exactly first.
        a = libm::fmodf(a, TWO_PI);
        while a > PI {
            a -= TWO_PI;
        }
        while a <= -PI {
            a += TWO_PI;
        }
    }
    a
}

/// `stdAngleDiff(a, b)`: shortest signed difference `a - b`.
pub fn angle_diff(a: f32, b: f32) -> f32 {
    normalize_angle(a - b)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A huge finite angle must wrap in O(1) instead of spinning the `normalizeAngle` loop. A
    /// command (`MSG_DOZER_CONSTRUCT`) can carry one; the old loop hung the sim on it.
    #[test]
    fn normalize_angle_wraps_huge_finite_values() {
        for a in [f32::MAX, f32::MIN, 1.0e30, -1.0e30, 1.0e7] {
            let n = normalize_angle(a);
            assert!(n > -PI && n <= PI, "{a} -> {n}");
        }
        assert_eq!(normalize_angle(f32::NAN), 0.0);
        assert_eq!(normalize_angle(f32::INFINITY), 0.0);
    }

    /// Values the engine actually produces must be unchanged by the `fmod` pre-reduction.
    #[test]
    fn normalize_angle_matches_the_loop_for_normal_values() {
        let loop_version = |mut a: f32| {
            while a > PI {
                a -= TWO_PI;
            }
            while a <= -PI {
                a += TWO_PI;
            }
            a
        };
        for i in -1000..=1000 {
            let a = i as f32 * 0.25;
            assert_eq!(normalize_angle(a), loop_version(a), "angle {a}");
        }
    }

    /// The C++ build's `sinf`/`cosf` are `(float)sin((double)x)` from the x86 UCRT, and its `atan2`,
    /// `asin` and `acos` are the double functions with the result stored in a `Real`. These vectors are
    /// inputs where the *float* `libm` routines (what the port used before lane 171) return a different
    /// last bit; the expected values were read back from a 32-bit MSVC probe calling the same CRT the
    /// comparison build links against. Reverting any of these to the float `libm` function fails here.
    #[test]
    fn the_transcendentals_are_the_crt_double_routines_not_the_float_libm_ones() {
        let sin_x = f32::from_bits(0x3f005386);
        assert_eq!(sin(sin_x).to_bits(), 0x3ef609d0, "sin: libm::sinf gives 0x3ef609cf");
        let cos_x = f32::from_bits(0x3f000562);
        assert_eq!(cos(cos_x).to_bits(), 0x3f60a6ac, "cos: libm::cosf gives 0x3f60a6ab");
        let (y, x) = (f32::from_bits(0x3f000002), f32::from_bits(0xbf000000));
        assert_eq!(atan2(y, x).to_bits(), 0x4016cbe3, "atan2: libm::atan2f gives 0x4016cbe4");
        let asin_v = f32::from_bits(0x3f000004);
        assert_eq!(asin(asin_v).to_bits(), 0x3f060a96, "asin: libm::asinf gives 0x3f060a97");
        let acos_v = f32::from_bits(0x3f000009);
        assert_eq!(acos(acos_v).to_bits(), 0x3f860a8d, "acos: libm::acosf gives 0x3f860a8c");
    }
}
