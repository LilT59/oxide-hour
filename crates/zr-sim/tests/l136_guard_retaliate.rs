//! `AIGuardRetaliateMachine` (`AIGuardRetaliate.cpp` / `AIGuardRetaliate.h`): `ActiveBody`'s
//! retaliation assist (`ActiveBody.cpp:688-725`) orders a human player's idle ally through
//! `aiGuardRetaliate` (`AIUpdate.cpp:3470-3486`), which enters `AI_GUARD_RETALIATE`
//! (`AIUpdate.cpp:3480`), not the ordinary `AI_GUARD`. Synthetic data only.

use std::sync::Arc;

use zr_sim::ai_guard_retaliate::RetaliateSub;
use zr_sim::ai_states::{AiStateType, CommandSource};
use zr_sim::body::{DamageInfo, DamageType};
use zr_sim::combat::AttackTarget;
use zr_sim::damage::DeathType;
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::weapon::{CombatTemplate, WeaponSetTemplate, WeaponTemplate};
use zr_sim::{Command, Coord3D, GameLogic, ObjectId, ObjectTemplate, TerrainLogic};

/// An armed vehicle: `vision` sets the guard-scan radius (`GuardInnerModifierHuman` 1.8 for the human
/// player), `range` the weapon's reach.
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

/// A weaponless, very tough vehicle: the aggressor. A reaction (`ai_attack_update_object`) only needs a
/// body and a relationship, and a weaponless one never fires back, so the scenario stays one-sided.
fn target(name: &str) -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new(name);
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
    g.add_player(0, 0); // human slot
    g.add_player(1, 0); // enemy
    g.set_enemies(0, 1);
    g
}

fn state(g: &GameLogic, id: ObjectId) -> AiStateType {
    g.find_object(id).expect("object").ai.machine.current
}

fn sub(g: &GameLogic, id: ObjectId) -> RetaliateSub {
    g.find_object(id).expect("object").ai.machine.retaliate.sub
}

fn attack_target(g: &GameLogic, id: ObjectId) -> Option<ObjectId> {
    match g.find_object(id).and_then(|o| o.combat.attack.as_ref()).map(|a| a.target) {
        Some(AttackTarget::Object(v)) => Some(v),
        _ => None,
    }
}

fn kill(g: &mut GameLogic, victim: ObjectId, source: ObjectId) {
    g.damage_object_with_death(
        victim,
        DamageInfo { source, amount: 1_000_000.0, kind: DamageType::Unresistable },
        DeathType::Normal,
    );
}

