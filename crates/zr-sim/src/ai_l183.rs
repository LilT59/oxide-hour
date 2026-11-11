//! Lane 183 (wave 21). Crate-internal tests (the functions under test are private, like the lane 177b and 182 tests).
//! - R23-9: `doPathfind`'s approach branch (AIUpdate.cpp:426-433) installs `findClosestPath`'s path and runs `updateGoal`.
//!   It has no `moveAllies` (`findClosestPath` is called with its `moveAllies` flag FALSE, and `moveAllies` has three
//!   callers only: AIUpdate.cpp:1747, :1996, :3267).

use crate::ai::LocoGoalType;
use crate::ai_states::{AiStateType, CommandSource, StateReturn};
use crate::chinook::{ChinookPhase, ChinookTemplate};
use crate::contain::{ContainKind, ContainTemplate, GarrisonFirePoints};
use crate::math::Coord3D;
use crate::template::{kind_of, LocomotorKind, LocomotorParams, ObjectTemplate, ZBehavior};
use crate::terrain::TerrainLogic;
use crate::command::Command;
use crate::GameLogic;
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

fn dozer() -> ObjectTemplate {
    let mut t = tank();
    t.kind_of = kind_of::VEHICLE | kind_of::DOZER | kind_of::SELECTABLE;
    t
}

/// R23-9. A dozer's approach path runs over an idle ally standing on its destination. `doPathfind`'s approach branch
/// installs the path and updates the goal, and it never calls `moveAllies` (AIUpdate.cpp:426-433), so the ally is not
/// asked to step aside. (A dozer's path would push it on a computed path: `moveAllies` pushes dozers unblocked, :10197.)
/// Revert check: with the approach install going through `store_ai_path` (which runs `moveAllies`) the ally gets a
/// temporary MoveOutOfTheWay.
#[test]
fn an_approach_path_does_not_push_the_allies_on_it() {
    let mut g = GameLogic::new(flat(), 1);
    let d = Arc::new(dozer());
    let t = Arc::new(tank());
    let mover = g.create_object(&d, Coord3D::new(100.0, 60.0, 0.0), 0.0, 0);
    let ally = g.create_object(&t, Coord3D::new(258.0, 60.0, 0.0), 0.0, 0);
    let dest = Coord3D::new(260.0, 60.0, 0.0);
    if let Some(o) = g.objects.get_mut(&mover) {
        o.ai.request_approach_path(&mut g.pathfinder, mover, dest);
    }
    g.process_pathfind_queue();
    let m = g.find_object(mover).unwrap();
    assert!(m.ai.has_path(), "the approach path is installed");
    let a = g.find_object(ally).unwrap();
    assert_eq!(a.ai.machine.temporary, None, "an approach path does not push the ally off its cell");
    assert_ne!(a.ai.machine.current, AiStateType::MoveOutOfTheWay, "the ally was not asked to move aside");
}

/// Gives `t` a jet module with fixed fields (the values are not read by these tests).
fn jet_template() -> ObjectTemplate {
    let mut t = tank();
    t.kind_of |= kind_of::AIRCRAFT;
    t.jet = Some(crate::modules2::JetAiTemplate {
        needs_runway: true,
        keeps_parking_space_when_airborne: true,
        takeoff_pause: 15,
        return_to_base_idle_time: 300,
        out_of_ammo_damage_per_second: 0.1,
        takeoff_dist_for_max_lift: 0.0,
        min_height: 5.0,
        parking_offset: 0.0,
        sneaky_offset_when_attacking: 0.0,
        attacking_loco: crate::template::LocomotorSet::Normal,
        returning_loco: crate::template::LocomotorSet::Normal,
        attack_loco_persist_time: 0,
        attackers_miss_persist_time: 0,
    });
    t
}

