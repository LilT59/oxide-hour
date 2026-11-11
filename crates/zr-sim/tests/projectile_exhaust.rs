//! Projectile exhaust (`ProjectileExhaust` / `VeterancyProjectileExhaust`) reaches the projectile
//! snapshot the client reads. Synthetic data only.
//!
//! - `Weapon::fireWeaponTemplate` picks `m_projectileExhausts[sourceObj->getVeterancyLevel()]`.
//! - Only `MissileAIUpdate` stores it; `DumbProjectileBehavior` takes the parameter and drops it.
//! - `MissileAIUpdate::doIgnitionState` attaches the system; `doAttackState` tosses it when out of
//!   fuel without `DetonateOnNoFuel`. The snapshot reports `exhaust_lit` for both.

use std::sync::Arc;
use zr_sim::damage::DamageType;
use zr_sim::experience::VeterancyLevel;
use zr_sim::projectile::{DumbProjectileParams, MissileParams, ProjectileBehavior, ProjectileTemplate};
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::weapon::{CombatTemplate, WeaponSetTemplate, WeaponTemplate};
use zr_sim::{Command, Coord3D, GameLogic, ObjectTemplate, ProjectileSnapshot, TerrainLogic};

fn weapon_with(proj: ProjectileTemplate, speed: f32) -> WeaponTemplate {
    let mut w = WeaponTemplate::new("W");
    w.primary_damage = 50.0;
    w.primary_damage_radius = 5.0;
    w.secondary_damage_radius = 5.0;
    w.attack_range = 400.0;
    w.min_delay_between_shots = 1000;
    w.max_delay_between_shots = 1000;
    w.damage_type = DamageType::Explosion;
    w.weapon_speed = speed;
    w.has_projectile = true;
    w.projectile = Some(Arc::new(proj));
    w
}

fn tank(name: &str, w: Option<WeaponTemplate>) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
    t.geometry.major_radius = 8.0;
    t.max_health = 200.0;
    t.initial_health = 200.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    t.combat = CombatTemplate {
        weapon_set: w.map(|w| {
            let mut s = WeaponSetTemplate::default();
            s.weapons[0] = Some(Arc::new(w));
            s
        }),
        ..CombatTemplate::default()
    };
    t
}

fn missile(params: MissileParams) -> ProjectileTemplate {
    ProjectileTemplate { behavior: ProjectileBehavior::Missile(params), bounding_radius: 3.0, small_missile: false, neutron_warhead: None, neutron_blast: None, garrison_hit_kill: None }
}

fn dumb() -> ProjectileTemplate {
    let dp = DumbProjectileParams { max_lifespan: 200, ..DumbProjectileParams::default() };
    ProjectileTemplate { behavior: ProjectileBehavior::Dumb(dp), bounding_radius: 3.0, small_missile: false, neutron_warhead: None, neutron_blast: None, garrison_hit_kill: None }
}

fn exhaust_names(names: [&str; 3]) -> [Option<String>; 4] {
    [Some(names[0].to_owned()), Some(names[1].to_owned()), Some(names[2].to_owned()), None]
}

