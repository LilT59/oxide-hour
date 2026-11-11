//! Lane 148 (wave 17): the upgrade modules `zr-game`'s adapter used to drop.
//!
//! `Upgrade_ChinaRadar` is the AN5/AN1 frame-401 divergence: `Object::affectedByUpgrade` only looks
//! at the object's upgrade modules, and the adapter kept none of `RadarUpgrade`, `SubObjectsUpgrade`,
//! `ReplaceObjectUpgrade`, `PassengersFireUpgrade`, `GrantScienceUpgrade` or `CostModifierUpgrade`.
//! These synthetic tests pin each module's effect in the sim (see `crates/zr-game/tests/l148_*` for
//! the adapter's conversion of the shipped data).

use std::sync::Arc;

use zr_sim::contain::{ContainKind, ContainTemplate, GarrisonFirePoints};
use zr_sim::template::{kind_of, SleepyModuleKind};
use zr_sim::upgrade::{SimUpgradeModule, UpgradeEffect, UpgradeInfo};
use zr_sim::{Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

fn world() -> GameLogic {
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 1000);
    g
}

fn upgrade_module(triggered_by: &str, effect: UpgradeEffect) -> Arc<SimUpgradeModule> {
    Arc::new(SimUpgradeModule { triggered_by: vec![triggered_by.into()], conflicts_with: vec![], removes_upgrades: vec![], requires_all_triggers: false, effect })
}

fn upgradable(effect: UpgradeEffect) -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new("Upgradable");
    t.kind_of = kind_of::STRUCTURE | kind_of::SELECTABLE;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.geometry.major_radius = 10.0;
    t.production = Some(zr_sim::production::ProductionTemplate { max_queue_entries: 5, ..Default::default() });
    t.upgrade_modules = vec![upgrade_module("Upgrade_X", effect)];
    Arc::new(t)
}

fn queue_info(g: &mut GameLogic) {
    g.set_upgrade_info(
        [("Upgrade_X".to_string(), UpgradeInfo { cost: 500, build_time_frames: 10 }), ("Upgrade_Y".to_string(), UpgradeInfo { cost: 500, build_time_frames: 10 })]
            .into_iter()
            .collect(),
    );
}

// ---- RadarUpgrade (`RadarUpgrade.cpp`) ------------------------------------------------------

/// `RadarUpgrade::upgradeImplementation` (`RadarUpgrade.cpp:128-143`) needs `affectedByUpgrade` to
/// accept the research (`queue_upgrade_typed` draws the 500), gives the player a radar and starts
/// the object's `RadarUpdate::extendRadar` (`RadarExtendTime` = 120 frames on the shipped command
/// centers). `RADAR_EXTENDING` is on until `update` passes the done frame, then `RADAR_UPGRADED`
/// (`RadarUpdate.cpp:93-123`).
#[test]
fn radar_upgrade_gives_the_player_radar_and_extends_the_dish() {
    let mut g = world();
    queue_info(&mut g);
    let mut t = (*upgradable(UpgradeEffect::Radar { disable_proof: false })).clone();
    t.update_module_order = vec![SleepyModuleKind::Radar];
    t.radar_extend_time = 120.0;
    let id = g.create_object(&Arc::new(t), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);

    assert!(!g.player_has_radar(0), "no radar before the upgrade");
    assert!(g.queue_upgrade_typed(0, id, "Upgrade_X", true), "the radar module makes the object affected by the upgrade");
    assert_eq!(g.player(0).unwrap().money, 500, "the research cost is withdrawn at queue time");

    g.give_upgrade(id, "Upgrade_X");
    assert!(g.player_has_radar(0), "the player owns a radar facility");
    assert_eq!(g.find_object(id).unwrap().behavior.radar_extend_done_frame, 120, "extendRadar sets frame + RadarExtendTime");
    g.update();
    let conditions = g.model_conditions(id).unwrap();
    assert!(conditions.test(zr_sim::condition::RADAR_EXTENDING));
    assert!(!conditions.test(zr_sim::condition::RADAR_UPGRADED));

    for _ in 0..130 {
        g.update();
    }
    let o = g.find_object(id).unwrap();
    assert!(o.behavior.radar_extend_complete, "the extension completes after the done frame");
    assert_eq!(o.behavior.radar_extend_done_frame, 0, "the done frame is cleared");
    let conditions = g.model_conditions(id).unwrap();
    assert!(!conditions.test(zr_sim::condition::RADAR_EXTENDING));
    assert!(conditions.test(zr_sim::condition::RADAR_UPGRADED));

    // `RadarUpgrade::onDelete` (`:72-92`) gives the radar back.
    g.destroy_object(id);
    assert!(!g.player_has_radar(0), "a destroyed radar facility takes its radar away");
}

