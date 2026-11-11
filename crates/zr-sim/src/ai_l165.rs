//! Lane 165 (wave 19), task 2(a): the AI module's sleepy-update gate (`GameLogic.cpp:3851-3885`, RETAIL_COMPATIBLE_CRC
//! branch `!dis.any() || dis.anyIntersectionWith(getDisabledTypesToProcess())`, `DISABLED_HELD` from AIUpdate.h:305).
//! A unit with a disabled type other than HELD is not updated at all: no state machine, no `doLocomotor`. A HELD unit
//! that is also disabled by EMP is updated, and its `doLocomotor` is not gated by EMP (`AIUpdate.cpp:2127-2301` has no
//! disabled check; the turret block at `:1082` is the only one).

use super::*;
use crate::ai_states::CommandSource;
use crate::disabled::DisabledType;
use crate::physics::PhysicsTemplate;
use crate::template::{LocomotorKind, LocomotorParams, ObjectTemplate};
use std::sync::Arc;

fn walker() -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new("Walker");
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.geometry.major_radius = 5.0;
    t.physics = Some(PhysicsTemplate { mass: 10.0, ..PhysicsTemplate::default() });
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 60.0, 100.0, 100.0, 360.0));
    Arc::new(t)
}

/// A walker at (50, 50) with a move order to (300, 50), disabled by `types`.
fn ordered_walker(types: &[DisabledType]) -> (GameLogic, ObjectId) {
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 0);
    let u = g.create_object(&walker(), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    g.ai_move_to_position_cmd(u, Coord3D::new(300.0, 50.0, 0.0), CommandSource::Player);
    for &t in types {
        g.set_disabled(u, t);
    }
    (g, u)
}

/// Guard (passes before and after this lane): a unit disabled by EMP alone is deferred by the sleepy gate that already
/// exists (`logic.rs::run_sleepy_updates_with`, `disabled_gate_defers`, GameLogic.cpp:3849-3885), so it neither updates its
/// state machine nor moves. Not a fix test: it pins the gate that test `a_held_unit_disabled_by_emp_still_runs_its_locomotor_kernel`
/// relies on being the only EMP filter.
#[test]
fn a_unit_disabled_by_a_type_other_than_held_is_not_updated_at_all() {
    let (mut g, u) = ordered_walker(&[DisabledType::Emp]);
    let before = g.find_object(u).unwrap().position;
    g.ai_pass_log.clear();
    g.run_frames(20);
    assert!(!g.ai_pass_log.contains(&u), "the sleepy gate keeps an EMP-only unit out of the AI pass");
    assert_eq!(g.find_object(u).unwrap().position, before, "and it does not move");
}

/// A HELD unit that is also EMP-disabled is updated (`dis` intersects HELD), and its `doLocomotor` still runs: EMP does
/// not stop it (AIUpdate.cpp:2127-2301 has no disabled check). The kernel marks the object driven
/// (`applyMotiveForce(&nullAccel)`, Locomotor.cpp:1035), which the physics would read. Its position does not change
/// anyway, since `PhysicsBehavior::update` skips `DISABLED_HELD` (PhysicsUpdate.cpp:641). Revert check: restore the
/// movement filter at the top of `ai_do_locomotor_move` and `motive_force_expires` stays 0.
#[test]
fn a_held_unit_disabled_by_emp_still_runs_its_locomotor_kernel() {
    let (mut g, u) = ordered_walker(&[DisabledType::Held, DisabledType::Emp]);
    g.ai_pass_log.clear();
    g.run_frames(3);
    assert!(g.ai_pass_log.contains(&u), "a HELD unit is in the AI pass");
    let expires = g.find_object(u).unwrap().physics.as_ref().unwrap().motive_force_expires;
    assert!(expires > 0, "doLocomotor ran the kernel while EMP is set: motive_force_expires {expires}");
}
