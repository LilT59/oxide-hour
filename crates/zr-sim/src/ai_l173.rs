//! Lane 173 (wave 20): fixes for the R19 adversarial review. Each test names its finding and the revert check it
//! was run with. This file is a child of `ai_states`, so it reaches the machine's private helpers.

use super::*;
use crate::template::{kind_of, LocomotorKind, LocomotorParams, ObjectTemplate};
use crate::terrain::TerrainLogic;
use crate::weapon::{CombatTemplate, WeaponSetTemplate, WeaponTemplate};
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

fn world() -> GameLogic {
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 10000);
    g
}

/// An armed guard: a treads vehicle with one 150-range gun and 200 vision.
fn armed() -> Arc<ObjectTemplate> {
    let mut w = WeaponTemplate::new("Gun");
    w.primary_damage = 20.0;
    w.attack_range = 150.0;
    w.min_delay_between_shots = 30;
    w.max_delay_between_shots = 30;
    let mut set = WeaponSetTemplate::default();
    set.weapons[0] = Some(Arc::new(w));
    let mut t = ObjectTemplate::new("Guard");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
    t.geometry.major_radius = 8.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.vision_range = 200.0;
    t.combat = CombatTemplate { weapon_set: Some(set), ..CombatTemplate::default() };
    t.locomotor = Some(LocomotorParams::from_engine_units(LocomotorKind::Treads, 1.0, 0.0, 0.2, 0.0556, 0.1, true));
    Arc::new(t)
}

/// Give `id` a live move toward `goal` (path, last node, timestamps) so its `AIInternalMoveToState::update` returns
/// CONTINUE instead of reporting an arrival.
fn walking(g: &mut GameLogic, id: ObjectId, goal: Coord3D) {
    let start = g.find_object(id).map_or(goal, |o| o.position);
    let path = crate::pathfind::Path {
        nodes: vec![
            crate::pathfind::PathNode { pos: start, can_optimize: false, next_optimized: Some(1) },
            crate::pathfind::PathNode { pos: goal, can_optimize: false, next_optimized: None },
        ],
        blocked_by_ally: false,
    };
    if let Some(o) = g.objects.get_mut(&id) {
        o.ai.move_goal = Some(goal);
        o.ai.path = vec![goal];
        o.ai.raw_path = Some(path);
        o.ai.path_goal_position = goal;
        o.ai.move_waiting_for_path = false;
        o.ai.move_path_timestamp = g.frame;
        o.ai.locomotor_goal_type = LocoGoalType::PositionOnPath;
    }
}

