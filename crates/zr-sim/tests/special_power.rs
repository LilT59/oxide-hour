//! Special powers and sciences, synthetic data only.

use std::collections::BTreeMap;
use std::sync::Arc;
use zr_sim::damage::DamageType;
use zr_sim::ocl::{DeliverPayloadTemplate, OclNugget, OclTemplate};
use zr_sim::science::{RankInfo, ScienceInfo};
use zr_sim::special_power::{SimSpecialPower, SimSpecialPowerTemplate, SpecialAbilityTemplate, SpecialPowerEffect, SpecialPowerTarget};
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::weapon::{CombatTemplate, WeaponSetTemplate, WeaponTemplate};
use zr_sim::{Command, Coord3D, GameLogic, ObjectId, ObjectTemplate, TerrainLogic};

fn flat() -> TerrainLogic {
    TerrainLogic::flat(100, 100, 0)
}

fn cannon(damage: f32) -> WeaponTemplate {
    let mut w = WeaponTemplate::new("Cannon");
    w.primary_damage = damage;
    w.damage_type = DamageType::Explosion;
    w.attack_range = 300.0;
    w.min_delay_between_shots = 1000;
    w.max_delay_between_shots = 1000;
    w
}

fn tank(name: &str, w: WeaponTemplate) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
    t.geometry.major_radius = 8.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    let mut s = WeaponSetTemplate::default();
    s.weapons[0] = Some(Arc::new(w));
    t.combat = CombatTemplate { weapon_set: Some(s), ..CombatTemplate::default() };
    t
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

