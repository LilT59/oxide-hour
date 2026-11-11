//! Shroud / fog-of-war behaviour on synthetic data: reveal, fog memory and fog-aware targeting.

use std::sync::Arc;
use zr_sim::shroud::{CellShroudStatus, ObjectShroudStatus};
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::weapon::{CombatTemplate, WeaponSetTemplate, WeaponTemplate};
use zr_sim::{Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

fn flat() -> TerrainLogic {
    TerrainLogic::flat(100, 100, 0)
}

fn cannon() -> WeaponTemplate {
    let mut w = WeaponTemplate::new("Cannon");
    w.primary_damage = 40.0;
    w.attack_range = 150.0;
    w.min_delay_between_shots = 30;
    w.max_delay_between_shots = 30;
    w
}

/// A vehicle with `vision_range` (idle scan range) and an explicit `shroud_clearing_range`
/// (the reveal circle; `-1` means "use vision_range").
fn unit(name: &str, vision_range: f32, shroud_clearing_range: f32) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
    t.geometry.major_radius = 8.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.vision_range = vision_range;
    t.shroud_clearing_range = shroud_clearing_range;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    t.combat = CombatTemplate { weapon_set: Some(WeaponSetTemplate { weapons: [Some(Arc::new(cannon())), None, None] }), ..CombatTemplate::default() };
    t
}

fn spotter(name: &str) -> ObjectTemplate {
    let mut t = unit(name, 200.0, 200.0);
    t.combat.weapon_set = None;
    t
}

#[test]
fn look_reveals_only_the_owner_and_allies() {
    let mut g = GameLogic::new(flat(), 1);
    let a = g.create_object(&Arc::new(unit("A", 200.0, 200.0)), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    g.run_frames(1);
    let _ = a;
    let pos = Coord3D::new(200.0, 200.0, 0.0);
    assert_eq!(g.cell_shroud_status(0, &pos), CellShroudStatus::Clear);
    assert_eq!(g.cell_shroud_status(1, &pos), CellShroudStatus::Shrouded);
    // Far from any looker is shrouded for the owner too.
    assert_eq!(g.cell_shroud_status(0, &Coord3D::new(900.0, 900.0, 0.0)), CellShroudStatus::Shrouded);
}

#[test]
fn object_status_is_clear_under_own_look_and_shrouded_otherwise() {
    let mut g = GameLogic::new(flat(), 1);
    let a = g.create_object(&Arc::new(unit("A", 200.0, 200.0)), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let enemy = g.create_object(&Arc::new(unit("E", 200.0, 200.0)), Coord3D::new(250.0, 200.0, 0.0), 0.0, 1);
    g.run_frames(1);
    assert_eq!(g.object_shroud_status(0, a), ObjectShroudStatus::Clear);
    assert_eq!(g.object_shroud_status(0, enemy), ObjectShroudStatus::Clear, "the enemy is inside A's reveal circle");
    assert_eq!(g.object_shroud_status(2, enemy), ObjectShroudStatus::Shrouded, "a third player sees nothing");
}

#[test]
fn idle_scan_ignores_fogged_enemies_until_spotted() {
    let mut g = GameLogic::new(flat(), 1);
    // The attacker can scan 200 units but only reveals 10: an enemy at 100 is in scan range but
    // fogged, so `PartitionFilterFreeOfFog` rejects it.
    let mut at = unit("A", 200.0, 10.0);
    at.combat.auto_acquire_enemies = true;
    at.combat.mood_attack_check_rate = 5;
    let a = g.create_object(&Arc::new(at), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let enemy = g.create_object(&Arc::new(unit("E", 200.0, 200.0)), Coord3D::new(300.0, 200.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.run_frames(60);
    assert!(g.find_object(a).unwrap().combat.attack.is_none(), "a fogged enemy must not be auto-acquired");
    assert_eq!(g.find_object(enemy).unwrap().health(), 100.0);

    // A friendly spotter reveals the area for the attacker's player.
    g.create_object(&Arc::new(spotter("Spotter")), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    g.run_frames(60);
    assert_eq!(g.find_object(a).unwrap().combat.attack.map(|o| o.target), Some(zr_sim::AttackTarget::Object(enemy)));
    assert!(g.find_object(enemy).unwrap().health() < 100.0);
}

#[test]
fn removed_look_becomes_fog_after_the_persist_window() {
    let mut g = GameLogic::new(flat(), 1);
    let a = g.create_object(&Arc::new(unit("A", 200.0, 200.0)), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let pos = Coord3D::new(200.0, 200.0, 0.0);
    g.run_frames(1);
    assert_eq!(g.cell_shroud_status(0, &pos), CellShroudStatus::Clear);
    // Destroying the only looker queues the undo; the cell stays clear for the persist window.
    g.destroy_object(a);
    g.run_frames(1);
    assert_eq!(g.cell_shroud_status(0, &pos), CellShroudStatus::Clear);
    g.run_frames(zr_sim::shroud::UNLOOK_PERSIST_FRAMES + 2);
    assert_eq!(g.cell_shroud_status(0, &pos), CellShroudStatus::Fogged);
}
