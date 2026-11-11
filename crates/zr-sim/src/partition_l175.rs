//! Lane 175 (wave 20): the partition's contact pairs are the engine's COI cells.
//!
//! `PartitionData::updateCellsTouched` (`PartitionManager.cpp:2086-2166`) places an object in the cells its
//! geometry fill covers: `doSmallFill` for a small geometry, `doCircleFill` (the retail midpoint algorithm —
//! `RETAIL_COMPATIBLE_CRC` is on in the comparison build) for a sphere/cylinder, and `doRectFill` for a box.
//! `addPossibleCollisions` (`:1980-2014`) turns a dirty object's cell list into contact-list pairs, so an
//! object pair only reaches `onCollide`/`AIUpdateInterface::processCollision` when their COI cell sets
//! intersect — however close the two boxes are geometrically. AN5 s3: the two China dozers at f1815 touch
//! (corner containment and a separating-axis test both say so) but their cells are `(23,9)` and
//! `(22,10)/(23,10)`, so the C++ never pairs them; the port's radius query did, blocked one of them, and the
//! dozer's build-site path diverged at f1848.
//!
//! The fill's step counts go through the engine's `REAL_TO_INT_CEIL` (`BaseFunctions.h:97`,
//! `fast_float2long_round(fast_float_ceil(x))`), which adds `0.99999994` and truncates: an exact integer `k`
//! becomes `k + 1` because `k + 1 - 2^-24` rounds up in f32. `doRectFill`'s `numStepsY` for the dozer's
//! 20-wide box is 2, not 1, and the second row is where the engine's extra cells come from.

#[cfg(test)]
mod tests {
    use crate::logic::GameLogic;
    use crate::partition::PartitionManager;
    use crate::physics::PhysicsTemplate;
    use crate::template::{kind_of, GeometryShape, LocomotorKind, LocomotorParams, ObjectTemplate};
    use crate::terrain::TerrainLogic;
    use crate::Coord3D;
    use std::sync::Arc;

    /// The China dozer's geometry: `Geometry = BOX`, `MajorRadius 17`, `MinorRadius 10`.
    fn boxed(name: &str) -> Arc<ObjectTemplate> {
        let mut t = ObjectTemplate::new(name);
        t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
        t.geometry.shape = GeometryShape::Box;
        t.geometry.major_radius = 17.0;
        t.geometry.minor_radius = 10.0;
        t.geometry.height = 15.0;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.physics = Some(PhysicsTemplate { mass: 10.0, ..PhysicsTemplate::default() });
        t.locomotor = Some(LocomotorParams::from_engine_units(LocomotorKind::Treads, 1.0, 0.0, 0.1, 0.1, 0.1, true));
        Arc::new(t)
    }

    /// `doRectFill`'s cells for the two poses the C++ `ZR_L175_ADD` gate printed for AN1 s3 seed 3 (dozer
    /// 230 and dozer 211 at f1840). Fails if the step count uses a plain `ceil` (one row instead of two).
    #[test]
    fn the_coi_fill_is_the_engines_rect_fill() {
        let pf = PartitionManager::new(4096.0, 4096.0, 40.0);
        let a = pf.coi_cells(Coord3D::new(948.1415, 385.9566, 18.75), 1.7604, GeometryShape::Box, 17.0, 10.0, false);
        assert_eq!(a, vec![(23, 9), (24, 9)], "China dozer 230 at AN1 s3 f1840 (C++ ZR_L175_ADD)");
        let b = pf.coi_cells(Coord3D::new(939.7138, 400.8687, 18.75), -0.9296, GeometryShape::Box, 17.0, 10.0, false);
        // In `m_coiArray` order, not sorted (lane 181): `doRectFill` samples the top-left row first, (23,10) at the
        // corner, then (23,9) on the second row (`PartitionManager.cpp:1821-1832`, `addSubPixToCoverage` `:1758-1777`).
        assert_eq!(b, vec![(23, 10), (23, 9)], "China dozer 211 at AN1 s3 f1840 (C++ ZR_L175_ADD cells, in fill order)");
        // The pair's cells intersect at (23,9): the engine pairs them (the C++ trace shows the collision).
        assert!(a.iter().any(|c| b.contains(c)));
    }

