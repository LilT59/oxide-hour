//! Lane 137 (wave 16): the R11 review's sim findings. Each test names the C++ it ports and the
//! revert check that makes it fail.
//!
//! F2 (`Pathfinder::moveAllies`, `AIPathfind.cpp:10262`: the ally's `isMoving` veto) was tested at the seam of the
//! deferred ally moves, which lane 177 removed: `aiMoveAwayFromUnit` now runs per visit, and the per-visit tests are
//! `GameLogic::ally_visit_pushable` (logic.rs).

use std::sync::Arc;
use zr_sim::ai_states::{AiStateType, CommandSource};
use zr_sim::math::Coord3D;
use zr_sim::orders::{GuardMode, GuardTarget};
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::weapon::{CombatTemplate, WeaponSetTemplate, WeaponTemplate};
use zr_sim::{Command, GameLogic, ObjectTemplate, TerrainLogic};

fn world(players: u32) -> GameLogic {
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    for p in 0..players as u8 {
        g.add_player(p, 10000);
    }
    g
}

fn mobile(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.geometry.major_radius = 5.0;
    t.geometry.height = 6.0;
    t.vision_range = 300.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 60.0, 100.0, 100.0, 360.0));
    // Every shipped mobile template carries `PhysicsBehavior`; without it the sim's reduced
    // kinematic mover stands in and a short goal behind the unit can be circled instead of reached.
    t.physics = Some(zr_sim::physics::PhysicsTemplate::default());
    t
}

fn armed(mut t: ObjectTemplate) -> ObjectTemplate {
    t.kind_of |= kind_of::CAN_ATTACK;
    let mut w = WeaponTemplate::new("Gun");
    w.primary_damage = 20.0;
    w.primary_damage_radius = 5.0;
    w.secondary_damage_radius = 5.0;
    w.attack_range = 120.0;
    w.min_delay_between_shots = 10;
    w.max_delay_between_shots = 10;
    let mut set = WeaponSetTemplate::default();
    set.weapons[0] = Some(Arc::new(w));
    t.combat = CombatTemplate { weapon_set: Some(set), ..CombatTemplate::default() };
    t
}


/// `AIGuardReturnState::onExit` (`AIGuard.cpp:684-687`) runs `AIInternalMoveToState::onExit`, so the
/// return move ends (`friend_endingMove`) on the transition to `AI_GUARD_IDLE` and the
/// completed-move block of the same update drops the path (`AIUpdate.cpp:1035-1041`).
/// Revert check: with the direct `ai_guard_idle_enter` call (no move exit) the guard idles with its
/// return path still held, so `has_path()`/`is_moving()` stay true.
#[test]
fn a_finished_guard_return_drops_its_path() {
    let mut g = world(1);
    let id = g.create_object(&Arc::new(armed(mobile("Guard"))), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    g.submit(0, Command::Guard { ids: vec![id], target: GuardTarget::Point(Coord3D::new(100.0, 180.0, 0.0)), mode: GuardMode::Normal });
    let mut arrived = false;
    for _ in 0..900 {
        g.update();
        let o = g.find_object(id).unwrap();
        if o.ai.machine.current == AiStateType::Guard && o.ai.machine.guard_sub == AiStateType::GuardIdle {
            arrived = true;
            break;
        }
    }
    assert!(arrived, "the guard machine reached its idle sub-state after the return");
    let o = g.find_object(id).unwrap();
    assert!(!o.ai.has_path(), "the return move's path is dropped by the completed-move block");
    assert!(!o.ai.is_moving(), "the guard is no longer flagged as moving");
}

/// Leaving the guard machine while `AI_GUARD_RETURN` is the current sub-state also runs its `onExit`:
/// `AIGuardState::onExit` deletes the guard machine and `StateMachine::~StateMachine` calls the current
/// state's `onExit(EXIT_RESET)` (`StateMachine.cpp:281-288`). A player's stop during a guard return
/// must end the move's path.
/// Revert check: without the `guard_sub == GuardReturn` exit in `ai_guard_state_exit` the stopped
/// guard keeps its return path.
#[test]
fn a_cancelled_guard_return_ends_its_move() {
    let mut g = world(1);
    let id = g.create_object(&Arc::new(armed(mobile("Guard"))), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    g.submit(0, Command::Guard { ids: vec![id], target: GuardTarget::Point(Coord3D::new(100.0, 260.0, 0.0)), mode: GuardMode::Normal });
    let mut returning = false;
    for _ in 0..120 {
        g.update();
        let o = g.find_object(id).unwrap();
        if o.ai.machine.current == AiStateType::Guard && o.ai.machine.guard_sub == AiStateType::GuardReturn && o.ai.has_path() {
            returning = true;
            break;
        }
    }
    assert!(returning, "the guard started its return move");
    g.submit(0, Command::Stop { ids: vec![id] });
    g.update();
    let o = g.find_object(id).unwrap();
    assert_eq!(o.ai.machine.current, AiStateType::Idle, "the stop left the guard state");
    assert!(!o.ai.has_path(), "the cancelled return's path is gone");
    assert!(!o.ai.is_moving());
}

/// `AIEnterState::update`'s enemy fallback (`AIStates.cpp:6339-6372`): when `canEnterObject` fails
/// on an enemy goal, `getCanAttackObject(..., ATTACK_NEW_TARGET)` decides between an
/// `aiAttackObject(goal, NO_MAX_SHOTS_LIMIT, getLastCommandSource())` and `STATE_FAILURE`. The port
/// used to stop instead of attacking when an enemy garrisoned the building first.
/// Revert check: with the plain `return StateReturn::Failure`, the unit goes idle with no attack order.
#[test]
fn an_enemy_that_blocks_the_enter_is_attacked() {
    let mut g = world(2);
    let id = g.create_object(&Arc::new(armed(mobile("Ranger"))), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let mut building = ObjectTemplate::new("Bunker");
    building.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;
    building.max_health = 500.0;
    building.initial_health = 500.0;
    building.geometry.major_radius = 15.0;
    building.geometry.minor_radius = 15.0;
    building.contain = Some(Arc::new(zr_sim::contain::ContainTemplate {
        kind: zr_sim::contain::ContainKind::Garrison,
        capacity: 4,
        allow_inside_kind_of: None,
        forbid_inside_kind_of: 0,
        allow_allies_inside: false,
        allow_enemies_inside: false,
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
        is_enclosing_container: true,
        fire_points: zr_sim::contain::GarrisonFirePoints::default(),
        passengers_in_turret: false,
        weapon_bonus_passed_to_passengers: false,
        immune_to_clear_building_attacks: false,
    }));
    let b = g.create_object(&Arc::new(building), Coord3D::new(240.0, 100.0, 0.0), 0.0, 1);
    // Neutral when the order is given (`canEnterObject` accepts), so the order reaches `AI_ENTER`.
    g.submit(0, Command::Enter { ids: vec![id], container: b });
    g.update();
    assert_eq!(g.find_object(id).unwrap().ai.machine.current, AiStateType::Enter, "the enter order was accepted");
    // The building turns enemy-held before the unit arrives: the per-update `canEnterObject` fails.
    g.set_relationship(0, 1, zr_sim::damage::Relationship::Enemies);
    g.update();
    let o = g.find_object(id).unwrap();
    assert_eq!(o.ai.machine.current, AiStateType::AttackObject, "the enter turned into an attack");
    assert_eq!(o.combat.attack.as_ref().map(|a| a.target), Some(zr_sim::combat::AttackTarget::Object(b)));
    assert_eq!(o.ai.machine.last_command_source, CommandSource::Player, "aiAttackObject keeps the last command source");
}
