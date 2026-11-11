//! Lane 185 (wave 21). Crate-internal tests (the state they set is private, like the lane 177b, 182 and 183 tests).
//! - R23-11: `AIUpdateInterface::isMoving` (AIUpdate.cpp:3194-3206) calls the virtual `isIdle()`, so the `JetAIUpdate` and
//!   `HackInternetAIUpdate` overrides (JetAIUpdate.cpp:1898-1906, HackInternetAIUpdate.cpp:68-76) read a unit holding a
//!   command as not idle. `Object::is_ai_moving` applies them.

use crate::ai::LocoGoalType;
use crate::ai_states::{AiStateType, CommandSource};
use crate::command::{Command, GameMessage};
use crate::hack_internet::{HackPhase, HackState};
use crate::jet_ai::jet_flag;
use crate::math::Coord3D;
use crate::modules2::JetAiTemplate;
use crate::template::{kind_of, LocomotorKind, LocomotorParams, LocomotorSet, ObjectTemplate};
use crate::terrain::TerrainLogic;
use crate::GameLogic;
use std::sync::Arc;

fn flat() -> TerrainLogic {
    TerrainLogic::flat(120, 120, 0)
}

fn vehicle(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 8.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    t
}

fn jet_template() -> ObjectTemplate {
    let mut t = vehicle("Jet");
    t.jet = Some(JetAiTemplate {
        needs_runway: false,
        keeps_parking_space_when_airborne: false,
        takeoff_pause: 0,
        return_to_base_idle_time: 0,
        out_of_ammo_damage_per_second: 0.0,
        takeoff_dist_for_max_lift: 0.0,
        min_height: 0.0,
        parking_offset: 0.0,
        sneaky_offset_when_attacking: 0.0,
        attacking_loco: LocomotorSet::Normal,
        returning_loco: LocomotorSet::Normal,
        attack_loco_persist_time: 0,
        attackers_miss_persist_time: 0,
    });
    t
}

/// A unit in `AI_IDLE` whose locomotor goal is set (a move that is ordered but not yet under way).
fn idle_with_a_goal(g: &mut GameLogic, id: crate::object::ObjectId) {
    let o = g.objects.get_mut(&id).unwrap();
    o.ai.machine.current = AiStateType::Idle;
    o.ai.locomotor_goal_type = LocoGoalType::PositionExplicit;
}

/// `JetAIUpdate::isIdle` returns false while `HAS_PENDING_COMMAND` is held, so `isMoving` falls through to the goal test
/// and reads TRUE. The base machine test alone still says idle. Revert check: `ai_idle_vetoed` without the jet test fails
/// the first assert.
#[test]
fn a_jet_holding_a_command_reads_as_moving_while_idle_with_a_goal() {
    let mut g = GameLogic::new(flat(), 1);
    let id = g.create_object(&Arc::new(jet_template()), Coord3D::new(20.0, 20.0, 0.0), 0.0, 0);
    idle_with_a_goal(&mut g, id);
    g.objects.get_mut(&id).unwrap().jet.flags |= jet_flag::HAS_PENDING_COMMAND;
    let o = g.find_object(id).unwrap();
    assert!(o.is_ai_moving(), "isIdle is false with a pending command, so isMoving reads the goal (AIUpdate.cpp:3196-3201)");
    assert!(!o.ai.is_moving(), "the base machine test alone still says idle");
}

/// Without the command the jet is idle, so `isMoving` is false whatever goal it holds (AIUpdate.cpp:3196-3198).
#[test]
fn a_jet_without_a_command_is_idle_so_not_moving() {
    let mut g = GameLogic::new(flat(), 1);
    let id = g.create_object(&Arc::new(jet_template()), Coord3D::new(20.0, 20.0, 0.0), 0.0, 0);
    idle_with_a_goal(&mut g, id);
    assert!(!g.find_object(id).unwrap().is_ai_moving());
}

/// `HackInternetAIUpdate::isIdle` returns false while `m_pendingCommand` is held (HackInternetAIUpdate.cpp:71-72), so the
/// goal reads as moving. Revert check: `ai_idle_vetoed` without the hack test fails the assert.
#[test]
fn a_hacker_holding_an_order_reads_as_moving_while_idle_with_a_goal() {
    let mut g = GameLogic::new(flat(), 1);
    let id = g.create_object(&Arc::new(vehicle("Hacker")), Coord3D::new(20.0, 40.0, 0.0), 0.0, 0);
    idle_with_a_goal(&mut g, id);
    g.objects.get_mut(&id).unwrap().hack_internet = Some(HackState {
        phase: HackPhase::Packing,
        frames_remaining: 10,
        pending: Some(GameMessage { player: 0, command: Command::Stop { ids: Vec::new() }, source: CommandSource::Player }),
    });
    assert!(g.find_object(id).unwrap().is_ai_moving(), "m_pendingCommand makes isIdle false, so the goal reads as moving");
}

