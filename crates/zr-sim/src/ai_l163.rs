//! Lane 163 (wave 19), adversarial review R17 of dde889c: the tail of `AIUpdateInterface::doLocomotor`
//! (`AIUpdate.cpp:2127-2301`) for the blocked-frame bookkeeping and the blocked speed cap.
//! - R17-1: `else { blocked = FALSE; ... }` (`:2228`): a speed under `m_curMaxBlockedSpeed` moves the unit as
//!   unblocked, so the kernel gets FALSE.
//! - R17-2: `if (!blocked && m_blockedFrames>1) m_blockedFrames = 1;` (`:2290-2293`), with the kernel's output.
//! - R17-9: `m_curMaxBlockedSpeed = FAST_AS_POSSIBLE;` (`:2301`) after the switch, for every goal type; a move
//!   still waiting for its path returns before it (`:2177-2179`).

use super::*;
use crate::physics::PhysicsTemplate;
use crate::template::{kind_of, LocomotorKind, LocomotorParams, ObjectTemplate};
use std::sync::Arc;

fn tread() -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new("Tread");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 5.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.physics = Some(PhysicsTemplate { mass: 10.0, ..PhysicsTemplate::default() });
    t.locomotor = Some(LocomotorParams::from_engine_units(LocomotorKind::Treads, 1.0, 0.0, 0.1, 0.5, 0.5, true));
    Arc::new(t)
}

