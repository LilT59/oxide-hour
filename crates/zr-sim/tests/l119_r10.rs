//! R10 findings 3 and 5: `StealthDetectorUpdate`'s gates and victim filter
//! (StealthDetectorUpdate.cpp:143-356) and `StealthUpdate`'s `m_enabled`, player-command grant loss and
//! `RevealDistanceFromTarget` self-reveal (StealthUpdate.cpp:132, :694-734). Synthetic data only.

use std::sync::Arc;

use zr_sim::behaviors::{StealthDetectorTemplate, StealthTemplate};
use zr_sim::damage::Relationship;
use zr_sim::object::status;
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::{Command, Coord3D, GameLogic, ObjectId, ObjectTemplate, TemplateSource, TerrainLogic};

fn flat() -> TerrainLogic {
    TerrainLogic::flat(400, 400, 0)
}

fn unit(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 8.0;
    t.max_health = 200.0;
    t.initial_health = 200.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    t
}

/// A `StealthUpdate`. `innate` sets `InnateStealth` (and so `CAN_STEALTH` at construction,
/// StealthUpdate.cpp:147-151); a non-innate one is `GrantedBySpecialPower` and sleeps until a grant.
fn stealth_template(delay: u32, innate: bool) -> StealthTemplate {
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

fn detector_template(rate: u32, range: f32) -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Detector");
    t.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE | kind_of::SELECTABLE;
    t.geometry.major_radius = 10.0;
    t.max_health = 500.0;
    t.initial_health = 500.0;
    t.stealth_detector = Some(StealthDetectorTemplate {
        detection_rate: rate,
        detection_range: range,
        initially_disabled: false,
        extra_required_kind_of: 0,
        extra_forbidden_kind_of: 0,
        can_detect_while_garrisoned: false,
        can_detect_while_contained: false,
    });
    t
}

fn detected_until(g: &GameLogic, id: ObjectId) -> u32 {
    g.find_object(id).expect("object").behavior.detected_until
}

/// `PartitionFilterStealthedOrStealthGarrisoned` (`StealthDetectorUpdate.cpp:125-138`) keeps only objects with
/// `OBJECT_STATUS_STEALTHED`; the port used to test the *presence* of a `StealthUpdate` module, so it revealed
/// a stealth unit that was not currently stealthed (a `GrantedBySpecialPower` module before its grant).
/// Revert check: keep the old `template.stealth.is_some()` filter.
#[test]
fn a_detector_reveals_only_stealthed_victims() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    g.create_object(&Arc::new(detector_template(1, 200.0)), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    // Stealthed from its first update (`InnateStealth`, delay 0).
    let mut shown = unit("Hidden");
    shown.stealth = Some(stealth_template(0, true));
    let hidden = g.create_object(&Arc::new(shown), Coord3D::new(260.0, 200.0, 0.0), 0.0, 1);
    // A stealth module that is off until a grant (`GrantedBySpecialPower`, StealthUpdate.cpp:132, :154-157).
    let mut dormant_t = unit("Dormant");
    dormant_t.stealth = Some(stealth_template(0, false));
    let dormant = g.create_object(&Arc::new(dormant_t), Coord3D::new(200.0, 260.0, 0.0), 0.0, 1);

    g.run_frames(6);
    assert!(g.find_object(hidden).unwrap().behavior.stealthed, "the innate unit stealths");
    assert!(detected_until(&g, hidden) > g.frame(), "the stealthed unit is revealed");
    assert!(!g.is_stealthed(dormant), "the granted-only unit has not stealthed");
    assert_eq!(detected_until(&g, dormant), 0, "an unstealthed unit is not revealed by the detector");
}

/// `PartitionFilterRelationship(ALLOW_ENEMIES | ALLOW_NEUTRAL)` (`StealthDetectorUpdate.cpp:187`) excludes
/// allies; the port used "a different player", which included them. Revert check: `o.player != player`.
#[test]
fn a_detector_ignores_allies() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.add_player(2, 0);
    g.set_enemies(0, 1);
    g.set_relationship_symmetric(0, 2, Relationship::Allies);
    g.create_object(&Arc::new(detector_template(1, 200.0)), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let mut t = unit("Friendly");
    t.stealth = Some(stealth_template(0, true));
    let ally = g.create_object(&Arc::new(t), Coord3D::new(240.0, 200.0, 0.0), 0.0, 2);
    let mut e = unit("Enemy");
    e.stealth = Some(stealth_template(0, true));
    let enemy = g.create_object(&Arc::new(e), Coord3D::new(200.0, 240.0, 0.0), 0.0, 1);

    g.run_frames(6);
    assert!(detected_until(&g, enemy) > g.frame(), "the enemy is revealed");
    assert_eq!(detected_until(&g, ally), 0, "an ally is not a detector victim");
    assert!(g.is_stealthed(ally), "the ally's stealth is untouched");
}

