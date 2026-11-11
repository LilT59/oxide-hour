//! Synthetic tests for the newly ported die / special-power / superweapon modules.

use std::sync::Arc;

use std::collections::BTreeMap;

use zr_sim::body::DamageInfo;
use zr_sim::damage::{DamageType, DeathType};
use zr_sim::die::{DieMux, DieModuleTemplate, DieTemplate};
use zr_sim::special_power::{SimSpecialPower, SimSpecialPowerTemplate, SpecialPowerEffect, SpecialPowerTarget};
use zr_sim::superweapon::{
    door_state, HeightDieTemplate, MissileLauncherBuildingTemplate, NeutronBlastTemplate, ParticleUplinkTemplate, SpyVisionTemplate,
};
use zr_sim::template::kind_of;
use zr_sim::{Command, Coord3D, GameLogic, ObjectTemplate, ObjectId, TemplateSource, TerrainLogic};

fn install_power_with_reload(g: &mut GameLogic, name: &str, reload: u32) {
    let mut map = BTreeMap::new();
    map.insert(
        name.to_string(),
        Arc::new(SimSpecialPowerTemplate {
            name: name.to_string(),
            kind: String::new(),
            reload_time: reload,
            required_science: None,
            shared_synced_timer: false,
            public_timer: false,
            radius_cursor_radius: 0.0,
        }),
    );
    g.set_special_powers(map);
}

fn install_power(g: &mut GameLogic, name: &str) {
    install_power_with_reload(g, name, 0);
}

struct FixedStore(Vec<Arc<ObjectTemplate>>);
impl TemplateSource for FixedStore {
    fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
        self.0.iter().find(|t| t.name == name).cloned()
    }
}

fn mux_all() -> DieMux {
    DieMux { death_types: u32::MAX, veterancy_levels: u32::MAX, exempt_status: 0, required_status: 0 }
}

fn body_template(name: &str, health: f32) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.max_health = health;
    t.initial_health = health;
    t
}

fn kill(g: &mut GameLogic, id: ObjectId) {
    g.damage_object(id, DamageInfo { source: ObjectId::INVALID, amount: 9999.0, kind: DamageType::Unresistable });
}

#[test]
fn upgrade_die_removes_the_producers_upgrade() {
    let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 1);
    let mut producer_t = body_template("Producer", 100.0);
    // A dummy upgrade module so `update_upgrade_modules` is happy; the set is what matters.
    producer_t.upgrade_modules = Vec::new();
    let producer = g.create_object(&Arc::new(producer_t), Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);

    let mut drone_t = body_template("Drone", 50.0);
    drone_t.die = vec![DieModuleTemplate {
        mux: mux_all(),
        kind: DieTemplate::Upgrade { upgrade: Some("Upgrade_Drone".into()) },
    }];
    let drone = g.create_object(&Arc::new(drone_t), Coord3D::new(20.0, 10.0, 0.0), 0.0, 0);
    g.set_producer(drone, producer);
    g.give_upgrade(producer, "Upgrade_Drone");
    assert!(g.object_upgrades(producer).any(|u| u == "Upgrade_Drone"));

    kill(&mut g, drone);
    g.update();
    assert!(!g.object_upgrades(producer).any(|u| u == "Upgrade_Drone"), "UpgradeDie removed the producer's upgrade");
}

#[test]
fn special_power_completion_die_records_the_event() {
    let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 1);
    let mut missile_t = body_template("Missile", 20.0);
    missile_t.die = vec![DieModuleTemplate {
        mux: mux_all(),
        kind: DieTemplate::SpecialPowerCompletion { power: "SuperweaponNuke".into() },
    }];
    let missile = g.create_object(&Arc::new(missile_t), Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);
    // `SpecialPowerCompletionDie::setCreator`.
    g.set_sp_creator(missile, ObjectId(999));
    assert!(!g.completed_special_power_seen(0));

    kill(&mut g, missile);
    g.update();
    assert!(g.completed_special_power_seen(0), "the payload reported its creator's power complete");
}

