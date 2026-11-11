//! The team command-button actions' `CommandButton` fields (`ScriptActions.cpp:4463-4533`, `Object::doCommandButton*`),
//! read from the player's Zero Hour install and checked against the raw button fields. They skip (pass) when none is found.

use zr_assets::{Installs, Vfs};
use zr_game::{Game, GameData, PlayerSetup};
use zr_sim::script::{CommandButtonAction, FireTargets};
use zr_templates::{NameEnum, WeaponSlot};
use zr_ui::{command_option, CommandDb, CommandKind};

fn vfs() -> Option<Vfs> {
    let zh = Installs::detect().zero_hour?;
    Vfs::zero_hour(&zh).ok()
}

/// Every retail `Stop`, `Sell`, `UnitBuild` and `FireWeapon` button is in the action table with the fields its raw
/// `CommandButton` block gives: the unit of a unit build (`Object`), the `WeaponSlot` (PRIMARY when absent,
/// ControlBar.cpp:568), `MaxShotsToFire` (0x7fffffff when absent, :569) and the `NEED_TARGET_*` options.
#[test]
fn retail_stop_sell_unit_build_and_fire_buttons_land_in_the_action_table() {
    let Some(vfs) = vfs() else { return };
    let data = GameData::load(&vfs);
    let Some(db) = data.ini_db() else { return };
    let buttons = CommandDb::from_ini(&db);
    let (mut stops, mut sells, mut builds, mut fires) = (0, 0, 0, 0);
    for b in buttons.buttons() {
        let got = data.command_button_actions.get(&b.name);
        match &b.command {
            CommandKind::Stop => {
                stops += 1;
                assert_eq!(got, Some(&CommandButtonAction::Stop), "{}", b.name);
            }
            CommandKind::Sell => {
                sells += 1;
                assert_eq!(got, Some(&CommandButtonAction::Sell), "{}", b.name);
            }
            CommandKind::UnitBuild => {
                builds += 1;
                let thing = b.object.clone().unwrap_or_else(|| panic!("{} names no unit", b.name));
                assert_eq!(got, Some(&CommandButtonAction::UnitBuild { thing }), "{}", b.name);
            }
            CommandKind::FireWeapon => {
                fires += 1;
                let slot = b
                    .weapon_slot
                    .as_deref()
                    .and_then(|n| WeaponSlot::NAMES.iter().position(|s| s.eq_ignore_ascii_case(n)))
                    .unwrap_or(0);
                let expected = CommandButtonAction::FireWeapon {
                    slot,
                    max_shots: b.max_shots_to_fire,
                    targets: FireTargets {
                        enemy: b.options & command_option::NEED_TARGET_ENEMY_OBJECT != 0,
                        neutral: b.options & command_option::NEED_TARGET_NEUTRAL_OBJECT != 0,
                        ally: b.options & command_option::NEED_TARGET_ALLY_OBJECT != 0,
                        position: b.options & command_option::NEED_TARGET_POS != 0,
                        attack_objects_position: b.options & command_option::ATTACK_OBJECTS_POSITION != 0,
                    },
                };
                assert_eq!(got, Some(&expected), "{}", b.name);
            }
            _ => {}
        }
    }
    assert!(stops > 0 && sells > 0 && builds > 0 && fires > 0, "the install has each kind of button");
}

/// The skirmish game installs the table in its script engine (`Game::install_scripts`), so the team actions can run it.
#[test]
fn a_skirmish_game_installs_the_action_table() {
    let Some(vfs) = vfs() else { return };
    let data = std::sync::Arc::new(GameData::load(&vfs));
    if data.command_button_actions.is_empty() {
        panic!("the install has no command-button actions");
    }
    let setups = [PlayerSetup::new("FactionGLA", 0), PlayerSetup::new("FactionAmerica", 1)];
    let game = Game::new_skirmish_with(data.clone(), &vfs, "Alpine Assault", &setups, 1).expect("the skirmish loads");
    let installed = &game.logic().scripts().command_button_actions;
    assert_eq!(installed.len(), data.command_button_actions.len(), "every action is installed");
    let name = data.command_button_actions.keys().next().expect("non-empty");
    assert!(installed.contains_key(&name.to_ascii_lowercase()), "{name} is installed under its lower-case name");
}