/// R19-3 (`AIGuard.cpp:182-191`, the retail branch): `AI_GUARD_RETURN` and `AI_GUARD_INNER` carry
/// `attackAggressors`, and a state's conditions run on its CONTINUE returns (`StateMachine.cpp:158-192`). A guard
/// walking back to its post that is shot from beyond its vision therefore turns on the attacker instead of walking
/// on. Revert check: drop the CONTINUE-return `ai_guard_aggressor_condition` from `ai_guard_return_update`; the
/// guard stays in `GuardReturn`.
#[test]
fn a_returning_guard_returns_fire_at_its_aggressor() {
    let mut g = world();
    g.add_player(1, 0);
    // A computer guard: a human player's AI-commanded attack is refused its approach (AIStates.cpp:2609-2621, lane 190).
    g.add_ai_player(0, crate::skirmish_ai::SkirmishAiConfig::default());
    g.set_enemies(0, 1);
    let guard = g.create_object(&armed(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let enemy = g.create_object(&armed(), Coord3D::new(300.0, 100.0, 0.0), 0.0, 1);
    let goal = Coord3D::new(200.0, 200.0, 0.0);
    g.frame = 50;
    walking(&mut g, guard, goal);
    if let Some(o) = g.objects.get_mut(&guard) {
        o.ai.machine.current = AiStateType::Guard;
        o.ai.machine.guard_sub = AiStateType::GuardReturn;
        o.ai.machine.guard.position_to_guard = Coord3D::new(100.0, 100.0, 0.0);
        o.ai.machine.guard.target_to_guard = ObjectId::INVALID;
        o.ai.machine.guard.next_scan_time = 5000; // not a scan frame: the update returns CONTINUE
        o.last_damage_info.source = enemy;
        o.last_damage_cleared = false;
    }
    g.ai_machine_update(guard);
    assert_eq!(
        g.find_object(guard).unwrap().ai.machine.guard_sub,
        AiStateType::GuardAttackAggressor,
        "`hasAttackedMeAndICanReturnFire` fires on the RETURN state's CONTINUE return"
    );
}

/// R19-3, the INNER entry (`AIGuard.cpp:183`): the attack's `onEnter` returns CONTINUE, so `internalSetState` runs
/// INNER's conditions (`StateMachine.cpp:647-651`). Revert check: drop the `ai_guard_aggressor_condition` from
/// `ai_guard_inner_enter`.
#[test]
fn a_guard_in_its_inner_circle_returns_fire_at_its_aggressor() {
    let mut g = world();
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    let guard = g.create_object(&armed(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let enemy = g.create_object(&armed(), Coord3D::new(200.0, 100.0, 0.0), 0.0, 1);
    if let Some(o) = g.objects.get_mut(&guard) {
        o.ai.machine.current = AiStateType::Guard;
        o.ai.machine.guard_sub = AiStateType::GuardInner;
        o.ai.machine.guard.position_to_guard = Coord3D::new(100.0, 100.0, 0.0);
        o.ai.machine.guard.target_to_guard = ObjectId::INVALID;
        o.ai.machine.guard.nemesis = enemy;
        o.last_damage_info.source = enemy;
        o.last_damage_cleared = false;
    }
    g.ai_guard_inner_enter(guard);
    assert_eq!(
        g.find_object(guard).unwrap().ai.machine.guard_sub,
        AiStateType::GuardAttackAggressor,
        "the INNER entry's CONTINUE result runs the state's attackAggressors"
    );
}

/// R19-4 (`AIGuard.cpp:745-777`): the idle scan is crate -> `lookForInnerTarget` -> guardee moved -> sleep, and only
/// the sleep return runs the conditions, so the guardee's move is recorded (and the RETURN entry's draw happens)
/// before `attackAggressors` can take over. Revert check: put the aggressor test back in front of
/// `ai_guard_look_for_target`; the guardee position stays stale.
#[test]
fn an_idle_guard_whose_guardee_moved_records_it_before_it_returns_fire() {
    let mut g = world();
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    let guard = g.create_object(&armed(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let ward = g.create_object(&armed(), Coord3D::new(300.0, 300.0, 0.0), 0.0, 0);
    // Beyond the guard circle centred on the ward (200 * 1.8 = 360), so `lookForInnerTarget` finds nothing and the
    // guardee check runs.
    let enemy = g.create_object(&armed(), Coord3D::new(20.0, 20.0, 0.0), 0.0, 1);
    g.frame = 50;
    if let Some(o) = g.objects.get_mut(&guard) {
        o.ai.machine.current = AiStateType::Guard;
        o.ai.machine.guard_sub = AiStateType::GuardIdle;
        o.ai.machine.guard.target_to_guard = ward;
        o.ai.machine.guard.position_to_guard = Coord3D::new(100.0, 100.0, 0.0);
        o.ai.machine.guard.guardee_pos = Coord3D::new(100.0, 100.0, 0.0); // where it stood when the guard last looked
        o.ai.machine.guard.next_scan_time = 50; // the scan frame
        o.last_damage_info.source = enemy;
        o.last_damage_cleared = false;
    }
    g.ai_machine_update(guard);
    let o = g.find_object(guard).unwrap();
    // Lane 186: the aggressor's attack entry already fails this enemy, which lies outside the guard circle (396 > 360).
    // `AIAttackState::onEnter` returns SUCCESS when `shouldExit` already holds (AIStates.cpp:5527-5528), so the aggressor
    // hands over to `AI_GUARD_INNER` (AIGuard.cpp:194), whose entry does the same and hands over to `AI_GUARD_OUTER`
    // (AIGuard.cpp:183). The guardee check below is unchanged; it is what the test is about.
    assert_eq!(o.ai.machine.guard_sub, AiStateType::GuardOuter, "the aggressor's entry gives up at once and the guard falls through to the outer state");
    assert_eq!(
        o.ai.machine.guard.guardee_pos,
        Coord3D::new(300.0, 300.0, 0.0),
        "the guardee's move is recorded before the sleep's aggressor test"
    );
}

/// R19-1 (`AIStates.cpp:3664-3674`): on the frame the attack move's retry sleep expires, the engine runs
/// `AIMoveToState::onEnter()` (which requests a fresh path, `AIStates.cpp:1671`) and `forceRepath()` before the
/// update. Revert check: drop the `sleeping == now` block; the finished path is re-run, no request is made and the
/// state sleeps again without moving.
#[test]
fn an_attack_move_waking_from_its_retry_sleep_requests_a_new_path() {
    let mut g = world();
    let u = g.create_object(&walker(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let goal = Coord3D::new(200.0, 100.0, 0.0);
    let path = crate::pathfind::Path {
        nodes: vec![
            crate::pathfind::PathNode { pos: Coord3D::new(100.0, 100.0, 0.0), can_optimize: false, next_optimized: Some(1) },
            crate::pathfind::PathNode { pos: goal, can_optimize: false, next_optimized: None },
        ],
        blocked_by_ally: false,
    };
    g.frame = 100;
    if let Some(o) = g.objects.get_mut(&u) {
        o.ai.machine.current = AiStateType::AttackMoveTo;
        o.ai.machine.goal_position = goal;
        o.ai.machine.attack_move_goal = goal;
        o.ai.move_goal = Some(goal);
        o.ai.path = vec![goal];
        o.ai.raw_path = Some(path);
        o.ai.path_goal_position = goal;
        o.ai.machine.attack_move_path_goal = goal;
        o.ai.move_waiting_for_path = false;
        o.ai.move_path_timestamp = 0;
        o.ai.locomotor_goal_type = LocoGoalType::PositionOnPath;
        // The state slept after a failed retry (`AIStates.cpp:3686-3689`); this frame is the wake.
        o.ai.machine.attack_move_sleep_until = 100;
        o.ai.machine.attack_move_retry = 2;
    }
    let _ = g.ai_attack_move_update(u);
    assert!(g.pathfinder.is_queued_for_path(u), "the wake's `AIMoveToState::onEnter` requests a path");
    assert_eq!(
        g.find_object(u).unwrap().ai.machine.attack_move_path_goal,
        Coord3D::new(-100.0, -100.0, -100.0),
        "`forceRepath` writes the state's own path goal (`AIStateMachine.h:277`)"
    );
}

/// R19-10 (`AIStates.cpp:2086-2100`): the mood conversion's `aiAttackMoveToPosition` makes a new main state, but the
/// *converting* move's `update` continues on its own fields (`m_goalPosition`, `m_waitingForPath`, `m_pathTimestamp`
/// belong to the old state object). R18-6's `apply_move_fields(id, own)` is what keeps them, and this is the proof
/// test: on the conversion frame the converting move's own arrival test runs, so `AIMoveToState::update` returns
/// SUCCESS and `getAdjustsDestination()` clears the locomotor goal (`AIStates.cpp:1929-1932`). Revert check: drop the
/// `apply_move_fields(id, own)` in `ai_move_to_update`; the update then reads the attack move's waiting flag and
/// returns CONTINUE with the goal still `POSITION_ON_PATH`.
#[test]
fn a_converted_move_reaches_its_own_arrival_on_the_conversion_frame() {
    use crate::skirmish_ai::SkirmishAiConfig;
    let mut g = world();
    g.add_ai_player(0, SkirmishAiConfig::default());
    let pos = Coord3D::new(100.0, 100.0, 0.0);
    let u = g.create_object(&walker(), pos, 0.0, 0);
    g.frame = 100;
    if let Some(o) = g.objects.get_mut(&u) {
        o.ai.attitude = 1; // the mood that turns moves into attack moves
        o.ai.move_goal = Some(pos);
        o.ai.path = vec![pos];
        o.ai.path_goal_position = pos;
        o.ai.move_waiting_for_path = false;
        o.ai.move_path_timestamp = 0;
        o.ai.machine.move_adjusts = true;
        o.ai.locomotor_goal_type = LocoGoalType::PositionOnPath;
    }
    let ret = g.ai_move_to_update(u);
    assert_eq!(ret, StateReturn::Success, "the converting move's own arrival test runs (`AIStates.cpp:1908-1939`)");
    assert_eq!(
        g.find_object(u).unwrap().ai.machine.current,
        AiStateType::AttackMoveTo,
        "and the conversion did make the attack move the main state"
    );
    assert_eq!(
        g.find_object(u).unwrap().ai.locomotor_goal_type,
        LocoGoalType::None,
        "`if (getAdjustsDestination()) ai->setLocomotorGoalNone()` (`AIStates.cpp:1929-1931`)"
    );
}

/// R19-2, second half (`AIStates.cpp:1881`): the attack move's `goal_moved` test reads its own `m_pathGoalPosition`
/// (`AiMachine::attack_move_path_goal`), not the shared live field. Revert check: make the `path_goal` of
/// `ai_internal_move_update` read `o.ai.path_goal_position` unconditionally; the stale value then reports a moved
/// goal and the state repaths.
#[test]
fn an_attack_move_judges_its_goal_against_its_own_path_goal() {
    let mut g = world();
    let u = g.create_object(&walker(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let goal = Coord3D::new(200.0, 100.0, 0.0);
    let path = crate::pathfind::Path {
        nodes: vec![
            crate::pathfind::PathNode { pos: Coord3D::new(100.0, 100.0, 0.0), can_optimize: false, next_optimized: Some(1) },
            crate::pathfind::PathNode { pos: goal, can_optimize: false, next_optimized: None },
        ],
        blocked_by_ally: false,
    };
    g.frame = 100;
    if let Some(o) = g.objects.get_mut(&u) {
        o.ai.machine.current = AiStateType::AttackMoveTo;
        o.ai.move_goal = Some(goal);
        o.ai.path = vec![goal];
        o.ai.raw_path = Some(path);
        o.ai.path_goal_position = Coord3D::ZERO; // the shared value belongs to another state (a temporary's leftover)
        o.ai.machine.attack_move_path_goal = goal;
        o.ai.move_waiting_for_path = false;
        o.ai.move_path_timestamp = 0;
        o.ai.locomotor_goal_type = LocoGoalType::PositionOnPath;
    }
    let _ = g.ai_internal_move_update(u);
    let o = g.find_object(u).unwrap();
    assert!(!g.pathfinder.is_queued_for_path(u), "no repath: the attack move's own path goal has not moved");
    assert_eq!(o.ai.move_path_timestamp, 0, "and no new path timestamp");
}

/// R19-2 (`AIUpdate.cpp:1767-1771`): `computePath`'s tail writes only the timestamp and the blocked flags; the
/// state object's `m_pathGoalPosition` is written by its wait block (`AIStates.cpp:1804`). The port wrote it from
/// `store_ai_path`, which `processPathfindQueue` runs after `updateStateMachine` — so a temporary move's finished
/// path landed on the main move's value, and the main move saw a moved goal (a spurious repath at
/// `AIStates.cpp:1881`). Revert check: write `o.ai.path_goal_position = goal` (the path request's goal) in
/// `store_ai_path` again; the main move's value becomes the temporary's and the repath is requested.
#[test]
fn a_temporary_path_does_not_change_the_main_moves_path_goal() {
    let mut g = world();
    let u = g.create_object(&walker(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    // The main move: a plain player MoveTo, serviced and then stamped by the state's wait block.
    let a = Coord3D::new(220.0, 100.0, 0.0);
    g.ai_move_to_position_cmd(u, a, CommandSource::Player);
    g.process_pathfind_queue();
    g.ai_machine_update(u);
    let main_path_goal = g.find_object(u).unwrap().ai.path_goal_position;
    assert!(main_path_goal.dist_2d(&a) < 20.0, "the main move stamped its own path goal: {main_path_goal:?}");

    // A temporary `AI_MOVE_TO` (`setTemporaryState`, AIStates.cpp:919), the dock-move shape, to a different point.
    g.frame = 20;
    let b = Coord3D::new(120.0, 200.0, 0.0);
    if let Some(o) = g.objects.get_mut(&u) {
        o.ai.machine.goal_position = b;
    }
    g.ai_set_temporary_state(u, AiStateType::MoveTo, 300);
    assert_eq!(g.find_object(u).unwrap().ai.machine.temporary, Some(AiStateType::MoveTo), "the temporary move runs");
    // Its path completes through the queue, while the main move's fields are the live ones (the port's stand-in for
    // a separate state object).
    g.process_pathfind_queue();
    let o = g.find_object(u).unwrap();    assert_eq!(o.ai.path_goal_position, main_path_goal, "the temporary's path does not touch the main move's path goal");

    // The temporary ends; the main move's next update must not see a moved goal and so must not repath.
    if let Some(o) = g.objects.get_mut(&u) {
        o.ai.machine.temporary_end = 0;
    }
    g.frame = 30;
    g.ai_machine_update(u);
    let o = g.find_object(u).unwrap();
    assert_eq!(o.ai.machine.temporary, None, "the temporary has ended");
    assert_eq!(o.ai.machine.current, AiStateType::MoveTo, "the main move is current again");
    assert!(!o.ai.waiting_for_path, "the main move did not request a new path");
    assert!(!g.pathfinder.is_queued_for_path(u), "and nothing of it is queued");
}
