//! Lane 160 (wave 19): a temporary move is its own state object in C++, so its goal cannot change the main move's
//! (`AIInternalMoveToState::m_goalPosition`, `m_pathGoalPosition` and the repath clocks live on each state, AIStateMachine.h:247-300).
//! AN1 s1 China dozer 230: the move-away (`AIMoveOutOfTheWayState`, `AIUpdate.cpp:3253-3263`) that stepped it aside at
//! f1678 overwrote the dock move's goal in the port. When the move-away ended at f1800 the dock move re-pathed to the
//! move-away's end point instead of its dock, and the dozer went idle at f1802 (C++ re-paths to the dock at f1801).

use super::*;
use crate::ai_states::{AiStateType, CommandSource};
use crate::template::{LocomotorKind, LocomotorParams, ObjectTemplate};
use std::sync::Arc;

fn walker() -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new("Walker");
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.geometry.major_radius = 5.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 60.0, 100.0, 100.0, 360.0));
    Arc::new(t)
}

/// The main move keeps its goal (and its repath clocks) while a move-away runs, and gets them back when the
/// move-away ends (`AIStateMachine::updateStateMachine` runs the temporary state's exit and then the main update,
/// `AIStates.cpp:850-879`). The move-away's own goal is on its own state object in C++ (AIStateMachine.h:247-300), so
/// between its runs the main move's fields are live and the move-away's set is parked (`AiMachine::parked_move`).
/// Revert check: leave the `leave_temp_fields` call out of `ai_set_temporary_state`, and the main's goal becomes the
/// move-away's end point while it runs.
#[test]
fn a_temporary_move_away_leaves_the_main_moves_goal_alone() {
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 0);
    let u = g.create_object(&walker(), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    let dock = Coord3D::new(300.0, 300.0, 0.0);
    g.ai_move_to_position_cmd(u, dock, CommandSource::Player);
    let main_goal = g.find_object(u).unwrap().ai.move_goal.expect("the dock move has a goal");
    let main_path_goal = g.find_object(u).unwrap().ai.path_goal_position;
    // `getMoveAwayFromPath` leaves a path to a point beside the unit (`AIUpdate.cpp:3253`, `:3333-3335`).
    let away = Coord3D::new(80.0, 50.0, 0.0);
    if let Some(o) = g.objects.get_mut(&u) {
        o.ai.path = vec![away];
    }
    g.ai_set_temporary_state(u, AiStateType::MoveOutOfTheWay, 300);
    let parked = g.find_object(u).unwrap();
    assert_eq!(parked.ai.move_goal, Some(main_goal), "the move-away leaves the main move's goal alone");
    let away_goal = parked.ai.machine.parked_move.and_then(|p| p.move_goal).expect("the move-away's goal is parked");
    assert!(away_goal.dist_2d(&main_goal) > 20.0, "the move-away's own goal is not the dock: {away_goal:?}");
    // The move-away ends (its time is up): the dock move is the main state again, with its own goal.
    if let Some(o) = g.objects.get_mut(&u) {
        o.ai.machine.temporary_end = 0;
    }
    g.frame = 5;
    g.ai_machine_update(u);
    let o = g.find_object(u).unwrap();
    assert_eq!(o.ai.machine.temporary, None, "the move-away has ended");
    assert_eq!(o.ai.move_goal, Some(main_goal), "the dock move resumes with its own goal, not the move-away's");
    assert_eq!(o.ai.path_goal_position, main_path_goal, "and its own path goal");
}

/// A main state entered during a temporary move keeps the fields its entry wrote. The mood conversion inside a dock
/// move (`AIMoveToState::update`, `AIStates.cpp:2087`) calls `aiAttackMoveToPosition`, which sets `AI_ATTACK_MOVE_TO`
/// as the main state (`privateAttackMoveToPosition`, `AIUpdate.cpp:3595-3615`); its `onEnter` computes a path, so
/// `m_waitingForPath` is true. When the dock move ends that flag and goal belong to the attack move (AN3 s1 dozer 230,
/// f1726: C++ waits for the attack move's path, the port's restore dropped the wait and went Idle a frame early).
/// Revert check: call `ai_attack_move_to_position` without `with_main_move_fields` and the restored main has the
/// pre-temporary goal and a clear waiting flag.
#[test]
fn a_main_entered_during_a_temporary_move_keeps_its_fields() {
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 0);
    let u = g.create_object(&walker(), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    let main_dock = Coord3D::new(300.0, 300.0, 0.0);
    g.ai_move_to_position_cmd(u, main_dock, CommandSource::Player);
    // The dock move (a temporary `AI_MOVE_TO`, `privateMoveToPosition` with CMD_FROM_AI while not idle).
    let dock = Coord3D::new(120.0, 80.0, 0.0);
    if let Some(o) = g.objects.get_mut(&u) {
        o.ai.machine.goal_position = dock;
    }
    g.ai_set_temporary_state(u, AiStateType::MoveTo, 300);
    // The mood conversion turns the dock move into an attack move toward `attack`.
    let attack = Coord3D::new(200.0, 60.0, 0.0);
    g.ai_attack_move_to_position(u, attack);
    assert_eq!(g.find_object(u).unwrap().ai.machine.current, AiStateType::AttackMoveTo);
    // The dock move ends: the attack move is still the main state and keeps its own move (the entry's goal and wait).
    g.ai_end_temporary_state(u, AiStateType::MoveTo);
    let o = g.find_object(u).unwrap();
    assert_eq!(o.ai.machine.current, AiStateType::AttackMoveTo, "the attack move is still the main state");
    assert!(o.ai.move_waiting_for_path, "the attack move's entry computed a path, so it waits for it");
    let goal = o.ai.move_goal.expect("the attack move has a goal");
    assert!(goal.dist_2d(&attack) < 10.0, "its goal is the attack point (adjusted by a cell at most), not the old main move's: {goal:?}");
}
