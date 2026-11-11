//! Object-object collisions and the `Collide` module families (synthetic data only).
//!
//! Ports under test: `PhysicsBehavior::onCollide` (`PhysicsUpdate.cpp:1155`), `SquishCollide`
//! (`SquishCollide.cpp:62`), `FireWeaponCollide` (`FireWeaponCollide.cpp:75`) and the
//! `CrateCollide` subclasses (`CrateCollide/*.cpp`).

use std::sync::Arc;

use zr_sim::die::{DieModuleTemplate, DieMux, DieTemplate, SlowDeathTemplate};
use zr_sim::disabled::DisabledType;
use zr_sim::modules2::{CrateCollideTemplate, CrateEffect, FireWeaponCollideTemplate};
use zr_sim::physics::PhysicsTemplate;
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::weapon::{WeaponTemplate, WEAPONSET_CRATEUPGRADE_ONE, WEAPONSET_CRATEUPGRADE_TWO};
use zr_sim::{Command, Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

fn flat() -> TerrainLogic {
    TerrainLogic::flat(200, 200, 0)
}

/// A mobile unit with a ground locomotor and physics.
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
    t
}

/// A cheap instant-hit weapon.
fn hit_weapon(damage: f32, range: f32) -> WeaponTemplate {
    let mut w = WeaponTemplate::new("Hit");
    w.primary_damage = damage;
    w.attack_range = range;
    w
}

/// Give a template a primary weapon so `Command::Attack` records an attack order (the sim's
/// `command_attack` requires a weapon; the engine's `aiAttackObject` does not).
fn armed(mut t: ObjectTemplate) -> ObjectTemplate {
    let mut set = zr_sim::weapon::WeaponSetTemplate::default();
    set.weapons[0] = Some(Arc::new(hit_weapon(1.0, 100.0)));
    t.combat.weapon_set = Some(set);
    t
}

/// A physics object with no locomotor, so no AI handle (`logic.rs` creates one only with a locomotor).
fn physics_only(name: &str) -> ObjectTemplate {
    let mut t = unit(name);
    t.locomotor = None;
    t
}

/// `PhysicsBehavior::onCollide`: two overlapping objects without AI are pushed apart.
#[test]
fn overlapping_non_ai_objects_are_pushed_apart() {
    let mut g = GameLogic::new(flat(), 1);
    let a = g.create_object(&Arc::new(physics_only("A")), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(physics_only("B")), Coord3D::new(101.0, 100.0, 0.0), 0.0, 1);
    g.run_frames(5);
    let pa = g.find_object(a).unwrap().position;
    let pb = g.find_object(b).unwrap().position;
    assert!(pa.dist_2d(&pb) > 1.5, "pushed apart: {pa:?} vs {pb:?}");
}

/// `AIUpdateInterface::processCollision` returns FALSE for an AI unit against another AI unit, so
/// `PhysicsBehavior::onCollide` never pushes it (`PhysicsUpdate.cpp:1271-1278`). Two idle units
/// standing on top of each other are not bounced apart by physics.
#[test]
fn overlapping_idle_ai_units_are_not_bounced_by_physics() {
    let mut g = GameLogic::new(flat(), 1);
    let a = g.create_object(&Arc::new(unit("A")), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(unit("B")), Coord3D::new(101.0, 100.0, 0.0), 0.0, 1);
    g.run_frames(5);
    let pa = g.find_object(a).unwrap().position;
    let pb = g.find_object(b).unwrap().position;
    assert!(pa.dist_2d(&pb) < 1.5, "not bounced apart: {pa:?} vs {pb:?}");
}

