//! Lane 121: an `AIUpdate` object enters `AI_IDLE` during its construction (`AIUpdateInterface::onObjectCreated`,
//! `AIUpdate.cpp:630`), and `AIIdleState::onEnter` runs `ai->resetNextMoodCheckTime()` there (`AIStates.cpp:1321` ->
//! `AIUpdate.cpp:4498-4502`): the first mood check is `ForceIdleMSEC` frames after the creation frame, with the one
//! random offset armed. The `AIIdleState::update` of the first update then caps its 60-frame idle sleep at that check
//! (`AIStates.cpp:1454-1466`). Before this lane the port left `m_nextMoodCheckTime` at 0, so an idle unit slept its
//! full 60-frame countdown on the first update instead of the 2-3 frames C++ uses (`AIData.ini` has
//! `ForceIdleMSEC = 67`, i.e. `ceil(67 * 30 / 1000)` = 3 frames, `INI.cpp:1798-1802`), and the seven civilian cars of
//! AN3 were two frames off the C++ heap key from then on.

use std::sync::Arc;
use zr_sim::ai::AiTunables;
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::{Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

/// A mobile unit with an `AIUpdate` module (so its machine enters `AI_IDLE` at construction) and no weapon.
fn car(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    t.ai_update = Some(0);
    t
}

/// The creation frame's mood check: `creation + ForceIdleMSEC`, and the offset armed (`resetNextMoodCheckTime`).
/// Revert check: drop the reset from the construction path and `next_mood_check` is 0.
#[test]
fn a_newly_created_unit_arms_its_first_mood_check_at_creation() {
    let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 1);
    let id = g.create_object(&Arc::new(car("Car")), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let o = g.find_object(id).unwrap();
    assert_eq!(o.combat.next_mood_check, 1, "the default ForceIdleMSEC is one frame");
    assert!(o.combat.randomly_offset_mood_check, "the first check carries the one random offset");

    // With the install's AIData value (`ForceIdleMSEC = 67` ms -> 3 frames) the first check is frame 3.
    g.set_ai_tunables(AiTunables { force_idle_frames: 3, ..Default::default() });
    let id2 = g.create_object(&Arc::new(car("Car2")), Coord3D::new(150.0, 100.0, 0.0), 0.0, 0);
    assert_eq!(g.find_object(id2).unwrap().combat.next_mood_check, 3, "creation frame + 3");
}

/// The consequence on the unit's first update (the AN3 car case): the mood check is not due at frame 1, so the idle
/// update leaves it at frame 3 and caps its sleep at two frames (`next - now`, AIStates.cpp:1454-1466) instead of
/// sleeping its full 60-frame idle countdown. Revert check: without the creation reset the check is due at once and
/// the value is no longer 3 after the first update.
#[test]
fn the_first_idle_update_does_not_consume_the_creation_mood_check() {
    let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 1);
    g.set_ai_tunables(AiTunables { force_idle_frames: 3, ..Default::default() });
    let id = g.create_object(&Arc::new(car("Car")), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    // The AI module's first update is frame 1 (its constructor asked for the next frame).
    g.run_frames(2);
    let o = g.find_object(id).unwrap();
    assert_eq!(o.combat.next_mood_check, 3, "frame 1's idle update keeps the creation check");
    assert!(o.combat.randomly_offset_mood_check, "and its offset is still pending");
}
