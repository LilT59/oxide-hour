//! `GarrisonContain` fire-point assignment on synthetic data
//! (`GameLogic/Object/Contain/GarrisonContain.cpp`).

use std::sync::Arc;

use zr_sim::body::DamageInfo;
use zr_sim::contain::{ContainKind, ContainTemplate, GarrisonFirePoints};
use zr_sim::damage::DamageType;
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::weapon::{CombatTemplate, WeaponSetTemplate, WeaponTemplate};
use zr_sim::{Command, Coord3D, GameLogic, ObjectId, ObjectTemplate, TerrainLogic};

fn contain(capacity: i32, fire_points: GarrisonFirePoints) -> ContainTemplate {
    ContainTemplate {
        kind: ContainKind::Garrison,
        capacity,
        allow_inside_kind_of: None,
        forbid_inside_kind_of: 0,
        allow_allies_inside: true,
        allow_enemies_inside: true,
        allow_neutral_inside: true,
        passengers_allowed_to_fire: true,
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
        fire_points,
        passengers_in_turret: false,
        weapon_bonus_passed_to_passengers: false,
        immune_to_clear_building_attacks: false,
    }
}

fn garrison(fire_points: GarrisonFirePoints) -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Garrison");
    t.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE | kind_of::SELECTABLE;
    t.geometry.major_radius = 30.0;
    t.max_health = 1000.0;
    t.initial_health = 1000.0;
    t.vision_range = 300.0;
    t.contain = Some(Arc::new(contain(5, fire_points)));
    t
}

fn rifle() -> WeaponTemplate {
    let mut w = WeaponTemplate::new("Rifle");
    w.primary_damage = 20.0;
    w.damage_type = DamageType::SmallArms;
    w.attack_range = 100.0;
    w.min_delay_between_shots = 30;
    w.max_delay_between_shots = 30;
    w
}

fn rifle_infantry() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Infantry");
    t.kind_of = kind_of::INFANTRY | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
    t.geometry.major_radius = 4.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.transport_slot_count = 1;
    t.vision_range = 200.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Legs, 20.0, 60.0, 120.0, 180.0));
    let mut set = WeaponSetTemplate::default();
    set.weapons[0] = Some(Arc::new(rifle()));
    t.combat = CombatTemplate { weapon_set: Some(set), auto_acquire_enemies: true, ..CombatTemplate::default() };
    t
}

/// Four points N/E/S/W (index 0..3).
fn compass() -> GarrisonFirePoints {
    GarrisonFirePoints {
        conditions: [
            vec![
                Coord3D::new(0.0, 30.0, 0.0),
                Coord3D::new(30.0, 0.0, 0.0),
                Coord3D::new(0.0, -30.0, 0.0),
                Coord3D::new(-30.0, 0.0, 0.0),
            ],
            Vec::new(),
            Vec::new(),
        ],
        ..GarrisonFirePoints::default()
    }
}

fn world() -> GameLogic {
    GameLogic::new(TerrainLogic::flat(200, 200, 0), 1)
}

