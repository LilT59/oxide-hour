//! Lane 168 (wave 19, the R18 review): a 3D turn writes its heading into the object's transform (R18-7,
//! `Locomotor.cpp:171-185`), and `handleBehaviorZ` runs after the kernel of every locomotor kind (R18-12,
//! `Locomotor.cpp:1109`, `:2217-2236`).

use super::*;
use crate::physics::PhysicsTemplate;
use crate::template::{kind_of, ObjectTemplate};
use std::sync::Arc;

fn flier() -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new("Flier");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.geometry.major_radius = 5.0;
    t.physics = Some(PhysicsTemplate { mass: 10.0, ..PhysicsTemplate::default() });
    t.locomotor = Some(LocomotorParams::from_engine_units(LocomotorKind::Wings, 1.0, 0.0, 0.2, 0.05, 0.1, false));
    Arc::new(t)
}

/// R18-7 (`Locomotor.cpp:171-185`): a 3D turn builds the new matrix from the object's position and the turned direction
/// (`buildTransformMatrix`, then `setTransformMatrix`), so the heading is its readback. Before the fix only `obj.angle`
/// was written. Revert check: write `obj.angle = normalize(atan2(actual.y, actual.x))` alone, as before.
#[test]
fn a_3d_turn_writes_its_heading_into_the_transform() {
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 0);
    let id = g.create_object(&flier(), Coord3D::new(50.0, 50.0, 20.0), 0.0, 0);
    let o = g.objects.get_mut(&id).unwrap();
    let rel = try_orient_in_this_direction_3d(o, 0.1, Coord3D::new(0.0, 1.0, 0.0));
    assert!((rel - 0.1).abs() < 1e-6, "the turn used its full rate: {rel}");
    assert!((o.angle - 0.1).abs() < 1e-5, "turned toward +y: {}", o.angle);
    assert_eq!(o.angle.to_bits(), o.transform.z_rotation().to_bits(), "the heading is the matrix readback");
    let x = o.transform.x_vector();
    assert!((x.x - 0.1f32.cos()).abs() < 1e-5 && (x.y - 0.1f32.sin()).abs() < 1e-5, "the X axis points where the turn went: {x:?}");
    assert_eq!(o.position, o.transform.translation(), "the centre is the matrix translation");
}

/// R18-12 (`Locomotor.cpp:1109`, `:2217-2236`): `handleBehaviorZ` runs after the kernel of every locomotor kind, not only
/// hover and wings. A treads unit with `Z_FIXED_ABSOLUTE_HEIGHT` is held at its preferred height by its move. Revert
/// check: gate the call after the kernel on `LocomotorKind::Hover | LocomotorKind::Wings` again.
#[test]
fn a_treads_move_runs_handle_behavior_z_after_its_kernel() {
    let mut loco = LocomotorParams::from_engine_units(LocomotorKind::Treads, 1.0, 0.0, 0.2, 0.1, 0.1, true);
    loco.behavior_z = ZBehavior::FixedAbsoluteHeight;
    loco.preferred_height = 50.0;
    let mut t = ObjectTemplate::new("Treads");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.geometry.major_radius = 5.0;
    t.physics = Some(PhysicsTemplate { mass: 10.0, ..PhysicsTemplate::default() });
    t.locomotor = Some(loco);
    let t = Arc::new(t);
    let terrain = TerrainLogic::flat(400, 400, 0);
    let mut g = GameLogic::new(terrain.clone(), 1);
    g.add_player(0, 0);
    let id = g.create_object(&t, Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    let goal = Coord3D::new(50.0, 80.0, 0.0);
    {
        let o = g.objects.get_mut(&id).unwrap();
        move_towards_position_force(o, &loco, &goal, &terrain, &g.pathfinder, 0, 1.0, -1.0, None);
    }
    assert_eq!(g.objects[&id].position.z, 50.0, "handleBehaviorZ held the treads at its preferred height");
}