#[test]
fn disable_proof_radar_survives_a_brownout() {
    let mut g = world();
    queue_info(&mut g);
    // A plain command center: the brownout hides its radar.
    let plain = g.create_object(&upgradable(UpgradeEffect::Radar { disable_proof: false }), Coord3D::new(120.0, 120.0, 0.0), 0.0, 0);
    g.give_upgrade(plain, "Upgrade_X");
    // A radar van: `DisableProof = Yes` keeps working during the brownout.
    let van = g.create_object(&upgradable(UpgradeEffect::Radar { disable_proof: true }), Coord3D::new(150.0, 150.0, 0.0), 0.0, 0);
    g.give_upgrade(van, "Upgrade_X");
    assert!(g.player_has_radar(0));

    // `Player::onPowerBrownOutChange` (`Player.cpp:3295-3302`) -> `disableRadar`: a consumer with no
    // producer drops the player's power below demand.
    let mut consumer = ObjectTemplate::new("PowerHungry");
    consumer.kind_of = kind_of::STRUCTURE;
    consumer.max_health = 100.0;
    consumer.initial_health = 100.0;
    consumer.energy_production = -10;
    let hungry = g.create_object(&Arc::new(consumer), Coord3D::new(250.0, 250.0, 0.0), 0.0, 0);
    g.update();
    // `Player::hasRadar` (Player.cpp:3270-3278): a `DisableProof` facility ignores `m_radarDisabled`.
    assert!(g.player_has_radar(0), "a disable-proof radar van keeps the minimap");

    g.destroy_object(van);
    assert!(!g.player_has_radar(0), "without the van the brownout hides the command center's radar");

    g.destroy_object(hungry);
    g.update();
    assert!(g.player_has_radar(0), "power restored, radar back (`enableRadar`)");
    g.destroy_object(plain);
    assert!(!g.player_has_radar(0), "removeRadar on delete");
}

// ---- GrantScienceUpgrade (`GrantScienceUpgrade.cpp`) ----------------------------------------

/// `GrantScienceUpgrade::upgradeImplementation` (`GrantScienceUpgrade.cpp:85-99`):
/// `Player::grantScience`, which refuses a science without `IsGrantable`
/// (`Player.cpp:2655-2664`), through `Player::addScience` (`Player.cpp:2565`).
#[test]
fn grant_science_upgrade_grants_only_a_grantable_science() {
    let mut g = world();
    g.set_science_tables(
        [
            ("SCIENCE_MOAB".to_string(), zr_sim::science::ScienceInfo { science_purchase_point_cost: 0, prerequisite_sciences: vec![], grantable: true }),
            ("SCIENCE_HIDDEN".to_string(), zr_sim::science::ScienceInfo { science_purchase_point_cost: 0, prerequisite_sciences: vec![], grantable: false }),
        ]
        .into_iter()
        .collect(),
        Vec::new(),
    );
    let mut t = (*upgradable(UpgradeEffect::GrantScience { science: "SCIENCE_MOAB".into() })).clone();
    t.upgrade_modules.push(upgrade_module("Upgrade_Y", UpgradeEffect::GrantScience { science: "SCIENCE_HIDDEN".into() }));
    let id = g.create_object(&Arc::new(t), Coord3D::new(150.0, 150.0, 0.0), 0.0, 0);

    assert!(!g.player_has_science(0, "SCIENCE_MOAB"));
    g.give_upgrade(id, "Upgrade_X");
    assert!(g.player_has_science(0, "SCIENCE_MOAB"), "the grantable science is granted");
    g.give_upgrade(id, "Upgrade_Y");
    assert!(!g.player_has_science(0, "SCIENCE_HIDDEN"), "isScienceGrantable refuses a non-grantable science");
}

// ---- CostModifierUpgrade (`CostModifierUpgrade.cpp`) ----------------------------------------

