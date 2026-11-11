//! Lane 179 (R21-11): `REAL_TO_INT_CEIL` in the non-`RTS_GENERALS` build is
//! `fast_float2long_round(fast_float_ceil(x))` (Core/Libraries/Include/Lib/BaseFunctions.h:97, `#else` of
//! :93-99). `fast_float_ceil` adds `0.99999994` before truncating, so an exact positive integer `k` gives
//! `k + 1`. The callers the port maps to it (`worldToCellDist`, PartitionManager.h:1546-1549; the grid
//! sizing, PartitionManager.cpp:2693; the ring sample count, :4024; the shroud radii, :4060) all need it.
//! Revert check: an exact `ceil` returns 1 for `1.0` and fails the first assertion.

use zr_sim::math::real_to_int_ceil;

#[test]
fn an_exact_positive_integer_rounds_up_by_one_as_the_engine_does() {
    assert_eq!(real_to_int_ceil(1.0), 2);
    assert_eq!(real_to_int_ceil(2.0), 3);
    assert_eq!(real_to_int_ceil(24.0), 25);
    // range 200 with cell size 40: `worldToCellDist(200)` = REAL_TO_INT_CEIL(200 * 0.025) = 6, not 5.
    assert_eq!(real_to_int_ceil(200.0 * (1.0 / 40.0)), 6);
    // `10 * 0.1f` is exactly 1.0f, so the COI box gets two steps (partition_l175.rs, AN1 s3 f1840).
    assert_eq!(real_to_int_ceil(10.0 * 0.1), 2);
}

#[test]
fn a_fraction_rounds_up_and_zero_stays_zero() {
    assert_eq!(real_to_int_ceil(1.7), 2);
    assert_eq!(real_to_int_ceil(0.5), 1);
    assert_eq!(real_to_int_ceil(0.0), 0);
}

#[test]
fn a_negative_input_is_only_truncated() {
    assert_eq!(real_to_int_ceil(-2.0), -2);
    assert_eq!(real_to_int_ceil(-2.5), -2);
}
