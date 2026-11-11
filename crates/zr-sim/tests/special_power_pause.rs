//! `SpecialPowerModule` pause and cooldown semantics (`StartsPaused`, `UnpauseSpecialPowerUpgrade`,
//! the disabled-edge `pauseAllSpecialPowers`, science wake-up). Synthetic data only.

use std::collections::BTreeMap;
use std::sync::Arc;

use zr_sim::disabled::DisabledType;
use zr_sim::special_power::{SimSpecialPower, SimSpecialPowerTemplate, SpecialPowerEffect, SpecialPowerTarget};
use zr_sim::template::kind_of;
use zr_sim::upgrade::{SimUpgradeModule, UpgradeEffect};
use zr_sim::{Command, Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

fn flat() -> TerrainLogic {
    TerrainLogic::flat(100, 100, 0)
}

fn install_power(g: &mut GameLogic, name: &str, reload: u32, science: Option<&str>) {
    let mut map = BTreeMap::new();
    map.insert(
        name.to_string(),
        Arc::new(SimSpecialPowerTemplate {
            name: name.to_string(),
            kind: String::new(),
            reload_time: reload,
            required_science: science.map(|s| s.to_string()),
            shared_synced_timer: false,
            public_timer: false,
            radius_cursor_radius: 0.0,
        }),
    );
    g.set_special_powers(map);
}

/// A unit with one `SpecialPowerModule` for `P`. The power pays 100 to its owner when fired, so a
/// fired power is visible in the owner's money.
fn unit(name: &str, starts_paused: bool, upgrades: Vec<SimUpgradeModule>) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::INFANTRY | kind_of::SELECTABLE;
    t.geometry.major_radius = 4.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.special_powers = vec![Arc::new(SimSpecialPower {
        power: "P".into(),
        effect: SpecialPowerEffect::CashHack { amount: 100 },
        starts_paused,
        power_module: true,
    })];
    t.upgrade_modules = upgrades.into_iter().map(Arc::new).collect();
    t
}

fn unpause_upgrade(trigger: &str) -> SimUpgradeModule {
    SimUpgradeModule {
        triggered_by: vec![trigger.into()],
        conflicts_with: Vec::new(),
        removes_upgrades: Vec::new(),
        requires_all_triggers: false,
        effect: UpgradeEffect::UnpauseSpecialPower { power: "P".into() },
    }
}

fn fire(g: &mut GameLogic, source: zr_sim::ObjectId) {
    g.submit(0, Command::DoSpecialPower { source, power: "P".into(), target: SpecialPowerTarget::None });
}

#[test]
fn starts_paused_power_is_unusable_until_the_trigger_upgrade() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    install_power(&mut g, "P", 100, None);
    let tmpl = Arc::new(unit("Ranger", true, vec![unpause_upgrade("Upgrade_U")]));
    let a = g.create_object(&tmpl, Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);

    // Created at frame 0: the constructor starts the recharge (ready at 100) and pauses (C++
    // SpecialPowerModule.cpp:116-132). Paused, it is not ready even after the reload.
    assert!(g.special_power_paused(a, "P"));
    assert_eq!(g.special_power_ready_at(a, "P"), 100);
    g.run_frames(150);
    assert!(!g.special_power_is_ready(a, "P"), "a paused power is never ready");
    fire(&mut g, a);
    g.run_frames(2);
    assert_eq!(g.player(0).unwrap().money, 0, "a paused power cannot be used (doSpecialPower returns)");

    // The trigger upgrade unpauses at frame 150 (UnpauseSpecialPowerUpgrade.cpp:92): the paused span
    // is added to the ready frame, so the cooldown is 100 frames from the unpause (150 + 100).
    let frame_before = g.frame();
    g.apply_player_upgrade(0, "Upgrade_U");
    assert!(!g.special_power_paused(a, "P"));
    // availableOnFrame (100) + (now - pausedOnFrame) = 100 + (frame - 0).
    assert_eq!(g.special_power_ready_at(a, "P"), 100 + frame_before);
    assert!(!g.special_power_is_ready(a, "P"), "unpaused, but the cooldown still runs");
    g.run_frames(100);
    assert!(g.special_power_is_ready(a, "P"));
    fire(&mut g, a);
    g.run_frames(2);
    assert_eq!(g.player(0).unwrap().money, 100, "the unpaused power fires");

    // A second application does not unpause again (UpgradeModule runs upgradeImplementation once).
    let ready = g.special_power_ready_at(a, "P");
    g.apply_player_upgrade(0, "Upgrade_U");
    assert_eq!(g.special_power_ready_at(a, "P"), ready);
}

#[test]
fn object_built_after_the_upgrade_starts_unpaused() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    install_power(&mut g, "P", 100, None);
    g.apply_player_upgrade(0, "Upgrade_U");
    g.run_frames(40);
    // `Object::updateUpgradeModules` runs at construction (Object.cpp:530): the new unit is unpaused
    // at once, and its recharge (from frame 40) is 100 frames.
    let tmpl = Arc::new(unit("Ranger", true, vec![unpause_upgrade("Upgrade_U")]));
    let b = g.create_object(&tmpl, Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);
    assert!(!g.special_power_paused(b, "P"));
    assert_eq!(g.special_power_ready_at(b, "P"), 140);
}

#[test]
fn disabled_edge_pauses_and_extends_the_cooldown() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    install_power(&mut g, "P", 100, None);
    let a = g.create_object(&Arc::new(unit("U", false, Vec::new())), Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);
    assert_eq!(g.special_power_ready_at(a, "P"), 100);
    g.run_frames(50);
    // A non-HELD disable pauses the module (Object.cpp:2166) at frame 50 for 60 frames.
    g.set_disabled_until(a, DisabledType::Emp, 110);
    assert!(g.special_power_paused(a, "P"));
    g.run_frames(30);
    assert!(!g.special_power_is_ready(a, "P"));
    fire(&mut g, a);
    g.run_frames(2);
    assert_eq!(g.player(0).unwrap().money, 0, "a disabled source cannot fire");
    g.run_frames(40);
    // Re-enabled at 110: the ready frame moves by the paused span, 100 + (110 - 50) = 160.
    assert!(!g.special_power_paused(a, "P"));
    assert_eq!(g.special_power_ready_at(a, "P"), 160);
    g.run_frames(40);
    assert!(g.special_power_is_ready(a, "P"));
}

#[test]
fn held_does_not_pause_the_power_but_still_blocks_it() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    install_power(&mut g, "P", 0, None);
    let a = g.create_object(&Arc::new(unit("U", false, Vec::new())), Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);
    g.set_disabled_until(a, DisabledType::Held, 1000);
    // `HELD` never pauses special powers (Object.cpp:2166), but Object::doSpecialPower refuses any
    // disabled object (Object.cpp:5427).
    assert!(!g.special_power_paused(a, "P"));
    fire(&mut g, a);
    g.run_frames(2);
    assert_eq!(g.player(0).unwrap().money, 0);
}

#[test]
fn science_grant_makes_the_power_ready_now() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    install_power(&mut g, "P", 1000, Some("Sci"));
    let a = g.create_object(&Arc::new(unit("U", false, Vec::new())), Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);
    assert_eq!(g.special_power_ready_at(a, "P"), 1000, "the constructor starts the recharge");
    g.run_frames(20);
    // `Player::addScience` wakes the module: onSpecialPowerCreation, then setReadyFrame(now) (Player.cpp:2585).
    g.add_science(0, "Sci");
    assert_eq!(g.special_power_ready_at(a, "P"), 20);
    assert!(g.special_power_is_ready(a, "P"));
}
