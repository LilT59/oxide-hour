//! Lane 126: a missile's AI move goal (`AIMoveToState::update`, AIStates.cpp:2085-2125) is the victim's position
//! lifted by half its `MaxHeightAbovePosition` (and again by the height difference while the missile is below), led by
//! `0.5 * distance * goalSpeed / mySpeed` for a moving, non-immobile goal. The KILL arrival gate and the drive both use
//! that goal; the missile converges on the victim's geometry centre, not its origin. Synthetic data only.

use std::sync::Arc;
use zr_sim::damage::DamageType;
use zr_sim::projectile::{MissileParams, ProjectileBehavior, ProjectileTemplate};
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::weapon::{CombatTemplate, LaunchBarrel, WeaponSetTemplate, WeaponTemplate};
use zr_sim::{Command, Coord3D, GameLogic, ObjectId, ObjectTemplate, TerrainLogic};

fn flat() -> TerrainLogic {
    TerrainLogic::flat(300, 300, 0)
}

fn weapon_with(proj: ProjectileTemplate, speed: f32) -> WeaponTemplate {
    let mut w = WeaponTemplate::new("W");
    w.primary_damage = 50.0;
    w.primary_damage_radius = 5.0;
    w.secondary_damage_radius = 5.0;
    w.attack_range = 2000.0;
    w.min_delay_between_shots = 1000;
    w.max_delay_between_shots = 1000;
    w.damage_type = DamageType::Explosion;
    w.weapon_speed = speed;
    w.has_projectile = true;
    w.projectile = Some(Arc::new(proj));
    w
}

fn missile(params: MissileParams) -> ProjectileTemplate {
    ProjectileTemplate { behavior: ProjectileBehavior::Missile(params), bounding_radius: 3.0, small_missile: false, neutron_warhead: None, neutron_blast: None, garrison_hit_kill: None }
}

fn tank(name: &str, w: Option<WeaponTemplate>, barrel_z: f32) -> ObjectTemplate {
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
    if barrel_z != 0.0 {
        t.combat.launch_bones.slots[0] = vec![LaunchBarrel { pos: [0.0, 0.0, barrel_z], rot: [0.0, 0.0, 0.0, 1.0] }];
    }
    t
}

/// A stationary box target: `height` tall, a 20-unit footprint.
fn tower(height: f32) -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Tower");
    t.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE | kind_of::SELECTABLE;
    t.geometry.shape = zr_sim::template::GeometryShape::Box;
    t.geometry.major_radius = 20.0;
    t.geometry.minor_radius = 20.0;
    t.geometry.height = height;
    t.max_health = 2000.0;
    t.initial_health = 2000.0;
    t
}

fn duel(params: MissileParams, victim: ObjectTemplate, victim_x: f32, barrel_z: f32) -> (GameLogic, ObjectId, ObjectId) {
    let mut g = GameLogic::new(flat(), 1);
    let mut ta = tank("A", Some(weapon_with(missile(params), 6.0)), barrel_z);
    ta.physics = Some(zr_sim::physics::PhysicsTemplate::default());
    let a = g.create_object(&Arc::new(ta), Coord3D::new(100.0, 300.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(victim), Coord3D::new(victim_x, 300.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(0, Command::Attack { ids: vec![a], target: b });
    (g, a, b)
}

/// `AIMoveToState::update` lifts the goal to `victim.z + 2 * halfHeight` while the missile is below the victim's
/// centre, so the missile flies at the target's centre height instead of diving at its origin. Reverting the lift
/// (using the raw victim position) keeps the missile near the ground and fails this test.
#[test]
fn a_missile_climbs_to_the_centre_of_a_tall_victim() {
    let p = MissileParams { initial_velocity: 6.0, lock_distance: 25.0, ..MissileParams::default() };
    // The launcher's barrel sits 40 up, so the missile starts below the tower's centre (50) but well above the ground.
    let (mut g, _, b) = duel(p, tower(100.0), 700.0, 40.0);
    let mut z_near = f32::NAN;
    for _ in 0..400 {
        g.run_frames(1);
        if let Some(s) = g.projectiles().first() {
            let d = ((s.pos.x - 700.0).powi(2) + (s.pos.y - 300.0).powi(2)).sqrt();
            if d <= 120.0 {
                z_near = s.pos.z;
            }
        }
        if g.projectiles().is_empty() && !z_near.is_nan() {
            break;
        }
    }
    assert!(g.find_object(b).is_none_or(|o| o.health() < 2000.0), "the missile reaches the tower");
    assert!(z_near > 30.0, "inside 120 units the missile flies at the tower's centre height, not its feet (z {z_near:.1})");
}

/// Against a low target (a sphere of radius 4) the lift is only that radius, so the missile still comes down onto it.
#[test]
fn a_low_victim_is_still_hit_near_its_own_height() {
    let p = MissileParams { initial_velocity: 6.0, lock_distance: 25.0, ..MissileParams::default() };
    let mut low = ObjectTemplate::new("Low");
    low.kind_of = kind_of::VEHICLE | kind_of::IMMOBILE | kind_of::SELECTABLE;
    low.geometry.major_radius = 4.0;
    low.max_health = 200.0;
    low.initial_health = 200.0;
    let (mut g, _, b) = duel(p, low, 700.0, 0.0);
    g.run_frames(400);
    assert!(g.find_object(b).is_none_or(|o| o.health() < 200.0), "the low target is still hit");
}
