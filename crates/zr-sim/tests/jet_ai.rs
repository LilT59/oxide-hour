//! `JetAIUpdate` + `ParkingPlaceBehavior` (the airfield jet cycle) on synthetic data: an airfield with
//! one parking space and one runway, a Raptor-like jet (wings + taxi locomotors, a
//! `RETURN_TO_BASE` missile weapon) and a helicopter that needs no runway.

use std::sync::Arc;

use zr_sim::ai_states::AiStateType;
use zr_sim::energy::GameRules;
use zr_sim::jet_ai::{jet_flag, Bone, ParkingBones, ParkingSpaceBones, RunwayBones};
use zr_sim::modules2::{JetAiTemplate, ParkingPlaceTemplate};
use zr_sim::physics::PhysicsTemplate;
use zr_sim::production::{ProductionExit, ProductionTemplate};
use zr_sim::template::{collide_kind, kind_of, LocomotorKind, LocomotorParams, LocomotorSet, ZBehavior};
use zr_sim::weapon::{CombatTemplate, WeaponSetTemplate, WeaponTemplate};
use zr_sim::{Command, Coord3D, DamageInfo, DamageType, GameLogic, ObjectId, ObjectTemplate, TemplateSource, TerrainLogic};

struct FixedStore(Vec<Arc<ObjectTemplate>>);

impl TemplateSource for FixedStore {
    fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
        self.0.iter().find(|t| t.name == name).cloned()
    }
}

fn airfield() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Airfield");
    t.kind_of = kind_of::STRUCTURE;
    t.collide_kinds = collide_kind::FS_AIRFIELD;
    t.max_health = 1000.0;
    t.initial_health = 1000.0;
    t.geometry.major_radius = 60.0;
    t.geometry.minor_radius = 40.0;
    t.geometry.height = 10.0;
    t.production = Some(ProductionTemplate {
        max_queue_entries: 9,
        quantity_modifiers: Vec::new(),
        exit: ProductionExit::Default { unit_create_point: (0.0, 0.0, 0.0), natural_rally_point: (0.0, 0.0, 0.0) },
        ..Default::default()
    });
    t.parking_place = Some(ParkingPlaceTemplate {
        num_rows: 1,
        num_cols: 1,
        has_runways: true,
        heal_amount_per_second: 30.0,
        approach_height: 50.0,
        landing_deck_height_offset: 0.0,
        park_in_hangars: false,
    });
    // The airfield faces +x. The jet exits the hangar to the south of the parking space, taxis north
    // onto the runway prep spot and takes off to the east.
    t.parking_bones = ParkingBones {
        spaces: vec![ParkingSpaceBones {
            hangar: Bone { pos: Coord3D::new(-20.0, 30.0, 0.0), orient: -std::f32::consts::FRAC_PI_2 },
            location: Bone { pos: Coord3D::new(-20.0, 0.0, 0.0), orient: -std::f32::consts::FRAC_PI_2 },
            prep: Coord3D::new(-20.0, -40.0, 0.0),
        }],
        runways: vec![RunwayBones { start: Coord3D::new(0.0, -40.0, 0.0), end: Coord3D::new(100.0, -40.0, 0.0) }],
        heli_park: Bone { pos: Coord3D::new(40.0, 0.0, 0.0), orient: 0.0 },
    };
    t
}

