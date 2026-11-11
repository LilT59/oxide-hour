//! Lane 158 (wave 18): `AIUpdateInterface::blockedBy` (`AIUpdate.cpp:1289`) ignores obstacles near the final goal
//! only when the pathfind goal cell is set (`goalCell.x>0 && goalCell.y>0`, `:1295-1296`), and it measures that
//! distance from the state machine's goal position (`getStateMachine()->getGoalPosition()`, `:1297`), not from the
//! pathfinder's goal cell. AN1 s1 f1663: the China dozer 211 is 9.8 / 9.4 units from its pathfinder goal cell
//! (940.5, 400.5) but 11.0 / 5.4 from its state-machine goal (941.68, 404.67), so the C++ still treats dozer 230 as
//! an obstacle (its contact at f1664 blocks 211) and the port did not. The rest of `processCollision` and
//! `blockedBy` (order of the flags, `needToRotate`, `hasHigherPathPriority`) is covered by the tests below.

use super::*;

/// A ground unit at `pos` facing `angle`, moving, with the given state-machine goal and goal-cell flag.
fn unit(id: u32, pos: (f32, f32), angle: f32, goal_cell_valid: bool, machine_goal: (f32, f32)) -> CollisionUnit {
    CollisionUnit {
        id: ObjectId(id),
        pos: Coord3D::new(pos.0, pos.1, 18.75),
        angle,
        infantry: false,
        ground: true,
        dead_state: false,
        moving: true,
        waiting_for_path: false,
        blocked_frames: 0,
        machine_goal: Coord3D::new(machine_goal.0, machine_goal.1, 18.75),
        goal_cell_valid,
        loco_goal: true,
    }
}

#[test]
fn the_near_goal_test_measures_from_the_state_machine_goal() {
    // AN1 s1 f1663 (C++ and port positions): the pathfinder goal cell (940.5, 400.5) is within a cell of the dozer
    // (dx 9.78, dy 9.44), but the state-machine goal (941.68, 404.67) is not (dx 10.97). The obstacle counts.
    let me = unit(211, (930.7163, 410.0602), -0.773839, true, (941.68256, 404.66626));
    let other = unit(230, (948.0769, 385.9914), 0.864245, true, (1050.5, 660.5));
    assert!(blocked_by(&me, &other), "the state-machine goal is a cell away, so 230 still blocks 211");
}

#[test]
fn a_unit_at_its_state_machine_goal_ignores_obstacles() {
    // Goal cell set and the state-machine goal a cell away on both axes: `blockedBy` returns FALSE (`:1300-1301`).
    let me = unit(211, (930.0, 410.0), -0.7738, true, (934.0, 406.0));
    let other = unit(230, (948.0, 386.0), 0.8642, true, (1050.5, 660.5));
    assert!(!blocked_by(&me, &other), "near the final goal the obstacle is ignored");
}

#[test]
fn no_goal_cell_means_no_near_goal_exemption() {
    // `goalCell.x>0 && goalCell.y>0` is false: the near-goal test is skipped even though the machine goal is on top of us.
    let me = unit(211, (930.0, 410.0), -0.7738, false, (930.0, 410.0));
    let other = unit(230, (948.0, 386.0), 0.8642, true, (1050.5, 660.5));
    assert!(blocked_by(&me, &other), "without a pathfind goal cell the exemption does not apply");
}

/// A BOX vehicle (17 x 10 half-extents, height 15) on the ground, with no crush levels.
fn boxed(name: &str) -> std::sync::Arc<crate::template::ObjectTemplate> {
    use crate::physics::PhysicsTemplate;
    use crate::template::{kind_of, LocomotorKind, LocomotorParams, ObjectTemplate};
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.shape = crate::template::GeometryShape::Box;
    t.geometry.major_radius = 17.0;
    t.geometry.minor_radius = 10.0;
    t.geometry.height = 15.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.physics = Some(PhysicsTemplate { mass: 10.0, ..PhysicsTemplate::default() });
    t.locomotor = Some(LocomotorParams::from_engine_units(LocomotorKind::Treads, 1.0, 0.0, 0.1, 0.1, 0.1, true));
    std::sync::Arc::new(t)
}

