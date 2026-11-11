//! Lane 148 (wave 17): the six upgrade modules `convert_upgrade_modules` used to drop, on the
//! player's shipped Zero Hour data.
//!
//! `Object::affectedByUpgrade` (Object.cpp:4505-4520) only looks at the object's upgrade modules, so
//! an upgrade whose object has nothing but `RadarUpgrade` / `SubObjectsUpgrade` /
//! `ReplaceObjectUpgrade` / `PassengersFireUpgrade` / `GrantScienceUpgrade` / `CostModifierUpgrade`
//! could not be researched anywhere. The first case is the AN5/AN1 frame-401 divergence:
//! `Upgrade_ChinaRadar` on `ChinaCommandCenter` (`RadarUpgrade ModuleTag_14`).
//!
//! The tests use the player's install and skip (pass) when none is found.

use std::sync::Arc;

use zr_assets::{Installs, Vfs};
use zr_game::{AiDifficulty, Game, GameData, PlayerSetup};
use zr_sim::template::kind_of;
use zr_sim::upgrade::UpgradeEffect;
use zr_sim::TemplateSource;

const CHINA_RADAR: &str = "Upgrade_ChinaRadar";

fn vfs() -> Option<Vfs> {
    let zh = Installs::detect().zero_hour?;
    Vfs::zero_hour(&zh).ok()
}

fn data() -> Option<Arc<GameData>> {
    let vfs = vfs()?;
    Some(Arc::new(GameData::load(&vfs)))
}

/// Every upgrade module of `template` as `(triggered_by, effect)`.
fn modules(data: &GameData, template: &str) -> Vec<(Vec<String>, UpgradeEffect)> {
    let t = data.store.find_template(template).unwrap_or_else(|| panic!("{template} is missing"));
    t.upgrade_modules.iter().map(|m| (m.triggered_by.clone(), m.effect.clone())).collect()
}

fn has(mods: &[(Vec<String>, UpgradeEffect)], trigger: &str, pred: impl Fn(&UpgradeEffect) -> bool) -> Option<UpgradeEffect> {
    mods.iter().find(|(trig, e)| trig.iter().any(|t| t == trigger) && pred(e)).map(|(_, e)| e.clone())
}

/// The adapter now keeps all six families with their fields: the China / America command centers'
/// `RadarUpgrade` (`DisableProof = No`), the GLA radar van's (`DisableProof = Yes`), the America
/// command center's `GrantScienceUpgrade` (`SCIENCE_MOAB`), the Helix's `PassengersFireUpgrade`,
/// the bomb truck's three `SubObjectsUpgrade` payloads, the fake GLA buildings'
/// `ReplaceObjectUpgrade`, and the tech oil refinery's `CostModifierUpgrade` (10% off vehicles).
#[test]
fn the_adapter_keeps_the_six_upgrade_families() {
    let Some(data) = data() else { return };

    let china = modules(&data, "ChinaCommandCenter");
    let Some(UpgradeEffect::Radar { disable_proof }) = has(&china, CHINA_RADAR, |e| matches!(e, UpgradeEffect::Radar { .. })) else {
        panic!("ChinaCommandCenter must carry a RadarUpgrade triggered by {CHINA_RADAR}: {china:?}");
    };
    assert!(!disable_proof, "the China radar is not DisableProof");
    let cc = data.store.find_template("ChinaCommandCenter").unwrap();
    assert_eq!(cc.radar_extend_time, 120.0, "RadarExtendTime = 4000 ms = 120 frames");

    let america = modules(&data, "AmericaCommandCenter");
    assert!(matches!(has(&america, "Upgrade_AmericaRadar", |e| matches!(e, UpgradeEffect::Radar { .. })), Some(UpgradeEffect::Radar { disable_proof: false })));
    assert!(matches!(
        has(&america, "Upgrade_AmericaMOAB", |e| matches!(e, UpgradeEffect::GrantScience { .. })),
        Some(UpgradeEffect::GrantScience { science }) if science == "SCIENCE_MOAB"
    ));

    let van = modules(&data, "GLAVehicleRadarVan");
    assert!(
        matches!(has(&van, "Upgrade_GLARadar", |e| matches!(e, UpgradeEffect::Radar { .. })), Some(UpgradeEffect::Radar { disable_proof: true })),
        "the GLA radar van's radar is DisableProof"
    );

    let helix = modules(&data, "ChinaVehicleHelix");
    assert!(has(&helix, "Upgrade_ChinaHelixBattleBunker", |e| matches!(e, UpgradeEffect::PassengersFire)).is_some());

    let truck = modules(&data, "GLAVehicleBombTruck");
    let payloads = truck.iter().filter(|(_, e)| matches!(e, UpgradeEffect::SubObjects)).count();
    assert_eq!(payloads, 3, "the bomb truck shows one payload sub-object per upgrade: {truck:?}");

    let fake = modules(&data, "FakeGLACommandCenter");
    let Some(UpgradeEffect::ReplaceObject { template }) = has(&fake, "Upgrade_BecomeRealGLACommandCenter", |e| matches!(e, UpgradeEffect::ReplaceObject { .. })) else {
        panic!("FakeGLACommandCenter must carry a ReplaceObjectUpgrade: {fake:?}");
    };
    assert_eq!(template.as_ref().map(|t| t.name.as_str()), Some("GLACommandCenter"), "the replacement template is resolved");

    let refinery = modules(&data, "TechOilRefinery");
    let Some(UpgradeEffect::CostModifier { kind_of: effect_kind_of, percentage }) = has(&refinery, "Upgrade_CostReduction", |e| matches!(e, UpgradeEffect::CostModifier { .. })) else {
        panic!("TechOilRefinery must carry a CostModifierUpgrade: {refinery:?}");
    };
    assert_eq!(effect_kind_of, kind_of::VEHICLE, "EffectKindOf = Vehicle");
    assert!((percentage + 0.1).abs() < 1e-6, "Percentage = -10%");
}