#[test]
fn height_die_kills_the_object_on_descent() {
    let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 1);
    let mut bomb_t = body_template("Bomb", 10.0);
    bomb_t.height_die = Some(HeightDieTemplate {
        target_height: 5.0,
        target_height_includes_structures: false,
        only_when_moving_down: false,
        destroy_attached_particles_at_height: -1.0,
        snap_to_ground_on_death: true,
        initial_delay: 0,
    });
    let bomb = g.create_object(&Arc::new(bomb_t), Coord3D::new(25.0, 25.0, 20.0), 0.0, 0);
    g.update();
    assert!(g.find_object(bomb).is_some(), "above the target height it stays alive");

    g.set_object_position(bomb, Coord3D::new(25.0, 25.0, 1.0));
    g.update();
    assert!(g.find_object(bomb).is_none(), "below the target height it dies");
}

#[test]
fn cash_bounty_pays_on_kill() {
    let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_player_cash_bounty(0, 0.25);
    let mut victim_t = body_template("Victim", 100.0);
    victim_t.build_cost = 1000;
    let victim = g.create_object(&Arc::new(victim_t), Coord3D::new(20.0, 20.0, 0.0), 0.0, 1);
    let attacker = g.create_object(&Arc::new(body_template("Attacker", 100.0)), Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);
    g.set_enemies(0, 1);
    assert_eq!(g.player_money(0), 0);
    g.damage_object(victim, DamageInfo { source: attacker, amount: 9999.0, kind: DamageType::Unresistable });
    assert_eq!(g.player_money(0), 250, "25% of the 1000 build cost");
}

#[test]
fn spy_vision_activates_then_expires() {
    let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 1);
    let mut sc_t = body_template("StrategyCenter", 100.0);
    sc_t.spy_vision = Some(SpyVisionTemplate { needs_upgrade: false, self_powered: false, self_powered_duration: 0, self_powered_interval: 0, spy_on_kind_of: 0 });
    let sc = g.create_object(&Arc::new(sc_t), Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);
    g.activate_spy_vision(sc, 10);
    g.update();
    assert!(g.spy_vision_active(sc));
    g.run_frames(20);
    assert!(!g.spy_vision_active(sc), "spy vision expired");
}

#[test]
fn template_source_is_used_by_the_demo_path() {
    // Sanity: `set_template_source` resolves a template by name.
    let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 1);
    let t = Arc::new(body_template("Resolved", 1.0));
    g.set_template_source(Arc::new(FixedStore(vec![t.clone()])));
    assert!(g.find_template("Resolved").is_some());
    assert!(g.find_template("Missing").is_none());
}

fn slow_with_special(special: zr_sim::die::SpecialDeathExtras, destruction_delay: u32) -> zr_sim::die::SlowDeathTemplate {
    zr_sim::die::SlowDeathTemplate {
        probability_modifier: 10,
        modifier_bonus_per_overkill_percent: 0.0,
        sink_rate: 0.0,
        sink_delay: 0,
        sink_delay_variance: 0,
        destruction_delay,
        destruction_delay_variance: 0,
        destruction_altitude: -10.0,
        fx: [Vec::new(), Vec::new(), Vec::new()],
        ocl: [Vec::new(), Vec::new(), Vec::new()],
        weapons: [Vec::new(), Vec::new(), Vec::new()],
        fling_force: 0.0,
        fling_force_variance: 0.0,
        fling_pitch: 0.0,
        fling_pitch_variance: 0.0,
        special: Some(special),
    }
}

#[test]
fn helicopter_slow_death_spirals_down_and_destroys() {
    let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 1);
    let mut heli_t = body_template("Heli", 100.0);
    heli_t.die = vec![DieModuleTemplate {
        mux: mux_all(),
        kind: DieTemplate::SlowDeath(Arc::new(slow_with_special(
            zr_sim::die::SpecialDeathExtras::Helicopter(zr_sim::die::HelicopterExtras {
                spiral_orbit_turn_rate: 0.1,
                spiral_orbit_forward_speed: 1.0,
                fall_how_fast: 0.5,
                min_blade_fly_off_delay: 3.0,
                max_blade_fly_off_delay: 3.0,
                delay_from_ground_to_final_death: 5.0,
                ..Default::default()
            }),
            999_999,
        ))),
    }];
    let heli = g.create_object(&Arc::new(heli_t), Coord3D::new(25.0, 25.0, 40.0), 0.0, 0);
    kill(&mut g, heli);
    g.run_frames(400);
    assert!(g.find_object(heli).is_none(), "the helicopter reached the ground and blew up");
}

