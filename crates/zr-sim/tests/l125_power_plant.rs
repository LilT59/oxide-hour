//! Lane 125: `PowerPlantUpdate` and `OverchargeBehavior` are separate modules with their own heap slots, and
//! `extendRods` / `enable` drive each other through the module interface.
//!
//! `PowerPlantUpdate`'s constructor sleeps forever (`PowerPlantUpdate.cpp:62`); `extendRods(TRUE)` wakes it
//! `RodsExtendTime` frames later (`:75-95`), `extendRods(FALSE)` puts it back to sleep and clears `m_extended`
//! (`:96-106`). `OverchargeBehavior` sleeps forever until `enable(TRUE)` (`OverchargeBehavior.cpp:83-88`, `:222-228`),
//! and its `update` drains `MaxHealth * HealthPercentToDrainPerSecond / LOGICFRAMES_PER_SECOND` as `DAMAGE_PENALTY`
//! until the body falls below `NotAllowedWhenHealthBelowPercent`, then switches itself off (`:95-160`). Its `enable`
//! extends the rods through `PowerPlantUpdateInterface` (`:189`, `:218`).

use std::sync::Arc;
use zr_sim::modules2::{OverchargeTemplate, PowerPlantTemplate};
use zr_sim::template::{kind_of, SleepyModuleKind};
use zr_sim::{Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

fn plant(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::STRUCTURE;
    t.max_health = 1000.0;
    t.initial_health = 1000.0;
    t.overcharge = Some(OverchargeTemplate {
        health_percent_to_drain_per_second: 0.3,
        not_allowed_when_health_below_percent: 0.5,
        rods_extend_time: 10,
    });
    t.power_plant = Some(PowerPlantTemplate { rods_extend_time: 10 });
    t.update_module_order = vec![SleepyModuleKind::PowerPlant, SleepyModuleKind::Overcharge];
    t
}

fn logic() -> GameLogic {
    GameLogic::new(TerrainLogic::flat(80, 80, 0), 1)
}

/// Both modules are heap entries asleep forever at creation (`PowerPlantUpdate.cpp:62`, `OverchargeBehavior.cpp:83-88`).
/// Revert check: the old single "one slot per PowerPlant order entry" loop plus a sweep gives the wrong slot count
/// (the `Overcharge` family would not exist and the plant would hold two `PowerPlantUpdate` slots).
#[test]
fn a_power_plant_holds_one_slot_for_each_of_its_two_modules() {
    use zr_sim::module::UPDATE_SLEEP_FOREVER;
    let t = Arc::new(plant("Plant"));
    let mut g = logic();
    let id = g.create_object(&t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let o = g.find_object(id).unwrap();
    assert!(o.behavior.power_plant_handle.is_some(), "PowerPlantUpdate has a slot");
    assert!(o.behavior.overcharge_handle.is_some(), "OverchargeBehavior has a slot");
    let pp = g.update_wake_frame(o.behavior.power_plant_handle.unwrap());
    let oc = g.update_wake_frame(o.behavior.overcharge_handle.unwrap());
    assert_eq!(pp, UPDATE_SLEEP_FOREVER, "PowerPlantUpdate starts asleep");
    assert_eq!(oc, UPDATE_SLEEP_FOREVER, "OverchargeBehavior starts asleep");
}

/// `OverchargeBehavior::enable(TRUE)` extends the rods (`OverchargeBehavior.cpp:218` -> `PowerPlantUpdate::extendRods`,
/// `PowerPlantUpdate.cpp:75-95`): the module wakes `RodsExtendTime` frames later, sets `m_extended`, then sleeps
/// forever (`:108-118`). Revert check: without the rods call the PowerPlantUpdate wake stays at forever.
#[test]
fn enabling_overcharge_extends_the_rods_on_the_rods_timer() {
    let t = Arc::new(plant("Plant"));
    let mut g = logic();
    let id = g.create_object(&t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let handle = g.find_object(id).unwrap().behavior.power_plant_handle.expect("power plant slot");
    assert!(g.update_wake_frame(handle) > 100, "asleep before the upgrade");
    assert!(g.set_overcharge(id, true));
    assert!(g.find_object(id).unwrap().behavior.power_plant_extended, "m_extended set before the wake");
    assert_eq!(g.update_wake_frame(handle), g.frame() + 10, "wakes RodsExtendTime frames later");
    g.run_frames(11);
    let o = g.find_object(id).unwrap();
    assert!(o.behavior.power_plant_extended, "still extended");
    assert!(g.update_wake_frame(o.behavior.power_plant_handle.unwrap()) > g.frame() + 100, "then sleeps forever");
    // Disabling retracts the rods at once and re-arms the forever sleep (`:96-106`).
    assert!(g.set_overcharge(id, false));
    let o = g.find_object(id).unwrap();
    assert!(!o.behavior.power_plant_extended, "m_extended cleared");
    assert!(
        g.update_wake_frame(o.behavior.power_plant_handle.unwrap()) > g.frame() + 100,
        "and the module is back asleep"
    );
}

/// The overcharge update drains the plant and turns itself off at `NotAllowedWhenHealthBelowPercent`
/// (`OverchargeBehavior.cpp:95-160`) from its own heap slot. Revert check: without the `Overcharge` family registration
/// nothing drains (the old per-frame sweep is gone).
#[test]
fn overcharge_is_a_heap_family_that_drains_and_switches_itself_off() {
    let t = Arc::new(plant("Plant"));
    let mut g = logic();
    let id = g.create_object(&t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    assert!(g.set_overcharge(id, true));
    let before = g.find_object(id).unwrap().health();
    g.run_frames(5);
    let after = g.find_object(id).unwrap().health();
    assert!(after < before, "overcharge drained health ({before} -> {after})");
    // 0.3 * 1000 / 30 = 10 per frame, so the 50% threshold is reached after ~50 frames.
    g.run_frames(60);
    let o = g.find_object(id).unwrap();
    assert!(!o.behavior.overcharge, "switched itself off below the threshold");
    assert!(!o.behavior.power_plant_extended, "and retracted the rods through enable(FALSE)");
}
