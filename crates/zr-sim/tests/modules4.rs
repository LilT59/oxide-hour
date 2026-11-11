//! Round-4 modules: GLA rebuild holes, countermeasures, jet slow death, demo traps, hazard
//! cleanup, command-button hunt and minefield generation. Synthetic data only.

use std::sync::Arc;

use zr_sim::combat::AttackTarget;
use zr_sim::damage::DamageType;
use zr_sim::die::{DieMux, DieModuleTemplate, DieTemplate, JetSlowDeathTemplate, SlowDeathTemplate};
use zr_sim::modules2::CountermeasuresTemplate;
use zr_sim::modules4::{
    CleanupHazardTemplate, CommandButtonHuntTemplate, DemoTrapTemplate, GenerateMinefieldTemplate, HuntCommand, RebuildHoleExposeTemplate,
    RebuildHoleTemplate,
};
use zr_sim::physics::PhysicsTemplate;
use zr_sim::projectile::{MissileParams, ProjectileBehavior, ProjectileTemplate};
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams, SimpleTemplateStore, ZBehavior};
use zr_sim::weapon::{CombatTemplate, WeaponSetTemplate, WeaponTemplate};
use zr_sim::{Command, Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

fn flat() -> TerrainLogic {
    TerrainLogic::flat(200, 200, 0)
}

fn all_mux() -> DieMux {
    DieMux { death_types: u32::MAX, veterancy_levels: u32::MAX, exempt_status: 0, required_status: 0 }
}

fn ground_unit(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
    t.geometry.major_radius = 8.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    // A ground locomotor sticks to the terrain (the INI's `StickToGround`; `from_ini` leaves it off).
    let mut l = LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0);
    l.stick_to_ground = true;
    t.locomotor = Some(l);
    t
}

fn air_unit(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::AIRCRAFT | kind_of::SELECTABLE | kind_of::VEHICLE;
    t.geometry.major_radius = 20.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.physics = Some(PhysicsTemplate::default());
    let mut l = LocomotorParams::from_ini(LocomotorKind::Thrust, 60.0, 60.0, 60.0, 60.0);
    l.stick_to_ground = false;
    t.locomotor = Some(l);
    t
}

fn weapon(name: &str, damage: f32, radius: f32) -> WeaponTemplate {
    let mut w = WeaponTemplate::new(name);
    w.primary_damage = damage;
    w.primary_damage_radius = radius;
    w.secondary_damage_radius = radius;
    w.attack_range = 500.0;
    w.min_delay_between_shots = 1000;
    w.max_delay_between_shots = 1000;
    w.damage_type = DamageType::Explosion;
    w.weapon_speed = 40.0;
    w
}

fn with_weapon(mut t: ObjectTemplate, w: WeaponTemplate) -> ObjectTemplate {
    let mut set = WeaponSetTemplate::default();
    set.weapons[0] = Some(Arc::new(w));
    t.combat = CombatTemplate { weapon_set: Some(set), ..CombatTemplate::default() };
    t
}

// ---- RebuildHoleBehavior -------------------------------------------------------------------

fn structure() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Struct");
    t.kind_of = kind_of::STRUCTURE | kind_of::SELECTABLE | kind_of::IMMOBILE;
    t.max_health = 1000.0;
    t.initial_health = 1000.0;
    t.geometry.major_radius = 20.0;
    t.build_time_frames = 6;
    t.die.push(DieModuleTemplate {
        mux: all_mux(),
        kind: DieTemplate::RebuildHoleExpose(RebuildHoleExposeTemplate {
            hole_name: Some("Hole".into()),
            hole_max_health: 500.0,
            transfer_attackers: true,
        }),
    });
    t.die.push(DieModuleTemplate { mux: all_mux(), kind: DieTemplate::Destroy });
    t
}

