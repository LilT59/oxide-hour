//! Lane 177b (wave 21). Crate-internal tests (the functions under test are private, like the lane 154/158/163 tests):
//! - `Pathfinder::moveAlliesAwayFromDestination` (AIPathfind.cpp:6985) runs `moveAlliesDestinationCallback` on every
//!   cell of the line (AIPathfind.cpp:6950-6980): an ally that covers two cells is asked twice, and each visit reads
//!   the ally's state live.
//! - The Chinook's combat drop is `MOVE_TO_COMBAT_DROP` (ChinookAIUpdate.cpp:692-782, :853): the move's own success
//!   (with its height wait) starts the drop, and its failure or interrupt ends it.
//! - `AIRappelState::update`'s scatter (AIStates.cpp, a full building) searches `findPositionAround` from 1.5*PI and
//!   follows the spot it finds.

use super::*;
use crate::ai_states::{AiStateType, CommandSource};
use crate::chinook::ChinookTemplate;
use crate::command::Command;
use crate::contain::{ContainKind, ContainTemplate, GarrisonFirePoints};
use crate::math::Coord3D;
use crate::template::{kind_of, LocomotorKind, LocomotorParams, ObjectTemplate, ZBehavior};
use crate::terrain::TerrainLogic;
use crate::GameRules;
use std::sync::Arc;

fn flat() -> TerrainLogic {
    TerrainLogic::flat(120, 120, 0)
}

fn tank() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("T");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 8.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    t
}

/// The ally stepped aside for `other` already (a temporary move-away with its own path). The mover's push visits the ally
/// on the first cell: the temporary is active, so `setTemporaryState` runs the old exit, whose `destroyPath` drops the
/// new path, and the new state's enter finds no path (AIStates.cpp:945-953, :2188-2196). The second cell of the same
/// ally is the next visit, which finds no temporary and steps the ally aside.
/// Revert check: with one visit per unit (the old `units_along_line` de-duplication) the ally is left with no temporary.
#[test]
fn a_failed_first_visit_is_retried_on_the_second_cell_of_the_same_ally() {
    let mut g = GameLogic::new(flat(), 1);
    let t = Arc::new(tank());
    let mover = g.create_object(&t, Coord3D::new(100.0, 60.0, 0.0), 0.0, 0);
    let ally = g.create_object(&t, Coord3D::new(130.0, 60.0, 0.0), 0.0, 0);
    let other = g.create_object(&t, Coord3D::new(130.0, 200.0, 0.0), 0.0, 0);
    g.submit(0, Command::MoveTo { ids: vec![mover], pos: Coord3D::new(260.0, 60.0, 0.0) });
    g.submit(0, Command::MoveTo { ids: vec![other], pos: Coord3D::new(260.0, 200.0, 0.0) });
    for _ in 0..3 {
        g.update();
    }
    assert!(g.find_object(mover).unwrap().ai.has_path(), "the mover holds its path");
    assert!(g.find_object(other).unwrap().ai.has_path(), "the other unit holds its path");
    g.ai_move_away_from_unit(ally, other);
    assert_eq!(g.find_object(ally).unwrap().ai.machine.temporary, Some(AiStateType::MoveOutOfTheWay), "the ally stepped aside for `other`");
    let dest = Coord3D::new(260.0, 60.0, 0.0);
    g.move_allies_away_from_destination(mover, &dest);
    let a = g.find_object(ally).unwrap();
    assert_eq!(a.ai.machine.temporary, Some(AiStateType::MoveOutOfTheWay), "the second visit stepped the ally aside");
    assert!(a.ai.has_path(), "the ally's new move-away path is in place");
}