/// `ProductionUpdate::queueUpgrade`'s object branch (`ProductionUpdate.cpp:271-274`) refuses an
/// upgrade the building is not affected by. `Upgrade_ChinaRadar` is `Type = OBJECT`, cost 500,
/// `BuildTime = 20`; before the adapter kept the `RadarUpgrade` module this returned false (the AN5
/// f401 divergence), so the assertion is exactly the lane-144 finding. Researching it gives the
/// player radar and starts the command center's 120-frame dish extension
/// (`RadarUpgrade.cpp:128-143`, `RadarUpdate.cpp:66-83`).
#[test]
fn an5_china_command_center_researches_radar() {
    let Some(vfs) = vfs() else { return };
    let data = Arc::new(GameData::load(&vfs));
    // AN5 seed 1: GLA start 0 vs China start 1, both Normal.
    let setups = [
        PlayerSetup::ai("FactionGLA", 0).with_difficulty(AiDifficulty::Normal),
        PlayerSetup::ai("FactionChina", 1).with_difficulty(AiDifficulty::Normal),
    ];
    let mut g = Game::new_skirmish_with(data, &vfs, "Alpine Assault", &setups, 1).unwrap();
    let china = g.players().iter().find(|p| p.faction == "FactionChina").expect("the China slot").index;
    let cc = g
        .logic()
        .objects()
        .find(|o| o.player == china && o.template.name == "ChinaCommandCenter")
        .expect("the China command center")
        .id;
    let money_before = g.logic().player(china).unwrap().money;

    assert!(
        g.logic_mut().queue_upgrade_typed(china, cc, CHINA_RADAR, true),
        "{CHINA_RADAR} must be queueable on the ChinaCommandCenter (the RadarUpgrade makes it affected)"
    );
    assert_eq!(g.logic().player(china).unwrap().money, money_before - 500, "the research cost is withdrawn at queue time");
    assert!(!g.logic().player_has_radar(china), "radar only after the research completes");

    // BuildTime = 20 s = 600 frames; allow the AI's own frame timing.
    for _ in 0..780 {
        g.step();
        if g.logic().player_has_radar(china) {
            break;
        }
    }
    assert!(g.logic().player_has_radar(china), "the completed RadarUpgrade gives the player radar");
    let o = g.logic().find_object(cc).expect("the command center is still alive");
    assert!(o.applied_upgrades.contains("radar:false"), "the radar module's one-shot latch is set");
}