/// R23-10 (jet). `JetAIUpdate::isAllowedToMoveAwayFromUnit` (JetAIUpdate.cpp:2409-2416) is false for a parked jet (no
/// `ALLOW_AIR_LOCO`), so `privateMoveAwayFromUnit` returns (AIUpdate.cpp:3215) and the jet is not pushed.
/// Revert check: without the jet veto the parked jet steps aside (a temporary MoveOutOfTheWay).
#[test]
fn a_parked_jet_is_not_pushed_out_of_its_state() {
    let mut g = GameLogic::new(flat(), 1);
    let jet = Arc::new(jet_template());
    let t = Arc::new(tank());
    let parked = g.create_object(&jet, Coord3D::new(130.0, 60.0, 0.0), 0.0, 0);
    let other = g.create_object(&t, Coord3D::new(130.0, 200.0, 0.0), 0.0, 0);
    g.submit(0, Command::MoveTo { ids: vec![other], pos: Coord3D::new(130.0, 260.0, 0.0) });
    for _ in 0..3 {
        g.update();
    }
    if let Some(o) = g.objects.get_mut(&parked) {
        o.jet.flags = crate::jet_ai::jet_flag::HAS_PRODUCER_LOCATION;
    }
    g.ai_move_away_from_unit(parked, other);
    assert_eq!(g.find_object(parked).unwrap().ai.machine.temporary, None, "a parked jet is not pushed");
}

/// R23-10 (disabled). `isMobile` is false for a disabled unit (Object.cpp:2947-2956), so `privateMoveAwayFromUnit` returns
/// (AIUpdate.cpp:3215) and an EMP-disabled ally is not asked to step aside.
/// Revert check: without the disabled test the EMP-disabled ally steps aside.
#[test]
fn a_disabled_unit_is_not_pushed_out_of_its_state() {
    let mut g = GameLogic::new(flat(), 1);
    let t = Arc::new(tank());
    let ally = g.create_object(&t, Coord3D::new(130.0, 60.0, 0.0), 0.0, 0);
    let other = g.create_object(&t, Coord3D::new(130.0, 200.0, 0.0), 0.0, 0);
    g.submit(0, Command::MoveTo { ids: vec![other], pos: Coord3D::new(130.0, 260.0, 0.0) });
    for _ in 0..3 {
        g.update();
    }
    g.set_disabled(ally, crate::disabled::DisabledType::Emp);
    g.ai_move_away_from_unit(ally, other);
    assert_eq!(g.find_object(ally).unwrap().ai.machine.temporary, None, "an EMP-disabled unit is not pushed");
}

/// A cleaner with a cleanup-hazard module and no weapon (the scan finds nothing in these tests).
fn cleaner_template() -> ObjectTemplate {
    let mut t = tank();
    t.cleanup_hazard = Some(crate::modules4::CleanupHazardTemplate { scan_frames: 1, scan_range: 150.0 });
    t
}

/// R23-12 (keep busy). `CleanupHazardUpdate::update` (CleanupHazardUpdate.cpp:133-139): an idle unit that is cleaning
/// an area is kept busy for the scripts (`aiBusy(CMD_FROM_AI)`).
/// Revert check: without the busy block the idle cleaner stays `Idle`.
#[test]
fn an_idle_cleanup_area_keeps_its_unit_busy() {
    let mut g = GameLogic::new(flat(), 1);
    let id = g.create_object(&Arc::new(cleaner_template()), Coord3D::new(200.0, 60.0, 0.0), 0.0, 0);
    if let Some(o) = g.objects.get_mut(&id) {
        o.behavior.cleanup_area = Some((Coord3D::new(200.0, 60.0, 0.0), 100.0));
    }
    g.cleanup_hazard_update(id);
    assert_eq!(g.find_object(id).unwrap().ai.machine.current, AiStateType::Busy, "an idle area cleaner is kept busy");
}

/// R23-12 (abandon). A player's or a script's new order (`getLastCommandSource() != CMD_FROM_AI`) abandons the cleanup
/// area (CleanupHazardUpdate.cpp:142-146), whatever the unit is doing.
/// Revert check: without the abandon the area stays set.
#[test]
fn a_new_player_order_abandons_the_cleanup_area() {
    let mut g = GameLogic::new(flat(), 1);
    let id = g.create_object(&Arc::new(cleaner_template()), Coord3D::new(200.0, 60.0, 0.0), 0.0, 0);
    if let Some(o) = g.objects.get_mut(&id) {
        o.behavior.cleanup_area = Some((Coord3D::new(200.0, 200.0, 0.0), 100.0));
    }
    g.ai_move_to_position_cmd(id, Coord3D::new(200.0, 260.0, 0.0), CommandSource::Player);
    g.cleanup_hazard_update(id);
    assert_eq!(g.find_object(id).unwrap().behavior.cleanup_area, None, "a player's order abandons the area");
}