/// A `SlowDeath` die module: the body stays on the map for `frames` after it dies (the dead unit is
/// still a collision candidate, `PartitionManager.cpp:1982`).
fn lingers_when_dead(mut t: ObjectTemplate, frames: u32) -> ObjectTemplate {
    let sd = SlowDeathTemplate {
        probability_modifier: 1,
        modifier_bonus_per_overkill_percent: 0.0,
        sink_rate: 0.0,
        sink_delay: 0,
        sink_delay_variance: 0,
        destruction_delay: frames,
        destruction_delay_variance: 0,
        destruction_altitude: -10.0,
        fx: [Vec::new(), Vec::new(), Vec::new()],
        ocl: [Vec::new(), Vec::new(), Vec::new()],
        weapons: [Vec::new(), Vec::new(), Vec::new()],
        fling_force: 0.0,
        fling_force_variance: 0.0,
        fling_pitch: 0.0,
        fling_pitch_variance: 0.0,
        special: None,
    };
    let mux = DieMux { death_types: u32::MAX, veterancy_levels: u32::MAX, exempt_status: 0, required_status: 0 };
    t.die.push(DieModuleTemplate { mux, kind: DieTemplate::SlowDeath(Arc::new(sd)) });
    t
}

/// A hovering helicopter: an airborne AI unit (`LocomotorSet` air-only, `AIUpdate.cpp:2368`).
fn hover_aircraft(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::AIRCRAFT | kind_of::SELECTABLE | kind_of::VEHICLE;
    t.geometry.major_radius = 8.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.physics = Some(PhysicsTemplate::default());
    let mut hover = LocomotorParams::from_engine_units(LocomotorKind::Hover, 120.0 / 30.0, 0.0, 60.0 / 900.0, 240.0 / 900.0, 180.0f32.to_radians() / 30.0, false);
    hover.surfaces_air = true;
    hover.surfaces_ground = false;
    hover.lift = 2.0;
    hover.preferred_height = 60.0;
    hover.behavior_z = zr_sim::template::ZBehavior::SurfaceRelativeHeight;
    hover.allow_motive_force_while_airborne = true;
    hover.apply_2d_friction_when_airborne = true;
    t.locomotor = Some(hover);
    t
}

/// `PhysicsBehavior::onCollide` + `AIUpdateInterface::processCollision` (`AIUpdate.cpp:1427`): an air
/// AI unit returns FALSE (`isDoingGroundMovement` is false for air, `AIUpdate.cpp:2368`,
/// `JetAIUpdate.cpp:2423`), so two overlapping aircraft are not pushed apart.
#[test]
fn overlapping_air_ai_units_are_not_pushed_by_physics() {
    let mut g = GameLogic::new(flat(), 1);
    let a = g.create_object(&Arc::new(hover_aircraft("A")), Coord3D::new(100.0, 100.0, 60.0), 0.0, 0);
    let b = g.create_object(&Arc::new(hover_aircraft("B")), Coord3D::new(101.0, 100.0, 60.0), 0.0, 1);
    g.run_frames(5);
    let pa = g.find_object(a).unwrap().position;
    let pb = g.find_object(b).unwrap().position;
    assert!(pa.dist_2d(&pb) < 1.5, "air AI is not bounced apart: {pa:?} vs {pb:?}");
}

/// `PhysicsBehavior::onCollide` exception (`PhysicsUpdate.cpp:1271-1273`): a dead AI object colliding
/// with something immobile is pushed (here a dead vehicle against a structure) instead of ignored.
#[test]
fn dead_vehicle_is_bounced_off_an_immobile_structure() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    let mut wall = ObjectTemplate::new("Wall");
    wall.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;
    wall.geometry.major_radius = 10.0;
    wall.max_health = 1000.0;
    wall.initial_health = 1000.0;
    let w = g.create_object(&Arc::new(wall), Coord3D::new(100.0, 100.0, 0.0), 0.0, 1);
    // The structure is in the partition from an earlier update (the map's buildings are). The partition walks the
    // dirty list newest first (`PartitionManager.cpp:2810-2838`), so a vehicle created in the same frame as the wall
    // is gathered before the wall's cells exist and makes no contact in that frame (lane 181).
    g.update_partition();
    let v = g.create_object(&Arc::new(lingers_when_dead(unit("Dead"), 300)), Coord3D::new(102.0, 100.0, 0.0), 0.0, 0);
    g.damage_object(v, zr_sim::body::DamageInfo { source: zr_sim::ObjectId::INVALID, amount: 10_000.0, kind: zr_sim::damage::DamageType::Explosion });
    g.run_frames(3);
    let pv = g.find_object(v).expect("the dead vehicle is still on the map").position;
    let pw = g.find_object(w).unwrap().position;
    // A stationary body against an immobile structure gets the minimum bounce speed
    // (`PhysicsUpdate.cpp:1363-1366`, `1 / (LOGICFRAMES_PER_SECOND * 5)`), so it moves a little, away
    // from the wall. Without the exception the dead vehicle is ignored and does not move at all.
    assert!(pv.x > 102.0 && pv.dist_2d(&pw) > 2.0, "the dead vehicle was bounced off the structure: {pv:?} vs {pw:?}");
}

