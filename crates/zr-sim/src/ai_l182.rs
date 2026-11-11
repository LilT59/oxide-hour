//! Lane 182 (wave 21). Crate-internal tests (the functions under test are private, like the lane 177b tests):
//! `AIUpdateInterface::computePath`'s straight-line shortcut (AIUpdate.cpp:1709-1712): `if (!m_isFinalGoal &&
//! isLinePassable(...)) return computeQuickPath(destination);`. A goal the unit is not asked to adjust to, on a passable
//! line from the unit, is a two-node quick path with no search, and the locomotor goal stays `NONE` (the goal is armed
//! only by a computed path, AIUpdate.cpp:1744-1745). AH7 s1 f3085: a Ranger's MoveTo whose goal object was its ignored
//! obstacle took the straight line in C++ and the port searched, so the port moved 31 frames early (f3092 vs f3123).

use super::*;
use crate::math::{Coord3D, LOGICFRAMES_PER_SECOND};
use crate::template::{kind_of, LocomotorKind, LocomotorParams, ObjectTemplate};
use crate::terrain::TerrainLogic;
use crate::GameLogic;
use std::sync::Arc;

fn flat() -> TerrainLogic {
    TerrainLogic::flat(120, 120, 0)
}

fn tank() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("T");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 8.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    t
}

/// A non-final goal on a clear line: `computePath` returns `computeQuickPath` (AIUpdate.cpp:1709-1712). The unit holds
/// a two-node path from its own position to the goal, its path timestamp is this frame, and it has no locomotor goal.
/// Revert check: without the shortcut the search builds a multi-node path and arms the goal (`POSITION_ON_PATH`).
#[test]
fn a_non_final_goal_on_a_passable_line_is_a_quick_path() {
    let mut g = GameLogic::new(flat(), 1);
    let t = Arc::new(tank());
    let id = g.create_object(&t, Coord3D::new(100.0, 60.0, 0.0), 0.0, 0);
    g.ai_request_path(id, Coord3D::new(260.0, 60.0, 0.0), false);
    g.process_pathfind_queue();
    let o = g.find_object(id).unwrap();
    assert_eq!(o.ai.raw_path.as_ref().map(|p| p.nodes.len()), Some(2), "a quick path is two nodes: the start and the goal");
    assert_eq!(o.ai.locomotor_goal_type, LocoGoalType::None, "computePath returns before the goal is armed");
    assert!(!o.ai.waiting_for_path, "the quick path is not waiting for the pathfinder");
}

/// AH7 s1 f3085-f3086. The quick path at f3085 stamps the path timestamp; the adjusted repath at f3086 is one frame later,
/// so `requestPath` sets the one-second anti-spin wait (AIUpdate.cpp:499-503) and returns without arming the goal. The
/// unit stays on its quick path with the goal `NONE`, and the deferred request is due at f3116.
/// Revert check: without the shortcut the f3085 request arms the goal (`POSITION_ON_PATH`) and the unit moves through the wait.
#[test]
fn a_quick_path_then_an_adjusted_repath_waits_a_second_with_no_goal() {
    let mut g = GameLogic::new(flat(), 1);
    let t = Arc::new(tank());
    let id = g.create_object(&t, Coord3D::new(100.0, 60.0, 0.0), 0.0, 0);
    g.frame = 3085;
    g.ai_request_path(id, Coord3D::new(260.0, 60.0, 0.0), false);
    g.process_pathfind_queue();
    assert_eq!(g.find_object(id).unwrap().ai.path_timestamp, 3085, "the quick path is stamped this frame");
    g.frame = 3086;
    g.ai_request_path(id, Coord3D::new(262.0, 60.0, 0.0), true);
    let o = g.find_object(id).unwrap();
    assert_eq!(o.ai.queue_for_path_frame, 3086 + LOGICFRAMES_PER_SECOND, "the repath waits one second (AIUpdate.cpp:503)");
    assert_eq!(o.ai.locomotor_goal_type, LocoGoalType::None, "the wait leaves the goal NONE, so the unit does not turn or move");
    assert_eq!(o.ai.raw_path.as_ref().map(|p| p.nodes.len()), Some(2), "the quick path is still the unit's path");
}

/// The control: a final goal (the move adjusts its destination) on the same clear line is searched, so the locomotor goal
/// is armed on the computed path (`computePath`'s `if (theNewPath) setLocomotorGoalPositionOnPath()`, AIUpdate.cpp:1744-1745).
/// This passes with or without the shortcut; it shows the shortcut is gated by the final-goal flag.
#[test]
fn a_final_goal_on_the_same_line_is_searched() {
    let mut g = GameLogic::new(flat(), 1);
    let t = Arc::new(tank());
    let id = g.create_object(&t, Coord3D::new(100.0, 60.0, 0.0), 0.0, 0);
    g.ai_request_path(id, Coord3D::new(260.0, 60.0, 0.0), true);
    g.process_pathfind_queue();
    let o = g.find_object(id).unwrap();
    assert!(o.ai.is_final_goal, "the request is a final goal");
    assert_eq!(o.ai.locomotor_goal_type, LocoGoalType::PositionOnPath, "a searched path arms the goal");
}
