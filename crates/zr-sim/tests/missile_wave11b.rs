//! Lane 60 wave 11b: the missile and shell update order and details checked against the C++ (synthetic data only).
//! Each test is named for the review item it covers and fails with that fix reverted (see the LOG entry).

use std::sync::Arc;
use zr_sim::damage::DamageType;
use zr_sim::physics::PhysicsTemplate;
use zr_sim::projectile::{DumbProjectileParams, MissileParams, ProjectileBehavior, ProjectileTemplate};
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::weapon::{CombatTemplate, LaunchBarrel, WeaponSetTemplate, WeaponTemplate};
use zr_sim::{Command, Coord3D, GameLogic, ObjectId, ObjectTemplate, TerrainLogic};

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

/// Attacker (player 0) at (100, 300) with `weapon`, victim (player 1) at (`vx`, `vy`).
fn duel_at(weapon: WeaponTemplate, vx: f32, vy: f32) -> (GameLogic, ObjectId, ObjectId) {
    let mut g = GameLogic::new(flat(), 1);
    let a = g.create_object(&Arc::new(tank("A", Some(weapon))), Coord3D::new(100.0, 300.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(tank("B", None)), Coord3D::new(vx, vy, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(0, Command::Attack { ids: vec![a], target: b });
    (g, a, b)
}

#[test]
fn a_diving_missile_with_the_default_braking_takes_no_kick_at_its_normal_speed() {
    // Item 1, the adapter's defaults. The locomotor's Braking defaults to BIGNUM (Locomotor.cpp:289; zr-templates too), so
    // a shipped missile that does not set it would brake at -BIGNUM whenever its forward speed is not below the desired
    // one. Every shipped missile sets Braking (118 at 0, the 3 AuroraBomb weapons at 1.07; no UseWeaponSpeed missile), so
    // none does. With the speed at its normal cruise, the forward speed (PhysicsUpdate.cpp:988-1000) stays below the max
    // and the thrust is never the braking one, even while diving on a diagonal.
    let p = MissileParams {
        braking: 99999.0,
        preferred_height: 60.0,
        preferred_height_damping: 0.7,
        dive_distance: 150.0,
        lock_distance: 25.0,
        ..MissileParams::default()
    };
    let (mut g, _, b) = duel_at(weapon_with(missile(p), 6.0), 500.0, 500.0);
    let mut prev: Option<Coord3D> = None;
    let mut steps = Vec::new();
    let mut launched = false;
    for _ in 0..300 {
        g.run_frames(1);
        match g.projectiles().first().map(|s| s.pos) {
            Some(s) => {
                launched = true;
                if let Some(q) = prev {
                    steps.push(((s.x - q.x).powi(2) + (s.y - q.y).powi(2) + (s.z - q.z).powi(2)).sqrt());
                }
                prev = Some(s);
            }
            None if launched => break,
            None => {}
        }
    }
    assert!(launched, "the missile was fired");
    let worst = steps.iter().copied().fold(0.0f32, f32::max);
    assert!(worst < 8.0, "no kick: no step beyond the 7.5 cruise (largest {worst:.2})");
    assert!(g.find_object(b).unwrap().health() < 200.0, "the missile reaches its target");
}

#[test]
fn a_missile_thrusts_in_the_frame_it_ignites_because_the_ai_runs_before_the_physics() {
    // Item 2(b). PHASE_INITIAL (the missile's AI and locomotor drive, AIUpdate.h:607) runs before PHASE_PHYSICS (the
    // missile's PhysicsBehavior, PhysicsUpdate.h:220). The thrust the drive sets in this frame is integrated in this
    // frame, so with no ignition delay the first update already moves the missile.
    let p = MissileParams { initial_velocity: 0.0, ignition_delay: 0, lock_distance: 25.0, ..MissileParams::default() };
    let (mut g, _, _) = duel_at(weapon_with(missile(p), 6.0), 400.0, 300.0);
    let mut before = None;
    for _ in 0..20 {
        if let Some(s) = g.projectiles().first() {
            before = Some(s.pos);
            break;
        }
        g.run_frames(1);
    }
    let before = before.expect("the missile was fired");
    g.run_frames(1);
    let after = g.projectiles()[0].pos;
    let step = ((after.x - before.x).powi(2) + (after.y - before.y).powi(2) + (after.z - before.z).powi(2)).sqrt();
    assert!(step > 0.5, "its first update thrusts (step {step:.3}; 0.75 is the thrust per frame)");
}

#[test]
fn a_tracked_missile_keeps_thrusting_after_its_victim_dies_until_it_is_removed() {
    // Item 12. KILL_SELF (airborneTargetGone) sets no locomotor state (doKillSelfState), so the drive keeps the last
    // thrust and goal for KillSelfDelay frames. The missile keeps speeding up instead of coasting.
    let p = MissileParams { initial_velocity: 0.0, lock_distance: 25.0, max_speed: 7.5, max_accel: 0.75, ..MissileParams::default() };
    let (mut g, _, b) = duel_at(weapon_with(missile(p), 6.0), 600.0, 300.0);
    let mut frames = 0;
    while g.projectiles().is_empty() && frames < 20 {
        g.run_frames(1);
        frames += 1;
    }
    g.run_frames(5);
    g.destroy_object(b);
    let mut steps = Vec::new();
    let mut prev = g.projectiles()[0].pos;
    for _ in 0..2 {
        g.run_frames(1);
        let now = g.projectiles()[0].pos;
        steps.push(((now.x - prev.x).powi(2) + (now.y - prev.y).powi(2) + (now.z - prev.z).powi(2)).sqrt());
        prev = now;
    }
    assert!(steps[1] > steps[0] + 0.05, "it keeps accelerating toward its last goal ({steps:?})");
}

#[test]
fn a_missile_braking_cheat_skips_the_xy_integration_of_the_first_kill_frame() {
    // Item 2(b) timing. doKillState sets OBJECT_STATUS_BRAKING in the AI phase, so the physics of the same frame does
    // not integrate. The sampled positions of a braking missile then move straight at the goal at the cheat's speed.
    let p = MissileParams { initial_velocity: 6.0, lock_distance: 75.0, ..MissileParams::default() };
    let (mut g, _, _) = duel_at(weapon_with(missile(p), 6.0), 400.0, 300.0);
    let mut last_x: Option<f32> = None;
    let mut max_drop = 0.0f32;
    for _ in 0..200 {
        g.run_frames(1);
        match g.projectiles().first().map(|s| s.pos) {
            Some(s) => {
                if let Some(x) = last_x {
                    // Toward the goal at x = 400 the missile only ever gets closer: no frame moves it away.
                    let moved_away: f32 = if x < 400.0 { (s.x - x).min(0.0f32).abs() } else { 0.0 };
                    max_drop = max_drop.max(moved_away);
                }
                last_x = Some(s.x);
            }
            None => break,
        }
    }
    assert!(max_drop < 1e-3, "no frame carries the braking missile away from its goal (drop {max_drop:.4})");
}

#[test]
fn a_tumbling_shell_turns_away_from_its_path_and_a_plain_shell_faces_its_step() {
    // Items 6 and 7. A plain shell faces its flight step (DumbProjectileBehavior.cpp:643-683): its orientation is the
    // direction of its last step, which is its velocity. A TumbleRandomly shell does not follow the path (line 634) and
    // turns by the rates drawn at launch, applied by its physics each frame.
    fn heading_against_step(tumble: bool, frames: u32) -> f32 {
        let dp = DumbProjectileParams { first_height: 60.0, second_height: 60.0, physics: true, tumble, ..DumbProjectileParams::default() };
        let mut ta = tank("A", Some(weapon_with(shell(dp), 2.0)));
        ta.combat.launch_bones.slots[0] = vec![LaunchBarrel { pos: [0.0, 0.0, 10.0], rot: [0.0, 0.0, 0.0, 1.0] }];
        ta.physics = Some(PhysicsTemplate::default());
        let mut g = GameLogic::new(flat(), 1);
        let a = g.create_object(&Arc::new(ta), Coord3D::new(100.0, 300.0, 0.0), 0.0, 0);
        let b = g.create_object(&Arc::new(tank("B", None)), Coord3D::new(400.0, 300.0, 0.0), 0.0, 1);
        g.set_enemies(0, 1);
        g.submit(0, Command::Attack { ids: vec![a], target: b });
        g.run_frames(frames);
        let s = &g.projectiles()[0];
        let v = s.velocity;
        let vl = (v.x * v.x + v.y * v.y + v.z * v.z).sqrt();
        let h = s.heading;
        (h.x * v.x + h.y * v.y + h.z * v.z) / vl
    }
    // Frame 1 of the flight is step 0 (the launch point itself, no movement); from the next step on the velocity is a step.
    // The fire is one update later than the old numbering (the first AI update is frame 1 for a unit created before the
    // first update, AIUpdate.cpp:299, GameLogic.cpp:3929), so the shell is launched in the third update and each count is one more.
    let plain_first = heading_against_step(false, 4);
    assert!(plain_first > 0.999, "a plain shell faces its first step (cos {plain_first:.4})");
    let plain = heading_against_step(false, 5);
    assert!(plain > 0.999, "and keeps facing it (cos {plain:.4})");
    // A tumbling shell is turned by its rates each frame: its heading changes between two frames of its flight.
    fn tumble_heading(frames: u32) -> Coord3D {
        let dp = DumbProjectileParams { first_height: 60.0, second_height: 60.0, physics: true, tumble: true, ..DumbProjectileParams::default() };
        let mut ta = tank("A", Some(weapon_with(shell(dp), 2.0)));
        ta.combat.launch_bones.slots[0] = vec![LaunchBarrel { pos: [0.0, 0.0, 10.0], rot: [0.0, 0.0, 0.0, 1.0] }];
        ta.physics = Some(PhysicsTemplate::default());
        let mut g = GameLogic::new(flat(), 1);
        let a = g.create_object(&Arc::new(ta), Coord3D::new(100.0, 300.0, 0.0), 0.0, 0);
        let b = g.create_object(&Arc::new(tank("B", None)), Coord3D::new(400.0, 300.0, 0.0), 0.0, 1);
        g.set_enemies(0, 1);
        g.submit(0, Command::Attack { ids: vec![a], target: b });
        g.run_frames(frames);
        g.projectiles()[0].heading
    }
    let (h2, h6) = (tumble_heading(3), tumble_heading(7));
    let turned = ((h2.x - h6.x).powi(2) + (h2.y - h6.y).powi(2) + (h2.z - h6.z).powi(2)).sqrt();
    assert!(turned > 0.1, "a tumbling shell turns between frames 2 and 6 (heading {h2:?} -> {h6:?})");
}

#[test]
fn a_shell_faces_its_first_arc_step_on_its_first_update() {
    // Item 6, step 0 (DumbProjectileBehavior.cpp:651-683): with no earlier step the orientation is path[1] - path[0], the
    // arc's first step, which climbs. The launch heading is the barrel's (level, along the body).
    let dp = DumbProjectileParams { first_height: 60.0, second_height: 60.0, physics: true, ..DumbProjectileParams::default() };
    let mut ta = tank("A", Some(weapon_with(shell(dp), 2.0)));
    ta.combat.launch_bones.slots[0] = vec![LaunchBarrel { pos: [0.0, 0.0, 10.0], rot: [0.0, 0.0, 0.0, 1.0] }];
    ta.physics = Some(PhysicsTemplate::default());
    let mut g = GameLogic::new(flat(), 1);
    let a = g.create_object(&Arc::new(ta), Coord3D::new(100.0, 300.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(tank("B", None)), Coord3D::new(400.0, 300.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(0, Command::Attack { ids: vec![a], target: b });
    for _ in 0..20 {
        if !g.projectiles().is_empty() {
            break;
        }
        g.run_frames(1);
    }
    // The Bezier leaves its launch point straight up (the first control point is above it), so step 0 is vertical. The
    // snapshot is taken after the shell's first update.
    let after = g.projectiles()[0].heading;
    assert!(after.z > 0.5, "after its first update it faces the arc's first step (heading {after:?})");
}

#[test]
fn a_shell_without_physics_follows_its_arc_under_the_ground() {
    // DumbProjectileBehavior::update has no ground test (only the lifespan, the path end and the bridge layer), so a shell
    // with no PhysicsBehavior that dips below the terrain keeps its path to the end. The old code detonated at z < ground.
    let dp = DumbProjectileParams { first_height: -30.0, second_height: -30.0, physics: false, ..DumbProjectileParams::default() };
    let mut w = weapon_with(shell(dp), 2.0);
    w.attack_range = 2000.0;
    let (mut g, _, _) = duel_at(w, 400.0, 300.0);
    let mut frames = 0;
    while g.projectiles().is_empty() && frames < 20 {
        g.run_frames(1);
        frames += 1;
    }
    frames = 0;
    while !g.projectiles().is_empty() && frames < 400 {
        g.run_frames(1);
        frames += 1;
    }
    // The path is 300 units at 2 per frame: it lasts about 150 frames.
    assert!(frames > 120, "it flies its whole path under the ground ({frames} frames)");
}

#[test]
fn a_shell_landing_away_from_its_victim_does_not_hit_it() {
    // DumbProjectileBehavior::detonate() has no victim argument: the blast is at the shell's own position (:523-539). A
    // victim that moved 20 units off the path end is not the blast's primary target.
    let dp = DumbProjectileParams { first_height: 60.0, second_height: 60.0, physics: false, ..DumbProjectileParams::default() };
    let mut w = weapon_with(shell(dp), 2.0);
    w.attack_range = 2000.0;
    let (mut g, _, b) = duel_at(w, 400.0, 300.0);
    let mut launched = false;
    for _ in 0..30 {
        g.run_frames(1);
        if !g.projectiles().is_empty() {
            launched = true;
            break;
        }
    }
    assert!(launched, "the shell was fired");
    g.set_object_position(b, Coord3D::new(400.0, 320.0, 0.0));
    let mut frames = 0;
    while !g.projectiles().is_empty() && frames < 400 {
        g.run_frames(1);
        frames += 1;
    }
    assert_eq!(g.find_object(b).unwrap().health(), 200.0, "the victim 20 units away is not hit");
}
