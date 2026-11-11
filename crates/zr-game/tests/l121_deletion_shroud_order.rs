//! Lane 121: `DeletionUpdate` and `DynamicShroudClearingRangeUpdate` reach the sim template and the sleepy-heap module
//! order (`adapter::sleepy_module_order`), in the template's INI order. The spy-satellite ping of AN3 seed 1
//! (`Object SpySatellitePing`, `data/ini/object/system.ini`) has `DynamicShroudClearingRangeUpdate ModuleTag_02`,
//! `DeletionUpdate ModuleTag_03` and `StealthDetectorUpdate ModuleTag_04`; the C++ heap of frame 1 pops its
//! `DynamicShroudClearingRangeUpdate` at the creation frame, and the deletion module sits at `13000 ms` = 390 frames.

use zr_game::adapter::convert_thing;
use zr_ini::IniDb;
use zr_sim::template::SleepyModuleKind;
use zr_templates::Templates;

/// The ping's module block (the numbers are the install's `system.ini` values).
const INI: &str = "
Object Ping
  KindOf = NO_COLLIDE IMMOBILE UNATTACKABLE INERT
  Body = ImmortalBody ModuleTag_01
    MaxHealth = 1
    InitialHealth = 1
  End
  Behavior = DynamicShroudClearingRangeUpdate ModuleTag_02
    FinalVision = 0.0
    ShrinkDelay = 10000
    ShrinkTime = 5000
    GrowDelay = 0
    GrowTime = 1000
    GrowInterval = 10
    ChangeInterval = 80
  End
  Behavior = DeletionUpdate ModuleTag_03
    MinLifetime = 13000
    MaxLifetime = 13000
  End
  Behavior = StealthDetectorUpdate ModuleTag_04
    DetectionRate = 500
  End
End
";

fn templates(text: &str) -> Templates {
    let blocks = zr_ini::parse("test.ini", text).expect("synthetic INI parses");
    let mut db = IniDb::new();
    db.extend(blocks);
    Templates::from_ini(&db).0
}

/// The heap order and the two module datas, as the engine's `parseDurationUnsignedInt` converts them (milliseconds to
/// whole frames, rounding up, `INI.cpp:1798-1802`).
/// Revert check: drop either `LogicModule` arm from `sleepy_module_order` and the order (and the field) is missing it.
#[test]
fn the_ping_modules_reach_the_sim_template_in_ini_order() {
    let all = templates(INI);
    let t = convert_thing(&all, all.thing("Ping").expect("the template exists"));
    assert_eq!(
        t.update_module_order,
        vec![SleepyModuleKind::DynamicShroud, SleepyModuleKind::Deletion, SleepyModuleKind::StealthDetector]
    );
    let shroud = t.dynamic_shroud.clone().expect("DynamicShroudClearingRangeUpdate reaches the template");
    assert_eq!(shroud.shrink_delay, 300, "10000 ms");
    assert_eq!(shroud.shrink_time, 150, "5000 ms");
    assert_eq!(shroud.grow_delay, 0);
    assert_eq!(shroud.grow_time, 30, "1000 ms");
    assert_eq!(shroud.grow_interval, 1, "10 ms rounds up to one frame");
    assert_eq!(shroud.change_interval, 3, "80 ms rounds up to three frames");
    assert_eq!(shroud.final_vision, 0.0);
    assert_eq!(t.deletion, Some((390, 390)), "13000 ms");
}

/// A template without either module keeps an empty field and no order entry.
#[test]
fn a_template_without_them_has_neither() {
    let all = templates("Object Bare\n  KindOf = STRUCTURE\nEnd\n");
    let t = convert_thing(&all, all.thing("Bare").expect("the template exists"));
    assert!(t.dynamic_shroud.is_none());
    assert_eq!(t.deletion, None);
    assert!(!t.update_module_order.contains(&SleepyModuleKind::DynamicShroud));
    assert!(!t.update_module_order.contains(&SleepyModuleKind::Deletion));
}