#[test]
fn passengers_take_the_point_closest_to_their_target() {
    let mut g = world();
    let bld = g.create_object(&Arc::new(garrison(compass())), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let u1 = g.create_object(&Arc::new(rifle_infantry()), Coord3D::new(230.0, 200.0, 0.0), 0.0, 0);
    let u2 = g.create_object(&Arc::new(rifle_infantry()), Coord3D::new(200.0, 230.0, 0.0), 0.0, 0);
    let east = g.create_object(&Arc::new(rifle_infantry()), Coord3D::new(270.0, 200.0, 0.0), 0.0, 1);
    let north = g.create_object(&Arc::new(rifle_infantry()), Coord3D::new(200.0, 270.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(0, Command::Enter { ids: vec![u1, u2], container: bld });
    g.run_frames(5);
    assert!(g.is_contained(u1) && g.is_contained(u2));
    g.submit(0, Command::Attack { ids: vec![u1], target: east });
    g.submit(0, Command::Attack { ids: vec![u2], target: north });
    g.run_frames(6);
    assert_eq!(g.find_object(u1).unwrap().position, Coord3D::new(230.0, 200.0, 0.0), "u1 takes the east point");
    assert_eq!(g.find_object(u2).unwrap().position, Coord3D::new(200.0, 230.0, 0.0), "u2 takes the north point");
}

#[test]
fn no_fire_points_falls_back_to_the_centre() {
    let mut g = world();
    let bld = g.create_object(&Arc::new(garrison(GarrisonFirePoints::default())), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let ranger = g.create_object(&Arc::new(rifle_infantry()), Coord3D::new(230.0, 200.0, 0.0), 0.0, 0);
    let enemy = g.create_object(&Arc::new(rifle_infantry()), Coord3D::new(200.0, 260.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(0, Command::Enter { ids: vec![ranger], container: bld });
    g.run_frames(5);
    assert!(g.is_contained(ranger));
    g.submit(0, Command::Attack { ids: vec![ranger], target: enemy });
    g.run_frames(60);
    let rp = g.find_object(ranger).unwrap().position;
    let bp = g.find_object(bld).unwrap().position;
    assert_eq!(rp, bp, "a garrison with no FIREPOINT bones fires from the centre");
    assert!(g.find_object(enemy).is_some_and(|e| e.health() < 100.0), "it can still shoot from the centre");
}

#[test]
fn damage_state_selects_its_condition_array() {
    let mut g = world();
    let damaged_points = GarrisonFirePoints {
        conditions: [Vec::new(), vec![Coord3D::new(0.0, 40.0, 0.0)], Vec::new()],
        ..GarrisonFirePoints::default()
    };
    let bld = g.create_object(&Arc::new(garrison(damaged_points)), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    // 300/1000 health: BODY_DAMAGED with the retail 0.5 / 0.1 thresholds.
    g.damage_object(bld, DamageInfo { source: ObjectId::INVALID, amount: 700.0, kind: DamageType::Unresistable });
    assert!(g.find_object(bld).unwrap().health() < 400.0);
    let ranger = g.create_object(&Arc::new(rifle_infantry()), Coord3D::new(230.0, 200.0, 0.0), 0.0, 0);
    let enemy = g.create_object(&Arc::new(rifle_infantry()), Coord3D::new(200.0, 260.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(0, Command::Enter { ids: vec![ranger], container: bld });
    g.run_frames(5);
    g.submit(0, Command::Attack { ids: vec![ranger], target: enemy });
    g.run_frames(6);
    assert_eq!(g.find_object(ranger).unwrap().position, Coord3D::new(200.0, 240.0, 0.0), "the damaged array's point is used");
}

#[test]
fn target_only_reachable_from_a_fire_point_is_engaged() {
    let mut g = world();
    let bld = g.create_object(&Arc::new(garrison(compass())), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let ranger = g.create_object(&Arc::new(rifle_infantry()), Coord3D::new(230.0, 200.0, 0.0), 0.0, 0);
    // 120 units north of the centre: out of the rifle's 100 range from the centre, but inside it
    // from the north fire point (30 + 90) — only `attemptBestFirePointPosition` can reach it.
    let enemy = g.create_object(&Arc::new(rifle_infantry()), Coord3D::new(200.0, 320.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(0, Command::Enter { ids: vec![ranger], container: bld });
    g.run_frames(5);
    assert!(g.is_contained(ranger));
    let hp = g.find_object(enemy).unwrap().health();
    g.run_frames(40);
    assert_eq!(g.find_object(ranger).unwrap().position, Coord3D::new(200.0, 230.0, 0.0), "fires from the north point");
    assert!(g.find_object(enemy).is_some_and(|e| e.health() < hp), "the edge reaches a target the centre cannot");
}

#[test]
fn passengers_are_reassigned_when_they_lose_their_target() {
    let mut g = world();
    let bld = g.create_object(&Arc::new(garrison(compass())), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let ranger = g.create_object(&Arc::new(rifle_infantry()), Coord3D::new(230.0, 200.0, 0.0), 0.0, 0);
    let enemy = g.create_object(&Arc::new(rifle_infantry()), Coord3D::new(200.0, 260.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(0, Command::Enter { ids: vec![ranger], container: bld });
    g.run_frames(5);
    g.submit(0, Command::Attack { ids: vec![ranger], target: enemy });
    g.run_frames(6);
    assert_eq!(g.find_object(ranger).unwrap().position, Coord3D::new(200.0, 230.0, 0.0));
    // The target dies: the passenger's attack ends and it returns to the centre.
    g.destroy_object(enemy);
    g.run_frames(3);
    assert_eq!(g.find_object(ranger).unwrap().position, g.find_object(bld).unwrap().position);
    // A new attack re-assigns it.
    let enemy2 = g.create_object(&Arc::new(rifle_infantry()), Coord3D::new(270.0, 200.0, 0.0), 0.0, 1);
    g.submit(0, Command::Attack { ids: vec![ranger], target: enemy2 });
    g.run_frames(6);
    assert_eq!(g.find_object(ranger).unwrap().position, Coord3D::new(230.0, 200.0, 0.0));
}