/// R23-12 (move gate). The return move is issued only when the unit is idle or busy (CleanupHazardUpdate.cpp:170-171,
/// :184). A unit in an attack state is neither, so it is not moved back to the area.
/// Revert check: with the old `!moving` gate the attacking cleaner gets a `CMD_FROM_AI` move, which for a non-idle unit is
/// the 20-second temporary `AI_MOVE_TO` (AIUpdate.cpp:2894-2901), so `temporary` becomes `MoveTo`.
#[test]
fn an_attacking_cleaner_is_not_moved_back_to_its_area() {
    let mut g = GameLogic::new(flat(), 1);
    let id = g.create_object(&Arc::new(cleaner_template()), Coord3D::new(200.0, 60.0, 0.0), 0.0, 0);
    if let Some(o) = g.objects.get_mut(&id) {
        o.behavior.cleanup_area = Some((Coord3D::new(200.0, 200.0, 0.0), 100.0));
        o.ai.machine.current = AiStateType::AttackObject;
    }
    g.cleanup_hazard_update(id);
    let o = g.find_object(id).unwrap();
    assert_eq!(o.ai.machine.temporary, None, "the attacking cleaner is not sent back to its area");
    assert_eq!(o.ai.machine.current, AiStateType::AttackObject, "its attack is kept");
    assert!(o.behavior.cleanup_area.is_some(), "the area is kept");
}

/// R23-12 (range 0). `m_moveRange` is the range (CleanupHazardUpdate.cpp:300): a zero range is no area (every test of
/// `m_moveRange` in the update is `> 0`), but the unit still moves to the position (:308).
/// Revert check: without the positive-range test the zero-range order keeps an area.
#[test]
fn a_zero_range_cleanup_moves_without_an_area() {
    let mut g = GameLogic::new(flat(), 1);
    let id = g.create_object(&Arc::new(cleaner_template()), Coord3D::new(200.0, 60.0, 0.0), 0.0, 0);
    g.set_cleanup_area(id, Coord3D::new(200.0, 200.0, 0.0), 0.0);
    let o = g.find_object(id).unwrap();
    assert_eq!(o.behavior.cleanup_area, None, "a zero range is not an area");
    assert_eq!(o.ai.machine.current, AiStateType::MoveTo, "the unit still moves to the position");
}

/// R23-8. `AIMoveOutOfTheWayState::onEnter` returns `AIInternalMoveToState::onEnter` (AIStates.cpp:2148-2160), which calls
/// `ai->friend_startingMove()` (:1635): `m_isMoving` is set when the step-aside starts. A unit in a non-idle main state
/// reads as moving through that flag (`isMoving`, AIUpdate.cpp:3194-3206), and its locomotor goal is still NONE here.
/// Revert check: without the enter's call `is_moving()` reads false.
#[test]
fn a_step_aside_sets_is_moving_when_it_starts() {
    let mut g = GameLogic::new(flat(), 1);
    let t = Arc::new(tank());
    let ally = g.create_object(&t, Coord3D::new(130.0, 60.0, 0.0), 0.0, 0);
    let other = g.create_object(&t, Coord3D::new(130.0, 200.0, 0.0), 0.0, 0);
    // `other` must hold a path (`unitPath == nullptr` returns, AIUpdate.cpp:3325-3326).
    g.submit(0, Command::MoveTo { ids: vec![other], pos: Coord3D::new(130.0, 260.0, 0.0) });
    for _ in 0..3 {
        g.update();
    }
    if let Some(o) = g.objects.get_mut(&ally) {
        o.ai.machine.current = AiStateType::AttackObject;
    }
    g.ai_move_away_from_unit(ally, other);
    let a = g.find_object(ally).unwrap();
    assert_eq!(a.ai.machine.temporary, Some(AiStateType::MoveOutOfTheWay), "the ally stepped aside");
    assert_eq!(a.ai.locomotor_goal_type, LocoGoalType::None, "no goal is armed by the step-aside itself");
    assert!(a.ai.is_moving(), "the step-aside's enter sets m_isMoving");
}

// ---- Chinook combat drop (R23-1 to R23-7) ------------------------------------------------------

