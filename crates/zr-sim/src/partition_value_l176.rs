//! Lane 176 (R20-7): `hLineAddValue`'s falloff keeps the engine's `Real` arithmetic
//! (`PartitionManager.cpp:5823-5832`, `:5851-5860`): `Real distance = sqrt(pow(x - xCenter, 2) +
//! pow(y - yCenter, 2))` stores the *double* result in a `Real` (rounded to `f32`), and
//! `mulVal = 1 - distance / parms->radius` is then a float expression. The old port did the whole
//! computation in `f64` and rounded at the end, which truncates to a different cell value where
//! `threatOrValue * mulVal` crosses an integer.

use super::*;

/// Find a `(dx, dy, value)` where the engine's `f32` falloff and the old `f64` form truncate
/// differently, then check the map holds the engine's value. Revert check: compute `distance` in `f64`
/// and cast only at the end (`(1.0 - distance / radius as f64) as f32`) and this fails.
#[test]
fn the_value_falloff_rounds_the_distance_to_a_real_first() {
    const CENTER: i32 = 40;
    const CELL_RADIUS: i32 = 37;
    let cell_size = 10.0f32;
    let f_cell_radius = (CELL_RADIUS + 1) as f32;
    let mut found = None;
    'search: for dx in -CELL_RADIUS..=CELL_RADIUS {
        for dy in -CELL_RADIUS..=CELL_RADIUS {
            let fx = dx as f32;
            let fy = dy as f32;
            let d64 = (f64::from(fx) * f64::from(fx) + f64::from(fy) * f64::from(fy)).sqrt();
            let m32 = (1.0f32 - (d64 as f32) / f_cell_radius).clamp(0.0, 1.0);
            let m64 = ((1.0 - d64 / f64::from(f_cell_radius)) as f32).clamp(0.0, 1.0);
            if m32 == m64 {
                continue;
            }
            for value in 1u32..=4000 {
                let want = (value as f32 * m32) as u32;
                let old = (value as f32 * m64) as u32;
                if want != old {
                    found = Some((dx, dy, value, want, old));
                    break 'search;
                }
            }
        }
    }
    let (dx, dy, value, want, old) = found.expect("a (dx, dy, value) that truncates differently");
    assert_ne!(want, old, "the two forms disagree on this cell");
    let mut map = ValueMap::new(100, 100, cell_size);
    let affect = ValueAffect {
        pos: Coord3D::new((CENTER as f32 + 0.5) * cell_size, (CENTER as f32 + 0.5) * cell_size, 0.0),
        // Off the exact multiple: `worldToCellDist(range)` is `REAL_TO_INT_CEIL(range / cell)`, which gives k+1
        // for an exact k (BaseFunctions.h:97, PartitionManager.h:1548). 36.5 cells gives CELL_RADIUS = 37 as intended.
        range: CELL_RADIUS as f32 * cell_size - 5.0,
        data: value,
        mask: 1,
    };
    map.apply(&affect, true);
    assert_eq!(map.cell_value(CENTER + dx, CENTER + dy, 0), want, "the engine's f32 falloff at ({dx},{dy}) for {value}");
}
