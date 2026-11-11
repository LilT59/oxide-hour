//! Lane 176 (R20-8): `Pathfinder::patchPath`'s start cell (`AIPathfind.cpp:10507-10515`). The shipped
//! code adds `PATHFIND_CELL_SIZE_F*0.5f` to `startPos.x` **twice** when the unit is not centred — the
//! second add was meant for `y` — and `startCellNdx` then feeds the path walk's two-cell transient
//! window (`:10581-10582`) and the neighbour expansion (`:6326-6329`, `:6353-6355`).

use super::*;
use crate::math::Coord3D;
use crate::object::ObjectId;

fn patch_unit() -> PathObject {
    PathObject {
        id: ObjectId(1),
        surfaces: surface::GROUND,
        bounding_radius: 8.0,
        layer: PathLayer::Ground,
        player: 0,
        infantry: false,
        crusher: false,
        dozer: false,
        harvester: false,
        aircraft_that_adjusts_destination: false,
        is_aircraft: false,
        can_path_through_units: false,
        ignore_obstacle: ObjectId::INVALID,
        attacking: false,
        busy: false,
        using_ability: false,
    }
}

fn flat() -> TerrainLogic {
    TerrainLogic::from_samples(62, 62, 0, vec![0u8; 62 * 62])
}

/// A patching unit at `from = (95, 100)` is not centred (`radius_and_center` = `(1, false)`), so the
/// engine's start position is `95 + 10 = 105` → cell (10,10), not the raw cell (9,10). The single path
/// node at (125,115) is cell (12,11): two cells from the engine's start cell (inside the transient
/// window, so the moving ally in its footprint ends the walk and there is no stitch point) but three
/// from the raw one (outside the window, so the ally is ignored and the node is accepted). Revert check:
/// `world_to_cell(from)` (no double add) returns a patched path.
#[test]
fn patch_path_measures_its_transient_window_from_the_doubly_adjusted_start_cell() {
    let t = flat();
    let mut pf = Pathfinder::new(&t);
    let unit = patch_unit();
    // A moving ally in cell (12,11)'s footprint.
    let mut ally = patch_unit();
    ally.id = ObjectId(2);
    pf.update_unit_pos(&ally, &Coord3D::new(120.0, 110.0, 0.0), true);
    let from = Coord3D::new(95.0, 100.0, 0.0);
    let original = Path {
        nodes: vec![
            PathNode { pos: from, can_optimize: true, next_optimized: Some(1) },
            PathNode { pos: Coord3D::new(125.0, 115.0, 0.0), can_optimize: true, next_optimized: None },
        ],
        blocked_by_ally: false,
    };
    assert!(
        pf.patch_path(&t, &unit, &from, &original, true).is_none(),
        "the node is inside the transient window of the engine's start cell, so the moving ally blocks it"
    );
}
