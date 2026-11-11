//! Lane 125: the module data the new families read reaches the sim template — `PowerPlantUpdate` vs
//! `OverchargeBehavior` (two separate modules parsed into one `ModuleData::Overcharge`), `TechBuildingBehavior`'s
//! pulse FX and `AutoDepositUpdate`'s `InitialCaptureBonus`.

use zr_game::adapter::convert_thing;
use zr_ini::IniDb;
use zr_sim::template::SleepyModuleKind;
use zr_templates::Templates;

fn templates(text: &str) -> Templates {
    let blocks = zr_ini::parse("test.ini", text).expect("synthetic INI parses");
    let mut db = IniDb::new();
    db.extend(blocks);
    Templates::from_ini(&db).0
}

/// The US power plant's two modules (`Object AmericaPowerPlant`, `data/ini/object/factionbuilding.ini`):
/// `PowerPlantUpdate` (`RodsExtendTime = 600`) and `OverchargeBehavior` (`HealthPercentToDrainPerSecond = 3%`) are
/// separate heap entries in INI order (`OverchargeBehavior.cpp:83-88`, `PowerPlantUpdate.cpp:62`). The class name
/// decides which is which, because both parse into `ModuleData::Overcharge`.
/// Revert check: with the merged `find_map` both fields come from the first module (the OverchargeBehavior numbers are
/// 0 and the rods timer is 0), and the order lists two `PowerPlant`s.
#[test]
fn a_power_plants_rods_and_overcharge_modules_are_told_apart() {
    let all = templates(
        "
Object Plant
  KindOf = STRUCTURE
  Body = StructureBody ModuleTag_06
    MaxHealth = 800
    InitialHealth = 800
  End
  Behavior = PowerPlantUpdate ModuleTag_09
    RodsExtendTime = 600
  End
  Behavior = OverchargeBehavior ModuleTag_10
    HealthPercentToDrainPerSecond = 3%
    NotAllowedWhenHealthBelowPercent = 0%
  End
End
",
    );
    let t = convert_thing(&all, all.thing("Plant").expect("the template exists"));
    assert_eq!(t.update_module_order, vec![SleepyModuleKind::PowerPlant, SleepyModuleKind::Overcharge]);
    let rods = t.power_plant.expect("PowerPlantUpdate reaches the template");
    assert_eq!(rods.rods_extend_time, 18, "600 ms at 30 frames/s");
    let oc = t.overcharge.expect("OverchargeBehavior reaches the template");
    assert!((oc.health_percent_to_drain_per_second - 0.03).abs() < 1e-6, "3% as a fraction");
    assert_eq!(oc.rods_extend_time, 18, "enable() drives the object's rods timer");
}

/// `TechBuildingBehavior`'s pulse data (`TechBuildingBehavior.cpp:64-72`) and its heap entry.
/// Revert check: without the adapter arm `tech_building` is `None`.
#[test]
fn a_tech_building_pulse_reaches_the_sim_template() {
    let all = templates(
        "
Object Derrick
  KindOf = STRUCTURE
  Body = StructureBody ModuleTag_01
    MaxHealth = 500
    InitialHealth = 500
  End
  Behavior = TechBuildingBehavior ModuleTag_02
    PulseFX = FX_DerrickPulse
    PulseFXRate = 500
  End
End
",
    );
    let t = convert_thing(&all, all.thing("Derrick").expect("the template exists"));
    assert!(t.update_module_order.contains(&SleepyModuleKind::TechBuilding));
    let tech = t.tech_building.clone().expect("TechBuildingBehavior reaches the template");
    assert_eq!(tech.pulse_fx.as_deref(), Some("FX_DerrickPulse"));
    assert_eq!(tech.pulse_fx_rate, 15, "500 ms");
}

/// `AutoDepositUpdate`'s `InitialCaptureBonus` (`AutoDepositUpdate.cpp:129-151`) reaches the template.
/// Revert check: without the adapter field the value is missing (0).
#[test]
fn an_auto_deposit_capture_bonus_reaches_the_sim_template() {
    let all = templates(
        "
Object Derrick
  KindOf = STRUCTURE
  Body = StructureBody ModuleTag_01
    MaxHealth = 500
    InitialHealth = 500
  End
  Behavior = AutoDepositUpdate ModuleTag_02
    DepositTiming = 1000
    DepositAmount = 200
    InitialCaptureBonus = 500
  End
End
",
    );
    let t = convert_thing(&all, all.thing("Derrick").expect("the template exists"));
    let deposit = t.auto_deposit.clone().expect("AutoDepositUpdate reaches the template");
    assert_eq!(deposit.initial_capture_bonus, 500);
    assert_eq!(deposit.deposit_amount, 200);
}
