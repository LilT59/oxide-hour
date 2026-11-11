//! Lane 183 (R22-3): the degenerate branch of `Point_In_Triangle_2D` (`WWMath/tri.h`) takes the end vector by the start's
//! index, `pSpE` = p0p1 for start 0, p1p2 for start 1, p2p0 for start 2 (tri.h:161-171). Choosing it by value takes the
//! wrong edge when two vertices share coordinates.

use super::*;

/// p0 == p2 == (0,0), p1 == (5,0). The engine's lengths are d01 = 25, d12 = 25, d20 = d12 = 25, so the start is p2 and
/// `pSpE` is p2p0, which is the zero vector. Every test point is then colinear, and a point is inside when
/// |pSpT|^2 <= 25. (3,3) has |pSpT|^2 = 18: inside. Choosing the end by value gives p0p1 = (5,0) and rejects (3,3).
/// Revert check: `let to_end = if start == p0 { p0p1 } ...` (the value test) rejects the point and the test fails.
#[test]
fn a_coincident_start_vertex_takes_the_end_edge_of_its_index() {
    assert!(point_in_triangle_2d((0.0, 0.0), (5.0, 0.0), (0.0, 0.0), (3.0, 3.0)), "tri.h picks p2p0 for start 2");
}