/// Attacker (player 0) at x=100 with `weapon` (already set up), a victim (player 1) at x=400.
fn duel(weapon: WeaponTemplate) -> (GameLogic, zr_sim::ObjectId) {
    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
    let a = g.create_object(&Arc::new(tank("A", Some(weapon))), Coord3D::new(100.0, 300.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(tank("B", None)), Coord3D::new(400.0, 300.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(0, Command::Attack { ids: vec![a], target: b });
    (g, a)
}

/// Runs until the first projectile is in the air and returns its snapshot.
fn first_projectile(g: &mut GameLogic) -> ProjectileSnapshot {
    for _ in 0..200 {
        if let Some(p) = g.projectiles().into_iter().next() {
            return p;
        }
        g.run_frames(1);
    }
    panic!("the weapon never fired");
}

#[test]
fn missile_carries_the_exhaust_for_the_firers_veterancy_level() {
    let p = MissileParams { initial_velocity: 12.0, lock_distance: 25.0, ..MissileParams::default() };
    let names = ["ExhaustRegular", "ExhaustVeteran", "ExhaustElite"];
    for (level, want) in [
        (VeterancyLevel::Regular, Some("ExhaustRegular")),
        (VeterancyLevel::Veteran, Some("ExhaustVeteran")),
        (VeterancyLevel::Elite, Some("ExhaustElite")),
        (VeterancyLevel::Heroic, None),
    ] {
        let mut w = weapon_with(missile(p), 12.0);
        w.projectile_exhaust = exhaust_names(names);
        let (mut g, a) = duel(w);
        g.set_veterancy_level(a, level);
        let snap = first_projectile(&mut g);
        assert_eq!(snap.exhaust.as_deref(), want, "{level:?} firer");
    }
}

#[test]
fn exhaust_is_lit_at_ignition_and_tossed_out_of_fuel() {
    // Ignition after 3 frames; the fuel runs out before the missile reaches the victim (the same
    // coasting setup as `projectile_leak`), so the missile is still in the air when the exhaust is tossed.
    let p = MissileParams { initial_velocity: 10.0, ignition_delay: 3, fuel_lifetime: 12, lock_distance: 25.0, detonate_on_no_fuel: false, ..MissileParams::default() };
    let mut w = weapon_with(missile(p), 2.0);
    w.projectile_exhaust = exhaust_names(["ExhaustRegular", "", ""]);
    let (mut g, _) = duel(w);
    let first = first_projectile(&mut g);
    assert!(!first.exhaust_lit, "the launch is not ignited yet");
    let mut lit_frames = 0;
    let mut tossed = false;
    for _ in 0..200 {
        let Some(snap) = g.projectiles().into_iter().next() else { break };
        if snap.exhaust_lit {
            assert!(!tossed, "a tossed exhaust does not relight");
            lit_frames += 1;
        } else if lit_frames > 0 {
            tossed = true;
        }
        g.run_frames(1);
    }
    assert!(lit_frames > 0, "the ignited missile shows its exhaust");
    assert!(tossed, "out of fuel without DetonateOnNoFuel the exhaust is tossed while the missile coasts");
}

#[test]
fn dumb_projectile_never_gets_an_exhaust() {
    // `DumbProjectileBehavior::projectileFireAtObjectOrPosition` ignores `exhaustSysOverride`.
    let mut w = weapon_with(dumb(), 4.0);
    w.attack_range = 5000.0;
    w.projectile_exhaust = exhaust_names(["ExhaustRegular", "ExhaustVeteran", "ExhaustElite"]);
    let (mut g, _) = duel(w);
    let snap = first_projectile(&mut g);
    assert_eq!(snap.exhaust, None);
    assert!(!snap.exhaust_lit);
}

#[test]
fn snapshot_heading_is_the_missile_orientation_including_pitch() {
    // A missile at a raised victim is tilted up toward it (`projectileFireAtObjectOrPosition` adds
    // 2*zFactor to the barrel's Z). The snapshot carries that unit orientation so the attached exhaust
    // follows the full transform, pitch included.
    let p = MissileParams { initial_velocity: 10.0, lock_distance: 25.0, ..MissileParams::default() };
    let mut w = weapon_with(missile(p), 12.0);
    w.projectile_exhaust = exhaust_names(["ExhaustRegular", "", ""]);
    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
    let a = g.create_object(&Arc::new(tank("A", Some(w))), Coord3D::new(100.0, 300.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(tank("B", None)), Coord3D::new(400.0, 300.0, 0.0), 0.0, 1);
    g.set_object_position(b, Coord3D::new(400.0, 300.0, 200.0));
    g.set_enemies(0, 1);
    g.submit(0, Command::Attack { ids: vec![a], target: b });
    let snap = first_projectile(&mut g);
    let h = snap.heading;
    let len = (h.x * h.x + h.y * h.y + h.z * h.z).sqrt();
    assert!((len - 1.0).abs() < 1e-3, "unit heading, got {h:?}");
    assert!(h.x > 0.0 && h.z > 0.3, "the missile points up toward the raised victim: {h:?}");
}
