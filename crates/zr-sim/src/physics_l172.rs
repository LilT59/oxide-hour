//! Lane 172: the rest of `PhysicsBehavior::update`'s transform handling (`PhysicsUpdate.cpp:700-815`).
//!
//! The angular rates post-multiply the object's matrix (`mtx.Rotate_X(roll)`, `Rotate_Y(pitch)`,
//! `Rotate_Z(yaw)`, `:747-749`), so pitch and roll turn the matrix — and therefore feed back into the
//! heading `Get_Z_Rotation` reads and into the X vector `getForwardSpeed3D` / `tryToOrientInThisDirection3D`
//! read. A bounce then re-rights the object with `setAngles(yaw, 0, 0)` (`:799-811`), the yaw taken from the
//! object's own matrix *before* this frame's rotation, and the translation from the integrated matrix.

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
    // A huge `MinFallSpeedForDamage` keeps the falling-damage path (`PhysicsUpdate.cpp:836-884`) out of the
    // bounce tests: they drop the object 20 units in one frame.
    t.physics = Some(PhysicsTemplate { mass: 10.0, min_fall_speed_for_damage: 1.0e9, ..PhysicsTemplate::default() });
    Arc::new(t)
}

/// `PhysicsUpdate.cpp:747-748`: `mtx.Rotate_Y(pitchRateToUse)` post-multiplies the object's matrix, so a
/// pitch rate tilts the X vector even with no yaw. The rate is scaled by `PitchRollYawFactor` (2.0 here).
/// Revert check: accumulate `p.pitch` without touching the matrix; the X vector stays level.
#[test]
fn a_pitch_rate_tilts_the_object_matrix() {
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 0);
    let id = g.create_object(&body(), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    g.set_angular_rates(id, 0.0, 0.0, 0.1);
    g.physics_update(id, 1);
    let o = g.find_object(id).unwrap();
    let x = o.transform.x_vector();
    // `applyFrictionalForces` damps the rates by `1 - DEFAULT_LATERAL_FRICTION` first (`PhysicsUpdate.cpp:645-646`,
    // `:461`): 0.1 * 0.85 = 0.085, then `PitchRollYawFactor` 2.0 gives 0.17. `Rotate_Y` leaves the X vector
    // at `(cos, 0, -sin)`.
    let rate = 0.1f32 * (1.0 - DEFAULT_LATERAL_FRICTION) * 2.0;
    assert!((x.z + rate.sin()).abs() < 1e-6, "the pitch rate tilted the X vector: {} vs {}", x.z, -rate.sin());
    assert_eq!(o.transform.z_rotation(), 0.0, "a pure pitch leaves the heading at 0");
    // The port's mirror of the matrix pitch (`Physics::pitch`) follows the same rate.
    assert!((o.physics.as_ref().unwrap().pitch - rate).abs() < 1e-6);
}

/// `PhysicsUpdate.cpp:747-749`: the three rotations post-multiply in X, Y, Z order, so with a pitch and a
/// yaw rate together the matrix's `Get_Z_Rotation` is not the bare yaw: `Rotate_Y` moved column 0 before
/// `Rotate_Z` turned it. Revert check: rotate only Z (the old `rotate_z_angle(yaw_delta)`); the heading is
/// then exactly the yaw delta.
#[test]
fn a_pitch_rate_offsets_the_heading_the_matrix_reports() {
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 0);
    let id = g.create_object(&body(), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    g.set_angular_rates(id, 0.15, 0.0, 0.2);
    g.physics_update(id, 1);
    let o = g.find_object(id).unwrap();
    // The rates after the friction damping and `PitchRollYawFactor`.
    let d = 1.0 - DEFAULT_LATERAL_FRICTION;
    let (yaw, pitch) = (0.15 * d * 2.0, 0.2 * d * 2.0);
    let mut mtx = crate::transform::Matrix3D::from_orientation(0.0, Coord3D::new(50.0, 50.0, 0.0));
    mtx.rotate_x(sin(0.0), cos(0.0));
    mtx.rotate_y(sin(pitch), cos(pitch));
    mtx.rotate_z(sin(yaw), cos(yaw));
    assert_eq!(o.transform.row[0][0].to_bits(), mtx.row[0][0].to_bits(), "Rotate_X/Y/Z in the engine's order");
    assert_eq!(o.angle.to_bits(), mtx.z_rotation().to_bits(), "the heading is the matrix readback");
    assert_ne!(o.angle, yaw, "not the bare yaw delta: the pitch turned column 0 before Rotate_Z");
}

