//! Lane 180 (R21-9): `Point_In_Triangle_2D`'s collinear branch measures its third edge with the second edge's
//! vector (`WWMath/tri.h:154`, `float p2p0dist2 = p1p2.Length2();`). The port copies that quirk.

use super::*;

/// The points (0,0), (5,0) and (-10,0) are collinear. The engine's lengths are d01 = 25, d12 = 225 and, with the
/// quirk, d20 = d12 = 225, so the segment is taken from (-10,0) with max length 225 and the test point (-12,0) is
/// within 225 of that start: inside. With the true p2p0 length (100) the segment is (5,0)..(-10,0) and (-12,0) lies
/// outside it. Revert check: `let d20 = d12;` back to the true `p2p0` length makes this false.
#[test]
fn a_collinear_triangle_takes_its_third_edge_length_from_the_second_edge() {
    assert!(point_in_triangle_2d((0.0, 0.0), (5.0, 0.0), (-10.0, 0.0), (-12.0, 0.0)), "tri.h:154 copies p1p2's length");
}
