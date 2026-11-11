//! Lane 154 (wave 18): a crusher only ignores a unit it can crush (`AIUpdate.cpp:1304`,
//! `obj->canCrushOrSquish(other, TEST_CRUSH_OR_SQUISH)`, `Object.cpp:1121`). The port's `blocked_by`
//! returned "not blocked" for every unit with `CrusherLevel > 0`, so the China dozer (`CrusherLevel 2`,
//! trees only) walked through a vehicle it cannot crush (`CrushableLevel 2`). AN1 s1 f1604: the dozer
//! 211 is blocked by dozer 230 in the C++ and not in the port.

use super::*;
use crate::physics::PhysicsTemplate;
use crate::template::{kind_of, LocomotorKind, LocomotorParams, ObjectTemplate};
use std::sync::Arc;

/// A treaded vehicle with the given crush levels, mass 10, and a `radius` of about 5.
fn vehicle(name: &str, crusher: u8, crushable: u8) -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 5.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.physics = Some(PhysicsTemplate { mass: 10.0, ..PhysicsTemplate::default() });
    t.locomotor = Some(LocomotorParams::from_engine_units(LocomotorKind::Treads, 1.0, 0.0, 0.1, 0.1, 0.1, true));
    t.crush.crusher_level = crusher;
    t.crush.crushable_level = crushable;
    Arc::new(t)
}

/// A `(me, other)` pair 4 units apart on the x axis, both facing +x, with `me` moving toward a goal far
/// ahead (so the "near the goal" exemption does not apply) and `other` standing still.
fn pair(me_t: &Arc<ObjectTemplate>, other_t: &Arc<ObjectTemplate>) -> (GameLogic, ObjectId, ObjectId) {
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    let me = g.create_object(me_t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let other = g.create_object(other_t, Coord3D::new(104.0, 100.0, 0.0), 0.0, 1);
    let o = g.objects.get_mut(&me).unwrap();
    o.ai.move_goal = Some(Coord3D::new(200.0, 100.0, 0.0));
    o.ai.waiting_for_path = true;
    (g, me, other)
}

#[test]
fn a_dozer_cannot_crush_a_vehicle_of_its_own_crushable_level() {
    // CrusherLevel 2 against CrushableLevel 2: `crusherLevel > crushableLevel` is false (`Object.cpp:1152`).
    let dozer = vehicle("Dozer", 2, 0);
    let vehicle_t = vehicle("Tank", 0, 2);
    let (g, me, other) = pair(&dozer, &vehicle_t);
    assert!(!g.can_crush_or_squish(me, other));
    assert!(g.unit_blocked_by(me, other), "a dozer is blocked by a vehicle it cannot crush");
}

#[test]
fn a_crusher_runs_over_a_vehicle_below_its_level() {
    // CrusherLevel 3 against CrushableLevel 2: the crusher runs over it (`canCrush`), so it is not blocked.
    let heavy = vehicle("Heavy", 3, 0);
    let vehicle_t = vehicle("Tank", 0, 2);
    let (g, me, other) = pair(&heavy, &vehicle_t);
    assert!(g.can_crush_or_squish(me, other));
    assert!(!g.unit_blocked_by(me, other), "a crusher that can crush the other does not block on it");
}

#[test]
fn a_non_crusher_is_blocked_by_anything_in_its_way() {
    let plain = vehicle("Plain", 0, 0);
    let vehicle_t = vehicle("Tank", 0, 2);
    let (g, me, other) = pair(&plain, &vehicle_t);
    assert!(g.unit_blocked_by(me, other));
}

/// A BOX vehicle (the China dozer's geometry: `GeometryMajorRadius 17`, `GeometryMinorRadius 10`).
fn boxed(name: &str) -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.shape = crate::template::GeometryShape::Box;
    t.geometry.major_radius = 17.0;
    t.geometry.minor_radius = 10.0;
    t.geometry.height = 15.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.physics = Some(PhysicsTemplate { mass: 10.0, ..PhysicsTemplate::default() });
    t.locomotor = Some(LocomotorParams::from_engine_units(LocomotorKind::Treads, 1.0, 0.0, 0.1, 0.1, 0.1, true));
    Arc::new(t)
}

#[test]
fn a_contact_is_the_geometry_test_not_the_bounding_circle() {
    // 36 units apart along the heading: the bounding circles (19.7 each) overlap, the boxes do not
    // (17 + 17 = 34 < 36). `PartitionData::collidesWith` (`PartitionManager.cpp:2017`) is the box test, so
    // the blocked-by pass (`processCollision`) never runs for this pair.
    let mover = boxed("Mover");
    let stopped = boxed("Stopped");
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    let me = g.create_object(&mover, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let other = g.create_object(&stopped, Coord3D::new(136.0, 100.0, 0.0), 0.0, 1);
    {
        let o = g.objects.get_mut(&me).unwrap();
        o.ai.move_goal = Some(Coord3D::new(300.0, 100.0, 0.0));
        o.ai.waiting_for_path = true;
        // A moving unit is in its move state with `m_isMoving` set (`isMoving`, AIUpdate.cpp:3196-3202).
        o.ai.machine.current = crate::ai_states::AiStateType::MoveTo;
        o.ai.friend_starting_move();
    }
    assert!(!g.geometry_contact(me, other), "the boxes do not touch");
    g.update_partition();
    assert!(!g.objects[&me].ai.is_blocked, "no contact, no block");
    // The same pair 30 units apart (inside the boxes): a contact, and the mover is blocked. `setPosition`
    // (`Thing.cpp:177`) is what dirties the partition (`reactToTransformChange`, `Object.cpp:1817`); a raw
    // position write would leave the COI cells where they were and the contact unchecked.
    g.objects.get_mut(&other).unwrap().set_position(Coord3D::new(130.0, 100.0, 0.0));
    g.update_partition();
    assert!(g.geometry_contact(me, other));
    assert!(g.objects[&me].ai.is_blocked, "a contact with the stopped unit blocks the mover");
}
