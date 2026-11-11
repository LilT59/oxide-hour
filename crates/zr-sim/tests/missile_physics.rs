//! Missile and shell physics the retail `MissileAIUpdate` / `DumbProjectileBehavior` / `Locomotor` code adds
//! (lane 60): the cruise height and dive, the braking floor, the arrival gate of `KILL`, a shell's physics
//! (launcher velocity, gravity, landing), and the barrel's rest rotation as the launch heading. Synthetic data only.

use std::sync::Arc;
use zr_sim::damage::DamageType;
use zr_sim::projectile::{DumbProjectileParams, MissileParams, ProjectileBehavior, ProjectileTemplate};
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::weapon::{CombatTemplate, LaunchBarrel, WeaponSetTemplate, WeaponTemplate};
use zr_sim::{Command, Coord3D, GameLogic, ObjectTemplate, ObjectId, TerrainLogic};

fn flat() -> TerrainLogic {
    TerrainLogic::flat(100, 100, 0)
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

fn shell(params: DumbProjectileParams) -> ProjectileTemplate {
    ProjectileTemplate { behavior: ProjectileBehavior::Dumb(params), bounding_radius: 3.0, small_missile: false, neutron_warhead: None, neutron_blast: None, garrison_hit_kill: None }
}

/// Attacker (player 0) at x=100 with `weapon` and a launch bone `barrel` (`None`: the object's origin), a victim
/// (player 1) at (`victim_x`, 300). `launcher_vel` gives the attacker a physics velocity (per frame, east).
fn duel(weapon: WeaponTemplate, victim_x: f32, barrel: Option<LaunchBarrel>, launcher_vel: f32) -> (GameLogic, ObjectId, ObjectId) {
    let mut g = GameLogic::new(flat(), 1);
    let mut ta = tank("A", Some(weapon));
    if let Some(b) = barrel {
        ta.combat.launch_bones.slots[0] = vec![b];
    }
    ta.physics = Some(zr_sim::physics::PhysicsTemplate::default());
    let a = g.create_object(&Arc::new(ta), Coord3D::new(100.0, 300.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(tank("B", None)), Coord3D::new(victim_x, 300.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    if launcher_vel != 0.0 {
        g.apply_motive_force(a, Coord3D::new(launcher_vel, 0.0, 0.0));
    }
    g.submit(0, Command::Attack { ids: vec![a], target: b });
    (g, a, b)
}

fn dist_2d(p: Coord3D, x: f32, y: f32) -> f32 {
    ((p.x - x).powi(2) + (p.y - y).powi(2)).sqrt()
}

#[test]
fn a_missile_cruises_at_its_preferred_height_then_dives_onto_its_goal() {
    // Locomotor PreferredHeight (Locomotor.cpp:1935-1943): with no dive yet the thrust aims at 120 above the
    // ground under the missile. Inside DistanceToTargetBeforeDiving (200) doAttackState sets PRECISE_Z_POS
    // (MissileAIUpdate.cpp:568-571) and the thrust aims at the goal's height, so the missile comes down onto it.
    let p = MissileParams { initial_velocity: 6.0, lock_distance: 25.0, preferred_height: 120.0, preferred_height_damping: 1.0, dive_distance: 200.0, ..MissileParams::default() };
    let (mut g, _, _) = duel(weapon_with(missile(p), 6.0), 900.0, None, 0.0);
    let mut trace: Vec<(f32, f32)> = Vec::new();
    let mut last_z = f32::NAN;
    let mut launched = false;
    for _ in 0..400 {
        g.run_frames(1);
        match g.projectiles().first() {
            Some(s) => {
                launched = true;
                trace.push((dist_2d(s.pos, 900.0, 300.0), s.pos.z));
                last_z = s.pos.z;
            }
            None if launched => break,
            None => {}
        }
    }
    // Out at 400 it has climbed off the ground (no cruise height: it would fly the straight line at z 0), and
    // inside the 200 dive distance it comes down onto the goal.
    let at_400 = trace.iter().find(|(d, _)| *d <= 400.0).map_or(f32::NAN, |(_, z)| *z);
    let peak = trace.iter().map(|(_, z)| *z).fold(f32::MIN, f32::max);
    let at_100 = trace.iter().find(|(d, _)| *d <= 100.0).map_or(f32::NAN, |(_, z)| *z);
    assert!(at_400 > 40.0, "it climbs toward its preferred height on the way out (z {at_400:.1} at 400)");
    assert!(peak > 80.0, "it reaches a cruise altitude before the dive (peak z {peak:.1})");
    assert!(at_100 < 70.0, "inside the dive distance it comes down (z {at_100:.1} at 100)");
    assert!(last_z < 30.0, "it dives onto the goal's height at the end (last z {last_z:.1})");
}

#[test]
fn a_braking_missile_slows_toward_its_min_speed_near_its_goal() {
    // Locomotor Braking (AuroraBomb only in the shipped data): `moveTowardsPositionThrust` drops the desired speed
    // to MinSpeed inside `calcSlowDownDist` of the goal and brakes with a negative acceleration (Locomotor.cpp:1917-1936).
    let p = MissileParams {
        initial_velocity: 6.0,
        lock_distance: 5.0,
        max_speed: 10.0,
        max_accel: 2.0,
        min_speed: 2.0,
        braking: 0.5,
        try_to_follow_target: true,
        ..MissileParams::default()
    };
    let (mut g, _, _) = duel(weapon_with(missile(p), 6.0), 500.0, None, 0.0);
    let mut far: Vec<f32> = Vec::new();
    let mut near: Vec<f32> = Vec::new();
    let mut prev: Option<Coord3D> = None;
    let mut launched = false;
    for _ in 0..400 {
        g.run_frames(1);
        let Some(s) = g.projectiles().first().map(|s| s.pos) else {
            if launched {
                break;
            }
            continue;
        };
        launched = true;
        if let Some(q) = prev {
            let step = ((s.x - q.x).powi(2) + (s.y - q.y).powi(2) + (s.z - q.z).powi(2)).sqrt();
            let d = dist_2d(s, 500.0, 300.0);
            if d > 150.0 {
                far.push(step);
            } else if (25.0..60.0).contains(&d) {
                near.push(step);
            }
        }
        prev = Some(s);
    }
    let top = far.iter().copied().fold(0.0f32, f32::max);
    let low = near.iter().copied().fold(f32::MAX, f32::min);
    assert!(top > 6.0, "it reaches its speed far out (top step {top:.2})");
    assert!(low < 0.85 * top, "it slows down near its goal (slowest step {low:.2}, top {top:.2}; {near:?})");
    assert!(low > 1.0, "it does not stop below MinSpeed (slowest step {low:.2})");
}

#[test]
fn a_kill_missile_detonates_once_it_has_arrived_on_its_goal() {
    // doKillState acts when the AI is idle: the move has arrived (`CloseEnoughDist`), then the missile is set on the
    // goal and detonates (MissileAIUpdate.cpp:604-630). The missile must still be seen on the goal, not up to a
    // max-speed step away as a bounding-sphere test would allow.
    let p = MissileParams { initial_velocity: 6.0, lock_distance: 75.0, ..MissileParams::default() };
    let (mut g, _, b) = duel(weapon_with(missile(p), 6.0), 400.0, None, 0.0);
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
    let d = dist_2d(last, 400.0, 300.0);
    assert!(d < 1.0, "the last place it is seen is the goal (its distance {d:.2})");
    assert_eq!(g.find_object(b).unwrap().health(), 150.0, "the warhead hits the target");
}

#[test]
fn a_shell_with_physics_lands_on_the_ground_before_its_flight_path_ends() {
    // PhysicsBehavior on a shell: gravity adds to m_vel every frame, the velocity moves the shell after its flight
    // step, and the landing (airborne -> grounded) detonates it (PhysicsUpdate.cpp:648-768, DumbProjectile
    // projectileHandleCollision(nullptr)). Without physics the shell detonates at the end of its arc.
    let flight = |physics: bool| {
        let dp = DumbProjectileParams { first_height: 60.0, second_height: 60.0, physics, ..DumbProjectileParams::default() };
        let mut ta = tank("A", Some(weapon_with(shell(dp), 2.0)));
        ta.combat.launch_bones.slots[0] = vec![LaunchBarrel { pos: [0.0, 0.0, 10.0], rot: [0.0, 0.0, 0.0, 1.0] }];
        let mut g = GameLogic::new(flat(), 1);
        let a = g.create_object(&Arc::new(ta), Coord3D::new(100.0, 300.0, 0.0), 0.0, 0);
        let b = g.create_object(&Arc::new(tank("B", None)), Coord3D::new(400.0, 300.0, 0.0), 0.0, 1);
        g.set_enemies(0, 1);
        g.submit(0, Command::Attack { ids: vec![a], target: b });
        let mut frames = 0;
        while g.projectiles().is_empty() && frames < 20 {
            g.run_frames(1);
            frames += 1;
        }
        let mut last_z = f32::NAN;
        let mut alive = 0;
        while let Some(s) = g.projectiles().first() {
            last_z = s.pos.z;
            alive += 1;
            g.run_frames(1);
            if alive > 400 {
                break;
            }
        }
        (alive, last_z)
    };
    let (plain, _) = flight(false);
    let (with_physics, landed_z) = flight(true);
    assert!(with_physics + 5 < plain, "the physics shell lands early ({with_physics} frames vs {plain})");
    // The landing point is the clamped physics position, one frame after the last snapshot the test can see, which is
    // still below the 10-unit launch height (the shell has been falling).
    assert!(landed_z < 11.0, "it has fallen before it lands (last seen z {landed_z:.2})");
}

#[test]
fn a_shell_moves_on_its_path_and_the_launchers_velocity_does_not_offset_the_step() {
    // Weapon::positionProjectileForLaunch -> transferVelocityTo (Weapon.cpp:3145) gives the shell the launcher's velocity.
    // The shell's PhysicsBehavior runs in PHASE_PHYSICS, before its DumbProjectileBehavior (PHASE_NORMAL, UpdateModule.h:
    // 80-87): the physics moves the object by that velocity, then the flight step sets the absolute path position
    // (DumbProjectileBehavior.cpp:632-689). The velocity therefore changes the landing, not the per-frame step.
    let run = |launcher_vel: f32| {
        let dp = DumbProjectileParams { first_height: 60.0, second_height: 60.0, physics: true, ..DumbProjectileParams::default() };
        let mut ta = tank("A", Some(weapon_with(shell(dp), 2.0)));
        ta.combat.launch_bones.slots[0] = vec![LaunchBarrel { pos: [0.0, 0.0, 10.0], rot: [0.0, 0.0, 0.0, 1.0] }];
        ta.physics = Some(zr_sim::physics::PhysicsTemplate::default()); // a vehicle has a PhysicsBehavior
        let mut g = GameLogic::new(flat(), 1);
        let a = g.create_object(&Arc::new(ta), Coord3D::new(100.0, 300.0, 0.0), 0.0, 0);
        let b = g.create_object(&Arc::new(tank("B", None)), Coord3D::new(400.0, 300.0, 0.0), 0.0, 1);
        g.set_enemies(0, 1);
        if launcher_vel != 0.0 {
            g.apply_motive_force(a, Coord3D::new(launcher_vel, 0.0, 0.0));
        }
        g.submit(0, Command::Attack { ids: vec![a], target: b });
        // The shell is launched in the third update: the first AI update is frame 1 for a unit created before the
        // first update (AIUpdate.cpp:299, GameLogic.cpp:3929), the aim is that frame and the fire the next.
        g.run_frames(3);
        // The shell's movement over the frame (start to end): the flight step only, since the physics displacement is
        // overwritten by the path position. Both shells take the same step.
        g.projectiles()[0].velocity
    };
    let still = run(0.0);
    let moving = run(6.0);
    let dx = moving.x - still.x;
    assert!(dx.abs() < 1e-3, "the launcher's velocity does not move the shell's path step (per frame dx {dx:.4})");
}

#[test]
fn a_missile_flies_along_its_barrels_rest_rotation() {
    // Weapon::calcProjectileLaunchPosition: the missile's transform is the launch bone's matrix (its rest rotation
    // included), and MissileAIUpdate::projectileFireAtObjectOrPosition takes its X vector as the heading. Here the
    // barrel bone faces +Y (a 90 degree turn about Z), so the missile leaves north though the body faces east.
    let half = std::f32::consts::FRAC_PI_4;
    let rest = [0.0, 0.0, half.sin(), half.cos()];
    let p = MissileParams { initial_velocity: 0.0, initial_dist: 40.0, lock_distance: 25.0, ..MissileParams::default() };
    let (mut g, _, _) = duel(weapon_with(missile(p), 6.0), 400.0, Some(LaunchBarrel { pos: [0.0, 0.0, 0.0], rot: rest }), 0.0);
    let h = first_heading(&mut g);
    assert!(h.y > 0.95 && h.x.abs() < 0.05, "it leaves along the barrel's +Y (heading {h:?})");
}

#[test]
fn a_missile_without_a_barrel_rotation_leaves_along_the_body() {
    // The same shot with an identity rest rotation: the body faces east, so the missile leaves east.
    let p = MissileParams { initial_velocity: 0.0, initial_dist: 40.0, lock_distance: 25.0, ..MissileParams::default() };
    let (mut g, _, _) = duel(weapon_with(missile(p), 6.0), 400.0, Some(LaunchBarrel { pos: [0.0, 0.0, 0.0], rot: [0.0, 0.0, 0.0, 1.0] }), 0.0);
    let h = first_heading(&mut g);
    assert!(h.x > 0.95, "it leaves east along the body (heading {h:?})");
}

/// Runs until the first missile is in the air and returns its heading (its first frame's orientation).
fn first_heading(g: &mut GameLogic) -> Coord3D {
    for _ in 0..40 {
        if let Some(s) = g.projectiles().first() {
            return s.heading;
        }
        g.run_frames(1);
    }
    panic!("no missile was fired");
}
