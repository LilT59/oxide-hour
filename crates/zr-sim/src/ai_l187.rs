//! Lane 187 (wave 21, the R24 review). Crate-internal tests; each names its finding and the revert check it was run with.
//! This file is a child of `ai_states`, so it reaches the state functions and the `ai` items the states use.

use super::*;
use crate::template::{kind_of, LocomotorKind, LocomotorParams, ObjectTemplate};
use crate::terrain::TerrainLogic;
use std::sync::Arc;

fn tank() -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new("Tank");
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

/// R24-3 (`AIUpdate.cpp:426-433`, the approach branch): `doPathfind` installs `findClosestPath`'s result as `m_path` and
/// runs `updateGoal`, and writes neither `m_pathTimestamp` nor `m_blockedFrames` / `m_isBlockedAndStuck` (those are the
/// `computePath` and `computeAttackPath` tails, :1767-1771, :2003-2006). So a move requested one frame after an approach
/// path is not held by the three-frame anti-spin wait (`requestPath`, :499-503). Revert check: install the approach path
/// through `install_ai_path` (the tail) again; the path timestamp reads 50 and the move at frame 51 queues its wait.
#[test]
fn an_approach_install_writes_no_path_timestamp_so_the_next_move_is_not_held() {
    let mut g = world();
    let u = g.create_object(&tank(), Coord3D::new(100.0, 60.0, 0.0), 0.0, 0);
    g.frame = 50;
    if let Some(o) = g.objects.get_mut(&u) {
        o.ai.request_approach_path(&mut g.pathfinder, u, Coord3D::new(160.0, 60.0, 0.0));
    }
    g.process_pathfind_queue();
    let o = g.find_object(u).unwrap();
    assert!(o.ai.has_path(), "the approach path is installed");
    assert_eq!(o.ai.path_timestamp, 0, "the approach install writes no path timestamp (AIUpdate.cpp:426-433)");
    g.frame = 51;
    g.ai_move_to_position_cmd(u, Coord3D::new(160.0, 160.0, 0.0), CommandSource::Player);
    g.update();
    let o = g.find_object(u).unwrap();
    assert_eq!(o.ai.queue_for_path_frame, 0, "a move one frame later is not given the anti-spin wait (requestPath, AIUpdate.cpp:499-503)");
}