/// A Chinook (ai_l177b's fixture) whose locomotor's preferred height is `preferred`.
fn chinook_with_height(preferred: f32) -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Chinook");
    t.kind_of = kind_of::VEHICLE | kind_of::AIRCRAFT | kind_of::TRANSPORT | kind_of::SELECTABLE;
    t.geometry.major_radius = 20.0;
    t.geometry.height = 12.0;
    t.max_health = 300.0;
    t.initial_health = 300.0;
    let mut loco = LocomotorParams::from_ini(LocomotorKind::Hover, 120.0, 60.0, 180.0, 120.0);
    loco.behavior_z = ZBehavior::SurfaceRelativeHeight;
    loco.preferred_height = preferred;
    t.locomotor = Some(loco);
    t.contain = Some(Arc::new(transport_contain(8)));
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

fn transport_contain(capacity: i32) -> ContainTemplate {
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

fn structure(height: f32) -> ObjectTemplate {
    let mut b = ObjectTemplate::new("Bldg");
    b.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;
    b.geometry.major_radius = 20.0;
    b.geometry.height = height;
    b.max_health = 1000.0;
    b.initial_health = 1000.0;
    b
}

/// R23-1 (ground drop height). `ChinookMoveToBldgState::onEnter` (ChinookAIUpdate.cpp:731-754): with no building the new
/// preferred height is the old one, so the drop's height is the ground plus the Chinook's cruise height, not plus
/// MinDropHeight (30 in this fixture).
/// Revert check: the old `ground + MinDropHeight` gives 30 and the test fails.
#[test]
fn a_ground_drop_hovers_at_the_cruise_height() {
    let mut g = GameLogic::new(flat(), 1);
    let id = g.create_object(&Arc::new(chinook_with_height(20.0)), Coord3D::new(100.0, 100.0, 40.0), 0.0, 0);
    assert!(g.start_combat_drop(id, Coord3D::new(300.0, 300.0, 0.0), None));
    let st = g.chinook_state(id).unwrap();
    assert_eq!(st.dest_z, 20.0, "the ground drop's height is the old preferred height above the ground");
    assert_eq!(g.find_object(id).unwrap().preferred_height_override, Some(20.0), "the Chinook flies at that height");
}

/// R23-2 (building height floor). `if (m_newPreferredHeight < m_oldPreferredHeight) m_newPreferredHeight = m_oldPreferredHeight`
/// (ChinookAIUpdate.cpp:739-746). A 15-high building plus MinDropHeight (45) is below the 80 cruise height, so the drop
/// stays at 80.
/// Revert check: without the floor the drop height is 45.
#[test]
fn a_building_drop_never_goes_below_the_cruise_height() {
    let mut g = GameLogic::new(flat(), 1);
    let bldg = g.create_object(&Arc::new(structure(15.0)), Coord3D::new(300.0, 300.0, 0.0), 0.0, 1);
    let id = g.create_object(&Arc::new(chinook_with_height(80.0)), Coord3D::new(100.0, 100.0, 80.0), 0.0, 0);
    assert!(g.start_combat_drop(id, Coord3D::new(300.0, 300.0, 0.0), Some(bldg)));
    assert_eq!(g.chinook_state(id).unwrap().dest_z, 80.0, "the building drop keeps the cruise height as its floor");
}

/// R23-3 (ultra accurate). `ChinookMoveToBldgState::onEnter` sets `setUltraAccurate(true)` (ChinookAIUpdate.cpp:731) and
/// `onExit` clears it (:777). The move's exit is an interrupt here, so the flag is cleared and the cruise height returns.
/// Revert check: without the flag the move is not ultra accurate while it runs.
#[test]
fn a_drop_move_is_ultra_accurate_until_it_exits() {
    let mut g = GameLogic::new(flat(), 1);
    let id = g.create_object(&Arc::new(chinook_with_height(20.0)), Coord3D::new(100.0, 100.0, 40.0), 0.0, 0);
    assert!(g.start_combat_drop(id, Coord3D::new(300.0, 300.0, 0.0), None));
    assert!(g.find_object(id).unwrap().loco_rt.ultra_accurate, "the drop's flight is ultra accurate");
    g.ai_command_set_state(id, AiStateType::Idle);
    g.run_frames(2);
    let o = g.find_object(id).unwrap();
    assert!(!o.loco_rt.ultra_accurate, "the move's exit ends the ultra-accurate flight");
    assert_eq!(o.preferred_height_override, None, "the move's exit restores the cruise height");
}

/// R23-4 (height restored at the move's end). When the drop move succeeds, its exit runs before `DO_COMBAT_DROP`'s enter
/// (ChinookAIUpdate.cpp:770-782, :853), so the Chinook is back at its cruise height when the drop starts.
/// Revert check: the port kept the lowered height until the drop ended, so the override stays set and the test fails.
#[test]
fn the_cruise_height_returns_when_the_drop_move_succeeds() {
    let mut g = GameLogic::new(flat(), 1);
    let id = g.create_object(&Arc::new(chinook_with_height(20.0)), Coord3D::new(100.0, 100.0, 40.0), 0.0, 0);
    assert!(g.start_combat_drop(id, Coord3D::new(300.0, 300.0, 0.0), None));
    if let Some(o) = g.objects.get_mut(&id) {
        o.ai.machine.drop_move_result = Some(true);
    }
    g.run_frames(2);
    let st = g.chinook_state(id).unwrap();
    assert_eq!(st.phase, ChinookPhase::CombatDrop, "the successful move starts the drop");
    assert_eq!(g.find_object(id).unwrap().preferred_height_override, None, "the cruise height is back before the drop");
}

/// R23-5 (the gate on every return). `ChinookMoveToBldgState::update` (ChinookAIUpdate.cpp:766-776) returns CONTINUE for any
/// status other than CONTINUE while the Chinook is more than 3 from its drop height. A drop move whose path search fails
/// while the Chinook is high keeps going.
/// Revert check: without the gate on the MoveTo update the failed move returns FAILURE.
#[test]
fn a_failed_drop_move_waits_for_the_drop_height() {
    let mut g = GameLogic::new(flat(), 1);
    let id = g.create_object(&Arc::new(chinook_with_height(20.0)), Coord3D::new(100.0, 100.0, 40.0), 0.0, 0);
    if let Some(o) = g.objects.get_mut(&id) {
        o.ai.machine.current = AiStateType::MoveTo;
        o.ai.machine.goal_position = Coord3D::new(300.0, 300.0, 0.0);
        o.ai.move_goal = Some(Coord3D::new(300.0, 300.0, 0.0));
        o.ai.move_waiting_for_path = true;
        o.ai.waiting_for_path = false;
        o.ai.machine.drop_move_z = Some(20.0);
    }
    let r = g.ai_state_update(id, AiStateType::MoveTo);
    assert_eq!(r, StateReturn::Continue, "the failed move waits while the Chinook is above its drop height");
    let z = g.find_object(id).unwrap().position.z;
    if let Some(o) = g.objects.get_mut(&id) {
        // The Chinook is at its drop height now (the gate opens), and the failed search is retried as failed.
        o.ai.machine.drop_move_z = Some(z);
        o.ai.move_waiting_for_path = true;
        o.ai.waiting_for_path = false;
    }
    let r = g.ai_state_update(id, AiStateType::MoveTo);
    assert_eq!(r, StateReturn::Failure, "once the height is reached the failed move is a failure");
}

/// R23-6 (main state). `setMyState(MOVE_TO_COMBAT_DROP, ...)` (ChinookAIUpdate.cpp:1219-1226, :1275) clears the machine and
/// makes the drop the main state. A Chinook already in a main move gets the drop as its main move, not a 20-second
/// temporary `AI_MOVE_TO` (AIUpdate.cpp:2894-2901) that leaves the old move in charge.
/// Revert check: with `ai_move_to_position_cmd(.., Ai)` the non-idle Chinook gets a temporary move.
#[test]
fn a_drop_replaces_a_busy_chinooks_main_move() {
    let mut g = GameLogic::new(flat(), 1);
    let id = g.create_object(&Arc::new(chinook_with_height(20.0)), Coord3D::new(100.0, 100.0, 40.0), 0.0, 0);
    g.ai_move_to_position_cmd(id, Coord3D::new(150.0, 100.0, 40.0), CommandSource::Player);
    assert_eq!(g.find_object(id).unwrap().ai.machine.current, AiStateType::MoveTo, "the Chinook is in a main move");
    assert!(g.start_combat_drop(id, Coord3D::new(300.0, 300.0, 0.0), None));
    let o = g.find_object(id).unwrap();
    assert_eq!(o.ai.machine.temporary, None, "the drop is not a temporary move");
    assert_eq!(o.ai.machine.current, AiStateType::MoveTo, "the drop is the main move");
    assert_eq!(o.ai.machine.goal_position.x, 300.0, "the main move goes to the drop");
}

/// R23-7 (ground drop position). `privateCombatDrop` (ChinookAIUpdate.cpp:1266-1274) moves a ground drop to a free spot
/// near the requested point, so the Chinook does not drop onto a structure's footprint.
/// Revert check: without the search the drop stays on the requested point, which is inside the structure.
#[test]
fn a_ground_drop_onto_a_structure_moves_to_a_free_spot() {
    let mut g = GameLogic::new(flat(), 1);
    let _bldg = g.create_object(&Arc::new(structure(15.0)), Coord3D::new(300.0, 300.0, 0.0), 0.0, 1);
    let id = g.create_object(&Arc::new(chinook_with_height(20.0)), Coord3D::new(100.0, 100.0, 40.0), 0.0, 0);
    assert!(g.start_combat_drop(id, Coord3D::new(300.0, 300.0, 0.0), None));
    let st = g.chinook_state(id).unwrap();
    assert!(st.target_pos.dist_2d(&Coord3D::new(300.0, 300.0, 0.0)) > 20.0, "the drop moved clear of the structure");
}

// ---- Pilot snipe: the AutoHeal undo (R22-1, R22-2) ----------------------------------------------

/// A vehicle with two AutoHeal modules: the first is the inherited veterancy heal, the second is its own upgrade heal.
fn two_heal_vehicle(trainable: bool) -> ObjectTemplate {
    let mut t = tank();
    t.is_trainable = trainable;
    let first = crate::behaviors::AutoHealTemplate {
        healing_amount: 10.0,
        healing_delay: 30,
        start_healing_delay: 0,
        radius: 0.0,
        affects_whole_player: false,
        skip_self_for_healing: false,
        single_burst: false,
        kind_of: kind_of::VEHICLE,
        forbidden_kind_of: 0,
        starts_active: true,
        triggered_by: vec![],
        conflicts_with: vec![],
    };
    let second = crate::behaviors::AutoHealTemplate { starts_active: false, triggered_by: vec!["Upgrade_GLAJunkRepair".to_owned()], ..first.clone() };
    t.auto_heal = vec![first, second];
    t
}

/// R22-1. A pilot snipe (`DISABLED_UNMANNED`) calls `undoUpgrade` on the first AutoHeal module only (Object.cpp:2243-2246):
/// `findUpdateModule` stops at the first match (Object.cpp:2916-2938). The second module keeps its upgraded heal.
/// Revert check: the undo of every module (the old loop) clears the second module's latch too.
#[test]
fn a_pilot_snipe_undoes_only_the_first_auto_heal_module() {
    let mut g = GameLogic::new(flat(), 1);
    let id = g.create_object(&Arc::new(two_heal_vehicle(true)), Coord3D::new(100.0, 60.0, 0.0), 0.0, 0);
    if let Some(o) = g.objects.get_mut(&id) {
        for a in o.behavior.auto_heal.iter_mut() {
            a.started = true;
        }
    }
    g.set_disabled(id, crate::disabled::DisabledType::Unmanned);
    let o = g.find_object(id).unwrap();
    assert!(!o.behavior.auto_heal[0].started, "the first module (the veterancy heal) is undone");
    assert!(o.behavior.auto_heal[1].started, "the second module keeps its upgraded heal");
}

/// R22-2. The AutoHeal undo of the pilot snipe does not depend on trainability: `setExperienceAndLevel` is the only
/// trainable-only part (ExperienceTracker.cpp:207-228), and the `else` branch runs the undo for any vehicle (Object.cpp:2242).
/// Revert check: the early return for an untrainable unit leaves its module started.
#[test]
fn a_pilot_snipe_undoes_the_auto_heal_of_an_untrainable_vehicle() {
    let mut g = GameLogic::new(flat(), 1);
    let id = g.create_object(&Arc::new(two_heal_vehicle(false)), Coord3D::new(100.0, 60.0, 0.0), 0.0, 0);
    if let Some(o) = g.objects.get_mut(&id) {
        o.behavior.auto_heal[0].started = true;
    }
    g.set_disabled(id, crate::disabled::DisabledType::Unmanned);
    assert!(!g.find_object(id).unwrap().behavior.auto_heal[0].started, "an untrainable vehicle's AutoHeal is undone too");
}
