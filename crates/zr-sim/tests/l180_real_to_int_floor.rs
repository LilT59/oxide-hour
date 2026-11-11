//! Lane 180 (R21-10): `REAL_TO_INT_FLOOR` in Zero Hour is `fast_float2long_round(fast_float_floor(x))`
//! (Core/Libraries/Include/Lib/BaseFunctions.h:98, the `#else` of :93-99, since `RTS_GENERALS` is off for ZH).
//! `fast_float_floor` (:75-80) subtracts `0.99999994` from a negative input before it truncates, so an exact
//! negative integer goes one lower than `floor`.

use zr_sim::math::real_to_int_floor;

/// Positive inputs and non-integers floor as usual; an exact negative integer `n` gives `n - 1`.
/// Revert check: the exact `floor(v) as i32` form gives -2 for -2.0 and this test fails.
#[test]
fn real_to_int_floor_is_the_zero_hour_form() {
    assert_eq!(real_to_int_floor(2.0), 2);
    assert_eq!(real_to_int_floor(2.7), 2);
    assert_eq!(real_to_int_floor(0.0), 0);
    assert_eq!(real_to_int_floor(-0.5), -1);
    assert_eq!(real_to_int_floor(-2.5), -3);
    assert_eq!(real_to_int_floor(-1.0), -2, "an exact negative integer goes one lower under fast_float_floor");
    assert_eq!(real_to_int_floor(-2.0), -3);
    assert_eq!(real_to_int_floor(-10.0), -11);
    // `-1e-8 - 0.99999994` rounds to `-0.99999994` in f32, which truncates to 0.
    assert_eq!(real_to_int_floor(-1e-8), 0);
}