/// A Raptor-ish jet: `SET_NORMAL` wings, `SET_TAXIING` treads, two `RETURN_TO_BASE` missiles.
fn jet() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Jet");
    t.kind_of = kind_of::AIRCRAFT | kind_of::VEHICLE | kind_of::CAN_ATTACK | kind_of::SELECTABLE;
    t.max_health = 160.0;
    t.initial_health = 160.0;
    t.vision_range = 300.0;
    t.build_cost = 100;
    t.build_time_frames = 5;
    t.geometry.major_radius = 14.0;
    t.geometry.minor_radius = 7.0;
    t.geometry.height = 5.0;
    t.physics = Some(PhysicsTemplate { mass: 1.0, ..PhysicsTemplate::default() });
    let f = 30.0f32;
    let mut wings = LocomotorParams::from_engine_units(LocomotorKind::Wings, 175.0 / f, 60.0 / f, 120.0 / (f * f), 10.0 / (f * f), 120.0f32.to_radians() / f, false);
    wings.lift = 120.0 / (f * f);
    wings.preferred_height = 100.0;
    wings.behavior_z = ZBehavior::SurfaceRelativeHeight;
    wings.allow_motive_force_while_airborne = true;
    wings.apply_2d_friction_when_airborne = true;
    wings.circling_radius = 100.0;
    // Raptor/Comanche `AirborneTargetingHeight = 30` (the jet's `OBJECT_STATUS_AIRBORNE_TARGET` threshold).
    wings.airborne_targeting_height = 30.0;
    let mut taxi = LocomotorParams::from_engine_units(LocomotorKind::Treads, 50.0 / f, 0.0, 100.0 / (f * f), 999_999.0 / (f * f), 180.0f32.to_radians() / f, true);
    taxi.behavior_z = ZBehavior::NoZMotiveForce;
    t.locomotor = Some(wings);
    t.locomotor_sets.insert(LocomotorSet::Normal, wings);
    t.locomotor_sets.insert(LocomotorSet::Taxiing, taxi);
    t.jet = Some(JetAiTemplate {
        needs_runway: true,
        keeps_parking_space_when_airborne: true,
        takeoff_pause: 15,
        return_to_base_idle_time: 300,
        out_of_ammo_damage_per_second: 0.1,
        takeoff_dist_for_max_lift: 0.0,
        min_height: 5.0,
        parking_offset: 0.0,
        sneaky_offset_when_attacking: 0.0,
        attacking_loco: LocomotorSet::Normal,
        returning_loco: LocomotorSet::Normal,
        attack_loco_persist_time: 0,
        attackers_miss_persist_time: 0,
    });
    let mut w = WeaponTemplate::new("Missile");
    w.primary_damage = 100.0;
    w.primary_damage_radius = 5.0;
    w.secondary_damage_radius = 5.0;
    w.attack_range = 320.0;
    w.acceptable_aim_delta = 30.0f32.to_radians();
    w.min_delay_between_shots = 5;
    w.max_delay_between_shots = 5;
    w.clip_size = 2;
    w.clip_reload_time = 60;
    w.auto_reloads_clip = false;
    w.returns_to_base_to_reload = true;
    let mut set = WeaponSetTemplate::default();
    set.weapons[0] = Some(Arc::new(w));
    t.combat = CombatTemplate { weapon_set: Some(set), ..CombatTemplate::default() };
    t
}

fn target() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Target");
    t.kind_of = kind_of::STRUCTURE;
    t.max_health = 10_000.0;
    t.initial_health = 10_000.0;
    t.geometry.major_radius = 15.0;
    t.geometry.minor_radius = 15.0;
    t.geometry.height = 10.0;
    t
}

struct World {
    g: GameLogic,
    airfield: ObjectId,
}

fn world() -> World {
    let af = Arc::new(airfield());
    let jt = Arc::new(jet());
    let tt = Arc::new(target());
    let mut g = GameLogic::new(TerrainLogic::flat(300, 300, 0), 1);
    g.set_rules(GameRules { gravity: -64.0 / 900.0, ..GameRules::default() });
    g.add_player(0, 10_000);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    g.set_template_source(Arc::new(FixedStore(vec![af.clone(), jt, tt])));
    let airfield = g.create_object(&af, Coord3D::new(500.0, 500.0, 0.0), 0.0, 0);
    World { g, airfield }
}

fn produce_jet(w: &mut World) -> ObjectId {
    assert!(w.g.queue_unit(0, w.airfield, "Jet"));
    for _ in 0..200 {
        w.g.update();
        if let Some(o) = w.g.objects().find(|o| o.template.name == "Jet") {
            return o.id;
        }
    }
    panic!("the jet never appeared");
}

fn state(g: &GameLogic, id: ObjectId) -> AiStateType {
    g.find_object(id).expect("alive").ai.machine.current
}

fn alt(g: &GameLogic, id: ObjectId) -> f32 {
    let o = g.find_object(id).expect("alive");
    o.position.z - g.terrain().ground_height(o.position.x, o.position.y)
}

