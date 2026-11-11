//! Projectile end states: a projectile must leave `GameLogic::projectiles()` once its behaviour
//! says it is over. Synthetic data only.
//!
//! - `MissileAIUpdate` with `DetonateOnNoFuel = No` that runs out of fuel coasts, and once inside
//!   the lock distance `airborneTargetGone` sends it to `KILL_SELF` (no warhead).
//! - A tracked missile whose victim dies goes to `KILL_SELF` too, and is destroyed
//!   `KillSelfDelay` (3) frames later.
//! - `DumbProjectileBehavior` is removed at the end of its `MaxLifespan`.

use std::sync::Arc;
use zr_sim::damage::DamageType;
use zr_sim::projectile::{DumbProjectileParams, MissileParams, ProjectileBehavior, ProjectileTemplate};
use zr_sim::template::{kind_of, Geometry, GeometryShape, LocomotorKind, LocomotorParams};
use zr_sim::weapon::{CombatTemplate, WeaponSetTemplate, WeaponTemplate};
use zr_sim::{Command, Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

fn flat() -> TerrainLogic {
    TerrainLogic::flat(100, 100, 0)
}

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

/// A static, flying-height target (structures do not run locomotors, so the height holds).
fn wall() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Wall");
    t.kind_of = kind_of::STRUCTURE;
    t.geometry = Geometry { shape: GeometryShape::Box, major_radius: 20.0, minor_radius: 20.0, height: 20.0, is_small: false };
    t.max_health = 200.0;
    t.initial_health = 200.0;
    t
}

fn missile(params: MissileParams) -> ProjectileTemplate {
    ProjectileTemplate {
        behavior: ProjectileBehavior::Missile(params),
        bounding_radius: 3.0,
        small_missile: false,
        neutron_warhead: None,
        neutron_blast: None,
        garrison_hit_kill: None,
    }
}

