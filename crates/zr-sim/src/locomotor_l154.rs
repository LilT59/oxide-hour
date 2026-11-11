//! Lane 154 (wave 18): `moveTowardsPositionTreads` reads its forward speed after the body has turned.
//! The unit tests of the treads kernel (`locomotor.rs`, `damage_and_set_tests`) do not cover the turn.
//! `Locomotor::moveTowardsPositionTreads` (`Locomotor.cpp:1185`) calls `rotateTowardsPosition` first, and
//! `Real actualSpeed = physics->getForwardSpeed2D()` (`:1207`) then projects the velocity on the turned
//! heading, because `setTransformMatrix` rebuilds `getUnitDirectionVector2D`. The port read it before the turn.

use super::*;
use crate::physics::PhysicsTemplate;
use crate::template::{kind_of, LocomotorKind, LocomotorParams, ObjectTemplate};
use std::sync::Arc;

#[test]
fn treads_forward_speed_is_projected_on_the_turned_heading() {
    let mut t = ObjectTemplate::new("Tread");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 5.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.physics = Some(PhysicsTemplate { mass: 10.0, ..PhysicsTemplate::default() });
    // max speed 1.0, acceleration 0.1, braking 0.5, turn rate 0.5 rad/frame (all per frame).
    t.locomotor = Some(LocomotorParams::from_engine_units(LocomotorKind::Treads, 1.0, 0.0, 0.1, 0.5, 0.5, true));
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    let id = g.create_object(&Arc::new(t), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    let loco = g.objects[&id].locomotor().unwrap();
    let o = g.objects.get_mut(&id).unwrap();
    o.physics.as_mut().unwrap().velocity = Coord3D::new(0.6, 0.0, 0.0);
    // The goal is PI/8 to the left of the heading (0): relAngle = PI/8, angleCoeff = 0.5, and the turn
    // (0.39) stays under the turn rate (0.5), so the body ends at heading PI/8. It is 100 units away, so
    // neither the "close to the goal" speed cut (`dist < 2 cells`) nor the braking test applies.
    let goal = Coord3D::new(50.0 + 100.0 * (PI / 8.0).cos(), 50.0 + 100.0 * (PI / 8.0).sin(), 0.0);
    move_towards_position_treads(o, &loco, &goal, 100.0, 1.0, 10);
    assert!((o.angle - PI / 8.0).abs() < 1e-5, "the body turned to PI/8: {}", o.angle);
    // goalSpeed = (1 - 0.5) * 1.0 = 0.5. actualSpeed is the velocity's projection on the NEW heading:
    // 0.6 * cos(PI/8) = 0.5543 (not 0.6, the old heading's). speedDelta = 0.5 - 0.5543; the braking
    // acceleration (0.5) is larger, so the force is clamped to the speed delta (`:1264-1270`).
    let expected = (0.5 - 0.6 * (PI / 8.0).cos()).abs();
    let a = o.physics.as_ref().unwrap().accel;
    let mag = (a.x * a.x + a.y * a.y).sqrt();
    assert!((mag - expected).abs() < 1e-4, "motive decel {mag} vs expected {expected} (0.1 would be the old heading's)");
    // The force is along the turned heading, opposing the motion: accel = -expected * (cos, sin)(PI/8).
    assert!((a.x + expected * (PI / 8.0).cos()).abs() < 1e-4, "accel x {}", a.x);
    assert!((a.y + expected * (PI / 8.0).sin()).abs() < 1e-4, "accel y {}", a.y);
}

#[test]
fn a_blocked_treads_unit_scrubs_to_the_blocked_speed_with_no_motive_force() {
    // `locoUpdate_moveTowardsPosition` (`Locomotor.cpp:1040-1058`): `if (*blocked)` scrubs the velocity to
    // `desiredSpeed` and returns before the kernel, so no motive force is applied this frame.
    let mut t = ObjectTemplate::new("Tread");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 5.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.physics = Some(PhysicsTemplate { mass: 10.0, ..PhysicsTemplate::default() });
    t.locomotor = Some(LocomotorParams::from_engine_units(LocomotorKind::Treads, 1.0, 0.0, 0.1, 0.5, 0.5, true));
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    let id = g.create_object(&Arc::new(t), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    let loco = g.objects[&id].locomotor().unwrap();
    let terrain = TerrainLogic::flat(400, 400, 0);
    let goal = Coord3D::new(150.0, 50.0, 0.0);
    {
        let o = g.objects.get_mut(&id).unwrap();
        o.physics.as_mut().unwrap().velocity = Coord3D::new(1.0, 0.0, 0.0);
        assert!(!move_towards_position_force_blocked(o, &loco, &goal, &terrain, &g.pathfinder, 10, 0.2, -1.0, Some(100.0), true));
        let p = o.physics.as_ref().unwrap();
        assert!((p.velocity_magnitude() - 0.2).abs() < 1e-5, "scrubbed to the blocked speed: {}", p.velocity_magnitude());
        assert_eq!(p.accel, Coord3D::ZERO, "no motive force while blocked");
    }
    // Not blocked: the same unit gets the braking force toward the desired speed (and keeps its velocity).
    {
        let o = g.objects.get_mut(&id).unwrap();
        o.physics.as_mut().unwrap().velocity = Coord3D::new(1.0, 0.0, 0.0);
        move_towards_position_force_blocked(o, &loco, &goal, &terrain, &g.pathfinder, 11, 0.2, -1.0, Some(100.0), false);
        let p = o.physics.as_ref().unwrap();
        assert!((p.velocity.x - 1.0).abs() < 1e-6, "the kernel does not change the velocity itself");
        assert!(p.accel.x < -0.01, "braking force along the heading: {}", p.accel.x);
    }
}