fn hole() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Hole");
    t.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;
    t.max_health = 9_999_999.0;
    t.initial_health = 9_999_999.0;
    t.geometry.major_radius = 25.0;
    t.rebuild_hole = Some(RebuildHoleTemplate {
        worker_object_name: Some("Worker".into()),
        worker_respawn_delay: 3,
        hole_health_regen_percent_per_second: 0.5,
    });
    t
}

fn worker() -> ObjectTemplate {
    let mut t = ground_unit("Worker");
    t.kind_of = kind_of::INFANTRY | kind_of::SELECTABLE;
    // The hole's worker is a dozer/worker object (`WorkerAIUpdate` in the engine): the rebuild task is
    // its dozer machine's, so the template needs the `DozerAIUpdate` data.
    t.dozer = Some(zr_sim::dozer::DozerTemplate::default());
    t
}

fn store() -> SimpleTemplateStore {
    let mut s = SimpleTemplateStore::default();
    s.add(structure());
    s.add(hole());
    s.add(worker());
    s
}

#[test]
fn destroyed_structure_leaves_a_hole_that_rebuilds() {
    let mut g = GameLogic::new(flat(), 1);
    g.set_template_source(Arc::new(store()));
    let s = g.create_object(&Arc::new(structure()), Coord3D::new(300.0, 300.0, 0.0), 0.0, 0);
    g.damage_object(s, zr_sim::body::DamageInfo { source: zr_sim::ObjectId::INVALID, amount: 10_000.0, kind: DamageType::Explosion });
    let holes: Vec<_> = g.objects().filter(|o| o.template.name == "Hole").map(|o| o.id).collect();
    assert_eq!(holes.len(), 1, "the structure left a hole");
    // The hole's health was set from `HoleMaxHealth`.
    assert_eq!(g.find_object(holes[0]).unwrap().health(), 500.0);

    // No worker yet; it spawns after `WorkerRespawnDelay` frames.
    assert!(g.find_object(holes[0]).unwrap().behavior.hole_worker.is_none());
    g.run_frames(3);
    let work = g.find_object(holes[0]).unwrap().behavior.hole_worker.expect("worker spawned");
    assert_eq!(g.find_object(work).unwrap().template.name, "Worker");

    // The worker rebuilds the structure; the hole and worker go away when it completes.
    g.run_frames(30);
    assert!(g.find_object(holes[0]).is_none(), "the hole is removed once rebuilt");
    assert!(g.find_object(work).is_none(), "the worker is removed once rebuilt");
    assert_eq!(g.objects().filter(|o| o.template.name == "Struct").count(), 1, "the structure is back");
}

#[test]
fn rebuild_hole_does_not_multiply_when_the_reconstruction_is_killed() {
    // Regression for the `GLAHole*` structure runaway: a hole rebuilds the structure as an
    // under-construction 1-hp object. `RebuildHoleExposeDie` must not leave another hole when that
    // object is killed while under construction (`RebuildHoleExposeDie.cpp:127-129`), or every
    // rebuild doubles the hole count.
    let mut g = GameLogic::new(flat(), 1);
    g.set_template_source(Arc::new(store()));
    let s = g.create_object(&Arc::new(structure()), Coord3D::new(300.0, 300.0, 0.0), 0.0, 0);
    g.damage_object(s, zr_sim::body::DamageInfo { source: zr_sim::ObjectId::INVALID, amount: 10_000.0, kind: DamageType::Explosion });
    assert_eq!(g.objects().filter(|o| o.template.name == "Hole").count(), 1);

    for cycle in 0..4 {
        // Wait for the worker to spawn and lay down the replacement under construction.
        let mut rebuilt = None;
        for _ in 0..12 {
            g.run_frames(1);
            rebuilt = g.objects().find(|o| o.template.name == "Struct" && o.under_construction).map(|o| o.id);
            if rebuilt.is_some() {
                break;
            }
        }
        let rebuilt = rebuilt.unwrap_or_else(|| panic!("cycle {cycle}: the hole laid no reconstruction"));
        g.damage_object(rebuilt, zr_sim::body::DamageInfo { source: zr_sim::ObjectId::INVALID, amount: 10_000.0, kind: DamageType::Explosion });
        let holes = g.objects().filter(|o| o.template.name == "Hole").count();
        assert_eq!(holes, 1, "cycle {cycle}: the hole must not multiply (got {holes})");
    }
}

