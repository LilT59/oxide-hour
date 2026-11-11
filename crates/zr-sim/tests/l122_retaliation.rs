//! Retaliation mode: `MSG_ENABLE_RETALIATION_MODE` (`GameLogicDispatch.cpp:1082`, default off
//! `Player.cpp:498`) and `ActiveBody::attemptDamage`'s retaliation assist (`ActiveBody.cpp:688-792`).
//! Synthetic data only.

use std::sync::Arc;

use zr_sim::ai_states::AiStateType;
use zr_sim::body::{DamageInfo, DamageType};
use zr_sim::damage::{DeathType, Relationship};
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::weapon::{CombatTemplate, WeaponSetTemplate, WeaponTemplate};
use zr_sim::{Command, Coord3D, GameLogic, ObjectId, ObjectTemplate, TerrainLogic};

fn cannon() -> WeaponTemplate {
    let mut w = WeaponTemplate::new("Cannon");
    w.primary_damage = 20.0;
    w.damage_type = DamageType::ArmorPiercing;
    w.attack_range = 150.0;
    w.min_delay_between_shots = 30;
    w.max_delay_between_shots = 30;
    w
}

fn tank_template(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
    t.geometry.major_radius = 8.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.vision_range = 200.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    let mut weapons = WeaponSetTemplate::default();
    weapons.weapons[0] = Some(Arc::new(cannon()));
    t.combat = CombatTemplate { weapon_set: Some(weapons), ..CombatTemplate::default() };
    t
}

fn world() -> GameLogic {
    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
    g.add_player(0, 0); // human slot
    g.add_player(1, 0); // enemy
    g.add_player(2, 0); // another slot, for the per-player flag check
    g.set_enemies(0, 1);
    g
}

fn state(g: &GameLogic, id: ObjectId) -> AiStateType {
    g.find_object(id).expect("object").ai.machine.current
}

