//! Lane 166 (wave 19), AN1 s1 draw 4281 (f2685, `range 30` vs C++ `range 15`) and the guard machine's
//! transitions around it (`AIGuard.cpp`):
//! - Scan timers: `AIGuardIdleState` uses `m_guardEnemyScanRate` for its first scan (`:723`) and for every
//!   later one (`:737`); `AIGuardReturnState` uses `m_guardEnemyReturnScanRate` (`:645`, `:674`). The port
//!   used `LOGICFRAMES_PER_SECOND` for both.
//! - Transitions: `AI_GUARD_OUTER` goes to `AI_GUARD_GET_CRATE` on success and on failure (`:192`),
//!   and GET_CRATE goes to `AI_GUARD_RETURN` (`:193`). Only RETURN's success reaches `AI_GUARD_IDLE`.
//! - The outer state's give-up timer is reset before the attack's exit test reads it (`:578-595`,
//!   `ExitConditions::shouldExit`, `:131`).
//! - An inner state with no attack or a lost nemesis is SUCCESS, which goes to `AI_GUARD_OUTER` (`:183`).
//!   The port left the sub-state where it was, so the guard stopped.
//!
//! Each test uses synthetic data only; `tests/l166_guard.rs` covers the same states through the public API.

use super::*;
use crate::ai::AiTunables;
use crate::ai_states::AiStateType;
use crate::Command;
use crate::combat::{AttackExitConditions, AttackOrder, AttackTarget};
use crate::damage::DeathType;
use crate::body::{DamageInfo, DamageType};
use crate::orders::{GuardMode, GuardTarget};
use crate::template::{kind_of, LocomotorKind, LocomotorParams, ObjectTemplate};
use crate::weapon::{CombatTemplate, WeaponSetTemplate, WeaponTemplate};
use std::sync::Arc;

/// An armed vehicle whose guard scan radius is `vision` and whose weapon reaches `range`.
fn armed(name: &str, vision: f32, range: f32) -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
    t.geometry.major_radius = 8.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.vision_range = vision;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 60.0, 120.0, 120.0, 360.0));
    let mut w = WeaponTemplate::new("Cannon");
    w.primary_damage = 20.0;
    w.damage_type = DamageType::ArmorPiercing;
    w.attack_range = range;
    w.min_delay_between_shots = 10;
    w.max_delay_between_shots = 10;
    let mut set = WeaponSetTemplate::default();
    set.weapons[0] = Some(Arc::new(w));
    t.combat = CombatTemplate { weapon_set: Some(set), ..CombatTemplate::default() };
    Arc::new(t)
}

/// A weaponless, very tough vehicle: a guard's chase target that never fights back.
fn dummy() -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new("Dummy");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 8.0;
    t.max_health = 1000.0;
    t.initial_health = 1000.0;
    t.vision_range = 40.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 240.0));
    Arc::new(t)
}

fn world() -> GameLogic {
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    g
}

fn guard_sub(g: &GameLogic, id: ObjectId) -> AiStateType {
    g.find_object(id).expect("object").ai.machine.guard_sub
}

/// Order `id` to guard `post`.
fn guard_point(g: &mut GameLogic, id: ObjectId, post: Coord3D) {
    g.submit(0, Command::Guard { ids: vec![id], target: GuardTarget::Point(post), mode: GuardMode::Normal });
}

