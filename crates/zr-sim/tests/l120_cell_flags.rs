//! `PathfindCell`'s flag model (`PathfindCell::CellFlags`, `AIPathfind.cpp:1420-1455`,
//! `:1492-1531`): the flags are derived from the cell's position unit (`setPosUnit`) and goal unit
//! (`setGoalUnit`), not from the unit's live moving state.
//!
//! * `UNIT_PRESENT_FIXED` means the unit here is on its own goal cell (`posUnit == goalUnit`).
//! * `UNIT_PRESENT_MOVING` is "present, not on its goal" — a stationary unit away from its goal is
//!   still transient.
//! * `UNIT_GOAL` is a goal with no unit; `UNIT_GOAL_OTHER_MOVING` a goal over somebody else's cell.
//!
//! `checkForMovement` (`AIPathfind.cpp:5097-5221`) reports `allyGoal` for the two goal states and
//! classifies the rest as transient/fixed; `checkDestination` (`:5013-5094`) blocks only on
//! `UNIT_PRESENT_FIXED` cells (and on an ally's goal). Synthetic data only.

use zr_sim::pathfind::{surface, PathLayer, Pathfinder};
use zr_sim::{Coord3D, ObjectId, TerrainLogic};

fn pos(x: f32, y: f32) -> Coord3D {
    Coord3D::new(x, y, 0.0)
}

/// A radius-1 (one-cell), non-centred ground unit.
fn unit(id: u32, player: u8) -> zr_sim::pathfind::PathObject {
    zr_sim::pathfind::PathObject {
        id: ObjectId(id),
        surfaces: surface::GROUND,
        bounding_radius: 15.0,
        layer: PathLayer::Ground,
        player,
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

fn world() -> Pathfinder {
    Pathfinder::new(&TerrainLogic::flat(400, 400, 0))
}

fn allied(pf: &mut Pathfinder) {
    let mut a = [0u32; 32];
    a[0] |= 1 << 0;
    a[1] |= 1 << 1;
    pf.set_allies(a);
}

/// A unit away from its goal is `UNIT_PRESENT_MOVING` (transient), even when it is standing still:
/// the flag is `posUnit != goalUnit`, not the AI's moving bit. The A* (`considerTransient` false)
/// only reports `allyMoving`; a caller that considers transient units counts it fixed.
#[test]
fn a_stationary_unit_away_from_its_goal_is_transient() {
    let mut pf = world();
    allied(&mut pf);
    let mover = unit(1, 0);
    let other = unit(2, 0);
    let here = pos(300.0, 300.0);
    pf.update_unit_pos(&other, &here, false);
    pf.update_goal(&other, &pos(500.0, 500.0));
    let info = pf.check_for_movement_full(&mover, &here, false);
    assert!(info.ally_moving && info.ally_fixed_count == 0, "UNIT_PRESENT_MOVING is transient: {info:?}");
    let info = pf.check_for_movement_full(&mover, &here, true);
    assert_eq!(info.ally_fixed_count, 1, "considerTransient counts it fixed: {info:?}");
}

/// A unit on its own goal cell is `UNIT_PRESENT_FIXED`; once its goal moves away the cell becomes
/// `UNIT_PRESENT_MOVING` (clearing the goal leaves the position), and the goal cell it left is
/// `UNIT_GOAL` with no unit.
#[test]
fn fixed_follows_the_goal_marks() {
    let mut pf = world();
    allied(&mut pf);
    let mover = unit(1, 0);
    let other = unit(2, 0);
    let here = pos(300.0, 300.0);
    pf.update_unit_pos(&other, &here, false);
    pf.update_goal(&other, &here);
    let info = pf.check_for_movement_full(&mover, &here, false);
    assert_eq!(info.ally_fixed_count, 1, "pos == goal is UNIT_PRESENT_FIXED: {info:?}");
    assert!(!info.ally_goal, "a fixed cell is not UNIT_GOAL");
    // Move the goal away: the old cell keeps the unit (`setGoalUnit(INVALID)` -> UNIT_PRESENT_MOVING)
    // and the new cell is a goal with no unit (`UNIT_GOAL`).
    let goal = pos(500.0, 500.0);
    pf.update_goal(&other, &goal);
    let info = pf.check_for_movement_full(&mover, &here, false);
    assert!(info.ally_moving && info.ally_fixed_count == 0, "goal cleared -> UNIT_PRESENT_MOVING: {info:?}");
    let info = pf.check_for_movement_full(&mover, &goal, false);
    assert!(info.ally_goal, "a goal with no unit is UNIT_GOAL: {info:?}");
}

/// `setPosUnit`'s single-unit rules (`AIPathfind.cpp:1492-1531`): a second unit moving into a cell
/// whose unit is already on its own goal does not take the cell over, while a cell whose unit is
/// merely passing (`UNIT_PRESENT_MOVING`) is overwritten. The probe is allied with the second unit
/// and not with the first, so which unit owns the cell decides the result.
#[test]
fn a_unit_on_its_goal_keeps_the_cell() {
    let mut pf = world();
    allied(&mut pf);
    let enemy = unit(2, 1);
    let second = unit(3, 0);
    let mover = unit(1, 0);
    let here = pos(300.0, 300.0);
    let away = pos(400.0, 300.0);
    // The enemy stands on its own goal: the cell is UNIT_PRESENT_FIXED and keeps the enemy.
    pf.update_unit_pos(&enemy, &here, false);
    pf.update_goal(&enemy, &here);
    pf.update_unit_pos(&second, &away, true);
    pf.update_unit_pos(&second, &here, true);
    let info = pf.check_for_movement_full(&mover, &here, true);
    assert!(info.enemy_fixed && info.ally_fixed_count == 0, "the fixed enemy keeps the cell: {info:?}");
    // The enemy's goal moves away, so the cell is UNIT_PRESENT_MOVING; the next `setPosUnit` takes
    // it over. The second unit is an ally of the probe.
    pf.update_goal(&enemy, &pos(500.0, 500.0));
    pf.update_unit_pos(&second, &away, true);
    pf.update_unit_pos(&second, &here, true);
    let info = pf.check_for_movement_full(&mover, &here, true);
    assert!(!info.enemy_fixed && info.ally_fixed_count == 1, "a transient unit's cell is overwritten: {info:?}");
}

/// A goal mark over another unit's cell is `UNIT_GOAL_OTHER_MOVING`: `allyGoal` plus the moving
/// ally, so the attack-path cost and the transient classification both see it.
#[test]
fn a_goal_over_another_units_cell_is_goal_other_moving() {
    let mut pf = world();
    allied(&mut pf);
    let mover = unit(1, 0);
    let stand = unit(2, 0);
    let target = unit(3, 0);
    let here = pos(300.0, 300.0);
    pf.update_unit_pos(&stand, &here, false);
    pf.update_goal(&target, &here);
    let info = pf.check_for_movement_full(&mover, &here, false);
    assert!(info.ally_goal && info.ally_moving, "UNIT_GOAL_OTHER_MOVING: {info:?}");
    assert_eq!(info.ally_fixed_count, 0, "transient under the A*");
}