/// `privateMoveAwayFromUnit` installs its path through `destroyPath` (AIUpdate.cpp:3333-3335), which ends with
/// `setLocomotorGoalNone()` (:2027). The install does not stamp `m_pathTimestamp`: only `computePath` and its reset write
/// it (AIStates.cpp:1789, :1870). Revert check: without the goal clear the goal stays `PositionExplicit`, and with the
/// stamp the timestamp moves to the frame; both asserts fail.
#[test]
fn a_step_aside_clears_the_goal_it_replaces_and_leaves_the_path_timestamp() {
    let mut g = GameLogic::new(flat(), 1);
    let t = Arc::new(vehicle("T"));
    let ally = g.create_object(&t, Coord3D::new(130.0, 60.0, 0.0), 0.0, 0);
    let other = g.create_object(&t, Coord3D::new(130.0, 200.0, 0.0), 0.0, 0);
    // `other` must hold a path (`unitPath == nullptr` returns, AIUpdate.cpp:3325-3326).
    g.submit(0, Command::MoveTo { ids: vec![other], pos: Coord3D::new(130.0, 260.0, 0.0) });
    for _ in 0..3 {
        g.update();
    }
    let stamp_before = g.objects[&ally].ai.path_timestamp;
    if let Some(o) = g.objects.get_mut(&ally) {
        o.ai.machine.current = AiStateType::AttackObject;
        o.ai.locomotor_goal_type = LocoGoalType::PositionExplicit;
    }
    g.ai_move_away_from_unit(ally, other);
    let a = g.find_object(ally).unwrap();
    assert_eq!(a.ai.machine.temporary, Some(AiStateType::MoveOutOfTheWay), "the ally stepped aside");
    assert_eq!(a.ai.locomotor_goal_type, LocoGoalType::None, "destroyPath cleared the goal (AIUpdate.cpp:2027)");
    assert_eq!(a.ai.path_timestamp, stamp_before, "the install does not stamp the path timestamp");
}

/// `AutoHealBehavior`'s constructor (AutoHealBehavior.cpp:98-109) draws `GameLogicRandomValue(1, delay)` for a module that
/// starts active. A `HealingDelay` of 0 draws nothing (RandomValue.cpp:267-272, RETAIL_COMPATIBLE_CRC), so the module's first
/// wake is the constructor's frame, and a delay of 1 or more draws in `1..=delay`. Revert check: the old clamp to 1 draws
/// for a zero delay and the first assert fails.
#[test]
fn a_zero_healing_delay_draws_nothing_and_wakes_on_the_spot() {
    let mut t = vehicle("Healer");
    t.auto_heal = vec![crate::behaviors::AutoHealTemplate {
        healing_amount: 10.0,
        healing_delay: 0,
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
    }];
    let mut g = GameLogic::new(flat(), 1);
    let before = g.rng_mut().clone();
    let id = g.create_object(&Arc::new(t), Coord3D::new(20.0, 20.0, 0.0), 0.0, 0);
    let o = g.find_object(id).unwrap();
    assert_eq!(o.behavior.auto_heal[0].next, 0, "a zero delay wakes at the constructor's frame");
    assert_eq!(g.rng_mut().clone().value(0, 1_000_000), before.clone().value(0, 1_000_000), "no draw was made");
}

/// A `DeliverPayload` OCL with a transport and no payload (the payload's drop is not what these tests look at).
fn delivery_ocl(delivery_distance: f32, transport: Arc<ObjectTemplate>) -> Arc<crate::ocl::OclTemplate> {
    Arc::new(crate::ocl::OclTemplate {
        name: "Delivery".into(),
        nuggets: vec![crate::ocl::OclNugget::DeliverPayload(Arc::new(crate::ocl::DeliverPayloadTemplate {
            transport: Some(transport),
            payload: Vec::new(),
            delivery_distance,
            drop_delay: 5,
            delay_delivery_frames_max: 0,
            drop_offset: (0.0, 0.0, 0.0),
            drop_variance: (0.0, 0.0, 0.0),
            is_parachute_directly: false,
            self_destruct_object: false,
            fire_weapon: false,
            strafing_weapon_slot: 0,
            strafe_length: 0.0,
        }))],
    })
}

