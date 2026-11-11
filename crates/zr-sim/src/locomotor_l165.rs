//! Lane 165 (wave 19): `Locomotor::locoUpdate_moveTowardsPosition` returns at once for a stunned unit
//! (`Locomotor.cpp:976-978`, `if(physics->getIsStunned()) return;`), after the far-goal braking reset (`:962-967`) and
//! before the distance and the motive force (`:1006`, `:1034`).

use super::*;
use crate::physics::PhysicsTemplate;
use crate::template::{kind_of, LocomotorKind, LocomotorParams, ObjectTemplate};
use std::sync::Arc;

fn tread() -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new("Tread");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 5.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.physics = Some(PhysicsTemplate { mass: 10.0, ..PhysicsTemplate::default() });
    t.locomotor = Some(LocomotorParams::from_engine_units(LocomotorKind::Treads, 1.0, 0.0, 0.1, 0.5, 0.5, true));
    Arc::new(t)
}

/// A stationary treaded unit at (50, 50) with a goal 100 units ahead, after one kernel call. Returns the physics
/// `accel` the call left behind (the motive force a moving unit applies).
fn accel_after_kernel(stunned: bool) -> Coord3D {
    let t = tread();
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    let id = g.create_object(&t, Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    let loco = g.objects[&id].locomotor().unwrap();
    let terrain = TerrainLogic::flat(400, 400, 0);
    let goal = Coord3D::new(150.0, 50.0, 0.0);
    let o = g.objects.get_mut(&id).unwrap();
    o.physics.as_mut().unwrap().is_stunned = stunned;
    let mut blocked = false;
    move_towards_position_force_blocked_out(o, &loco, &goal, &terrain, &g.pathfinder, 10, 1.0, -1.0, Some(100.0), &mut blocked);
    o.physics.as_ref().unwrap().accel
}

/// `if(physics->getIsStunned()) return;` (`Locomotor.cpp:976-978`): a stunned unit gets no motive force from its kernel,
/// while an unstunned unit with the same goal does (the control). Revert check: with the early return removed the stunned
/// unit's `accel` is the same non-zero force as the control, and the first assertion fails.
#[test]
fn a_stunned_unit_gets_no_motive_force_from_its_locomotor_kernel() {
    let control = accel_after_kernel(false);
    assert!(control.x.abs() > 1e-6, "the control unit is driven toward its goal: {control:?}");
    let stunned = accel_after_kernel(true);
    assert_eq!(stunned, Coord3D::ZERO, "a stunned unit's kernel returns before the motive force");
}
