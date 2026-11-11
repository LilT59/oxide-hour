//! Lane 161 (wave 19): `Pathfinder::checkDestination` (`AIPathfind.cpp:5009-5097`) tests a footprint cell only for an
//! obstacle (unless it holds the ignored one), an impassable cell (`IS_IMPASSABLE`, `:78-91`, `:5047`), and the units
//! standing there. It has no surface-mask test. The port's `check_destination` also rejected a water, cliff or rubble
//! footprint cell that the unit's surfaces do not include, so a ground destination next to a lake was refused where
//! C++ accepts it. The surface test belongs to the centre cell's `validMovementPosition` (the path gate of
//! `checkForAdjust`), which the port keeps.

use super::*;

/// A ground unit (`LocomotorSet` surfaces `GROUND`), no radius beyond the footprint argument, no units.
fn ground_unit() -> PathObject {
    PathObject {
        id: ObjectId::INVALID,
        surfaces: surface::GROUND,
        bounding_radius: 6.0,
        layer: crate::pathfind::PathLayer::Ground,
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

/// Flat 62x62 playable terrain (the same shape as the pathfind tests' `terrain_with`).
fn flat_terrain() -> TerrainLogic {
    TerrainLogic::from_samples(62, 62, 0, vec![0u8; 62 * 62])
}

/// A footprint of 2x2 cells at (29..31, 29..31) whose cell (30, 30) has type `ty`; the other cells are clear.
fn footprint_with(ty: CellType) -> bool {
    let t = flat_terrain();
    let mut pf = Pathfinder::new(&t);
    let ci = pf.get_cell(30, 30).expect("an on-map cell");
    pf.cells[ci].ty = ty;
    pf.check_destination(&ground_unit(), 30, 30, 1, false)
}

#[test]
fn a_water_cliff_or_rubble_footprint_cell_does_not_block_a_destination() {
    // C++ `checkDestination` returns true here: none of these is `CELL_OBSTACLE`, `CELL_IMPASSABLE` or
    // `CELL_BRIDGE_IMPASSABLE`, and nothing stands on the cell. Revert check: restoring the surface-mask test in
    // `check_destination` makes these three asserts fail.
    assert!(footprint_with(CellType::Water), "a lake cell in the footprint");
    assert!(footprint_with(CellType::Cliff), "a cliff cell in the footprint");
    assert!(footprint_with(CellType::Rubble), "a rubble cell in the footprint");
}

#[test]
fn an_impassable_or_obstacle_footprint_cell_still_blocks_a_destination() {
    // `IS_IMPASSABLE` (:78-91) and the obstacle branch (:5041-5046) still return false.
    assert!(!footprint_with(CellType::Impassable), "an impassable cell");
    assert!(!footprint_with(CellType::BridgeImpassable), "a bridge-impassable cell");
    assert!(!footprint_with(CellType::Obstacle), "an obstacle cell");
}
