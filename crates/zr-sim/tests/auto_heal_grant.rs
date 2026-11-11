//! A TriggeredBy AutoHeal activates through a real upgrade grant: `give_upgrade` on the object, and
//! `apply_player_upgrade` on the player (`Player::addUpgrade`, which applies to every owned object). Both reach
//! `update_upgrade_modules`, which calls the activation first (upgrade.rs; `UpgradeMux::attemptUpgrade`,
//! UpgradeModule.cpp:111-150, and `upgradeImplementation`, AutoHealBehavior.h:143-145).

use std::sync::Arc;

use zr_sim::behaviors::AutoHealTemplate;
use zr_sim::body::DamageInfo;
use zr_sim::damage::DamageType;
use zr_sim::{Coord3D, GameLogic, ObjectId, ObjectTemplate, TerrainLogic};

fn gated_unit() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Gated");
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.auto_heal = vec![AutoHealTemplate {
        healing_amount: 10.0,
        healing_delay: 5,
        start_healing_delay: 0,
        radius: 0.0,
        affects_whole_player: false,
        skip_self_for_healing: false,
        single_burst: false,
        kind_of: 0,
        forbidden_kind_of: 0,
        starts_active: false,
        triggered_by: vec!["Upgrade_Test".to_owned()],
        conflicts_with: vec![],
    }];
    t
}

fn hurt_unit(g: &mut GameLogic) -> ObjectId {
    let id = g.create_object(&Arc::new(gated_unit()), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    g.damage_object(id, DamageInfo { source: ObjectId::INVALID, amount: 50.0, kind: DamageType::Explosion });
    id
}

/// `give_upgrade` is the object's own grant: the module activates, wakes for the next frame and heals.
/// Revert check: remove the `auto_heal_upgrade_event` call from `update_upgrade_modules` (upgrade.rs) and the
/// unit is never healed.
#[test]
fn a_triggered_auto_heal_activates_on_its_upgrade_grant() {
    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
    let id = hurt_unit(&mut g);
    let hurt = g.find_object(id).unwrap().health();
    g.run_frames(10);
    assert_eq!(g.find_object(id).unwrap().health(), hurt, "not active without its upgrade");
    g.give_upgrade(id, "Upgrade_Test");
    g.run_frames(10);
    assert!(g.find_object(id).unwrap().health() > hurt, "healed after the grant");
}

/// The player's upgrade reaches every owned object through `apply_player_upgrade` (`Player::addUpgrade`), and the
/// module activates from it too. Revert check: as above, the player path goes through the same call.
#[test]
fn a_triggered_auto_heal_activates_on_the_player_upgrade() {
    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
    let id = hurt_unit(&mut g);
    let hurt = g.find_object(id).unwrap().health();
    g.apply_player_upgrade(0, "Upgrade_Test");
    g.run_frames(10);
    assert!(g.find_object(id).unwrap().health() > hurt, "healed after the player's upgrade");
}
