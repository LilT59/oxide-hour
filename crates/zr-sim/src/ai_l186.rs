//! Lane 186 (wave 21). Crate-internal tests (the functions under test are private, like the lane 182 and 184 tests).
//!
//! AN5 s1 f3129: a Troop Crawler's guard transition (`AIGuardOuterState::onEnter`, AIGuard.cpp:525-570) runs its
//! attack's entry in the same frame: an out-of-range victim's approach path is requested at once (AIStates.cpp:5516-5592,
//! :2554-2660, :2409-2513). The port requested it on the next update, one frame late. The pursue request is a non-final
//! `requestPath(&m_goalPosition, false)` (AIStates.cpp:2901-2905), and the current victim is set by the update, not on entry
//! (AIStates.cpp:5630, :2704; `setCurrentVictim` is cleared on exit, :5739).

use super::*;
use crate::ai::PathRequest;
use crate::combat::AttackExitConditions;
use crate::math::Coord3D;
use crate::template::{kind_of, LocomotorKind, LocomotorParams, ObjectTemplate};
use crate::terrain::TerrainLogic;
use crate::weapon::{CombatTemplate, WeaponSetTemplate, WeaponTemplate};
use crate::GameLogic;
use std::sync::Arc;

/// An armed vehicle (a 40-unit cannon). `turret` gives it a turret, which `canPursue` needs (`turret_turn_rate`).
fn armed(name: &str, turret: Option<f32>) -> Arc<ObjectTemplate> {
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
    t.combat = CombatTemplate { weapon_set: Some(set), turret_turn_rate: turret, ..CombatTemplate::default() };
    Arc::new(t)
}

/// A weaponless, very tough vehicle with a slow locomotor (speed 30): an enemy that never fights back.
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

/// `AIGuardOuterState::onEnter` -> `AIAttackState::onEnter` (AIStates.cpp:5516-5592): the out-of-range victim's
/// approach path is requested in the enter, so the frame's pathfinder pass (`process_pathfind_queue`, the end of the frame,
/// `logic.rs`) installs it. Revert check: remove the `ai_attack_enter_pass` call from `ai_guard_begin_attack`; the request
/// is then queued on the next update, after this pass, and no path exists here.
#[test]
fn a_guard_attack_queues_its_approach_path_in_the_same_frame() {
    let mut g = world();
    // A computer player (AN5's crawler is the AI's): a human player's AI-commanded attack is refused its approach (lane 190).
    g.add_ai_player(0, crate::skirmish_ai::SkirmishAiConfig::default());
    let a = g.create_object(&armed("Crawler", None), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let v = g.create_object(&dummy(), Coord3D::new(300.0, 100.0, 0.0), 0.0, 1);
    g.frame = 500;
    g.ai_guard_begin_attack(a, v, exit_at(Coord3D::new(100.0, 100.0, 0.0)), false);
    g.process_pathfind_queue();
    let o = g.find_object(a).unwrap();
    assert!(o.ai.raw_path.is_some(), "the approach path is installed by this frame's pathfinder pass");
}

/// The current victim (`getCurrentVictim`, the snapshot's `atk`) is set by the attack's update (AIStates.cpp:5630), not
/// by its entry. Revert check: drop the `!entering` gate in `ai_attack_pass` so the entry sets it too; the first assertion
/// then fails.
#[test]
fn the_current_victim_is_set_by_the_first_update_not_by_the_entry() {
    let mut g = world();
    g.add_ai_player(0, crate::skirmish_ai::SkirmishAiConfig::default());
    let a = g.create_object(&armed("Crawler", None), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let v = g.create_object(&dummy(), Coord3D::new(300.0, 100.0, 0.0), 0.0, 1);
    g.frame = 500;
    g.ai_guard_begin_attack(a, v, exit_at(Coord3D::new(100.0, 100.0, 0.0)), false);
    assert_eq!(g.find_object(a).unwrap().ai.machine.current_victim, ObjectId::INVALID, "the entry sets no current victim");
    g.ai_attack_update_object(a);
    assert_eq!(g.find_object(a).unwrap().ai.machine.current_victim, v, "the update sets the current victim");
}

/// `AIAttackPursueTargetState::computePath` (AIStates.cpp:2901-2905) calls `requestPath(&m_goalPosition, false)` after
/// `setAdjustsDestination(true)`: the pursue is a non-final goal, so `computePath`'s straight-line shortcut (AIUpdate.cpp:1709)
/// applies to it. Setup: a turreted attacker (`canPursue` needs a turret) and a victim moving away at speed 10 (between a tenth
/// of and all of the attacker's speed 60, AIStates.cpp:2385-2401). Revert check: remove `is_final_goal = false` after the
/// `start_move` in the pursue branch of `ai_attack_pass`; `is_final_goal` is then the adjust flag, true.
#[test]
fn a_pursue_request_is_not_a_final_goal() {
    let mut g = world();
    let a = g.create_object(&armed("Crawler", Some(360.0)), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let v = g.create_object(&dummy(), Coord3D::new(200.0, 100.0, 0.0), 0.0, 1);
    let p = g.objects.get_mut(&v).and_then(|o| o.physics.as_mut()).expect("the victim has a physics module");
    p.velocity = Coord3D::new(1.0, 0.0, 0.0);
    g.frame = 500;
    g.ai_guard_begin_attack(a, v, exit_at(Coord3D::new(100.0, 100.0, 0.0)), false);
    g.ai_attack_update_object(a);
    let o = g.find_object(a).unwrap();
    assert_eq!(o.ai.request, PathRequest::Move, "the pursue asks for a move path");
    // The state's flag (`AIInternalMoveToState::m_adjustsDestination`, the machine's `move_adjusts`): the request's compute
    // never adjusts again (`ai_request_path`), so the state's own flag is what `setAdjustsDestination(true)` sets.
    assert!(o.ai.machine.move_adjusts, "the pursue adjusts its destination (setAdjustsDestination(true))");
    assert!(!o.ai.is_final_goal, "the pursue's requestPath is not a final goal (AIStates.cpp:2905)");
}
