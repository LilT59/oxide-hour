//! `SpecialAbilityUpdate` timings and aborts (`SpecialAbilityUpdate.cpp`), with synthetic data only.
//! A capture unit carries a `SpecialAbility` power module (no timings, `UpdateModuleStartsAttack`) and a
//! `SpecialAbilityUpdate` for the same power, as the shipped Ranger does.

use std::collections::BTreeMap;
use std::sync::Arc;
use zr_sim::contain::{ContainKind, ContainTemplate, GarrisonFirePoints};
use zr_sim::damage::DamageType;
use zr_sim::special_power::{SimSpecialPowerTemplate, SpecialAbilityTemplate, SpecialPowerTarget};
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::weapon::{CombatTemplate, WeaponSetTemplate, WeaponTemplate};
use zr_sim::{Command, Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

const POWER: &str = "Capture";

fn world() -> GameLogic {
    let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
    // Players 0 and 1 with their default teams: a capture's colleague check compares team instances (F6).
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.scripts_mut().set_player_name("p0", 0);
    g.scripts_mut().set_player_name("p1", 1);
    g.scripts_mut().load_default_team_protos(&[("teamP0", "p0"), ("teamP1", "p1")]);
    g.init_from_sides(&[("teamP0", 0), ("teamP1", 1)]);
    g
}

/// Installs the capture power with `reload` frames of recharge.
fn install_capture(g: &mut GameLogic, reload: u32) {
    g.set_special_powers(BTreeMap::from([(
        POWER.to_string(),
        Arc::new(SimSpecialPowerTemplate {
            name: POWER.to_string(),
            kind: "SPECIAL_INFANTRY_CAPTURE_BUILDING".to_string(),
            reload_time: reload,
            required_science: None,
            shared_synced_timer: false,
            public_timer: false,
            radius_cursor_radius: 0.0,
        }),
    )]));
}

fn vehicle(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
    t.geometry.major_radius = 8.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    let mut w = WeaponTemplate::new("Cannon");
    w.primary_damage = 0.0;
    w.damage_type = DamageType::Explosion;
    w.attack_range = 300.0;
    w.min_delay_between_shots = 1000;
    w.max_delay_between_shots = 1000;
    let mut s = WeaponSetTemplate::default();
    s.weapons[0] = Some(Arc::new(w));
    t.combat = CombatTemplate { weapon_set: Some(s), ..CombatTemplate::default() };
    t
}

/// A capture unit: the `SpecialAbility` power module (range and timings unset, so the engine defaults)
/// plus the `SpecialAbilityUpdate` with a 200 start range and the given unpack / preparation / pack frames.
fn capture_unit(name: &str, unpack: u32, prep: u32, pack: u32) -> ObjectTemplate {
    let module = SpecialAbilityTemplate {
        power: POWER.to_string(),
        start_ability_range: 10_000_000.0,
        ability_abort_range: 10_000_000.0,
        approach_requires_los: true,
        preparation_frames: 0,
        pack_time: 0,
        unpack_time: 0,
        pack_unpack_variation: 0.0,
        skip_packing_with_no_target: false,
        award_xp_for_triggering: 0,
        skill_points_for_triggering: -1,
        effect_duration: 0,
        effect_value: 1,
        special_object: None,
        max_special_objects: 1,
        need_to_face: true,
        flee_range_after_completion: 0.0,
        starts_paused: false,
        power_module: true,
    };
    let update = SpecialAbilityTemplate {
        start_ability_range: 200.0,
        preparation_frames: prep,
        unpack_time: unpack,
        pack_time: pack,
        power_module: false,
        ..module.clone()
    };
    let mut t = vehicle(name);
    t.special_abilities = vec![Arc::new(module), Arc::new(update)];
    t
}

fn capture_order(a: zr_sim::ObjectId, b: zr_sim::ObjectId) -> Command {
    Command::DoSpecialPower { source: a, power: POWER.into(), target: SpecialPowerTarget::Object(b) }
}

#[test]
fn update_module_timings_drive_the_capture() {
    // The power module carries no timings. Before the fix the preparation read 0 from it and the
    // capture fired in one frame. Here the unpack (30) and preparation (60) must both pass first.
    let mut g = world();
    install_capture(&mut g, 0);
    let a = g.create_object(&Arc::new(capture_unit("Ranger", 30, 60, 0)), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(vehicle("Bldg")), Coord3D::new(150.0, 100.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(0, capture_order(a, b));
    g.run_frames(60);
    assert_eq!(g.find_object(b).unwrap().player, 1, "still unpacking or preparing at frame 60");
    g.run_frames(60);
    assert_eq!(g.find_object(b).unwrap().player, 0, "captured once unpacked and prepared");
}

#[test]
fn a_move_order_aborts_the_capture() {
    // `ai->getLastCommandSource() != CMD_FROM_AI` -> `onExit(false)` (SpecialAbilityUpdate.cpp:~205).
    let mut g = world();
    install_capture(&mut g, 0);
    let a = g.create_object(&Arc::new(capture_unit("Ranger", 0, 200, 0)), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(vehicle("Bldg")), Coord3D::new(150.0, 100.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(0, capture_order(a, b));
    g.run_frames(20);
    assert!(g.find_object(a).unwrap().ability.is_some(), "the capture is preparing");
    g.submit(0, Command::MoveTo { ids: vec![a], pos: Coord3D::new(20.0, 20.0, 0.0) });
    g.run_frames(260);
    assert!(g.find_object(a).unwrap().ability.is_none(), "the move ended the capture");
    assert_eq!(g.find_object(b).unwrap().player, 1, "the building was not captured");
}

#[test]
fn infantry_capture_recharge_runs_while_preparing() {
    // `spmInterface->startPowerRecharge()` every frame of an infantry capture's preparation (:1192):
    // the ready frame keeps moving with the clock, so the power is not usable during the capture.
    let mut g = world();
    install_capture(&mut g, 100);
    let a = g.create_object(&Arc::new(capture_unit("Ranger", 0, 200, 0)), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(vehicle("Bldg")), Coord3D::new(150.0, 100.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    // The power starts recharging when the unit is created (`SpecialPowerModule` constructor), so the
    // first use waits for the 100-frame recharge.
    g.run_frames(110);
    g.submit(0, capture_order(a, b));
    g.run_frames(50);
    assert!(g.find_object(a).unwrap().ability.is_some(), "still preparing");
    let ready = g.special_power_ready_at(a, POWER);
    assert!(ready + 2 >= g.frame() + 100, "the recharge keeps running: ready {ready} at frame {}", g.frame());
    assert!(!g.special_power_is_ready(a, POWER));
}

#[test]
fn the_unit_packs_after_the_capture_triggers() {
    // `needToPack` after the trigger: `PackTime` runs before the ability ends (`handlePackingProcessing`).
    let mut g = world();
    install_capture(&mut g, 0);
    let a = g.create_object(&Arc::new(capture_unit("Ranger", 0, 10, 30)), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(vehicle("Bldg")), Coord3D::new(150.0, 100.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(0, capture_order(a, b));
    g.run_frames(15);
    assert_eq!(g.find_object(b).unwrap().player, 0, "captured after 10 frames of preparation");
    assert!(g.find_object(a).unwrap().ability.is_some(), "still packing");
    g.run_frames(60);
    assert!(g.find_object(a).unwrap().ability.is_none(), "packed and finished");
}

#[test]
fn a_garrisoned_building_ejects_its_garrison_instead_of_defecting() {
    // `TRIGGER_GARRISON_EJECT` (SpecialAbilityUpdate.cpp:~1428): the garrison leaves, the building stays.
    let mut g = world();
    install_capture(&mut g, 0);
    let contain = ContainTemplate {
        kind: ContainKind::Garrison,
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
    };
    let mut bld = ObjectTemplate::new("Garrisoned");
    bld.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE | kind_of::SELECTABLE;
    bld.geometry.major_radius = 30.0;
    bld.max_health = 1000.0;
    bld.initial_health = 1000.0;
    bld.contain = Some(Arc::new(contain));
    let b = g.create_object(&Arc::new(bld), Coord3D::new(200.0, 200.0, 0.0), 0.0, 1);
    let mut inf = vehicle("Guard");
    inf.kind_of = kind_of::INFANTRY | kind_of::SELECTABLE;
    inf.geometry.major_radius = 4.0;
    inf.transport_slot_count = 1;
    let guard = g.create_object(&Arc::new(inf), Coord3D::new(240.0, 200.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(1, Command::Enter { ids: vec![guard], container: b });
    g.run_frames(60);
    assert!(g.is_contained(guard), "the guard is inside before the capture");
    let a = g.create_object(&Arc::new(capture_unit("Ranger", 0, 0, 0)), Coord3D::new(150.0, 200.0, 0.0), 0.0, 0);
    g.submit(0, capture_order(a, b));
    g.run_frames(20);
    assert!(!g.is_contained(guard), "the garrison was ejected");
    assert_eq!(g.find_object(b).unwrap().player, 1, "the building did not defect");
}