struct FixedStore(Vec<Arc<ObjectTemplate>>);

impl TemplateSource for FixedStore {
    fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
        self.0.iter().find(|t| t.name == name).cloned()
    }
}

/// `if (self->testStatus(OBJECT_STATUS_UNDER_CONSTRUCTION)) return UPDATE_SLEEP_NONE;`
/// (`StealthDetectorUpdate.cpp:152-153`): a construction site does not sweep until it is finished.
/// Revert check: drop the under-construction return.
#[test]
fn a_detector_under_construction_does_not_sweep() {
    let mut dozer = unit("Dozer");
    dozer.dozer = Some(zr_sim::dozer::DozerTemplate::default());
    let mut det = detector_template(1, 300.0);
    det.build_cost = 100;
    det.build_time_frames = 600;
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 2000);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    g.set_template_source(Arc::new(FixedStore(vec![Arc::new(dozer), Arc::new(det)])));
    let d = g.create_object(&g.find_template("Dozer").unwrap(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let site = g.construct(d, "Detector", Coord3D::new(200.0, 200.0, 0.0), 0.0).expect("construct");
    assert!(g.find_object(site).unwrap().under_construction);
    let mut t = unit("Hidden");
    t.stealth = Some(stealth_template(0, true));
    let hidden = g.create_object(&Arc::new(t), Coord3D::new(260.0, 200.0, 0.0), 0.0, 1);

    g.run_frames(8);
    assert!(g.is_stealthed(hidden), "the enemy stays hidden");
    assert_eq!(detected_until(&g, hidden), 0, "a detector site never sweeps");
}

/// `else // perhaps they are garrisoning something stealthy, eh?` (`StealthDetectorUpdate.cpp:328-351`): a
/// garrisonable container holding a `KINDOF_STEALTH_GARRISON` rider passes the stealth filter, and its
/// contained stealth riders are revealed with `updateRate + 2`.
/// Revert check: drop that branch (`mark_detected(rider, rate + 2)`).
#[test]
fn a_detector_reveals_the_stealth_riders_of_a_garrison() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    g.create_object(&Arc::new(detector_template(2, 300.0)), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let mut bld = ObjectTemplate::new("Garrison");
    bld.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;
    bld.geometry.major_radius = 25.0;
    bld.max_health = 500.0;
    bld.initial_health = 500.0;
    bld.contain = Some(Arc::new(zr_sim::contain::ContainTemplate {
        kind: zr_sim::contain::ContainKind::Garrison,
        capacity: 5,
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
        heal_frames: 1.0,
        initial_payload: Vec::new(),
        tunnel_frames_for_full_heal: 1.0,
        is_enclosing_container: true,
        fire_points: zr_sim::contain::GarrisonFirePoints::default(),
        passengers_in_turret: false,
        weapon_bonus_passed_to_passengers: false,
        immune_to_clear_building_attacks: false,
    }));
    let gb = g.create_object(&Arc::new(bld), Coord3D::new(240.0, 200.0, 0.0), 0.0, 1);
    // `KINDOF_STEALTH_GARRISON` is what `getStealthUnitsContained` counts (`OpenContain.cpp:409-412`).
    let mut inf = ObjectTemplate::new("StealthRifle");
    inf.kind_of = kind_of::INFANTRY | kind_of::SELECTABLE | kind_of::STEALTH_GARRISON;
    inf.geometry.major_radius = 4.0;
    inf.max_health = 100.0;
    inf.initial_health = 100.0;
    inf.transport_slot_count = 1;
    inf.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Legs, 20.0, 60.0, 120.0, 180.0));
    // Not currently stealthed (grant-only), so only the garrison branch can reveal it.
    inf.stealth = Some(stealth_template(0, false));
    let rider = g.create_object(&Arc::new(inf), Coord3D::new(230.0, 200.0, 0.0), 0.0, 1);

    g.submit(1, Command::Enter { ids: vec![rider], container: gb });
    for _ in 0..120 {
        if g.container_of(rider) == Some(gb) {
            break;
        }
        g.run_frames(1);
    }
    assert_eq!(g.container_of(rider), Some(gb), "the rider boards the garrison");
    assert!(!g.is_stealthed(rider), "the rider is not stealthed on its own");
    g.run_frames(4);
    assert!(detected_until(&g, rider) > g.frame(), "the garrison's stealth rider is revealed");
}