/// `AIUpdateInterface::processCollision` not-moving branch (`AIUpdate.cpp:1520-1527`): a dead infantry
/// that is not moving is pushed around by a squisher (`canCrushOrSquish(TEST_SQUISH_ONLY)`).
#[test]
fn dead_infantry_is_pushed_by_a_squisher() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    let mut tank = unit("Tank");
    tank.crush = zr_sim::modules2::CrushInfo { crusher_level: 1, crushable_level: 255, squish_collide: false };
    let t = g.create_object(&Arc::new(tank), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let i = g.create_object(&Arc::new(lingers_when_dead(infantry("Inf"), 300)), Coord3D::new(101.0, 100.0, 0.0), 0.0, 1);
    g.damage_object(i, zr_sim::body::DamageInfo { source: zr_sim::ObjectId::INVALID, amount: 10_000.0, kind: zr_sim::damage::DamageType::Explosion });
    g.run_frames(3);
    let pi = g.find_object(i).expect("the dead infantry is still on the map").position;
    let pt = g.find_object(t).unwrap().position;
    assert!(pi.dist_2d(&pt) > 1.5, "the dead infantry was pushed by the squisher: {pi:?} vs {pt:?}");
}

/// `SquishCollide` + `PhysicsBehavior::checkForOverlapCollision`: a moving tank crushes infantry.
#[test]
fn a_moving_tank_squishes_infantry() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    let mut tank = unit("Tank");
    tank.crush = zr_sim::modules2::CrushInfo { crusher_level: 1, crushable_level: 255, squish_collide: false };
    let t = g.create_object(&Arc::new(tank), Coord3D::new(50.0, 100.0, 0.0), 0.0, 0);
    let v = g.create_object(&Arc::new(infantry("Inf")), Coord3D::new(110.0, 100.0, 0.0), 0.0, 1);
    g.submit(0, Command::MoveTo { ids: vec![t], pos: Coord3D::new(180.0, 100.0, 0.0) });
    g.run_frames(90);
    assert!(g.find_object(v).is_none_or(|o| o.is_dead()), "the infantry was squished");
}

