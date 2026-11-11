//! Lane 193: a building captured by one player is an ally to the rival capturer's preparation for the frame of the
//! defection (`Object::defect` + `Object::getRelationship`, Object.cpp:1595-1613, :6239-6370; `continuePreparation`,
//! SpecialAbilityUpdate.cpp:1204-1209). Synthetic data only.

use std::collections::BTreeMap;
use std::sync::Arc;
use zr_sim::damage::DamageType;
use zr_sim::special_power::{SimSpecialPowerTemplate, SpecialAbilityTemplate, SpecialPowerTarget};
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::weapon::{CombatTemplate, WeaponSetTemplate, WeaponTemplate};
use zr_sim::{Command, Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

const POWER: &str = "Capture";

fn world() -> GameLogic {
    let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    give_default_teams(&mut g, &[0, 1]);
    g
}

/// Each listed player's default team (`TeamFactory::initFromSides`, Team.cpp:217-241): a synthetic world has no
/// sides, so its objects are on no team unless this makes them (F6). Same as l193_attack.rs.
fn give_default_teams(g: &mut GameLogic, players: &[u8]) {
    let names: Vec<(String, String)> = players.iter().map(|p| (format!("teamP{p}"), format!("p{p}"))).collect();
    for p in players {
        g.scripts_mut().set_player_name(&format!("p{p}"), *p);
    }
    let protos: Vec<(&str, &str)> = names.iter().map(|(t, p)| (t.as_str(), p.as_str())).collect();
    g.scripts_mut().load_default_team_protos(&protos);
    let sides: Vec<(&str, u8)> = names.iter().zip(players).map(|((t, _), p)| (t.as_str(), *p)).collect();
    g.init_from_sides(&sides);
}

fn install_capture(g: &mut GameLogic) {
    g.set_special_powers(BTreeMap::from([(
        POWER.to_string(),
        Arc::new(SimSpecialPowerTemplate {
            name: POWER.to_string(),
            kind: "SPECIAL_INFANTRY_CAPTURE_BUILDING".to_string(),
            reload_time: 0,
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

/// A capturer: the `SpecialAbility` power module plus the `SpecialAbilityUpdate` with the given preparation and
/// pack frames (no unpack), as `capture_unit` in `special_ability_update.rs`.
fn capturer(name: &str, prep: u32, pack: u32) -> ObjectTemplate {
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

/// Installs `POWER` with the given C++ power kind (the hacker's disable, for `SPECIAL_HACKER_DISABLE_BUILDING`).
fn install_power_kind(g: &mut GameLogic, kind: &str) {
    g.set_special_powers(BTreeMap::from([(
        POWER.to_string(),
        Arc::new(SimSpecialPowerTemplate {
            name: POWER.to_string(),
            kind: kind.to_string(),
            reload_time: 0,
            required_science: None,
            shared_synced_timer: false,
            public_timer: false,
            radius_cursor_radius: 0.0,
        }),
    )]));
}

#[test]
fn a_hacker_does_not_start_preparing_against_a_building_that_reads_as_an_ally() {
    // `startPreparation` (SpecialAbilityUpdate.cpp:1047-1053): `getObject()->getRelationship(target) == ALLIES`
    // returns before the power is marked triggered and the unit's `IS_USING_ABILITY` status is set (:1089). The
    // target is an undetected defector (it reads as an ally to the hacker), owned by an enemy of the hacker's owner.
    let mut g = world();
    install_power_kind(&mut g, "SPECIAL_HACKER_DISABLE_BUILDING");
    let a = g.create_object(&Arc::new(capturer("Hacker", 10, 0)), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(vehicle("Bldg")), Coord3D::new(150.0, 100.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.set_undetected_defector(b, true);
    g.submit(0, capture_order(a, b));
    let mut seen_using = false;
    for _ in 0..6 {
        g.run_frames(1);
        let frame = g.frame();
        seen_using |= g.find_object(a).is_some_and(|o| o.status_mask(frame) & zr_sim::object::status::USING_ABILITY != 0);
    }
    assert!(!seen_using, "the hacker never set IS_USING_ABILITY against the ally-reading building");
    assert_eq!(g.find_object(b).unwrap().player, 1, "the building was not touched");
}

#[test]
fn a_rival_preparation_aborts_on_the_frame_the_building_is_defected_and_does_not_recapture_it() {
    // C (player 2) finishes its 5-frame preparation first and defects the building to player 2. A (player 0) has a
    // 10-frame preparation of its own. On the frame after C's capture the building is an undetected defector, so
    // it reads as ALLIES to A (`getRelationship`), and A's `continuePreparation` fails (:1204-1209): A packs
    // (`startPacking(false)`, :391-395) and does not take the building back. Without the defection, A's own
    // capture would fire at its 10th frame and move the building to player 0.
    let mut g = world();
    install_capture(&mut g);
    let a = g.create_object(&Arc::new(capturer("RangerA", 10, 30)), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(vehicle("Bldg")), Coord3D::new(150.0, 100.0, 0.0), 0.0, 1);
    let c = g.create_object(&Arc::new(capturer("RangerC", 5, 0)), Coord3D::new(150.0, 130.0, 0.0), 0.0, 2);
    g.set_enemies(0, 1);
    g.set_enemies(2, 1);
    g.submit(0, capture_order(a, b));
    g.submit(2, capture_order(c, b));
    g.run_frames(8);
    assert_eq!(g.find_object(b).unwrap().player, 2, "C captured the building first");
    g.run_frames(60);
    assert_eq!(g.find_object(b).unwrap().player, 2, "A did not recapture the defected building");
    assert!(g.find_object(a).unwrap().ability.is_none(), "A's capture ended after its pack");
}

#[test]
fn the_defected_building_is_undetected_for_one_frame_and_then_reads_as_an_enemy() {
    // `defect(newTeam, 1)`: `setUndetectedDefector(TRUE)` and a one-frame defection timer (Object.cpp:6281-6284).
    // The timer clears the flag, so a later look at the building is the plain team relation again.
    let mut g = world();
    install_capture(&mut g);
    let a = g.create_object(&Arc::new(capturer("RangerA", 0, 0)), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(vehicle("Bldg")), Coord3D::new(150.0, 100.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(0, capture_order(a, b));
    // Find the capture frame without pinning it (F19): the first frame the owner has changed.
    let mut captured = false;
    for _ in 0..200 {
        g.run_frames(1);
        if g.find_object(b).unwrap().player == 0 {
            captured = true;
            break;
        }
    }
    assert!(captured, "the building was captured");
    assert!(g.find_object(b).unwrap().undetected_defector, "the defection is undetected on the capture frame");
    g.run_frames(1);
    assert!(!g.find_object(b).unwrap().undetected_defector, "the one-frame defection timer cleared the flag");
}

/// A capture target with a `Open` contain module (kick out on capture, OpenContain.h:168), and a factory with a
/// production queue. Synthetic templates; the store is local to these tests.
struct FixedStore(Vec<Arc<ObjectTemplate>>);

impl zr_sim::TemplateSource for FixedStore {
    fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
        self.0.iter().find(|t| t.name == name).cloned()
    }
}

/// `cancelAndRefundAllProduction` on defect (Object.cpp:6261-6268): a captured factory refunds its queued unit to
/// the old owner (`refund_all_production` runs before the switch), so the old owner ends with the money it paid.
#[test]
fn a_captured_factory_refunds_its_queue_to_the_old_owner() {
    use zr_sim::production::{ProductionExit, ProductionTemplate};
    let mut f = vehicle("Factory");
    f.production = Some(ProductionTemplate {
        max_queue_entries: 9,
        quantity_modifiers: Vec::new(),
        exit: ProductionExit::Default { unit_create_point: (10.0, 0.0, 0.0), natural_rally_point: (100.0, 0.0, 0.0) },
        ..Default::default()
    });
    let f = Arc::new(f);
    let mut u = vehicle("Soldier");
    u.build_cost = 300;
    u.build_time_frames = 3000;
    let u = Arc::new(u);
    let mut g = GameLogic::new(world_flat(), 1);
    g.add_player(0, 1000);
    g.add_player(1, 1000);
    give_default_teams(&mut g, &[0, 1]);
    g.set_enemies(0, 1);
    g.set_template_source(Arc::new(FixedStore(vec![f.clone(), u])));
    install_capture(&mut g);
    let factory = g.create_object(&f, Coord3D::new(150.0, 100.0, 0.0), 0.0, 1);
    assert!(g.queue_unit(1, factory, "Soldier"));
    assert_eq!(g.player_money(1), 700, "the queued unit is paid for");
    let a = g.create_object(&Arc::new(capturer("RangerA", 0, 0)), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    g.submit(0, capture_order(a, factory));
    g.run_frames(20);
    assert_eq!(g.find_object(factory).unwrap().player, 0, "the factory was captured");
    assert_eq!(g.player_money(1), 1000, "the queue is refunded to the old owner");
    assert!(g.find_object(factory).unwrap().production.as_ref().is_none_or(|p| p.queue.is_empty()), "the new owner has no queue");
}

fn world_flat() -> TerrainLogic {
    TerrainLogic::flat(200, 200, 0)
}

/// `if (ct && ct->isKickOutOnCapture()) ct->removeAllContained(TRUE)` (Object.cpp:6315-6320): an `Open` container
/// (kick out TRUE, OpenContain.h:168) that is captured lets its rider out. The garrison branch of the capture trigger
/// does the same before `defect`, so this is the path for every other container.
#[test]
fn a_captured_open_container_kicks_its_riders_out() {
    use zr_sim::contain::{ContainKind, ContainTemplate, GarrisonFirePoints};
    let contain = ContainTemplate {
        kind: ContainKind::Open,
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
    let mut bld = ObjectTemplate::new("Bunker");
    bld.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE | kind_of::SELECTABLE;
    bld.geometry.major_radius = 30.0;
    bld.max_health = 1000.0;
    bld.initial_health = 1000.0;
    bld.contain = Some(Arc::new(contain));
    let mut g = world();
    install_capture(&mut g);
    g.set_enemies(0, 1);
    let b = g.create_object(&Arc::new(bld), Coord3D::new(200.0, 200.0, 0.0), 0.0, 1);
    let mut inf = vehicle("Guard");
    inf.kind_of = kind_of::INFANTRY | kind_of::SELECTABLE;
    inf.geometry.major_radius = 4.0;
    inf.transport_slot_count = 1;
    let guard = g.create_object(&Arc::new(inf), Coord3D::new(240.0, 200.0, 0.0), 0.0, 1);
    g.submit(1, Command::Enter { ids: vec![guard], container: b });
    g.run_frames(60);
    assert!(g.is_contained(guard), "the guard is inside before the capture");
    let a = g.create_object(&Arc::new(capturer("RangerA", 0, 0)), Coord3D::new(150.0, 200.0, 0.0), 0.0, 0);
    g.submit(0, capture_order(a, b));
    g.run_frames(20);
    assert_eq!(g.find_object(b).unwrap().player, 0, "the container was captured");
    assert!(!g.is_contained(guard), "the open container kicked its rider out on capture");
}
