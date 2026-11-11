//! Containment leftovers ported from the C++ (`GameLogic/Object/Contain/*.cpp`): non-enclosing
//! garrisons' `STATION` bones, `KINDOF_NO_GARRISON`, really-damaged garrison rejection/ejection
//! and `WeaponBonusPassedToPassengers`. Synthetic data only.

use std::sync::Arc;

use zr_sim::body::DamageInfo;
use zr_sim::bonus::{condition, WeaponBonusField, WeaponBonusSet};
use zr_sim::contain::{ContainKind, ContainTemplate, GarrisonFirePoints};
use zr_sim::damage::DamageType;
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::weapon::WeaponTemplate;
use zr_sim::{Command, Coord3D, GameLogic, ObjectId, ObjectTemplate, TerrainLogic};

fn base_contain(kind: ContainKind) -> ContainTemplate {
    ContainTemplate {
        kind,
        capacity: 5,
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

fn structure(c: ContainTemplate) -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Structure");
    t.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE | kind_of::SELECTABLE;
    t.geometry.major_radius = 30.0;
    t.max_health = 1000.0;
    t.initial_health = 1000.0;
    t.contain = Some(Arc::new(c));
    t
}

fn infantry() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Infantry");
    t.kind_of = kind_of::INFANTRY | kind_of::SELECTABLE;
    t.geometry.major_radius = 4.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.transport_slot_count = 1;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Legs, 20.0, 60.0, 120.0, 180.0));
    t
}

fn world() -> GameLogic {
    GameLogic::new(TerrainLogic::flat(200, 200, 0), 1)
}

