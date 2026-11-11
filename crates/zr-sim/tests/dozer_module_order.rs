//! The dozer/worker module's own scheduling (`DozerAIUpdate::update`, `DozerAIUpdate.cpp:1596-1668`;
//! `WorkerAIUpdate::update`, `WorkerAIUpdate.cpp:236-311`): the engine's dozer update *is* the
//! `AIUpdateInterface` module, so the AI state machine, `doLocomotor` and the dozer machine run in one
//! scheduled call, at one wake frame (`AIUpdate.cpp:299-301`, `UpdateModule.cpp:80-84`), and
//! `DozerActionPickActionPosState::update` runs on the module's *next* update after `newTask`
//! (`DozerAIUpdate.cpp:2044` -> `DozerActionState::onEnter`, `:1230-1240`). AN3 seed 1, C++: the US
//! dozer gets its task in the f=1 update and is still `AI_IDLE` in the f=2 snapshot; the first move
//! order (`AI_MOVE_TO`) appears in the f=3 update. Synthetic data only.

use std::sync::Arc;

use zr_sim::ai_states::AiStateType;
use zr_sim::command::Command;
use zr_sim::dozer::{DozerActionSub, DozerTemplate};
use zr_sim::supply::SupplyTruckTemplate;
use zr_sim::template::{kind_of, GeometryShape, LocomotorKind, LocomotorParams};
use zr_sim::{Coord3D, GameLogic, ObjectId, ObjectTemplate, TemplateSource, TerrainLogic};

struct FixedStore(Vec<Arc<ObjectTemplate>>);

impl TemplateSource for FixedStore {
    fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
        self.0.iter().find(|t| t.name == name).cloned()
    }
}

fn dozer_like(name: &str, worker: bool) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.geometry.major_radius = 5.0;
    t.geometry.minor_radius = 5.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 60.0, 100.0, 100.0, 360.0));
    t.ai_update = Some(1);
    t.kind_of |= kind_of::DOZER;
    t.dozer = Some(DozerTemplate { repair_health_percent_per_second: 0.0, bored_time_frames: 1.0, bored_range: 0.0 });
    if worker {
        // `WorkerAIUpdate` derives from both `SupplyTruckAIUpdate` and `DozerAIUpdate`; the port
        // recognises a worker by the two module data blocks (no separate flag).
        t.supply_truck = Some(SupplyTruckTemplate {
            max_boxes: 1,
            center_action_delay: 0,
            warehouse_action_delay: 0,
            scan_distance: 100.0,
        });
    }
    t
}

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
    let store =
        FixedStore(vec![Arc::new(dozer_like("Dozer", false)), Arc::new(dozer_like("Worker", true)), Arc::new(barracks())]);
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 1000);
    g.set_template_source(Arc::new(store));
    g
}

fn spawn(g: &mut GameLogic, name: &str, at: Coord3D) -> ObjectId {
    let t = g.find_template(name).expect("template");
    g.create_object(&t, at, 0.0, 0)
}

fn action(g: &GameLogic, id: ObjectId) -> DozerActionSub {
    g.find_object(id).expect("object").ai.dozer_action.sub
}

fn state(g: &GameLogic, id: ObjectId) -> AiStateType {
    g.find_object(id).expect("object").ai.machine.current
}

/// `DozerAIUpdate::newTask` (`DozerAIUpdate.cpp:2044`) ends with
/// `m_dozerMachine->resetToDefaultState()`, which enters `DOZER_PRIMARY_BUILD` at once; the action
/// machine only gets reset to `PickActionPos` there (`DozerActionState::onEnter`, `:1230-1240`). Its
/// `update` — `aiMoveToPosition` (`:229-230`) — runs on the module's next call, so the AI stays
/// `AI_IDLE` in the frame the task is given (AN3 s1: task at f=1, C++ `ai=Idle` at f=2).
#[test]
fn the_first_build_pick_runs_on_the_modules_next_update() {
    let mut g = world();
    let site = Coord3D::new(200.0, 100.0, 0.0);
    let d = spawn(&mut g, "Dozer", Coord3D::new(100.0, 100.0, 0.0));
    // Let the module's creation wake frame pass, so the next update is an ordinary one.
    g.run_frames(2);
    let _b = g.construct(d, "Barracks", site, 0.0).expect("construct");
    assert_eq!(action(&g, d), DozerActionSub::PickActionPos, "BUILD is entered, the pick is not run yet");
    assert_eq!(state(&g, d), AiStateType::Idle, "no move order in the task's frame");

    // The module's next update: `AIUpdateInterface::update` and the dozer machine in one call.
    g.run_frames(1);
    assert_eq!(action(&g, d), DozerActionSub::MoveToActionPos, "the pick ordered the start move");
    assert_eq!(state(&g, d), AiStateType::MoveTo);
}