/// `DeliverPayloadNugget::create` (ObjectCreationList.cpp:331-365): the transport starts at the OCL's primary point, the
/// creation edge nearest the source (`OCLSpecialPower` `CREATE_AT_EDGE_NEAR_SOURCE`, OCLSpecialPower.cpp:192), backed
/// off `DeliveryDistance * 1.5` along the approach (:348-352), and no `z + 120` is added. Revert check: the old start
/// (`target - dist`, `z + 120`) fails the position assert.
#[test]
fn a_delivery_transport_starts_a_slop_back_from_the_edge_near_the_source() {
    let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
    let source = g.create_object(&Arc::new(vehicle("Source")), Coord3D::new(300.0, 1000.0, 0.0), 0.0, 0);
    let target = Coord3D::new(900.0, 1000.0, 0.0);
    let ocl = delivery_ocl(300.0, Arc::new(vehicle("Plane")));
    g.run_ocl_at_angle(&ocl, source, Some(target), false, None, Some(crate::ocl::CreateLocation::EdgeNearSource));
    let (lo, _) = g.terrain().world_extent();
    let plane = g.objects.values().find(|o| o.template.name == "Plane").expect("the transport was created");
    // The nearest edge to (300, 1000) is the west edge; the approach is along +x, so the start is 450 units west of it.
    assert!((plane.position.x - (lo.x - 1.5 * 300.0)).abs() < 0.01, "start x {}", plane.position.x);
    assert!((plane.position.y - 1000.0).abs() < 0.01, "start y {}", plane.position.y);
    assert!(plane.position.z.abs() < 0.01, "no z + 120 on the start, got {}", plane.position.z);
    assert!(plane.angle.abs() < 1e-6, "the transport faces the target (orient = atan2 of the approach), got {}", plane.angle);
}

/// `ApproachState::onEnter` (DeliverPayloadAIUpdate.cpp:586) flies the transport to `moveToPos`, the target, not to an
/// exit point. Revert check: the old flight to `target + dist` fails the goal assert.
#[test]
fn a_delivery_transport_first_flies_to_the_target() {
    let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
    let source = g.create_object(&Arc::new(vehicle("Source")), Coord3D::new(300.0, 1000.0, 0.0), 0.0, 0);
    let target = Coord3D::new(900.0, 1000.0, 0.0);
    let ocl = delivery_ocl(300.0, Arc::new(vehicle("Plane")));
    g.run_ocl_at_angle(&ocl, source, Some(target), false, None, Some(crate::ocl::CreateLocation::EdgeNearSource));
    let plane = g.objects.values().find(|o| o.template.name == "Plane").unwrap().id;
    let goal = g.find_object(plane).unwrap().ai.goal().expect("the approach has a goal");
    assert!(goal.dist_2d(&target) < 1.0, "the approach ends at the target, got ({}, {})", goal.x, goal.y);
}

/// `ExitState::onEnter` (DeliverPayloadAIUpdate.cpp:1142-1158): once the payload is out and the approach is done, the
/// transport flies straight on for `HUGE_DIST` (1.2 times the map diagonal) and leaves the map. The goal of that exit
/// move lies beyond the map's far edge. Revert check: the old exit (`target + dist`) stays inside the map and fails.
#[test]
fn a_delivery_transport_exits_past_the_far_edge_after_it_drops() {
    let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
    let source = g.create_object(&Arc::new(vehicle("Source")), Coord3D::new(300.0, 1000.0, 0.0), 0.0, 0);
    let target = Coord3D::new(900.0, 1000.0, 0.0);
    let ocl = delivery_ocl(300.0, Arc::new(vehicle("Plane")));
    // `CREATE_AT_LOCATION` keeps the start on the map (450 units west of the target), so the flight is short.
    g.run_ocl_at_angle(&ocl, source, Some(target), false, None, Some(crate::ocl::CreateLocation::AtLocation));
    let plane = g.objects.values().find(|o| o.template.name == "Plane").unwrap().id;
    let (lo, hi) = g.terrain().world_extent();
    let mut far_goal = f32::MIN;
    for _ in 0..3000 {
        g.run_frames(1);
        let Some(o) = g.find_object(plane) else { break };
        if let Some(goal) = o.ai.goal() {
            far_goal = far_goal.max(goal.x);
        }
    }
    assert!(far_goal > hi.x, "the exit goal ({far_goal}) lies past the far edge ({}) of ({})", hi.x, lo.x);
}

/// A hacker with nothing held is idle, so its goal does not make it move.
#[test]
fn a_hacker_without_an_order_is_idle_so_not_moving() {
    let mut g = GameLogic::new(flat(), 1);
    let id = g.create_object(&Arc::new(vehicle("Hacker")), Coord3D::new(20.0, 40.0, 0.0), 0.0, 0);
    idle_with_a_goal(&mut g, id);
    assert!(!g.find_object(id).unwrap().is_ai_moving());
}
