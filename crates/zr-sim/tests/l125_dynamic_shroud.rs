//! Lane 125: `DynamicShroudClearingRangeUpdate`'s ramp feeds `Object::setShroudClearingRange`.
//!
//! The constructor captures `m_nativeClearingRange = obj->getShroudClearingRange()` (the spy-satellite ping's
//! `VisionRange` 300), derives the grow/sustain/shrink deadlines from `ShrinkDelay`, `ShrinkTime`, `GrowDelay` and
//! `GrowTime`, and `update` ramps `m_currentClearingRange` and calls `Object::setShroudClearingRange`
//! (`DynamicShroudClearingRangeUpdate.cpp:84-295`, the set at :283). Before this lane the port only modelled the
//! module's heap slot and the `UPDATE_SLEEP_NONE` return, so the live range stayed at the template value.

use std::sync::Arc;
use zr_sim::template::{kind_of, DynamicShroudTemplate};
use zr_sim::{Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

/// The shipped ping's numbers (`data/ini/object/system.ini`, `Object SpySatellitePing`): a 300-unit vision grows
/// over `GrowTime` and shrinks to `FinalVision` after `ShrinkDelay`/`ShrinkTime`.
fn ping(dynamic: DynamicShroudTemplate) -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Ping");
    t.kind_of = kind_of::IMMOBILE;
    t.max_health = 1.0;
    t.initial_health = 1.0;
    t.vision_range = 300.0;
    t.shroud_clearing_range = -1.0;
    t.dynamic_shroud = Some(dynamic);
    t.update_module_order = vec![zr_sim::template::SleepyModuleKind::DynamicShroud];
    t
}

fn shipped() -> DynamicShroudTemplate {
    DynamicShroudTemplate {
        change_interval: 3,
        grow_interval: 1,
        shrink_delay: 300,
        shrink_time: 150,
        grow_delay: 0,
        grow_time: 30,
        final_vision: 0.0,
    }
}

/// `Object::Object` starts at the template's range and the ramp overwrites it: `native / growTime` per frame while
/// growing (`DynamicShroudClearingRangeUpdate.cpp:238-243`). Revert check: without the family's ramp the range stays
/// at the template value forever.
#[test]
fn the_dynamic_shroud_module_ramps_the_live_clearing_range() {
    let t = Arc::new(ping(shipped()));
    let mut g = GameLogic::new(TerrainLogic::flat(80, 80, 0), 1);
    let id = g.create_object(&t, Coord3D::new(400.0, 400.0, 0.0), 0.0, 0);
    assert_eq!(g.find_object(id).unwrap().shroud_clearing_range, 300.0, "-1 means VisionRange (Object.cpp:286-289)");
    // The module's first update is frame 1 (registration clamps to the creation frame); each adds 300/30 = 10.
    g.run_frames(2);
    let o = g.find_object(id).unwrap();
    assert_eq!(o.behavior.ds_current_clearing_range, 10.0, "one grow step");
    assert_eq!(o.shroud_clearing_range, 10.0, "the change-interval tick stores it");
    // It reaches the native range on the 30th update and the change interval then keeps it there.
    g.run_frames(29);
    assert_eq!(g.find_object(id).unwrap().behavior.ds_current_clearing_range, 300.0, "grown to the native range");
    g.run_frames(2);
    assert_eq!(g.find_object(id).unwrap().shroud_clearing_range, 300.0, "the stored range follows");
}

/// The shrink half: after `ShrinkDelay` frames the range falls by `(native - FinalVision) / ShrinkTime` per change
/// tick until `FinalVision` (`DynamicShroudClearingRangeUpdate.cpp:255-258`), which is what the ping's reveal
/// collapse is. Revert check: without the ramp the range never shrinks.
#[test]
fn the_dynamic_shroud_module_shrinks_to_the_final_vision() {
    let t = Arc::new(ping(shipped()));
    let mut g = GameLogic::new(TerrainLogic::flat(80, 80, 0), 1);
    let id = g.create_object(&t, Coord3D::new(400.0, 400.0, 0.0), 0.0, 0);
    // Growing ends at the 30th update; the state countdown is 450, so shrinking starts when it reaches 150, i.e.
    // 301 frames after the creation frame.
    g.run_frames(300);
    let at_shrink_start = g.find_object(id).unwrap().behavior.ds_current_clearing_range;
    assert_eq!(at_shrink_start, 300.0, "still at full range when the shrink starts");
    g.run_frames(10);
    assert!(
        g.find_object(id).unwrap().behavior.ds_current_clearing_range < 300.0,
        "the shrink steps lower the range"
    );
    // 150 shrink frames later the countdown reaches 0 and the range is FinalVision.
    g.run_frames(160);
    assert_eq!(g.find_object(id).unwrap().shroud_clearing_range, 0.0, "shrunk to FinalVision");
}

/// The ramp reaches the shroud grid: the ping's look covers cells 280 units out while grown, and the look is
/// dropped (the cell falls back to fog after the unlook trail) once the range has shrunk to `FinalVision`.
/// Revert check: with a static range the far cell stays clear forever.
#[test]
fn the_ramped_range_drives_the_shroud_look() {
    use zr_sim::shroud::CellShroudStatus;
    let t = Arc::new(ping(shipped()));
    let mut g = GameLogic::new(TerrainLogic::flat(120, 120, 0), 1);
    let _id = g.create_object(&t, Coord3D::new(600.0, 600.0, 0.0), 0.0, 0);
    g.run_frames(31);
    let far = Coord3D::new(880.0, 600.0, 0.0);
    assert_eq!(g.cell_shroud_status(0, &far), CellShroudStatus::Clear, "the grown look reaches 280 units out");
    // The range hits FinalVision 450 frames after creation; the look's unlook trails for the engine's 150 frames.
    g.run_frames(600);
    assert_eq!(g.cell_shroud_status(0, &far), CellShroudStatus::Fogged, "the shrunken look no longer reaches");
}
