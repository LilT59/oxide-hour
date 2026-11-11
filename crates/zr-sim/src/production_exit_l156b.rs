//! Lane 156b: the exit path of a production exit (`aiFollowExitProductionPath`). A queue exit snaps its natural rally
//! point to the new unit's grid cell before it is pushed (QueueProductionExitUpdate.cpp:132-150); the default exit
//! does not (DefaultProductionExitUpdate.cpp:97-99). The unit's last path point therefore differs from the unsnapped
//! natural point, which left Red Guard 246 (AN5 s1) one exit leg short (its Idle entry came 5.9 units early).

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::production::ProductionExit;
    use crate::template::{kind_of, LocomotorKind, LocomotorParams};
    use crate::{Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

    fn soldier() -> Arc<ObjectTemplate> {
        let mut t = ObjectTemplate::new("Redguard");
        t.kind_of = kind_of::INFANTRY | kind_of::SELECTABLE;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Legs, 20.0, 60.0, 120.0, 180.0));
        t.ai_update = Some(1);
        Arc::new(t)
    }

    /// Whether `x` sits on a cell centre (`(cell + 0.5) * 10`, or the footprint's `(cell + 0.05) * 10`).
    fn on_grid(v: f32) -> bool {
        let f = v.rem_euclid(10.0);
        (f - 5.0).abs() < 1e-3 || (f - 0.5).abs() < 1e-3
    }

    fn queue_exit() -> ProductionExit {
        ProductionExit::Queue {
            unit_create_point: (0.0, 0.0, 0.0),
            natural_rally_point: (20.0, 0.0, 0.0),
            exit_delay_frames: 9,
            initial_burst: 0,
            allow_airborne_creation: false,
        }
    }

    /// `QueueProductionExitUpdate.cpp:134` snaps the natural point, and with no rally point it is pushed twice
    /// (:149-150). Revert check: without the snap the points are off the grid and this fails.
    #[test]
    fn a_queue_exit_path_ends_on_the_snapped_natural_point() {
        let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
        g.add_player(0, 0);
        let id = g.create_object(&soldier(), Coord3D::new(200.3, 200.7, 0.0), 0.0, 0);
        let origin = Coord3D::new(200.3, 200.7, 0.0);
        let path = g.exit_path(id, &queue_exit(), &origin, 0.0, None);
        assert_eq!(path.len(), 2, "the natural point twice, no rally point");
        assert_eq!(path[0], path[1], "the same snapped point");
        assert!(on_grid(path[0].x) && on_grid(path[0].y), "snapped to a cell: {:?}", path[0]);
    }

    /// The default exit pushes its natural point unsnapped (DefaultProductionExitUpdate.cpp:97-99). Revert check:
    /// snapping every exit would move this point onto the grid and fail.
    #[test]
    fn a_default_exit_path_keeps_its_natural_point() {
        let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
        g.add_player(0, 0);
        let id = g.create_object(&soldier(), Coord3D::new(200.3, 200.7, 0.0), 0.0, 0);
        let origin = Coord3D::new(200.3, 200.7, 0.0);
        let exit = ProductionExit::Default { unit_create_point: (0.0, 0.0, 0.0), natural_rally_point: (20.0, 0.0, 0.0) };
        let path = g.exit_path(id, &exit, &origin, 0.0, None);
        assert_eq!(path.len(), 1, "the natural point alone");
        assert!(!on_grid(path[0].x) || !on_grid(path[0].y), "not snapped: {:?}", path[0]);
    }
}