/// The retaliation assist enters `AI_GUARD_RETALIATE` (`privateGuardRetaliate`, AIUpdate.cpp:3480), whose
/// default sub-state is `AI_GUARD_RETALIATE_ATTACK_AGGRESSOR` (`AIGuardRetaliate.cpp:183-198`): the ally
/// attacks the damager even though the damager is outside its guard scan radius (the machine's own
/// aggressor attack reaches `outer + inner` ranges, `AIGuardRetaliate.cpp:813-820`), and the main state
/// keeps its name while the attack sub-machine runs (C++ `getCurrentStateID()` is `AI_GUARD_RETALIATE`).
#[test]
fn retaliation_enters_the_guard_retaliate_machine_and_attacks_the_aggressor() {
    let mut g = world();
    let victim = g.create_object(&target("Victim"), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    // A 50-unit vision gives a 90-unit standard guard circle (GuardInnerModifierHuman 1.8) and a
    // 110-unit outer one; the 70-unit weapon lets the ally fire from inside its own guard radius.
    let ally = g.create_object(&armed("Ally", 50.0, 70.0), Coord3D::new(260.0, 200.0, 0.0), 0.0, 0);
    // 150 units from the ally: outside its 70-unit weapon range and its 90-unit guard scan, but inside
    // the aggressor state's (50 * 2.2 + 90) = 200-unit exit radius.
    let damager = g.create_object(&target("Damager"), Coord3D::new(410.0, 200.0, 0.0), 0.0, 1);
    g.submit(0, Command::EnableRetaliationMode { enabled: true });
    g.update();

    g.damage_object_with_death(victim, DamageInfo { source: damager, amount: 10.0, kind: DamageType::ArmorPiercing }, DeathType::Normal);
    let o = g.find_object(ally).unwrap();
    assert_eq!(state(&g, ally), AiStateType::GuardRetaliate, "aiGuardRetaliate enters AI_GUARD_RETALIATE");
    assert_eq!(sub(&g, ally), RetaliateSub::AttackAggressor, "the machine's default state attacks the aggressor");
    assert_eq!(o.ai.machine.retaliate.nemesis, damager, "the goal object is the nemesis");
    assert_eq!(attack_target(&g, ally), Some(damager), "the aggressor attack is ordered");
    // `privateGuardRetaliate` writes `setLastCommandSource(cmdSource)` (AIUpdate.cpp:3479); ActiveBody
    // passes CMD_FROM_AI (ActiveBody.cpp:722).
    assert_eq!(o.ai.machine.last_command_source, CommandSource::Ai);

    // The ally closes in and hits the aggressor; its main state stays AI_GUARD_RETALIATE the whole time.
    let mut fired = false;
    for _ in 0..600 {
        g.update();
        assert_eq!(state(&g, ally), AiStateType::GuardRetaliate, "the sub-machine's attack does not change the main state");
        if g.find_object(damager).is_none_or(|d| d.health() < 1000.0) {
            fired = true;
            break;
        }
    }
    assert!(fired, "the ally attacked the aggressor (state {:?}, sub {:?})", state(&g, ally), sub(&g, ally));
}

/// After the aggressor dies the machine returns to the guarded position
/// (`AIGuardRetaliateAttackAggressorState` success -> `AI_GUARD_RETALIATE_RETURN`,
/// AIGuardRetaliate.cpp:186), and the `AI_GUARD_RETALIATE_IDLE` scan that finds nothing fails with
/// `EXIT_MACHINE_WITH_SUCCESS` (`:194`, `StateMachine.cpp:144-147`), so the whole order succeeds into
/// `AI_IDLE` (`AIStates.cpp:720`). The ordinary `AI_GUARD` machine would keep guarding instead
/// (`AI_GUARD_IDLE`'s failure target is `AI_GUARD_RETURN`, `AIGuard.cpp:191`).
#[test]
fn the_guard_retaliate_machine_returns_and_its_order_ends() {
    let mut g = world();
    let victim = g.create_object(&target("Victim"), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let ally = g.create_object(&armed("Ally", 50.0, 30.0), Coord3D::new(260.0, 200.0, 0.0), 0.0, 0);
    let damager = g.create_object(&target("Damager"), Coord3D::new(410.0, 200.0, 0.0), 0.0, 1);
    g.submit(0, Command::EnableRetaliationMode { enabled: true });
    g.update();
    let post = g.find_object(ally).unwrap().position;
    g.damage_object_with_death(victim, DamageInfo { source: damager, amount: 10.0, kind: DamageType::ArmorPiercing }, DeathType::Normal);

    // Let the attack run for a while: the aggressor is out of range, so the ally has to leave its post.
    g.run_frames(30);
    let moved = g.find_object(ally).unwrap().position.dist_2d(&post);
    assert!(moved > 5.0, "the ally left its post to reach the aggressor (moved {moved:.1})");

    kill(&mut g, damager, ally);
    let mut saw_return = false;
    for _ in 0..900 {
        g.update();
        let o = g.find_object(ally).unwrap();
        if o.ai.machine.retaliate.sub == RetaliateSub::Return {
            saw_return = true;
        }
        if o.ai.machine.current == AiStateType::Idle {
            break;
        }
    }
    assert!(saw_return, "the machine passed through AI_GUARD_RETALIATE_RETURN");
    let o = g.find_object(ally).unwrap();
    assert_eq!(o.ai.machine.current, AiStateType::Idle, "the idle scan found nothing: the order ended");
    // The return's `adjustDestination` may move the goal onto the nearest valid cell (AIPathfind's
    // adjust, AIStates.cpp:1661-1666), so allow one pathfind cell.
    let back = o.position.dist_2d(&post);
    assert!(back < 12.0, "the ally returned to its guard position (off by {back:.1})");
}

/// The `attackAggressors` condition of `AI_GUARD_RETALIATE_RETURN`/`_IDLE`
/// (`hasAttackedMeAndICanReturnFire`, AIGuardRetaliate.cpp:58-107, attached at `:188,194`; retail keeps
/// it, `RETAIL_COMPATIBLE_CRC` is 1 in `GameDefines.h:103`): a hit during the return moves the machine
/// back to `AI_GUARD_RETALIATE_ATTACK_AGGRESSOR`. With the old nemesis gone, the state falls back to the
/// body's last damage source (`:794-803`) and makes it the nemesis.
#[test]
fn a_hit_during_the_return_attacks_the_new_aggressor() {
    let mut g = world();
    let victim = g.create_object(&target("Victim"), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let ally = g.create_object(&armed("Ally", 50.0, 30.0), Coord3D::new(260.0, 200.0, 0.0), 0.0, 0);
    let damager = g.create_object(&target("Damager"), Coord3D::new(410.0, 200.0, 0.0), 0.0, 1);
    let second = g.create_object(&target("Second"), Coord3D::new(320.0, 260.0, 0.0), 0.0, 1);
    g.submit(0, Command::EnableRetaliationMode { enabled: true });
    g.update();
    g.damage_object_with_death(victim, DamageInfo { source: damager, amount: 10.0, kind: DamageType::ArmorPiercing }, DeathType::Normal);

    g.run_frames(20);
    kill(&mut g, damager, ally);
    // The next update ends the attack (the victim is gone) and the one after starts the return.
    let mut returning = false;
    for _ in 0..30 {
        g.update();
        if sub(&g, ally) == RetaliateSub::Return {
            returning = true;
            break;
        }
    }
    assert!(returning, "the machine entered AI_GUARD_RETALIATE_RETURN");

    // Shoot the ally from a second enemy while it walks home.
    let ally_pos = g.find_object(ally).unwrap().position;
    assert!(ally_pos.dist_2d(&Coord3D::new(320.0, 260.0, 0.0)) > 30.0, "the second enemy is out of the guard scan");
    g.damage_object_with_death(ally, DamageInfo { source: second, amount: 1.0, kind: DamageType::SmallArms }, DeathType::Normal);
    let o = g.find_object(ally).unwrap();
    assert_eq!(o.ai.machine.retaliate.nemesis, damager, "the nemesis is still the first aggressor before the condition runs");

    let mut retargeted = false;
    for _ in 0..10 {
        g.update();
        if attack_target(&g, ally) == Some(second) {
            retargeted = true;
            break;
        }
    }
    assert!(retargeted, "the return's attack-aggressor condition attacks the new aggressor (sub {:?})", sub(&g, ally));
    assert_eq!(sub(&g, ally), RetaliateSub::AttackAggressor);
    assert_eq!(g.find_object(ally).unwrap().ai.machine.retaliate.nemesis, second);
}

/// `GuardRetaliateExitConditions::shouldExit`'s second radius test (`AIGuardRetaliate.cpp:158-162`):
/// "Never go beyond standard guard radius." The aggressor state's own exit radius (`range + std`) is
/// wider than the standard guard circle; the ally must stop chasing once it is `std` away from its
/// post even though the victim is still inside the aggressor radius.
#[test]
fn a_retaliating_unit_never_chases_beyond_its_standard_guard_radius() {
    let mut g = world();
    let victim = g.create_object(&target("Victim"), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    // 10-unit weapon: it cannot fire before the 90-unit owner-radius check stops it (std = 50 * 1.8).
    let ally = g.create_object(&armed("Ally", 50.0, 10.0), Coord3D::new(260.0, 200.0, 0.0), 0.0, 0);
    let damager = g.create_object(&target("Damager"), Coord3D::new(410.0, 200.0, 0.0), 0.0, 1);
    g.submit(0, Command::EnableRetaliationMode { enabled: true });
    g.update();
    let post = g.find_object(ally).unwrap().position;
    g.damage_object_with_death(victim, DamageInfo { source: damager, amount: 10.0, kind: DamageType::ArmorPiercing }, DeathType::Normal);
    assert_eq!(sub(&g, ally), RetaliateSub::AttackAggressor);

    let mut farthest = 0.0f32;
    let mut saw_return = false;
    for _ in 0..200 {
        g.update();
        let o = g.find_object(ally).unwrap();
        farthest = farthest.max(o.position.dist_2d(&post));
        if o.ai.machine.retaliate.sub == RetaliateSub::Return {
            saw_return = true;
            break;
        }
    }
    assert!(saw_return, "the owner-radius check ended the aggressor attack (farthest {farthest:.1})");
    // The aggressor's exit ends its approach move, but the return move's first request is served at the end of the frame
    // (AI.cpp:359), and until then the chase path keeps driving: `requestPath` leaves `m_path` in place (AIUpdate.cpp:496-511).
    // So the ally may take one more step of that path past the 90-unit radius (lane 191 R27: the exit no longer stops the
    // unit at once, F1). Allow one frame of motion; the 90-unit exit itself is still the one that ends the chase.
    assert!(farthest <= 93.0, "the ally stayed within one frame of its 90-unit standard guard radius (farthest {farthest:.2})");
}

/// The `AI_GUARD_RETALIATE_RETURN`/`_IDLE` scan (`lookForInnerTarget`, AIGuardRetaliate.cpp:224-305)
/// finds an enemy within `getStdGuardRange` of the guarded position, fails the state (`:627-628`) and
/// enters `AI_GUARD_RETALIATE_INNER` (`:194` success -> inner), which attacks with the
/// `1.5 * stdGuardRange` exit radius (`:406-409`). The main state stays `AI_GUARD_RETALIATE`.
#[test]
fn the_idle_scan_engages_an_enemy_at_the_guard_position() {
    let mut g = world();
    let victim = g.create_object(&target("Victim"), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let ally = g.create_object(&armed("Ally", 50.0, 30.0), Coord3D::new(260.0, 200.0, 0.0), 0.0, 0);
    let damager = g.create_object(&target("Damager"), Coord3D::new(410.0, 200.0, 0.0), 0.0, 1);
    // 70 from the guarded position: inside the 90-unit standard guard circle.
    let lurker = g.create_object(&target("Lurker"), Coord3D::new(265.0, 270.0, 0.0), 0.0, 1);
    g.submit(0, Command::EnableRetaliationMode { enabled: true });
    g.update();
    g.damage_object_with_death(victim, DamageInfo { source: damager, amount: 10.0, kind: DamageType::ArmorPiercing }, DeathType::Normal);
    g.run_frames(20);
    kill(&mut g, damager, ally);

    let mut engaged = false;
    for _ in 0..600 {
        g.update();
        assert_eq!(state(&g, ally), AiStateType::GuardRetaliate, "the guard-retaliate order owns the whole fight");
        if attack_target(&g, ally) == Some(lurker) {
            engaged = true;
            break;
        }
    }
    assert!(engaged, "the inner scan engaged the enemy at the guard position (sub {:?})", sub(&g, ally));
    assert_eq!(sub(&g, ally), RetaliateSub::Inner, "the inner state owns an enemy in the guard circle");
    assert_eq!(g.find_object(ally).unwrap().ai.machine.retaliate.nemesis, lurker);
}
