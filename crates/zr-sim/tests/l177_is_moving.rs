//! Lane 177 (wave 21): `AIUpdateInterface::isMoving` (AIUpdate.cpp:3194-3206) and `m_isMoving`, set by
//! `friend_startingMove` and cleared by `friend_endingMove` (AIUpdate.cpp:2034-2050). Synthetic data only.

use std::sync::Arc;
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::{Command, Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

fn flat() -> TerrainLogic {
    TerrainLogic::flat(120, 120, 0)
}

fn tank() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("T");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.kind_of |= kind_of::DOZER;
    t.geometry.major_radius = 8.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    t
}

/// `AIInternalMoveToState::onEnter` calls `friend_startingMove` (AIStates.cpp:1635), so the unit reads as moving and its
/// `m_isMoving` is set while the move runs.
/// Revert check: without the `friend_starting_move` in `ai_internal_move_enter_with` the field stays FALSE here.
#[test]
fn a_move_sets_is_moving_for_as_long_as_its_state_runs() {
    let mut g = GameLogic::new(flat(), 1);
    let t = Arc::new(tank());
    let id = g.create_object(&t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    g.submit(0, Command::MoveTo { ids: vec![id], pos: Coord3D::new(600.0, 100.0, 0.0) });
    g.update();
    g.update();
    let o = g.find_object(id).unwrap();
    assert!(o.ai.is_moving, "the move state set m_isMoving (friend_startingMove)");
    assert!(o.ai.is_moving(), "a moving unit reads as moving");
}

/// `isMoving` is FALSE for an idle unit (AIUpdate.cpp:3196-3198), and the move's exit calls `friend_endingMove`
/// (AIStates.cpp:1747), which clears `m_isMoving` (AIUpdate.cpp:2049). A stopped unit is therefore not moving, even
/// though the path it held is not dropped in the same frame.
/// Revert check: the old reader (`has_path() || waiting_for_path`) reads the held path as moving, and without the
/// exit's `friend_ending_move` the field stays TRUE after the stop.
#[test]
fn a_stopped_unit_is_not_moving_and_its_field_is_cleared() {
    let mut g = GameLogic::new(flat(), 1);
    let t = Arc::new(tank());
    let id = g.create_object(&t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    g.submit(0, Command::MoveTo { ids: vec![id], pos: Coord3D::new(600.0, 100.0, 0.0) });
    for _ in 0..3 {
        g.update();
    }
    assert!(g.find_object(id).unwrap().ai.is_moving(), "the unit is under way before the stop");
    g.submit(0, Command::Stop { ids: vec![id] });
    g.update();
    let o = g.find_object(id).unwrap();
    assert!(!o.ai.is_moving(), "an idle unit is not moving");
    assert!(!o.ai.is_moving, "the move's onExit cleared m_isMoving (friend_endingMove)");
}

/// An idle ally that steps aside for a moving tank holds a move-away path, but its main machine is `AI_IDLE` and the
/// step is a temporary state. `isIdle` reads the main machine only (AIUpdate.cpp:3120-3127), so it is not moving
/// (AIUpdate.cpp:3196-3198), and the pushed unit is not blocked by the mover (`processCollision`, :1444-1448).
/// Revert check: the old `has_path() || waiting_for_path` reader reads the step-aside path as moving.
#[test]
fn an_idle_ally_stepping_aside_is_not_moving() {
    let mut g = GameLogic::new(flat(), 1);
    let t = Arc::new(tank());
    let mover = g.create_object(&t, Coord3D::new(100.0, 60.0, 0.0), 0.0, 0);
    let idle = g.create_object(&t, Coord3D::new(130.0, 60.0, 0.0), 0.0, 0);
    g.submit(0, Command::MoveTo { ids: vec![mover], pos: Coord3D::new(260.0, 60.0, 0.0) });
    let mut stepped = 0;
    for _ in 0..240 {
        g.update();
        let o = g.find_object(idle).unwrap();
        if o.ai.machine.current == zr_sim::ai_states::AiStateType::Idle && o.ai.has_path() {
            stepped += 1;
            assert!(!o.ai.is_moving(), "an idle unit stepping aside is not isMoving (AIUpdate.cpp:3196-3198)");
        }
    }
    assert!(stepped > 0, "the idle unit held a step-aside path while idle, so the test saw the case it names");
}

/// The arrival path: when the move completes, the exit clears `m_isMoving` and the unit is not moving any more.
#[test]
fn a_finished_move_clears_is_moving() {
    let mut g = GameLogic::new(flat(), 1);
    let t = Arc::new(tank());
    let id = g.create_object(&t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    g.submit(0, Command::MoveTo { ids: vec![id], pos: Coord3D::new(160.0, 100.0, 0.0) });
    g.update();
    let mut frames = 1;
    while g.find_object(id).unwrap().ai.is_moving() && frames < 30 * 60 {
        g.update();
        frames += 1;
    }
    assert!(frames > 0 && frames < 30 * 60, "the move finishes in a minute or less ({frames} frames)");
    let o = g.find_object(id).unwrap();
    assert!(!o.ai.is_moving, "the move's exit cleared m_isMoving");
    assert!(!o.ai.is_moving());
}
