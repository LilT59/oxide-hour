//! The dozer's build action (`DozerActionStateMachine`, `DozerAIUpdate.cpp:788-830`) and the
//! temporary `AI_MOVE_TO` a dock order becomes while the start move is still active
//! (`AIUpdate.cpp:2909`). Synthetic data, no game install.

use std::sync::Arc;

use zr_sim::ai_states::AiStateType;
use zr_sim::dozer::{DozerAction, DozerActionSub, DozerTemplate};
use zr_sim::template::{kind_of, GeometryShape, LocomotorKind, LocomotorParams};
use zr_sim::{Coord3D, GameLogic, ObjectId, ObjectTemplate, TemplateSource, TerrainLogic};

struct FixedStore(Vec<Arc<ObjectTemplate>>);

impl TemplateSource for FixedStore {
    fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
        self.0.iter().find(|t| t.name == name).cloned()
    }
}

fn dozer_template() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Dozer");
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.geometry.major_radius = 5.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 60.0, 100.0, 100.0, 360.0));
    t.kind_of |= kind_of::DOZER;
    t.dozer = Some(DozerTemplate { repair_health_percent_per_second: 0.0, bored_time_frames: 1.0, bored_range: 0.0 });
    t
}

/// A 30-unit box with a short build, so construction is visible within a few seconds.
fn barracks() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Barracks");
    t.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;
    t.max_health = 500.0;
    t.initial_health = 500.0;
    t.geometry.shape = GeometryShape::Box;
    t.geometry.major_radius = 15.0;
    t.geometry.minor_radius = 15.0;
    t.build_cost = 300;
    t.build_time_frames = 60;
    t
}

fn world() -> GameLogic {
    let store = FixedStore(vec![Arc::new(dozer_template()), Arc::new(barracks())]);
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 1000);
    g.set_template_source(Arc::new(store));
    g
}

fn spawn(g: &mut GameLogic, name: &str, at: Coord3D) -> ObjectId {
    let t = g.find_template(name).expect("template");
    g.create_object(&t, at, 0.0, 0)
}

/// The dozer's action as set by `DozerAIUpdate::newTask` (the start point, the build dock and the end
/// point). The start point is `findGoodBuildOrRepairPosition`'s ring-search spot, so the tests read it
/// back rather than assuming a geometry.
fn action_of(g: &GameLogic, d: ObjectId) -> DozerAction {
    g.find_object(d).expect("dozer").ai.dozer_action
}

/// `distCalcProc_BoundaryAndBoundary_2D` gap from the dozer's bounding circle to a point, as
/// `DozerActionMoveToActionPosState::update` measures it (`DozerAIUpdate.cpp:340`).
fn gap_to(g: &GameLogic, d: ObjectId, p: &Coord3D) -> f32 {
    let o = g.find_object(d).expect("dozer");
    (o.position.dist_2d(p) - o.template.geometry.bounding_circle_radius()).max(0.0)
}

