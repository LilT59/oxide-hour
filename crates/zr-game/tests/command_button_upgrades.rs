//! The upgrade command buttons the team command-button actions queue (`ScriptActions.cpp:4463`), read from the
//! player's Zero Hour install. They skip (pass) when none is found.

use zr_assets::{Installs, Vfs};
use zr_game::GameData;
use zr_sim::TemplateSource;

fn vfs() -> Option<Vfs> {
    let zh = Installs::detect().zero_hour?;
    Vfs::zero_hour(&zh).ok()
}

/// `Command_UpgradeAmericaAdvancedControlRods` (the Normal `USA Upgrade Power Plant - N` script's button) maps to
/// `Upgrade_AmericaAdvancedControlRods`, an object upgrade (`Type = OBJECT`: the power plant gets it, the player does
/// not), and the American power plant's command set holds that button (`Object::canProduceUpgrade`, `Object.cpp:6218`).
#[test]
fn america_power_plant_upgrade_button_maps_to_its_upgrade() {
    let Some(vfs) = vfs() else { return };
    let data = GameData::load(&vfs);
    let b = data.command_button_upgrades.get("Command_UpgradeAmericaAdvancedControlRods").expect("the button is in the table");
    assert_eq!(b.upgrade, "Upgrade_AmericaAdvancedControlRods");
    assert!(b.object, "AdvancedControlRods is an object upgrade");
    assert_eq!(
        data.store.can_build_upgrade("AmericaPowerPlant", "Upgrade_AmericaAdvancedControlRods"),
        Some(true),
        "the power plant's command set holds the button",
    );
    // The table holds both kinds of upgrade button; the shipped data also has prefixed names.
    assert!(data.command_button_upgrades.len() > 10, "the shipped upgrade buttons are all in the table");
    assert!(data.command_button_upgrades.values().any(|b| !b.object), "player upgrades are listed too");
    let sup = data.command_button_upgrades.get("SupW_Command_UpgradeAmericaAdvancedControlRods").expect("SupW variant");
    assert_eq!(sup.upgrade, "SupW_Upgrade_AmericaAdvancedControlRods");
}
