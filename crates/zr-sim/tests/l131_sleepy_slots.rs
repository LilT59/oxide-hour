//! Lane 131: the sleepy-heap slots of `CommandButtonHuntUpdate`, `SpecialAbilityUpdate` and
//! `AutoFindHealingUpdate`, and the wake rules their C++ modules have. Synthetic data only.
//!
//! C++: `GameEngine/Source/GameLogic/Object/Update/{CommandButtonHuntUpdate,SpecialAbilityUpdate,
//! AutoFindHealingUpdate}.cpp`, `GameLogic/System/GameLogic.cpp:4013-4046` (`registerObject`), `Object.cpp:300-410`
//! (the behaviour array is built in INI order).

use std::sync::Arc;

use zr_sim::contain::{ContainKind, ContainTemplate, GarrisonFirePoints};
use zr_sim::modules4::{CommandButtonHuntTemplate, HuntCommand};
use zr_sim::special_power::{SimSpecialPowerTemplate, SpecialAbilityTemplate};
use zr_sim::template::{kind_of, GeometryShape, LocomotorKind, LocomotorParams};
use zr_sim::{Coord3D, DamageInfo, DamageType, GameLogic, ObjectId, ObjectTemplate, TerrainLogic};

fn mobile(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.geometry.major_radius = 5.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 60.0, 100.0, 100.0, 360.0));
    t
}

fn infantry(name: &str) -> ObjectTemplate {
    let mut t = mobile(name);
    t.kind_of = kind_of::INFANTRY | kind_of::SELECTABLE;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Legs, 30.0, 60.0, 120.0, 180.0));
    t
}

fn hospital() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Hospital");
    t.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE | kind_of::HEAL_PAD;
    t.max_health = 500.0;
    t.initial_health = 500.0;
    t.geometry.shape = GeometryShape::Box;
    t.geometry.major_radius = 10.0;
    t.geometry.minor_radius = 10.0;
    t.contain = Some(Arc::new(ContainTemplate {
        // A heal pad is a `HealContain` (`GameLogic/Object/Contain/HealContain.cpp`), not an
        // `OpenContain` with `HealthRegen%PerSec` (OpenContainModuleData has no such field).
        kind: ContainKind::Heal,
        capacity: -1,
        allow_inside_kind_of: None,
        forbid_inside_kind_of: 0,
        allow_allies_inside: true,
        allow_enemies_inside: false,
        allow_neutral_inside: false,
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
        is_enclosing_container: false,
        fire_points: GarrisonFirePoints::default(),
        passengers_in_turret: false,
        weapon_bonus_passed_to_passengers: false,
        immune_to_clear_building_attacks: false,
    }));
    t
}

fn world() -> GameLogic {
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 0);
    g
}

fn ranger_with_healing() -> ObjectTemplate {
    let mut t = infantry("Ranger");
    t.auto_find_healing = Some(zr_sim::template::AutoFindHealingTemplate {
        scan_frames: 30,
        scan_range: 300.0,
        never_heal: 0.85,
        always_heal: 0.25,
    });
    t
}

fn damage(g: &mut GameLogic, id: ObjectId, amount: f32) {
    g.damage_object(id, DamageInfo { source: ObjectId::INVALID, amount, kind: DamageType::Unresistable });
}

/// One half of a `SpecialAbility` / `SpecialAbilityUpdate` pair (`SpecialAbilityUpdate.h:169`): the power module
/// carries the cooldown and pause count, the update module the approach/prep timings.
fn ability(power: &str, power_module: bool) -> Arc<SpecialAbilityTemplate> {
    Arc::new(SpecialAbilityTemplate {
        power: power.to_string(),
        start_ability_range: 300.0,
        ability_abort_range: 10_000_000.0,
        approach_requires_los: false,
        preparation_frames: 0,
        pack_time: 0,
        unpack_time: 0,
        pack_unpack_variation: 0.0,
        skip_packing_with_no_target: false,
        award_xp_for_triggering: 0,
        skill_points_for_triggering: -1,
        effect_duration: 100,
        effect_value: 0,
        special_object: None,
        max_special_objects: 1,
        need_to_face: false,
        flee_range_after_completion: 0.0,
        starts_paused: false,
        power_module,
    })
}

fn install_disable_power(g: &mut GameLogic) {
    g.set_special_powers(std::collections::BTreeMap::from([(
        "Disable".to_string(),
        Arc::new(SimSpecialPowerTemplate {
            name: "Disable".to_string(),
            kind: "SPECIAL_BLACKLOTUS_DISABLE_VEHICLE_HACK".to_string(),
            reload_time: 0,
            required_science: None,
            shared_synced_timer: false,
            public_timer: false,
            radius_cursor_radius: 0.0,
        }),
    )]));
}

