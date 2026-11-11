//! Lane 121: the `ObjectHelper` base constructor sleeps the helper at `FOREVER` (`ObjectHelper.h:58`:
//! `setWakeFrame(getObject(), UPDATE_SLEEP_FOREVER)`). `ObjectSMCHelper` (`ObjectSMCHelper.h:52`) and
//! `ObjectDefectionHelper` (`ObjectDefectionHelper.h:62`) add no wake of their own, so they are created asleep; only
//! `ObjectWeaponStatusHelper` moves itself to the next frame (`setWakeFrame(NONE)`, `ObjectWeaponStatusHelper.h:66`).
//! Before this lane the port registered the SMC and defection helpers at the creation frame, so they ran on frame 1
//! in every object's life and sifted the whole sleepy heap once per object (the heap layout never matched C++).

use std::sync::Arc;
use zr_sim::template::kind_of;
use zr_sim::weapon::{WeaponSetTemplate, WeaponTemplate};
use zr_sim::{Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

/// An armed infantryman: a `Body` (so the status and subdual helpers exist) and one weapon (so the weapon status,
/// firing tracker and temporary-bonus helpers exist). Every constructor helper applies (Object.cpp:320-395).
fn armed(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::INFANTRY | kind_of::SELECTABLE;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    let mut w = WeaponTemplate::new("Gun");
    w.primary_damage = 1.0;
    w.attack_range = 100.0;
    let mut set = WeaponSetTemplate::default();
    set.weapons[0] = Some(Arc::new(w));
    t.weapon_sets.push((0, set));
    t
}

/// One object carries the seven constructor helpers; on its first update only the weapon status helper runs.
/// Revert check: register the SMC and defection helpers at the creation frame again and the update count is 3.
#[test]
fn only_the_weapon_status_helper_runs_on_the_first_frame() {
    let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 1);
    let id = g.create_object(&Arc::new(armed("Rifleman")), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    assert_eq!(g.update_count(), 7, "SMC, status, subdual, defection, weapon status, firing tracker, temp bonus");
    assert_eq!(g.stats().module_updates, 0);
    // Frame 0's sleepy loop has nothing due (the earliest wake is 1); frame 1's loop runs the weapon status helper.
    g.run_frames(1);
    assert_eq!(g.stats().module_updates, 0, "a helper created at frame 0 first runs on frame 1");
    g.run_frames(1);
    assert_eq!(g.stats().module_updates, 1, "only ObjectWeaponStatusHelper wakes at creation + 1");
    assert!(g.find_object(id).is_some());
}

/// A body-less shrubbery has the SMC helper only (`InactiveBody` -> no status/subdual helper, Object.cpp:330-340;
/// `KINDOF_SHRUBBERY` -> no defection helper, :370-378; no weapon -> no weapon helpers, :385-395), and it is asleep.
#[test]
fn a_bodiless_shrubbery_has_one_helper_and_it_does_not_run() {
    let mut t = ObjectTemplate::new("Bush");
    t.kind_of = kind_of::SHRUBBERY;
    t.max_health = 0.0;
    let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 1);
    g.create_object(&Arc::new(t), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    assert_eq!(g.update_count(), 1, "ObjectSMCHelper");
    g.run_frames(3);
    assert_eq!(g.stats().module_updates, 0, "the SMC helper sleeps until something wakes it");
}
