//! Wave 19 wrap-up: the attack-move's retry test reads the state object's own `m_pathGoalPosition`
//! (`AIStates.cpp:3680`), which only that state's update writes (`:1804`). AN1 s1 Red Guard 241: its main attack-move
//! never updated under a 600-frame temporary move, so the position stayed zero, and the state slept 3 s and retried
//! (`:3686-3689`) instead of ending.

use super::*;
use crate::template::{kind_of, LocomotorKind, LocomotorParams, ObjectTemplate};
use crate::terrain::TerrainLogic;
use std::sync::Arc;

fn walker() -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new("Walker");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.geometry.major_radius = 5.0;
    t.physics = Some(crate::physics::PhysicsTemplate { mass: 10.0, ..crate::physics::PhysicsTemplate::default() });
    t.locomotor = Some(LocomotorParams::from_engine_units(LocomotorKind::Treads, 1.0, 0.0, 0.2, 0.0556, 0.1, true));
    Arc::new(t)
}

/// Revert check: read `o.ai.path_goal_position` in `ai_attack_move_update` again; the unit is then close to that shared
/// value, the state ends and the sleep is never set.
#[test]
fn an_attack_move_that_never_got_its_own_path_sleeps_and_retries() {
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 0);
    let id = g.create_object(&walker(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    // Off frame 0: there `m_frameToSleepUntil == now` would take the sleep-retry wake (`AIStates.cpp:3664-3668`),
    // and the retry path this test is about is the one below that.
    g.frame = 500;
    let now = g.frame;
    {
        let o = g.objects.get_mut(&id).unwrap();
        o.ai.machine.current = AiStateType::AttackMoveTo;
        o.ai.machine.attack_move_retry = 2;
        o.ai.machine.attack_move_path_goal = Coord3D::ZERO;
        // The ended temporary's value: the unit stands on it.
        o.ai.path_goal_position = o.position;
        o.ai.move_goal = None;
    }
    assert_eq!(g.ai_attack_move_update(id), StateReturn::Continue, "far from its own path goal, it retries");
    let o = g.objects.get(&id).unwrap();
    assert_eq!(o.ai.machine.attack_move_sleep_until, now + 3 * LOGICFRAMES_PER_SECOND);
    assert_eq!(o.ai.machine.attack_move_retry, 1);
}

/// `m_pathGoalPosition` near the unit: the retry test passes and the state's result stands.
#[test]
fn an_attack_move_close_to_its_own_path_goal_ends() {
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 0);
    let id = g.create_object(&walker(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    // Off frame 0 (see the test above): the sleep-retry wake would re-enter the move instead of ending.
    g.frame = 500;
    {
        let o = g.objects.get_mut(&id).unwrap();
        o.ai.machine.current = AiStateType::AttackMoveTo;
        o.ai.machine.attack_move_retry = 2;
        o.ai.machine.attack_move_path_goal = o.position;
        o.ai.move_goal = None;
    }
    assert_ne!(g.ai_attack_move_update(id), StateReturn::Continue);
}