/// Attacker (player 0) at x=100 with `weapon`, a static victim (player 1) at x=`victim_x`.
fn duel(weapon: WeaponTemplate, victim_x: f32) -> (GameLogic, zr_sim::ObjectId, zr_sim::ObjectId) {
    let mut g = GameLogic::new(flat(), 1);
    let a = g.create_object(&Arc::new(tank("A", Some(weapon))), Coord3D::new(100.0, 300.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(tank("B", None)), Coord3D::new(victim_x, 300.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(0, Command::Attack { ids: vec![a], target: b });
    (g, a, b)
}

#[test]
fn spent_missile_falls_and_detonates_on_landing_instead_of_hanging_in_the_air() {
    // DetonateOnNoFuel = No: out of fuel the missile has no thrust, so its physics (gravity on its
    // last velocity) takes it down. Landing is the airborne -> grounded edge, and with the goal
    // below it `projectileHandleCollision(nullptr)` detonates there. The old code froze it wherever
    // it was out of fuel.
    // The launch impulse (10 per frame, tilted up toward the raised target) must beat gravity (-1 per
    // frame²) so the missile is airborne when its fuel runs out.
    let p = MissileParams {
        lock_distance: 25.0,
        initial_velocity: 10.0,
        fuel_lifetime: 5,
        detonate_on_no_fuel: false,
        ..MissileParams::default()
    };
    // The target is a structure raised to 200 units, so the missile is still climbing when it runs
    // out of fuel (a missile at ground level would land on the spot either way).
    let mut g = GameLogic::new(flat(), 1);
    let a = g.create_object(&Arc::new(tank("A", Some(weapon_with(missile(p), 2.0)))), Coord3D::new(100.0, 300.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(wall()), Coord3D::new(400.0, 300.0, 0.0), 0.0, 1);
    g.set_object_position(b, Coord3D::new(400.0, 300.0, 200.0));
    g.set_enemies(0, 1);
    g.submit(0, Command::Attack { ids: vec![a], target: b });
    let mut peak_z = 0.0f32;
    for _ in 0..20 {
        g.run_frames(1);
        for live in g.projectiles() {
            peak_z = peak_z.max(live.pos.z);
        }
    }
    assert!(peak_z > 1.0, "the missile climbs toward the raised target (peak z {peak_z})");
    g.run_frames(400);
    assert_eq!(g.projectiles().len(), 0, "the spent missile lands and detonates (projectileHandleCollision(nullptr))");
    assert_eq!(g.find_object(b).unwrap().health(), 200.0, "the landing is far from the structure, so it is not hit");
}

#[test]
fn missile_whose_target_dies_in_flight_removes_itself() {
    // airborneTargetGone: a tracked missile with no victim goes KILL_SELF and is destroyed
    // KillSelfDelay (3) frames later, without detonating.
    let p = MissileParams { initial_velocity: 12.0, lock_distance: 25.0, ..MissileParams::default() };
    let (mut g, _, b) = duel(weapon_with(missile(p), 12.0), 400.0);
    g.run_frames(5);
    assert_eq!(g.projectiles().len(), 1, "the missile is in flight");
    g.destroy_object(b);
    g.run_frames(6);
    assert_eq!(g.projectiles().len(), 0, "the missile is gone KillSelfDelay frames after its target dies");
}

#[test]
fn dumb_shell_is_removed_at_the_end_of_its_lifespan() {
    // DumbProjectileBehavior::update: MaxLifespan forces detonation even mid-arc.
    let dp = DumbProjectileParams { max_lifespan: 20, ..DumbProjectileParams::default() };
    let proj = ProjectileTemplate {
        behavior: ProjectileBehavior::Dumb(dp),
        bounding_radius: 3.0,
        small_missile: false,
        neutron_warhead: None,
        neutron_blast: None,
        garrison_hit_kill: None,
    };
    let mut w = weapon_with(proj, 2.0);
    w.attack_range = 5000.0; // fire at once, from across the map
    let (mut g, _, _) = duel(w, 2100.0);
    g.run_frames(10);
    assert_eq!(g.projectiles().len(), 1, "the shell is still in the air");
    g.run_frames(15);
    assert_eq!(g.projectiles().len(), 0, "the shell detonates at its lifespan");
}

#[test]
fn missile_flies_along_its_orientation_in_the_no_turn_phase() {
    // DistanceToTravelBeforeTurning: `setMaxTurnRate(0)` keeps the launch orientation, so the missile
    // flies along it (the old code moved it along a zero displacement and it sat at its launcher).
    // No launch impulse: only the thrust along the orientation can move it during the no-turn distance.
    let p = MissileParams { initial_velocity: 0.0, initial_dist: 40.0, lock_distance: 25.0, ..MissileParams::default() };
    let (mut g, _, _) = duel(weapon_with(missile(p), 2.0), 400.0);
    g.run_frames(6);
    let live = g.projectiles();
    assert_eq!(live.len(), 1, "the missile is in flight");
    assert!(live[0].pos.x > 105.0, "it flies along its orientation (x {})", live[0].pos.x);
    assert!((live[0].pos.y - 300.0).abs() < 1e-3, "it does not turn (y {})", live[0].pos.y);
}

#[test]
fn missile_without_an_initial_velocity_gets_no_launch_impulse() {
    // `projectileFireAtObjectOrPosition`: `applyMotiveForce` only when `initialVelToUse > 0`, and
    // there is no thrust before ignition.
    let p = MissileParams { initial_velocity: 0.0, ignition_delay: 10, lock_distance: 25.0, ..MissileParams::default() };
    let (mut g, _, _) = duel(weapon_with(missile(p), 6.0), 400.0);
    g.run_frames(5);
    let live = g.projectiles();
    assert_eq!(live.len(), 1, "the missile is in its launch phase");
    assert!((live[0].pos.x - 100.0).abs() < 1e-3, "no impulse, so it stays put (x {})", live[0].pos.x);
}

#[test]
fn missile_launches_along_the_barrel_before_it_tilts_to_the_target() {
    // `MissileAIUpdate::projectileFireAtObjectOrPosition`: the missile starts facing the launch transform's
    // X vector. Here the body faces north and the turret swings east to the target.
    let mut g = GameLogic::new(flat(), 1);
    let p = MissileParams { initial_velocity: 6.0, ignition_delay: 10, lock_distance: 25.0, ..MissileParams::default() };
    let mut t = tank("A", Some(weapon_with(missile(p), 6.0)));
    t.combat.turret_turn_rate = Some(1.0);
    let a = g.create_object(&Arc::new(t), Coord3D::new(100.0, 300.0, 0.0), std::f32::consts::FRAC_PI_2, 0);
    let b = g.create_object(&Arc::new(tank("B", None)), Coord3D::new(400.0, 300.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(0, Command::Attack { ids: vec![a], target: b });
    g.run_frames(5);
    let live = g.projectiles();
    assert_eq!(live.len(), 1, "the missile is in flight");
    assert!(live[0].pos.y > 310.0, "it flies out along the body (y {})", live[0].pos.y);
    assert!((live[0].pos.x - 100.0).abs() < 1.0, "it does not head for the target yet (x {})", live[0].pos.x);
}

#[test]
fn missile_accelerates_under_its_locomotor_thrust() {
    // `Locomotor`: from rest the thrust builds the speed up by `Acceleration` (0.75 per frame²) to `Speed`.
    let p = MissileParams { initial_velocity: 0.0, lock_distance: 25.0, max_speed: 7.5, max_accel: 0.75, ..MissileParams::default() };
    let (mut g, _, _) = duel(weapon_with(missile(p), 6.0), 400.0);
    let mut frames = 0;
    while g.projectiles().is_empty() && frames < 20 {
        g.run_frames(1);
        frames += 1;
    }
    let mut prev = g.projectiles()[0].pos;
    let mut steps = Vec::new();
    for _ in 0..8 {
        g.run_frames(1);
        let now = g.projectiles()[0].pos;
        steps.push(((now.x - prev.x).powi(2) + (now.y - prev.y).powi(2) + (now.z - prev.z).powi(2)).sqrt());
        prev = now;
    }
    assert!(steps[0] < 2.0, "it starts slowly (first step {:.3})", steps[0]);
    assert!(steps[7] > steps[0] + 3.0, "it speeds up ({steps:?})");
    assert!(steps.iter().all(|s| *s <= 7.6), "it is capped at its speed ({steps:?})");
}

#[test]
fn missile_brakes_onto_its_target_instead_of_detonating_at_the_lock_distance() {
    // `doKillState`: inside the lock distance the missile brakes straight at its goal and detonates when
    // it arrives, so the last place it is seen is near the target, not 75 units out.
    let p = MissileParams { initial_velocity: 6.0, lock_distance: 75.0, ..MissileParams::default() };
    let (mut g, _, b) = duel(weapon_with(missile(p), 6.0), 400.0);
    let mut last = None;
    for _ in 0..200 {
        g.run_frames(1);
        match g.projectiles().first() {
            Some(s) => last = Some(s.pos),
            None if last.is_some() => break,
            None => {}
        }
    }
    let last = last.expect("the missile was in flight");
    let d = ((last.x - 400.0).powi(2) + (last.y - 300.0).powi(2) + last.z.powi(2)).sqrt();
    assert!(d < 40.0, "it detonates at the target (last seen {d:.1} away)");
    assert_eq!(g.find_object(b).unwrap().health(), 150.0, "the warhead hits the target");
}

#[test]
fn coordinate_shot_does_not_follow_a_victim_that_moves_away() {
    // TryToFollowTarget = No: the missile flies to the spot it was aimed at (`aiMoveToPosition`) and does not
    // track the victim, so the victim that moved away is not hit.
    let p = MissileParams { try_to_follow_target: false, initial_velocity: 6.0, lock_distance: 75.0, ..MissileParams::default() };
    let (mut g, _, b) = duel(weapon_with(missile(p), 6.0), 400.0);
    g.run_frames(5);
    g.set_object_position(b, Coord3D::new(400.0, 600.0, 0.0));
    g.run_frames(200);
    assert_eq!(g.projectiles().len(), 0, "it has detonated at the spot it was aimed at");
    assert_eq!(g.find_object(b).unwrap().health(), 200.0, "the victim, which moved away, is not hit");
}

#[test]
fn missile_takes_on_its_launchers_velocity() {
    // `Weapon::positionProjectileForLaunch`: `launcherPhys->transferVelocityTo(missilePhys)`. With no
    // impulse of its own the missile can only move with its launcher during the launch phase.
    let mut g = GameLogic::new(flat(), 1);
    let p = MissileParams { initial_velocity: 0.0, ignition_delay: 10, lock_distance: 25.0, ..MissileParams::default() };
    let mut ta = tank("A", Some(weapon_with(missile(p), 6.0)));
    ta.physics = Some(zr_sim::physics::PhysicsTemplate::default()); // a vehicle has a PhysicsBehavior
    // A ground locomotor's `locoUpdate_maintainCurrentPosition` is `scrubVelocity2D(0)` with no move
    // order (`Locomotor.cpp:2601-2608`), so an idle tank stops dead; HOVER brakes with a motive
    // force instead (`:2567-2597`) and keeps the velocity this test hands it.
    ta.locomotor = Some(zr_sim::template::LocomotorParams::from_engine_units(zr_sim::template::LocomotorKind::Hover, 1.0, 0.0, 0.2, 0.0556, 0.1, true));
    let a = g.create_object(&Arc::new(ta), Coord3D::new(100.0, 300.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(tank("B", None)), Coord3D::new(400.0, 300.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    // Give the launcher a physics velocity (6 per frame east) and fire straight away. The motive
    // force only becomes velocity in the launcher's physics update, and a `PhysicsBehavior`'s first
    // update is one frame after creation (`PhysicsUpdate.cpp:231-233`), so add the velocity the
    // force would produce as well.
    g.apply_motive_force(a, Coord3D::new(6.0, 0.0, 0.0));
    g.add_velocity_to(a, Coord3D::new(6.0, 0.0, 0.0));
    g.submit(0, Command::Attack { ids: vec![a], target: b });
    let mut frames = 0;
    while g.projectiles().is_empty() && frames < 20 {
        g.run_frames(1);
        frames += 1;
    }
    let first = g.projectiles()[0].pos;
    let launcher_then = g.find_object(a).unwrap().position.x;
    g.run_frames(3);
    let missile_dx = g.projectiles()[0].pos.x - first.x;
    let launcher_dx = g.find_object(a).unwrap().position.x - launcher_then;
    assert!(launcher_dx.abs() > 0.5, "the launcher was moving ({launcher_dx})");
    assert!(missile_dx > 0.5 * launcher_dx && missile_dx > 0.5, "the missile moves with the launcher (missile {missile_dx}, launcher {launcher_dx})");
}
