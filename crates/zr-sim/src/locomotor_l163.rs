//! Lane 163 (wave 19), adversarial review R17 of dde889c: the blocked branch of the movement kernel.
//! `Locomotor::locoUpdate_moveTowardsPosition` (`Locomotor.cpp:1040-1061`):
//! - R17-2: `*blocked = (TURN_NONE != rotateObjAroundLocoPivot(...))` (`:1056`) is the kernel's output, read back by
//!   `doLocomotor` for the blocked-frame clamp (`AIUpdate.cpp:2290`).
//! - R17-3: `handleBehaviorZ(obj, physics, goalPos)` runs before the blocked return (`:1059-1061`), so a blocked
//!   hover or wing unit keeps its height.

use super::*;
use crate::physics::PhysicsTemplate;
use crate::template::{kind_of, LocomotorKind, LocomotorParams, ObjectTemplate, ZBehavior};
use std::sync::Arc;

fn tread(loco: LocomotorParams) -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new("Tread");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 5.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.physics = Some(PhysicsTemplate { mass: 10.0, ..PhysicsTemplate::default() });
    t.locomotor = Some(loco);
    Arc::new(t)
}

#[test]
fn a_blocked_treads_unit_facing_its_goal_is_no_longer_blocked_after_the_kernel() {
    // `*blocked = (TURN_NONE != rotateObjAroundLocoPivot(...))` (`Locomotor.cpp:1056`): the goal is straight ahead,
    // so the turn is TURN_NONE and the kernel hands `doLocomotor` FALSE. The unit is still blocked at its
    // velocity (1.0 is not below the desired 0.2, `:1040`), so the scrub and the turn run.
    let t = tread(LocomotorParams::from_engine_units(LocomotorKind::Treads, 1.0, 0.0, 0.1, 0.5, 0.5, true));
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    let id = g.create_object(&t, Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    let loco = g.objects[&id].locomotor().unwrap();
    let terrain = TerrainLogic::flat(400, 400, 0);
    let goal = Coord3D::new(150.0, 50.0, 0.0);
    let o = g.objects.get_mut(&id).unwrap();
    o.physics.as_mut().unwrap().velocity = Coord3D::new(1.0, 0.0, 0.0);
    let mut blocked = true;
    move_towards_position_force_blocked_out(o, &loco, &goal, &terrain, &g.pathfinder, 10, 0.2, -1.0, Some(100.0), &mut blocked);
    assert!(!blocked, "a unit that turned to its goal this frame is not blocked any more (TURN_NONE)");
    assert!((o.physics.as_ref().unwrap().velocity_magnitude() - 0.2).abs() < 1e-5, "the blocked branch still scrubs");
}

#[test]
fn a_blocked_treads_unit_that_must_turn_stays_blocked_after_the_kernel() {
    // The same unit facing away from the goal: the turn is TURN_POSITIVE or TURN_NEGATIVE, so `*blocked` stays TRUE.
    let t = tread(LocomotorParams::from_engine_units(LocomotorKind::Treads, 1.0, 0.0, 0.1, 0.5, 0.5, true));
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    let id = g.create_object(&t, Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    let loco = g.objects[&id].locomotor().unwrap();
    let terrain = TerrainLogic::flat(400, 400, 0);
    let goal = Coord3D::new(150.0, 50.0, 0.0);
    let o = g.objects.get_mut(&id).unwrap();
    o.angle = PI;
    o.physics.as_mut().unwrap().velocity = Coord3D::new(-1.0, 0.0, 0.0);
    let mut blocked = true;
    move_towards_position_force_blocked_out(o, &loco, &goal, &terrain, &g.pathfinder, 10, 0.2, -1.0, Some(100.0), &mut blocked);
    assert!(blocked, "a unit still turning toward its goal stays blocked (TURN_POSITIVE/NEGATIVE)");
}

#[test]
fn a_blocked_hover_keeps_its_height_on_the_blocked_branch() {
    // `handleBehaviorZ(obj, physics, goalPos)` runs on every path of the blocked branch (`Locomotor.cpp:1059-1061`).
    // A hover at z 5 with `AbsoluteHeight` 10 gets its lift even though it is blocked and returns before the
    // kernel's motive force. Revert check: drop the Z call from the blocked branch and `accel.z` stays zero.
    let mut hover = LocomotorParams::from_engine_units(LocomotorKind::Hover, 1.0, 0.0, 0.2, 0.1, 0.1, true);
    hover.preferred_height = 10.0;
    hover.behavior_z = ZBehavior::AbsoluteHeight;
    hover.lift = 0.5; // `calcLiftToUseAtPt` clamps the lift to the gross lift (`Locomotor.cpp:2043`); 0 gives no force.
    let t = tread(hover);
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    let id = g.create_object(&t, Coord3D::new(50.0, 50.0, 5.0), 0.0, 0);
    let loco = g.objects[&id].locomotor().unwrap();
    let terrain = TerrainLogic::flat(400, 400, 0);
    let goal = Coord3D::new(150.0, 50.0, 5.0);
    let o = g.objects.get_mut(&id).unwrap();
    assert!(!move_towards_position_force_blocked(o, &loco, &goal, &terrain, &g.pathfinder, 10, 0.0, -1.0, Some(100.0), true));
    let a = o.physics.as_ref().unwrap().accel;
    assert!(a.z > 0.0, "the blocked hover is lifted toward its preferred height: accel.z {}", a.z);
}
