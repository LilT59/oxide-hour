//! Lane 131: the adapter's `SleepyModuleKind` mapping for the modules the AN3 objects use that lane 110 left out:
//! `CommandButtonHuntUpdate`, the `SpecialAbility` / `SpecialAbilityUpdate` pair and `AutoFindHealingUpdate`. Synthetic
//! INI first; the install case (the GLA rebel and the US ranger) skips when no install is found.
//!
//! C++: `Object::Object` builds the behaviour array in INI order (`Object.cpp:300-410`) and `registerObject` walks it
//! (`GameLogic.cpp:4013-4046`); a `SpecialAbility` power module is not an `UpdateModule` (`SpecialAbilityUpdate.h:169`).

use zr_game::adapter::convert_thing;
use zr_ini::IniDb;
use zr_templates::Templates;
use zr_sim::template::SleepyModuleKind;
use zr_sim::TemplateSource;

fn templates(text: &str) -> Templates {
    let blocks = zr_ini::parse("test.ini", text).expect("synthetic INI parses");
    let mut db = IniDb::new();
    db.extend(blocks);
    Templates::from_ini(&db).0
}

const INI: &str = "
Object HuntRanger
  KindOf = INFANTRY
  Behavior = CommandButtonHuntUpdate ModuleTag_02
    ScanRate = 1000
  End
  Behavior = AutoFindHealingUpdate ModuleTag_05
    ScanRate = 1000
    ScanRange = 200
  End
  Behavior = AIUpdateInterface ModuleTag_06
  End
  Behavior = PhysicsBehavior ModuleTag_07
  End
  Behavior = CommandButtonHuntUpdate ModuleTag_09
  End
  Behavior = SlowDeathBehavior ModuleTag_Death01
  End
  Behavior = SpecialAbility ModuleTag_18
    SpecialPowerTemplate = SpecialAbilityTest
  End
  Behavior = SpecialAbilityUpdate ModuleTag_19
    SpecialPowerTemplate = SpecialAbilityTest
    StartAbilityRange = 5.0
  End
  Behavior = CleanupHazardUpdate ModuleTag_30
  End
  Behavior = DemoTrapUpdate ModuleTag_31
  End
End
";

/// The two `CommandButtonHuntUpdate` modules keep their INI positions (ModuleTag_02 before the AI and physics,
/// ModuleTag_09 after them) and the `SpecialAbility` power module is not in the order: only `SpecialAbilityUpdate` is an
/// `UpdateModule`. Revert check: the old `ModuleData::SpecialAbility(_)` arm added a power-module entry, and the old
/// `SimpleUpdate` arm did not name the hunt class.
#[test]
fn the_hunt_ability_and_heal_modules_are_named_in_ini_order() {
    use SleepyModuleKind::*;
    let all = templates(INI);
    let t = convert_thing(&all, all.thing("HuntRanger").expect("the template exists"));
    assert_eq!(
        t.update_module_order,
        vec![
            CommandButtonHunt,
            AutoFindHealing,
            Ai,
            Physics,
            CommandButtonHunt,
            SlowDeath { die: 0 },
            SpecialAbility,
            CleanupHazard,
            DemoTrap,
        ]
    );
    // The power module is carried (cooldown/pause) but not in the heap order.
    assert_eq!(t.special_abilities.iter().filter(|a| a.power_module).count(), 1);
    assert_eq!(t.special_abilities.iter().filter(|a| !a.power_module).count(), 1);
}

/// The install's two hunt templates: the GLA rebel (`GLAInfantry.ini ModuleTag_03..22`) and the US ranger
/// (`AmericaInfantry.ini:527-...`, two hunts with the AI, physics and production between them). Skips without an
/// install.
#[test]
fn the_install_rebel_and_ranger_orders_match_their_ini_module_lists() {
    let Some(zh) = zr_assets::Installs::detect().zero_hour else { return };
    let Ok(vfs) = zr_assets::Vfs::zero_hour(&zh) else { return };
    let data = zr_game::GameData::load(&vfs);
    use SleepyModuleKind::*;
    let slow = |n: usize| (0..n).map(|die| SlowDeath { die }).collect::<Vec<_>>();
    // The rebel is trainable, so the inheritable `DefaultThingTemplate` modules come first
    // (`ModuleTag_DefaultAutoHealBehavior`; `zr-templates/src/thing.rs`, `Object.cpp:300-410`).
    let mut rebel = vec![AutoHeal, Ai, CommandButtonHunt, Physics, Stealth];
    rebel.extend(slow(7));
    rebel.extend([Poisoned, SpecialAbility, SpecialAbility]);
    let mut ranger = vec![AutoHeal, Stealth, CommandButtonHunt, AutoFindHealing, Ai, Physics, Production, CommandButtonHunt];
    ranger.extend(slow(7));
    ranger.push(Poisoned);
    ranger.push(SpecialAbility);
    for (name, order) in [("GLAInfantryRebel", rebel), ("AmericaInfantryRanger", ranger)] {
        let t = data.store.find_template(name).unwrap_or_else(|| panic!("{name} is in the install"));
        assert_eq!(t.update_module_order, order, "{name}");
    }
}
