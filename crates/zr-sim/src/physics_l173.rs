//! Lane 173 (wave 20): R19-5 — the tail `PhysicsBehavior::update` runs for a `DISABLED_HELD` object
//! (`PhysicsUpdate.cpp:818-913`). Only the integration is inside the HELD guard (`:641`): the accumulator is zeroed,
//! the overlaps roll, the landing block, the freefall clear, `KillWhenRestingOnGround` and the airborne flags all run.

use super::*;
use crate::template::{kind_of, ObjectTemplate};
use crate::terrain::TerrainLogic;
use std::sync::Arc;

/// A ground object with `KillWhenRestingOnGround`: a cling-to-ground drone that splats when it comes to rest.
fn ground_killer() -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new("Mortar");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.geometry.major_radius = 4.0;
    t.physics = Some(PhysicsTemplate {
        mass: 10.0,
        kill_when_resting_on_ground: true,
        min_fall_speed_for_damage: 1.0e9,
        ..PhysicsTemplate::default()
    });
    Arc::new(t)
}

/// R19-5 (`PhysicsUpdate.cpp:641` guards only the integration; `:896-907` kills a resting object): a HELD object that
/// was airborne last frame, is on the ground this frame, has (almost) no velocity and `KillWhenRestingOnGround` is
/// killed by one physics update. Its `DISABLED_FREEFALL` and `IS_IN_FREEFALL` are cleared too (`:886-893`).
/// Revert check: return from the HELD branch after the flag writes (the old early return); the object survives and
/// keeps its freefall state.
#[test]
fn a_held_object_that_comes_to_rest_on_the_ground_is_killed() {
    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
    let id = g.create_object(&ground_killer(), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    {
        let p = g.objects.get_mut(&id).unwrap().physics.as_mut().expect("physics");
        p.was_airborne_last_frame = true; // it fell onto the ground this frame
        p.is_in_freefall = true;
        p.velocity = Coord3D::ZERO;
    }
    g.set_disabled(id, DisabledType::Held);
    g.set_disabled(id, DisabledType::Freefall);
    let now = g.frame();
    g.update_object_physics(id);
    let o = g.objects.get(&id);
    assert!(
        o.is_none_or(|o| o.destroyed || o.effectively_dead),
        "`KillWhenRestingOnGround` runs for a HELD object (`PhysicsUpdate.cpp:896-907`)"
    );
    let _ = now;
}

/// R19-5, the freefall half: a HELD object that is already on the ground loses `IS_IN_FREEFALL` and
/// `DISABLED_FREEFALL` (`PhysicsUpdate.cpp:886-893`), and its `UPDATE_EVER_RUN` is set so `calcSleepTime` can put the
/// module to sleep (`:909-913`). Revert check: the old early return leaves all three untouched.
#[test]
fn a_held_object_on_the_ground_clears_its_freefall_state() {
    let mut t = ObjectTemplate::new("Clinger");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.geometry.major_radius = 4.0;
    t.physics = Some(PhysicsTemplate { mass: 10.0, ..PhysicsTemplate::default() });
    let t = Arc::new(t);
    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
    let id = g.create_object(&t, Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    {
        let p = g.objects.get_mut(&id).unwrap().physics.as_mut().expect("physics");
        p.is_in_freefall = true;
    }
    g.set_disabled(id, DisabledType::Held);
    g.set_disabled(id, DisabledType::Freefall);
    g.update_object_physics(id);
    let o = g.objects.get(&id).expect("alive");
    let p = o.physics.as_ref().expect("physics");
    assert!(!p.is_in_freefall, "`setFlag(IS_IN_FREEFALL, false)` (`PhysicsUpdate.cpp:889`)");
    assert!(!o.is_disabled_by_type(DisabledType::Freefall), "`clearDisabled(DISABLED_FREEFALL)` (`:890-891`)");
    assert!(p.update_ever_run, "`setFlag(UPDATE_EVER_RUN, true)` (`:909`)");
}
