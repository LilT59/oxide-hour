//! Disabled-edge side effects: power adjustment, dozer task cancel, UNMANNED effects, slave
//! propagation and carbomb detonation. Synthetic data only.

use std::sync::Arc;

use zr_sim::damage::{DamageType, DeathType};
use zr_sim::disabled::DisabledType;
use zr_sim::experience::VeterancyLevel;
use zr_sim::modules2::{OverchargeTemplate, SpawnTemplate};
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::upgrade::{SimUpgradeModule, UpgradeEffect};
use zr_sim::weapon::WeaponSetTemplate;
use zr_sim::{body::DamageInfo, Coord3D, GameLogic, ObjectId, ObjectTemplate, TemplateSource, TerrainLogic};

struct FixedStore(Vec<Arc<ObjectTemplate>>);

impl TemplateSource for FixedStore {
    fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
        self.0.iter().find(|t| t.name == name).cloned()
    }
}

fn flat() -> TerrainLogic {
    TerrainLogic::flat(200, 200, 0)
}

fn vehicle(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 8.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    t
}

/// A structure that produces (`production > 0`) or consumes (`production < 0`) power.
fn energy_obj(name: &str, production: i32, bonus: i32, powered: bool) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::STRUCTURE | if powered { kind_of::POWERED } else { 0 };
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.energy_production = production;
    t.energy_bonus = bonus;
    t
}

#[test]
fn a_disabled_producer_stops_producing_power() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    let producer = g.create_object(&Arc::new(energy_obj("Producer", 10, 0, false)), Coord3D::new(0.0, 0.0, 0.0), 0.0, 0);
    g.create_object(&Arc::new(energy_obj("Consumer", -5, 0, true)), Coord3D::new(50.0, 0.0, 0.0), 0.0, 0);
    assert!(g.has_sufficient_power(0), "10 >= 5");
    g.set_disabled(producer, DisabledType::Emp);
    assert!(!g.has_sufficient_power(0), "a disabled producer contributes nothing (Object.cpp:3845)");
    g.clear_disabled(producer, DisabledType::Emp);
    assert!(g.has_sufficient_power(0), "re-enabling restores production");
}

#[test]
fn power_plant_upgrade_and_overcharge_add_the_energy_bonus() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    let mut plant = energy_obj("Plant", 5, 10, false);
    plant.upgrade_modules = vec![Arc::new(SimUpgradeModule {
        triggered_by: vec!["UpgControlRods".into()],
        conflicts_with: vec![],
        removes_upgrades: vec![],
        requires_all_triggers: false,
        effect: UpgradeEffect::PowerPlant,
    })];
    plant.overcharge = Some(OverchargeTemplate {
        health_percent_to_drain_per_second: 0.0,
        not_allowed_when_health_below_percent: 0.0,
        rods_extend_time: 0,
    });
    let plant = g.create_object(&Arc::new(plant), Coord3D::new(0.0, 0.0, 0.0), 0.0, 0);
    g.create_object(&Arc::new(energy_obj("Consumer", -22, 0, true)), Coord3D::new(50.0, 0.0, 0.0), 0.0, 0);
    assert!(!g.has_sufficient_power(0), "base 5 < 22");
    g.give_upgrade(plant, "UpgControlRods");
    assert!(!g.has_sufficient_power(0), "5 + 10 < 22");
    assert!(g.set_overcharge(plant, true));
    assert!(g.has_sufficient_power(0), "5 + 10 + 10 >= 22");
    assert!(g.set_overcharge(plant, false));
    assert!(!g.has_sufficient_power(0), "overcharge off removes its bonus");
}

#[test]
fn a_disabled_dozer_cancels_its_build_task() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 1000);
    let mut dozer = vehicle("Dozer");
    dozer.kind_of = kind_of::DOZER | kind_of::VEHICLE;
    let dozer = Arc::new(dozer);
    let mut bldg = ObjectTemplate::new("Bldg");
    bldg.kind_of = kind_of::STRUCTURE;
    bldg.max_health = 100.0;
    bldg.initial_health = 100.0;
    bldg.build_cost = 100;
    bldg.build_time_frames = 100;
    let bldg = Arc::new(bldg);
    g.set_template_source(Arc::new(FixedStore(vec![dozer.clone(), bldg.clone()])));
    let d = g.create_object(&dozer, Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    assert!(g.construct(d, "Bldg", Coord3D::new(120.0, 120.0, 0.0), 0.0).is_some());
    assert!(g.find_object(d).unwrap().build_target.is_some());
    g.set_disabled(d, DisabledType::Emp);
    assert!(g.find_object(d).unwrap().build_target.is_none(), "DozerAIUpdate::onDisabledEdge cancels the task");
}