/// The start move's arrival test is the gap under `max(70, radius + 15)` (`DozerAIUpdate.cpp:340`). The
/// dock move is ordered only when that test passes, and the start move is still running at that point.
/// The dock move becomes a 20-second temporary `AI_MOVE_TO` (`AIUpdate.cpp:2909`).
#[test]
fn dock_move_during_the_start_move_is_a_twenty_second_temporary_move() {
    let mut g = world();
    let site = Coord3D::new(200.0, 100.0, 0.0);
    let d = spawn(&mut g, "Dozer", Coord3D::new(100.0, 100.0, 0.0));
    let b = g.construct(d, "Barracks", site, 0.0).expect("construct");

    // `newTask` ends in `resetToDefaultState` (`DozerAIUpdate.cpp:2044`), which enters BUILD and only
    // resets the action machine (`DozerActionState::onEnter`, `:1230-1240`): the start move is ordered by
    // the module's next update, not by the order itself (C++ AN3 s1: task at f=1, move order at f=2).
    assert_eq!(action_of(&g, d).sub, DozerActionSub::PickActionPos, "BUILD entered, the pick is pending");
    assert_eq!(g.find_object(d).unwrap().ai.machine.current, AiStateType::Idle);
    // The dozer was created before the first update, so its module first runs at frame 1, the second update
    // (`setWakeFrame(NONE)` at creation, AIUpdate.cpp:299; `m_frame++` at the end of the update, GameLogic.cpp:3929).
    g.run_frames(2);
    let action = action_of(&g, d);
    assert_eq!(action.sub, DozerActionSub::MoveToActionPos, "the module's first update orders the start move");
    assert_eq!(g.find_object(d).unwrap().ai.machine.current, AiStateType::MoveTo);

    let mut temp_frame = None;
    for _ in 0..600 {
        g.run_frames(1);
        if g.find_object(d).unwrap().ai.machine.temporary.is_some() {
            temp_frame = Some(g.frame());
            break;
        }
    }
    let temp_frame = temp_frame.expect("the dock move becomes a temporary move");
    let gap = gap_to(&g, d, &action.dock);
    assert!(gap <= 75.0, "the dock move waited for the start arrival test, gap {gap}");

    let m = &g.find_object(d).unwrap().ai.machine;
    assert_eq!(m.temporary, Some(AiStateType::MoveTo));
    assert_eq!(m.current, AiStateType::MoveTo, "the start move keeps running underneath");
    // Set on the frame of the order (or the one before the loop stopped), 20 s = 600 frames.
    assert!(
        (temp_frame + 599..=temp_frame + 601).contains(&m.temporary_end),
        "20 seconds from the dock order, got end {} for order around {temp_frame}",
        m.temporary_end
    );
    assert_eq!(g.find_object(d).unwrap().ai.dozer_action.sub, DozerActionSub::MovingToBuildDock);
    assert!(g.find_object(b).unwrap().construction_percent == 0.0, "no construction while moving");
}

/// Construction advances only once the dozer is idle at its dock (`DOZER_DO_BUILD_AT_DOCK`), and
/// finishes with a move to the end dock point (`DozerAIUpdate.cpp:645-652`).
#[test]
fn the_build_starts_at_the_dock_and_the_dozer_leaves_for_its_end_point() {
    let mut g = world();
    let site = Coord3D::new(200.0, 100.0, 0.0);
    let d = spawn(&mut g, "Dozer", Coord3D::new(100.0, 100.0, 0.0));
    let b = g.construct(d, "Barracks", site, 0.0).expect("construct");
    let action = action_of(&g, d);
    let (dock, end) = (action.dock, action.end);
    // `DOZER_DOCK_POINT_END`: five cells (50 units) from the dock, away from the structure.
    assert!((end.dist_2d(&dock) - 50.0).abs() < 0.5, "the end point is five cells from the dock");
    assert!(end.dist_2d(&site) > dock.dist_2d(&site), "the end point is on the far side of the dock");

    let mut first_progress = None;
    for _ in 0..3000 {
        g.run_frames(1);
        let built = g.find_object(b).unwrap();
        if first_progress.is_none() && built.construction_percent > 0.0 {
            first_progress = Some(g.frame());
            // The build only starts with the dozer idle at the dock.
            let dozer = g.find_object(d).unwrap();
            assert_eq!(dozer.ai.machine.current, AiStateType::Idle, "construction waits for the dock move");
            assert_eq!(dozer.ai.dozer_action.sub, DozerActionSub::DoBuildAtDock);
            // The move ends at the path's last node, which `adjustDestination` moves off the dock when the
            // dock lies in a blocked cell (`AIPathfind.cpp:5462`), so the dozer stops within a cell or two.
            assert!(dozer.position.dist_2d(&dock) < 15.0, "the dozer is at the dock {:?}, got {:?}", dock, dozer.position);
        }
        if !built.under_construction {
            break;
        }
    }
    assert!(first_progress.is_some(), "the structure was built");
    assert!(!g.find_object(b).unwrap().under_construction, "construction completed");

    g.run_frames(400);
    let dozer = g.find_object(d).unwrap();
    assert!(dozer.position.dist_2d(&end) < 15.0, "the dozer moved to its end dock point, got {:?}", dozer.position);
}

