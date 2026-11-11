//! Lane 168 (wave 19, the R18 review): the main/temporary move-field ownership (R18-1..R18-4), a sleeping main under a
//! temporary (R18-5), the last point of a follow path (R18-10, R18-11) and the converted move's fields (R18-6). Each test
//! names its finding and the revert check it was run with. This file is a child of `ai_states`, so it reaches the
//! machine's private helpers.

use super::*;
use crate::skirmish_ai::SkirmishAiConfig;
use crate::template::{kind_of, LocomotorKind, LocomotorParams, ObjectTemplate};
use crate::terrain::TerrainLogic;
use std::sync::Arc;

fn walker() -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new("Walker");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.geometry.major_radius = 5.0;
    t.physics = Some(crate::physics::PhysicsTemplate { mass: 10.0, ..crate::physics::PhysicsTemplate::default() });
    t.locomotor = Some(LocomotorParams::from_engine_units(LocomotorKind::Treads, 1.0, 0.0, 0.2, 0.0556, 0.1, true));
    Arc::new(t)
}

/// An airborne unit (`Wings`): `is_doing_ground_movement` is false for it (`AIUpdate.cpp:2362`).
fn flier() -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new("Flier");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.geometry.major_radius = 5.0;
    t.physics = Some(crate::physics::PhysicsTemplate { mass: 10.0, ..crate::physics::PhysicsTemplate::default() });
    t.locomotor = Some(LocomotorParams::from_engine_units(LocomotorKind::Wings, 1.0, 0.0, 0.2, 0.05, 0.1, false));
    Arc::new(t)
}

fn world() -> GameLogic {
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 10000);
    g
}

/// A move-away has a path to a point beside the unit (`AIUpdate.cpp:3333-3335`); the temporary state reads its end.
fn with_move_away_path(g: &mut GameLogic, id: ObjectId) {
    if let Some(o) = g.objects.get_mut(&id) {
        o.ai.path = vec![Coord3D::new(80.0, 50.0, 0.0)];
    }
}

