//! Lane 179 (R21-1): `Pathfinder::iterateCellsAlongLine` calls its callback on the cell beside each
//! diagonal step as well (`AIPathfind.cpp:9261-9272`). The line-of-sight obstacle march
//! (`los_cells_along_line`) must visit it, or an obstacle on the staircase corner is never tested.

use super::*;
use crate::math::Coord3D;

fn flat() -> TerrainLogic {
    TerrainLogic::from_samples(62, 62, 0, vec![0u8; 62 * 62])
}

/// From cell (0,0) to cell (4,2) the engine's walk is (0,0),(0,1),(1,1),(2,1),(2,2),(3,2),(4,2),(4,3):
/// the `num >= den` steps at the first and third pixels move in y and call the callback on the new
/// cell, and the last step overshoots to (4,3) (`numpixels + 1` cells in all). The Bresenham line alone
/// would skip (0,1). An obstacle placed on (0,1) must therefore be reported, and the walk has 8 cells.
/// Revert check: without the extra push the walk has 5 cells and the obstacle is missing.
#[test]
fn los_march_tests_the_cell_beside_each_diagonal_step() {
    let t = flat();
    let mut pf = Pathfinder::new(&t);
    let ci = pf.idx(0, 1);
    pf.cells[ci].ty = CellType::Obstacle;
    pf.cells[ci].obstacle = 9;
    let cells = pf.los_cells_along_line(&Coord3D::new(5.0, 5.0, 0.0), &Coord3D::new(45.0, 25.0, 0.0));
    assert_eq!(cells.len(), 8, "engine visits (0,0),(0,1),(1,1),(2,1),(2,2),(3,2),(4,2),(4,3): {cells:?}");
    assert!(cells.iter().any(|c| c.0 == CellType::Obstacle && c.1 == 9), "the corner obstacle is tested: {cells:?}");
}