/// `CommandButtonHuntUpdate::huntSpecialPower` returns `UPDATE_SLEEP_FOREVER` when
/// `findSpecialAbilityUpdate` is null (`CommandButtonHuntUpdate.cpp:207-208`): a unit with only the `SpecialAbility`
/// power module has no update module, so the hunt never scans or fires. Revert check: without the null check the port
/// scans and the victim is hacked (the old behaviour).
#[test]
fn a_hunt_without_a_special_ability_update_never_fires() {
    let mut g = world();
    install_disable_power(&mut g);
    let mut t = mobile("Hacker");
    t.command_button_hunt = Some(CommandButtonHuntTemplate { scan_frames: 1, scan_range: 200.0 });
    t.special_abilities = vec![ability("Disable", true)];
    let hacker = g.create_object(&Arc::new(t), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let victim = g.create_object(&Arc::new(mobile("Victim")), Coord3D::new(150.0, 100.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    assert!(g.set_hunt_command(hacker, HuntCommand::SpecialPower { power: "Disable".into(), object_type: String::new() }));
    g.run_frames(60);
    assert!(
        !g.find_object(victim).unwrap().is_disabled_by_type(zr_sim::disabled::DisabledType::Hacked),
        "no SpecialAbilityUpdate, no hunt"
    );
    assert!(g.find_object(hacker).unwrap().ability.is_none(), "and no ability intent was started");
}

/// The same hunt with the real pair (`SpecialAbility` + `SpecialAbilityUpdate`) starts the ability, whose module the
/// heap slot runs: the victim is hacked. Revert check: without the `SpecialAbilityUpdate` registration no slot exists,
/// so `initiateIntentToDoSpecialPower`'s wake has nothing to wake and the effect never runs.
#[test]
fn a_hunt_with_an_update_module_fires_the_ability() {
    let mut g = world();
    install_disable_power(&mut g);
    let mut t = mobile("Hacker");
    t.command_button_hunt = Some(CommandButtonHuntTemplate { scan_frames: 1, scan_range: 200.0 });
    t.special_abilities = vec![ability("Disable", true), ability("Disable", false)];
    let hacker = g.create_object(&Arc::new(t), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let victim = g.create_object(&Arc::new(mobile("Victim")), Coord3D::new(150.0, 100.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    assert!(g.set_hunt_command(hacker, HuntCommand::SpecialPower { power: "Disable".into(), object_type: String::new() }));
    g.run_frames(60);
    assert!(
        g.find_object(victim).unwrap().is_disabled_by_type(zr_sim::disabled::DisabledType::Hacked),
        "the hunt's ability fired"
    );
}

/// `AutoFindHealingUpdate::update` (`AutoFindHealingUpdate.cpp:102-147`): a hurt, idle computer player's infantry
/// scans for the closest `KINDOF_HEAL_PAD` object and enters it (`aiGetHealed` -> `privateGetHealed`, `AIUpdate.cpp:3744`
/// -> `aiEnter`); the `HealContain` slot heals and ejects it. Revert check: without the `AutoFindHealing` registration
/// the unit never leaves its position.
#[test]
fn an_ai_infantry_goes_to_a_heal_pad_when_hurt() {
    let mut g = world();
    g.add_ai_player(0, zr_sim::skirmish_ai::SkirmishAiConfig::default());
    let pad = g.create_object(&Arc::new(hospital()), Coord3D::new(200.0, 100.0, 0.0), 0.0, 0);
    let ranger = g.create_object(&Arc::new(ranger_with_healing()), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    damage(&mut g, ranger, 50.0);
    assert_eq!(g.find_object(ranger).unwrap().contained_by, None);
    let mut entered = false;
    for _ in 0..600 {
        g.update();
        if g.find_object(ranger).and_then(|o| o.contained_by) == Some(pad) {
            entered = true;
            break;
        }
    }
    assert!(entered, "the hurt infantry walked into the heal pad");
    g.run_frames(30);
    let ranger_obj = g.find_object(ranger).unwrap();
    assert_eq!(ranger_obj.health(), 100.0, "healed to full");
}

/// The engine skips the scan for a human player's unit (`getPlayerType() == PLAYER_HUMAN`,
/// `AutoFindHealingUpdate.cpp:104-108`). Revert check: without the player-type guard the unit walks to the pad too.
#[test]
fn a_human_players_unit_does_not_auto_find_healing() {
    let mut g = world();
    let pad = g.create_object(&Arc::new(hospital()), Coord3D::new(200.0, 100.0, 0.0), 0.0, 0);
    let ranger = g.create_object(&Arc::new(ranger_with_healing()), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    damage(&mut g, ranger, 50.0);
    for _ in 0..300 {
        g.update();
        assert_ne!(g.find_object(ranger).and_then(|o| o.contained_by), Some(pad), "a human unit never auto-heals");
    }
    assert!(g.find_object(ranger).unwrap().health() < 100.0, "still hurt");
}
