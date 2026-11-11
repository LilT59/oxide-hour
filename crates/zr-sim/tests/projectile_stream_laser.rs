//! Projectile streams (`ProjectileStreamUpdate`) and laser beams (`Weapon::createLaser`) as the
//! client reads them. Synthetic data only.
//!
//! - A `ProjectileStreamName` weapon adds each projectile to its stream, which is destroyed only once
//!   its projectiles are gone and its owner is dead (`considerDying`).
//! - A `LaserName` weapon leaves a beam from the source to the victim (lifted 10 above the ground
//!   unless the victim is airborne), removed when the laser template's lifetime runs out.

use std::sync::Arc;
use zr_sim::damage::DamageType;
use zr_sim::projectile::{DumbProjectileParams, ProjectileBehavior, ProjectileTemplate};
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::weapon::{CombatTemplate, WeaponSetTemplate, WeaponTemplate};
use zr_sim::{Command, Coord3D, GameLogic, LaserRollKind, LaserSpec, ObjectTemplate, TerrainLogic};

fn weapon(speed: f32) -> WeaponTemplate {
    let mut w = WeaponTemplate::new("W");
    w.primary_damage = 5.0;
    w.primary_damage_radius = 5.0;
    w.secondary_damage_radius = 5.0;
    w.attack_range = 400.0;
    // Delays are logic frames: a shot every second (30 frames).
    w.min_delay_between_shots = 30;
    w.max_delay_between_shots = 30;
    w.damage_type = DamageType::Explosion;
    w.weapon_speed = speed;
    w
}

