//! AI death (`AI_DEAD`), THRUST altitude hold and paratroopers, synthetic data only.
//!
//! Ports under test: `AIUpdateInterface::update`'s death check (`AIUpdate.cpp:1102`) and `AIDeadState`
//! (`AIStates.cpp:1514`); `Locomotor::maintainCurrentPositionThrust` (`Locomotor.cpp:2501`); the
//! `OBJECT_STATUS_PARACHUTING` rider status (`ParachuteContain.cpp:454`, `:473`).

use std::collections::BTreeMap;
use std::sync::Arc;

use zr_sim::ai_states::AiStateType;
use zr_sim::body::DamageInfo;
use zr_sim::damage::DamageType;
use zr_sim::die::{DieModuleTemplate, DieMux, DieTemplate, SlowDeathTemplate};
use zr_sim::ocl::{DeliverPayloadTemplate, OclNugget, OclTemplate};
use zr_sim::physics::PhysicsTemplate;
use zr_sim::special_power::{SimSpecialPower, SimSpecialPowerTemplate, SpecialPowerEffect, SpecialPowerTarget};
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::weapon::{CombatTemplate, WeaponSetTemplate, WeaponTemplate};
use zr_sim::{Command, Coord3D, GameLogic, ObjectId, ObjectTemplate, TerrainLogic};

fn flat() -> TerrainLogic {
    TerrainLogic::flat(300, 300, 0)
}

/// A mobile ground unit with a treads locomotor and physics.
fn unit(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 8.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.physics = Some(PhysicsTemplate::default());
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    t
}

fn infantry(name: &str) -> ObjectTemplate {
    let mut t = unit(name);
    t.kind_of = kind_of::INFANTRY | kind_of::SELECTABLE;
    t
}