/// `FireWeaponCollide::shouldFireWeapon`: the tree only shoots while it is on fire.
#[test]
fn fire_weapon_collide_honours_the_required_status() {
    let mut g = GameLogic::new(flat(), 1);
    let mut tree = ObjectTemplate::new("Tree");
    tree.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;
    tree.geometry.major_radius = 6.0;
    tree.max_health = 100.0;
    tree.initial_health = 100.0;
    tree.fire_weapon_collide = Some(FireWeaponCollideTemplate {
        weapon: Arc::new(hit_weapon(10.0, 100.0)),
        fire_once: false,
        required_status: zr_sim::object::status::AFLAME,
        forbidden_status: 0,
    });
    let tid = g.create_object(&Arc::new(tree), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    // The unit collides with the tree, but the tree is not aflame, so it does not shoot.
    g.create_object(&Arc::new(unit("U")), Coord3D::new(102.0, 100.0, 0.0), 0.0, 1);
    g.run_frames(2);
    assert!(!g.find_object(tid).unwrap().behavior.fire_collide_fired, "not aflame, so no shot");
}

/// `CrateCollide::isValidToExecute`'s `RequiredKindOf` gate.
#[test]
fn crate_required_kindof_blocks_pickup() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    let mut crate_t = unit("Crate");
    crate_t.locomotor = None;
    crate_t.crate_collides = vec![CrateCollideTemplate {
        required_kind_of: kind_of::INFANTRY,
        effect: CrateEffect::Money { money_provided: 500, upgrade_boost: vec![] },
        ..Default::default()
    }];
    let cid = g.create_object(&Arc::new(crate_t), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    // A vehicle does not match `RequiredKindOf = INFANTRY`, so it cannot pick the crate up.
    g.create_object(&Arc::new(unit("Tank")), Coord3D::new(103.0, 100.0, 0.0), 0.0, 0);
    g.run_frames(2);
    assert!(g.find_object(cid).is_some(), "the vehicle was rejected");
    assert_eq!(g.player(0).unwrap().money, 0);
}

/// `SalvageCrateCollide`: a weapon salvager gains `WEAPONSET_CRATEUPGRADE_ONE`.
#[test]
fn salvage_crate_upgrades_a_weapon_salvager() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    let mut crate_t = unit("Crate");
    crate_t.locomotor = None;
    crate_t.crate_collides = vec![CrateCollideTemplate {
        effect: CrateEffect::Salvage { weapon_chance: 1.0, level_chance: 0.0, money_chance: 0.0, min_money: 0, max_money: 0 },
        ..Default::default()
    }];
    let cid = g.create_object(&Arc::new(crate_t), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let mut salvager = unit("Salvager");
    salvager.kind_of |= kind_of::SALVAGER | kind_of::WEAPON_SALVAGER;
    let sid = g.create_object(&Arc::new(salvager), Coord3D::new(103.0, 100.0, 0.0), 0.0, 0);
    g.run_frames(2);
    assert!(g.find_object(cid).is_none(), "the crate was consumed");
    assert_ne!(g.find_object(sid).unwrap().weapon_set_flags & WEAPONSET_CRATEUPGRADE_ONE, 0, "weapon upgrade gained");
    assert_eq!(g.find_object(sid).unwrap().weapon_set_flags & WEAPONSET_CRATEUPGRADE_TWO, 0);
}

/// `ConvertToHijackedVehicleCrateCollide`: a hijacker takes an enemy vehicle.
#[test]
fn hijacker_takes_an_enemy_vehicle() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    let mut hijacker = armed(unit("Hijacker"));
    hijacker.crate_collides = vec![CrateCollideTemplate { effect: CrateEffect::ConvertToHijackedVehicle, ..Default::default() }];
    let hid = g.create_object(&Arc::new(hijacker), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let vid = g.create_object(&Arc::new(unit("Vehicle")), Coord3D::new(103.0, 100.0, 0.0), 0.0, 1);
    // The engine requires the hijacker's AI goal to be the target (`ai->getGoalObject() == other`).
    g.submit(0, Command::Attack { ids: vec![hid], target: vid });
    g.run_frames(2);
    assert!(g.find_object(hid).is_none(), "the hijacker was consumed");
    assert_eq!(g.find_object(vid).unwrap().player, 0, "the vehicle defected");
}

/// `SabotagePowerPlantCrateCollide`: the target player browns out for the sabotage duration.
#[test]
fn saboteur_sabotages_a_power_plant() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    let mut saboteur = armed(unit("Saboteur"));
    saboteur.crate_collides = vec![CrateCollideTemplate {
        building_pickup: true,
        effect: CrateEffect::SabotagePowerPlant { power_sabotage_frames: 900 },
        ..Default::default()
    }];
    let sid = g.create_object(&Arc::new(saboteur), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let mut plant = ObjectTemplate::new("Plant");
    plant.kind_of = kind_of::STRUCTURE | kind_of::FS_POWER | kind_of::POWERED;
    plant.geometry.major_radius = 20.0;
    plant.max_health = 500.0;
    plant.initial_health = 500.0;
    let pid = g.create_object(&Arc::new(plant), Coord3D::new(110.0, 100.0, 0.0), 0.0, 1);
    g.submit(0, Command::Attack { ids: vec![sid], target: pid });
    g.run_frames(2);
    assert!(g.find_object(sid).is_none(), "the saboteur was consumed");
    assert!(g.find_object(pid).unwrap().is_disabled_by_type(DisabledType::Underpowered), "the plant browned out");
}

/// `SabotageSupplyCenterCrateCollide`: the saboteur steals cash from the target.
#[test]
fn saboteur_steals_cash_from_a_supply_center() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    g.add_player(1, 5000);
    g.set_enemies(0, 1);
    let mut saboteur = armed(unit("Saboteur"));
    saboteur.crate_collides = vec![CrateCollideTemplate {
        building_pickup: true,
        effect: CrateEffect::SabotageSupplyCenter { steal_cash_amount: 1000 },
        ..Default::default()
    }];
    let sid = g.create_object(&Arc::new(saboteur), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let mut center = ObjectTemplate::new("Center");
    center.kind_of = kind_of::STRUCTURE;
    center.geometry.major_radius = 20.0;
    center.max_health = 500.0;
    center.initial_health = 500.0;
    center.collide_kinds = zr_sim::template::collide_kind::FS_SUPPLY_CENTER;
    let cid = g.create_object(&Arc::new(center), Coord3D::new(110.0, 100.0, 0.0), 0.0, 1);
    g.submit(0, Command::Attack { ids: vec![sid], target: cid });
    g.run_frames(2);
    assert_eq!(g.player(0).unwrap().money, 1000, "the saboteur's player got the cash");
    assert_eq!(g.player(1).unwrap().money, 4000, "the victim lost the cash");
}

/// `HijackerUpdate` (`HijackerUpdate.cpp`): a hijacker riding a vehicle that ejects a pilot stays
/// hidden with it and is ejected when the vehicle dies.
#[test]
fn hijacker_rides_and_ejects() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    let mut hijacker = armed(unit("Hijacker"));
    hijacker.crate_collides = vec![CrateCollideTemplate { effect: CrateEffect::ConvertToHijackedVehicle, ..Default::default() }];
    hijacker.hijacker = Some(zr_sim::template::HijackerTemplate { attach_to_bone: String::new(), parachute: None });
    let hid = g.create_object(&Arc::new(hijacker), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let mut vehicle = unit("Vehicle");
    // EjectPilotDie present: `targetCanEject` lets the hijacker ride.
    vehicle.eject_pilot = Some(zr_sim::modules2::EjectPilotTemplate { air_ocl: None, ground_ocl: None });
    let vid = g.create_object(&Arc::new(vehicle), Coord3D::new(103.0, 100.0, 0.0), 0.0, 1);
    g.submit(0, Command::Attack { ids: vec![hid], target: vid });
    g.run_frames(2);
    assert_eq!(g.find_object(vid).unwrap().player, 0, "the vehicle defected");
    let h = g.find_object(hid).expect("the hijacker survives the hijack");
    assert_eq!(h.behavior.hijacker_vehicle, Some(vid), "the hijacker rides the vehicle");
    // The vehicle dies; the hijacker is ejected in place and is selectable again.
    g.destroy_object(vid);
    g.run_frames(2);
    let h = g.find_object(hid).expect("the hijacker is ejected");
    assert_eq!(h.behavior.hijacker_vehicle, None, "the ride ended");
    assert!(h.behavior.hijacker_eject_pos.dist_2d(&Coord3D::new(103.0, 100.0, 0.0)) < 1.0, "ejected at the vehicle");
}

/// The collision pass is deterministic: identical worlds give identical CRCs.
#[test]
fn collision_pass_is_deterministic() {
    let run = || {
        let mut g = GameLogic::new(flat(), 7);
        g.add_player(0, 0);
        g.add_player(1, 0);
        g.set_enemies(0, 1);
        let a = g.create_object(&Arc::new(unit("A")), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let b = g.create_object(&Arc::new(unit("B")), Coord3D::new(101.0, 100.0, 0.0), 0.0, 1);
        g.submit(0, Command::MoveTo { ids: vec![a], pos: Coord3D::new(180.0, 100.0, 0.0) });
        let _ = b;
        g.run_frames(40);
        g.crc()
    };
    assert_eq!(run(), run());
}