/// Two boxed vehicles 30 units apart on the x axis (a contact: the boxes are 34 units wide), the first at
/// `(100, 100)` moving toward `(300, 100)`, the second at `(130, 100)`.
fn touching_pair() -> (GameLogic, ObjectId, ObjectId) {
    let t = boxed("Box");
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    let me = g.create_object(&t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let other = g.create_object(&t, Coord3D::new(130.0, 100.0, 0.0), 0.0, 1);
    let o = g.objects.get_mut(&me).unwrap();
    o.ai.move_goal = Some(Coord3D::new(300.0, 100.0, 0.0));
    o.ai.waiting_for_path = true;
    // The mover is in its move state (`friend_startingMove`, AIStates.cpp:1635): `isMoving` needs it.
    o.ai.machine.current = crate::ai_states::AiStateType::MoveTo;
    o.ai.friend_starting_move();
    (g, me, other)
}

#[test]
fn a_unit_that_must_turn_to_its_path_is_not_stuck_against_a_stationary_unit() {
    // `needToRotate()` (`AIUpdate.cpp:1400`) is TRUE for a unit still waiting for its path (`:1401`), so the
    // blocked unit is "rotating": `m_isBlockedAndStuck` stays FALSE and `m_blockedFrames` is set to 1 (`:1536-1539`).
    let (mut g, me, other) = touching_pair();
    assert!(g.need_to_rotate(me));
    g.update_partition();
    let o = &g.objects[&me];
    assert!(o.ai.is_blocked, "the stationary unit blocks the mover");
    assert!(!o.ai.is_blocked_and_stuck, "a unit that must turn to its path is not stuck");
    assert_eq!(o.ai.blocked_frames, 1, "rotating units do not accumulate blocked frames");
    assert!(!g.objects[&other].ai.is_blocked, "the stationary unit is not blocked by a unit it does not move toward");
}

#[test]
fn a_block_is_set_before_the_wait_for_path_check() {
    // `m_isBlocked = TRUE` (`AIUpdate.cpp:1457`) comes before `if (otherMoving && aiOther->isWaitingForPath())
    // return FALSE` (`:1458-1459`): the mover is blocked, but takes no max speed from a unit that is waiting.
    let (mut g, me, other) = touching_pair();
    {
        let o = g.objects.get_mut(&other).unwrap();
        o.ai.move_goal = Some(Coord3D::new(0.0, 100.0, 0.0));
        o.ai.waiting_for_path = true;
        o.ai.machine.current = crate::ai_states::AiStateType::MoveTo;
        o.ai.friend_starting_move();
    }
    g.update_partition();
    let o = &g.objects[&me];
    assert!(o.ai.is_blocked, "blocked even though the other unit is waiting for its path");
    assert_eq!(o.ai.cur_max_blocked_speed, FAST_AS_POSSIBLE, "the max speed is not taken from a waiting unit");
}

#[test]
fn crossing_units_the_lower_priority_one_is_blocked() {
    // `blockedBy` (`AIUpdate.cpp:1377-1392`): the two units run into each other, and the closer one keeps going
    // only when it has the higher path priority (`hasHigherPathPriority`, `:1380-1381`). Without it, the unit is blocked.
    let me = unit(9, (0.0, 0.0), std::f32::consts::FRAC_PI_2, false, (1000.0, 1000.0));
    let other = unit(3, (-14.0, 10.0), std::f32::consts::FRAC_PI_4, false, (1000.0, 1000.0));
    assert!(blocked_by_pri(&me, &other, false), "the lower priority unit is blocked");
    assert!(!blocked_by_pri(&me, &other, true), "the higher priority unit goes on");
}