/// `CostModifierUpgrade::upgradeImplementation` adds the `(EffectKindOf, Percentage)` entry
/// (`Player::addKindOfProductionCostChange`, Player.cpp:3862-3886) that
/// `ThingTemplate::calcCostToBuild` multiplies in (`Core/.../ThingTemplate.cpp:1563-1572`,
/// `getProductionCostChangeBasedOnKindOf`, Player.cpp:3913-3932). The result is an `Int`
/// truncation (375 * 0.9 = 337.49 -> 337), and the refcount is per object: two oil refineries
/// stack, and `onDelete` removes one reference (`CostModifierUpgrade.cpp:118-135`).
#[test]
fn cost_modifier_upgrade_changes_calc_cost_to_build_by_kind_of() {
    let mut g = world();
    queue_info(&mut g);
    let mut t = (*upgradable(UpgradeEffect::CostModifier { kind_of: kind_of::VEHICLE, percentage: -0.1 })).clone();
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.build_cost = 375;
    let template = Arc::new(t);
    let id = g.create_object(&template, Coord3D::new(150.0, 150.0, 0.0), 0.0, 0);
    assert_eq!(g.calc_cost_to_build(0, &template), 375, "no modifier before the upgrade");

    g.give_upgrade(id, "Upgrade_X");
    assert_eq!(g.calc_cost_to_build(0, &template), 337, "375 * (1 - 0.1) truncates like the engine's Int return");

    let second = g.create_object(&template, Coord3D::new(200.0, 150.0, 0.0), 0.0, 0);
    g.give_upgrade(second, "Upgrade_X");
    // The list entry is refcounted but applied once (`Player::getProductionCostChangeBasedOnKindOf`
    // multiplies per *entry*, not per reference), so two oil refineries still give one 10% discount.
    assert_eq!(g.calc_cost_to_build(0, &template), 337, "one entry, refcount 2");
    assert_eq!(g.player(0).unwrap().production_cost_changes, vec![(kind_of::VEHICLE, -0.1, 2)]);

    g.destroy_object(second);
    assert_eq!(g.calc_cost_to_build(0, &template), 337, "one reference removed on delete");
    g.destroy_object(id);
    assert_eq!(g.calc_cost_to_build(0, &template), 375, "the last reference removes the entry");

    // The kind-of gate: a structure's own build cost is unchanged by a vehicle-only modifier.
    let mut structure = (*template).clone();
    structure.name = "Structure".into();
    structure.kind_of = kind_of::STRUCTURE;
    let structure = Arc::new(structure);
    let sid = g.create_object(&structure, Coord3D::new(250.0, 150.0, 0.0), 0.0, 0);
    g.give_upgrade(sid, "Upgrade_X");
    assert_eq!(g.calc_cost_to_build(0, &structure), 375, "EffectKindOf = Vehicle does not touch a structure");
}

// ---- PassengersFireUpgrade (`PassengersFireUpgrade.cpp`) -----------------------------------

/// `PassengersFireUpgrade::upgradeImplementation` (`PassengersFireUpgrade.cpp:52-66`) sets the
/// contain module's `m_passengerAllowedToFire`, which `OpenContain::isPassengerAllowedToFire`
/// (`OpenContain.cpp:1225-1237`) reads.
#[test]
fn passengers_fire_upgrade_lets_a_transports_infantry_shoot() {
    let mut g = world();
    queue_info(&mut g);
    let mut transport = ObjectTemplate::new("Transport");
    transport.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::TRANSPORT;
    transport.max_health = 200.0;
    transport.initial_health = 200.0;
    transport.geometry.major_radius = 10.0;
    transport.contain = Some(Arc::new(ContainTemplate {
        kind: ContainKind::Transport,
        capacity: 2,
        allow_inside_kind_of: None,
        forbid_inside_kind_of: 0,
        allow_allies_inside: true,
        allow_enemies_inside: true,
        allow_neutral_inside: true,
        passengers_allowed_to_fire: false,
        damage_percentage_to_units: 0.0,
        number_of_exit_paths: 1,
        exit_delay: 0,
        door_open_time: 1,
        scatter_nearby_on_exit: true,
        orient_like_container_on_exit: false,
        reset_mood_check_on_exit: false,
        health_regen: 0.0,
        do_i_heal_objects: false,
        heal_frames: 0.0,
        initial_payload: Vec::new(),
        tunnel_frames_for_full_heal: 1.0,
        is_enclosing_container: true,
        fire_points: GarrisonFirePoints::default(),
        passengers_in_turret: false,
        weapon_bonus_passed_to_passengers: false,
        immune_to_clear_building_attacks: false,
    }));
    transport.upgrade_modules = vec![upgrade_module("Upgrade_X", UpgradeEffect::PassengersFire)];
    let transport = Arc::new(transport);
    let id = g.create_object(&transport, Coord3D::new(150.0, 150.0, 0.0), 0.0, 0);

    let mut infantry = ObjectTemplate::new("Rifleman");
    infantry.kind_of = kind_of::INFANTRY | kind_of::SELECTABLE;
    infantry.max_health = 50.0;
    infantry.initial_health = 50.0;
    infantry.geometry.major_radius = 3.0;
    infantry.transport_slot_count = 1;
    infantry.locomotor = Some(zr_sim::LocomotorParams::from_ini(zr_sim::template::LocomotorKind::Legs, 20.0, 60.0, 120.0, 180.0));
    let soldier = g.create_object(&Arc::new(infantry), Coord3D::new(160.0, 150.0, 0.0), 0.0, 0);
    g.submit(0, zr_sim::Command::Enter { ids: vec![soldier], container: id });
    g.run_frames(200);
    assert!(g.is_contained(soldier), "the soldier boards the empty transport");

    assert!(!g.is_passenger_allowed_to_fire(id), "PassengersAllowedToFire = No before the upgrade");
    g.give_upgrade(id, "Upgrade_X");
    assert!(g.is_passenger_allowed_to_fire(id), "setPassengerAllowedToFire(TRUE)");
}

