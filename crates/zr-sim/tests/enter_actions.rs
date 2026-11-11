//! The `ActionManager` entry queries and the enter paths (`Command::Enter`), synthetic data only.
//!
//! Ports under test: `ActionManager::{canEnterObject, canHijackVehicle, canConvertObjectToCarBomb,
//! canSabotageBuilding, canCaptureBuilding}` (`Common/RTS/ActionManager.cpp`) and the
//! `CommandButtonHuntUpdate` enter hunt.

use std::sync::Arc;

use zr_sim::contain::{ContainKind, ContainTemplate, GarrisonFirePoints};
use zr_sim::enter_actions::EnterKind;
use zr_sim::modules2::{CrateCollideTemplate, CrateEffect};
use zr_sim::physics::PhysicsTemplate;
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::weapon::WEAPONSET_CARBOMB;
use zr_sim::{Command, Coord3D, EnterAction, GameLogic, ObjectTemplate, TerrainLogic};

fn flat() -> TerrainLogic {
    TerrainLogic::flat(200, 200, 0)
}

fn unit(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 8.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.physics = Some(PhysicsTemplate::default());
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    t
}

fn infantry(name: &str) -> ObjectTemplate {
    let mut t = unit(name);
    t.kind_of = kind_of::INFANTRY | kind_of::SELECTABLE;
    t.crush = zr_sim::modules2::CrushInfo { crusher_level: 0, crushable_level: 0, squish_collide: true };
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Legs, 30.0, 60.0, 120.0, 180.0));
    t.transport_slot_count = 1;
    t
}

fn contain(kind: ContainKind, capacity: i32) -> ContainTemplate {
    ContainTemplate {
        kind,
        capacity,
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
        heal_frames: 1.0,
        initial_payload: Vec::new(),
        tunnel_frames_for_full_heal: 1.0,
        is_enclosing_container: true,
        fire_points: GarrisonFirePoints::default(),
        passengers_in_turret: false,
        weapon_bonus_passed_to_passengers: false,
        immune_to_clear_building_attacks: false,
    }
}

fn transport(slots: i32) -> ObjectTemplate {
    let mut t = unit("Transport");
    t.kind_of |= kind_of::TRANSPORT;
    t.transport_slot_count = slots as u32;
    t.contain = Some(Arc::new(contain(ContainKind::Transport, slots)));
    t
}

/// A GLA Hijacker: an infantry carrying `ConvertToHijackedVehicleCrateCollide` on `VEHICLE`s.
fn hijacker() -> ObjectTemplate {
    let mut t = infantry("Hijacker");
    t.crate_collides = vec![CrateCollideTemplate {
        required_kind_of: kind_of::VEHICLE,
        effect: CrateEffect::ConvertToHijackedVehicle,
        ..Default::default()
    }];
    t
}

/// A GLA Terrorist: an infantry carrying `ConvertToCarBombCrateCollide` on `VEHICLE`s.
fn terrorist() -> ObjectTemplate {
    let mut t = infantry("Terrorist");
    t.crate_collides = vec![CrateCollideTemplate {
        required_kind_of: kind_of::VEHICLE,
        effect: CrateEffect::ConvertToCarBomb,
        ..Default::default()
    }];
    t
}

/// A GLA Saboteur: an infantry carrying the `Sabotage*CrateCollide`s.
fn saboteur() -> ObjectTemplate {
    let mut t = infantry("Saboteur");
    t.crate_collides = vec![
        CrateCollideTemplate { building_pickup: true, effect: CrateEffect::SabotagePowerPlant { power_sabotage_frames: 900 }, ..Default::default() },
        CrateCollideTemplate {
            building_pickup: true,
            effect: CrateEffect::SabotageSupplyCenter { steal_cash_amount: 1000 },
            ..Default::default()
        },
    ];
    t
}

/// A car that can be turned into a car bomb (`has_carbomb_weapon_set`).
fn carbombable(name: &str) -> ObjectTemplate {
    let mut t = unit(name);
    t.weapon_sets = vec![(WEAPONSET_CARBOMB, zr_sim::weapon::WeaponSetTemplate::default())];
    t
}

fn power_plant() -> ObjectTemplate {
    let mut t = unit("PowerPlant");
    t.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE | kind_of::FS_POWER | kind_of::POWERED;
    t.geometry.major_radius = 20.0;
    t.max_health = 500.0;
    t.initial_health = 500.0;
    t.physics = None;
    t.locomotor = None;
    t
}

/// `canEnterObject`: a plain container entry passes, and the client maps it to `EnterContainer`.
#[test]
fn can_enter_object_accepts_a_transport() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    let c = g.create_object(&Arc::new(transport(2)), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let u = g.create_object(&Arc::new(infantry("Ranger")), Coord3D::new(140.0, 100.0, 0.0), 0.0, 0);
    assert!(g.can_enter_object(u, c, true), "a transport is a valid container");
    assert_eq!(g.enter_action_for(u, c), Some(EnterAction::EnterContainer));
}

/// `canEnterObject`'s collide early-out: a Hijacker enters without a contain module on the target.
#[test]
fn can_enter_object_accepts_an_enter_crate_target() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    let h = g.create_object(&Arc::new(hijacker()), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let v = g.create_object(&Arc::new(unit("Scorpion")), Coord3D::new(140.0, 100.0, 0.0), 0.0, 1);
    assert!(g.can_enter_object(h, v, true), "the hijack collide wants to collide");
    assert_eq!(g.enter_action_for(h, v), Some(EnterAction::HijackVehicle));
}