/// `m_enabled = !data->m_teamDisguised` (`StealthUpdate.cpp:132`): `InnateStealth` does not switch the module
/// off. A module with neither `InnateStealth` nor `GrantedBySpecialPower` (the GLA camouflage upgrades' module,
/// `InnateStealth = No ;Requires upgrade first`) runs every frame; `allowedToStealth`'s `CAN_STEALTH` failure
/// branch keeps re-arming `m_stealthAllowedFrame` (`:755-759`), and that loop is what notices a `StealthUpgrade`
/// that sets `CAN_STEALTH` later ("The logic that does the stealthupdate will notice this",
/// StealthUpgrade.cpp:51-55).
/// Revert check: the old `InnateStealth && !GrantedBySpecialPower` rule sleeps the module after its first update
/// and `m_stealthAllowedFrame` freezes.
#[test]
fn an_upgrade_gated_stealth_module_stays_awake() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    let mut t = unit("CamoTank");
    let mut st = stealth_template(10, false);
    st.granted_by_special_power = false;
    t.stealth = Some(st);
    let id = g.create_object(&Arc::new(t), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    g.run_frames(5);
    let o = g.find_object(id).unwrap();
    assert!(!o.behavior.stealthed, "without CAN_STEALTH it does not stealth");
    assert_eq!(o.status_mask(g.frame()) & status::CAN_STEALTH, 0, "the upgrade has not set CAN_STEALTH");
    // `m_stealthAllowedFrame` starts at creation + StealthDelay (`:129`) and is re-armed on every update.
    let armed = o.behavior.stealth_allowed;
    assert!(armed > 10, "the awake module re-arms its delay every frame (got {armed})");
    g.run_frames(2);
    assert!(g.find_object(id).unwrap().behavior.stealth_allowed > armed, "it keeps re-arming");
}

/// "If the last AI command given was by the player... then LOSE the stealth now!" (`StealthUpdate.cpp:720-729`):
/// a temporary grant is dropped on the next update after a player command.
/// Revert check: skip the `CMD_FROM_PLAYER` test.
#[test]
fn a_player_order_loses_the_temporary_stealth_grant() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    let mut t = unit("Saboteur");
    t.stealth = Some(stealth_template(0, false));
    let id = g.create_object(&Arc::new(t), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    g.run_frames(1);
    g.stealth_receive_grant(id, true, 20);
    assert!(g.is_stealthed(id), "the grant turns the stealth on");
    g.submit(0, Command::Stop { ids: vec![id] });
    g.run_frames(3);
    assert!(!g.is_stealthed(id), "a player order drops the grant");
    assert_eq!(g.find_object(id).unwrap().behavior.stealth_frames_granted, 0, "the countdown is gone");
    assert_eq!(g.find_object(id).unwrap().status_mask(g.frame()) & status::CAN_STEALTH, 0, "CAN_STEALTH is cleared");
}

/// `getRevealDistanceFromTarget()` (`StealthUpdate.cpp:694-713`): a stealth unit that gets within that distance
/// of its current victim reveals itself (`markAsDetected()` with the stealth delay).
/// Revert check: drop the reveal-distance block.
#[test]
fn getting_close_to_its_target_reveals_a_stealth_unit() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    let mut t = unit("Tank");
    t.kind_of |= kind_of::CAN_ATTACK;
    t.vision_range = 200.0;
    let mut w = zr_sim::weapon::WeaponTemplate::new("Gun");
    w.primary_damage = 10.0;
    w.attack_range = 100.0;
    let mut set = zr_sim::weapon::WeaponSetTemplate::default();
    set.weapons[0] = Some(Arc::new(w));
    t.combat.weapon_set = Some(set);
    let mut st = stealth_template(10, true);
    st.reveal_distance_from_target = 60.0;
    t.stealth = Some(st);
    let me = g.create_object(&Arc::new(t), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let victim = g.create_object(&Arc::new(unit("Victim")), Coord3D::new(240.0, 200.0, 0.0), 0.0, 1);
    // Let the innate stealth engage (`m_stealthAllowedFrame = creation + StealthDelay`).
    g.run_frames(12);
    assert!(g.is_stealthed(me), "the unit is stealthed at its own distance");

    g.submit(0, Command::Attack { ids: vec![me], target: victim });
    g.run_frames(2);
    assert!(g.find_object(me).unwrap().combat.attack.is_some(), "the attack is running");
    assert!(detected_until(&g, me) > g.frame(), "its own reveal distance marks it detected");
    assert!(!g.is_stealthed(me), "a revealed unit is no longer stealthed");
}