// ---- ReplaceObjectUpgrade (`ReplaceObjectUpgrade.cpp`) --------------------------------------

/// `ReplaceObjectUpgrade::upgradeImplementation` (`ReplaceObjectUpgrade.cpp:71-140`): the old object
/// is destroyed and the replacement is created at the same transform, team and owner, at full
/// health, with a fresh id (the engine allocates `newObject`'s id after the destroy).
#[test]
fn replace_object_upgrade_swaps_the_building_in_place() {
    let mut g = world();
    queue_info(&mut g);
    let mut real = ObjectTemplate::new("RealBuilding");
    real.kind_of = kind_of::STRUCTURE | kind_of::SELECTABLE;
    real.max_health = 500.0;
    real.initial_health = 500.0;
    real.geometry.major_radius = 10.0;
    let real = Arc::new(real);

    let mut fake = ObjectTemplate::new("FakeBuilding");
    fake.kind_of = kind_of::STRUCTURE | kind_of::SELECTABLE;
    fake.max_health = 100.0;
    fake.initial_health = 100.0;
    fake.geometry.major_radius = 10.0;
    fake.upgrade_modules = vec![upgrade_module("Upgrade_BecomeReal", UpgradeEffect::ReplaceObject { template: Some(real.clone()) })];
    let fake = Arc::new(fake);

    let old = g.create_object(&fake, Coord3D::new(321.5, 123.25, 0.0), 0.75, 0);
    g.set_object_team_key(old, "team");
    g.give_upgrade(old, "Upgrade_BecomeReal");

    assert!(g.find_object(old).is_none(), "the fake building is gone");
    let new_id = g.objects().find(|o| o.template.name == "RealBuilding").expect("the replacement exists").id;
    let new = g.find_object(new_id).unwrap();
    assert_eq!(new.position, Coord3D::new(321.5, 123.25, 0.0), "the engine copies the transform matrix");
    assert!((new.angle - 0.75).abs() < 1e-6);
    assert_eq!(new.player, 0);
    assert_eq!(new.team.as_deref(), Some("team"), "newObject(replacementTemplate, myTeam) keeps the team");
    assert_eq!(new.body.as_ref().map(|b| b.max_health()), Some(500.0), "full health from the new template");
}

// ---- SubObjectsUpgrade (`SubObjectsUpgrade.cpp`) -------------------------------------------

/// `SubObjectsUpgrade` is client-only (`SubObjectsUpgrade.cpp:85-130`): the sim needs the module only
/// so `Object::affectedByUpgrade` and the object upgrade mask see the upgrade (the bomb truck's
/// payload upgrades). It must be researchable and given, with no sim side effect.
#[test]
fn sub_objects_upgrade_is_carried_so_it_can_be_researched() {
    let mut g = world();
    queue_info(&mut g);
    let id = g.create_object(&upgradable(UpgradeEffect::SubObjects), Coord3D::new(150.0, 150.0, 0.0), 0.0, 0);
    assert!(g.queue_upgrade_typed(0, id, "Upgrade_X", true), "the module makes the object affected by the upgrade");
    g.give_upgrade(id, "Upgrade_X");
    assert!(g.find_object(id).unwrap().upgrades.contains("Upgrade_X"), "the upgrade lands in the object's mask");
    assert_eq!(g.find_object(id).unwrap().position, Coord3D::new(150.0, 150.0, 0.0), "nothing moves");
}