fn run_until(g: &mut GameLogic, max: u32, mut cond: impl FnMut(&GameLogic) -> bool) -> bool {
    for _ in 0..max {
        g.update();
        if cond(g) {
            return true;
        }
    }
    false
}

/// The jet is parked: idle, taxiing locomotor, no air flag and its parking space reserved.
fn assert_parked(w: &World, jet: ObjectId) {
    let o = w.g.find_object(jet).expect("jet");
    assert_eq!(o.ai.machine.current, AiStateType::Idle);
    assert_eq!(o.locomotor_set, LocomotorSet::Taxiing);
    assert!(!o.jet.flag(jet_flag::ALLOW_AIR_LOCO));
    assert!(alt(&w.g, jet) < 0.5);
    let rt = w.g.find_object(w.airfield).unwrap().parking.as_ref().unwrap();
    assert!(rt.spaces.iter().any(|s| s.object_in_space == jet), "the jet holds a parking space");
}

#[test]
fn a_produced_jet_taxis_out_of_the_hangar_and_parks() {
    let mut w = world();
    let jet = produce_jet(&mut w);
    // It appears in the hangar, taxiing (a takeoff-in-progress flag is set while taxiing out).
    assert_eq!(state(&w.g, jet), AiStateType::JetTaxiFromHangar);
    assert_eq!(w.g.find_object(jet).unwrap().producer_id, w.airfield);
    assert!(run_until(&mut w.g, 600, |g| state(g, jet) == AiStateType::Idle));
    assert_parked(&w, jet);
    // Parked on the `Parking` bone, facing its orientation.
    let o = w.g.find_object(jet).unwrap();
    assert!(o.position.dist_2d(&Coord3D::new(480.0, 500.0, 0.0)) < 1.0, "parked at {:?}", o.position);
    assert!((o.angle + std::f32::consts::FRAC_PI_2).abs() < 0.01, "parking orientation {}", o.angle);
}

#[test]
fn a_parked_jet_takes_off_for_a_move_flies_and_lands_when_idle_too_long() {
    let mut w = world();
    let jet = produce_jet(&mut w);
    assert!(run_until(&mut w.g, 600, |g| state(g, jet) == AiStateType::Idle));
    w.g.submit(0, Command::MoveTo { ids: vec![jet], pos: Coord3D::new(1500.0, 1500.0, 0.0) });
    w.g.update();
    // The order is held pending while the jet taxis, pauses and takes off.
    assert_eq!(state(&w.g, jet), AiStateType::JetTaxiToTakeoff);
    assert!(w.g.find_object(jet).unwrap().jet.flag(jet_flag::HAS_PENDING_COMMAND));
    assert!(run_until(&mut w.g, 600, |g| state(g, jet) == AiStateType::JetTakingOff));
    assert!(w.g.find_object(jet).unwrap().jet.flag(jet_flag::ALLOW_AIR_LOCO));
    assert_eq!(w.g.find_object(jet).unwrap().locomotor_set, LocomotorSet::Normal);
    assert!(run_until(&mut w.g, 600, |g| state(g, jet) == AiStateType::MoveTo), "the pending move runs once airborne");
    assert!(alt(&w.g, jet) > 10.0, "airborne after the takeoff run: {}", alt(&w.g, jet));
    // Fly to the goal, idle there for ReturnToBaseIdleTime (300 frames), then come home and land.
    assert!(run_until(&mut w.g, 2000, |g| state(g, jet) == AiStateType::JetReturningForLanding), "returns after idling");
    assert!(run_until(&mut w.g, 2500, |g| state(g, jet) == AiStateType::JetLanding));
    assert!(run_until(&mut w.g, 2000, |g| state(g, jet) == AiStateType::JetReloadAmmo));
    assert!(run_until(&mut w.g, 1000, |g| state(g, jet) == AiStateType::Idle));
    assert_parked(&w, jet);
}