/// `PhysicsUpdate.cpp:799-811`: on a bounce the object is re-righted with `setAngles(yaw, 0, 0)`, where the
/// yaw is read from the object's own matrix — still last frame's, because the integrated `mtx` has not been
/// written yet — and the translation comes from the integrated matrix. This frame's yaw rate and pitch are
/// therefore discarded. Revert check: keep the integrated matrix (`mtx`) on the bounce path; the heading
/// becomes 0.2 and the X vector keeps its pitch.
#[test]
fn a_bounce_re_rights_the_object_to_last_frames_yaw() {
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 0);
    let id = g.create_object(&body(), Coord3D::new(50.0, 50.0, 20.0), 0.0, 0);
    g.set_allow_bouncing(id, true);
    g.add_velocity_to(id, Coord3D::new(0.0, 0.0, -25.0));
    g.set_angular_rates(id, 0.1, 0.05, 0.1);
    g.physics_update(id, 1);
    let o = g.find_object(id).unwrap();
    assert_eq!(o.position.z, 0.0, "the ground slam put it on the ground");
    assert_eq!(o.transform.z_rotation(), 0.0, "the heading is last frame's yaw (0), not this frame's 0.2");
    assert_eq!(o.transform.x_vector().z, 0.0, "pitch and roll are cleared by the re-right");
    let p = o.physics.as_ref().unwrap();
    assert_eq!(p.pitch, 0.0, "the port's pitch mirror is cleared with the matrix");
    assert_eq!(p.roll, 0.0, "the port's roll mirror is cleared with the matrix");
    // The bounce force itself still went into the acceleration for the next frame (`applyForce`,
    // `:825-828`; `applyForce` adds `F / m` to `m_accel`, it does not touch the velocity).
    assert!(p.accel.z > 0.0, "the bounce force is in the acceleration: {}", p.accel.z);
}

/// `handleBounce`'s `if (vz < 0) setAngles(yaw, 0, rollAngle)` (`PhysicsUpdate.cpp:526-532`) leaves no trace:
/// `update()` overwrites the object's matrix in the same call — with the re-right's matrix when
/// `bounceForce.z > 0` (`:799-811`) and with `mtx` otherwise (`:814`) — and `setAngles` pre-rotates, so
/// `Get_Z_Rotation()` reads the same yaw before and after the flip. This pins that: a bouncer already on the
/// ground (bounce force 0) keeps the pitch its matrix carried, as the engine does.
#[test]
fn the_handle_bounce_flip_is_overwritten_by_the_matrix_write() {
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 0);
    let id = g.create_object(&body(), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    g.set_angles(id, 0.0, 0.5, 0.0);
    assert!((g.find_object(id).unwrap().transform.x_vector().z + sin(0.5)).abs() < 1e-6, "tilted to start");
    g.set_allow_bouncing(id, true);
    g.add_velocity_to(id, Coord3D::new(0.0, 0.0, -1.0));
    g.physics_update(id, 1);
    let o = g.find_object(id).unwrap();
    assert_eq!(o.transform.x_vector().z.to_bits(), (-sin(0.5)).to_bits(), "the pitch survives: the flip is dead code");
    assert_eq!(o.transform.row[2][2].to_bits(), cos(0.5).to_bits(), "the Z vector keeps its tilt");
}
