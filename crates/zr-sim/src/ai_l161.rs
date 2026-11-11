//! Lane 161 (wave 19): `AIMoveToState::update` hands `m_goalPosition` to `aiAttackMoveToPosition` (AIStates.cpp:2087-2088).
//! `AIInternalMoveToState::onEnter` has already moved that goal in place (`adjustDestination` or
//! `snapClosestGoalPosition`, AIStates.cpp:1661-1664), so the attack-move starts from the adjusted point. The port
//! passed the state machine's unadjusted goal. A China Command Center is the goal of a reinforcement's join move
//! (`AIUpdate.cpp:2536-2587`, `joinTeam`): its centre is inside the footprint, so the adjust matters there.

use crate::ai_states::{AiStateType, CommandSource};
use crate::logic::GameLogic;
use crate::math::Coord3D;
use crate::skirmish_ai::SkirmishAiConfig;
use crate::template::{kind_of, LocomotorKind, LocomotorParams, ObjectTemplate};
use crate::terrain::TerrainLogic;
use std::sync::Arc;

/// A treaded vehicle that can attack and has an AI.
fn vehicle() -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new("Vehicle");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.geometry.major_radius = 5.0;
    t.vision_range = 300.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 60.0, 100.0, 100.0, 360.0));
    Arc::new(t)
}

/// A structure whose centre is an obstacle cell (its footprint covers the goal).
fn block() -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new("Block");
    t.kind_of |= kind_of::STRUCTURE | kind_of::IMMOBILE;
    t.geometry.major_radius = 10.0;
    t.geometry.minor_radius = 10.0;
    t.geometry.height = 20.0;
    t.max_health = 1000.0;
    t.initial_health = 1000.0;
    Arc::new(t)
}

fn dist(a: &Coord3D, b: &Coord3D) -> f32 {
    a.dist_2d(b)
}

#[test]
fn a_mood_converted_move_gives_the_attack_move_the_adjusted_goal() {
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 0);
    g.add_ai_player(0, SkirmishAiConfig::default());
    let unit = g.create_object(&vehicle(), Coord3D::new(100.0, 200.0, 0.0), 0.0, 0);
    g.set_unit_attitude(unit, 1);
    g.create_object(&block(), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let raw = Coord3D::new(200.0, 200.0, 0.0);
    // A player move: the unit is idle, so the machine goes to `AI_MOVE_TO` with the raw goal. Its enter adjusts the
    // goal (AIStates.cpp:1661); its update converts to an attack-move on the adjusted goal (AIStates.cpp:2087-2088).
    g.ai_private_move_to_position(unit, raw, CommandSource::Player);
    // The move waits for its path before the update converts it (a few frames).
    for _ in 0..10 {
        if g.find_object(unit).expect("the vehicle").ai.machine.current == AiStateType::AttackMoveTo {
            break;
        }
        g.run_frames(1);
    }
    let o = g.find_object(unit).expect("the vehicle");
    assert_eq!(o.ai.machine.current, AiStateType::AttackMoveTo, "the mood turned the move into an attack-move");
    let adjusted = o.ai.move_goal.expect("the move's adjusted goal");
    assert!(dist(&adjusted, &raw) > 5.0, "the centre of the building is moved to a free cell, {adjusted:?}");
    assert_eq!(
        o.ai.machine.goal_position, adjusted,
        "`aiAttackMoveToPosition(&m_goalPosition)` passes the adjusted goal (AIStates.cpp:2088)"
    );
}
