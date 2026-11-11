//! Lane 191 (wave 22, AN5 s1 crawler 263). Two pathfinder facts behind the attack search's endpoint and the attack path's
//! failure at f3128:
//!
//! - `checkDestination` (`AIPathfind.cpp:5082-5086`) lets a unit stand on a goal cell that holds a fixed unit when
//!   `obj->canCrushOrSquish(unit, TEST_CRUSH_OR_SQUISH)` (`Object.cpp:1121-1200`). The port refused every fixed unit, so
//!   the crawler's fallback adjust picked (415,1255) where C++ picked (415,1245).
//! - `examineNeighboringCells` counts a neighbour only when it had no `PathfindCellInfo` (`AIPathfind.cpp:6344-6349`).
//!   A unit, goal or obstacle cell keeps one (`releaseInfo`, `:1395`), so the attack search's `ATTACK_CELL_LIMIT`
//!   (`:10765`, `:11014-11018`) did not count it. The port counted it and stopped earlier.

use super::*;
use crate::modules2::CrushInfo;

fn flat() -> TerrainLogic {
    TerrainLogic::from_samples(62, 62, 0, vec![0u8; 62 * 62])
}

fn unit(id: u32, radius: f32) -> PathObject {
    PathObject { id: ObjectId(id), surfaces: surface::GROUND, bounding_radius: radius, layer: PathLayer::Ground, player: 0, infantry: false, crusher: false, ..Default::default() }
}

/// A unit that stands still on a cell: its position cell is also its goal (`UNIT_PRESENT_FIXED`, as `pathfind_l188`).
fn fixed_unit(pf: &mut Pathfinder, obj: &PathObject, at: &Coord3D) {
    pf.update_unit_pos(obj, at, false);
    pf.update_goal(obj, at);
}

/// A crusher (level 2) and an infantry victim standing on a cell. The footprint (radius 1, not centred) covers the
/// victim's cell, so `check_destination` asks whether the crusher can move onto the fixed victim. Revert check: the
/// old `UnitPresentFixed` refusal (no `can_crush_unit` call) makes the first assertion fail.
#[test]
fn a_crusher_may_move_onto_a_fixed_squishable_unit() {
    let t = flat();
    let mut pf = Pathfinder::new(&t);
    let mut crusher = unit(1, 5.0);
    crusher.crusher = true;
    // The victim belongs to player 1 (an enemy of player 0: a same-player goal is an ally, `checkDestination` refuses it).
    let victim = PathObject { player: 1, ..unit(2, 5.0) };
    let at = Coord3D::new(420.0, 500.0, 0.0);
    fixed_unit(&mut pf, &victim, &at);
    let owner = |u: u32| -> Option<crate::object::PlayerIndex> { Some(if u == 2 { 1 } else { 0 }) };
    let (cx, cy, _) = pf.world_to_cell(&at);
    pf.set_unit_crush([
        (1, CrushInfo { crusher_level: 2, crushable_level: 0, squish_collide: false }, false),
        (2, CrushInfo { crusher_level: 0, crushable_level: 0, squish_collide: true }, false),
    ]);
    assert!(pf.check_destination_unit(&crusher, cx, cy, 1, false, &owner), "a crusher may stand on a squishable unit's cell (Object.cpp:1152)");
    // The same victim is not squishable and its crushable level is 2: a level-2 crusher is not greater, so it is refused.
    pf.set_unit_crush([
        (1, CrushInfo { crusher_level: 2, crushable_level: 0, squish_collide: false }, false),
        (2, CrushInfo { crusher_level: 0, crushable_level: 2, squish_collide: false }, false),
    ]);
    assert!(!pf.check_destination_unit(&crusher, cx, cy, 1, false, &owner), "a crusher level that is not above CrushableLevel does not crush");
    // An unmanned crusher cannot crush (Object.cpp:1137-1140, `DISABLED_UNMANNED`).
    pf.set_unit_crush([
        (1, CrushInfo { crusher_level: 2, crushable_level: 0, squish_collide: false }, true),
        (2, CrushInfo { crusher_level: 0, crushable_level: 0, squish_collide: true }, false),
    ]);
    assert!(!pf.check_destination_unit(&crusher, cx, cy, 1, false, &owner), "an unmanned crusher does not crush");
}

/// `examineNeighboringCells` counts only the neighbours that had no `PathfindCellInfo`. An ally standing on a neighbour
/// keeps its info (a unit cell), so the count is one less than with that neighbour empty. Revert check: count every
/// neighbour that `!info.has` (the old `if !self.info[nc].has { ...; cell_count += 1 }`): the difference is zero.
#[test]
fn a_neighbour_with_a_unit_is_not_counted_toward_the_search_limit() {
    let t = flat();
    let goal_at = Coord3D::new(400.0, 500.0, 0.0);
    let parent_at = Coord3D::new(300.0, 500.0, 0.0);
    let count_with = |occupied: bool| {
        let mut pf = Pathfinder::new(&t);
        let mover = unit(1, 5.0);
        if occupied {
            // The ally sits on the cell east of the parent, which the expansion reaches first.
            fixed_unit(&mut pf, &unit(3, 5.0), &Coord3D::new(310.0, 500.0, 0.0));
        }
        let (px, py, _) = pf.world_to_cell(&parent_at);
        let (gx, gy, _) = pf.world_to_cell(&goal_at);
        let parent = pf.idx(px, py);
        let goal = pf.idx(gx, gy);
        assert!(pf.allocate_info(parent));
        pf.info[parent].so_far = 0;
        pf.info[parent].total = 0;
        pf.info[parent].parent = NONE;
        let ctx = Pathfinder::ctx_for(&mover);
        pf.examine_neighboring_cells(&t, parent, Some(goal), Some(120.0), &ctx, (px, py))
    };
    let empty = count_with(false);
    let blocked = count_with(true);
    assert_eq!(empty - blocked, 1, "the occupied neighbour is not counted (empty {empty}, occupied {blocked})");
}