#[test]
fn a_jet_attacks_returns_when_out_of_ammo_rearms_and_flies_again() {
    let mut w = world();
    let jet = produce_jet(&mut w);
    assert!(run_until(&mut w.g, 600, |g| state(g, jet) == AiStateType::Idle));
    let tt = Arc::new(target());
    let victim = w.g.create_object(&tt, Coord3D::new(500.0, 900.0, 0.0), 0.0, 1);
    w.g.submit(0, Command::Attack { ids: vec![jet], target: victim });
    // Takeoff, then the attack fires both missiles.
    assert!(run_until(&mut w.g, 1500, |g| state(g, jet) == AiStateType::AttackObject));
    assert!(run_until(&mut w.g, 2500, |g| g.find_object(victim).unwrap().health() < 9_900.0), "missiles hit");
    // "OUT_OF_AMMO + RETURN_TO_BASE_TO_RELOAD": the jet goes home on its own.
    assert!(run_until(&mut w.g, 2500, |g| state(g, jet) == AiStateType::JetReturningForLanding));
    assert!(w.g.find_object(jet).unwrap().jet.flag(jet_flag::USE_SPECIAL_RETURN_LOCO));
    let ammo = |g: &GameLogic| g.find_object(jet).unwrap().combat.weapons.weapons[0].as_ref().unwrap().ammo_in_clip();
    assert_eq!(ammo(&w.g), 0);
    assert!(run_until(&mut w.g, 3000, |g| state(g, jet) == AiStateType::JetReloadAmmo));
    assert!(run_until(&mut w.g, 1000, |g| state(g, jet) == AiStateType::Idle));
    assert_eq!(ammo(&w.g), 2, "rearmed");
    assert_parked(&w, jet);
    // A second order works the same.
    let hp = w.g.find_object(victim).unwrap().health();
    w.g.submit(0, Command::Attack { ids: vec![jet], target: victim });
    assert!(run_until(&mut w.g, 4000, |g| g.find_object(victim).unwrap().health() < hp - 50.0));
}

#[test]
fn a_stop_order_sends_an_airborne_jet_home() {
    let mut w = world();
    let jet = produce_jet(&mut w);
    assert!(run_until(&mut w.g, 600, |g| state(g, jet) == AiStateType::Idle));
    w.g.submit(0, Command::MoveTo { ids: vec![jet], pos: Coord3D::new(1200.0, 900.0, 0.0) });
    assert!(run_until(&mut w.g, 1500, |g| state(g, jet) == AiStateType::MoveTo));
    w.g.submit(0, Command::Stop { ids: vec![jet] });
    w.g.update();
    assert_eq!(state(&w.g, jet), AiStateType::JetReturningForLanding, "aiIdle on an airborne jet returns it for landing");
}

#[test]
fn a_destroyed_airfield_kills_parked_jets_but_not_airborne_ones() {
    let mut w = world();
    let a = produce_jet(&mut w);
    assert!(run_until(&mut w.g, 600, |g| state(g, a) == AiStateType::Idle));
    w.g.submit(0, Command::MoveTo { ids: vec![a], pos: Coord3D::new(1500.0, 1500.0, 0.0) });
    assert!(run_until(&mut w.g, 1500, |g| state(g, a) == AiStateType::MoveTo));
    // `keepsParkingSpaceWhenAirborne`: A still holds the only space, so the airfield is "full".
    assert!(!w.g.queue_unit(0, w.airfield, "Jet"), "parking places full while the first jet is out");
    assert!(run_until(&mut w.g, 200, |g| alt(g, a) > 50.0));
    w.g.damage_object(w.airfield, DamageInfo { source: ObjectId::INVALID, amount: 1.0e9, kind: DamageType::Unresistable });
    w.g.run_frames(5);
    assert!(w.g.find_object(a).is_some(), "an airborne jet survives its airfield");
}

#[test]
fn a_destroyed_airfield_kills_the_parked_jet() {
    let mut w = world();
    let a = produce_jet(&mut w);
    assert!(run_until(&mut w.g, 600, |g| state(g, a) == AiStateType::Idle));
    w.g.damage_object(w.airfield, DamageInfo { source: ObjectId::INVALID, amount: 1.0e9, kind: DamageType::Unresistable });
    w.g.run_frames(5);
    assert!(w.g.find_object(a).is_none() || w.g.find_object(a).unwrap().is_dying(), "a parked jet dies with its airfield");
}