/// A `SlowDeath` die module: the body lingers on the map for `frames` after it dies.
fn lingers_when_dead(mut t: ObjectTemplate, frames: u32) -> ObjectTemplate {
    let sd = SlowDeathTemplate {
        probability_modifier: 1,
        modifier_bonus_per_overkill_percent: 0.0,
        sink_rate: 0.0,
        sink_delay: 0,
        sink_delay_variance: 0,
        destruction_delay: frames,
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
    let mux = DieMux { death_types: u32::MAX, veterancy_levels: u32::MAX, exempt_status: 0, required_status: 0 };
    t.die.push(DieModuleTemplate { mux, kind: DieTemplate::SlowDeath(Arc::new(sd)) });
    t
}

fn kill(g: &mut GameLogic, id: ObjectId) {
    g.damage_object(id, DamageInfo { source: ObjectId::INVALID, amount: 10_000.0, kind: DamageType::Explosion });
}

/// `AIUpdateInterface::update` (`AIUpdate.cpp:1102`): a killed AI is switched to `AI_DEAD` and locked.
#[test]
fn dead_ai_switches_to_ai_dead_and_is_locked() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    let v = g.create_object(&Arc::new(lingers_when_dead(unit("Dead"), 300)), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    assert_eq!(g.find_object(v).unwrap().ai.machine.current, AiStateType::Idle);
    kill(&mut g, v);
    g.run_frames(2);
    let o = g.find_object(v).expect("the dead vehicle lingers");
    assert!(o.is_dying());
    assert_eq!(o.ai.machine.current, AiStateType::Dead, "the dead AI is in AI_DEAD");
    assert!(o.ai.machine.locked, "AI_DEAD locks the machine");
}

/// A locked `AI_DEAD` machine refuses new orders (`StateMachine::setState`, `StateMachine.cpp:544`),
/// and `doLocomotor` does not move a dead unit (`AIUpdate.cpp:2154`).
#[test]
fn dead_ai_ignores_later_orders() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    let v = g.create_object(&Arc::new(lingers_when_dead(unit("Dead"), 300)), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    kill(&mut g, v);
    g.run_frames(2);
    g.submit(0, Command::MoveTo { ids: vec![v], pos: Coord3D::new(200.0, 100.0, 0.0) });
    g.run_frames(30);
    let o = g.find_object(v).expect("still on the map");
    assert_eq!(o.ai.machine.current, AiStateType::Dead);
    assert!(o.position.dist_2d(&Coord3D::new(100.0, 100.0, 0.0)) < 0.01, "a dead unit does not move: {:?}", o.position);
}

/// `AIDeadState::update` (`AIStates.cpp:1525`): an infantry's 2D velocity is scrubbed to 80% a frame,
/// so a dead infantry that was sliding stops within about a second.
#[test]
fn dead_infantry_scrubs_its_sliding_velocity() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    let i = g.create_object(&Arc::new(lingers_when_dead(infantry("Inf"), 300)), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    kill(&mut g, i);
    g.run_frames(1);
    g.add_velocity_to(i, Coord3D::new(2.0, 0.0, 0.0));
    g.run_frames(30);
    let v = g.find_object(i).unwrap().physics.as_ref().unwrap().velocity;
    assert!(v.x.abs() + v.y.abs() < 0.05, "the dead infantry slowed to a stop: {v:?}");
}

/// A THRUST plane with no lift and no orders. THRUST has no Z behaviour and no lift (`Locomotor.cpp:400`).
fn thrust_plane(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::AIRCRAFT | kind_of::SELECTABLE | kind_of::VEHICLE;
    t.geometry.major_radius = 8.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.physics = Some(PhysicsTemplate::default());
    // Engine units per frame. Gravity is -1.0 per frame^2, so the thrust must beat it (acceleration 3.0).
    // `MaxThrustAngle` lets the thrust point up (`Locomotor.cpp:1969`); `MinSpeed` is the cruise speed.
    let mut l = LocomotorParams::from_engine_units(LocomotorKind::Thrust, 2.0, 1.0, 3.0, 0.5, 0.1, false);
    l.max_thrust_angle = 1.0;
    t.locomotor = Some(l);
    t
}

/// `Locomotor::locoUpdate_maintainCurrentPosition` -> `maintainCurrentPositionThrust` (`Locomotor.cpp:2501`):
/// an idle THRUST plane thrusts toward its latched maintain point (`m_maintainPos`, `Locomotor.cpp:2433`),
/// not toward its current position, so the first frames do not fall straight down. This test checks only
/// those first 10 frames. Sustained flight is not held: the kernel always thrusts at `Acceleration` when
/// `Braking` is 0 (as the shipped missiles set it), and it overshoots the latch point and oscillates.
#[test]
fn idle_thrust_plane_does_not_free_fall() {
    let mut g = GameLogic::new(flat(), 1);
    let p = g.create_object(&Arc::new(thrust_plane("Plane")), Coord3D::new(100.0, 100.0, 60.0), 0.0, 0);
    // Start at cruise speed: from rest the plane thrusts forward until it is faster than `MinSpeed`,
    // then brakes with a reversed thrust vector (`moveTowardsPositionThrust`), which is not what this test is about.
    g.add_velocity_to(p, Coord3D::new(1.0, 0.0, 0.0));
    g.run_frames(10);
    let o = g.find_object(p).unwrap();
    assert!(o.position.z > 40.0, "the plane did not free-fall (z {})", o.position.z);
}

/// A DeliverPayload drop with `ParachuteDirectly` (`DeliverPayloadAIUpdate.cpp:743`): the paratrooper
/// descends with `OBJECT_STATUS_PARACHUTING` set and loses it on landing (`ParachuteContain.cpp:473`).
#[test]
fn paratrooper_is_parachuting_until_it_lands() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    let mut map = BTreeMap::new();
    map.insert(
        "Drop".to_string(),
        Arc::new(SimSpecialPowerTemplate {
            name: "Drop".into(),
            kind: String::new(),
            reload_time: 0,
            required_science: None,
            shared_synced_timer: false,
            public_timer: false,
            radius_cursor_radius: 0.0,
        }),
    );
    g.set_special_powers(map);
    let mut set = WeaponSetTemplate::default();
    set.weapons[0] = Some(Arc::new(WeaponTemplate::new("Cannon")));
    let mut transport = unit("Transport");
    transport.combat = CombatTemplate { weapon_set: Some(set), ..CombatTemplate::default() };
    let ocl = Arc::new(OclTemplate {
        name: "DropOcl".into(),
        nuggets: vec![OclNugget::DeliverPayload(Arc::new(DeliverPayloadTemplate {
            transport: Some(Arc::new(transport.clone())),
            payload: vec![(Arc::new(unit("Paratrooper")), 1)],
            delivery_distance: 300.0,
            drop_delay: 5,
            delay_delivery_frames_max: 0,
            drop_offset: (0.0, 0.0, 0.0),
            drop_variance: (0.0, 0.0, 0.0),
            is_parachute_directly: true,
            self_destruct_object: false,
            fire_weapon: false,
            strafing_weapon_slot: 0,
            strafe_length: 0.0,
        }))],
    });
    transport.special_powers = vec![Arc::new(SimSpecialPower {
        power: "Drop".into(),
        effect: SpecialPowerEffect::Ocl { ocl: Some(ocl), create_at_target: true, create_location: zr_sim::ocl::CreateLocation::AtLocation },
        starts_paused: false,
        power_module: true,
    })];
    let src = g.create_object(&Arc::new(transport), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    g.submit(0, Command::DoSpecialPower { source: src, power: "Drop".into(), target: SpecialPowerTarget::Position(Coord3D::new(200.0, 100.0, 0.0)) });

    let mut seen_parachuting = false;
    let mut landed_clear = false;
    for _ in 0..400 {
        g.run_frames(1);
        let frame = g.frame();
        let troopers: Vec<bool> = g
            .objects()
            .filter(|o| o.template.name == "Paratrooper")
            .map(|o| o.status_mask(frame) & zr_sim::object::status::PARACHUTING != 0)
            .collect();
        if troopers.iter().any(|&p| p) {
            seen_parachuting = true;
        } else if seen_parachuting && !troopers.is_empty() {
            landed_clear = true;
            break;
        }
    }
    assert!(seen_parachuting, "the paratrooper descended with the PARACHUTING status");
    assert!(landed_clear, "the status is cleared when the paratrooper lands");
}
