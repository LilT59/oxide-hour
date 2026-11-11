//! Wave 19 wrap-up: `locoUpdate_moveTowardsPosition` has no arrival step. AN1 s1 Red Guard 241: the port's old block
//! cleared `IS_BRAKING` near the goal, and the unit then walked around the goal under motive force where the C++ stays
//! pinned on it (`Locomotor.cpp:1117-1160`). AH7's first meaningful divergence moved from f2387 to f2657 with this.

use super::*;
use crate::physics::PhysicsTemplate;
use crate::template::{kind_of, ObjectTemplate};
use std::sync::Arc;

fn walker() -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new("Walker");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.geometry.major_radius = 5.0;
    t.physics = Some(PhysicsTemplate { mass: 10.0, ..PhysicsTemplate::default() });
    t.locomotor = Some(LocomotorParams::from_engine_units(LocomotorKind::Treads, 1.0, 0.0, 0.2, 0.0556, 0.1, true));
    Arc::new(t)
}

/// Revert check: restore the block that snaps the position, zeroes the velocity and clears `p.braking` when the unit is
/// near and slow.
#[test]
fn a_braking_unit_near_its_goal_keeps_braking() {
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 0);
    let id = g.create_object(&walker(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let goal = Coord3D::new(100.4, 100.0, 0.0);
    let terrain = TerrainLogic::flat(400, 400, 0);
    let o = g.objects.get_mut(&id).unwrap();
    let loco = o.template.locomotor.unwrap();
    let p = o.physics.as_mut().unwrap();
    p.braking = true;
    p.velocity = Coord3D::new(0.01, 0.0, 0.0);
    move_towards_position_force(o, &loco, &goal, &terrain, &g.pathfinder, 10, 1.0, -1.0, Some(0.4));
    assert!(o.physics.as_ref().unwrap().braking, "`IS_BRAKING` clears only on a far goal (`Locomotor.cpp:963-967`)");
}
