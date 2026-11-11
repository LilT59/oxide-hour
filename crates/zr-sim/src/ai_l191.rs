//! Lane 191 (wave 22). Crate-internal tests (the functions under test are private, like the lane 186 and 190 tests).
//!
//! AN5 s1 f3129: crawler 263 is in its guard's return move when a guard transition runs the attack's entry in the same
//! update. The entry only queues the attack path (`requestAttackPath`, AIUpdate.cpp:520-540; the entry's own request,
//! AIStates.cpp:5516-5592), and `computeAttackPath` runs later from the end-of-frame queue (`doPathfind`, AIUpdate.cpp:
//! 380-470; `AI::update`, AI.cpp:359). Until then the unit keeps its move: `m_path` and its `PositionOnPath` goal are
//! still there, so `doLocomotor` drives it (AIUpdate.cpp:2254). The port's entry called `stop()`, which cleared both, so
//! the crawler stopped one frame early and the attack path was built from a different position.

use crate::ai::{LocoGoalType, PathRequest};
use crate::combat::AttackExitConditions;
use crate::object::ObjectId;
use crate::math::Coord3D;
use crate::template::{kind_of, LocomotorKind, LocomotorParams, ObjectTemplate};
use crate::terrain::TerrainLogic;
use crate::weapon::{CombatTemplate, WeaponSetTemplate, WeaponTemplate};
use crate::GameLogic;
use std::sync::Arc;

/// An armed vehicle (a 40-unit cannon), as the lane 186 tests build it.
fn armed(name: &str) -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
    t.geometry.major_radius = 8.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.vision_range = 50.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 60.0, 120.0, 120.0, 360.0));
    let mut w = WeaponTemplate::new("Cannon");
    w.primary_damage = 20.0;
    w.damage_type = crate::body::DamageType::ArmorPiercing;
    w.attack_range = 40.0;
    w.min_delay_between_shots = 10;
    w.max_delay_between_shots = 10;
    let mut set = WeaponSetTemplate::default();
    set.weapons[0] = Some(Arc::new(w));
    t.combat = CombatTemplate { weapon_set: Some(set), ..CombatTemplate::default() };
    Arc::new(t)
}

/// A weaponless, very tough vehicle: an enemy that never fights back.
fn dummy() -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new("Dummy");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 8.0;
    t.max_health = 1000.0;
    t.initial_health = 1000.0;
    t.vision_range = 40.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 240.0));
    t.physics = Some(crate::physics::PhysicsTemplate::default());
    Arc::new(t)
}