/// R18-1 (`AIUpdate.cpp:2912-2918`): a move the player orders while a move-away runs is the main move once the
/// move-away ends. Before the fix the new main's goal was written into the temporary's set and the end of the
/// temporary restored the main's old set over it. Revert check: remove the `leave_temp_fields` at the end of
/// `ai_set_temporary_state` and the `with_main_move_fields` wrap in `ai_internal_set_state`; the goal is then lost.
#[test]
fn a_move_ordered_during_a_move_away_is_the_main_move_once_it_ends() {
    let mut g = world();
    let u = g.create_object(&walker(), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    with_move_away_path(&mut g, u);
    g.ai_set_temporary_state(u, AiStateType::MoveOutOfTheWay, 300);
    let target = Coord3D::new(300.0, 300.0, 0.0);
    g.ai_move_to_position_cmd(u, target, CommandSource::Player);
    assert_eq!(g.find_object(u).unwrap().ai.machine.current, AiStateType::MoveTo);
    if let Some(o) = g.objects.get_mut(&u) {
        o.ai.machine.temporary_end = 0;
    }
    g.frame = 5;
    g.ai_machine_update(u);
    let o = g.find_object(u).unwrap();
    assert_eq!(o.ai.machine.temporary, None, "the move-away has ended");
    let goal = o.ai.move_goal.expect("the ordered move has its goal");
    assert!(goal.dist_2d(&target) < 10.0, "the player's move is the main move's goal: {goal:?}");
}

/// R18-1, the other route: a main state changed from a temporary state's own code gets the main move's fields. The
/// mood conversion inside a dock move does this (`AIMoveToState::update`, `AIStates.cpp:2087`). Revert check: remove
/// the `with_main_move_fields` wrap from `ai_internal_set_state`; the attack move's entry then writes the dock move's
/// set, and the dock move's end restores the main's goal.
#[test]
fn a_main_state_changed_from_a_temporary_code_gets_the_main_moves_fields() {
    let mut g = world();
    let u = g.create_object(&walker(), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    let main_goal = Coord3D::new(300.0, 300.0, 0.0);
    g.ai_move_to_position_cmd(u, main_goal, CommandSource::Player);
    let dock = Coord3D::new(120.0, 80.0, 0.0);
    if let Some(o) = g.objects.get_mut(&u) {
        o.ai.machine.goal_position = dock;
    }
    g.ai_set_temporary_state(u, AiStateType::MoveTo, 300);
    // The temporary's own code runs (its set is live) and the mood conversion makes the attack move the main state.
    g.enter_temp_fields(u);
    let attack = Coord3D::new(200.0, 60.0, 0.0);
    g.ai_attack_move_to_position(u, attack);
    g.leave_temp_fields(u);
    g.ai_end_temporary_state(u, AiStateType::MoveTo);
    let o = g.find_object(u).unwrap();
    assert_eq!(o.ai.machine.current, AiStateType::AttackMoveTo, "the attack move is the main state");
    assert!(o.ai.move_waiting_for_path, "the attack move's entry computed a path, so it waits for it");
    let goal = o.ai.move_goal.expect("the attack move has a goal");
    assert!(goal.dist_2d(&attack) < 10.0, "its goal is the attack point, not the main move's: {goal:?}");
}

/// R18-2 (`AIUpdate.cpp:3259-3263`): `privateMoveAwayFromUnit` writes only `m_path`; no state's `m_pathGoalPosition`
/// changes, so the main move keeps its own. Revert check: write `path_goal_position` again in `ai_move_away_from_unit`
/// before `ai_set_temporary_state`.
#[test]
fn a_move_away_leaves_the_main_moves_path_goal_alone() {
    let mut g = world();
    let mover = g.create_object(&walker(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let other = g.create_object(&walker(), Coord3D::new(110.0, 100.0, 0.0), 0.0, 0);
    let main_path_goal = Coord3D::new(150.0, 150.0, 0.0);
    if let Some(o) = g.objects.get_mut(&mover) {
        o.ai.path_goal_position = main_path_goal;
    }
    assert!(g.compute_quick_path(other, &Coord3D::new(200.0, 100.0, 0.0)));
    g.ai_move_away_from_unit(mover, other);
    let o = g.find_object(mover).unwrap();
    assert_eq!(o.ai.machine.temporary, Some(AiStateType::MoveOutOfTheWay), "the move-away started");
    assert_eq!(o.ai.path_goal_position, main_path_goal, "the main move's path goal is not the move-away's end point");
}

/// R18-3 (`AIUpdate.cpp:3092-3098`, `privateIdle`): stopping a unit clears its main move's goal, and a running
/// temporary keeps its own. `privateIdle` runs the machine's `clear()` (which runs the current state's exit and
/// drops the machine's goal) and then enters `AI_IDLE`; lane 172 replaced the old `stop_object_ai` shortcut with
/// that path, so this test drives `ai_idle` (`AIUpdate.cpp:3092-3117`). Revert check: remove the
/// `leave_temp_fields` at the end of `ai_set_temporary_state`; the parked set is then the main's, and the
/// move-away's goal is gone.
#[test]
fn a_stop_during_a_move_away_leaves_the_move_aways_goal() {
    let mut g = world();
    let u = g.create_object(&walker(), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    with_move_away_path(&mut g, u);
    g.ai_set_temporary_state(u, AiStateType::MoveOutOfTheWay, 300);
    let away = g.find_object(u).unwrap().ai.machine.parked_move.and_then(|p| p.move_goal);
    assert!(away.is_some(), "the move-away's goal is parked while the main move's fields are live");
    g.ai_idle(u, CommandSource::Ai);
    let after = g.find_object(u).unwrap().ai.machine.parked_move.and_then(|p| p.move_goal);
    assert_eq!(after, away, "the stop clears the main move, not the running move-away's goal");
}

/// R18-4 (`AIStates.cpp:951-955`): a temporary state whose enter returns anything but CONTINUE ends at once, with its
/// exit run, and the main move is live again. `GuardGetCrate` enters with SUCCESS. Revert check: change the enter test
/// in `ai_set_temporary_state` back to `== StateReturn::Failure`; the temporary then stays set.
#[test]
fn a_temporary_whose_enter_succeeds_ends_at_once() {
    let mut g = world();
    let u = g.create_object(&walker(), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    g.ai_move_to_position_cmd(u, Coord3D::new(300.0, 300.0, 0.0), CommandSource::Player);
    let goal = g.find_object(u).unwrap().ai.move_goal;
    g.ai_set_temporary_state(u, AiStateType::GuardGetCrate, 90);
    let o = g.find_object(u).unwrap();
    assert_eq!(o.ai.machine.temporary, None, "a success from the enter ends the temporary");
    assert_eq!(o.ai.move_goal, goal, "the main move is live again");
    assert_eq!(o.ai.machine.parked_move, None, "nothing is parked");
}

/// R18-5 (`StateMachine.cpp:414-425`): a main still asleep when its temporary ends stays asleep until its wake frame;
/// `m_sleepTill` is cleared only when the main runs. Revert check: clear `sleep_till` before the temporary block of
/// `ai_machine_update` (the old order).
#[test]
fn a_main_asleep_when_its_temporary_ends_stays_asleep() {
    let mut g = world();
    let u = g.create_object(&walker(), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    g.frame = 10;
    g.ai_set_temporary_state(u, AiStateType::Wait, 1);
    if let Some(o) = g.objects.get_mut(&u) {
        o.ai.machine.sleep_till = 500;
    }
    g.frame = 12;
    g.ai_machine_update(u);
    let m = &g.find_object(u).unwrap().ai.machine;
    assert_eq!(m.temporary, None, "the temporary has ended");
    assert_eq!(m.sleep_till, 500, "the main is still asleep");
}

/// R18-10 (`AIStates.cpp:3308-3311`): an exit-production path with one point adjusts its destination, because the
/// base call's flag is set back to `m_adjustFinal` (TRUE) when there is no next point. Revert check: drop the
/// `else if let Some(o)` branch of `ai_follow_path_enter_with`.
#[test]
fn an_exit_production_path_with_one_point_adjusts_its_destination() {
    let mut g = world();
    let u = g.create_object(&walker(), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    if let Some(o) = g.objects.get_mut(&u) {
        o.ai.machine.goal_path = vec![Coord3D::new(120.0, 120.0, 0.0)];
    }
    g.ai_set_state(u, AiStateType::FollowExitProductionPath);
    let o = g.find_object(u).unwrap();
    assert_eq!(o.ai.machine.current, AiStateType::FollowExitProductionPath);
    assert!(o.ai.machine.move_adjusts, "the single exit point adjusts its destination");
}

/// R18-11 (`AIStates.cpp:3403`): the last point of a follow path adjusts only when `m_adjustFinal && (override ||
/// isDoingGroundMovement())`. An airborne unit walks to its last point without adjusting it. Revert check: drop
/// `&& self.is_doing_ground_movement(id)` from the `adjusts` of `ai_follow_path_update`.
#[test]
fn an_airborne_unit_does_not_adjust_the_last_point_of_its_path() {
    let mut g = world();
    let u = g.create_object(&flier(), Coord3D::new(60.0, 50.0, 0.0), 0.0, 0);
    if let Some(o) = g.objects.get_mut(&u) {
        o.ai.machine.goal_path = vec![Coord3D::new(60.0, 50.0, 0.0), Coord3D::new(120.0, 50.0, 0.0)];
    }
    g.ai_set_state(u, AiStateType::FollowPath);
    for _ in 0..300 {
        g.run_frames(1);
        if g.find_object(u).unwrap().ai.machine.goal_path_index == 1 {
            break;
        }
    }
    let o = g.find_object(u).unwrap();
    assert_eq!(o.ai.machine.goal_path_index, 1, "the flier reached the segment to its last point");
    assert!(!o.ai.machine.move_adjusts, "an airborne unit does not adjust its last point");
}

/// R18-6 (`AIStates.cpp:2086-2100`): the converting move updates on its own fields, not the attack move's. Its waiting
/// flag is not the attack move's, so no timestamp of the converting move is written from the attack move's wait. The
/// attack move's own fields are live once the conversion frame ends. Revert check: drop the `apply_move_fields(id, own)`
/// in `ai_move_to_update`, so the converting move reads the attack move's waiting flag and writes its timestamp.
#[test]
fn a_converted_move_updates_on_its_own_fields() {
    let mut g = world();
    g.add_ai_player(0, SkirmishAiConfig::default());
    let u = g.create_object(&walker(), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    g.ai_move_to_position_cmd(u, Coord3D::new(300.0, 300.0, 0.0), CommandSource::Player);
    if let Some(o) = g.objects.get_mut(&u) {
        o.ai.attitude = 1;
        o.ai.move_waiting_for_path = false;
        o.ai.move_path_timestamp = 0;
    }
    g.frame = 100;
    g.ai_move_to_update(u);
    let o = g.find_object(u).unwrap();
    assert_eq!(o.ai.machine.current, AiStateType::AttackMoveTo, "the mood converted the move");
    assert_ne!(o.ai.move_path_timestamp, 100, "the converting move did not take the attack move's waiting flag");
}