    /// `REAL_TO_INT_CEIL` for Zero Hour is `fast_float2long_round(fast_float_ceil(x))`, and `fast_float_ceil`
    /// rounds an exact integer up (f32 round-to-even on `k + 1 - 2^-24`).
    #[test]
    fn the_engines_ceil_rounds_an_exact_integer_up() {
        assert_eq!(crate::partition::engine_ceil_for_test(1.0), 2);
        assert_eq!(crate::partition::engine_ceil_for_test(10.0 * 0.1), 2, "10 * 0.1f is exactly 1.0f");
        assert_eq!(crate::partition::engine_ceil_for_test(1.7), 2);
        assert_eq!(crate::partition::engine_ceil_for_test(0.5), 1);
        assert_eq!(crate::partition::engine_ceil_for_test(0.0), 0);
    }

    /// AN5 s3 f1815: dozers 230 and 211 geometrically touch, but their COI cells do not intersect, so the
    /// contact list never pairs them and neither is blocked. At the AN1 s3 f1840 poses the cells do intersect
    /// and 230 is blocked by 211. Fails if the candidates come from a range query instead of the COI cells.
    #[test]
    fn a_pair_outside_the_coi_cells_is_not_a_contact() {
        let dozer = boxed("Dozer");
        // A small flat map, with the two world poses translated by a whole number of 40-unit cells so the COI
        // cell relationships the C++ log reports are unchanged.
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        let (sx, sy) = (-800.0, -320.0);
        // AN5 s3 f1815's two dozers, with the first facing 1.4 rad: the boxes touch (separating axis and the
        // engine's corner test both say so) and `blockedBy` would hold (relative angle 0.57 rad < 45°), but the
        // COI cells are (23,9)/(24,9) against (22,10)/(23,10), so the engine never lists the pair.
        let a = g.create_object(&dozer, Coord3D::new(948.1259 + sx, 385.9587 + sy, 0.0), 1.4, 0);
        let b = g.create_object(&dozer, Coord3D::new(936.609 + sx, 413.004 + sy, 0.0), -0.5717, 1);
        for id in [a, b] {
            let o = g.objects.get_mut(&id).unwrap();
            o.ai.move_goal = Some(Coord3D::new(1500.0, 1500.0, 0.0));
            o.ai.path = vec![Coord3D::new(1500.0, 1500.0, 0.0)];
            // Both are in their move state with a path, so both are `isMoving` (AIUpdate.cpp:3196-3202).
            o.ai.machine.current = crate::ai_states::AiStateType::MoveTo;
            o.ai.friend_starting_move();
        }
        g.update_partition();
        assert!(!g.objects[&a].ai.is_blocked, "outside the COI cells the engine has no contact pair");
        assert!(!g.objects[&a].ai.is_blocked_and_stuck);

        // AN1 s3 f1840: the cells intersect at (23,9), so 230 is blocked by 211 with a zero blocked speed.
        g.objects.get_mut(&a).unwrap().set_position(Coord3D::new(948.1415 + sx, 385.9566 + sy, 0.0));
        g.objects.get_mut(&a).unwrap().set_orientation(1.7604);
        g.objects.get_mut(&b).unwrap().set_position(Coord3D::new(939.7138 + sx, 400.8687 + sy, 0.0));
        g.objects.get_mut(&b).unwrap().set_orientation(-0.9296);
        g.update_partition();
        assert!(g.objects[&a].ai.is_blocked, "inside the COI cells the pair is a contact");
        assert_eq!(g.objects[&a].ai.cur_max_blocked_speed, 0.0, "they are running into us (AIUpdate.cpp:1257-1259)");
    }
}