/// A dozer far from its start point does not order the dock move until its start move's arrival test
/// passes (`DozerActionMoveToActionPosState::update`, `DozerAIUpdate.cpp:340`): no temporary move while
/// it is still outside the tolerance, and the first one comes within the tolerance.
#[test]
fn a_far_dozer_orders_the_dock_move_only_on_arrival() {
    let mut g = world();
    let site = Coord3D::new(200.0, 100.0, 0.0);
    let d = spawn(&mut g, "Dozer", Coord3D::new(320.0, 100.0, 0.0));
    g.construct(d, "Barracks", site, 0.0).expect("construct");
    // The module's first update runs the pick (the task alone does not; see the timing tests in
    // `dozer_module_order.rs`). That update is frame 1, the second one, for a dozer created before the first
    // update (AIUpdate.cpp:299, GameLogic.cpp:3929).
    g.run_frames(2);
    let start = action_of(&g, d).dock;
    assert!(gap_to(&g, d, &start) > 70.0, "the dozer starts outside the tolerance");

    let mut ordered = None;
    for _ in 0..1000 {
        let dozer = g.find_object(d).unwrap();
        if dozer.ai.machine.temporary.is_some() {
            ordered = Some(gap_to(&g, d, &start));
            break;
        }
        assert!(
            matches!(dozer.ai.dozer_action.sub, DozerActionSub::MoveToActionPos | DozerActionSub::SelectBuildDock),
            "still walking to the start point, got {:?}",
            dozer.ai.dozer_action.sub
        );
        g.run_frames(1);
    }
    let gap = ordered.expect("the dock move is ordered");
    // The tolerance is 70 from the dozer's bounding circle (radius 5), with a little travel per frame.
    assert!(gap <= 75.0, "the dock move waited for the start point, {gap} units out");
    assert!(gap > 60.0, "the dock move did not start before arriving near the start point, {gap} units out");
}

/// `GameLogic::placeObjectAtPosition` (`GameLogic.cpp:520-527`): a starting unit with an AI spot that is not a
/// valid cell moves to the nearest valid one (`Pathfinder::adjustDestination`); a valid spot is kept.
#[test]
fn a_starting_unit_inside_a_structure_moves_to_the_nearest_valid_cell() {
    let mut g = world();
    let site = Coord3D::new(200.0, 100.0, 0.0);
    let _b = spawn(&mut g, "Barracks", site);
    g.update_partition();
    // Dead centre of the 30-unit box: an obstacle cell.
    let inside = spawn(&mut g, "Dozer", site);
    g.adjust_placed_unit(inside);
    let moved = g.find_object(inside).unwrap().position;
    assert!(moved.dist_2d(&site) > 15.0, "the unit left the footprint, now at {moved:?}");
    // A radius-5 dozer is a centred footprint (`getRadiusAndCenter`, AIPathfind.cpp:9776-9796), so the retail
    // snap puts it on a cell centre (`adjustCoordToCell`, AIPathfind.cpp:9013-9015): x and y are 5 mod 10.
    // A valid spot is snapped too (`checkForAdjust` sets `*dest`, AIPathfind.cpp:5355), so the clear
    // spot's position is a cell centre as well, though it was not on one.
    let clear = Coord3D::new(60.0, 300.0, 0.0);
    let far = spawn(&mut g, "Dozer", clear);
    g.adjust_placed_unit(far);
    let p = g.find_object(far).unwrap().position;
    assert!((p.x - 5.0).rem_euclid(10.0) < 1e-3 && (p.y - 5.0).rem_euclid(10.0) < 1e-3, "on a cell centre: {p:?}");
    assert!(p.dist_2d(&clear) > 0.0, "the clear spot is snapped, not kept");
}