/// A treaded unit at (50, 50) facing +x with a goal 100 units ahead, moving at `speed`.
fn unit_moving(speed: f32) -> (GameLogic, ObjectId) {
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    let id = g.create_object(&tread(), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    let o = g.objects.get_mut(&id).unwrap();
    o.physics.as_mut().unwrap().velocity = Coord3D::new(speed, 0.0, 0.0);
    o.ai.locomotor_goal_type = LocoGoalType::PositionOnPath;
    o.ai.path = vec![Coord3D::new(150.0, 50.0, 0.0)];
    (g, id)
}

#[test]
fn a_blocked_unit_whose_speed_is_under_the_blocked_cap_moves_as_unblocked() {
    // `Locomotor.cpp:1040`: with the kernel's `blocked` TRUE a unit whose velocity (1.0) is not below the desired
    // speed (0.5) is still blocked, so it would be scrubbed to 0.5 with no motive force. `doLocomotor` sets
    // `blocked = FALSE` when the speed is under the cap (`AIUpdate.cpp:2228`): the kernel brakes it normally.
    // Revert check: remove `blocked = false;` from the else branch of do_locomotor_obj and this fails (velocity 0.5).
    let (mut g, id) = unit_moving(1.0);
    {
        let o = g.objects.get_mut(&id).unwrap();
        o.ai.desired_speed = 0.5;
        o.ai.is_blocked = true;
    }
    g.ai_do_locomotor(id);
    let p = g.objects[&id].physics.as_ref().unwrap();
    assert!((p.velocity_magnitude() - 1.0).abs() < 1e-5, "not scrubbed to the desired speed: {}", p.velocity_magnitude());
    assert!(p.accel.x < -0.01, "the unblocked kernel brakes toward the desired speed: {}", p.accel.x);
}

#[test]
fn a_frame_the_unit_is_no_longer_blocked_drops_its_blocked_counter_to_one() {
    // The unit was blocked for 5 frames and is free this frame (its velocity 0 is below the desired 0.5, which clears
    // the blocked flag in the kernel, `:1040`). The kernel's output is FALSE, so `if (!blocked && m_blockedFrames>1)
    // m_blockedFrames = 1;` (`AIUpdate.cpp:2290-2293`) brings the counter back to 1. Revert check: remove the clamp
    // from finish_locomotor_frame and the counter stays at 6.
    let (mut g, id) = unit_moving(0.0);
    {
        let o = g.objects.get_mut(&id).unwrap();
        o.ai.desired_speed = 0.5;
        o.ai.blocked_frames = 5;
        o.ai.is_blocked = true;
    }
    g.ai_do_locomotor(id);
    assert_eq!(g.objects[&id].ai.blocked_frames, 1, "a clear frame keeps the counter at one");
}

#[test]
fn a_blocked_counter_keeps_counting_while_the_unit_still_turns() {
    // The same counter while the kernel's output stays TRUE: the unit faces away from its goal and must turn, and
    // its speed (0.5) is above the blocked cap (0.2), so `blocked` stays TRUE (`AIUpdate.cpp:2212`, the `if` arm).
    // The turn keeps the output TRUE, so the clamp does not run and the counter grows (`:2290`).
    let (mut g, id) = unit_moving(1.0);
    {
        let o = g.objects.get_mut(&id).unwrap();
        o.angle = PI;
        o.ai.desired_speed = 0.5;
        o.ai.cur_max_blocked_speed = 0.2;
        o.ai.blocked_frames = 5;
        o.ai.is_blocked = true;
    }
    g.ai_do_locomotor(id);
    assert_eq!(g.objects[&id].ai.blocked_frames, 6, "a unit still turning out of its block keeps counting");
}

#[test]
fn an_explicit_goal_resets_the_blocked_speed_cap() {
    // `m_curMaxBlockedSpeed = FAST_AS_POSSIBLE` runs for every goal type (`AIUpdate.cpp:2301`), so a jet takeoff or
    // face move does not leave a low cap for a later `calculateMaxBlockedSpeed` `min()`. Revert check: move the
    // reset back to the ground path (after the speed clamp) and the cap stays at 0.1 here.
    let (mut g, id) = unit_moving(0.0);
    {
        let o = g.objects.get_mut(&id).unwrap();
        o.ai.locomotor_goal_type = LocoGoalType::PositionExplicit;
        o.ai.locomotor_goal_data = Coord3D::new(150.0, 50.0, 0.0);
        o.ai.cur_max_blocked_speed = 0.1;
    }
    g.ai_do_locomotor(id);
    assert_eq!(g.objects[&id].ai.cur_max_blocked_speed, FAST_AS_POSSIBLE);
}

#[test]
fn a_move_waiting_for_its_path_returns_before_the_blocked_speed_reset() {
    // `if (!getPath()) { if (m_waitingForPath) return UPDATE_SLEEP_FOREVER; }` (`AIUpdate.cpp:2177-2179`): the
    // tail, including the reset, does not run.
    let (mut g, id) = unit_moving(0.0);
    {
        let o = g.objects.get_mut(&id).unwrap();
        o.ai.path.clear();
        o.ai.waiting_for_path = true;
        o.ai.cur_max_blocked_speed = 0.1;
    }
    g.ai_do_locomotor(id);
    assert_eq!(g.objects[&id].ai.cur_max_blocked_speed, 0.1, "the waiting return skips the reset");
}

/// `processCollision` runs for each contact with `selfMoving = isMoving()` read at that contact (`AIUpdate.cpp:1444`).
/// A tank (A) moving along +x pushes an infantry (X) that stands in its way, and X holds a queued path request. X's main
/// machine is `AI_IDLE`, and `isIdle` reads the main machine only (`AIUpdate.cpp:3120-3127`), so `isMoving` is FALSE
/// (`:3196-3198`) and `processCollision` returns before it blocks X on its contact with Z (`:1444-1448`). Lane 163's version
/// assumed X counted as moving from that request (the port's old `has_path || waiting` reader).
/// Revert check: the old reader makes X moving, so X is blocked by Z.
#[test]
fn a_move_away_under_an_idle_main_state_is_not_moving_so_its_contact_is_not_processed() {
    let mut t = ObjectTemplate::new("Tank");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 5.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.physics = Some(PhysicsTemplate { mass: 10.0, ..PhysicsTemplate::default() });
    t.locomotor = Some(LocomotorParams::from_engine_units(LocomotorKind::Treads, 1.0, 0.0, 0.1, 0.5, 0.5, true));
    let mut inf = ObjectTemplate::new("Inf");
    inf.kind_of = kind_of::INFANTRY | kind_of::SELECTABLE;
    inf.geometry.major_radius = 4.0;
    inf.max_health = 100.0;
    inf.initial_health = 100.0;
    inf.physics = Some(PhysicsTemplate { mass: 5.0, ..PhysicsTemplate::default() });
    inf.locomotor = Some(LocomotorParams::from_engine_units(LocomotorKind::Legs, 0.5, 0.0, 0.1, 0.5, 0.5, true));
    let (tank, inf) = (Arc::new(t), Arc::new(inf));
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    let a = g.create_object(&tank, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let x = g.create_object(&inf, Coord3D::new(106.0, 100.0, 0.0), 0.0, 0);
    let _z = g.create_object(&tank, Coord3D::new(113.0, 103.0, 0.0), 0.0, 0);
    {
        let o = g.objects.get_mut(&a).unwrap();
        o.ai.path = vec![Coord3D::new(200.0, 100.0, 0.0)];
        o.ai.locomotor_goal_type = LocoGoalType::PositionOnPath;
        o.physics.as_mut().unwrap().velocity = Coord3D::new(1.0, 0.0, 0.0);
    }
    assert!(!g.objects[&x].ai.is_moving(), "X stands still at the start of the pass");
    // Each contact's `onCollide` runs `processCollision` for its object (`PhysicsUpdate.cpp:1278`), in this order.
    let zero = Coord3D::new(0.0, 0.0, 0.0);
    for (me, other) in [(a, x), (a, _z), (x, a), (_z, a), (x, _z), (_z, x)] {
        g.on_collide(me, other, zero, zero);
    }
    assert!(!g.objects[&x].ai.is_moving(), "its main machine is AI_IDLE, so it is not isMoving");
    assert!(!g.objects[&x].ai.is_blocked, "X is not moving, so its contact with Z is not processed");
}

#[test]
fn an_allied_player_is_not_crushed_but_a_neutral_one_is() {
    // `Object::canCrushOrSquish` (`Object.cpp`): `getRelationship(otherObj) == ALLIES` returns FALSE ("Friends don't
    // let friends crush friends") before the crush levels are compared. A level-3 crusher over a level-0 vehicle
    // crushes a neutral player's unit and not an allied player's. Revert check: compare the players only
    // (`a.player == b.player`) and the allied case reads as crushable.
    let mut heavy = ObjectTemplate::new("Heavy");
    heavy.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    heavy.geometry.major_radius = 5.0;
    heavy.max_health = 100.0;
    heavy.initial_health = 100.0;
    heavy.physics = Some(PhysicsTemplate { mass: 10.0, ..PhysicsTemplate::default() });
    heavy.crush.crusher_level = 3;
    let mut victim = ObjectTemplate::new("Victim");
    victim.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    victim.geometry.major_radius = 5.0;
    victim.max_health = 100.0;
    victim.initial_health = 100.0;
    victim.physics = Some(PhysicsTemplate { mass: 10.0, ..PhysicsTemplate::default() });
    let (heavy, victim) = (Arc::new(heavy), Arc::new(victim));
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    let me = g.create_object(&heavy, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let neutral = g.create_object(&victim, Coord3D::new(104.0, 100.0, 0.0), 0.0, 1);
    let ally = g.create_object(&victim, Coord3D::new(108.0, 100.0, 0.0), 0.0, 2);
    g.set_relationship_symmetric(0, 2, crate::damage::Relationship::Allies);
    assert!(g.can_crush_or_squish(me, neutral), "a neutral player's unit is crushed");
    assert!(!g.can_crush_or_squish(me, ally), "an allied player's unit is not crushed");
}

#[test]
fn a_waypoint_leg_with_a_link_ahead_adjusts_its_first_goal_before_the_flag_is_cleared() {
    // `AIFollowWaypointPathState::onEnter` (AIStates.cpp:4070-4090): the base `AIInternalMoveToState::onEnter` adjusts
    // the waypoint with the state's default flag (AIStateMachine.h:257), and only then `setAdjustsDestination(false)`
    // for a link ahead (`hasNextWaypoint`). The port skipped the adjust for any walk with a link ahead.
    // The waypoint (105, 105) under a 20-unit footprint adjusts to the non-centred cell centre (110.5, 110.5), as in
    // `a_plain_paths_first_point_adjusts_its_goal` (ai_states.rs). Revert check: the `!continues` condition on the
    // base call leaves the goal unset (`ai_pathfind_goal` None).
    let mut t = ObjectTemplate::new("Walker");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.geometry.major_radius = 10.0;
    t.physics = Some(PhysicsTemplate { mass: 10.0, ..PhysicsTemplate::default() });
    t.locomotor = Some(LocomotorParams::from_engine_units(LocomotorKind::Treads, 1.0, 0.0, 0.1, 0.5, 0.5, true));
    let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
    let id = g.create_object(&Arc::new(t), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    g.run_frames(1);
    let walk = crate::ai_states::WalkAt { waypoint: 1, open: true };
    g.ai_set_follow_path(id, vec![Coord3D::new(105.0, 105.0, 0.0)], crate::ai_states::AiStateType::FollowWaypointPath, None, Some(walk));
    let goal = g.ai_pathfind_goal(id).expect("the waypoint's adjusted goal cell is recorded");
    assert!((goal.x - 110.5).abs() < 1e-3 && (goal.y - 110.5).abs() < 1e-3, "adjusted goal {goal:?}");
    assert!(!g.objects[&id].ai.machine.move_adjusts, "a link ahead turns the adjust off after the base call");
}
