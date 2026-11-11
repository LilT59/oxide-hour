//! Lane 171: `PhysicsBehavior::applyFrictionalForces` hands its friction to `applyForce`
//! (`PhysicsUpdate.cpp:446-496`), so the force is built as `mass * friction * velocity`, divided by
//! the mass again inside `applyForce` and, while `isMotive()`, projected onto the lateral axis a
//! second time. The port used to add the friction *acceleration* directly, which rounds differently
//! (one ulp on about a third of inputs) and is the drift this test pins down. Synthetic data only.

use std::sync::Arc;

use zr_sim::math::{cos, sin};
use zr_sim::physics::PhysicsTemplate;
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::{Coord3D, GameLogic, ObjectId, ObjectTemplate, TerrainLogic};

fn flat() -> TerrainLogic {
    TerrainLogic::flat(200, 200, 0)
}

fn truck(mass: f32, lateral_friction: f32, forward_friction: f32) -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Truck");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 8.0;
    t.max_health = 300.0;
    t.initial_health = 300.0;
    let p = PhysicsTemplate { mass, lateral_friction, forward_friction, ..PhysicsTemplate::default() };
    t.physics = Some(p);
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Wheels, 30.0, 60.0, 120.0, 120.0));
    t
}

fn velocity(g: &GameLogic, id: ObjectId) -> Coord3D {
    g.objects().find(|o| o.id == id).and_then(|o| o.physics.as_ref()).map(|p| p.velocity).expect("physics")
}

/// The friction acceleration as `PhysicsUpdate.cpp:446-496` computes it: a force of `mass * friction`
/// times the velocity component, through `applyForce` (`:322-343`, `F * (1/mass)`).
fn cpp_friction_accel(vel: Coord3D, angle: f32, mass: f32, lateral_friction: f32, forward_friction: f32) -> (f32, f32) {
    let (dx, dy) = (cos(angle), sin(angle));
    let lateral = vel.x * (-dy) + vel.y * dx;
    let lateral_x = lateral * -dy;
    let lateral_y = lateral * dx;
    let lf = mass * lateral_friction;
    let mut force_x = -(lf * lateral_x);
    let mut force_y = -(lf * lateral_y);
    // Not motive: the forward friction term is included too.
    let forward = vel.x * dx + vel.y * dy;
    let ff = mass * forward_friction;
    force_x += -(ff * (forward * dx));
    force_y += -(ff * (forward * dy));
    let mass_inv = 1.0f32 / mass;
    (force_x * mass_inv, force_y * mass_inv)
}

/// The old port's form: `-friction * velocity`, added as an acceleration, and no second lateral
/// projection. Used to show that this test tells the two apart.
fn simplified_friction_accel(vel: Coord3D, angle: f32, lateral_friction: f32, forward_friction: f32) -> (f32, f32) {
    let (dx, dy) = (cos(angle), sin(angle));
    let lateral = vel.x * (-dy) + vel.y * dx;
    let lateral_x = lateral * -dy;
    let lateral_y = lateral * dx;
    let mut ax = -(lateral_friction * lateral_x);
    let mut ay = -(lateral_friction * lateral_y);
    let forward = vel.x * dx + vel.y * dy;
    ax += -(forward_friction * (forward * dx));
    ay += -(forward_friction * (forward * dy));
    (ax, ay)
}

#[test]
fn the_friction_force_goes_through_apply_force() {
    let (mass, lateral_friction, forward_friction) = (1000.0f32, 0.11f32, 0.02f32);
    let angle = f32::from_bits(0xbf5709c3);
    let start = Coord3D::new(f32::from_bits(0x3e60c6f3), f32::from_bits(0x3fe2e836), 0.0);
    let mut g = GameLogic::new(flat(), 1);
    let id = g.create_object(&Arc::new(truck(mass, lateral_friction, forward_friction)), Coord3D::new(100.0, 100.0, 0.0), angle, 0);
    g.add_velocity_to(id, start);
    g.run_frames(2);
    let got = velocity(&g, id);

    // The physics module's first update is one frame after creation, so two frames are one update.
    let (ax, ay) = cpp_friction_accel(start, angle, mass, lateral_friction, forward_friction);
    let (want_x, want_y) = (start.x + ax, start.y + ay);
    assert_eq!(got.x.to_bits(), want_x.to_bits(), "vx {} vs {}", got.x, want_x);
    assert_eq!(got.y.to_bits(), want_y.to_bits(), "vy {} vs {}", got.y, want_y);

    // The simplified form this test replaced gives a different float for the same state.
    let (sx, sy) = simplified_friction_accel(start, angle, lateral_friction, forward_friction);
    let (old_x, old_y) = (start.x + sx, start.y + sy);
    assert_ne!(want_x.to_bits(), old_x.to_bits(), "the vector does not discriminate x");
    assert_ne!(want_y.to_bits(), old_y.to_bits(), "the vector does not discriminate y");
}
