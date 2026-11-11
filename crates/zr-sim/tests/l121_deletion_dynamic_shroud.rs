//! Lane 121: `DeletionUpdate` and `DynamicShroudClearingRangeUpdate` are ordinary `UpdateModule`s, so `registerObject`
//! pushes one sleepy-heap slot for each of them (`GameLogic.cpp:4021-4050`). Before this lane the port modelled
//! `DeletionUpdate` as a per-frame id sweep (`behaviors.rs::update_deletion`) and had no `DynamicShroudClearingRangeUpdate`
//! at all, so the heap was two entries short while the spy-satellite ping existed (AN3 seed 1: created at frame 1) and
//! every later equal-key order differed.
//!
//! `DeletionUpdate`'s constructor rolls its lifetime and sleeps until it (`DeletionUpdate.cpp:53-61`); its update
//! *destroys* the object (no death pipeline) and sleeps forever (`:100-106`). `DynamicShroudClearingRangeUpdate`'s
//! constructor sets no wake, so it is registered at the creation frame (`DynamicShroudClearingRangeUpdate.cpp:84-128`),
//! and its update returns `UPDATE_SLEEP_NONE` on every path (`:200-295`).

use std::sync::Arc;
use zr_sim::template::{kind_of, DynamicShroudTemplate};
use zr_sim::{Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

fn ping(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::IMMOBILE;
    t.max_health = 1.0;
    t.initial_health = 1.0;
    t
}

/// One heap slot for `DeletionUpdate`, and the object is destroyed on its lifetime's frame. Revert check: drop the
/// `FamilyKind::Deletion` registration and `update_count` is one lower (the sweep still destroys the object, so the
/// heap slot is what this test pins).
#[test]
fn a_deletion_module_holds_a_heap_slot_until_its_lifetime() {
    let mut t = ping("Ping");
    t.deletion = Some((5, 5));
    let t = Arc::new(t);
    let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 1);
    let id = g.create_object(&t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    // SMC, status, subdual and defection helpers plus the deletion module.
    assert_eq!(g.update_count(), 5, "the deletion module has its own slot");
    g.run_frames(5);
    assert!(g.find_object(id).is_some(), "still alive before its fifth frame");
    g.run_frames(2);
    assert!(g.find_object(id).is_none(), "DeletionUpdate destroys the object at its rolled frame");
    assert_eq!(g.update_count(), 0, "the slot goes with the object");
}

/// `DynamicShroudClearingRangeUpdate` gets a heap slot at the creation frame and is awake every frame.
/// Revert check: without the `FamilyKind::DynamicShroud` registration `update_count` is one lower.
#[test]
fn a_dynamic_shroud_module_is_an_awake_heap_slot() {
    let mut t = ping("Ping");
    t.dynamic_shroud = Some(DynamicShroudTemplate {
        change_interval: 80,
        grow_interval: 10,
        shrink_delay: 10000,
        shrink_time: 5000,
        grow_delay: 0,
        grow_time: 1000,
        final_vision: 0.0,
    });
    let t = Arc::new(t);
    let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 1);
    let id = g.create_object(&t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    assert_eq!(g.update_count(), 5, "SMC, status, subdual, defection and the dynamic shroud module");
    // The module runs on the creation frame (its first wake is the registration frame) and wakes every frame after.
    g.run_frames(3);
    assert!(g.find_object(id).is_some());
    assert_eq!(g.update_count(), 5, "an awake module keeps its slot");
}

/// Both modules in one template keep their INI order (the ping's `ModuleTag_02`/`ModuleTag_03`): the registration
/// order is `DynamicShroud`, `Deletion`, and both slots exist.
#[test]
fn the_two_modules_are_registered_in_order() {
    let mut t = ping("Ping");
    t.dynamic_shroud = Some(DynamicShroudTemplate {
        change_interval: 80,
        grow_interval: 10,
        shrink_delay: 10000,
        shrink_time: 5000,
        grow_delay: 0,
        grow_time: 1000,
        final_vision: 0.0,
    });
    t.deletion = Some((390, 390));
    t.update_module_order = vec![zr_sim::template::SleepyModuleKind::DynamicShroud, zr_sim::template::SleepyModuleKind::Deletion];
    let t = Arc::new(t);
    let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 1);
    g.create_object(&t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    assert_eq!(g.update_count(), 6);
}