#[test]
fn fire_weapon_power_fires_then_cooldowns() {
    let mut g = GameLogic::new(flat(), 1);
    install_power(&mut g, "P", 100, None);
    let mut a = tank("A", cannon(150.0));
    a.special_powers = vec![Arc::new(SimSpecialPower { power: "P".into(), effect: SpecialPowerEffect::FireWeapon, starts_paused: false, power_module: true })];
    let ia = g.create_object(&Arc::new(a), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(tank("B", cannon(0.0))), Coord3D::new(200.0, 100.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    // The constructor starts the 100-frame recharge, so the first use waits for it.
    g.run_frames(100);
    g.submit(0, Command::DoSpecialPower { source: ia, power: "P".into(), target: SpecialPowerTarget::Object(b) });
    g.run_frames(4);
    assert!(g.find_object(b).is_none(), "the power fires the weapon");
    assert!(g.special_power_ready_at(ia, "P") > g.frame() - 1, "the cooldown is set");
}

/// `Object::fireCurrentWeapon` (`Object.cpp:1500-1512`) releases the temporary weapon lock when the shot reloads
/// the clip. `FireWeaponPower` reloads every clip first (`FireWeaponPower.cpp:161`, `reloadAllAmmo(TRUE)` ->
/// `WeaponSet::reloadAllAmmo(obj, TRUE)` -> `Weapon::loadAmmoNow`), so the power's one shot starts from a full
/// clip and does not spend it: the temporary lock a previous order left on the slot survives the shot. (The
/// engine would then fire again from the AI attack order until `MaxShotsToFire`; the port fires once per use.)
#[test]
fn fire_weapon_power_reloads_the_clip_so_its_shot_does_not_spend_it() {
    let mut g = GameLogic::new(flat(), 1);
    install_power(&mut g, "P", 100, None);
    let mut gun = WeaponTemplate::new("Cannon");
    gun.primary_damage = 150.0;
    gun.damage_type = DamageType::Explosion;
    gun.attack_range = 300.0;
    gun.min_delay_between_shots = 1;
    gun.max_delay_between_shots = 1;
    gun.clip_size = 2;
    gun.auto_reloads_clip = true;
    gun.clip_reload_time = 1;
    let mut a = tank("A", gun.clone());
    a.special_powers = vec![Arc::new(SimSpecialPower { power: "P".into(), effect: SpecialPowerEffect::FireWeapon, starts_paused: false, power_module: true })];
    let ia = g.create_object(&Arc::new(a), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(tank("B", cannon(0.0))), Coord3D::new(200.0, 100.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    // A ground order with `max_shots` 1 leaves a temporary lock on slot 0 after its one shot (the clip is not spent).
    g.submit(0, Command::DoWeaponAtLocation { ids: vec![ia], slot: 0, pos: Coord3D::new(100.0, 300.0, 0.0), max_shots: 1 });
    g.run_frames(120);
    assert_eq!(g.find_object(ia).unwrap().combat.weapons.temporary_lock, Some(0));
    assert_eq!(g.find_object(ia).unwrap().combat.weapons.weapon(0).unwrap().ammo_in_clip(), 1);

    // `reloadAllAmmo(TRUE)` refills the clip before the power's shot, so the shot is not the reloading one.
    g.run_frames(100);
    g.submit(0, Command::DoSpecialPower { source: ia, power: "P".into(), target: SpecialPowerTarget::Object(b) });
    g.run_frames(4);
    assert!(g.find_object(b).is_none(), "the power fires the weapon");
    assert_eq!(g.find_object(ia).unwrap().combat.weapons.weapon(0).unwrap().ammo_in_clip(), 1, "reload 2 then fire one");
    assert_eq!(g.find_object(ia).unwrap().combat.weapons.temporary_lock, Some(0), "no reloaded shot, so the lock stays");
}

/// A strafing transport locks its strafing slot temporarily (`DeliverPayloadAIUpdate.cpp:224`) and each reloaded
/// shot releases it (`fireCurrentWeapon`, `Object.cpp:1519-1534`). A one-shot clip reloads on every shot, so the
/// lock never survives a strafing frame.
#[test]
fn a_strafing_transports_reloaded_shot_releases_its_temporary_lock() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    install_power(&mut g, "Strafe", 0, None);
    let mut bomb = WeaponTemplate::new("Bomb");
    bomb.primary_damage = 10.0;
    bomb.damage_type = DamageType::Explosion;
    bomb.attack_range = 300.0;
    bomb.min_delay_between_shots = 1;
    bomb.max_delay_between_shots = 1;
    bomb.clip_size = 1;
    bomb.auto_reloads_clip = true;
    bomb.clip_reload_time = 1;
    let mut set = WeaponSetTemplate::default();
    set.weapons[0] = Some(Arc::new(bomb));
    let mut transport = tank("Transport", WeaponTemplate::new("Unused"));
    transport.combat = CombatTemplate { weapon_set: Some(set), ..CombatTemplate::default() };
    let ocl = Arc::new(OclTemplate {
        name: "Strafe".into(),
        nuggets: vec![OclNugget::DeliverPayload(Arc::new(DeliverPayloadTemplate {
            transport: Some(Arc::new(transport.clone())),
            payload: Vec::new(),
            delivery_distance: 300.0,
            drop_delay: 5,
            delay_delivery_frames_max: 0,
            drop_offset: (0.0, 0.0, 0.0),
            drop_variance: (0.0, 0.0, 0.0),
            is_parachute_directly: false,
            self_destruct_object: false,
            fire_weapon: true,
            strafing_weapon_slot: 0,
            strafe_length: 0.0,
        }))],
    });
    transport.special_powers = vec![Arc::new(SimSpecialPower {
        power: "Strafe".into(),
        effect: SpecialPowerEffect::Ocl { ocl: Some(ocl), create_at_target: true, create_location: zr_sim::ocl::CreateLocation::AtLocation },
        starts_paused: false,
        power_module: true,
    })];
    let src = g.create_object(&Arc::new(transport), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    // The target is at x=600 so the start, `DeliveryDistance * 1.5` back along the approach (ObjectCreationList.cpp:348-352),
    // is on the map: a transport off the map once its exit is ordered is removed (DeliverPayloadAIUpdate.cpp:1177-1181).
    g.submit(0, Command::DoSpecialPower { source: src, power: "Strafe".into(), target: SpecialPowerTarget::Position(Coord3D::new(600.0, 100.0, 0.0)) });
    let mut last_shot = 0;
    for _ in 0..600 {
        g.run_frames(1);
        if let Some(o) = g.objects().find(|o| o.template.name == "Transport" && o.id != src) {
            assert_eq!(o.combat.weapons.temporary_lock, None, "a reloaded strafing shot releases the lock");
            last_shot = last_shot.max(o.combat.weapons.weapon(0).unwrap().last_fire_frame());
        }
    }
    assert!(last_shot > 0, "the transport strafed the target");
}

#[test]
fn cash_hack_steals_from_the_victim() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    g.add_player(1, 500);
    install_power(&mut g, "Hack", 0, None);
    let mut a = tank("A", cannon(0.0));
    a.special_powers =
        vec![Arc::new(SimSpecialPower { power: "Hack".into(), effect: SpecialPowerEffect::CashHack { amount: 100 }, starts_paused: false, power_module: true })];
    let ia = g.create_object(&Arc::new(a), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(tank("B", cannon(0.0))), Coord3D::new(200.0, 100.0, 0.0), 0.0, 1);
    g.submit(0, Command::DoSpecialPower { source: ia, power: "Hack".into(), target: SpecialPowerTarget::Object(b) });
    g.run_frames(2);
    assert_eq!(g.player(0).unwrap().money, 100);
    assert_eq!(g.player(1).unwrap().money, 400);
}

#[test]
fn required_science_gates_the_power() {
    let mut g = GameLogic::new(flat(), 1);
    install_power(&mut g, "P", 0, Some("Sci"));
    let mut a = tank("A", cannon(150.0));
    a.special_powers = vec![Arc::new(SimSpecialPower { power: "P".into(), effect: SpecialPowerEffect::FireWeapon, starts_paused: false, power_module: true })];
    let ia = g.create_object(&Arc::new(a), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(tank("B", cannon(0.0))), Coord3D::new(200.0, 100.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(0, Command::DoSpecialPower { source: ia, power: "P".into(), target: SpecialPowerTarget::Object(b) });
    g.run_frames(4);
    assert_eq!(g.find_object(b).unwrap().health(), 100.0, "without the science the power is refused");
    g.add_science(0, "Sci");
    g.submit(0, Command::DoSpecialPower { source: ia, power: "P".into(), target: SpecialPowerTarget::Object(b) });
    g.run_frames(4);
    assert!(g.find_object(b).is_none(), "with the science the power fires");
}

#[test]
fn rank_promotion_grants_purchase_points_and_sciences() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    let sciences = BTreeMap::from([(
        "Sci".to_string(),
        ScienceInfo { science_purchase_point_cost: 1, prerequisite_sciences: vec![], grantable: false },
    )]);
    let ranks = vec![
        RankInfo::default(),
        RankInfo { skill_points_needed: 10, sciences_granted: vec!["Granted".into()], science_purchase_points_granted: 1 },
    ];
    g.set_science_tables(sciences, ranks);
    assert!(!g.command_purchase_science(0, "Sci"), "no purchase points yet");
    g.add_skill_points(0, 10);
    assert!(g.player_has_science(0, "Granted"), "the rank grants its sciences");
    assert!(g.command_purchase_science(0, "Sci"), "the rank grants a purchase point");
    assert!(g.player_has_science(0, "Sci"));
}

#[test]
fn deliver_payload_spawns_a_transport_and_drops_its_payload() {
    let mut g = GameLogic::new(flat(), 1);
    install_power(&mut g, "Drop", 0, None);
    let dummy = Arc::new(tank("DropDummy", cannon(0.0)));
    let transport = Arc::new(tank("Transport", cannon(0.0)));
    let ocl = Arc::new(OclTemplate {
        name: "DropOcl".into(),
        nuggets: vec![OclNugget::DeliverPayload(Arc::new(DeliverPayloadTemplate {
            transport: Some(transport),
            payload: vec![(dummy, 2)],
            delivery_distance: 300.0,
            drop_delay: 5,
            delay_delivery_frames_max: 0,
            drop_offset: (0.0, 0.0, 0.0),
            drop_variance: (0.0, 0.0, 0.0),
            is_parachute_directly: false,
            self_destruct_object: false,
            fire_weapon: false,
            strafing_weapon_slot: 0,
            strafe_length: 0.0,
        }))],
    });
    let mut a = tank("A", cannon(0.0));
    a.special_powers = vec![Arc::new(SimSpecialPower {
        power: "Drop".into(),
        effect: SpecialPowerEffect::Ocl { ocl: Some(ocl), create_at_target: true, create_location: zr_sim::ocl::CreateLocation::AtLocation },
        starts_paused: false,
        power_module: true,
    })];
    let ia = g.create_object(&Arc::new(a), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let before = g.object_count();
    g.submit(0, Command::DoSpecialPower { source: ia, power: "Drop".into(), target: SpecialPowerTarget::Position(Coord3D::new(300.0, 300.0, 0.0)) });
    // The payload drops once the transport is within `DeliveryDistance` of the target (DeliverPayloadAIUpdate.cpp:357-375),
    // and it starts `DeliveryDistance * 1.5` before the target (ObjectCreationList.cpp:348-352), so the window covers that
    // approach (lane 185).
    g.run_frames(600);
    // The transport despawns; the two payload units remain.
    assert!(g.object_count() >= before + 2, "the payload should be dropped: {} -> {}", before, g.object_count());
}

fn install_ability(g: &mut GameLogic, name: &str, kind: &str) {
    g.set_special_powers(BTreeMap::from([(
        name.to_string(),
        Arc::new(SimSpecialPowerTemplate {
            name: name.to_string(),
            kind: kind.to_string(),
            reload_time: 0,
            required_science: None,
            shared_synced_timer: false,
            public_timer: false,
            radius_cursor_radius: 0.0,
        }),
    )]));
}

fn ability_unit(name: &str, power: &str, prep: u32) -> ObjectTemplate {
    let mut t = tank(name, cannon(0.0));
    // A unit ability is the pair the engine requires: the `SpecialAbility` power module (a `SpecialPowerModule`, which
    // owns the cooldown and pause count) and the `SpecialAbilityUpdate` module that
    // `SpecialPowerModule::initiateIntentToDoSpecialPower` drives (SpecialPowerModule.cpp:447-467,
    // `Object::getSpecialPowerUpdateInterface`).
    let ability = SpecialAbilityTemplate {
        power: power.to_string(),
        start_ability_range: 200.0,
        ability_abort_range: 10_000_000.0,
        approach_requires_los: false,
        preparation_frames: prep,
        pack_time: 0,
        unpack_time: 0,
        pack_unpack_variation: 0.0,
        skip_packing_with_no_target: false,
        award_xp_for_triggering: 0,
        skill_points_for_triggering: -1,
        effect_duration: 100,
        effect_value: 200,
        special_object: None,
        max_special_objects: 1,
        need_to_face: false,
        flee_range_after_completion: 0.0,
        starts_paused: false,
        power_module: true,
    };
    t.special_abilities = vec![
        Arc::new(ability.clone()),
        Arc::new(SpecialAbilityTemplate { power_module: false, ..ability }),
    ];
    t
}

#[test]
fn capture_building_changes_owner() {
    let mut g = GameLogic::new(flat(), 1);
    // Players 0 and 1 with their default teams: the capture's colleague check compares team instances (F6).
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.scripts_mut().set_player_name("p0", 0);
    g.scripts_mut().set_player_name("p1", 1);
    g.scripts_mut().load_default_team_protos(&[("teamP0", "p0"), ("teamP1", "p1")]);
    g.init_from_sides(&[("teamP0", 0), ("teamP1", 1)]);
    install_ability(&mut g, "Capture", "SPECIAL_BLACKLOTUS_CAPTURE_BUILDING");
    let a = g.create_object(&Arc::new(ability_unit("Lotus", "Capture", 0)), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(tank("Bldg", cannon(0.0))), Coord3D::new(150.0, 100.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(0, Command::DoSpecialPower { source: a, power: "Capture".into(), target: SpecialPowerTarget::Object(b) });
    g.run_frames(10);
    assert_eq!(g.find_object(b).unwrap().player, 0, "the building is captured");
}

#[test]
fn disable_hack_disables_a_vehicle() {
    let mut g = GameLogic::new(flat(), 1);
    install_ability(&mut g, "Disable", "SPECIAL_BLACKLOTUS_DISABLE_VEHICLE_HACK");
    let a = g.create_object(&Arc::new(ability_unit("Hacker", "Disable", 0)), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(tank("B", cannon(0.0))), Coord3D::new(150.0, 100.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(0, Command::DoSpecialPower { source: a, power: "Disable".into(), target: SpecialPowerTarget::Object(b) });
    g.run_frames(10);
    assert!(g.find_object(b).unwrap().is_disabled_by_type(zr_sim::disabled::DisabledType::Hacked), "the vehicle is hacked");
    assert!(g.find_object(b).unwrap().disabled_until() > g.frame(), "the vehicle is disabled");
}

#[test]
fn steal_cash_hack_takes_money() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    g.add_player(1, 500);
    install_ability(&mut g, "Steal", "SPECIAL_BLACKLOTUS_STEAL_CASH_HACK");
    let a = g.create_object(&Arc::new(ability_unit("Lotus", "Steal", 0)), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(tank("B", cannon(0.0))), Coord3D::new(150.0, 100.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(0, Command::DoSpecialPower { source: a, power: "Steal".into(), target: SpecialPowerTarget::Object(b) });
    g.run_frames(10);
    assert_eq!(g.player(0).unwrap().money, 200);
    assert_eq!(g.player(1).unwrap().money, 300);
}

#[test]
fn kill_pilot_makes_a_vehicle_unmanned() {
    use zr_sim::body::DamageInfo;
    let mut g = GameLogic::new(flat(), 1);
    let b = g.create_object(&Arc::new(tank("B", cannon(0.0))), Coord3D::new(100.0, 100.0, 0.0), 0.0, 1);
    g.damage_object_with_death(b, DamageInfo { source: ObjectId::INVALID, amount: 50.0, kind: DamageType::KillPilot }, zr_sim::damage::DeathType::Normal);
    let o = g.find_object(b).unwrap();
    assert!(o.unmanned(), "the driver was killed");
    assert_eq!(o.health(), 100.0, "the snipe does no health damage");
}
