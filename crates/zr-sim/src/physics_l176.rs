//! Lane 176 (R20-6): `PhysicsBehavior::getMass()` includes the contained items' mass
//! (`PhysicsUpdate.cpp:1089-1096`: `m_mass + contain->getContainedItemsMass()`, `OpenContain.cpp:835-845`).
//! Every caller is affected — `applyForce` (`:324`), the friction force (`:460`), the bounce
//! (`:519`), the falling damage (`:866`) and the locomotor's force kernels (`Locomotor.cpp:1254`,
//! `:1488`, `:1684`, `:1812`, `:2015`, `:2565`) — so a loaded transport brakes and bounces with the
//! mass of what it carries.

use super::*;
use crate::contain::{ContainKind, ContainTemplate};
use crate::template::{kind_of, LocomotorKind, LocomotorParams, ObjectTemplate};
use crate::terrain::TerrainLogic;
use std::sync::Arc;

fn physics_template(mass: f32) -> PhysicsTemplate {
    PhysicsTemplate { mass, lateral_friction: 0.15, forward_friction: 0.15, ..PhysicsTemplate::default() }
}

fn transport_template(mass: f32, capacity: i32) -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Transport");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 10.0;
    t.max_health = 1000.0;
    t.initial_health = 1000.0;
    t.transport_slot_count = 1;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    t.physics = Some(physics_template(mass));
    t.contain = Some(Arc::new(ContainTemplate {
        kind: ContainKind::Transport,
        capacity,
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
    t.kind_of = kind_of::INFANTRY | kind_of::SELECTABLE;
    t.geometry.major_radius = 4.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.transport_slot_count = 1;
    t.physics = Some(physics_template(mass));
    t
}

/// `getMass` (`PhysicsUpdate.cpp:1089-1096`) sums the contained objects' own `getMass`, so a loaded
/// transport weighs its own mass plus its riders' — and a rider that is itself a container
/// contributes its own contents too (`OpenContain.cpp:835-845` recurses through `phys->getMass()`).
/// Revert check: drop `contained_mass` from `Object::get_mass` and every value here is the bare
/// template mass.
#[test]
fn a_loaded_transport_weighs_its_own_mass_plus_its_riders() {
    let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
    let big = g.create_object(&Arc::new(transport_template(1000.0, 3)), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    let rider = g.create_object(&Arc::new(passenger_template(200.0)), Coord3D::new(60.0, 50.0, 0.0), 0.0, 0);
    let inner = g.create_object(&Arc::new(transport_template(100.0, 1)), Coord3D::new(70.0, 50.0, 0.0), 0.0, 0);
    let nested = g.create_object(&Arc::new(passenger_template(50.0)), Coord3D::new(80.0, 50.0, 0.0), 0.0, 0);
    assert_eq!(g.get_mass(big), 1000.0, "an empty transport weighs its template mass");
    g.add_to_contain(big, rider);
    assert_eq!(g.get_mass(big), 1200.0, "1000 + the 200-mass rider");
    g.add_to_contain(inner, nested);
    assert_eq!(g.get_mass(inner), 150.0, "the inner container: 100 + 50");
    g.add_to_contain(big, inner);
    assert_eq!(g.get_mass(big), 1350.0, "1000 + 200 + (100 + 50), recursively");
    // A removal updates the list the engine walks.
    g.remove_from_contain_ex(big, inner, false);
    assert_eq!(g.get_mass(big), 1200.0, "the nested container left");
    g.remove_from_contain_ex(inner, nested, false);
    assert_eq!(g.get_mass(inner), 100.0, "the nested rider left the inner container");
    // A rider destroyed in the transport leaves its list (`Object::onDestroy` removes it).
    g.destroy_object(rider);
    g.update();
    assert_eq!(g.get_mass(big), 1000.0, "a destroyed rider no longer weighs");
}

/// `applyForce` divides by `getMass()` (`PhysicsUpdate.cpp:324`, `:335-338`): with a loaded
/// transport the reciprocal is the loaded mass's, not the template's. The force here is 1000 and
/// the masses 1000 vs 1337, so the template-mass form gives exactly 1.0 and the loaded one does
/// not. Revert check: `1.0 / t.mass` gives 1.0 and fails.
#[test]
fn a_force_on_a_loaded_transport_is_divided_by_the_loaded_mass() {
    let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
    let big = g.create_object(&Arc::new(transport_template(1000.0, 1)), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    let rider = g.create_object(&Arc::new(passenger_template(337.0)), Coord3D::new(60.0, 50.0, 0.0), 0.0, 0);
    g.add_to_contain(big, rider);
    g.apply_force(big, Coord3D::new(1000.0, 0.0, 0.0));
    let accel = g.find_object(big).unwrap().physics.as_ref().unwrap().accel.x;
    let expected = 1000.0f32 * (1.0f32 / 1337.0f32);
    assert_eq!(accel.to_bits(), expected.to_bits(), "F / getMass() with 1000 + 337");
    assert_ne!(accel, 1.0, "not F / the template mass (1000)");
}

/// `applyFrictionalForces` builds its force from `getMass()` (`PhysicsUpdate.cpp:460`) and hands it
/// to `applyForce`, which divides by the same value: the loaded mass cancels mathematically but not
/// in float. One forward-friction step must equal the engine's operation order with 1337.
/// Revert check: the template mass (1000) gives a different last bit here.
#[test]
fn the_friction_force_of_a_loaded_transport_uses_the_loaded_mass() {
    let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
    let big = g.create_object(&Arc::new(transport_template(1000.0, 1)), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    let rider = g.create_object(&Arc::new(passenger_template(337.0)), Coord3D::new(60.0, 50.0, 0.0), 0.0, 0);
    g.add_to_contain(big, rider);
    let t = physics_template(1000.0);
    if let Some(p) = g.objects.get_mut(&big).and_then(|o| o.physics.as_mut()) {
        p.velocity = Coord3D::new(10.0, 0.0, 0.0);
        p.motive_force_expires = 0;
    }
    g.apply_frictional_forces(big, &t, 0.0, -1.0);
    let accel = g.find_object(big).unwrap().physics.as_ref().unwrap().accel.x;
    // Angle 0, so the lateral axis is y: only the forward term `-(ff * forwardVel)` is non-zero,
    // and `applyForce` divides it by `getMass()`.
    let ff = 1337.0f32 * 0.15f32;
    let force = -(ff * 10.0f32);
    let expected = force * (1.0f32 / 1337.0f32);
    let template_only = (-(1000.0f32 * 0.15f32 * 10.0f32)) * (1.0f32 / 1000.0f32);
    assert_eq!(accel.to_bits(), expected.to_bits(), "the loaded mass's force and reciprocal");
    assert_ne!(expected.to_bits(), template_only.to_bits(), "the template mass rounds differently");
}

/// `obj->onCollide(nullptr, obj->getPosition(), &normal)` (`PhysicsUpdate.cpp:842-845`) reaches the rider's
/// own collide modules; `PhysicsBehavior::onCollide` forwards a null contact to the container
/// (`:1167-1175`) and `ParachuteContain::onCollide` (`ParachuteContain.cpp:674-687`) releases its riders
/// and kills the chute. Revert check: the old `ground_landing_collision` called the container directly
/// with `on_collide(container, INVALID, ...)`, which never ran the rider's physics module's null branch —
/// or drop the parachute branch and the chute survives.
#[test]
fn a_rider_landing_under_its_parachute_releases_it_and_kills_the_chute() {
    let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
    let mut chute = transport_template(60.0, 1);
    chute.max_health = 100.0;
    chute.initial_health = 100.0;
    if let Some(c) = chute.contain.as_mut() {
        Arc::get_mut(c).expect("unique").kind = ContainKind::Parachute;
    }
    let chute = g.create_object(&Arc::new(chute), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    let rider = g.create_object(&Arc::new(passenger_template(50.0)), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    g.add_to_contain(chute, rider);
    assert_eq!(g.container_of(rider), Some(chute), "the rider is under the chute");
    // The landing block runs when the previous frame was airborne and this one is not
    // (`PhysicsUpdate.cpp:836-845`); the rider is HELD so only the tail runs. `update_ever_run` keeps
    // `PhysicsBehavior::update` from overwriting the airborne flag with the creation frame's.
    if let Some(p) = g.objects.get_mut(&rider).and_then(|o| o.physics.as_mut()) {
        p.was_airborne_last_frame = true;
        p.update_ever_run = true;
    }
    g.update_object_physics(rider);
    assert_eq!(g.container_of(rider), None, "`removeAllContained` released the rider");
    // `Object::kill` damages the chute's body; the object is marked destroyed for the frame-end sweep.
    let chute_obj = g.objects.get(&chute).expect("the chute is still in the object list this frame");
    assert!(chute_obj.destroyed || chute_obj.effectively_dead || chute_obj.is_dying(), "`getObject()->kill()` killed the chute");
}

/// The CRC digest leaves out the port's `pitch`/`roll` diagnostic mirror: `PhysicsBehavior` keeps only
/// the three angular rates (`PhysicsUpdate.h:263-265`) and its `crc` is empty (`PhysicsUpdate.cpp:1851-1859`).
/// Revert check: hash `pitch`/`roll` again (the old `has_angles` terms) and the two objects' digests
/// differ.
#[test]
fn the_crc_does_not_hash_the_ports_euler_mirror() {
    let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
    let a = g.create_object(&Arc::new(transport_template(1000.0, 1)), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(transport_template(1000.0, 1)), Coord3D::new(80.0, 50.0, 0.0), 0.0, 0);
    // `setAngles` builds the matrix; the port also mirrors the pitch/roll it was given.
    g.set_angles(a, 0.0, 0.5, 0.25);
    let digest = |g: &GameLogic, id| {
        let mut c = Crc::new();
        g.find_object(id).unwrap().physics.as_ref().unwrap().crc(&mut c);
        c.get()
    };
    assert_eq!(digest(&g, a), digest(&g, b), "the pitch/roll mirror is not simulation state");
    assert_ne!(g.find_object(a).unwrap().physics.as_ref().unwrap().pitch, 0.0, "the mirror holds the pitch");
}

/// The falling damage is `netSpeed * getMass() * m_fallHeightDamageFactor` (`PhysicsUpdate.cpp:866`),
/// and the bounce force `getMass() * desiredAccelZ` (`:519`): both scale with the carried mass. Two
/// identical transports dropped from the same height — one loaded with 337 mass — take damage in the
/// ratio of their masses, so the loaded one's loss is `netSpeed * 1337` and the empty one's
/// `netSpeed * 1000`. Revert check: `t.mass` gives both the same damage and the ratio is 1.
#[test]
fn the_falling_damage_of_a_loaded_transport_uses_the_loaded_mass() {
    let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
    let falling = |mass: f32| {
        let mut t = transport_template(mass, 1);
        // Small enough that the f32 health's ulp does not swallow the damage's last digits.
        t.max_health = 20_000.0;
        t.initial_health = 20_000.0;
        t.physics = Some(PhysicsTemplate { mass, min_fall_speed_for_damage: 1.0, fall_height_damage_factor: 1.0, ..PhysicsTemplate::default() });
        Arc::new(t)
    };
    let loaded = g.create_object(&falling(1000.0), Coord3D::new(50.0, 50.0, 20.0), 0.0, 0);
    let empty = g.create_object(&falling(1000.0), Coord3D::new(120.0, 50.0, 20.0), 0.0, 0);
    let rider = g.create_object(&Arc::new(passenger_template(337.0)), Coord3D::new(60.0, 50.0, 0.0), 0.0, 0);
    g.add_to_contain(loaded, rider);
    g.add_velocity_to(loaded, Coord3D::new(0.0, 0.0, -4.0));
    g.add_velocity_to(empty, Coord3D::new(0.0, 0.0, -4.0));
    for _ in 0..40 {
        g.update();
    }
    let lost = |id| {
        let h = g.find_object(id).unwrap().body.as_ref().unwrap().health();
        20_000.0f32 - h
    };
    let (loaded_lost, empty_lost) = (lost(loaded), lost(empty));
    assert!(loaded_lost > 0.0 && empty_lost > 0.0, "both transports took falling damage");
    // `netSpeed = -activeVelZ - minFallSpeed` is the same for both (`:872`); only the mass differs.
    let net_loaded = loaded_lost / 1337.0;
    let net_empty = empty_lost / 1000.0;
    assert!((net_loaded - net_empty).abs() < 1e-3, "the same net speed, scaled by 1337 vs 1000: {loaded_lost} vs {empty_lost}");
    assert!((loaded_lost / empty_lost - 1.337).abs() < 1e-3, "the loaded transport takes 1.337 times the damage");
}
