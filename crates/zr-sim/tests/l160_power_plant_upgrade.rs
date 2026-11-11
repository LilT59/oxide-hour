//! Lane 160 (wave 19): `PowerPlantUpgrade::upgradeImplementation` (`PowerPlantUpgrade.cpp:110-128`) adds the power
//! bonus and calls `extendRods(TRUE)` on every power plant interface, so the plant's `PowerPlantUpdate` wakes
//! `RodsExtendTime` frames later. AN1 s1 China power plant 227 (f1914): C++ re-keys its `PowerPlantUpdate` to wake
//! 1931; the port left it asleep forever, which moved the heap layout and the dozer/truck tie order at f2070.

use std::sync::Arc;
use zr_sim::modules2::PowerPlantTemplate;
use zr_sim::template::{kind_of, SleepyModuleKind};
use zr_sim::upgrade::{SimUpgradeModule, UpgradeEffect};
use zr_sim::{Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

fn upgradable_plant() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("UpgradablePlant");
    t.kind_of = kind_of::STRUCTURE;
    t.max_health = 1000.0;
    t.initial_health = 1000.0;
    t.power_plant = Some(PowerPlantTemplate { rods_extend_time: 10 });
    t.update_module_order = vec![SleepyModuleKind::PowerPlant];
    t.upgrade_modules = vec![Arc::new(SimUpgradeModule {
        triggered_by: vec!["Upgrade_PowerPlant".to_owned()],
        conflicts_with: vec![],
        removes_upgrades: vec![],
        requires_all_triggers: false,
        effect: UpgradeEffect::PowerPlant,
    })];
    t
}

/// The upgrade extends the rods: `extendRods(TRUE)` sets `m_extended` and wakes the module `RodsExtendTime` frames
/// later (`PowerPlantUpdate.cpp:75-95`), and the upgrade runs once (a second grant does not re-wake it).
/// Revert check: drop the `extend_power_plant_rods` call in `update_upgrade_modules` (upgrade.rs) and the plant stays
/// asleep forever with `power_plant_extended` false.
#[test]
fn a_power_plant_upgrade_extends_the_rods_once() {
    let mut g = GameLogic::new(TerrainLogic::flat(80, 80, 0), 1);
    let t = Arc::new(upgradable_plant());
    let id = g.create_object(&t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let handle = g.find_object(id).unwrap().behavior.power_plant_handle.expect("power plant slot");
    assert!(g.update_wake_frame(handle) > g.frame() + 100, "asleep before the upgrade");
    assert!(!g.find_object(id).unwrap().behavior.power_plant_extended);

    g.give_upgrade(id, "Upgrade_PowerPlant");
    let o = g.find_object(id).unwrap();
    assert!(o.behavior.power_plant_upgraded, "the upgrade is on");
    assert!(o.behavior.power_plant_extended, "PowerPlantUpgrade extends the rods (PowerPlantUpgrade.cpp:125)");
    assert_eq!(g.update_wake_frame(handle), g.frame() + 10, "wakes RodsExtendTime frames later");

    // A second grant of the same upgrade does not extend again (the upgrade executes once).
    g.run_frames(11);
    g.give_upgrade(id, "Upgrade_PowerPlant");
    assert!(g.update_wake_frame(handle) > g.frame() + 100, "no second wake from the same upgrade");
}
