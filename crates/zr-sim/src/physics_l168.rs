//! Lane 168 (wave 19, the R18 review, R18-7): the physics writes the heading into the object's transform.
//! `setAngles` builds the matrix (`PhysicsUpdate.cpp:1074-1086`), and the yaw integration post-multiplies the
//! object's matrix by `Rotate_Z` and calls `setTransformMatrix` (`:749`, `:814`). A later pivot turn reads the matrix.

use super::*;
use crate::template::{kind_of, ObjectTemplate};
use crate::terrain::TerrainLogic;
use std::sync::Arc;

fn body() -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new("Body");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.geometry.major_radius = 5.0;
    t.physics = Some(PhysicsTemplate { mass: 10.0, ..PhysicsTemplate::default() });
    Arc::new(t)
}

/// R18-7 (`PhysicsUpdate.cpp:1074-1086`): `setAngles` builds the matrix at the object's position, pre-rotated
/// `X(-roll)`, `Y(pitch)`, `Z(yaw)`, and `setTransformMatrix` takes it, so the heading is its `Get_Z_Rotation`. Before the
/// fix only the angle was written and the transform kept the old heading. Revert check: write `o.angle` alone.
#[test]
fn set_angles_writes_the_heading_into_the_transform() {
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 0);
    let id = g.create_object(&body(), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    g.set_angles(id, 1.0, 0.0, 0.0);
    let o = g.find_object(id).unwrap();
    assert!((o.transform.z_rotation() - 1.0).abs() < 1e-5, "the matrix turned: {}", o.transform.z_rotation());
    assert_eq!(o.angle.to_bits(), o.transform.z_rotation().to_bits(), "the heading is the matrix readback");
    assert_eq!(o.position, o.transform.translation(), "the centre is the matrix translation");
}

/// R18-7 (`PhysicsUpdate.cpp:749`, `:814`): the yaw rate turns the object's matrix (`Rotate_Z`), and the heading is its
/// readback. Before the fix the integration wrote only `o.angle`, so a pivot turn after a yaw read the old matrix and
/// snapped back. Revert check: write `o.angle = normalize(o.angle + yaw)` again in `update_object_physics`.
#[test]
fn a_yaw_rate_turns_the_matrix_and_the_heading_reads_it_back() {
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 0);
    let id = g.create_object(&body(), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    g.set_angular_rates(id, 0.1, 0.0, 0.0);
    g.physics_update(id, 1);
    let o = g.find_object(id).unwrap();
    assert!(o.angle > 0.05, "the yaw rate turned the object: {}", o.angle);
    assert_eq!(o.angle.to_bits(), o.transform.z_rotation().to_bits(), "the heading is the matrix readback");
}