/// The dozer machine is part of the AI module, so it is gated by the module's wake frame: a dozer
/// created at frame 0 gets its first module update at frame 1 (`AIUpdate.cpp:299-301`,
/// `UpdateModule.cpp:80-84`). A task given before that update is not picked up by the frame-0 update.
#[test]
fn the_dozer_machine_waits_for_the_ai_modules_wake_frame() {
    let mut g = world();
    let site = Coord3D::new(200.0, 100.0, 0.0);
    let d = spawn(&mut g, "Dozer", Coord3D::new(100.0, 100.0, 0.0));
    let _b = g.construct(d, "Barracks", site, 0.0).expect("construct");
    assert_eq!(action(&g, d), DozerActionSub::PickActionPos);

    // The update at frame 0: the module is not due (wake frame 1), so neither half runs.
    g.run_frames(1);
    assert_eq!(action(&g, d), DozerActionSub::PickActionPos, "the module has not run yet");
    assert_eq!(state(&g, d), AiStateType::Idle);

    // The update at frame 1: both halves run together and the move is ordered.
    g.run_frames(1);
    assert_eq!(action(&g, d), DozerActionSub::MoveToActionPos);
    assert_eq!(state(&g, d), AiStateType::MoveTo);
}

/// A pending BUILD task only enters `DOZER_PRIMARY_BUILD` when `AIUpdateInterface::isIdle`
/// (`isBuildMostImportant`, `DozerAIUpdate.cpp:1346-1366`): a dozer ordered to build while it is still
/// walking keeps its move, and the build action starts from the idle state (the primary machine's
/// conditions) rather than interrupting it.
#[test]
fn a_pending_build_waits_for_an_idle_ai() {
    let mut g = world();
    let site = Coord3D::new(200.0, 100.0, 0.0);
    let d = spawn(&mut g, "Dozer", Coord3D::new(100.0, 100.0, 0.0));
    g.run_frames(2);
    g.submit(0, Command::MoveTo { ids: vec![d], pos: Coord3D::new(320.0, 320.0, 0.0) });
    g.run_frames(3);
    assert_eq!(state(&g, d), AiStateType::MoveTo, "the move order is running");

    let _b = g.construct(d, "Barracks", site, 0.0).expect("construct");
    assert_eq!(action(&g, d), DozerActionSub::Inactive, "the primary machine is still IDLE");
    assert_eq!(state(&g, d), AiStateType::MoveTo, "the build does not interrupt the move");

    // Once the dozer idles, the next module update enters BUILD (the pick is its following update).
    let mut entered = None;
    for _ in 0..2000 {
        g.run_frames(1);
        if state(&g, d) == AiStateType::Idle {
            entered = Some(action(&g, d));
            break;
        }
    }
    assert_eq!(entered, Some(DozerActionSub::PickActionPos), "the idle conditions enter BUILD");
    g.run_frames(1);
    assert_eq!(action(&g, d), DozerActionSub::MoveToActionPos, "then the pick orders the approach move");
}

/// The same module, for the GLA worker (`WorkerAIUpdate` is the dozer module plus the supply truck):
/// its dozer half has the same wake frame and the same next-update pick.
#[test]
fn the_worker_module_has_the_same_wake_frame_and_pick_timing() {
    let mut g = world();
    let site = Coord3D::new(200.0, 100.0, 0.0);
    let w = spawn(&mut g, "Worker", Coord3D::new(100.0, 100.0, 0.0));
    let _b = g.construct(w, "Barracks", site, 0.0).expect("construct");
    assert_eq!(action(&g, w), DozerActionSub::PickActionPos);
    g.run_frames(1);
    assert_eq!(action(&g, w), DozerActionSub::PickActionPos, "the worker module has not run yet");
    g.run_frames(1);
    assert_eq!(action(&g, w), DozerActionSub::MoveToActionPos);
    assert_eq!(state(&g, w), AiStateType::MoveTo);
}