#[test]
fn kill_pilot_resets_veterancy() {
    let mut g = GameLogic::new(flat(), 1);
    let mut t = vehicle("V");
    t.is_trainable = true;
    t.experience_required = [0, 10, 20, 30];
    let id = g.create_object(&Arc::new(t), Coord3D::new(0.0, 0.0, 0.0), 0.0, 1);
    g.set_veterancy_level(id, VeterancyLevel::Veteran);
    assert_eq!(g.find_object(id).unwrap().experience.level, VeterancyLevel::Veteran);
    g.damage_object_with_death(id, DamageInfo { source: ObjectId::INVALID, amount: 50.0, kind: DamageType::KillPilot }, DeathType::Normal);
    let o = g.find_object(id).unwrap();
    assert!(o.unmanned());
    assert_eq!(o.experience.level, VeterancyLevel::Regular, "Object.cpp:2233 resets veterancy");
    assert_eq!(o.experience.current, 0);
}

#[test]
fn a_disabled_spawner_disables_and_clears_its_slaves() {
    let mut g = GameLogic::new(flat(), 1);
    let mut site = ObjectTemplate::new("Site");
    site.kind_of = kind_of::STRUCTURE | kind_of::SPAWNS_ARE_THE_WEAPONS;
    site.max_health = 100.0;
    site.initial_health = 100.0;
    site.spawn = Some(SpawnTemplate {
        spawn_number: 1,
        spawn_replace_delay: 0,
        one_shot: false,
        spawn_template_names: vec!["Slave".into()],
        spawned_require_spawner: true,
        initial_burst: 0,
        aggregate_health: false,
    });
    let site = Arc::new(site);
    let slave = Arc::new(vehicle("Slave"));
    g.set_template_source(Arc::new(FixedStore(vec![site.clone(), slave.clone()])));
    let s = g.create_object(&site, Coord3D::new(0.0, 0.0, 0.0), 0.0, 0);
    g.run_frames(2);
    let slave_id = g.objects().find(|o| o.template.name == "Slave").map(|o| o.id).expect("slave spawned");
    // `SpawnBehavior::createSpawn` sets `setProducer(parent)` for every spawn (`SpawnBehavior.cpp:655`) but only calls
    // the enslave hook on a module that answers `getSlavedUpdateInterface` (`:658-666`); this synthetic slave has no
    // `SlavedUpdate`, so it has a producer, not a slaver.
    assert_eq!(g.find_object(slave_id).unwrap().producer_id, s);
    g.set_disabled(s, DisabledType::Emp);
    assert!(g.find_object(slave_id).unwrap().is_disabled_by_type(DisabledType::Emp), "orderSlavesDisabledUntil");
    g.clear_disabled(s, DisabledType::Emp);
    assert!(!g.find_object(slave_id).unwrap().is_disabled_by_type(DisabledType::Emp), "orderSlavesToClearDisabled");
}

#[test]
fn a_sniped_carbomb_detonates_instead_of_going_unmanned() {
    let mut g = GameLogic::new(flat(), 1);
    let mut t = vehicle("Carbomb");
    t.weapon_sets = vec![(zr_sim::weapon::WEAPONSET_CARBOMB, WeaponSetTemplate::default())];
    let id = g.create_object(&Arc::new(t), Coord3D::new(0.0, 0.0, 0.0), 0.0, 1);
    let shooter = g.create_object(&Arc::new(vehicle("Sniper")), Coord3D::new(50.0, 0.0, 0.0), 0.0, 0);
    g.damage_object_with_death(id, DamageInfo { source: shooter, amount: 50.0, kind: DamageType::KillPilot }, DeathType::Normal);
    assert!(g.find_object(id).is_none(), "the carbomb blows up instead of going unmanned");
}
