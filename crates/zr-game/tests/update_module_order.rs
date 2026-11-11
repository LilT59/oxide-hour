//! The sleepy-heap module order of a template (`ObjectTemplate::update_module_order`, `adapter::sleepy_module_order`).
//! The synthetic test needs no install; the install test skips when none is found.

use zr_game::adapter::convert_thing;
use zr_ini::IniDb;
use zr_templates::Templates;
use zr_sim::template::SleepyModuleKind;
use zr_sim::TemplateSource;

const INI: &str = "
Object OrderTank
  KindOf = VEHICLE
  Body = ActiveBody ModuleTag_Body
    MaxHealth = 480.0
    InitialHealth = 400.0
  End
  Behavior = FlammableUpdate ModuleTag_Flammable
  End
  Behavior = AutoHealBehavior ModuleTag_AutoHeal
  End
  Behavior = SlowDeathBehavior ModuleTag_Slow
  End
  Behavior = StealthUpdate ModuleTag_Stealth
  End
  Behavior = PoisonedBehavior ModuleTag_Poisoned
  End
End
";

fn templates(text: &str) -> Templates {
    let blocks = zr_ini::parse("test.ini", text).expect("synthetic INI parses");
    let mut db = IniDb::new();
    db.extend(blocks);
    Templates::from_ini(&db).0
}

/// The behaviour list in INI order, one entry per sleepy module; the body and the non-heap modules are absent.
#[test]
fn the_sleepy_modules_follow_the_ini_order() {
    let all = templates(INI);
    let t = convert_thing(&all, all.thing("OrderTank").expect("the template exists"));
    assert_eq!(
        t.update_module_order,
        vec![
            SleepyModuleKind::Flammable,
            SleepyModuleKind::AutoHeal,
            SleepyModuleKind::SlowDeath { die: 0 },
            SleepyModuleKind::Stealth,
            SleepyModuleKind::Poisoned,
        ]
    );
}

/// `ReplaceModule` removes the tagged module and appends its replacement (`ThingTemplate::removeModuleInfo` erases it,
/// then `ModuleInfo::addModuleInfo` pushes the new one, ThingTemplate.cpp:401-412 and :428-); so a replaced module moves
/// to the end of the order, as the template's own module list does.
#[test]
fn a_replaced_module_moves_to_the_end_of_the_order() {
    let all = templates(
        "
Object Replaced
  Behavior = FlammableUpdate ModuleTag_A
  End
  Behavior = AutoHealBehavior ModuleTag_B
  End
  ReplaceModule ModuleTag_A
    Behavior = FlammableUpdate ModuleTag_A2
    End
  End
End
",
    );
    let t = convert_thing(&all, all.thing("Replaced").expect("the template exists"));
    assert_eq!(t.update_module_order, vec![SleepyModuleKind::AutoHeal, SleepyModuleKind::Flammable]);
}

#[test]
fn an_object_without_sleepy_modules_has_an_empty_order() {
    let all = templates("Object Bare\n  KindOf = STRUCTURE\nEnd\n");
    let t = convert_thing(&all, all.thing("Bare").expect("the template exists"));
    assert!(t.update_module_order.is_empty());
}

/// `AIPlayer::queueDozer` walks `ThingFactory::firstTemplate` (AIPlayer.cpp:3169-3236): templates are prepended, so the
/// last one defined comes first, and only `KINDOF_DOZER` templates are candidates.
/// Revert-check: without the reversal the order is oldest first and the assert fails.
#[test]
fn dozer_templates_are_newest_first_and_only_dozers() {
    let blocks = zr_ini::parse(
        "test.ini",
        "Object DozerOld\n  KindOf = DOZER\nEnd\nObject Soldier\n  KindOf = INFANTRY\nEnd\nObject DozerNew\n  KindOf = DOZER\nEnd\n",
    )
    .expect("synthetic INI parses");
    let mut db = IniDb::new();
    db.extend(blocks);
    let data = zr_game::GameData::from_ini(&db);
    assert_eq!(data.store.dozer_templates(), vec!["DozerNew".to_owned(), "DozerOld".to_owned()]);
}

/// The four templates of the C++ heap dump at frame 1000 (AN3 seed 1): the orders are the install's INI module order.
/// Their classes agree with the dump's entries for the objects 231 (worker), 211 (dozer), 228 (power plant) and 212
/// (GLA command center), and the same-key entries keep this order. Skips without an install.
#[test]
fn the_install_templates_follow_their_ini_order() {
    let Some(zh) = zr_assets::Installs::detect().zero_hour else { return };
    let Ok(vfs) = zr_assets::Vfs::zero_hour(&zh) else { return };
    let data = zr_game::GameData::load(&vfs);
    use SleepyModuleKind::*;
    let slow = |n: usize| (0..n).map(|die| SlowDeath { die }).collect::<Vec<_>>();
    let mut worker = vec![Stealth, Ai, Physics];
    worker.extend(slow(7));
    worker.extend([StealthDetector, Poisoned, Production]);
    let expect = [
        ("GLAInfantryWorker", worker),
        ("AmericaVehicleDozer", vec![Stealth, Ai, Physics, StealthDetector, SlowDeath { die: 0 }, Flammable]),
        ("AmericaPowerPlant", vec![BaseRegenerate, PowerPlant, Production, Flammable]),
        ("GLACommandCenter", vec![Production, ProductionExit, Flammable]),
    ];
    for (name, order) in expect {
        let t = data.store.find_template(name).unwrap_or_else(|| panic!("{name} is in the install"));
        assert_eq!(t.update_module_order, order, "{name}");
    }
}
