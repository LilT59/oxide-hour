//! Lane 123: the adapter types `StealthUpgrade` (`StealthUpgrade.cpp:40-70`, parsed as
//! `LogicModule::StealthUpgrade`) as the sim's `UpgradeEffect::Stealth`, so a camouflage upgrade
//! reaches `StealthUpgrade::upgradeImplementation`. Synthetic INI only.

use zr_game::adapter::convert_thing;
use zr_ini::IniDb;
use zr_sim::upgrade::UpgradeEffect;
use zr_templates::Templates;

const INI: &str = "
Object CamoTank
  KindOf = VEHICLE
  Body = ActiveBody ModuleTag_Body
    MaxHealth = 200.0
    InitialHealth = 200.0
  End
  Behavior = StealthUpdate ModuleTag_Stealth
    InnateStealth = No
    StealthDelay = 0
  End
  Behavior = StealthUpgrade ModuleTag_Camo
    TriggeredBy = Upgrade_GLA_Camo
  End
End
";

fn templates(text: &str) -> Templates {
    let blocks = zr_ini::parse("test.ini", text).expect("synthetic INI parses");
    let mut db = IniDb::new();
    db.extend(blocks);
    Templates::from_ini(&db).0
}

/// `convert_upgrade_modules` maps `LogicModule::StealthUpgrade` to `UpgradeEffect::Stealth` with its
/// `UpgradeMuxData` triggers. Revert check: drop the adapter arm and no upgrade module is produced.
#[test]
fn the_adapter_maps_stealth_upgrade_to_the_stealth_effect() {
    let all = templates(INI);
    let t = convert_thing(&all, all.thing("CamoTank").expect("the template exists"));
    let module = t
        .upgrade_modules
        .iter()
        .find(|m| matches!(m.effect, UpgradeEffect::Stealth))
        .expect("StealthUpgrade becomes an upgrade module");
    assert_eq!(module.triggered_by, vec!["Upgrade_GLA_Camo".to_owned()]);
    assert!(!module.requires_all_triggers);
}
