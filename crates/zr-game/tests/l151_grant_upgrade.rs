//! Lane 151 (wave 17): `GrantUpgradeCreate` (`Object/Create/GrantUpgradeCreate.cpp`) on the player's
//! shipped Zero Hour data.
//!
//! `onCreate` (`:82-115`) grants when `ExemptStatus` names `UNDER_CONSTRUCTION` and the object is not
//! under construction; `onBuildComplete` (`:117-139`) always grants. A player-type upgrade goes to the
//! controlling player (`Player::addUpgrade`), an object-type one to the object (`Object::giveUpgrade`).
//! The America command center's `Upgrade_AmericaRadar` (OBJECT) is what keeps its `RadarUpgrade` on
//! (`RadarUpgrade.cpp:128-143`), i.e. the USA player's radar; the GLA radar van grants
//! `Upgrade_GLARadar` at build complete (it has no `ExemptStatus`), and the tech oil refinery's
//! `Upgrade_CostReduction` is the shipped `CostModifierUpgrade` user.
//!
//! The tests use the player's install and skip (pass) when none is found.

use std::sync::Arc;

use zr_assets::{Installs, Vfs};
use zr_game::{AiDifficulty, Game, GameData, PlayerSetup};
use zr_sim::TemplateSource;

fn vfs() -> Option<Vfs> {
    let zh = Installs::detect().zero_hour?;
    Vfs::zero_hour(&zh).ok()
}

fn data() -> Option<Arc<GameData>> {
    let vfs = vfs()?;
    Some(Arc::new(GameData::load(&vfs)))
}

/// The converted `GrantUpgradeCreate` modules of a template as `(upgrade, player_upgrade,
/// exempt_under_construction)`.
fn grants(data: &GameData, template: &str) -> Vec<(String, bool, bool)> {
    let t = data.store.find_template(template).unwrap_or_else(|| panic!("{template} is missing"));
    t.grant_upgrade_create.iter().map(|g| (g.upgrade.clone(), g.player_upgrade, g.exempt_under_construction)).collect()
}

/// The adapter keeps the module and its two fields: the upgrade's scope comes from
/// `UpgradeTemplate::getUpgradeType()` (`Upgrade.ini`), the exemption from `ExemptStatus`.
#[test]
fn the_adapter_keeps_each_grant_upgrade_create_module() {
    let Some(data) = data() else { return };
    assert_eq!(grants(&data, "AmericaCommandCenter"), vec![("Upgrade_AmericaRadar".to_owned(), false, true)]);
    assert_eq!(grants(&data, "GLAVehicleRadarVan"), vec![("Upgrade_GLARadar".to_owned(), false, false)]);
    assert_eq!(grants(&data, "TechOilRefinery"), vec![("Upgrade_CostReduction".to_owned(), false, false)]);
    // The Toxin general's command center has two player-type grants and no ExemptStatus.
    assert_eq!(
        grants(&data, "Chem_GLACommandCenter"),
        vec![("Upgrade_GLAToxinShells".to_owned(), true, false), ("Upgrade_GLAAnthraxBeta".to_owned(), true, false)]
    );
    // Infantry general: a player upgrade with the construction exemption.
    assert_eq!(grants(&data, "Infa_ChinaBarracks"), vec![("Upgrade_Nationalism".to_owned(), true, true)]);
}

/// AN3 (GLA p2 vs America p3, Normal): the starting America command center is created complete, so its
/// `onCreate` grants `Upgrade_AmericaRadar` to itself (`ExemptStatus = UNDER_CONSTRUCTION` only skips the
/// grant for a scaffold), and its `RadarUpgrade` then gives player 3 radar.
#[test]
fn an3_the_america_command_center_grants_itself_the_radar_upgrade_at_start() {
    let Some(vfs) = vfs() else { return };
    let data = Arc::new(GameData::load(&vfs));
    let setups = [PlayerSetup::ai("FactionGLA", 0), PlayerSetup::ai("FactionAmerica", 1).with_difficulty(AiDifficulty::Normal)];
    let mut g = Game::new_skirmish_with(data, &vfs, "Alpine Assault", &setups, 1).unwrap();
    g.run_frames(2);
    let cc = g
        .logic()
        .objects()
        .find(|o| o.template.name == "AmericaCommandCenter" && o.player == 3)
        .expect("the America starting command center");
    assert!(cc.upgrades.contains("Upgrade_AmericaRadar"), "the object upgrade was granted at creation: {:?}", cc.upgrades);
    assert!(g.logic().player_has_radar(3), "RadarUpgrade reacted to the grant: player 3 has radar");
    assert!(!g.logic().player_has_radar(2), "the GLA player's radar van is not built yet");
}
