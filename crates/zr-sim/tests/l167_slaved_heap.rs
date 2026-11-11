//! Lane 167: the sleepy-heap composition of AH7 s1's stinger soldiers (`PhysicsUpdate.cpp:641-913`, `SlavedUpdate.cpp:110-130`).
//!
//! Two faithful fixes that change the heap's entries and keys (and so the tie order of equal keys):
//! - A HELD object (a spawn-point exit unit, "You are stuck here, little man.", `SpawnPointProductionExitUpdate.cpp:112`)
//!   skips only the integration; `PhysicsBehavior::update` still reaches its tail (`UPDATE_EVER_RUN`, `calcSleepTime`).
//! - `SlavedUpdate` is an every-frame heap entry whose body runs every `SLAVED_UPDATE_RATE` frames (`m_framesToWait`,
//!   SlavedUpdate.cpp:114-127).

use std::sync::Arc;
use zr_sim::disabled::DisabledType;
use zr_sim::modules2::SlavedTemplate;
use zr_sim::physics::PhysicsTemplate;
use zr_sim::template::{kind_of, SimpleTemplateStore};
use zr_sim::{Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

fn unit(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::INFANTRY;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.physics = Some(PhysicsTemplate::default());
    t
}

/// A resting unit held by a spawn-point exit: its integration is skipped, but the update's tail runs and sets
/// `UPDATE_EVER_RUN` (so `calcSleepTime` can sleep it forever). Revert check: returning early from the HELD branch
/// (before the tail) leaves `update_ever_run` false, and the physics never sleeps.
#[test]
fn a_held_resting_unit_runs_the_update_tail() {
    let mut g = GameLogic::new(TerrainLogic::flat(80, 80, 0), 1);
    let mut store = SimpleTemplateStore::default();
    store.add(unit("Held"));
    g.set_template_source(Arc::new(store));
    let t = Arc::new(unit("Held"));
    let id = g.create_object(&t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    g.set_disabled(id, DisabledType::Held);
    g.run_frames(2);
    let o = g.find_object(id).expect("alive");
    let p = o.physics.as_ref().expect("a physics module");
    assert!(p.update_ever_run, "the held update reached the tail (`setFlag(UPDATE_EVER_RUN, true)`, PhysicsUpdate.cpp:909)");
    assert_eq!(p.velocity.x, 0.0, "no integration while held");
    assert_eq!(p.velocity.y, 0.0, "no integration while held");
}

/// A slaved unit's `SlavedUpdate` body runs when `m_framesToWait` reaches zero and then every `SLAVED_UPDATE_RATE`
/// (7) frames: the countdown reads 7, 6, ..., 1 and is reset to 7 on the body's frame (SlavedUpdate.cpp:114-127).
/// Revert check: without the countdown gate the body runs every frame and the countdown stays 0.
#[test]
fn a_slaved_units_body_runs_every_seven_frames() {
    let mut g = GameLogic::new(TerrainLogic::flat(80, 80, 0), 1);
    let mut store = SimpleTemplateStore::default();
    let mut slave = unit("Slave");
    slave.slaved = Some(SlavedTemplate {
        guard_max_range: 0,
        guard_wander_range: 0,
        attack_range: 0,
        repair_rate_per_second: 0.0,
        stay_on_same_layer_as_master: false,
    });
    store.add(slave.clone());
    g.set_template_source(Arc::new(store));
    let slave_id = g.create_object(&Arc::new(slave), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let countdown = |g: &GameLogic| g.find_object(slave_id).expect("alive").behavior.slaved_frames_to_wait;
    // The first update (the constructor sets no wake) runs the body and resets the countdown.
    for _ in 0..3 {
        if countdown(&g) != 0 {
            break;
        }
        g.run_frames(1);
    }
    assert_eq!(countdown(&g), 7, "the first update runs the body and resets the countdown to SLAVED_UPDATE_RATE");
    let mut seen = Vec::new();
    for _ in 0..7 {
        g.run_frames(1);
        seen.push(countdown(&g));
    }
    assert_eq!(seen, vec![6, 5, 4, 3, 2, 1, 7], "the countdown runs down and the body runs again on the seventh frame");
}
