//! Locomotor appearances and physics (synthetic data, no game install).

use std::sync::Arc;

use zr_sim::locomotor::{maintain_current_position, move_towards_position};
use zr_sim::physics::PhysicsTemplate;
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams, ZBehavior};
use zr_sim::{Command, Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

fn ground() -> LocomotorParams {
    LocomotorParams::from_engine_units(LocomotorKind::Treads, 2.0, 0.0, 0.1, 0.2, 0.1, true)
}

fn hover() -> LocomotorParams {
    let mut l = LocomotorParams::from_engine_units(LocomotorKind::Hover, 2.0, 0.0, 0.1, 0.2, 0.1, false);
    l.behavior_z = ZBehavior::SurfaceRelativeHeight;
    l.preferred_height = 50.0;
    l.surfaces_air = false;
    l.surfaces_water = true;
    l
}

fn wings() -> LocomotorParams {
    let mut l = LocomotorParams::from_engine_units(LocomotorKind::Wings, 5.0, 2.0, 0.5, 0.1, 0.1, false);
    l.behavior_z = ZBehavior::SurfaceRelativeHeight;
    l.preferred_height = 100.0;
    l.surfaces_air = true;
    l
}

#[test]
fn ground_accelerates_and_brakes() {
    let t = TerrainLogic::flat(200, 200, 0);
    let l = ground();
    let (mut pos, mut ang, mut spd) = (Coord3D::new(50.0, 50.0, 0.0), 0.0, 0.0);
    let goal = Coord3D::new(150.0, 50.0, 0.0);
    // Accelerate: speed rises by the acceleration each frame, capped at max.
    move_towards_position(&l, &mut pos, &mut ang, &mut spd, &goal, &t);
    assert!((spd - l.acceleration).abs() < 1e-6);
    for _ in 0..100 {
        if move_towards_position(&l, &mut pos, &mut ang, &mut spd, &goal, &t) {
            break;
        }
    }
    assert_eq!(spd, 0.0, "stops on arrival");
    assert_eq!(pos.x, goal.x);
    // Maintain: `maintainCurrentPositionTreads` is `maintainCurrentPositionOther`
    // (`Locomotor.h:384`), `physics->scrubVelocity2D(0)` (`Locomotor.cpp:2601-2608`): the unit stops
    // dead in place, it does not coast a braking step forward.
    let (mut pos, mut spd) = (Coord3D::new(10.0, 20.0, 0.0), 2.0);
    maintain_current_position(&l, &mut pos, 0.0, &mut spd, &t);
    assert_eq!(spd, 0.0, "scrubbed to a stop in one call");
    assert_eq!(pos, Coord3D::new(10.0, 20.0, 0.0), "and it does not move while stopping");
}

#[test]
fn turn_rate_is_limited() {
    let t = TerrainLogic::flat(200, 200, 0);
    let l = ground();
    let (mut pos, mut ang, mut spd) = (Coord3D::new(50.0, 50.0, 0.0), 0.0, 0.0);
    let goal = Coord3D::new(50.0, 150.0, 0.0); // 90 degrees left
    move_towards_position(&l, &mut pos, &mut ang, &mut spd, &goal, &t);
    assert!(ang.abs() <= l.max_turn_rate + 1e-6, "turned {} in one frame", ang);
}

#[test]
fn hover_climbs_to_preferred_height() {
    let t = TerrainLogic::flat(200, 200, 0);
    let l = hover();
    let (mut pos, mut ang, mut spd) = (Coord3D::new(50.0, 50.0, 0.0), 0.0, 0.0);
    let goal = Coord3D::new(150.0, 50.0, 0.0);
    for _ in 0..400 {
        move_towards_position(&l, &mut pos, &mut ang, &mut spd, &goal, &t);
    }
    assert!((pos.z - l.preferred_height).abs() < 1.0, "hover height {}", pos.z);
}

#[test]
fn wings_never_drop_below_min_speed() {
    let t = TerrainLogic::flat(500, 500, 0);
    let l = wings();
    let (mut pos, mut ang, mut spd) = (Coord3D::new(50.0, 50.0, 0.0), 0.0, 0.0);
    let goal = Coord3D::new(400.0, 50.0, 0.0);
    for _ in 0..600 {
        move_towards_position(&l, &mut pos, &mut ang, &mut spd, &goal, &t);
        assert!(spd >= l.min_speed - 1e-6, "speed {} below min {}", spd, l.min_speed);
    }
    assert!((pos.z - l.preferred_height).abs() < 1.0, "wings fly at preferred height");
}

#[test]
fn physics_gravity_fall_and_bounce() {
    let mut t = ObjectTemplate::new("Faller");
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.physics = Some(PhysicsTemplate { mass: 1.0, allow_bouncing: false, ..PhysicsTemplate::default() });
    let t = Arc::new(t);
    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
    let id = g.create_object(&t, Coord3D::new(50.0, 50.0, 100.0), 0.0, 0);
    g.apply_force(id, Coord3D::ZERO);
    g.run_frames(300);
    let o = g.find_object(id).unwrap();
    assert!(o.position.z.abs() < 0.01, "fell to the ground: {}", o.position.z);
    assert_eq!(o.physics.as_ref().unwrap().velocity.z, 0.0);

    // Bouncing: the object leaves the ground again after the first hit.
    let mut t = ObjectTemplate::new("Bouncer");
    t.max_health = 100.0;
    t.physics = Some(PhysicsTemplate { mass: 1.0, allow_bouncing: true, ..PhysicsTemplate::default() });
    let t = Arc::new(t);
    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
    let id = g.create_object(&t, Coord3D::new(50.0, 50.0, 100.0), 0.0, 0);
    g.apply_force(id, Coord3D::ZERO);
    let mut bounced = false;
    for _ in 0..300 {
        g.update();
        if g.find_object(id).is_some_and(|o| o.position.z > 0.5) {
            bounced = true;
            break;
        }
    }
    assert!(bounced, "bounced off the ground");
}

#[test]
fn ground_unit_moves_by_motive_force() {
    // A ground locomotor with a `PhysicsBehavior` is driven by motive force: the locomotor applies
    // `mass * acceleration` (`Locomotor::moveTowardsPositionTreads`) and `PhysicsBehavior::update`
    // integrates it into position.
    let mut t = ObjectTemplate::new("Tank");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 5.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.physics = Some(PhysicsTemplate { mass: 10.0, ..PhysicsTemplate::default() });
    t.locomotor = Some(LocomotorParams::from_engine_units(LocomotorKind::Treads, 1.0, 0.0, 0.05, 0.1, 0.05, true));
    let t = Arc::new(t);
    let mut g = GameLogic::new(TerrainLogic::flat(300, 300, 0), 1);
    let id = g.create_object(&t, Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    g.submit(0, Command::MoveTo { ids: vec![id], pos: Coord3D::new(200.0, 50.0, 0.0) });
    g.run_frames(5);
    let o = g.find_object(id).unwrap();
    assert!(o.position.x > 50.0, "the unit moved: x {}", o.position.x);
    assert!(o.physics.as_ref().unwrap().velocity.x > 0.0, "the locomotor drove the physics velocity");
    for _ in 0..900 {
        if !g.find_object(id).unwrap().ai.is_moving() {
            break;
        }
        g.update();
    }
    let o = g.find_object(id).unwrap();
    // The pathfinder's last waypoint is a cell centre, so "arrived" is within a cell of the goal.
    assert!((o.position.x - 200.0).abs() < 15.0, "arrived near the goal: x {}", o.position.x);
    assert!(!o.ai.is_moving(), "the path emptied");
}

#[test]
fn hover_unit_moves_by_motive_force() {
    // `moveTowardsPositionHover` -> `moveTowardsPositionOther` (`Locomotor.cpp:1884`): a hover
    // locomotor with `PhysicsBehavior` is driven by `applyMotiveForce`, not position stepping.
    let mut t = ObjectTemplate::new("Helicopter");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 5.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.physics = Some(PhysicsTemplate { mass: 3.0, ..PhysicsTemplate::default() });
    let mut loco = LocomotorParams::from_engine_units(LocomotorKind::Hover, 4.0, 0.0, 0.2, 0.1, 0.05, false);
    loco.behavior_z = ZBehavior::SurfaceRelativeHeight;
    loco.preferred_height = 50.0;
    loco.lift = 0.1;
    loco.allow_motive_force_while_airborne = true;
    loco.apply_2d_friction_when_airborne = true;
    t.locomotor = Some(loco);
    let t = Arc::new(t);
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    // The real game gravity (the synthetic default is a placeholder); lift must exceed it.
    let mut rules = g.rules();
    rules.gravity = -0.07111112;
    g.set_rules(rules);
    let id = g.create_object(&t, Coord3D::new(50.0, 50.0, 50.0), 0.0, 0);
    g.submit(0, Command::MoveTo { ids: vec![id], pos: Coord3D::new(300.0, 50.0, 0.0) });
    g.run_frames(30);
    let o = g.find_object(id).unwrap();
    assert!(o.position.x > 60.0, "the helicopter flew: x {}", o.position.x);
    let v = o.physics.as_ref().unwrap().velocity;
    assert!(v.x.abs() + v.y.abs() > 0.0, "motive force produced velocity");
    assert!((o.position.z - 50.0).abs() < 5.0, "holds its preferred height: z {}", o.position.z);
}

#[test]
fn wings_unit_moves_by_motive_force() {
    // `moveTowardsPositionWings` -> `moveTowardsPositionOther` (`Locomotor.cpp:1842`): a fixed-wing
    // locomotor is force-driven and never drops below `MinSpeed`.
    let mut t = ObjectTemplate::new("Jet");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 5.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.physics = Some(PhysicsTemplate { mass: 4.0, ..PhysicsTemplate::default() });
    let mut loco = LocomotorParams::from_engine_units(LocomotorKind::Wings, 6.0, 2.0, 0.1, 0.1, 0.05, false);
    loco.behavior_z = ZBehavior::SurfaceRelativeHeight;
    loco.preferred_height = 100.0;
    loco.allow_motive_force_while_airborne = true;
    loco.apply_2d_friction_when_airborne = true;
    t.locomotor = Some(loco);
    let t = Arc::new(t);
    let mut g = GameLogic::new(TerrainLogic::flat(500, 500, 0), 1);
    let id = g.create_object(&t, Coord3D::new(50.0, 50.0, 100.0), 0.0, 0);
    g.submit(0, Command::MoveTo { ids: vec![id], pos: Coord3D::new(400.0, 50.0, 0.0) });
    g.run_frames(40);
    let o = g.find_object(id).unwrap();
    assert!(o.position.x > 80.0, "the jet flew: x {}", o.position.x);
    let v = o.physics.as_ref().unwrap().velocity;
    assert!(v.x.abs() + v.y.abs() > 0.0, "motive force produced velocity");
}

#[test]
fn thrust_unit_moves_by_the_3d_kernel() {
    // `moveTowardsPositionThrust` (`Locomotor.cpp:1912`): a THRUST locomotor with a `PhysicsBehavior`
    // is driven by `calcDirectionToApplyThrust` + the `MaxThrustAngle` clamp.
    let mut t = ObjectTemplate::new("Missile");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 4.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.physics = Some(PhysicsTemplate { mass: 5.0, ..PhysicsTemplate::default() });
    let mut loco = LocomotorParams::from_engine_units(LocomotorKind::Thrust, 2.0, 0.2, 0.2, 0.1, 0.05, false);
    loco.behavior_z = ZBehavior::SurfaceRelativeHeight;
    loco.preferred_height = 50.0;
    loco.max_thrust_angle = 0.3;
    // A THRUST unit flying above the airborne threshold only gets a motive force with
    // `AllowAirborneMotiveForce` (`Locomotor.cpp:1076`: the kernel is skipped when `treatAsAirborne`
    // and the flag is off). Real missile/aircraft locomotors set it; without it the unit could only
    // ever move while `doLocomotor` had no path, which the engine leaves to the AI state.
    loco.allow_motive_force_while_airborne = true;
    t.locomotor = Some(loco);
    let t = Arc::new(t);
    let mut g = GameLogic::new(TerrainLogic::flat(300, 300, 0), 1);
    let id = g.create_object(&t, Coord3D::new(50.0, 50.0, 50.0), 0.0, 0);
    g.submit(0, Command::MoveTo { ids: vec![id], pos: Coord3D::new(200.0, 120.0, 50.0) });
    g.run_frames(30);
    let o = g.find_object(id).unwrap();
    assert!(o.position.x > 50.0, "the missile flew: x {}", o.position.x);
    let v = o.physics.as_ref().unwrap().velocity;
    assert!(v.x.abs() + v.y.abs() + v.z.abs() > 0.0, "motive force produced velocity");
}

#[test]
fn angular_rates_rotate_the_object() {
    // `PhysicsBehavior::update`'s `HAS_PITCHROLLYAW` block (`PhysicsUpdate.cpp:700`): the stored
    // yaw/pitch/roll rates are integrated into the object's orientation each frame. Previously only
    // yaw was applied; pitch/roll were ignored.
    let mut t = ObjectTemplate::new("Spinner");
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.physics = Some(PhysicsTemplate { mass: 1.0, ..PhysicsTemplate::default() });
    let t = Arc::new(t);
    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
    let id = g.create_object(&t, Coord3D::new(50.0, 50.0, 50.0), 0.0, 0);
    g.set_allow_to_fall(id, true);
    g.set_angular_rates(id, 0.1, 0.2, 0.3);
    // A `PhysicsBehavior`'s first update is one frame after creation (`PhysicsUpdate.cpp:231-233`).
    g.update();
    g.update();
    let o = g.find_object(id).unwrap();
    let p = o.physics.as_ref().unwrap();
    assert!(p.yaw_rate != 0.0 && p.roll_rate != 0.0 && p.pitch_rate != 0.0);
    assert!(o.angle != 0.0, "yaw applied: {}", o.angle);
    assert!(p.pitch != 0.0, "pitch applied: {}", p.pitch);
    assert!(p.roll != 0.0, "roll applied: {}", p.roll);
}

#[test]
fn kill_when_resting_on_ground() {
    let mut t = ObjectTemplate::new("Rock");
    t.max_health = 100.0;
    t.physics = Some(PhysicsTemplate { mass: 1.0, kill_when_resting_on_ground: true, ..PhysicsTemplate::default() });
    let t = Arc::new(t);
    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
    let id = g.create_object(&t, Coord3D::new(50.0, 50.0, 50.0), 0.0, 0);
    g.apply_force(id, Coord3D::ZERO);
    g.run_frames(300);
    g.update();
    assert!(g.find_object(id).is_none(), "killed when resting on the ground");
}
