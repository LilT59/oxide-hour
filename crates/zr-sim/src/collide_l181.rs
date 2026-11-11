//! Lane 181 (wave 21): the contact-list pass in the engine's order, and the AI `processCollision` moving branch
//! inside the physics `onCollide`.
//!
//! `PartitionManager::update` (`PartitionManager.cpp:2808-2840`) takes the dirty modules from the head of the dirty
//! list (`prependToDirtyModules`, `PartitionManager.h:564`), places each one's cells and gathers its pairs before the
//! next module is taken. `addToContactList` prepends each new pair, and `processContactList` (`:2557`) walks the list
//! from its head, re-testing each pair's geometry (`friend_collidesWith`, `:2567`), skipping `NO_COLLISIONS`
//! (`:2573`), calling `onCollide` on the gathered object and then on the other, with the normal flipped (`:2586-2595`).
//! `PhysicsBehavior::onCollide` calls `processCollision` before its push (`PhysicsUpdate.cpp:1278`).

#[cfg(test)]
mod tests {
    use crate::collide::ON_COLLIDE_LOG;
    use crate::logic::GameLogic;
    use crate::object::{status, ObjectId};
    use crate::physics::PhysicsTemplate;
    use crate::template::{kind_of, GeometryShape, LocomotorKind, LocomotorParams, ObjectTemplate};
    use crate::terrain::TerrainLogic;
    use crate::Coord3D;
    use std::sync::Arc;

    /// A round object with no AI and no physics module: a contact needs only its geometry and its cells.
    fn pebble() -> Arc<ObjectTemplate> {
        let mut t = ObjectTemplate::new("Pebble");
        t.geometry.shape = GeometryShape::Sphere;
        t.geometry.major_radius = 5.0;
        t.geometry.minor_radius = 5.0;
        Arc::new(t)
    }

    /// The China dozer's box (`MajorRadius 17`, `MinorRadius 10`), as in `partition_l175`, with or without a physics module.
    fn boxed(physics: bool) -> Arc<ObjectTemplate> {
        let mut t = ObjectTemplate::new("Dozer");
        t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
        t.geometry.shape = GeometryShape::Box;
        t.geometry.major_radius = 17.0;
        t.geometry.minor_radius = 10.0;
        t.geometry.height = 15.0;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        if physics {
            t.physics = Some(PhysicsTemplate { mass: 10.0, ..PhysicsTemplate::default() });
        }
        t.locomotor = Some(LocomotorParams::from_engine_units(LocomotorKind::Treads, 1.0, 0.0, 0.1, 0.1, 0.1, true));
        Arc::new(t)
    }

    /// The `(obj, other)` of every `on_collide` call since the log was cleared, in call order.
    fn calls() -> Vec<(ObjectId, ObjectId)> {
        ON_COLLIDE_LOG.with(|l| l.borrow().iter().map(|&(a, b, _)| (a, b)).collect())
    }

    fn clear_calls() {
        ON_COLLIDE_LOG.with(|l| l.borrow_mut().clear());
    }

    /// Three pebbles on one spot, created in one frame (ids ascending). The partition walks the dirty list newest
    /// first, so `c` is placed and gathers alone (its cell holds one object); `b` is placed after `c` and gathers
    /// `c`, giving `(b,c)`; `a` is placed last and gathers `b` and `c` from the head of its cell, giving `(a,b)` and
    /// `(a,c)`. The contact list in insertion order is `[(b,c), (a,b), (a,c)]`, and `processContactList` walks it
    /// from the head (the last insertion): `(a,c)`, `(a,b)`, `(b,c)`. Each pair calls `onCollide` on the gathered
    /// object first. Fails with the old id-sorted pair list (`(a,b)`, `(a,c)`, `(b,c)`).
    #[test]
    fn the_contact_list_is_walked_in_the_engines_order() {
        let t = pebble();
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        let at = Coord3D::new(100.0, 100.0, 0.0);
        let a = g.create_object(&t, at, 0.0, 0);
        let b = g.create_object(&t, at, 0.0, 0);
        let c = g.create_object(&t, at, 0.0, 0);
        clear_calls();
        g.update_partition();
        assert_eq!(calls(), vec![(a, c), (c, a), (a, b), (b, a), (b, c), (c, b)]);
    }

    /// `addCoverage` prepends an object to each cell's COI list (`friend_addToCellList`, `PartitionManager.cpp:1148`),
    /// so the latest placement is the head, which `addPossibleCollisions` (`:2005`) reads first. Placement order here
    /// is `c`, `b`, `a`, so the head-first list is `[a, b, c]`.
    #[test]
    fn a_cells_coi_list_has_the_latest_placement_at_its_head() {
        let t = pebble();
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        let at = Coord3D::new(100.0, 100.0, 0.0);
        let a = g.create_object(&t, at, 0.0, 0);
        let b = g.create_object(&t, at, 0.0, 0);
        let c = g.create_object(&t, at, 0.0, 0);
        g.update_partition();
        let cell = g.partition().world_to_cell(100.0, 100.0);
        let head_first: Vec<ObjectId> = g.partition().coi_list(cell).iter().rev().copied().collect();
        assert_eq!(head_first, vec![a, b, c]);
    }