fn dumb_weapon(speed: f32, stream: &str) -> WeaponTemplate {
    let mut w = weapon(speed);
    w.has_projectile = true;
    let dp = DumbProjectileParams { max_lifespan: 200, ..DumbProjectileParams::default() };
    w.projectile = Some(Arc::new(ProjectileTemplate { behavior: ProjectileBehavior::Dumb(dp), bounding_radius: 3.0, small_missile: false, neutron_warhead: None, neutron_blast: None, garrison_hit_kill: None }));
    w.projectile_stream_name = stream.to_owned();
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

/// Attacker (player 0) at x=100 with `w`, a victim (player 1) at x=400, the attacker ordered to attack.
fn duel(w: WeaponTemplate) -> (GameLogic, zr_sim::ObjectId, zr_sim::ObjectId) {
    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
    let a = g.create_object(&Arc::new(tank("A", Some(w))), Coord3D::new(100.0, 300.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(tank("B", None)), Coord3D::new(400.0, 300.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(0, Command::Attack { ids: vec![a], target: b });
    (g, a, b)
}

#[test]
fn stream_collects_every_shot_and_dies_with_its_owner() {
    let (mut g, a, _b) = duel(dumb_weapon(4.0, "ToxinStream"));
    // Until two shots are in the air at once (the attack order keeps firing).
    for _ in 0..400 {
        g.run_frames(1);
        if g.projectiles().len() >= 2 {
            break;
        }
    }
    assert!(g.projectiles().len() >= 2, "two shots in the air");
    let streams = g.projectile_streams();
    assert_eq!(streams.len(), 1, "one stream per weapon");
    let s = &streams[0];
    assert_eq!(s.template_name, "ToxinStream");
    assert_eq!(s.owner, a);
    assert!(s.points.iter().all(|p| *p != Coord3D::ZERO), "same-target shots make no hole");
    assert!(s.points.len() >= 2);

    // The projectiles hit the victim and the owner dies: the stream is destroyed once it is empty.
    g.destroy_object(a);
    for _ in 0..600 {
        g.run_frames(1);
        if g.projectile_streams().is_empty() {
            break;
        }
    }
    assert!(g.projectile_streams().is_empty(), "a stream whose owner is dead and whose shots are gone is destroyed");
}

#[test]
fn laser_beam_runs_to_the_victim_and_expires_with_its_lifetime() {
    let mut w = weapon(999_999.0);
    w.laser = Some(LaserSpec { template_name: "TestLaser".into(), bone_name: "WEAPON01".into(), rolls: vec![(LaserRollKind::Lifetime, 10, 10)] });
    let (mut g, a, b) = duel(w);
    let mut seen = false;
    for _ in 0..200 {
        g.run_frames(1);
        if !g.lasers().is_empty() {
            seen = true;
            break;
        }
    }
    assert!(seen, "the laser weapon fired");
    // `run_frames(1)` ran the update that created the beam at frame `created`; the frame counter then reads
    // `created + 1`.
    let created = g.frame() - 1;
    let beam = g.lasers().remove(0);
    assert_eq!(beam.source, a);
    assert_eq!(beam.template_name, "TestLaser");
    assert_eq!(beam.bone_name, "WEAPON01");
    // The victim is a ground vehicle: the end point is lifted 10 above its position (Weapon.cpp:2559-2563).
    let victim = g.find_object(b).expect("victim alive").position;
    assert_eq!(beam.end, Coord3D::new(victim.x, victim.y, victim.z + 10.0));
    // `LifetimeUpdate` with min == max: the beam dies ten frames after it was created.
    assert_eq!(beam.die_frame, created + 10);
    // It is removed in the update whose frame is its die frame, so the counter reads `die + 1` afterwards.
    let mut frames = 0;
    while !g.lasers().is_empty() {
        g.run_frames(1);
        frames += 1;
        assert!(frames <= 20, "the beam never expired");
    }
    assert_eq!(g.frame(), beam.die_frame + 1, "removed in the update at its die frame");
}

/// The victim is created first, then the attacker: the attacker's id is the victim's + 1. The shooting
/// weapon's first shot is the next id, the next created object follows it.
fn victim_then_attacker(w: WeaponTemplate) -> (GameLogic, zr_sim::ObjectId, zr_sim::ObjectId) {
    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
    let b = g.create_object(&Arc::new(tank("B", None)), Coord3D::new(400.0, 300.0, 0.0), 0.0, 1);
    let a = g.create_object(&Arc::new(tank("A", Some(w))), Coord3D::new(100.0, 300.0, 0.0), 0.0, 0);
    g.set_enemies(0, 1);
    g.submit(0, Command::Attack { ids: vec![a], target: b });
    (g, a, b)
}

/// `GameLogic::allocateObjectID` is shared by objects and by the projectiles, laser beams and streams the
/// original creates with `newObject` (Weapon.cpp:1105, 2543, 2515). create A, fire one projectile, create B:
/// B = A + 2. Reverting the projectile's `allocate_object_id` makes B = A + 1.
#[test]
fn a_projectile_takes_an_object_id() {
    let (mut g, a, _b) = victim_then_attacker(dumb_weapon(4.0, ""));
    for _ in 0..200 {
        g.run_frames(1);
        if !g.projectiles().is_empty() {
            break;
        }
    }
    assert_eq!(g.projectiles().len(), 1, "one shot in the air");
    let next = g.create_object(&Arc::new(tank("C", None)), Coord3D::new(10.0, 10.0, 0.0), 0.0, 1);
    assert_eq!(next.0, a.0 + 2, "the projectile between A and C took A + 1");
}

/// A laser beam takes an object id too: created before its lifetime rolls (Object.cpp:294).
#[test]
fn a_laser_beam_takes_an_object_id() {
    let mut w = weapon(999_999.0);
    w.laser = Some(LaserSpec { template_name: "TestLaser".into(), bone_name: String::new(), rolls: vec![(LaserRollKind::Lifetime, 10, 10)] });
    let (mut g, a, _b) = victim_then_attacker(w);
    for _ in 0..200 {
        g.run_frames(1);
        if !g.lasers().is_empty() {
            break;
        }
    }
    let beam = g.lasers().remove(0);
    assert_eq!(beam.id, a.0 + 1, "the beam is the next object after A");
    let next = g.create_object(&Arc::new(tank("C", None)), Coord3D::new(10.0, 10.0, 0.0), 0.0, 1);
    assert_eq!(next.0, a.0 + 2);
}

/// A new projectile stream takes an object id after its first projectile (Weapon.cpp:2515 follows the
/// projectile's newObject at 1105): the projectile is A + 1, the stream A + 2.
#[test]
fn a_new_stream_takes_the_id_after_its_first_projectile() {
    let (mut g, a, _b) = victim_then_attacker(dumb_weapon(4.0, "ToxinStream"));
    for _ in 0..200 {
        g.run_frames(1);
        if !g.projectile_streams().is_empty() {
            break;
        }
    }
    let streams = g.projectile_streams();
    assert_eq!(streams.len(), 1);
    assert_eq!(streams[0].id, a.0 + 2, "the stream is created after the projectile");
}

/// A laser with a finite `WeaponSpeed` deals its damage in the frame it fires (Weapon.cpp:1051-1057 returns
/// with no DelayedDamage). The victim is 300 away at speed 100, a 3-frame delay on the delayed path.
#[test]
fn a_laser_hits_the_same_frame_when_it_is_on_the_victim() {
    let mut w = weapon(100.0);
    w.primary_damage = 50.0;
    w.laser = Some(LaserSpec { template_name: "TestLaser".into(), bone_name: String::new(), rolls: vec![(LaserRollKind::Lifetime, 10, 10)] });
    let (mut g, _a, b) = duel(w);
    let before = g.find_object(b).unwrap().health();
    for _ in 0..200 {
        g.run_frames(1);
        if !g.lasers().is_empty() {
            break;
        }
    }
    assert!(!g.lasers().is_empty(), "the laser fired");
    assert_eq!(g.find_object(b).unwrap().health(), before - 50.0, "the damage landed in the firing frame");
}

/// A miss: the scatter lies beyond the damage radii, so the beam ends on the ground at the scattered point
/// and the damage goes to no one (`damageID = INVALID`, Weapon.cpp:1049-1050). The victim stays at full
/// health after the shot and in the frames that follow (no DelayedDamage either).
#[test]
fn a_laser_that_misses_deals_no_damage_to_the_victim() {
    let mut w = weapon(100.0);
    w.primary_damage = 50.0;
    w.primary_damage_radius = 0.5;
    w.secondary_damage_radius = 0.5;
    w.scatter_radius = 1000.0;
    w.laser = Some(LaserSpec { template_name: "TestLaser".into(), bone_name: String::new(), rolls: vec![(LaserRollKind::Lifetime, 10, 10)] });
    let (mut g, _a, b) = duel(w);
    let before = g.find_object(b).unwrap().health();
    for _ in 0..200 {
        g.run_frames(1);
        if !g.lasers().is_empty() {
            break;
        }
    }
    let beam = g.lasers().remove(0);
    let victim = g.find_object(b).unwrap().position;
    assert!(beam.end.dist_2d(&victim) > 0.5, "the beam ends at the scattered ground point, not on the victim");
    // The flat test map is at z = 0, and a miss is not lifted by 10 (Weapon.cpp:1051, `createLaser` gets no victim).
    assert!(beam.end.z.abs() < 1e-3, "a miss ends on the ground: z = {}", beam.end.z);
    for _ in 0..20 {
        g.run_frames(1);
    }
    assert_eq!(g.find_object(b).unwrap().health(), before, "no damage to the victim");
}

/// A destroyed owner is still found until the end of the frame (`destroyObject` only queues it, and
/// `findObjectByID` reads the object vector, GameLogic.cpp:4081-4110, GameLogic.h:501-512): with its shots gone,
/// the stream survives the frame the owner is destroyed and dies the frame after. Reverting the owner test
/// to the destroyed flag makes it die in the first frame.
#[test]
fn a_stream_dies_the_frame_after_its_owner_is_removed() {
    let (mut g, a, _b) = duel(dumb_weapon(4.0, "ToxinStream"));
    for _ in 0..400 {
        g.run_frames(1);
        if !g.projectile_streams().is_empty() {
            break;
        }
    }
    // One shot, then stop firing so the ring can empty.
    g.submit(0, Command::Stop { ids: vec![a] });
    for _ in 0..400 {
        g.run_frames(1);
        if g.projectiles().is_empty() {
            break;
        }
    }
    g.run_frames(3);
    assert_eq!(g.projectile_streams().len(), 1, "the stream is alive while its owner is");
    g.destroy_object(a);
    g.run_frames(1);
    assert_eq!(g.projectile_streams().len(), 1, "the owner is still found in the frame it is destroyed");
    g.run_frames(1);
    assert!(g.projectile_streams().is_empty(), "the stream dies the frame after its owner is removed");
}

/// A weapon's FX secondary point is the victim's centre (`victimObj->getGeometryInfo().getCenterPosition`,
/// Weapon.cpp:918-926): a box is half its height above the foot (Geometry.cpp:244-255). Reverting the
/// centre to the foot fails the z check.
#[test]
fn a_weapon_fx_aims_at_the_victims_centre() {
    let mut w = weapon(999_999.0);
    w.fire_fx = Some("FX_Muzzle".into());
    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
    let a = g.create_object(&Arc::new(tank("A", Some(w))), Coord3D::new(100.0, 300.0, 0.0), 0.0, 0);
    let mut bt = tank("B", None);
    bt.geometry.shape = zr_sim::template::GeometryShape::Box;
    bt.geometry.height = 10.0;
    let b = g.create_object(&Arc::new(bt), Coord3D::new(400.0, 300.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(0, Command::Attack { ids: vec![a], target: b });
    for _ in 0..200 {
        g.run_frames(1);
        if g.fx_events().iter().any(|e| e.name == "FX_Muzzle") {
            break;
        }
    }
    let fx = g.fx_events().iter().find(|e| e.name == "FX_Muzzle").expect("the muzzle FX played");
    let t = fx.target.expect("a weapon FX has a target");
    assert!((t.z - 5.0).abs() < 1e-3, "a box's centre is half its height up, z = {}", t.z);
    assert!((t.x - 400.0).abs() < 1e-3, "x = {}", t.x);
}

/// `ProjectileStreamUpdate::getAllPoints` lifts a point within 1.5 major radii of a vehicle owner to its roof:
/// `max(z, MaxHeightAbovePosition + owner z + 0.5)` (ProjectileStreamUpdate.cpp:169-179). The first shot leaves
/// from the owner, so its point is lifted to 8 + 0.5 for an 8-radius sphere. Reverting the lift fails.
#[test]
fn a_vehicle_owner_lifts_the_stream_to_its_roof() {
    let (mut g, _a, _b) = duel(dumb_weapon(4.0, "ToxinStream"));
    for _ in 0..200 {
        g.run_frames(1);
        if !g.projectile_streams().is_empty() && !g.projectile_streams()[0].points.iter().all(|p| *p == Coord3D::ZERO) {
            break;
        }
    }
    let s = &g.projectile_streams()[0];
    let lifted: Vec<&Coord3D> = s.points.iter().filter(|p| **p != Coord3D::ZERO && (p.x - 100.0).abs() < 13.0).collect();
    assert!(!lifted.is_empty(), "a point near the owner");
    for p in lifted {
        assert!(p.z >= 8.5 - 1e-3, "the point is on the roof, z = {}", p.z);
    }
}