fn world() -> GameLogic {
    let mut g = GameLogic::new(TerrainLogic::flat(800, 800, 0), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    g
}

/// The guard attack's exit conditions as the guard states set them for a victim: the no-unit-found exit only.
fn exit_at(center: Coord3D) -> AttackExitConditions {
    AttackExitConditions {
        exit_if_no_unit_found: true,
        exit_if_expired_duration: false,
        exit_if_outside_radius: false,
        center,
        radius_sqr: 0.0,
        give_up_frame: 0,
    }
}

/// An attack with a move in progress: the attack in `phase`, a two-node path with a position goal, and a queued attack
/// request (what an approach or pursue has when it is exited mid-request).
fn attacking_with_move(g: &mut GameLogic, a: ObjectId, v: ObjectId, phase: crate::combat::AttackPhase) {
    let o = g.objects.get_mut(&a).expect("the crawler exists");
    let mut order = crate::combat::AttackOrder::new(crate::combat::AttackTarget::Object(v));
    order.phase = phase;
    o.combat.attack = Some(order);
    o.ai.path = vec![Coord3D::new(100.0, 100.0, 0.0), Coord3D::new(150.0, 100.0, 0.0)];
    o.ai.locomotor_goal_type = LocoGoalType::PositionOnPath;
    o.ai.request = PathRequest::Attack;
    o.ai.waiting_for_path = true;
}

/// `AIAttackApproachTargetState::onExit` (AIStates.cpp:2801-2806) calls `AIInternalMoveToState::onExit`, whose
/// `friend_endingMove` sets `m_movementComplete` (AIStates.cpp:2804; AIUpdate.cpp:2046-2050). The attack's
/// `AIAttackState::onExit` deletes its sub-machine (AIStates.cpp:5723), so the approach exits with the attack. Nothing
/// is dropped in the exit itself: the next update's completed-move block drops the path, the goal and the queued attack
/// request (AIUpdate.cpp:1035-1041, :2018-2028). Revert check: remove the `ai_internal_move_exit` call from
/// `end_attack_with`; `movement_complete` stays false and the first assertion fails.
#[test]
fn an_approach_exit_ends_its_move_and_the_completed_block_drops_it() {
    let mut g = world();
    g.add_ai_player(0, crate::skirmish_ai::SkirmishAiConfig::default());
    let a = g.create_object(&armed("Crawler"), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let v = g.create_object(&dummy(), Coord3D::new(300.0, 100.0, 0.0), 0.0, 1);
    attacking_with_move(&mut g, a, v, crate::combat::AttackPhase::Approach);
    g.end_attack_internal(a);
    let o = g.find_object(a).unwrap();
    assert!(o.combat.attack.is_none(), "the attack ends");
    assert!(o.ai.movement_complete, "the approach's exit ends its move (friend_endingMove)");
    assert_eq!(o.ai.path.len(), 2, "the exit itself drops nothing: the completed block does");
    g.ai_end_completed_move(a);
    let o = g.find_object(a).unwrap();
    assert!(o.ai.path.is_empty(), "the completed block drops the path (destroyPath, AIUpdate.cpp:2018-2028)");
    assert_eq!(o.ai.locomotor_goal_type, LocoGoalType::None, "and the locomotor goal (setLocomotorGoalNone)");
    assert_eq!(o.ai.request, PathRequest::None, "and the queued attack request (m_isAttackPath = FALSE)");
    assert!(!o.ai.waiting_for_path, "and the wait for it");
}

/// `AIAttackPursueTargetState::onExit` (AIStates.cpp:3130-3136) ends the move the same way as the approach's exit. The
/// command path (`end_attack`, from `aiIdle` or a new order's `clear()`) ends it too, because `StateMachine::clear` exits
/// the current sub-state (StateMachine.cpp:281-288). Revert check: as the approach test.
#[test]
fn a_pursue_exit_ends_its_move_on_the_command_path_too() {
    let mut g = world();
    g.add_ai_player(0, crate::skirmish_ai::SkirmishAiConfig::default());
    let a = g.create_object(&armed("Crawler"), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let v = g.create_object(&dummy(), Coord3D::new(300.0, 100.0, 0.0), 0.0, 1);
    attacking_with_move(&mut g, a, v, crate::combat::AttackPhase::Pursue);
    g.end_attack(a);
    let o = g.find_object(a).unwrap();
    assert!(o.ai.movement_complete, "the pursue's exit ends its move");
    g.ai_end_completed_move(a);
    let o = g.find_object(a).unwrap();
    assert!(o.ai.path.is_empty() && o.ai.request == PathRequest::None, "the completed block drops the pursue's path and request");
}

/// The aim and fire sub-states are not moves: `AIAttackAimAtTargetState` and `AIAttackFireWeaponState` have no
/// `AIInternalMoveToState` base (AIStates.cpp:5714-5745 clears only the statuses, the victim and the turrets). A unit that
/// was aiming with a move still in progress (a guard's return) keeps that move. Revert check: make `end_attack_with` treat
/// every phase as a move; `movement_complete` is then set and the first assertion fails.
#[test]
fn an_aim_or_fire_exit_leaves_the_move_alone() {
    for phase in [crate::combat::AttackPhase::Aim, crate::combat::AttackPhase::Fire] {
        let mut g = world();
        g.add_ai_player(0, crate::skirmish_ai::SkirmishAiConfig::default());
        let a = g.create_object(&armed("Crawler"), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let v = g.create_object(&dummy(), Coord3D::new(300.0, 100.0, 0.0), 0.0, 1);
        attacking_with_move(&mut g, a, v, phase);
        g.end_attack_internal(a);
        g.ai_end_completed_move(a);
        let o = g.find_object(a).unwrap();
        assert!(o.combat.attack.is_none(), "{phase:?}: the attack ends");
        assert!(!o.ai.movement_complete, "{phase:?}: no move exit");
        assert_eq!(o.ai.path.len(), 2, "{phase:?}: the path is kept");
        assert_eq!(o.ai.locomotor_goal_type, LocoGoalType::PositionOnPath, "{phase:?}: the position goal is kept");
        assert_eq!(o.ai.request, PathRequest::Attack, "{phase:?}: the queued request is kept");
    }
}

/// The attack state's exit on a state change (`ai_attack_exit`, the `AttackObject` arm of `ai_state_exit`) is an internal
/// transition: `AIAttackState::onExit` does not notify (AIStates.cpp:5714-5745; StateMachine.cpp:126-140), so the module
/// stays asleep. Only the command path (`ai_clear`, `end_attack`) wakes it. Revert check: make `ai_attack_exit` call
/// `end_attack` again; the key is pulled to the next frame and the assertion fails (lane 191 R28 F4).
#[test]
fn an_attack_state_exit_does_not_wake_the_module() {
    let mut g = world();
    g.add_ai_player(0, crate::skirmish_ai::SkirmishAiConfig::default());
    let a = g.create_object(&armed("Crawler"), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let v = g.create_object(&dummy(), Coord3D::new(300.0, 100.0, 0.0), 0.0, 1);
    let now = g.frame();
    let h = g.objects[&a].ai_handle.expect("an AI module");
    g.schedule.awaken(h, now, now + 50);
    g.objects.get_mut(&a).expect("the crawler exists").combat.attack =
        Some(crate::combat::AttackOrder::new(crate::combat::AttackTarget::Object(v)));
    g.ai_attack_exit(a);
    assert!(g.find_object(a).unwrap().combat.attack.is_none(), "the attack ends");
    assert_eq!(g.schedule.slot(h.0).next_call_and_phase >> 2, now + 50, "the internal exit does not wake the module");
}

/// `friend_startingMove` cancels the block (AIUpdate.cpp:2034-2036): a move that starts before the completed block runs
/// (the next state's `computePath`, AIStates.cpp:1606-1607) keeps its path. Revert check: drop the `friend_starting_move`
/// call and the path is dropped.
#[test]
fn a_move_started_after_an_approach_exit_keeps_its_path() {
    let mut g = world();
    g.add_ai_player(0, crate::skirmish_ai::SkirmishAiConfig::default());
    let a = g.create_object(&armed("Crawler"), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let v = g.create_object(&dummy(), Coord3D::new(300.0, 100.0, 0.0), 0.0, 1);
    attacking_with_move(&mut g, a, v, crate::combat::AttackPhase::Approach);
    g.end_attack_internal(a);
    g.objects.get_mut(&a).expect("the crawler exists").ai.friend_starting_move();
    g.ai_end_completed_move(a);
    let o = g.find_object(a).unwrap();
    assert_eq!(o.ai.path.len(), 2, "the new move's path survives the completed block");
}

/// `AIGuardOuterState::onEnter` -> `AIAttackState::onEnter` (AIGuard.cpp:548-562, AIStates.cpp:5516-5592) does not stop
/// the unit: the guard's return move keeps its path and its position goal, and the attack path is only queued (it replaces
/// the path when `computeAttackPath` runs, AIUpdate.cpp:1782). Revert check: put back the `o.ai.stop()` that
/// `ai_guard_begin_attack` had; the path is then empty and the goal `None`, and both assertions fail.
#[test]
fn a_guard_attack_entry_keeps_the_units_move_until_its_path_is_installed() {
    let mut g = world();
    // A computer player (AN5's crawler is the AI's): a human player's AI-commanded attack is refused its approach (lane 190).
    g.add_ai_player(0, crate::skirmish_ai::SkirmishAiConfig::default());
    let a = g.create_object(&armed("Crawler"), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let v = g.create_object(&dummy(), Coord3D::new(300.0, 100.0, 0.0), 0.0, 1);
    g.frame = 500;
    {
        // The unit is mid-move on its return (a path and a position goal, as `AIUpdate.cpp:2254-2285` reads them).
        let o = g.objects.get_mut(&a).expect("the crawler exists");
        o.ai.path = vec![Coord3D::new(100.0, 100.0, 0.0), Coord3D::new(150.0, 100.0, 0.0)];
        o.ai.locomotor_goal_type = LocoGoalType::PositionOnPath;
    }
    g.ai_guard_begin_attack(a, v, exit_at(Coord3D::new(100.0, 100.0, 0.0)), false);
    let o = g.find_object(a).unwrap();
    assert_eq!(o.ai.locomotor_goal_type, LocoGoalType::PositionOnPath, "the entry keeps the position goal (no aiIdle)");
    assert_eq!(o.ai.path.len(), 2, "the entry keeps the move's path until the queued attack path replaces it");
    assert!(o.ai.waiting_for_path, "the attack path is queued for the end-of-frame pass");
}
