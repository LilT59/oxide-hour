//! Lane 138: review R12's template/adapter findings F3 (`MobNexusContain`/`CaveContain` are real
//! contain classes with their own heap slot) and F6 (`HealContain`/`GarrisonContain` must not
//! overload `TransportContainModuleData::m_healthRegen`). Synthetic INI only.

use zr_game::adapter::convert_thing;
use zr_ini::IniDb;
use zr_sim::contain::{ContainKind, ContainTemplate};
use zr_sim::template::SleepyModuleKind;
use zr_templates::Templates;

fn templates(text: &str) -> Templates {
    let blocks = zr_ini::parse("test.ini", text).expect("synthetic INI parses");
    let mut db = IniDb::new();
    db.extend(blocks);
    Templates::from_ini(&db).0
}

fn convert(text: &str, name: &str) -> zr_sim::ObjectTemplate {
    let all = templates(text);
    convert_thing(&all, all.thing(name).expect("the template exists"))
}

fn contain(t: &zr_sim::ObjectTemplate) -> &ContainTemplate {
    t.contain.as_deref().expect("the template has a contain module")
}

/// `MobNexusContain` and `CaveContain` are in the module table (`module_table.rs`) and derive from
/// `OpenContain` (`MobNexusContain.h:62`, `CaveContain.h:66`), so they parse as typed contain
/// modules and get one heap slot each (`registerObject`, GameLogic.cpp:4013-4046). Before F3 they fell
/// into the `SimpleUpdate` catch-all: no `ContainTemplate`, no slot.
/// Revert check: without the two names in `ContainKind::from_class_name` the update order is empty and
/// `t.contain` is `None`.
#[test]
fn mob_nexus_and_cave_are_their_own_contain_classes_and_heap_slots() {
    let ini = "
Object Nexus
  KindOf = VEHICLE
  Behavior = MobNexusContain ModuleTag_01
    Slots = 3
    HealthRegen%PerSec = 5
    InitialPayload = AmericaInfantryRanger 2
  End
End
Object CaveNet
  KindOf = STRUCTURE
  Behavior = CaveContain ModuleTag_01
    CaveIndex = 7
    ContainMax = 4
  End
End
";
    let nexus = convert(ini, "Nexus");
    assert_eq!(nexus.update_module_order, vec![SleepyModuleKind::MobNexusContain]);
    let c = contain(&nexus);
    assert_eq!(c.kind, ContainKind::MobNexus);
    // `MobNexusContain::getContainMax` returns `m_slotCapacity` (`Slots`, MobNexusContain.cpp:100-107).
    assert_eq!(c.capacity, 3);
    // `HealthRegen%PerSec` is `MobNexusContainModuleData::m_healthRegen` (MobNexusContain.cpp:104).
    assert_eq!(c.health_regen, 5.0);
    assert_eq!(c.initial_payload, vec![("AmericaInfantryRanger".to_string(), 2)]);

    let cave = convert(ini, "CaveNet");
    assert_eq!(cave.update_module_order, vec![SleepyModuleKind::CaveContain]);
    let c = contain(&cave);
    assert_eq!(c.kind, ContainKind::Cave);
    assert_eq!(c.capacity, 4);
}

/// F6: `HealContainModuleData`/`GarrisonContainModuleData` have their own `TimeForFullHeal` and the
/// garrison a `DoIHealObjects` flag; only `TransportContainModuleData` (and the `MobNexusContain`
/// copy) has `HealthRegen%PerSec` (`TransportContain.cpp:108`, `GarrisonContain.h:69-70`,
/// `HealContain.cpp:62`). The adapter used to write `1.0 / frames` into `health_regen` for a Heal
/// (a value nothing read) and a 0/1 flag for a Garrison; each now has its own sim field.
/// Revert check: a Heal's `health_regen` is 1/75 and the Garrison's is 1.0, `do_i_heal_objects` false.
#[test]
fn heal_and_garrison_use_their_own_fields_not_health_regen() {
    let ini = "
Object Hospital
  KindOf = STRUCTURE
  Behavior = HealContain ModuleTag_01
    TimeForFullHeal = 2500
  End
End
Object Bunker
  KindOf = STRUCTURE
  Behavior = GarrisonContain ModuleTag_01
    HealObjects = Yes
    TimeForFullHeal = 3000
  End
End
Object BunkerNoHeal
  KindOf = STRUCTURE
  Behavior = GarrisonContain ModuleTag_01
    TimeForFullHeal = 3000
  End
End
Object Truck
  KindOf = VEHICLE
  Behavior = TransportContain ModuleTag_01
    Slots = 2
    HealthRegen%PerSec = 3
  End
End
";
    let hospital = convert(ini, "Hospital");
    let c = contain(&hospital);
    assert_eq!(c.health_regen, 0.0, "a HealContain has no HealthRegen%PerSec");
    assert_eq!(c.heal_frames, 75.0, "TimeForFullHeal 2500 ms = 75 whole frames (HealContain.cpp:62)");
    assert!(!c.do_i_heal_objects, "only a garrison has DoIHealObjects");
    let bunker = convert(ini, "Bunker");
    let c = contain(&bunker);
    assert!(c.do_i_heal_objects, "HealObjects = Yes (GarrisonContain.h:69)");
    assert_eq!(c.heal_frames, 90.0, "TimeForFullHeal 3000 ms = 90 frames (parseDurationReal)");
    assert_eq!(c.health_regen, 0.0, "a garrison has no HealthRegen%PerSec");
    let no_heal = convert(ini, "BunkerNoHeal");
    assert!(!contain(&no_heal).do_i_heal_objects);
    let truck = convert(ini, "Truck");
    assert_eq!(contain(&truck).health_regen, 3.0, "a transport keeps its HealthRegen%PerSec");
}