/// The guard's sub-state is `AI_GUARD_IDLE` (`AIGuard.cpp:723`) and its scan timer is set after each scan.
#[test]
fn an_idle_guard_rescans_every_guard_enemy_scan_rate_frames() {
    // `AIGuardIdleState::update` (`AIGuard.cpp:737`): the next scan is `m_guardEnemyScanRate` frames away.
    // Revert check: change the idle update's `rate` back to `LOGICFRAMES_PER_SECOND` and this fails (gap 30).
    let mut g = world();
    g.set_ai_tunables(AiTunables { guard_enemy_scan_rate: 7, ..AiTunables::default() });
    let id = g.create_object(&armed("Gun", 100.0, 50.0), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    guard_point(&mut g, id, Coord3D::new(100.0, 100.0, 0.0));
    for _ in 0..300 {
        g.update();
        if guard_sub(&g, id) == AiStateType::GuardIdle {
            break;
        }
    }
    assert_eq!(guard_sub(&g, id), AiStateType::GuardIdle, "the guard reaches its post");
    let mut values = Vec::new();
    for _ in 0..80 {
        g.update();
        let next = g.find_object(id).unwrap().ai.machine.guard.next_scan_time;
        if values.last() != Some(&next) {
            values.push(next);
        }
    }
    // The first value is the entry's random offset; the first gap ends at the first scan, so only the gaps
    // between later scans measure the period.
    let gaps: Vec<u32> = values.windows(2).skip(1).map(|w| w[1] - w[0]).collect();
    assert!(gaps.len() >= 3, "several scans in 80 frames: {values:?}");
    assert!(gaps.iter().all(|d| *d == 7), "every idle scan is m_guardEnemyScanRate apart: {gaps:?}");
}

/// `AIGuardReturnState::update` (`AIGuard.cpp:674`): the return rescans every `m_guardEnemyReturnScanRate` frames.
#[test]
fn a_returning_guard_rescans_every_guard_enemy_return_scan_rate_frames() {
    // Revert check: change the return update's `rate` back to `LOGICFRAMES_PER_SECOND` and this fails (gap 30).
    let mut g = world();
    g.set_ai_tunables(AiTunables { guard_enemy_return_scan_rate: 9, ..AiTunables::default() });
    let id = g.create_object(&armed("Gun", 100.0, 50.0), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    guard_point(&mut g, id, Coord3D::new(300.0, 100.0, 0.0));
    let mut values = Vec::new();
    for _ in 0..400 {
        g.update();
        if guard_sub(&g, id) != AiStateType::GuardReturn {
            break;
        }
        let next = g.find_object(id).unwrap().ai.machine.guard.next_scan_time;
        if values.last() != Some(&next) {
            values.push(next);
        }
    }
    let gaps: Vec<u32> = values.windows(2).skip(1).map(|w| w[1] - w[0]).collect();
    assert!(gaps.len() >= 3, "several return scans on the way back: {values:?}");
    assert!(gaps.iter().all(|d| *d == 9), "every return scan is m_guardEnemyReturnScanRate apart: {gaps:?}");
}

/// A guard that loses its chase target (the target dies inside the inner state) enters `AI_GUARD_OUTER`,
/// whose SUCCESS goes to `AI_GUARD_GET_CRATE` and then `AI_GUARD_RETURN` (`AIGuard.cpp:183`, `:192`, `:193`)
/// before it can idle. The port went Outer -> Idle directly.
#[test]
fn a_guard_whose_target_dies_returns_to_its_post_before_it_idles() {
    // Revert check: make `ai_guard_outer_enter`'s early exit (nemesis gone) set `GuardIdle` and return, instead of
    // `ai_guard_get_crate_then_return`, and this fails (no `GuardReturn` in the sequence).
    let mut g = world();
    let guard = g.create_object(&armed("Gun", 150.0, 60.0), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let enemy = g.create_object(&dummy(), Coord3D::new(130.0, 100.0, 0.0), 0.0, 1);
    guard_point(&mut g, guard, Coord3D::new(100.0, 100.0, 0.0));
    for _ in 0..200 {
        g.update();
        if g.find_object(guard).is_some_and(|o| o.combat.attack.is_some()) {
            break;
        }
    }
    assert!(g.find_object(guard).unwrap().combat.attack.is_some(), "the guard engages the enemy in range");
    g.damage_object_with_death(enemy, DamageInfo { source: guard, amount: 1_000_000.0, kind: DamageType::Unresistable }, DeathType::Normal);
    let mut seq = Vec::new();
    for _ in 0..200 {
        g.update();
        let s = guard_sub(&g, guard);
        if seq.last() != Some(&s) {
            seq.push(s);
        }
        if s == AiStateType::GuardIdle {
            break;
        }
    }
    let ret = seq.iter().position(|s| *s == AiStateType::GuardReturn);
    let idle = seq.iter().position(|s| *s == AiStateType::GuardIdle);
    assert!(matches!((ret, idle), (Some(r), Some(i)) if r < i), "return, then idle: {seq:?}");
}

/// `AIGuardOuterState::update` (`AIGuard.cpp:578-595`) resets the give-up frame before the attack's
/// `ExitConditions::shouldExit` (`:131`) reads it. The nemesis here is inside the guard's range, so the
/// reset gives the chase more time and the attack goes on. The port tested the expiry first and dropped it.
#[test]
fn an_outer_chase_whose_nemesis_is_still_in_range_keeps_going_past_the_give_up_frame() {
    // Revert check: move the give-up test back before the reset (the old `now >= give_up` block) and the
    // guard leaves the outer state at once (`GuardReturn`), so this fails.
    let mut g = world();
    let guard = g.create_object(&armed("Gun", 200.0, 60.0), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let enemy = g.create_object(&dummy(), Coord3D::new(130.0, 100.0, 0.0), 0.0, 1);
    let centre = Coord3D::new(100.0, 100.0, 0.0);
    {
        let o = g.objects.get_mut(&guard).unwrap();
        o.ai.machine.current = AiStateType::Guard;
        o.ai.machine.guard_sub = AiStateType::GuardOuter;
        o.ai.machine.guard_attack_active = true;
        o.ai.machine.in_guard_attack = true;
        o.ai.machine.guard.nemesis = enemy;
        o.ai.machine.guard.attack_center = centre;
        o.ai.machine.guard.attack_radius_sqr = 200.0 * 200.0;
        // Already expired: the old pre-check would finish here.
        o.ai.machine.guard.attack_give_up_frame = 0;
        let exit = AttackExitConditions {
            exit_if_no_unit_found: true,
            exit_if_expired_duration: true,
            exit_if_outside_radius: true,
            center: centre,
            radius_sqr: 200.0 * 200.0,
            give_up_frame: 0,
        };
        o.combat.attack = Some(AttackOrder::new(AttackTarget::Object(enemy)).with_exit(exit, true));
    }
    g.ai_machine_update(guard);
    assert_eq!(guard_sub(&g, guard), AiStateType::GuardOuter, "still chasing the nemesis in range");
    assert!(g.find_object(guard).unwrap().combat.attack.is_some(), "the attack is still running");
}

/// `AIGuardOuterState::update` (`AIGuard.cpp:573`): when the chase ends while the outer state is running (the
/// give-up frame has passed and the nemesis is out of the guard's range, so nothing resets the timer), the
/// attack's SUCCESS goes to `AI_GUARD_GET_CRATE` and then `AI_GUARD_RETURN` (`AIGuard.cpp:192`, `:193`).
#[test]
fn an_outer_chase_that_expires_out_of_range_returns_to_its_post() {
    // Revert check: put `guard_sub = GuardIdle` and `ai_guard_idle_enter` back in the outer update's finish branch
    // and the sub-state becomes `GuardIdle`, so this fails.
    let mut g = world();
    let guard = g.create_object(&armed("Gun", 200.0, 60.0), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let enemy = g.create_object(&dummy(), Coord3D::new(600.0, 100.0, 0.0), 0.0, 1);
    let centre = Coord3D::new(100.0, 100.0, 0.0);
    {
        let o = g.objects.get_mut(&guard).unwrap();
        o.ai.machine.current = AiStateType::Guard;
        o.ai.machine.guard_sub = AiStateType::GuardOuter;
        o.ai.machine.guard_attack_active = true;
        o.ai.machine.in_guard_attack = true;
        o.ai.machine.guard.nemesis = enemy;
        o.ai.machine.guard.attack_center = centre;
        o.ai.machine.guard.attack_radius_sqr = 200.0 * 200.0;
        o.ai.machine.guard.attack_give_up_frame = 0;
        let exit = AttackExitConditions {
            exit_if_no_unit_found: true,
            exit_if_expired_duration: true,
            exit_if_outside_radius: true,
            center: centre,
            radius_sqr: 200.0 * 200.0,
            give_up_frame: 0,
        };
        o.combat.attack = Some(AttackOrder::new(AttackTarget::Object(enemy)).with_exit(exit, true));
    }
    g.ai_machine_update(guard);
    assert_eq!(guard_sub(&g, guard), AiStateType::GuardReturn, "the chase's end goes to the return, not the idle");
    assert!(g.find_object(guard).unwrap().combat.attack.is_none(), "the chase is over");
}

/// `AIGuardOuterState::onEnter` (`AIGuard.cpp:546`): the chase's exit radius is the adjusted vision range squared,
/// with no minimum of one unit. A guard whose outer range is under a unit gets that smaller radius.
#[test]
fn a_guard_chase_keeps_a_sub_unit_outer_range() {
    // Revert check: put `.max(1.0)` back on the outer range in `ai_guard_outer_enter` and the radius becomes 1.0
    // (sqr 1.0) instead of (0.4 * 1.5)^2 = 0.36, so this fails.
    let mut g = world();
    g.ai_tunables.guard_outer_modifier_human = 1.5;
    let centre = Coord3D::new(100.0, 100.0, 0.0);
    let guard = g.create_object(&armed("Gun", 0.4, 60.0), centre, 0.0, 0);
    // Lane 184: the enemy is inside the 0.6-unit outer circle. `AIAttackState::onEnter` exits at once when the victim
    // is already outside its radius (AIStates.cpp:5526-5527, `shouldExit`), so an enemy 30 units out starts no chase.
    let enemy = g.create_object(&dummy(), Coord3D::new(100.3, 100.0, 0.0), 0.0, 1);
    {
        let o = g.objects.get_mut(&guard).unwrap();
        o.ai.machine.current = AiStateType::Guard;
        o.ai.machine.guard_sub = AiStateType::GuardInner;
        o.ai.machine.guard_attack_active = true;
        o.ai.machine.guard.nemesis = enemy;
        o.ai.machine.guard.position_to_guard = centre;
        o.combat.attack = None;
    }
    g.ai_machine_update(guard);
    let o = g.find_object(guard).unwrap();
    assert_eq!(o.ai.machine.guard_sub, AiStateType::GuardOuter, "the inner state ended into the outer chase");
    let attack = o.combat.attack.as_ref().expect("the chase starts");
    assert!((attack.exit.radius_sqr - 0.36).abs() < 1e-4, "radius_sqr is the outer range squared: {}", attack.exit.radius_sqr);
}

/// `AIGuardInnerState::update` (`AIGuard.cpp:446`): an inner state whose nemesis is gone is SUCCESS, which goes to
/// `AI_GUARD_OUTER`, then GET_CRATE and RETURN (`AIGuard.cpp:183`, `:192`, `:193`). The port stayed in
/// `GuardInner` and never moved again.
#[test]
fn an_inner_guard_that_lost_its_nemesis_goes_on_to_return() {
    // Revert check: return `Success` without `ai_guard_inner_done` in the `!nemesis_ok` branch and the sub-state
    // stays `GuardInner` (the assertion fails).
    let mut g = world();
    let guard = g.create_object(&armed("Gun", 200.0, 60.0), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let enemy = g.create_object(&dummy(), Coord3D::new(130.0, 100.0, 0.0), 0.0, 1);
    let centre = Coord3D::new(100.0, 100.0, 0.0);
    {
        let o = g.objects.get_mut(&guard).unwrap();
        o.ai.machine.current = AiStateType::Guard;
        o.ai.machine.guard_sub = AiStateType::GuardInner;
        o.ai.machine.guard_attack_active = true;
        o.ai.machine.in_guard_attack = true;
        o.ai.machine.guard.nemesis = ObjectId::INVALID;
        o.ai.machine.guard.position_to_guard = centre;
        let exit = AttackExitConditions {
            exit_if_no_unit_found: true,
            exit_if_expired_duration: false,
            exit_if_outside_radius: true,
            center: centre,
            radius_sqr: 200.0 * 200.0,
            give_up_frame: 0,
        };
        o.combat.attack = Some(AttackOrder::new(AttackTarget::Object(enemy)).with_exit(exit, false));
    }
    g.ai_machine_update(guard);
    assert_eq!(guard_sub(&g, guard), AiStateType::GuardReturn, "the inner state's SUCCESS reaches the return");
    assert!(g.find_object(guard).unwrap().combat.attack.is_none(), "the lost attack is ended");
}