/// A hit on a human player's unit orders the idle allies within `RetaliationFriendsRadius` (+ the hit
/// object's bounding circle) to get revenge (`ActiveBody.cpp:694-724`). `aiGuardRetaliate` enters
/// `AI_GUARD_RETALIATE` (`privateGuardRetaliate`, AIUpdate.cpp:3480), not the ordinary `AI_GUARD`.
#[test]
fn retaliation_mode_orders_the_nearby_idle_allies_to_guard() {
    let mut g = world();
    let t = Arc::new(tank_template("Tank"));
    let victim = g.create_object(&t, Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let ally = g.create_object(&t, Coord3D::new(260.0, 200.0, 0.0), 0.0, 0);
    let damager = g.create_object(&t, Coord3D::new(300.0, 200.0, 0.0), 0.0, 1);
    g.submit(0, Command::EnableRetaliationMode { enabled: true });
    g.update();
    assert!(g.player(0).unwrap().logical_retaliation_mode_enabled, "the message sets the issuing player's flag");

    g.damage_object_with_death(victim, DamageInfo { source: damager, amount: 10.0, kind: DamageType::ArmorPiercing }, DeathType::Normal);
    assert_eq!(state(&g, ally), AiStateType::GuardRetaliate, "the ally within the friends radius is ordered to guard-retaliate");
}

/// With retaliation mode off (the default, `Player.cpp:498`) the hit orders nobody.
#[test]
fn retaliation_mode_off_orders_nobody() {
    let mut g = world();
    let t = Arc::new(tank_template("Tank"));
    let victim = g.create_object(&t, Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let ally = g.create_object(&t, Coord3D::new(260.0, 200.0, 0.0), 0.0, 0);
    let damager = g.create_object(&t, Coord3D::new(300.0, 200.0, 0.0), 0.0, 1);
    g.update();
    assert!(!g.player(0).unwrap().logical_retaliation_mode_enabled, "off at the start of a game");
    g.damage_object_with_death(victim, DamageInfo { source: damager, amount: 10.0, kind: DamageType::ArmorPiercing }, DeathType::Normal);
    assert_eq!(state(&g, ally), AiStateType::Idle);
}

/// A computer player never retaliates, even with its flag on: `getPlayerType() == PLAYER_HUMAN`
/// (`ActiveBody.cpp:748-751`). The sim's computer players are its skirmish-AI players.
#[test]
fn a_computer_players_units_do_not_retaliate() {
    let mut g = world();
    g.set_enemies(1, 2);
    g.add_ai_player(1, zr_sim::skirmish_ai::SkirmishAiConfig::default());
    let t = Arc::new(tank_template("Tank"));
    let victim = g.create_object(&t, Coord3D::new(200.0, 200.0, 0.0), 0.0, 1);
    let ally = g.create_object(&t, Coord3D::new(260.0, 200.0, 0.0), 0.0, 1);
    let damager = g.create_object(&t, Coord3D::new(300.0, 200.0, 0.0), 0.0, 2);
    g.submit(1, Command::EnableRetaliationMode { enabled: true });
    g.update();
    assert!(g.player(1).unwrap().logical_retaliation_mode_enabled);
    g.damage_object_with_death(victim, DamageInfo { source: damager, amount: 10.0, kind: DamageType::ArmorPiercing }, DeathType::Normal);
    assert_eq!(state(&g, ally), AiStateType::Idle, "only human players retaliate");
}

/// `shouldRetaliateAgainstAggressor` (`ActiveBody.cpp:730-757`): no assist against an ally or a damager
/// beyond `MaxRetaliationDistance` (the shipped `AIData.ini` 200, measured bounding sphere to bounding
/// sphere). `shouldRetaliate` (`:761-792`): an immobile or non-idle ally is skipped.
#[test]
fn retaliation_ignores_allies_distant_attackers_and_unfit_helpers() {
    let mut g = world();
    let t = Arc::new(tank_template("Tank"));
    g.set_allied(0, 2);
    let victim = g.create_object(&t, Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let ally = g.create_object(&t, Coord3D::new(260.0, 200.0, 0.0), 0.0, 0);
    g.submit(0, Command::EnableRetaliationMode { enabled: true });
    g.update();

    // Beyond MaxRetaliationDistance: 200 surface distance + the two 8-unit radii.
    let far = g.create_object(&t, Coord3D::new(500.0, 200.0, 0.0), 0.0, 1);
    g.damage_object_with_death(victim, DamageInfo { source: far, amount: 10.0, kind: DamageType::ArmorPiercing }, DeathType::Normal);
    assert_eq!(state(&g, ally), AiStateType::Idle, "too far away");

    // An allied damager: no revenge.
    let friend = g.create_object(&t, Coord3D::new(300.0, 200.0, 0.0), 0.0, 2);
    g.damage_object_with_death(victim, DamageInfo { source: friend, amount: 10.0, kind: DamageType::ArmorPiercing }, DeathType::Normal);
    assert_eq!(state(&g, ally), AiStateType::Idle, "an ally's hit is no aggressor");

    // Sanity: an enemy on the ground at the same place does trigger it.
    let enemy = g.create_object(&t, Coord3D::new(300.0, 200.0, 0.0), 0.0, 1);
    g.damage_object_with_death(victim, DamageInfo { source: enemy, amount: 10.0, kind: DamageType::ArmorPiercing }, DeathType::Normal);
    assert_eq!(state(&g, ally), AiStateType::GuardRetaliate);

    // An immobile structure next to the victim does not assist.
    let mut turret_t = ObjectTemplate::new("Turret");
    turret_t.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE | kind_of::CAN_ATTACK;
    turret_t.geometry.major_radius = 8.0;
    turret_t.max_health = 100.0;
    turret_t.initial_health = 100.0;
    let turret = g.create_object(&Arc::new(turret_t), Coord3D::new(200.0, 260.0, 0.0), 0.0, 0);
    // A busy ally (moving, so not `isIdle`) does not assist either.
    let mover = g.create_object(&t, Coord3D::new(200.0, 140.0, 0.0), 0.0, 0);
    g.submit(0, Command::MoveTo { ids: vec![mover], pos: Coord3D::new(300.0, 140.0, 0.0) });
    g.update();
    assert_ne!(state(&g, mover), AiStateType::Idle, "the mover is under orders");
    g.damage_object_with_death(victim, DamageInfo { source: enemy, amount: 10.0, kind: DamageType::ArmorPiercing }, DeathType::Normal);
    assert_eq!(state(&g, turret), AiStateType::Idle, "an immobile structure does not assist");
    assert_ne!(state(&g, mover), AiStateType::GuardRetaliate, "a unit under orders is not re-tasked");
}

/// The flag is per player: the message's player sets only its own (`GameLogicDispatch.cpp:1103-1105`).
#[test]
fn the_retaliation_flag_is_per_player() {
    let mut g = world();
    g.submit(0, Command::EnableRetaliationMode { enabled: true });
    g.update();
    assert!(g.player(0).unwrap().logical_retaliation_mode_enabled);
    assert!(!g.player(1).unwrap().logical_retaliation_mode_enabled, "another player is untouched");
    assert!(!g.player(2).unwrap().logical_retaliation_mode_enabled);
    g.submit(0, Command::EnableRetaliationMode { enabled: false });
    g.update();
    assert!(!g.player(0).unwrap().logical_retaliation_mode_enabled, "the message can turn it off again");
}

/// `Relationship` helper sanity used by the tests above.
#[test]
fn the_world_starts_with_enemies() {
    let g = world();
    assert_eq!(g.relationship(0, 1), Relationship::Enemies);
    assert_eq!(g.relationship(0, 0), Relationship::Allies);
}