#[test]
fn a_parked_damaged_jet_is_healed_by_the_airfield() {
    let mut w = world();
    let a = produce_jet(&mut w);
    assert!(run_until(&mut w.g, 600, |g| state(g, a) == AiStateType::Idle));
    w.g.damage_object(a, DamageInfo { source: ObjectId::INVALID, amount: 100.0, kind: DamageType::Explosion });
    let before = w.g.find_object(a).unwrap().health();
    w.g.run_frames(60);
    let after = w.g.find_object(a).unwrap().health();
    assert!(after > before + 20.0, "healed {before} -> {after} (30/s)");
}

#[test]
fn a_full_airfield_refuses_another_jet() {
    let mut w = world();
    let _a = produce_jet(&mut w);
    assert!(!w.g.queue_unit(0, w.airfield, "Jet"), "the single parking place is taken");
}

#[test]
fn the_jet_cycle_is_deterministic() {
    let crc = |seed: u32| {
        let mut w = world();
        let _ = seed;
        let jet = produce_jet(&mut w);
        assert!(run_until(&mut w.g, 600, |g| state(g, jet) == AiStateType::Idle));
        w.g.submit(0, Command::MoveTo { ids: vec![jet], pos: Coord3D::new(900.0, 900.0, 0.0) });
        w.g.run_frames(900);
        w.g.crc()
    };
    assert_eq!(crc(1), crc(2));
}

/// A Comanche-ish helicopter: `NeedsRunway = No`, `KINDOF_PRODUCED_AT_HELIPAD`, hover locomotor.
fn heli() -> ObjectTemplate {
    let mut t = jet();
    t.name = "Heli".into();
    t.collide_kinds = collide_kind::PRODUCED_AT_HELIPAD;
    t.combat = CombatTemplate::default();
    let f = 30.0f32;
    let mut hover = LocomotorParams::from_engine_units(LocomotorKind::Hover, 120.0 / f, 0.0, 60.0 / (f * f), 240.0 / (f * f), 180.0f32.to_radians() / f, false);
    hover.surfaces_air = true;
    hover.surfaces_ground = false;
    hover.lift = 120.0 / (f * f);
    hover.preferred_height = 100.0;
    hover.airborne_targeting_height = 30.0;
    hover.behavior_z = ZBehavior::SurfaceRelativeHeight;
    hover.allow_motive_force_while_airborne = true;
    hover.apply_2d_friction_when_airborne = true;
    let mut taxi = LocomotorParams::from_engine_units(LocomotorKind::Treads, 20.0 / f, 0.0, 100.0 / (f * f), 999_999.0 / (f * f), 180.0f32.to_radians() / f, true);
    taxi.behavior_z = ZBehavior::NoZMotiveForce;
    t.locomotor = Some(hover);
    t.locomotor_sets.clear();
    t.locomotor_sets.insert(LocomotorSet::Normal, hover);
    t.locomotor_sets.insert(LocomotorSet::Taxiing, taxi);
    let mut j = t.jet.clone().unwrap();
    j.needs_runway = false;
    j.keeps_parking_space_when_airborne = false;
    j.return_to_base_idle_time = 0;
    t.jet = Some(j);
    t
}

fn world_with(extra: ObjectTemplate) -> World {
    let mut w = world();
    let af = Arc::new(airfield());
    let store: Vec<Arc<ObjectTemplate>> = vec![af, Arc::new(jet()), Arc::new(target()), Arc::new(extra)];
    w.g.set_template_source(Arc::new(FixedStore(store)));
    w
}

