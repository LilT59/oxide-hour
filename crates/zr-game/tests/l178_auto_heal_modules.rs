//! The adapter keeps every AutoHeal module of a template (lane 178). A trainable vehicle with its own AutoHeal gets the
//! inherited `ModuleTag_DefaultAutoHealBehavior` as well (`ThingTemplate.cpp:467`: the inheritable default is only
//! dropped for untrainable templates), so C++ builds two AutoHealBehavior modules. The synthetic INI needs no install.
//! Revert check: the adapter's `.collect()` back to `find_map` makes the length 1 and the test fails.

use zr_game::adapter::convert_thing;
use zr_ini::IniDb;
use zr_templates::Templates;

const INI: &str = "
Object DefaultThingTemplate
  Body = ActiveBody ModuleTag_DefaultBody
    MaxHealth = 100.0
    InitialHealth = 100.0
  End
  InheritableModule
    Behavior = AutoHealBehavior ModuleTag_DefaultAutoHealBehavior
      HealingAmount = 10
      HealingDelay = 1000
      TriggeredBy = Upgrade_Veterancy_ELITE Upgrade_Veterancy_HEROIC
    End
  End
End

Object TwoHealTank
  KindOf = VEHICLE
  IsTrainable = Yes
  Body = ActiveBody ModuleTag_Body
    MaxHealth = 480.0
    InitialHealth = 400.0
  End
  Behavior = AutoHealBehavior ModuleTag_05
    HealingAmount = 2
    HealingDelay = 1000
    TriggeredBy = Upgrade_GLAJunkRepair
  End
End
";

fn templates(text: &str) -> Templates {
    let blocks = zr_ini::parse("test.ini", text).expect("synthetic INI parses");
    let mut db = IniDb::new();
    db.extend(blocks);
    Templates::from_ini(&db).0
}

/// The inherited veterancy module comes first (the template's module order), then the object's own.
#[test]
fn a_trainable_template_keeps_its_default_and_its_own_auto_heal() {
    let all = templates(INI);
    let t = convert_thing(&all, all.thing("TwoHealTank").expect("the template exists"));
    let amounts: Vec<(f32, Vec<String>)> = t.auto_heal.iter().map(|a| (a.healing_amount, a.triggered_by.clone())).collect();
    assert_eq!(
        amounts,
        vec![
            (10.0, vec!["Upgrade_Veterancy_ELITE".to_owned(), "Upgrade_Veterancy_HEROIC".to_owned()]),
            (2.0, vec!["Upgrade_GLAJunkRepair".to_owned()]),
        ]
    );
}
