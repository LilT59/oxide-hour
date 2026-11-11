//! Lane 170 (wave 20): the sleepy heap carries one entry per `UpdateModule` of a template.
//!
//! Two module classes the port was missing:
//! - `ParkingPlaceBehavior` is an `UpdateModule` (`ParkingPlaceBehavior.h:93`) whose constructor sets
//!   `UPDATE_SLEEP_NONE` (`ParkingPlaceBehavior.cpp:57`): every airfield owns a heap entry (AN1 s1 f2728).
//! - The retail data names `ProductionUpdate` twice on the China Gattling Cannon
//!   (`ChinaGattlingCannon.ini` `ModuleTag_07` and `ModuleTag_10`), so the C++ object has two modules and
//!   two heap entries (`registerObject` walks `m_behaviors`, GameLogic.cpp:4165-4200). The second one's
//!   queue is never used (`Object::getProductionUpdateInterface` returns the first), but its heap entry
//!   changes the tie order of equal keys (AN1 s1 f2727).

use std::sync::Arc;
use zr_sim::modules2::ParkingPlaceTemplate;
use zr_sim::production::ProductionTemplate;
use zr_sim::template::{kind_of, ObjectTemplate, SleepyModuleKind};
use zr_sim::{Coord3D, GameLogic, TerrainLogic};

fn building(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::STRUCTURE | kind_of::SELECTABLE;
    t.max_health = 500.0;
    t.initial_health = 500.0;
    t.geometry.major_radius = 15.0;
    t
}

fn world() -> GameLogic {
    let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
    g.add_player(0, 0);
    g
}

/// A template that lists `ProductionUpdate` twice gets two heap entries, as the C++ object does.
/// Revert check: `family_registrations` pushing a single `Production` family (no `ProductionIdle` for
/// the extra copies) makes the two counts equal and the assert fails.
#[test]
fn a_template_that_lists_production_twice_gets_two_heap_entries() {
    let mut g = world();
    let mut once = building("Factory");
    once.production = Some(ProductionTemplate::default());
    once.update_module_order = vec![SleepyModuleKind::Production];
    let mut twice = building("FactoryTwice");
    twice.production = Some(ProductionTemplate::default());
    twice.update_module_order = vec![SleepyModuleKind::Production, SleepyModuleKind::Production];
    let a = g.create_object(&Arc::new(once), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(twice), Coord3D::new(600.0, 600.0, 0.0), 0.0, 0);
    assert_eq!(
        g.update_module_count(b),
        g.update_module_count(a) + 1,
        "the second `ProductionUpdate` is its own heap entry (`registerObject`)"
    );
    // The first module keeps the queue: one production entry produces at most one unit per frame, which
    // the single-module template shows. A second slot must not tick the same queue twice.
    g.run_frames(4);
    assert_eq!(g.update_module_count(b), g.update_module_count(a) + 1, "the slots stay");
}

/// An airfield owns a `ParkingPlaceBehavior` heap entry, awake every frame.
/// Revert check: dropping the `parking_place` family registration makes the counts equal.
#[test]
fn an_airfield_gets_a_parking_place_heap_entry() {
    let mut g = world();
    let mut plain = building("Plain");
    plain.production = Some(ProductionTemplate::default());
    let mut airfield = building("Airfield");
    airfield.production = Some(ProductionTemplate::default());
    airfield.parking_place = Some(ParkingPlaceTemplate {
        num_rows: 1,
        num_cols: 1,
        has_runways: false,
        heal_amount_per_second: 10.0,
        approach_height: 0.0,
        landing_deck_height_offset: 0.0,
        park_in_hangars: false,
    });
    let a = g.create_object(&Arc::new(plain), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(airfield), Coord3D::new(600.0, 600.0, 0.0), 0.0, 0);
    assert_eq!(
        g.update_module_count(b),
        g.update_module_count(a) + 1,
        "`ParkingPlaceBehavior` is an `UpdateModule` with its own heap entry"
    );
}