#[test]
fn an_enter_order_lands_an_airborne_jet_at_the_airfield() {
    let mut w = world();
    let jet = produce_jet(&mut w);
    assert!(run_until(&mut w.g, 600, |g| state(g, jet) == AiStateType::Idle));
    w.g.submit(0, Command::MoveTo { ids: vec![jet], pos: Coord3D::new(1200.0, 900.0, 0.0) });
    assert!(run_until(&mut w.g, 1500, |g| state(g, jet) == AiStateType::MoveTo));
    w.g.submit(0, Command::Enter { ids: vec![jet], container: w.airfield });
    w.g.update();
    assert_eq!(state(&w.g, jet), AiStateType::JetReturningForLanding, "`privateEnter` -> `doLandingCommand`");
    assert!(run_until(&mut w.g, 3000, |g| state(g, jet) == AiStateType::JetLanding));
    assert!(run_until(&mut w.g, 2000, |g| state(g, jet) == AiStateType::Idle));
    assert_parked(&w, jet);
}

#[test]
fn a_helicopter_needs_no_runway_and_lands_to_heal_then_takes_off_again() {
    let mut w = world_with(heli());
    assert!(w.g.queue_unit(0, w.airfield, "Heli"), "a helipad unit needs no parking space");
    let mut heli = None;
    for _ in 0..200 {
        w.g.update();
        heli = w.g.objects().find(|o| o.template.name == "Heli").map(|o| o.id);
        if heli.is_some() {
            break;
        }
    }
    let heli = heli.expect("the helicopter appeared");
    // `getProducerLocation`: no reserved space, so it is allowed to fly at once and rises to its hover height.
    assert!(run_until(&mut w.g, 600, |g| alt(g, heli) > 90.0), "hovers at {}", alt(&w.g, heli));
    assert!(w.g.find_object(heli).unwrap().jet.flag(jet_flag::ALLOW_AIR_LOCO));
    assert!(!w.g.find_object(w.airfield).unwrap().parking.as_ref().unwrap().spaces.iter().any(|s| s.object_in_space == heli));
    // Land it (`AICMD_ENTER` on the airfield); damage it first so the pad has to heal it.
    w.g.damage_object(heli, DamageInfo { source: ObjectId::INVALID, amount: 100.0, kind: DamageType::Explosion });
    w.g.submit(0, Command::Enter { ids: vec![heli], container: w.airfield });
    w.g.update();
    assert_eq!(state(&w.g, heli), AiStateType::JetReturningForLanding);
    assert!(run_until(&mut w.g, 2000, |g| state(g, heli) == AiStateType::HeliLanding));
    assert!(run_until(&mut w.g, 600, |g| !g.find_object(heli).unwrap().jet.flag(jet_flag::ALLOW_AIR_LOCO)), "landed");
    assert!(alt(&w.g, heli) < 1.5, "on the pad: {}", alt(&w.g, heli));
    assert_eq!(w.g.find_object(heli).unwrap().locomotor_set, LocomotorSet::Taxiing);
    let hurt = w.g.find_object(heli).unwrap().health();
    w.g.run_frames(30);
    assert!(w.g.find_object(heli).unwrap().health() > hurt, "healing on the pad");
    // Healed -> "we're completely healed, so take off again".
    assert!(run_until(&mut w.g, 600, |g| g.find_object(heli).unwrap().jet.flag(jet_flag::ALLOW_AIR_LOCO)), "takes off once healed");
}

/// Possession (an addition): a parked jet is made airborne-capable and flies by hand.
#[cfg(feature = "possession")]
#[test]
fn a_possessed_parked_jet_flies() {
    use zr_sim::PossessInput;
    let mut w = world();
    let jet = produce_jet(&mut w);
    assert!(run_until(&mut w.g, 600, |g| state(g, jet) == AiStateType::Idle));
    w.g.submit(0, Command::Possess { id: jet });
    // Full throttle (W), no turn.
    for _ in 0..400 {
        w.g.submit(0, Command::PossessInput(PossessInput { move_x: 0, move_y: 32767, aim: Coord3D::ZERO, buttons: 0 }));
        w.g.update();
    }
    let o = w.g.find_object(jet).unwrap();
    assert_eq!(o.locomotor_set, LocomotorSet::Normal);
    assert!(o.jet.flag(jet_flag::ALLOW_AIR_LOCO));
    assert!(alt(&w.g, jet) > 30.0, "airborne under possession: {}", alt(&w.g, jet));
    assert!(o.position.dist_2d(&Coord3D::new(480.0, 500.0, 0.0)) > 200.0, "it flew away from the airfield");
}
