//! Lane 173 (wave 20): the "arrival return" note of the R19 review. The port's locomotor kernel returns an arrival
//! flag (`locomotor::move_towards_position_force_blocked_out`'s tail) that the engine has no equivalent of: the C++
//! `locoUpdate_moveTowardsPosition` is `void`, and a move state ends when `getLocomotorDistanceToGoal() <
//! getCloseEnoughDist()` (`AIStates.cpp:1908-1910`), measured along `Path::computePointOnPath` (ground) or
//! `computeFlightDistToGoal` (air). The port only uses the flag for its own `ai.path` bookkeeping in
//! `do_locomotor_obj`, and only when `ai_goal_on_path` did not give a look-ahead point — i.e. for a mover without a
//! `PhysicsBehavior` (the reduced `move_towards_position` kernel). This test pins that no shipped mover is in that
//! case. Skips when no install is found.

use zr_assets::{Installs, Vfs};
use zr_templates::{ModuleData, Templates};

/// Every template whose `AIUpdateInterface` has a locomotor set also has a `PhysicsBehavior`, so the force-driven
/// kernel (the one whose goal comes from `computePointOnPath`) runs for every shipped mover, and the port's
/// arrival-flag pop never takes the reduced branch.
#[test]
fn every_shipped_mover_has_physics() {
    let Some(zh) = Installs::detect().zero_hour else { return };
    let Ok(vfs) = Vfs::zero_hour(&zh) else { return };
    let loaded = zr_ini::load_zero_hour(&vfs);
    let (t, _) = Templates::from_ini(&loaded.db);
    let mut movers = 0;
    let mut without_physics = Vec::new();
    for thing in t.things() {
        let mut has_locomotor = false;
        let mut has_physics = false;
        for m in &thing.modules {
            match &m.data {
                ModuleData::Ai(a) if !a.locomotor_sets.is_empty() => has_locomotor = true,
                ModuleData::Physics(_) => has_physics = true,
                _ => {}
            }
        }
        if !has_locomotor {
            continue;
        }
        movers += 1;
        if !has_physics {
            without_physics.push(thing.name.clone());
        }
    }
    println!("{movers} templates with a locomotor; without physics: {without_physics:?}");
    assert!(movers > 0, "the install has movable templates");
    assert!(
        without_physics.is_empty(),
        "a mover without PhysicsBehavior would take the port's reduced `move_towards_position` kernel: {without_physics:?}"
    );
}
