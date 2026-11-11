//! Lane 180 (R21-21): `ParachuteContain::onCollide` (`ParachuteContain.cpp:674-687`) acts only on the ground
//! contact (`other == nullptr`). For any other object its body is empty, so a unit that is trying to enter a chute
//! and touches it is not taken in (the entering logic is `OpenContain::onCollide`, which the chute does not call).

use super::*;
use crate::template::{kind_of, LocomotorKind, LocomotorParams, ObjectTemplate};
use crate::terrain::TerrainLogic;
use std::sync::Arc;

fn chute() -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new("Chute");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.geometry.major_radius = 6.0;
    t.contain = Some(Arc::new(ContainTemplate {
        kind: ContainKind::Parachute,
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
        is_enclosing_container: false,
        fire_points: GarrisonFirePoints::default(),
        passengers_in_turret: false,
        weapon_bonus_passed_to_passengers: false,
        immune_to_clear_building_attacks: false,
    }));
    Arc::new(t)
}

fn infantry() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Infantry");
    t.kind_of = kind_of::INFANTRY | kind_of::SELECTABLE;
    t.geometry.major_radius = 4.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.transport_slot_count = 1;
    t
}

/// A unit whose `enter_target` is the chute, touching it (a non-null `other`): the chute's `onCollide` is empty, so
/// the unit is not contained. Revert check: drop the `ParachuteContain` early return and the unit is added.
#[test]
fn a_chute_does_not_take_in_a_unit_that_touches_it() {
    let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 1);
    let c = g.create_object(&chute(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let mut it = infantry();
    it.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Legs, 20.0, 60.0, 120.0, 180.0));
    let u = g.create_object(&Arc::new(it), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    g.objects.get_mut(&u).unwrap().enter_target = Some(c);
    g.contain_on_collide(c, u);
    assert!(!g.is_contained(u), "a non-ground contact does nothing to a parachute");
}