/// `moveAlliesDestinationCallback` skips an ally that is busy or using an ability (AIPathfind.cpp:6971-6972:"Black Lotus
/// exploit fix -- moving while hacking"). The busy state is set directly (the test sets the machine's state).
/// Revert check: without the busy veto the ally on the line steps aside.
#[test]
fn a_busy_ally_on_the_line_is_not_asked_to_step_aside() {
    let mut g = GameLogic::new(flat(), 1);
    let t = Arc::new(tank());
    let mover = g.create_object(&t, Coord3D::new(100.0, 60.0, 0.0), 0.0, 0);
    let ally = g.create_object(&t, Coord3D::new(130.0, 60.0, 0.0), 0.0, 0);
    g.submit(0, Command::MoveTo { ids: vec![mover], pos: Coord3D::new(260.0, 60.0, 0.0) });
    for _ in 0..3 {
        g.update();
    }
    assert!(g.find_object(mover).unwrap().ai.has_path(), "the mover holds its path");
    if let Some(o) = g.objects.get_mut(&ally) {
        o.ai.machine.current = AiStateType::Busy;
    }
    g.move_allies_away_from_destination(mover, &Coord3D::new(260.0, 60.0, 0.0));
    assert_eq!(g.find_object(ally).unwrap().ai.machine.temporary, None, "a busy ally is not pushed");
}

/// `ChinookMoveToBldgState::update` (ChinookAIUpdate.cpp:766-776): a finished move is not a success while the Chinook is
/// more than 3 from its drop height, so the drop's move stays in `AI_MOVE_TO` at the goal, and its result is the
/// success once the height is reached.
/// Revert check: without the z wait the move succeeds at the goal and its result is recorded at once.
#[test]
fn a_drop_move_waits_at_the_goal_for_the_drop_height() {
    let mut g = GameLogic::new(flat(), 1);
    let t = Arc::new(tank());
    let id = g.create_object(&t, Coord3D::new(100.0, 60.0, 0.0), 0.0, 0);
    g.ai_move_to_position_cmd(id, Coord3D::new(110.0, 60.0, 0.0), CommandSource::Ai);
    if let Some(o) = g.objects.get_mut(&id) {
        o.ai.machine.drop_move_z = Some(50.0);
    }
    for _ in 0..60 {
        g.update();
    }
    let o = g.find_object(id).unwrap();
    assert_eq!(o.ai.machine.current, AiStateType::MoveTo, "the drop's move waits at the goal for its height");
    assert_eq!(o.ai.machine.drop_move_result, None, "no result while the move waits");
    if let Some(o) = g.objects.get_mut(&id) {
        o.ai.machine.drop_move_z = Some(0.0);
    }
    g.update();
    let o = g.find_object(id).unwrap();
    assert_eq!(o.ai.machine.drop_move_result, Some(true), "the move succeeds once the Chinook is at its height");
    assert_eq!(o.ai.machine.current, AiStateType::Idle, "the move ended");
}

/// `MOVE_TO_COMBAT_DROP` is replaced when its move is interrupted (ChinookAIUpdate.cpp:853): the drop's z gate ends with
/// the move, and no result is kept, so the Chinook's update cancels the drop.
/// Revert check: without the exit's clear the z gate survives the interrupt.
#[test]
fn an_interrupted_drop_move_clears_its_gate_and_keeps_no_result() {
    let mut g = GameLogic::new(flat(), 1);
    let t = Arc::new(tank());
    let id = g.create_object(&t, Coord3D::new(100.0, 60.0, 0.0), 0.0, 0);
    g.ai_move_to_position_cmd(id, Coord3D::new(110.0, 60.0, 0.0), CommandSource::Ai);
    if let Some(o) = g.objects.get_mut(&id) {
        o.ai.machine.drop_move_z = Some(50.0);
    }
    g.update();
    g.ai_command_set_state(id, AiStateType::Idle);
    let o = g.find_object(id).unwrap();
    assert_eq!(o.ai.machine.drop_move_z, None, "the interrupted move's z gate is cleared");
    assert_eq!(o.ai.machine.drop_move_result, None, "an interrupted move has no result");
}

fn chinook_template() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Chinook");
    t.kind_of = kind_of::VEHICLE | kind_of::AIRCRAFT | kind_of::TRANSPORT | kind_of::SELECTABLE;
    t.geometry.major_radius = 20.0;
    t.geometry.height = 12.0;
    t.max_health = 300.0;
    t.initial_health = 300.0;
    let mut loco = LocomotorParams::from_ini(LocomotorKind::Hover, 120.0, 60.0, 180.0, 120.0);
    loco.behavior_z = ZBehavior::SurfaceRelativeHeight;
    t.locomotor = Some(loco);
    t.contain = Some(Arc::new(garrison_or_transport(ContainKind::Transport, 8)));
    t.chinook = Some(ChinookTemplate {
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

fn ranger_template() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Ranger");
    t.kind_of = kind_of::INFANTRY | kind_of::SELECTABLE | kind_of::CAN_RAPPEL;
    t.geometry.major_radius = 4.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.transport_slot_count = 1;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Legs, 20.0, 60.0, 120.0, 180.0));
    t
}

