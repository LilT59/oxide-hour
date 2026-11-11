//! Lane 129 (R10 finding 1): the sleepy loop's disabled-types gate applies to the AI module too.
//!
//! `GameLogic::update`'s loop tests `!dis.any() || dis.anyIntersectionWith(u->getDisabledTypesToProcess())`
//! (`GameLogic.cpp:3849-3885`, the `RETAIL_COMPATIBLE_CRC` branch, `GameDefines.h:102`) before it runs a module.
//! `AIUpdateInterface::getDisabledTypesToProcess` is `MAKE_DISABLED_MASK(DISABLED_HELD)` (`AIUpdate.h:305`), so a
//! unit disabled by anything else (EMP, PARALYZED, SUBDUED, HACKED, UNMANNED, UNDERPOWERED, ...) is deferred: the
//! module is re-pushed at `now + 1` and its AI does not run at all. A unit disabled only by `DISABLED_HELD`
//! (a contained passenger) keeps running its AI. The port used to give the AI slot `u16::MAX` as its mask and never
//! called the gate on that branch, so every disabled unit ran its AI every frame.

use std::sync::Arc;

use zr_sim::disabled::DisabledType;
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::{Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

/// A mobile unit: a locomotor gives it an `AIUpdateInterface` (`ai_handle`).
fn vehicle(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 8.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    t
}

fn flat() -> TerrainLogic {
    TerrainLogic::flat(200, 200, 0)
}

/// An EMP- or PARALYZED-disabled unit's AI is deferred to `now + 1` instead of running: the idle update would
/// otherwise consume the idle sleep offset and set the machine's `sleep_till` to `IDLE_COUNTDOWN_DELAY + offset`
/// (AIStates.cpp:1400), i.e. the heap key would become ~60 frames out (`ai_update_sleep`, AIUpdate.cpp:1013-1028).
/// Revert checks: drop the AI branch's `disabled_gate_defers` call and the key is the idle sleep; leave the AI
/// mask at `u16::MAX` and the EMP/PARALYZED masks intersect it, so the AI runs as well.
#[test]
fn a_disabled_ai_does_not_run_and_is_deferred_to_the_next_frame() {
    for disabled in [DisabledType::Emp, DisabledType::Paralyzed] {
        let mut g = GameLogic::new(flat(), 1);
        let id = g.create_object(&Arc::new(vehicle("Tank")), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
        let h = g.ai_update_handle(id).expect("a locomotor gives the object an AI module");
        // `AIUpdateInterface`'s constructor schedules its first update one frame after creation (AIUpdate.cpp:299-301).
        assert_eq!(g.update_wake_frame(h), 1);
        // Frame 0's loop has nothing due, so at frame 1 the module is due but has not run yet.
        g.run_frames(1);
        assert_eq!(g.frame(), 1);
        assert_eq!(g.update_wake_frame(h), 1);

        g.set_disabled(id, disabled);
        g.run_frames(1);
        assert_eq!(g.frame(), 2);
        assert_eq!(
            g.update_wake_frame(h),
            2,
            "{disabled:?}: a gated update is re-pushed at now + 1 (GameLogic.cpp:3875-3876)"
        );
        let obj = g.find_object(id).expect("still alive");
        assert_eq!(obj.ai.machine.sleep_till, 0, "{disabled:?}: the AI machine did not run: its idle update would set sleep_till");
    }
}

/// `DISABLED_HELD` is in the AI module's mask (`AIUpdate.h:305`): a held unit still runs its AI (that is how a
/// passenger's order completes while the container carries it). Here the idle update runs, consumes its sleep
/// offset and sets `sleep_till` / the heap key to the idle countdown.
#[test]
fn a_held_units_ai_still_runs() {
    let mut g = GameLogic::new(flat(), 1);
    let id = g.create_object(&Arc::new(vehicle("Tank")), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    let h = g.ai_update_handle(id).expect("a locomotor gives the object an AI module");
    g.run_frames(1);
    assert_eq!(g.update_wake_frame(h), 1);

    g.set_disabled(id, DisabledType::Held);
    g.run_frames(1);
    assert_eq!(g.frame(), 2);
    let obj = g.find_object(id).expect("still alive");
    assert!(obj.ai.machine.sleep_till > g.frame(), "the idle update ran and set its countdown");
    assert_eq!(
        g.update_wake_frame(h),
        obj.ai.machine.sleep_till,
        "an awake AI keeps the machine's sleep as its heap key"
    );
}