#[test]
fn neutron_slow_death_blasts_damage_nearby_objects() {
    let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 1);
    let mut victim_t = body_template("Victim", 5000.0);
    victim_t.geometry.major_radius = 10.0;
    let victim = g.create_object(&Arc::new(victim_t), Coord3D::new(30.0, 25.0, 0.0), 0.0, 1);
    let mut warhead_t = body_template("Warhead", 10.0);
    warhead_t.die = vec![DieModuleTemplate {
        mux: mux_all(),
        kind: DieTemplate::SlowDeath(Arc::new(slow_with_special(
            zr_sim::die::SpecialDeathExtras::Neutron(zr_sim::die::NeutronExtras {
                blasts: vec![zr_sim::die::NeutronBlast {
                    enabled: true,
                    delay: 2.0,
                    scorch_delay: 0.0,
                    inner_radius: 20.0,
                    outer_radius: 40.0,
                    max_damage: 1000.0,
                    min_damage: 100.0,
                    topple_speed: 0.0,
                    push_force: 0.0,
                }],
                ..Default::default()
            }),
            60,
        ))),
    }];
    let warhead = g.create_object(&Arc::new(warhead_t), Coord3D::new(25.0, 25.0, 0.0), 0.0, 0);
    kill(&mut g, warhead);
    g.run_frames(10);
    assert!(g.find_object(victim).map_or(0.0, |o| o.health()) < 5000.0, "the neutron blast damaged the nearby victim");
}

#[test]
fn neutron_blast_kills_infantry_and_unmans_vehicles() {
    let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.add_player(2, 0);
    g.set_neutral_player(Some(2));
    g.set_enemies(0, 1);
    let mut shell_t = body_template("Shell", 10.0);
    shell_t.die = vec![DieModuleTemplate {
        mux: mux_all(),
        kind: DieTemplate::NeutronBlast(NeutronBlastTemplate { blast_radius: 50.0, affect_airborne: false, affect_allies: true }),
    }];
    let shell = g.create_object(&Arc::new(shell_t), Coord3D::new(20.0, 20.0, 0.0), 0.0, 0);
    let mut inf_t = body_template("Inf", 100.0);
    inf_t.kind_of = kind_of::INFANTRY | kind_of::SELECTABLE;
    let inf = g.create_object(&Arc::new(inf_t), Coord3D::new(30.0, 20.0, 0.0), 0.0, 1);
    let mut veh_t = body_template("Veh", 100.0);
    veh_t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    let veh = g.create_object(&Arc::new(veh_t), Coord3D::new(40.0, 20.0, 0.0), 0.0, 1);

    kill(&mut g, shell);
    g.run_frames(2);
    assert!(g.find_object(inf).is_none_or(|o| o.is_dead()), "infantry is killed");
    assert!(g.find_object(veh).unwrap().is_disabled_by_type(zr_sim::disabled::DisabledType::Unmanned), "the vehicle is unmanned");
    assert_eq!(g.find_object(veh).unwrap().player, 2, "the unmanned vehicle turns neutral");
}

