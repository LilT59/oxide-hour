//! Round-3 modules: aura heal, overcharge, prone, transition FX, model-condition upgrade, keep-die,
//! and the jet airfield state machine. Synthetic data only.

use std::sync::Arc;

use zr_sim::body::DamageInfo;
use zr_sim::damage::DamageType;
use zr_sim::modules2::{OverchargeTemplate, ProneTemplate, PropagandaTemplate};
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::upgrade::{SimUpgradeModule, UpgradeEffect};
use zr_sim::{Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

fn flat() -> TerrainLogic {
    TerrainLogic::flat(100, 100, 0)
}

fn unit(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 8.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    t
}

/// `PropagandaTowerBehavior` heals a nearby allied unit.
#[test]
fn propaganda_heals_nearby_allies() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    let mut tower = unit("Tower");
    tower.locomotor = None;
    tower.propaganda = Some(PropagandaTemplate {
        radius: 200.0,
        delay_between_updates: 1,
        heal_percent_each_second: 1.0,
        upgraded_heal_percent_each_second: 2.0,
        upgrade_required: None,
        affects_self: false,
    });
    g.create_object(&Arc::new(tower), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let friend = unit("Friend");
    let fid = g.create_object(&Arc::new(friend), Coord3D::new(220.0, 200.0, 0.0), 0.0, 0);
    g.damage_object(fid, DamageInfo { source: zr_sim::ObjectId::INVALID, amount: 50.0, kind: DamageType::Explosion });
    let before = g.find_object(fid).unwrap().health();
    g.run_frames(31);
    assert!(g.find_object(fid).unwrap().health() > before, "the aura healed the ally");
}

/// `OverchargeBehavior` drains the power plant's health while overcharged.
#[test]
fn overcharge_drains_health() {
    let mut g = GameLogic::new(flat(), 1);
    let mut t = unit("Plant");
    t.overcharge = Some(OverchargeTemplate { health_percent_to_drain_per_second: 0.5, not_allowed_when_health_below_percent: 0.0, rods_extend_time: 0 });
    let id = g.create_object(&Arc::new(t), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    assert!(g.set_overcharge(id, true));
    let before = g.find_object(id).unwrap().health();
    g.run_frames(31);
    assert!(g.find_object(id).unwrap().health() < before, "overcharge drained health");
}

/// `ProneUpdate` adds prone frames on damage and counts them down.
#[test]
fn prone_frames_follow_damage() {
    let mut g = GameLogic::new(flat(), 1);
    let mut t = unit("Infantry");
    t.prone = Some(ProneTemplate { damage_to_frames_ratio: 1.0 });
    let id = g.create_object(&Arc::new(t), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    g.damage_object(id, DamageInfo { source: zr_sim::ObjectId::INVALID, amount: 20.0, kind: DamageType::SmallArms });
    assert_eq!(g.find_object(id).unwrap().behavior.prone_frames, 20);
    // ProneUpdate is a sleepy module registered at the creation frame, whose first update is the next
    // frame (`registerObject` clamps frame 0 to 1, GameLogic.cpp:4018-4045), so five updates from the
    // damage frame decrement it five times only after six frames.
    g.run_frames(6);
    assert_eq!(g.find_object(id).unwrap().behavior.prone_frames, 15);
}

/// `TransitionDamageFX` records its FX when the body state worsens.
#[test]
fn transition_damage_records_fx() {
    let mut g = GameLogic::new(flat(), 1);
    let mut t = unit("Smoker");
    t.transition_damage = Some(zr_sim::modules2::TransitionDamageTemplate {
        damaged_fx: vec!["FX_Damaged".into()],
        really_damaged_fx: vec!["FX_ReallyDamaged".into()],
        rubble_fx: vec!["FX_Rubble".into()],
    });
    let id = g.create_object(&Arc::new(t), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    g.damage_object(id, DamageInfo { source: zr_sim::ObjectId::INVALID, amount: 60.0, kind: DamageType::Explosion });
    assert!(g.fx_events().iter().any(|e| e.name == "FX_Damaged"), "recorded the damaged FX");
}

/// `ModelConditionUpgrade` ORs its bit into the model conditions.
#[test]
fn model_condition_upgrade_sets_bit() {
    let mut g = GameLogic::new(flat(), 1);
    let mut t = unit("Upgraded");
    t.upgrade_modules = vec![Arc::new(SimUpgradeModule {
        triggered_by: vec!["MC".into()],
        conflicts_with: vec![],
        removes_upgrades: vec![],
        requires_all_triggers: false,
        effect: UpgradeEffect::ModelCondition { flag: Some("MOVING".into()) },
    })];
    let id = g.create_object(&Arc::new(t), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    g.give_upgrade(id, "MC");
    g.update_upgrade_modules(id);
    g.run_frames(1);
    assert!(g.find_object(id).unwrap().model_conditions.test(zr_sim::condition::MOVING), "condition bit set");
}

/// `KeepObjectDie` leaves the object in place on death.
#[test]
fn keep_object_die_leaves_rubble() {
    let mut g = GameLogic::new(flat(), 1);
    let mut t = unit("Building");
    t.max_health = 50.0;
    t.initial_health = 50.0;
    t.keep_object_die = true;
    // A die module is needed so the fallback does not destroy it.
    t.die.push(zr_sim::die::DieModuleTemplate {
        mux: zr_sim::die::DieMux { death_types: u32::MAX, veterancy_levels: u32::MAX, exempt_status: 0, required_status: 0 },
        kind: zr_sim::die::DieTemplate::FxList { default_death_fx: None, orient_to_object: false, initially_active: false },
    });
    let id = g.create_object(&Arc::new(t), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    g.damage_object(id, DamageInfo { source: zr_sim::ObjectId::INVALID, amount: 100.0, kind: DamageType::Explosion });
    assert!(g.find_object(id).is_some(), "the rubble is kept");
}
