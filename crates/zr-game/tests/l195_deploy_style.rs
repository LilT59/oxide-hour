//! Lane 195: the `DeployStyleAIUpdate` flag reaches the sim template from the INI module list (`adapter.rs`,
//! `deploy_style_ai: t.modules.iter().any(|m| m.name == "DeployStyleAIUpdate")`).

use zr_game::adapter::convert_thing;
use zr_templates::Templates;

fn templates(text: &str) -> Templates {
    let blocks = zr_ini::parse("test.ini", text).expect("synthetic INI parses");
    let mut db = zr_ini::IniDb::new();
    db.extend(blocks);
    Templates::from_ini(&db).0
}

/// A sentry drone lists `DeployStyleAIUpdate` next to its `AIUpdateInterface` (the INI class name is the module name).
/// A plain unit with only `AIUpdateInterface` does not get the flag. Revert check: make the adapter's check `false`, and
/// the first assert fails.
#[test]
fn the_deploy_style_flag_comes_from_the_module_class_name() {
    let all = templates(
        "
Object SentryDrone
  KindOf = VEHICLE SELECTABLE
  Body = ActiveBody ModuleTag_01
    MaxHealth = 100
    InitialHealth = 100
  End
  Behavior = AIUpdateInterface ModuleTag_02
  End
  Behavior = DeployStyleAIUpdate ModuleTag_03
  End
  Locomotor = SET_NORMAL TankLoco
  Geometry = BOX
  GeometryMajorRadius = 5
  GeometryMinorRadius = 5
  GeometryHeight = 5
End
Object PlainTank
  KindOf = VEHICLE SELECTABLE
  Body = ActiveBody ModuleTag_01
    MaxHealth = 100
    InitialHealth = 100
  End
  Behavior = AIUpdateInterface ModuleTag_02
  End
  Geometry = BOX
  GeometryMajorRadius = 5
  GeometryMinorRadius = 5
  GeometryHeight = 5
End
",
    );
    let drone = convert_thing(&all, all.thing("SentryDrone").expect("the template exists"));
    assert!(drone.deploy_style_ai, "DeployStyleAIUpdate sets the flag");
    let tank = convert_thing(&all, all.thing("PlainTank").expect("the template exists"));
    assert!(!tank.deploy_style_ai, "AIUpdateInterface alone does not");
}