/// R24-4 (`AIUpdate.cpp:3252-3268`, `privateMoveAwayFromUnit`): the install is `destroyPath(); m_path = newPath;` with no
/// stuck-flag write. The temporary state's enter then runs `friend_startingMove`, which clears `m_isBlockedAndStuck`
/// (AIUpdate.cpp:2035-2038), so the flag is clear once the step aside has started. This pins that outcome. The removed
/// install write had no effect on this path; it differs only if the enter fails before `friend_startingMove`, which no
/// reachable fixture gives (the install always leaves a path, `AIStates.cpp:2154`). No discriminating revert test exists.
#[test]
fn a_step_aside_clears_the_stuck_flag_at_its_start_as_friend_starting_move_does() {
    let mut g = world();
    let mover = g.create_object(&tank(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let other = g.create_object(&tank(), Coord3D::new(110.0, 100.0, 0.0), 0.0, 0);
    assert!(g.compute_quick_path(other, &Coord3D::new(200.0, 100.0, 0.0)));
    if let Some(o) = g.objects.get_mut(&mover) {
        o.ai.is_blocked_and_stuck = true;
    }
    g.ai_move_away_from_unit(mover, other);
    let o = g.find_object(mover).unwrap();
    assert_eq!(o.ai.machine.temporary, Some(AiStateType::MoveOutOfTheWay), "the step aside started");
    assert!(!o.ai.is_blocked_and_stuck, "friend_startingMove clears the stuck flag when the step aside starts (AIUpdate.cpp:2038)");
}

/// R24-10 (`AIUpdate.cpp:543-549`, `requestApproachPath`): `m_isFinalGoal = TRUE`, whatever the state's adjust flag. The
/// `AIMoveAndTightenState` enter asks for an approach path with `setAdjustsDestination(false)` (AIStates.cpp:2236), so the
/// flag the request leaves must still be TRUE. Revert check: drop the `is_final_goal = true` after `start_move` in
/// `ai_move_and_tighten_enter`; the flag reads false.
#[test]
fn a_move_and_tighten_approach_request_is_a_final_goal() {
    let mut g = world();
    let u = g.create_object(&tank(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    if let Some(o) = g.objects.get_mut(&u) {
        o.ai.machine.goal_position = Coord3D::new(160.0, 100.0, 0.0);
    }
    let _ = g.ai_move_and_tighten_enter(u);
    let o = g.find_object(u).unwrap();
    assert_eq!(o.ai.request, crate::ai::PathRequest::Approach, "the tighten move asks for an approach path");
    assert!(o.ai.is_final_goal, "requestApproachPath sets m_isFinalGoal (AIUpdate.cpp:549)");
}

fn transport_contain(capacity: i32) -> crate::contain::ContainTemplate {
    use crate::contain::{ContainKind, ContainTemplate, GarrisonFirePoints};
    ContainTemplate {
        kind: ContainKind::Transport,
        capacity,
        allow_inside_kind_of: None,
        forbid_inside_kind_of: 0,
        allow_allies_inside: true,
        allow_enemies_inside: true,
        allow_neutral_inside: true,
        passengers_allowed_to_fire: false,
        damage_percentage_to_units: 0.0,
        number_of_exit_paths: 1,
        exit_delay: 0,
        door_open_time: 1,
        scatter_nearby_on_exit: true,
        orient_like_container_on_exit: false,
        reset_mood_check_on_exit: false,
        health_regen: 0.0,
        do_i_heal_objects: false,
        heal_frames: 0.0,
        initial_payload: Vec::new(),
        tunnel_frames_for_full_heal: 1.0,
        is_enclosing_container: false,
        fire_points: GarrisonFirePoints::default(),
        passengers_in_turret: false,
        weapon_bonus_passed_to_passengers: false,
        immune_to_clear_building_attacks: false,
    }
}

fn chinook_with_height(preferred: f32) -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Chinook");
    t.kind_of = kind_of::VEHICLE | kind_of::AIRCRAFT | kind_of::TRANSPORT | kind_of::SELECTABLE;
    t.geometry.major_radius = 20.0;
    t.geometry.height = 12.0;
    t.max_health = 300.0;
    t.initial_health = 300.0;
    let mut loco = LocomotorParams::from_ini(LocomotorKind::Hover, 120.0, 60.0, 180.0, 120.0);
    loco.behavior_z = crate::template::ZBehavior::SurfaceRelativeHeight;
    loco.preferred_height = preferred;
    t.locomotor = Some(loco);
    t.contain = Some(Arc::new(transport_contain(8)));
    t.chinook = Some(crate::chinook::ChinookTemplate {
        num_ropes: 2,
        rope_drop_speed: 1000.0,
        rappel_speed: 1.0,
        per_rope_delay_min: 0,
        per_rope_delay_max: 0,
        min_drop_height: 30.0,
        wait_for_ropes_to_drop: true,
        rope_final_height: 0.0,
    });
    t
}

/// R24-5 (`ChinookAIUpdate.cpp:770-782`, `ChinookMoveToBldgState::onExit`): the drop move's exit restores the cruise height
/// and ends the ultra-accurate flight on EVERY exit. A new command replaces the machine (`setMyState`, :1212-1222), which
/// exits the drop move at once. The height and flag must be back right after the order, and the drop is over.
/// Revert check: drop the `end_drop_move_settings` call from `ai_internal_move_exit`; the lowered override stays set after
/// the order and the test fails.
#[test]
fn an_interrupted_drop_move_restores_the_cruise_height_at_once() {
    let mut g = GameLogic::new(crate::terrain::TerrainLogic::flat(120, 120, 0), 1);
    let id = g.create_object(&Arc::new(chinook_with_height(20.0)), Coord3D::new(100.0, 100.0, 40.0), 0.0, 0);
    assert!(g.start_combat_drop(id, Coord3D::new(300.0, 300.0, 0.0), None));
    assert!(g.find_object(id).unwrap().loco_rt.ultra_accurate, "the drop's flight is ultra accurate");
    assert!(g.find_object(id).unwrap().preferred_height_override.is_some(), "the drop's height is set");
    g.ai_move_to_position_cmd(id, Coord3D::new(150.0, 100.0, 40.0), CommandSource::Player);
    let o = g.find_object(id).unwrap();
    assert_eq!(o.preferred_height_override, None, "the interrupt restores the cruise height at once");
    assert!(!o.loco_rt.ultra_accurate, "the interrupt ends the ultra-accurate flight at once");
    g.run_frames(2);
    assert!(g.chinook_state(id).is_none(), "the interrupted drop is over; no combat drop starts");
}

/// R24-6 (`ChinookAIUpdate.cpp:1212-1222`, `setMyState`): the move's goal keeps the commanded z (`setGoalPositionClipped(pos)`),
/// and the drop height is `m_destZ` (:754), which the port carries in `drop_move_z`. The goal z is the commanded 40, not the
/// drop height (ground 0 + cruise 20 = 20), so the quick path's end node and the aircraft path take the commanded z.
/// Revert check: write `dest_z` as the goal's z again; the goal reads 20 and the test fails.
#[test]
fn a_drop_move_keeps_the_commanded_z_as_its_goal_height() {
    let mut g = GameLogic::new(crate::terrain::TerrainLogic::flat(120, 120, 0), 1);
    let id = g.create_object(&Arc::new(chinook_with_height(20.0)), Coord3D::new(100.0, 100.0, 40.0), 0.0, 0);
    assert!(g.start_combat_drop(id, Coord3D::new(300.0, 300.0, 40.0), None));
    let o = g.find_object(id).unwrap();
    assert_eq!(o.ai.machine.goal_position.z, 40.0, "the goal keeps the commanded z");
    assert_eq!(o.ai.machine.drop_move_z, Some(20.0), "the drop height is the move's own z gate");
}

fn garrison_building() -> crate::template::ObjectTemplate {
    use crate::contain::{ContainKind, ContainTemplate, GarrisonFirePoints};
    let mut t = ObjectTemplate::new("Bunker");
    t.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE | kind_of::SELECTABLE;
    t.geometry.major_radius = 30.0;
    t.geometry.height = 40.0;
    t.max_health = 1000.0;
    t.initial_health = 1000.0;
    t.contain = Some(Arc::new(ContainTemplate {
        kind: ContainKind::Garrison,
        capacity: 5,
        allow_inside_kind_of: None,
        forbid_inside_kind_of: 0,
        allow_allies_inside: true,
        allow_enemies_inside: true,
        allow_neutral_inside: true,
        passengers_allowed_to_fire: false,
        damage_percentage_to_units: 0.0,
        number_of_exit_paths: 1,
        exit_delay: 0,
        door_open_time: 1,
        scatter_nearby_on_exit: true,
        orient_like_container_on_exit: false,
        reset_mood_check_on_exit: false,
        health_regen: 0.0,
        do_i_heal_objects: false,
        heal_frames: 1.0,
        initial_payload: Vec::new(),
        tunnel_frames_for_full_heal: 1.0,
        is_enclosing_container: true,
        fire_points: GarrisonFirePoints::default(),
        passengers_in_turret: false,
        weapon_bonus_passed_to_passengers: false,
        immune_to_clear_building_attacks: false,
    }));
    t
}

/// R24-9 (`AIStates.cpp:6269`, `AIEnterState::onEnter`: `setAdjustsDestination(false)`, then `AIInternalMoveToState::onEnter`,
/// whose `computePath` requests the path with `getAdjustsDestination()`, `:1606`). The Enter move is a NON-final request, so
/// `computePath`'s straight-line shortcut (AIUpdate.cpp:1709-1712) takes a clear line to the garrison as a two-node quick path
/// with the locomotor goal `NONE`, and no search. That is the mechanism behind the walk to the door in `l126_garrison`; the
/// enter ORDER there is the physics of three units on one line and is not checked against a C++ trace (R24-9, open).
/// Revert check: request the Enter move as final (`ai_internal_move_enter(id, cpos, true)`); the path is searched, the goal is
/// armed (`PositionOnPath`), and the test fails.
#[test]
fn an_enter_move_to_a_clear_garrison_is_a_quick_path() {
    let mut g = world();
    let bld = g.create_object(&Arc::new(garrison_building()), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let u = g.create_object(&tank(), Coord3D::new(260.0, 200.0, 0.0), 0.0, 0);
    g.submit(0, crate::command::Command::Enter { ids: vec![u], container: bld });
    g.update();
    let o = g.find_object(u).unwrap();
    assert_eq!(o.ai.machine.current, AiStateType::Enter, "the enter move runs");
    assert!(!o.ai.is_final_goal, "the enter request is not a final goal (AIStates.cpp:6269, :1606)");
    assert_eq!(o.ai.raw_path.as_ref().map(|r| r.nodes.len()), Some(2), "a clear line is a two-node quick path");
    assert_eq!(o.ai.locomotor_goal_type, LocoGoalType::None, "the quick path arms no goal (AIUpdate.cpp:1709-1712)");
}

/// Control for R24-5: a drop replaced by a second drop. The first drop's interrupt ends its own state (`done`), and the
/// second drop's state is then inserted, so the second drop's move is still running. Passes with or without R24-5 (it guards
/// the replacement path against the interrupt's new `done` marking).
#[test]
fn a_second_drop_is_not_cancelled_by_the_first_drops_interrupt() {
    let mut g = GameLogic::new(crate::terrain::TerrainLogic::flat(120, 120, 0), 1);
    let id = g.create_object(&Arc::new(chinook_with_height(20.0)), Coord3D::new(100.0, 100.0, 40.0), 0.0, 0);
    assert!(g.start_combat_drop(id, Coord3D::new(300.0, 300.0, 0.0), None));
    assert!(g.start_combat_drop(id, Coord3D::new(200.0, 300.0, 0.0), None));
    g.run_frames(2);
    let st = g.chinook_state(id).expect("the second drop is still in its move");
    assert_eq!(st.phase, crate::chinook::ChinookPhase::MoveToDrop, "the second drop's move is still running");
}
