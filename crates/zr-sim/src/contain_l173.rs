//! Lane 173 (wave 20): R19-9 — `TransportContain::onRemoving`'s `AllowToFall` test is `Object::isAboveTerrain`
//! (`TransportContain.cpp:386-391`): height above the *terrain* ground height, `> 0` and with no epsilon
//! (`Thing.h:129`, `Thing.cpp:293-311`).

use super::*;
use crate::template::{kind_of, ObjectTemplate};
use crate::terrain::TerrainLogic;
use std::sync::Arc;

fn transport() -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new("Transport");
    t.kind_of = kind_of::VEHICLE | kind_of::TRANSPORT | kind_of::SELECTABLE;
    t.max_health = 300.0;
    t.initial_health = 300.0;
    t.geometry.major_radius = 10.0;
    t.contain = Some(Arc::new(ContainTemplate {
        kind: ContainKind::Transport,
        capacity: 8,
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

fn rider() -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new("Rider");
    t.kind_of = kind_of::INFANTRY | kind_of::SELECTABLE;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.geometry.major_radius = 3.0;
    t.physics = Some(crate::physics::PhysicsTemplate { mass: 10.0, ..crate::physics::PhysicsTemplate::default() });
    Arc::new(t)
}

/// A rider leaving a transport that stands a fraction of a unit above the terrain may fall: `Object::isAboveTerrain` is
/// `getHeightAboveTerrain() > 0.0f` with no epsilon (`Thing.cpp:293-311`), and the port's `+ 0.001` held it back.
/// A transport exactly on the ground does not. Revert check: put the `surface + 0.001` test back; the first rider keeps
/// `allow_to_fall` false.
#[test]
fn a_rider_leaving_a_transport_just_above_the_ground_may_fall() {
    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
    g.add_player(0, 0);
    let high = g.create_object(&transport(), Coord3D::new(50.0, 50.0, 0.0005), 0.0, 0);
    let a = g.create_object(&rider(), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    assert!(g.contain_remove_unit(high, a, false));
    assert!(
        g.find_object(a).and_then(|o| o.physics.as_ref()).is_some_and(|p| p.allow_to_fall),
        "a transport above the terrain lets its rider fall, whatever the tiny height"
    );
    let low = g.create_object(&transport(), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    let b = g.create_object(&rider(), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    assert!(g.contain_remove_unit(low, b, false));
    assert!(
        g.find_object(b).and_then(|o| o.physics.as_ref()).is_some_and(|p| !p.allow_to_fall),
        "a grounded transport does not"
    );
}