#[test]
fn rebuild_hole_transfers_attackers_to_the_building() {
    let mut g = GameLogic::new(flat(), 1);
    g.set_template_source(Arc::new(store()));
    let s = g.create_object(&Arc::new(structure()), Coord3D::new(300.0, 300.0, 0.0), 0.0, 0);
    let mut attacker_t = ground_unit("Attacker");
    attacker_t = with_weapon(attacker_t, weapon("W", 5.0, 5.0));
    let a = g.create_object(&Arc::new(attacker_t), Coord3D::new(500.0, 300.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(1, Command::Attack { ids: vec![a], target: s });
    g.run_frames(1);
    assert!(matches!(g.find_object(a).unwrap().combat.attack.as_ref().unwrap().target, AttackTarget::Object(id) if id == s));

    g.damage_object(s, zr_sim::body::DamageInfo { source: zr_sim::ObjectId::INVALID, amount: 10_000.0, kind: DamageType::Explosion });
    let h = g.objects().find(|o| o.template.name == "Hole").unwrap().id;
    // The attacker was retargeted to the hole as the structure fell.
    assert!(matches!(g.find_object(a).unwrap().combat.attack.as_ref().unwrap().target, AttackTarget::Object(id) if id == h));
}

// ---- CountermeasuresBehavior ---------------------------------------------------------------

fn countermeasure_flare() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Flare");
    t.kind_of = kind_of::PROJECTILE;
    t.geometry.major_radius = 4.0;
    t.physics = Some(PhysicsTemplate::default());
    t.lifetime = Some((90, 90));
    let mut l = LocomotorParams::from_ini(LocomotorKind::Thrust, 60.0, 60.0, 60.0, 60.0);
    l.stick_to_ground = false;
    t.locomotor = Some(l);
    t
}

fn aircraft_with_countermeasures() -> ObjectTemplate {
    let mut t = air_unit("Aircraft");
    // A hovering aircraft holds its cruise altitude: lift plus `SurfaceRelativeHeight` towards
    // `PreferredHeight` (`locomotor.rs:436`). An idle Thrust plane has no lift and no Z behaviour
    // (`Locomotor.cpp:400`), so it cannot hold an altitude: it thrusts back toward its latched point and
    // overshoots (`Locomotor.cpp:2501`; see `ai_death_thrust.rs`), and it can still hit the ground. The
    // landing damage (`physics.rs`) was what this test counted as a missile hit.
    // Hover settings as the helicopter fixture in `jet_ai.rs`, with lift large enough to hold 60.
    let mut hover = LocomotorParams::from_engine_units(LocomotorKind::Hover, 120.0 / 30.0, 0.0, 60.0 / (30.0 * 30.0), 240.0 / (30.0 * 30.0), 180.0f32.to_radians() / 30.0, false);
    hover.surfaces_air = true;
    hover.surfaces_ground = false;
    hover.lift = 2.0;
    hover.preferred_height = 60.0;
    // `AirborneTargetingHeight = 30` (the aircraft locomotors in `locomotor.ini`): `OBJECT_STATUS_AIRBORNE_TARGET`
    // is what `CountermeasuresBehavior` reads (`CountermeasuresBehavior.cpp:240`).
    hover.airborne_targeting_height = 30.0;
    hover.behavior_z = ZBehavior::SurfaceRelativeHeight;
    hover.allow_motive_force_while_airborne = true;
    hover.apply_2d_friction_when_airborne = true;
    t.locomotor = Some(hover);
    t.countermeasures = Some(CountermeasuresTemplate {
        flare_template_name: Some("Flare".into()),
        evasion_rate: 1.0,
        volley_size: 2,
        volley_arc_angle: 0.0,
        volley_velocity_factor: 1.0,
        number_of_volleys: 3,
        delay_between_volleys: 100,
        reload_time: 0,
        must_reload_at_airfield: true,
        missile_decoy_delay: 0,
        countermeasure_reaction_frames: 0,
        starts_active: true,
        triggered_by: vec![],
        conflicts_with: vec![],
        flare_lifetime_frames: 90,
    });
    t
}

fn missile_attacker() -> ObjectTemplate {
    let missile = ProjectileTemplate {
        behavior: ProjectileBehavior::Missile(MissileParams {
            try_to_follow_target: true,
            initial_velocity: 12.0,
            lock_distance: 20.0,
            ..MissileParams::default()
        }),
        bounding_radius: 3.0,
        small_missile: true,
        neutron_warhead: None,
        neutron_blast: None,
        garrison_hit_kill: None,
    };
    let mut w = weapon("Missile", 90.0, 10.0);
    w.has_projectile = true;
    w.projectile = Some(Arc::new(missile));
    // A missile that can track an aircraft (anti-air).
    w.anti_mask = zr_sim::weapon::anti::GROUND | zr_sim::weapon::anti::AIRBORNE_VEHICLE;
    with_weapon(ground_unit("Attacker"), w)
}

#[test]
fn countermeasures_divert_incoming_missiles() {
    let mut g = GameLogic::new(flat(), 1);
    let mut s = SimpleTemplateStore::default();
    s.add(countermeasure_flare());
    let plane_t = s.add(aircraft_with_countermeasures());
    let atk_t = s.add(missile_attacker());
    g.set_template_source(Arc::new(s));

    let plane = g.create_object(&plane_t, Coord3D::new(400.0, 300.0, 60.0), 0.0, 1);
    let atk = g.create_object(&atk_t, Coord3D::new(200.0, 300.0, 0.0), 0.0, 0);
    g.set_enemies(0, 1);
    g.submit(0, Command::Attack { ids: vec![atk], target: plane });
    g.run_frames(120);
    // A volley of flares was launched, and the plane survived untouched (the missile was diverted).
    let flares = g.objects().filter(|o| o.template.name == "Flare").count();
    assert!(flares > 0, "countermeasures launched at least one flare (got {flares})");
    assert_eq!(g.find_object(plane).unwrap().health(), 100.0, "the diverted missile did no damage");
}

/// `CountermeasuresBehavior::update` reacts only while `obj->isAirborneTarget()` (`CountermeasuresBehavior.cpp:240`), the
/// `OBJECT_STATUS_AIRBORNE_TARGET` bit `doLocomotor` keeps. A plane whose locomotor never reaches its
/// `AirborneTargetingHeight` (the INT_MAX default) is not an airborne target, so it launches no flares even at altitude.
#[test]
fn countermeasures_follow_the_airborne_target_bit_not_the_aircraft_kind() {
    let mut g = GameLogic::new(flat(), 1);
    let mut s = SimpleTemplateStore::default();
    s.add(countermeasure_flare());
    let mut plane_t = aircraft_with_countermeasures();
    if let Some(l) = plane_t.locomotor.as_mut() {
        l.airborne_targeting_height = f32::MAX;
    }
    let plane_t = s.add(plane_t);
    let atk_t = s.add(missile_attacker());
    g.set_template_source(Arc::new(s));

    let plane = g.create_object(&plane_t, Coord3D::new(400.0, 300.0, 60.0), 0.0, 1);
    let atk = g.create_object(&atk_t, Coord3D::new(200.0, 300.0, 0.0), 0.0, 0);
    g.set_enemies(0, 1);
    g.submit(0, Command::Attack { ids: vec![atk], target: plane });
    g.run_frames(120);
    assert!(g.objects().all(|o| o.template.name != "Flare"), "an aircraft that is not an airborne target launches no flares");
}

#[test]
fn countermeasures_need_the_upgrade() {
    let mut g = GameLogic::new(flat(), 1);
    let mut s = SimpleTemplateStore::default();
    s.add(countermeasure_flare());
    let mut plane_t = aircraft_with_countermeasures();
    if let Some(c) = plane_t.countermeasures.as_mut() {
        c.starts_active = false;
        c.triggered_by = vec!["Upgrade_CM".into()];
    }
    let plane_t = s.add(plane_t);
    let atk_t = s.add(missile_attacker());
    g.set_template_source(Arc::new(s));
    let plane = g.create_object(&plane_t, Coord3D::new(400.0, 300.0, 60.0), 0.0, 1);
    let atk = g.create_object(&atk_t, Coord3D::new(200.0, 300.0, 0.0), 0.0, 0);
    g.set_enemies(0, 1);
    g.submit(0, Command::Attack { ids: vec![atk], target: plane });
    g.run_frames(120);
    assert_eq!(g.objects().filter(|o| o.template.name == "Flare").count(), 0, "no flares without the upgrade");
    assert!(g.find_object(plane).is_none_or(|o| o.health() < 100.0), "the missile hit the un-protected plane");
}

// ---- JetSlowDeathBehavior ------------------------------------------------------------------

fn jet_slow_template() -> JetSlowDeathTemplate {
    let slow = SlowDeathTemplate {
        probability_modifier: 10,
        modifier_bonus_per_overkill_percent: 0.0,
        sink_rate: 0.0,
        sink_delay: 0,
        sink_delay_variance: 0,
        destruction_delay: 999_999,
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
    JetSlowDeathTemplate {
        slow: Arc::new(slow),
        fx_on_ground_death: vec!["FX_OnGround".into()],
        ocl_on_ground_death: vec![],
        fx_initial_death: vec!["FX_Initial".into()],
        ocl_initial_death: vec![],
        delay_secondary_from_initial_death: 3,
        fx_secondary: vec!["FX_Secondary".into()],
        ocl_secondary: vec![],
        fx_hit_ground: vec!["FX_HitGround".into()],
        ocl_hit_ground: vec![],
        delay_final_blow_up_from_hit_ground: 3,
        fx_final_blow_up: vec!["FX_Final".into()],
        ocl_final_blow_up: vec![],
        roll_rate: 0.2,
        roll_rate_delta: 1.0,
        pitch_rate: 0.0,
        fall_how_fast: 1.1,
    }
}

fn jet() -> ObjectTemplate {
    let mut t = air_unit("Jet");
    t.die.push(DieModuleTemplate { mux: all_mux(), kind: DieTemplate::JetSlowDeath(Arc::new(jet_slow_template())) });
    t
}

#[test]
fn jet_on_ground_death_hits_ground_immediately() {
    let mut g = GameLogic::new(flat(), 1);
    let j = g.create_object(&Arc::new(jet()), Coord3D::new(300.0, 300.0, 0.0), 0.0, 0);
    g.damage_object(j, zr_sim::body::DamageInfo { source: zr_sim::ObjectId::INVALID, amount: 10_000.0, kind: DamageType::Explosion });
    assert!(g.find_object(j).is_none(), "an on-ground jet dies at once");
    assert!(g.fx_events().iter().any(|e| e.name == "FX_OnGround"), "the ground FX ran");
}

#[test]
fn jet_in_air_spirals_down_and_blows_up() {
    let mut g = GameLogic::new(flat(), 1);
    let j = g.create_object(&Arc::new(jet()), Coord3D::new(300.0, 300.0, 120.0), 0.0, 0);
    g.damage_object(j, zr_sim::body::DamageInfo { source: zr_sim::ObjectId::INVALID, amount: 10_000.0, kind: DamageType::Explosion });
    assert!(g.fx_events().iter().any(|e| e.name == "FX_Initial"), "the initial FX ran");
    assert!(g.find_object(j).is_some(), "the jet is still falling");
    // The fall takes as long as the shipped gravity (-64 per second²) allows: about 58 frames from 120 up.
    for _ in 0..200 {
        if g.fx_events().iter().any(|e| e.name == "FX_HitGround") || g.find_object(j).is_none() {
            break;
        }
        g.run_frames(1);
    }
    assert!(g.fx_events().iter().any(|e| e.name == "FX_HitGround") || g.find_object(j).is_none(), "the jet reached the ground");
    g.run_frames(6);
    assert!(g.find_object(j).is_none(), "the final blow-up destroys the jet");
}

// ---- DemoTrapUpdate ------------------------------------------------------------------------

#[test]
fn demo_trap_detonates_on_a_nearby_enemy() {
    let mut g = GameLogic::new(flat(), 1);
    let mut trap = ObjectTemplate::new("Trap");
    trap.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;
    trap.geometry.major_radius = 10.0;
    trap.max_health = 50.0;
    trap.initial_health = 50.0;
    trap.demo_trap = Some(DemoTrapTemplate {
        detonation_weapon: Some(Arc::new(weapon("TrapBoom", 9_999.0, 60.0))),
        trigger_detonation_range: 40.0,
        scan_frames: 1,
        ignore_kind_of: 0,
        friendly_detonation: true,
        detonate_when_killed: true,
        default_proximity_mode: true,
    });
    trap.die.push(DieModuleTemplate { mux: all_mux(), kind: DieTemplate::Destroy });
    let t = g.create_object(&Arc::new(trap), Coord3D::new(300.0, 300.0, 0.0), 0.0, 0);
    let e = g.create_object(&Arc::new(ground_unit("Enemy")), Coord3D::new(320.0, 300.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.run_frames(6);
    assert!(g.find_object(t).is_none(), "the trap detonated");
    assert!(g.find_object(e).is_none_or(|o| o.is_dead() || o.health() <= 0.0), "the enemy was caught in the blast");
}

// ---- CleanupHazardUpdate -------------------------------------------------------------------

#[test]
fn cleanup_hazard_attacks_a_hazard() {
    let mut g = GameLogic::new(flat(), 1);
    let mut cleaner_t = with_weapon(ground_unit("Cleaner"), weapon("Clean", 100.0, 10.0));
    cleaner_t.cleanup_hazard = Some(CleanupHazardTemplate { scan_frames: 1, scan_range: 150.0 });
    let c = g.create_object(&Arc::new(cleaner_t), Coord3D::new(200.0, 300.0, 0.0), 0.0, 0);
    let mut hazard_t = ObjectTemplate::new("Hazard");
    hazard_t.kind_of = kind_of::CLEANUP_HAZARD;
    hazard_t.geometry.major_radius = 5.0;
    hazard_t.max_health = 10.0;
    hazard_t.initial_health = 10.0;
    let h = g.create_object(&Arc::new(hazard_t), Coord3D::new(260.0, 300.0, 0.0), 0.0, 1);
    // `CleanupHazardUpdate` has no constructor wake, so `registerObject` gives it `now = max(frame, 1)`
    // (GameLogic.cpp:4039-4046): the first scan is frame 1, not the creation frame.
    g.run_frames(2);
    assert!(matches!(g.find_object(c).unwrap().combat.attack.as_ref().map(|a| a.target), Some(AttackTarget::Object(id)) if id == h));
    g.run_frames(60);
    assert!(g.find_object(h).is_none_or(|o| o.is_dead()), "the hazard was cleaned up");
}

// ---- CommandButtonHuntUpdate ---------------------------------------------------------------

#[test]
fn command_button_hunt_drives_the_ai_hunt() {
    let mut g = GameLogic::new(flat(), 1);
    let mut t = with_weapon(ground_unit("Hunter"), weapon("W", 5.0, 5.0));
    t.command_button_hunt = Some(CommandButtonHuntTemplate { scan_frames: 1, scan_range: 100.0 });
    let id = g.create_object(&Arc::new(t), Coord3D::new(200.0, 300.0, 0.0), 0.0, 0);
    assert!(g.set_hunt_command(id, HuntCommand::Weapon));
    g.run_frames(1);
    assert!(g.find_object(id).unwrap().combat.hunt, "the unit started hunting");
}

#[test]
fn special_power_hunt_scans_and_uses_the_power() {
    use std::collections::BTreeMap;
    use zr_sim::special_power::{SimSpecialPowerTemplate, SpecialAbilityTemplate};
    let mut g = GameLogic::new(flat(), 1);
    g.set_special_powers(BTreeMap::from([(
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
    let mut t = ground_unit("Hacker");
    t.command_button_hunt = Some(CommandButtonHuntTemplate { scan_frames: 1, scan_range: 200.0 });
    // The two halves of a real ability: the `SpecialAbility` power module (its cooldown/pause live there) and the
    // `SpecialAbilityUpdate` module `CommandButtonHuntUpdate::huntSpecialPower` looks up with
    // `findSpecialAbilityUpdate` (CommandButtonHuntUpdate.cpp:207).
    let ability = SpecialAbilityTemplate {
        power: "Disable".to_string(),
        start_ability_range: 200.0,
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
        power_module: true,
    };
    t.special_abilities = vec![
        Arc::new(ability.clone()),
        Arc::new(SpecialAbilityTemplate { power_module: false, ..ability }),
    ];
    let hacker = g.create_object(&Arc::new(t), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let victim = g.create_object(&Arc::new(ground_unit("Victim")), Coord3D::new(150.0, 100.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    assert!(g.set_hunt_command(hacker, HuntCommand::SpecialPower { power: "Disable".into(), object_type: String::new() }));
    g.run_frames(60);
    assert!(g.find_object(victim).unwrap().is_disabled_by_type(zr_sim::disabled::DisabledType::Hacked), "the hunt disabled the enemy");
}

// ---- GenerateMinefieldBehavior -------------------------------------------------------------

#[test]
fn generate_minefield_lays_a_border() {
    let mut g = GameLogic::new(flat(), 1);
    let mut s = SimpleTemplateStore::default();
    let mut mine_t = ObjectTemplate::new("Mine");
    mine_t.kind_of = kind_of::MINE;
    mine_t.geometry.major_radius = 4.0;
    mine_t.max_health = 5.0;
    mine_t.initial_health = 5.0;
    s.add(mine_t);
    let mut gen_src = ObjectTemplate::new("Generator");
    gen_src.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;
    gen_src.geometry.major_radius = 20.0;
    gen_src.max_health = 100.0;
    gen_src.initial_health = 100.0;
    gen_src.generate_minefield = Some(GenerateMinefieldTemplate {
        mine_name: Some("Mine".into()),
        upgraded_mine_name: None,
        upgraded_trigger: None,
        distance_around_object: 40.0,
        mines_per_square_foot: 0.0,
        generate_only_on_death: false,
        border_only: true,
        smart_border: false,
        smart_border_skip_interior: true,
        always_circular: false,
        upgradable: false,
        random_jitter: 0.0,
        skip_if_this_much_under_structure: 0.33,
        starts_active: true,
        triggered_by: vec![],
        conflicts_with: vec![],
    });
    let gen_arc = s.add(gen_src);
    g.set_template_source(Arc::new(s));
    let id = g.create_object(&gen_arc, Coord3D::new(300.0, 300.0, 0.0), 0.0, 0);
    g.run_frames(1);
    assert!(g.find_object(id).unwrap().behavior.minefield_generated);
    let mines = g.objects().filter(|o| o.template.name == "Mine").count();
    assert!(mines >= 4, "a border of mines was laid (got {mines})");
}
