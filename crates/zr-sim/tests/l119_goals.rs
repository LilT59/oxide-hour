//! R10 finding 7: `AIGroup::groupMoveToPosition`'s `removeGoal` runs inside the member loop, after its
//! `DISABLED_HELD`, `KINDOF_IMMOBILE`, no-AI and column filters (`AIGroup.cpp:1685-1716`): a selection member
//! the order skips keeps its goal cell. Synthetic data only.

use std::sync::Arc;

use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::{Command, Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

fn flat() -> TerrainLogic {
    TerrainLogic::flat(400, 400, 0)
}

fn unit(name: &str) -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 8.0;
    t.max_health = 200.0;
    t.initial_health = 200.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    Arc::new(t)
}

/// A group order only removes the goals of the members it will actually move. A unit of another player in the
/// same selection is dropped before the loop (`GameLogicDispatch.cpp:386-400`,
/// `AIGroup::removeAnyObjectsNotOwnedByPlayer`, `AIGroup.cpp:295-318`), so it keeps its goal cell.
/// Revert check: `removeGoal` for every id in the selection (the pre-R10 loop).
#[test]
fn a_group_move_only_clears_the_goals_of_its_own_members() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    let t = unit("Tank");
    let mover = g.create_object(&t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let foreign = g.create_object(&t, Coord3D::new(260.0, 200.0, 0.0), 0.0, 1);

    // A finished move of its own leaves the foreign unit in its goal cell: the pop to goal keys it
    // (`AIUpdate.cpp:1044-1058`).
    g.submit(1, Command::MoveTo { ids: vec![foreign], pos: Coord3D::new(300.0, 260.0, 0.0) });
    g.run_frames(200);
    assert!(!g.find_object(foreign).unwrap().ai.is_moving(), "the foreign unit has stopped");
    let before = g.ai_pathfind_goal(foreign).expect("a settled unit holds its goal cell");

    // A player-0 selection that includes the player-1 unit.
    g.submit(0, Command::MoveTo { ids: vec![mover, foreign], pos: Coord3D::new(100.0, 300.0, 0.0) });
    g.run_frames(2);
    assert_eq!(g.ai_pathfind_goal(foreign), Some(before), "a filtered member keeps its goal cell");
    assert!(g.ai_pathfind_goal(mover).is_some(), "the moved member takes its own goal cell");
    assert_eq!(g.find_object(foreign).unwrap().player, 1, "the foreign unit is untouched");
}
