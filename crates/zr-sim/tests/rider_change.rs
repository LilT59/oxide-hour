//! `RiderChangeContain` (combat bike) through the command path: a listed rider walks up and boards
//! (`Command::Enter`), an unlisted infantry does not. Also the `CommandSet` override, the reveal of a
//! stealthed bike on boarding, and a stealth grant passing from the bike to its rider. Synthetic data
//! only; the state checks are in `zr-sim/src/rider.rs` unit tests.

use std::sync::Arc;

use zr_sim::behaviors::StealthTemplate;
use zr_sim::condition::bit_for_name;
use zr_sim::contain::{ContainKind, ContainTemplate, GarrisonFirePoints};
use zr_sim::object::status;
use zr_sim::rider::{RiderChangeTemplate, RiderSlot};
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams, LocomotorSet};
use zr_sim::weapon::{WEAPONSET_RIDER1, WEAPONSET_RIDER2};
use zr_sim::{Command, Coord3D, GameLogic, ObjectId, ObjectTemplate, TerrainLogic};

fn rider_slot(name: &str, model: &str, weapon: u32, st: u64, command_set: &str) -> RiderSlot {
    RiderSlot {
        template_name: name.into(),
        model_condition: bit_for_name(model),
        weapon_set_flag: weapon,
        object_status: st,
        locomotor_set: LocomotorSet::Normal,
        command_set: command_set.into(),
    }
}

/// A combat bike with a rider table: a Rebel (command set `CycleRebel`) and a Terrorist with no
/// command set of its own (`""`, so the override is empty). `stealth` is the bike's `StealthUpdate`.
fn bike_with(stealth: Option<StealthTemplate>) -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Bike");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 8.0;
    t.max_health = 200.0;
    t.initial_health = 200.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    t.contain = Some(Arc::new(ContainTemplate {
        kind: ContainKind::Transport,
        capacity: 1,
        allow_inside_kind_of: Some(kind_of::INFANTRY),
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
        is_enclosing_container: false,
        fire_points: GarrisonFirePoints::default(),
        passengers_in_turret: false,
        weapon_bonus_passed_to_passengers: false,
        immune_to_clear_building_attacks: false,
    }));
    t.rider_change = Some(Arc::new(RiderChangeTemplate {
        riders: vec![
            rider_slot("Rebel", "RIDER1", WEAPONSET_RIDER1, status::RIDER1, "CycleRebel"),
            rider_slot("Terrorist", "RIDER2", WEAPONSET_RIDER2, status::RIDER2, ""),
        ],
        scuttle_frames: 30,
        scuttle_state: bit_for_name("TOPPLED"),
    }));
    t.stealth = stealth;
    t
}

fn bike() -> ObjectTemplate {
    bike_with(None)
}

/// A `StealthUpdate` with the given delay. Innate stealth is on from the first update; a granted one
/// waits for `receiveGrant`.
fn stealth(innate: bool, delay: u32) -> StealthTemplate {
    StealthTemplate {
        stealth_delay: delay,
        move_threshold_speed: 0.0,
        forbidden_conditions: 0,
        required_status: 0,
        forbidden_status: 0,
        innate_stealth: innate,
        granted_by_special_power: !innate,
        disguises_as_team: false,
        reveal_distance_from_target: 0.0,
        order_idle_enemies_to_attack_on_reveal: false,
        use_rider_stealth: false,
    }
}

/// `stealth(innate, delay)` with `UseRiderStealth = Yes`: the rider's rules drive the bike.
fn rider_stealth(innate: bool, delay: u32) -> StealthTemplate {
    StealthTemplate { use_rider_stealth: true, ..stealth(innate, delay) }
}

fn infantry(name: &str, stealth: Option<StealthTemplate>) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::INFANTRY | kind_of::SELECTABLE;
    t.geometry.major_radius = 4.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Legs, 20.0, 60.0, 120.0, 180.0));
    t.transport_slot_count = 1;
    t.stealth = stealth;
    t
}

/// Orders `rider` to board `bike` and runs until it is inside.
fn board(g: &mut GameLogic, rider: ObjectId, bike: ObjectId) {
    g.submit(0, Command::Enter { ids: vec![rider], container: bike });
    for _ in 0..300 {
        if g.container_of(rider) == Some(bike) {
            return;
        }
        g.run_frames(1);
    }
    panic!("the rider never boarded");
}

fn override_of(g: &GameLogic, b: ObjectId) -> Option<String> {
    g.find_object(b).and_then(|o| o.command_set_override()).map(str::to_owned)
}