/// `canHijackVehicle`: enemies only, vehicles only, never aircraft or drones.
#[test]
fn can_hijack_vehicle_filters_by_relationship_and_kind() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    let h = g.create_object(&Arc::new(hijacker()), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let enemy_car = g.create_object(&Arc::new(unit("Scorpion")), Coord3D::new(120.0, 100.0, 0.0), 0.0, 1);
    let ally_car = g.create_object(&Arc::new(unit("Scorpion")), Coord3D::new(130.0, 100.0, 0.0), 0.0, 0);
    let mut plane_t = unit("Plane");
    plane_t.kind_of |= kind_of::AIRCRAFT;
    let plane = g.create_object(&Arc::new(plane_t), Coord3D::new(140.0, 100.0, 0.0), 0.0, 1);
    let building = g.create_object(&Arc::new(power_plant()), Coord3D::new(160.0, 100.0, 0.0), 0.0, 1);

    assert!(g.can_hijack_vehicle(h, enemy_car));
    assert!(!g.can_hijack_vehicle(h, ally_car), "allies are not hijackable");
    assert!(!g.can_hijack_vehicle(h, plane), "aircraft are not hijackable");
    assert!(!g.can_hijack_vehicle(h, building), "structures are not hijackable");
}

/// `canConvertObjectToCarBomb`: neutral-or-enemy vehicle with a carbomb weapon set.
#[test]
fn can_convert_object_to_carbomb_needs_a_carbomb_weapon_set() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    let t = g.create_object(&Arc::new(terrorist()), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let car = g.create_object(&Arc::new(carbombable("CivilianCar")), Coord3D::new(120.0, 100.0, 0.0), 0.0, 1);
    let plain = g.create_object(&Arc::new(unit("Scorpion")), Coord3D::new(130.0, 100.0, 0.0), 0.0, 1);
    assert!(g.can_convert_object_to_carbomb(t, car));
    assert!(!g.can_convert_object_to_carbomb(t, plain), "no carbomb weapon set");
    assert_eq!(g.enter_action_for(t, car), Some(EnterAction::ConvertToCarBomb));
}

/// `canSabotageBuilding`: enemies only, and only a building the crate accepts.
#[test]
fn can_sabotage_building_filters_by_relationship_and_kind() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    let s = g.create_object(&Arc::new(saboteur()), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let enemy_plant = g.create_object(&Arc::new(power_plant()), Coord3D::new(130.0, 100.0, 0.0), 0.0, 1);
    let ally_plant = g.create_object(&Arc::new(power_plant()), Coord3D::new(150.0, 100.0, 0.0), 0.0, 0);
    assert!(g.can_sabotage_building(s, enemy_plant));
    assert!(!g.can_sabotage_building(s, ally_plant), "allies are not sabotageable");
    assert_eq!(g.enter_action_for(s, enemy_plant), Some(EnterAction::SabotageBuilding));
}

/// `Command::Enter` on a Hijacker takes an enemy vehicle (no weapon required).
#[test]
fn command_enter_hijacks_an_enemy_vehicle() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    let h = g.create_object(&Arc::new(hijacker()), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let v = g.create_object(&Arc::new(unit("Scorpion")), Coord3D::new(104.0, 100.0, 0.0), 0.0, 1);
    g.submit(0, Command::Enter { ids: vec![h], container: v });
    g.run_frames(2);
    assert!(g.find_object(h).is_none(), "the hijacker was consumed");
    assert_eq!(g.find_object(v).unwrap().player, 0, "the vehicle defected");
}

/// `Command::Enter` on a Terrorist converts a car to a car bomb (no weapon required).
#[test]
fn command_enter_converts_a_vehicle_to_a_carbomb() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    let t = g.create_object(&Arc::new(terrorist()), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let car = g.create_object(&Arc::new(carbombable("CivilianCar")), Coord3D::new(104.0, 100.0, 0.0), 0.0, 1);
    g.submit(0, Command::Enter { ids: vec![t], container: car });
    g.run_frames(2);
    assert!(g.find_object(t).is_none(), "the terrorist was consumed");
    let car = g.find_object(car).unwrap();
    assert_ne!(car.weapon_set_flags & WEAPONSET_CARBOMB, 0, "the car is a bomb");
    assert_eq!(car.player, 0, "the car now belongs to the terrorist's player");
}

/// `Command::Enter` on a Saboteur disables an enemy power plant (no weapon required).
#[test]
fn command_enter_sabotages_a_power_plant() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    let s = g.create_object(&Arc::new(saboteur()), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let p = g.create_object(&Arc::new(power_plant()), Coord3D::new(110.0, 100.0, 0.0), 0.0, 1);
    g.submit(0, Command::Enter { ids: vec![s], container: p });
    g.run_frames(2);
    assert!(g.find_object(s).is_none(), "the saboteur was consumed");
    assert!(g.find_object(p).unwrap().is_disabled_by_type(zr_sim::DisabledType::Underpowered), "the plant browned out");
}

/// `CommandButtonHuntUpdate::huntEnter`: an idle Hijacker with an enter hunt takes the nearest
/// enemy vehicle on its own.
#[test]
fn enter_hunt_hijacks_the_nearest_vehicle() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    let mut h = hijacker();
    h.command_button_hunt = Some(zr_sim::modules4::CommandButtonHuntTemplate { scan_frames: 1, scan_range: 500.0 });
    let hid = g.create_object(&Arc::new(h), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let v = g.create_object(&Arc::new(unit("Scorpion")), Coord3D::new(140.0, 100.0, 0.0), 0.0, 1);
    assert!(g.set_hunt_command(hid, zr_sim::modules4::HuntCommand::Enter { kind: EnterKind::HijackVehicle, object_type: String::new() }));
    g.run_frames(180);
    assert_eq!(g.find_object(v).unwrap().player, 0, "the hunt took the vehicle");
    assert!(g.find_object(hid).is_none(), "the hijacker was consumed");
}