    /// `processContactList` re-tests each pair at dispatch (`friend_collidesWith`, `:2567-2570`). A pair that an
    /// earlier `onCollide` has pulled apart is no contact: here `b` moves away after the gather, before dispatch.
    /// Fails if the geometry is taken from the gather (no live re-test).
    #[test]
    fn a_pair_separated_before_its_dispatch_is_no_contact() {
        let t = pebble();
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        let a = g.create_object(&t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let b = g.create_object(&t, Coord3D::new(103.0, 100.0, 0.0), 0.0, 0);
        let contacts = g.gather_contacts();
        assert_eq!(contacts.len(), 1, "the two pebbles overlap at the gather");
        g.objects.get_mut(&b).unwrap().set_position(Coord3D::new(300.0, 300.0, 0.0));
        clear_calls();
        g.dispatch_contacts(&contacts);
        assert!(calls().is_empty(), "the pair no longer overlaps when it is dispatched ({a:?}, {b:?})");
    }

    /// `OBJECT_STATUS_NO_COLLISIONS` on either object keeps the pair out of the dispatch (`PartitionManager.cpp:2573-2575`).
    /// Fails with the status check removed.
    #[test]
    fn a_no_collisions_object_is_in_no_contact() {
        let t = pebble();
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        let a = g.create_object(&t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let _b = g.create_object(&t, Coord3D::new(103.0, 100.0, 0.0), 0.0, 0);
        let contacts = g.gather_contacts();
        assert_eq!(contacts.len(), 1);
        g.objects.get_mut(&a).unwrap().status_bits |= status::NO_COLLISIONS;
        clear_calls();
        g.dispatch_contacts(&contacts);
        assert!(calls().is_empty(), "a NO_COLLISIONS object is in no contact");
    }

    /// The second `onCollide` of a contact gets the normal flipped (`PartitionManager.cpp:2587`, `flipCoord3D`).
    /// Fails with the second call passed the first call's normal.
    #[test]
    fn the_second_on_collide_gets_the_flipped_normal() {
        let t = pebble();
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        let a = g.create_object(&t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let b = g.create_object(&t, Coord3D::new(103.0, 100.0, 0.0), 0.0, 0);
        clear_calls();
        g.update_partition();
        let log = ON_COLLIDE_LOG.with(|l| l.borrow().clone());
        assert_eq!(log.iter().map(|&(x, y, _)| (x, y)).collect::<Vec<_>>(), vec![(a, b), (b, a)]);
        let (first, second) = (log[0].2, log[1].2);
        assert!(first.x > 0.0, "the first call's normal points from a to b");
        assert_eq!(second, Coord3D::new(-first.x, -first.y, -first.z), "the second call's normal is flipped");
    }

    /// Two dozers in the AN1 s3 f1840 poses, both moving, as `partition_l175` sets them up. `physics_a` and
    /// `physics_b` give each one its `PhysicsBehavior` or not.
    fn two_moving_dozers(g: &mut GameLogic, physics_a: bool, physics_b: bool) -> (ObjectId, ObjectId) {
        let (sx, sy) = (-800.0, -320.0);
        let a = g.create_object(&boxed(physics_a), Coord3D::new(948.1415 + sx, 385.9566 + sy, 0.0), 1.7604, 0);
        let b = g.create_object(&boxed(physics_b), Coord3D::new(939.7138 + sx, 400.8687 + sy, 0.0), -0.9296, 1);
        for id in [a, b] {
            let o = g.objects.get_mut(&id).unwrap();
            o.ai.move_goal = Some(Coord3D::new(1500.0, 1500.0, 0.0));
            o.ai.path = vec![Coord3D::new(1500.0, 1500.0, 0.0)];
            o.ai.machine.current = crate::ai_states::AiStateType::MoveTo;
            o.ai.friend_starting_move();
        }
        (a, b)
    }

    /// `PhysicsBehavior::onCollide` runs `processCollision` before its push (`PhysicsUpdate.cpp:1278`), so the moving
    /// branch's blocked flag is set by the contact's own `onCollide`, not by a scan after the dispatch. Fails with the
    /// call removed from `physics_on_collide`.
    #[test]
    fn a_moving_units_contact_sets_its_blocked_flag_in_its_on_collide() {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        let (a, b) = two_moving_dozers(&mut g, true, true);
        assert!(!g.objects[&a].ai.is_blocked, "nothing has run yet");
        g.on_collide(a, b, Coord3D::new(0.0, 0.0, 0.0), Coord3D::new(0.0, 0.0, 0.0));
        assert!(g.objects[&a].ai.is_blocked, "the moving unit is blocked by its contact (AIUpdate.cpp:1457)");
    }

    /// A unit with no `PhysicsBehavior` never reaches `processCollision` (`PhysicsUpdate.cpp:1278` is the only caller):
    /// `Object::onCollide` runs its collide modules, and none of them asks the AI. Here `a` has no physics module and
    /// `b` has one, in a contact (the poses share COI cells). `a`'s own contact sets no blocked flag. Fails with the
    /// physics gate in `on_collide` removed.
    #[test]
    fn a_unit_without_a_physics_module_sets_no_blocked_flag() {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        let (a, _b) = two_moving_dozers(&mut g, false, true);
        g.update_partition();
        assert!(!g.objects[&a].ai.is_blocked, "no physics module on a, so no processCollision for a");
    }
}