#[test]
fn a_listed_rider_boards_the_bike_and_an_unlisted_one_does_not() {
    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
    let b = g.create_object(&Arc::new(bike()), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    let rebel = g.create_object(&Arc::new(infantry("Rebel", None)), Coord3D::new(60.0, 50.0, 0.0), 0.0, 0);
    let ranger = g.create_object(&Arc::new(infantry("Ranger", None)), Coord3D::new(50.0, 60.0, 0.0), 0.0, 0);
    g.submit(0, Command::Enter { ids: vec![rebel, ranger], container: b });
    g.run_frames(200);

    assert_eq!(g.container_of(rebel), Some(b), "the listed rebel boards");
    assert_eq!(g.container_of(ranger), None, "an unlisted ranger is refused");
    let bike_now = g.find_object(b).expect("bike");
    assert_ne!(bike_now.weapon_set_flags & WEAPONSET_RIDER1, 0, "the bike took the rebel's weapon set");
    let rider_bit = bit_for_name("RIDER1").unwrap();
    assert!(g.model_conditions(b).is_some_and(|m| m.test(rider_bit)), "the bike shows the rebel's condition");
}

#[test]
fn the_riders_command_set_overrides_the_bikes_and_outlives_the_rider() {
    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
    let b = g.create_object(&Arc::new(bike()), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    let rebel = g.create_object(&Arc::new(infantry("Rebel", None)), Coord3D::new(60.0, 50.0, 0.0), 0.0, 0);
    assert_eq!(override_of(&g, b), None, "an empty bike has no override");

    board(&mut g, rebel, b);
    assert_eq!(override_of(&g, b).as_deref(), Some("CycleRebel"), "the rebel's command set is the override");

    // `onRemoving` never clears the override (RiderChangeContain.cpp:269-351): the riderless bike
    // keeps the set until the next rider replaces it.
    g.submit(0, Command::Exit { container: b, ids: vec![rebel] });
    for _ in 0..300 {
        if g.container_of(rebel).is_none() {
            break;
        }
        g.run_frames(1);
    }
    assert_eq!(g.container_of(rebel), None, "the rebel left");
    assert_eq!(override_of(&g, b).as_deref(), Some("CycleRebel"), "the riderless bike keeps the last set");
}

#[test]
fn an_empty_rider_command_set_replaces_the_override_with_none() {
    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
    let b = g.create_object(&Arc::new(bike()), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    let rebel = g.create_object(&Arc::new(infantry("Rebel", None)), Coord3D::new(60.0, 50.0, 0.0), 0.0, 0);
    let terrorist = g.create_object(&Arc::new(infantry("Terrorist", None)), Coord3D::new(50.0, 60.0, 0.0), 0.0, 0);
    board(&mut g, rebel, b);
    // The terrorist's set is empty: `Object::setCommandSetStringOverride("")`, so `getCommandSetString`
    // falls back to the bike's own set (no override here).
    board(&mut g, terrorist, b);
    assert_eq!(g.container_of(rebel), None, "the rebel was put out");
    assert_eq!(override_of(&g, b), None, "an empty rider set leaves no override");
}

#[test]
fn a_stealthed_bike_is_revealed_while_a_rider_boards() {
    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
    let b = g.create_object(&Arc::new(bike_with(Some(stealth(true, 10)))), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    let rebel = g.create_object(&Arc::new(infantry("Rebel", None)), Coord3D::new(60.0, 50.0, 0.0), 0.0, 0);
    // Innate stealth engages once `StealthDelay` has passed since creation (StealthUpdate.cpp:129).
    g.run_frames(12);
    assert!(g.is_stealthed(b), "the innate stealth is on before a rider");

    board(&mut g, rebel, b);
    assert!(!g.is_stealthed(b), "boarding reveals the bike (RiderChangeContain.cpp:237-244)");
    let mut steps = 0;
    while !g.is_stealthed(b) && steps < 30 {
        g.run_frames(1);
        steps += 1;
    }
    // `markAsDetected()` with the default delay: about StealthDelay (10) frames of reveal.
    assert!((8..=11).contains(&steps), "the reveal lasts about StealthDelay frames, got {steps}");
}

#[test]
fn a_grant_on_the_bike_reaches_its_rider_and_lapses_with_its_countdown() {
    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
    let b = g.create_object(&Arc::new(bike_with(Some(stealth(false, 0)))), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    let rebel = g.create_object(&Arc::new(infantry("Rebel", Some(stealth(false, 0)))), Coord3D::new(60.0, 50.0, 0.0), 0.0, 0);
    board(&mut g, rebel, b);
    g.run_frames(1);
    assert!(!g.is_stealthed(b) && !g.is_stealthed(rebel), "a granted stealth is off until a grant");

    // `receiveGrant(TRUE, 5)`: the bike goes stealthy, and the grant passes on to the rider.
    g.stealth_receive_grant(b, true, 5);
    assert!(g.is_stealthed(b), "the bike is stealthed");
    assert!(g.is_stealthed(rebel), "the rider takes the grant (StealthUpdate.cpp:231-241)");
    let frame = g.frame();
    assert_ne!(g.find_object(b).unwrap().status_mask(frame) & status::CAN_STEALTH, 0, "CAN_STEALTH is set");

    // Five frames of grant. `receiveGrant(TRUE)` wakes the module for the next frame (StealthUpdate.cpp:215), so
    // the countdown's five updates run on the frames after the grant: it lapses on the sixth frame after it.
    // The rider's last command is the player's `Enter` (`privateEnterObject`, AIUpdate.cpp:2998
    // `setLastCommandSource(cmdSource)`), and a temporary grant is dropped at once on an update whose previous
    // command came from the player (`StealthUpdate.cpp:720-729`); the bike was never player-ordered and counts down.
    g.run_frames(5);
    assert!(g.is_stealthed(b), "still stealthed with one update left");
    assert!(!g.is_stealthed(rebel), "the player-ordered rider loses the temporary grant (StealthUpdate.cpp:720-729)");
    g.run_frames(1);
    assert!(!g.is_stealthed(b), "the bike's grant lapsed");
    assert!(!g.is_stealthed(rebel), "the rider has no grant left");
    let frame = g.frame();
    assert_eq!(g.find_object(b).unwrap().status_mask(frame) & status::CAN_STEALTH, 0, "CAN_STEALTH is cleared");
}

#[test]
fn a_rider_stealth_bike_never_stealths_with_a_plain_rider() {
    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
    let b = g.create_object(&Arc::new(bike_with(Some(rider_stealth(true, 10)))), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    let rebel = g.create_object(&Arc::new(infantry("Rebel", None)), Coord3D::new(60.0, 50.0, 0.0), 0.0, 0);
    g.run_frames(12);
    // `calcStealthOwner` returns the bike itself when no rider is aboard, so its own innate stealth applies.
    assert!(g.is_stealthed(b), "a riderless bike uses its own innate stealth");

    board(&mut g, rebel, b);
    for _ in 0..60 {
        g.run_frames(1);
        assert!(!g.is_stealthed(b), "a rider without stealth leaves the UseRiderStealth bike visible");
    }
}

#[test]
fn a_rider_stealth_bike_follows_the_riders_stealth() {
    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
    // The bike's own innate stealth (delay 6) holds only while no rider owns it. A plain rider is the
    // owner, and its `CAN_STEALTH` is off (cpp:309), so the bike is visible until that rider leaves.
    let b = g.create_object(&Arc::new(bike_with(Some(rider_stealth(true, 6)))), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    let rebel = g.create_object(&Arc::new(infantry("Rebel", None)), Coord3D::new(60.0, 50.0, 0.0), 0.0, 0);
    g.run_frames(8);
    assert!(g.is_stealthed(b), "riderless, the bike stealths on its own innate rule");

    board(&mut g, rebel, b);
    g.run_frames(1);
    assert!(!g.is_stealthed(b), "the plain rider owns the bike, so it is visible");

    g.submit(0, Command::Exit { container: b, ids: vec![rebel] });
    for _ in 0..300 {
        if g.container_of(rebel).is_none() {
            break;
        }
        g.run_frames(1);
    }
    let mut steps = 0;
    while !g.is_stealthed(b) && steps < 30 {
        g.run_frames(1);
        steps += 1;
    }
    // The else branch kept `m_stealthAllowedFrame` at now + the bike's delay (6) while the rider was aboard.
    assert!((1..=10).contains(&steps), "riderless again, the bike re-stealths about StealthDelay (6) frames on, got {steps}");
}

#[test]
fn the_boarding_reveal_lasts_the_riders_stealth_delay() {
    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
    // The bike's own delay is 100 frames; its innate stealth makes it stealthed while riderless.
    let b = g.create_object(&Arc::new(bike_with(Some(rider_stealth(true, 100)))), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    let rebel = g.create_object(&Arc::new(infantry("Rebel", Some(stealth(true, 6)))), Coord3D::new(60.0, 50.0, 0.0), 0.0, 0);
    // Its own 100-frame StealthDelay has to pass first (StealthUpdate.cpp:129).
    g.run_frames(102);
    assert!(g.is_stealthed(b), "the riderless bike is stealthed");

    board(&mut g, rebel, b);
    assert!(!g.is_stealthed(b), "boarding reveals the bike");
    let mut steps = 0;
    while !g.is_stealthed(b) && steps < 30 {
        g.run_frames(1);
        steps += 1;
    }
    // `markAsDetected()` takes the stealth owner's `StealthDelay` (the rider's 6), not the bike's 100.
    assert!((5..=7).contains(&steps), "the reveal lasts the rider's StealthDelay (6 frames), got {steps}");
}