fn bunker_template(capacity: i32) -> ObjectTemplate {
    let mut b = ObjectTemplate::new("Bunker");
    b.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;
    b.geometry.major_radius = 20.0;
    b.geometry.height = 15.0;
    b.max_health = 1000.0;
    b.initial_health = 1000.0;
    b.contain = Some(Arc::new(garrison_or_transport(ContainKind::Garrison, capacity)));
    b
}

fn garrison_or_transport(kind: ContainKind, capacity: i32) -> ContainTemplate {
    ContainTemplate {
        kind,
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
        is_enclosing_container: kind == ContainKind::Garrison,
        fire_points: GarrisonFirePoints::default(),
        passengers_in_turret: false,
        weapon_bonus_passed_to_passengers: false,
        immune_to_clear_building_attacks: false,
    }
}

/// `AIRappelState::update` (AIStates.cpp, the building is full): the rappeller lands near the building and follows the
/// spot `findPositionAround` finds from 1.5*PI out to 200 (`aiFollowPath`), instead of moving to a made-up point.
/// Revert check: the old "move to (x, y - 200)" leaves the rappeller in `AI_MOVE_TO`, not `AI_FOLLOW_PATH`.
#[test]
fn a_rappeller_into_a_full_building_follows_a_path_to_the_ground() {
    let mut g = GameLogic::new(flat(), 1);
    g.set_rules(GameRules { gravity: -1.0, ..GameRules::default() });
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    let ch = g.create_object(&Arc::new(chinook_template()), Coord3D::new(100.0, 100.0, 30.0), 0.0, 0);
    let r1 = g.create_object(&Arc::new(ranger_template()), Coord3D::new(100.0, 100.0, 30.0), 0.0, 0);
    g.force_enter_contain(ch, r1);
    let bldg = g.create_object(&Arc::new(bunker_template(1)), Coord3D::new(400.0, 400.0, 0.0), 0.0, 1);
    // A friend fills the one garrison slot, so the rappeller cannot enter.
    let friend = g.create_object(&Arc::new(ranger_template()), Coord3D::new(400.0, 400.0, 0.0), 0.0, 0);
    g.force_enter_contain(bldg, friend);
    assert!(g.start_combat_drop(ch, Coord3D::new(400.0, 400.0, 0.0), Some(bldg)));
    let mut landed = false;
    for _ in 0..600 {
        g.update();
        if g.container_of(r1).is_none() && g.rappelling_count() == 0 {
            landed = true;
            break;
        }
    }
    assert!(landed, "the rappeller reached the ground");
    let o = g.find_object(r1).unwrap();
    assert_eq!(o.ai.machine.current, AiStateType::FollowPath, "the scatter follows the spot found by findPositionAround");
}

/// `PartitionManager::findPositionAround` with `startAngle` set (PartitionManager.cpp:3996-4000) searches from that angle
/// and not a random one: on open ground with a single ring at 50, from 1.5*PI (down, -y) the first spot is directly south.
/// Revert check: a start angle that is ignored (the random start) puts the spot elsewhere on the ring.
#[test]
fn the_position_search_starts_at_the_given_angle() {
    let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
    let center = Coord3D::new(400.0, 400.0, 0.0);
    let opts = crate::dock::FindPositionOptions {
        min_radius: 50.0,
        max_radius: 50.0,
        start_angle: Some(1.5 * crate::math::PI),
        ..Default::default()
    };
    let spot = g.find_position_around(&center, &opts).expect("open ground has a spot");
    assert!((spot.x - 400.0).abs() < 1.0, "x is directly south of the center: {spot:?}");
    assert!((spot.y - 350.0).abs() < 1.0, "y is 50 south of the center: {spot:?}");
}
