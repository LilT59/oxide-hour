//! Lane 125: the `ObjectDefectionHelper` defection timer.
//!
//! `ObjectDefectionHelper::startDefectionTimer` (`ObjectDefectionHelper.cpp:108-127`) starts the hidden-defector
//! countdown on an object that is already an undetected defector; `update` (`:48-102`) ticks every frame and clears the
//! flag when the timer runs out ("you are vulnerable now"), or when the object dies or fires. The flash/sound are
//! client-only.
//!
//! Reachability: the C++ callers are `Object::defect(newTeam, detectionTime)` (`Object.cpp:6281-6284`) and
//! `Object::goInvulnerable(time)` (`Object.cpp:6353-6361`). The port's capture path (`special_power.rs`) calls
//! `GameLogic::set_object_team` rather than modelling `Object::defect`, so no game path reaches this yet; the helper is
//! exercised through `GameLogic::set_undetected_defector` + `start_defection_timer`.

use std::sync::Arc;
use zr_sim::template::kind_of;
use zr_sim::{Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

fn unit(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::INFANTRY;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t
}

/// The timer clears `UNDETECTED_DEFECTOR` when it runs out (`ObjectDefectionHelper.cpp:56-78`). Revert check: with the
/// helper's update stubbed to `UPDATE_SLEEP_FOREVER` the flag stays set.
#[test]
fn the_defection_helper_clears_the_flag_when_the_timer_runs_out() {
    let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 1);
    let id = g.create_object(&Arc::new(unit("Defector")), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    g.set_undetected_defector(id, true);
    g.start_defection_timer(id, 3, true);
    // The helper wakes next frame; the flag is still set until the timer runs out at frame 3.
    g.run_frames(2);
    assert!(g.find_object(id).unwrap().undetected_defector, "still hidden at frame 2");
    g.run_frames(2);
    assert!(!g.find_object(id).unwrap().undetected_defector, "the timer ran out at frame 3");
}

/// `startDefectionTimer` on an object that is not an undetected defector only puts the helper back to sleep
/// (`ObjectDefectionHelper.cpp:112-116`); the timer does not start. Revert check: without the guard the helper would
/// run (its wake would be frame + 1).
#[test]
fn starting_a_timer_without_being_a_defector_only_sleeps_the_helper() {
    let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 1);
    let id = g.create_object(&Arc::new(unit("Loyal")), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    g.start_defection_timer(id, 10, false);
    g.run_frames(1);
    let o = g.find_object(id).unwrap();
    assert!(!o.undetected_defector);
    assert_eq!(o.combat.helpers.defection.end, 0, "the timer did not start");
    assert_eq!(g.update_count(), 4, "SMC, status, subdual and defection helpers");
}
