//! Lane 126: a projectile's detonation is positional. `DumbProjectileBehavior::detonate` calls
//! `handleProjectileDetonation(tmpl, obj, obj->getPosition())` (DumbProjectileBehavior.cpp:531-539), so the blast is
//! centred on the *shell's* position, not on the object it hit, and `dealDamageInternal` gets no victim object
//! (Weapon.cpp:1361-1365, 1386-1398). Synthetic data only.

use std::sync::Arc;

use zr_sim::damage::DamageType;
use zr_sim::projectile::{DumbProjectileParams, ProjectileBehavior, ProjectileTemplate};
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::weapon::{CombatTemplate, WeaponSetTemplate, WeaponTemplate};
use zr_sim::{Command, Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

fn building() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Building");
    t.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE | kind_of::SELECTABLE;
    t.geometry.major_radius = 30.0;
    t.geometry.height = 40.0;
    t.max_health = 1000.0;
    t.initial_health = 1000.0;
    t
}

fn infantry() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Bystander");
    t.kind_of = kind_of::INFANTRY | kind_of::SELECTABLE;
    t.geometry.major_radius = 4.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t
}

/// A shell: 50 primary in a 5 radius, 15 secondary out to 25.
fn shell_weapon() -> WeaponTemplate {
    let mut w = WeaponTemplate::new("Shell");
    w.primary_damage = 50.0;
    w.primary_damage_radius = 5.0;
    w.secondary_damage = 15.0;
    w.secondary_damage_radius = 25.0;
    w.attack_range = 2000.0;
    w.min_delay_between_shots = 1000;
    w.max_delay_between_shots = 1000;
    w.damage_type = DamageType::Explosion;
    w.weapon_speed = 10.0;
    w.has_projectile = true;
    w.projectile = Some(Arc::new(ProjectileTemplate {
        behavior: ProjectileBehavior::Dumb(DumbProjectileParams { max_lifespan: 300, ..DumbProjectileParams::default() }),
        bounding_radius: 3.0,
        small_missile: false,
        neutron_warhead: None,
        neutron_blast: None,
        garrison_hit_kill: None,
    }));
    w.projectile_detonation_fx = Some("FX_ShellDetonation".into());
    w
}

fn gunner() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Gunner");
    t.kind_of = kind_of::INFANTRY | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
    t.geometry.major_radius = 5.0;
    t.max_health = 200.0;
    t.initial_health = 200.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Legs, 20.0, 60.0, 120.0, 180.0));
    let mut set = WeaponSetTemplate::default();
    set.weapons[0] = Some(Arc::new(shell_weapon()));
    t.combat = CombatTemplate { weapon_set: Some(set), ..CombatTemplate::default() };
    t
}

/// A shell fired at a building collides at the building's *edge* (its bounding circle), so the blast is centred
/// there. A bystander 20 units off the impact point is inside the 25-radius secondary damage after the fix; while the
/// blast was centred on the building's position (30 + 20 = 36 away) it was out of range. Reverting the positional
/// detonation (passing the hit object as the primary victim) makes this fail.
#[test]
fn a_shell_detonates_where_it_stopped_not_at_the_victims_centre() {
    let mut g = GameLogic::new(TerrainLogic::flat(300, 300, 0), 1);
    let bld = g.create_object(&Arc::new(building()), Coord3D::new(500.0, 300.0, 0.0), 0.0, 0);
    // 20 units north of the building's west edge (x = 470), clear of the shell's flight line (y = 300).
    let bystander = g.create_object(&Arc::new(infantry()), Coord3D::new(470.0, 320.0, 0.0), 0.0, 0);
    let a = g.create_object(&Arc::new(gunner()), Coord3D::new(100.0, 300.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(1, Command::Attack { ids: vec![a], target: bld });
    let mut fx_target = None;
    let mut seen = false;
    for _ in 0..300 {
        g.run_frames(1);
        if !g.projectiles().is_empty() {
            seen = true;
        } else if seen {
            fx_target = g.fx_events().iter().find(|e| e.name == "FX_ShellDetonation").and_then(|e| e.target);
            break;
        }
    }
    assert!(seen, "the shell is fired");
    let hp = g.find_object(bystander).unwrap().health();
    assert!(hp < 100.0, "the bystander is inside the blast at the impact point (hp {hp})");
    assert!((hp - 85.0).abs() < 1e-3, "secondary damage (15), not primary (50): hp {hp}");
    assert_eq!(g.find_object(bld).unwrap().health(), 950.0, "the building takes the primary 50");
    // The detonation FX's secondary point is the blast point itself (`targetPos = *victimPos`, Weapon.cpp:918-926).
    let target = fx_target.expect("the detonation FX is recorded");
    assert!(target.x < 480.0 && target.x > 450.0, "the FX target is the impact point, not the building's centre: {target:?}");
    assert_eq!(target.y, 300.0);
}

/// Against a tall target the flight path ends at its geometry centre (`DumbProjectileBehavior.cpp:355-360`: "aim at the
/// center, not the ground part"), so a shell fired from the ground rises to the target's centre height. Reverting to the
/// victim's origin keeps the flight along the ground and fails this.
#[test]
fn a_shell_aims_at_the_victims_geometry_centre() {
    let mut g = GameLogic::new(TerrainLogic::flat(300, 300, 0), 1);
    // A tall thin tower: a cylinder 60 high, so `getCenterPosition` is z = 30.
    let mut tower = ObjectTemplate::new("Tower");
    tower.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE | kind_of::SELECTABLE;
    tower.geometry.shape = zr_sim::template::GeometryShape::Cylinder;
    tower.geometry.major_radius = 5.0;
    tower.geometry.height = 60.0;
    tower.max_health = 2000.0;
    tower.initial_health = 2000.0;
    let bld = g.create_object(&Arc::new(tower), Coord3D::new(300.0, 300.0, 0.0), 0.0, 0);
    let a = g.create_object(&Arc::new(gunner()), Coord3D::new(100.0, 300.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(1, Command::Attack { ids: vec![a], target: bld });
    let mut blast_z = None;
    let mut seen = false;
    for _ in 0..300 {
        g.run_frames(1);
        if !g.projectiles().is_empty() {
            seen = true;
        } else if seen {
            blast_z = g.fx_events().iter().find(|e| e.name == "FX_ShellDetonation").map(|e| e.pos.z);
            break;
        }
    }
    assert!(seen, "the shell is fired");
    let z = blast_z.expect("the detonation FX is recorded");
    assert!(z > 15.0, "the shell detonates at the tower's centre height, not at its feet (z {z:.1})");
}
