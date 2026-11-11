//! Lane 176 (R20-6): the locomotor's force kernels read `physics->getMass()` (`Locomotor.cpp:1254`,
//! `:1488`, `:1684`, `:1812`, `:2015`, `:2565`), which includes the contained items' mass
//! (`PhysicsUpdate.cpp:1089-1096`). `maintainGoalSpeed` builds `mass * acceleration` and
//! `applyMotiveForce` divides by the same `getMass`, so a loaded transport's force cancels
//! mathematically but not in float.

use super::*;
use crate::contain::{ContainKind, ContainTemplate};
use crate::template::{kind_of, ObjectTemplate};
use crate::PhysicsTemplate;
use std::sync::Arc;

fn transport_template(mass: f32) -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Transport");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 10.0;
    t.max_health = 1000.0;
    t.initial_health = 1000.0;
    t.transport_slot_count = 1;
    t.physics = Some(PhysicsTemplate { mass, ..PhysicsTemplate::default() });
    t.contain = Some(Arc::new(ContainTemplate {
        kind: ContainKind::Transport,
        capacity: 1,
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
        heal_frames: 0.0,
        initial_payload: Vec::new(),
        tunnel_frames_for_full_heal: 1.0,
        is_enclosing_container: true,
        fire_points: Default::default(),
        passengers_in_turret: false,
        weapon_bonus_passed_to_passengers: false,
        immune_to_clear_building_attacks: false,
    }));
    t
}

fn passenger_template(mass: f32) -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Passenger");
    t.kind_of = kind_of::INFANTRY;
    t.geometry.major_radius = 4.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.transport_slot_count = 1;
    t.physics = Some(PhysicsTemplate { mass, ..PhysicsTemplate::default() });
    t
}

/// `maintainGoalSpeed` (`Locomotor.cpp:1249-1268`) with a loaded transport: `accelForce = mass *
/// acceleration` where `mass = getMass()`, then `applyMotiveForce` divides by the same `getMass()`.
/// Revert check: `obj_mass` on the template mass gives the 1000-arithmetic value instead.
#[test]
fn the_treads_goal_speed_force_of_a_loaded_transport_uses_the_loaded_mass() {
    let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
    let id = g.create_object(&Arc::new(transport_template(1000.0)), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let rider = g.create_object(&Arc::new(passenger_template(337.0)), Coord3D::new(110.0, 100.0, 0.0), 0.0, 0);
    g.add_to_contain(id, rider);
    let loco = LocomotorParams::from_ini(LocomotorKind::Treads, 60.0, 120.0, 120.0, 360.0);
    let speed_delta = 10.0f32;
    let acceleration = loco.acceleration;
    let mass = 1337.0f32;
    let mut accel_force = mass * acceleration;
    let max_force_needed = mass * speed_delta;
    if accel_force.abs() > max_force_needed.abs() {
        accel_force = max_force_needed;
    }
    let expected = (accel_force * 1.0) * (1.0 / mass);
    let template_only = (1000.0f32 * acceleration) * (1.0 / 1000.0);
    assert_ne!(expected.to_bits(), template_only.to_bits(), "the two masses round differently");
    {
        let obj = g.objects.get_mut(&id).expect("transport");
        maintain_goal_speed(obj, &loco, speed_delta, 0.0, 1.0, 0);
    }
    let accel = g.find_object(id).unwrap().physics.as_ref().unwrap().accel.x;
    assert_eq!(accel.to_bits(), expected.to_bits(), "mass * acceleration / the same mass, with 1337");
}