#[test]
fn particle_uplink_swath_moves_and_damages() {
    let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
    install_power(&mut g, "PUC");
    let mut cannon_t = body_template("Cannon", 5000.0);
    cannon_t.kind_of = kind_of::STRUCTURE;
    cannon_t.particle_uplink = Some(ParticleUplinkTemplate {
        special_power: Some("PUC".into()),
        total_firing_frames: 60,
        beam_travel_frames: 10,
        width_grow_frames: 10,
        swath_of_death_distance: 100.0,
        swath_of_death_amplitude: 20.0,
        total_damage_pulses: 10,
        damage_per_second: 400.0,
        damage_radius_scalar: 3.0,
        beam_radius: 13.0,
        damage_type: DamageType::Explosion,
        death_type: DeathType::Normal,
        ..Default::default()
    });
    cannon_t.special_powers =
        vec![Arc::new(SimSpecialPower { power: "PUC".into(), effect: SpecialPowerEffect::ParticleUplink, starts_paused: false, power_module: false })];
    let cannon = g.create_object(&Arc::new(cannon_t), Coord3D::new(400.0, 400.0, 0.0), 0.0, 0);
    let mut target_t = body_template("Target", 5000.0);
    target_t.kind_of = kind_of::STRUCTURE;
    let target = g.create_object(&Arc::new(target_t), Coord3D::new(1200.0, 400.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);

    g.submit(0, Command::DoSpecialPower { source: cannon, power: "PUC".into(), target: SpecialPowerTarget::Object(target) });
    g.run_frames(40);
    let st = g.find_object(cannon).unwrap().superweapon.puc.clone().expect("firing");
    assert!(st.current_target.dist_2d(&st.initial_target) > 1.0, "the beam swaths off the initial target");
    assert!(g.find_object(target).map_or(0.0, |o| o.health()) < 5000.0, "the moving beam damaged the target");

    g.run_frames(60);
    assert!(g.find_object(cannon).unwrap().superweapon.puc.is_none(), "the firing sequence ends");
}

#[test]
fn spy_vision_reveals_the_enemys_vision() {
    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    let far = Coord3D::new(700.0, 700.0, 0.0);
    let mut spy_t = body_template("SpyCenter", 100.0);
    spy_t.spy_vision = Some(SpyVisionTemplate { needs_upgrade: false, self_powered: false, self_powered_duration: 0, self_powered_interval: 0, spy_on_kind_of: kind_of::INFANTRY });
    let spy = g.create_object(&Arc::new(spy_t), Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);
    let mut inf_t = body_template("EnemyInfantry", 100.0);
    inf_t.kind_of = kind_of::INFANTRY | kind_of::SELECTABLE;
    inf_t.vision_range = 100.0;
    let inf = g.create_object(&Arc::new(inf_t), far, 0.0, 1);

    g.update();
    assert_eq!(
        g.cell_shroud_status(0, &far),
        zr_sim::CellShroudStatus::Shrouded,
        "without spy vision the owner cannot see the enemy's cells"
    );

    g.activate_spy_vision(spy, 0);
    g.run_frames(2);
    assert!(g.spy_vision_active(spy));
    assert_ne!(
        g.cell_shroud_status(0, &far),
        zr_sim::CellShroudStatus::Shrouded,
        "spy vision reveals the enemy unit's own vision to the owner"
    );
    let _ = inf;
}

#[test]
fn missile_launcher_door_opens_and_closes() {
    let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 1);
    install_power_with_reload(&mut g, "Nuke", 100);
    let mut launcher_t = body_template("Launcher", 1000.0);
    launcher_t.missile_launcher = Some(MissileLauncherBuildingTemplate {
        special_power: Some("Nuke".into()),
        door_open_frames: 10,
        door_wait_open_frames: 5,
        door_closing_frames: 10,
    });
    launcher_t.special_powers =
        vec![Arc::new(SimSpecialPower { power: "Nuke".into(), effect: SpecialPowerEffect::FireWeapon, starts_paused: false, power_module: true })];
    let launcher = g.create_object(&Arc::new(launcher_t), Coord3D::new(25.0, 25.0, 0.0), 0.0, 0);
    // The constructor starts the 100-frame recharge (SpecialPowerModule.cpp:116-124); the door
    // opens `door_open_frames` before the ready frame and settles open just after it.
    g.run_frames(110);
    let door = |g: &GameLogic| g.find_object(launcher).unwrap().superweapon.missile_launcher.clone().unwrap_or_default().door_state;
    assert_eq!(door(&g), door_state::OPEN, "the door pops open when the power is ready");

    g.submit(0, Command::DoSpecialPower { source: launcher, power: "Nuke".into(), target: SpecialPowerTarget::None });
    g.run_frames(1);
    assert_eq!(door(&g), door_state::WAITING_TO_CLOSE, "firing queues the door closed");
    g.run_frames(20);
    assert!(door(&g) == door_state::CLOSED || door(&g) == door_state::CLOSING, "the door closes after firing");
}