#[test]
fn non_enclosing_garrison_uses_station_bones() {
    let mut g = world();
    let mut c = base_contain(ContainKind::Garrison);
    c.is_enclosing_container = false;
    // North / east station bones (`loadStationGarrisonPoints`).
    c.fire_points = GarrisonFirePoints {
        station_points: vec![Coord3D::new(0.0, 30.0, 0.0), Coord3D::new(30.0, 0.0, 0.0)],
        ..GarrisonFirePoints::default()
    };
    let bld = g.create_object(&Arc::new(structure(c)), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let u1 = g.create_object(&Arc::new(infantry()), Coord3D::new(240.0, 200.0, 0.0), 0.0, 0);
    let u2 = g.create_object(&Arc::new(infantry()), Coord3D::new(200.0, 240.0, 0.0), 0.0, 0);
    g.submit(0, Command::Enter { ids: vec![u1, u2], container: bld });
    // AIEnterState contains a unit only after its move succeeds inside the container's radius
    // (AIStates.cpp:6390-6417); the infantry walk in from 40 units out.
    g.run_frames(300);
    assert!(g.is_contained(u1) && g.is_contained(u2));
    let p1 = g.find_object(u1).unwrap().position;
    let p2 = g.find_object(u2).unwrap().position;
    let stations = [Coord3D::new(200.0, 230.0, 0.0), Coord3D::new(230.0, 200.0, 0.0)];
    assert!(stations.contains(&p1), "u1 at a station, got {p1:?}");
    assert!(stations.contains(&p2), "u2 at a station, got {p2:?}");
    assert_ne!(p1, p2, "each passenger gets its own STATION bone");
}

#[test]
fn no_garrison_kind_is_rejected() {
    let mut g = world();
    let bld = g.create_object(&Arc::new(structure(base_contain(ContainKind::Garrison))), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let mut t = infantry();
    t.kind_of |= kind_of::NO_GARRISON;
    let u = g.create_object(&Arc::new(t), Coord3D::new(230.0, 200.0, 0.0), 0.0, 0);
    assert!(!g.is_valid_container_for(bld, u, false), "KINDOF_NO_GARRISON units cannot be garrisoned");
    g.submit(0, Command::Enter { ids: vec![u], container: bld });
    g.run_frames(30);
    assert!(!g.is_contained(u));
}

#[test]
fn really_damaged_garrison_rejects_and_ejects() {
    let mut g = world();
    let bld = g.create_object(&Arc::new(structure(base_contain(ContainKind::Garrison))), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let ranger = g.create_object(&Arc::new(infantry()), Coord3D::new(230.0, 200.0, 0.0), 0.0, 0);
    g.submit(0, Command::Enter { ids: vec![ranger], container: bld });
    // The ranger has to walk in (`AIEnterState::update`, AIStates.cpp:6390-6417).
    g.run_frames(300);
    assert!(g.is_contained(ranger));
    // 50/1000 health is BODY_REALLYDAMAGED (< the 0.1 threshold).
    g.damage_object(bld, DamageInfo { source: ObjectId::INVALID, amount: 950.0, kind: DamageType::Unresistable });
    g.run_frames(3);
    assert!(!g.is_contained(ranger), "a really-damaged garrison evacuates");
    let newcomer = g.create_object(&Arc::new(infantry()), Coord3D::new(230.0, 200.0, 0.0), 0.0, 0);
    assert!(!g.is_valid_container_for(bld, newcomer, true), "really-damaged garrisons reject new occupants");
}

#[test]
fn garrisonable_until_destroyed_survives_really_damaged() {
    let mut g = world();
    let mut c = base_contain(ContainKind::Garrison);
    c.is_enclosing_container = true;
    let mut t = structure(c);
    t.kind_of |= kind_of::GARRISONABLE_UNTIL_DESTROYED;
    let bld = g.create_object(&Arc::new(t), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let ranger = g.create_object(&Arc::new(infantry()), Coord3D::new(230.0, 200.0, 0.0), 0.0, 0);
    g.submit(0, Command::Enter { ids: vec![ranger], container: bld });
    g.run_frames(300);
    g.damage_object(bld, DamageInfo { source: ObjectId::INVALID, amount: 950.0, kind: DamageType::Unresistable });
    g.run_frames(3);
    assert!(g.is_contained(ranger), "GARRISONABLE_UNTIL_DESTROYED keeps its occupants");
}

#[test]
fn weapon_bonus_is_passed_to_passengers() {
    let mut g = world();
    let mut c = base_contain(ContainKind::Transport);
    c.weapon_bonus_passed_to_passengers = true;
    let mut t = ObjectTemplate::new("Transport");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 10.0;
    t.max_health = 200.0;
    t.initial_health = 200.0;
    t.contain = Some(Arc::new(c));
    // NATIONALISM is set only by `AIUpdateInterface::evaluateMoraleBonus` (`AIUpdate.cpp:4790-4824`), which
    // `HordeUpdate::update` calls; the container needs a horde module and an AI for the flag (`HordeUpdate.cpp:298-301`).
    t.ai_update = Some(1);
    t.horde = Some(zr_sim::modules2::HordeTemplate {
        update_rate: 1,
        kind_of: kind_of::INFANTRY,
        count: 1,
        radius: 100.0,
        rub_off_radius: 20.0,
        allies_only: true,
        exact_match: false,
        allowed_nationalism: true,
    });
    let bld = g.create_object(&Arc::new(t), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let u = g.create_object(&Arc::new(infantry()), Coord3D::new(215.0, 200.0, 0.0), 0.0, 0);
    g.submit(0, Command::Enter { ids: vec![u], container: bld });
    g.run_frames(200);
    assert!(g.is_contained(u));
    // The container's NATIONALISM flag comes from the horde evaluation and is passed to the rider.
    g.apply_player_upgrade(0, "Upgrade_Nationalism");
    g.set_global_bonuses(
        WeaponBonusSet { entries: vec![(condition::NATIONALISM, WeaponBonusField::Damage as u8, 2.0)] },
        [1.0; 4],
    );
    g.run_frames(3);
    let mut w = WeaponTemplate::new("Rifle");
    w.primary_damage = 10.0;
    w.attack_range = 100.0;
    let dmg = g.combined_weapon_bonus(u, &w).get(WeaponBonusField::Damage);
    assert!((dmg - 2.0).abs() < 1e-4, "rider inherits the container's bonus, got {dmg}");

    // A container without the flag does not pass it.
    let mut c2 = base_contain(ContainKind::Transport);
    c2.weapon_bonus_passed_to_passengers = false;
    let mut t2 = ObjectTemplate::new("Transport2");
    t2.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t2.geometry.major_radius = 10.0;
    t2.max_health = 200.0;
    t2.initial_health = 200.0;
    t2.contain = Some(Arc::new(c2));
    // Player 1 has no upgrade, so its rider gets no bonus.
    let bld2 = g.create_object(&Arc::new(t2), Coord3D::new(300.0, 200.0, 0.0), 0.0, 1);
    let u2 = g.create_object(&Arc::new(infantry()), Coord3D::new(315.0, 200.0, 0.0), 0.0, 1);
    g.submit(1, Command::Enter { ids: vec![u2], container: bld2 });
    g.run_frames(20);
    assert!(g.is_contained(u2));
    let dmg = g.combined_weapon_bonus(u2, &w).get(WeaponBonusField::Damage);
    assert!((dmg - 1.0).abs() < 1e-4, "no pass-through, got {dmg}");
}
