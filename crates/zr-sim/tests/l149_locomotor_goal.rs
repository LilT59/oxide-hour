//! Lane 149 (wave 17), R14 finding 8: the locomotor goal setters must maintain
//! `m_locomotorGoalData` exactly as `AIUpdate.cpp:2450-2489` does, because the per-object trace's
//! `gx` field and the sim CRC (`AiState::crc` hashes `locomotor_goal_data`) both read it.
//!
//! * `setLocomotorGoalPositionOnPath` (`:2450-2454`) sets the type **and zeroes the data**.
//! * `setLocomotorGoalPositionExplicit` (`:2461-2466`) sets the type and the full position.
//! * `setLocomotorGoalOrientation` (`:2478-2483`) sets the type and writes **only `.x`**; y and z keep
//!   whatever the previous goal left there.
//! * `setLocomotorGoalNone` (`:2486-2489`) sets the type and leaves the data alone.

use zr_sim::ai::{AiState, LocoGoalType};
use zr_sim::math::Coord3D;
use zr_sim::TerrainLogic;

/// `move_to` is the quick-path arm of `computePath`, which calls `setLocomotorGoalPositionOnPath()`
/// when it installs a path (`AIUpdate.cpp:1740-1745`): the old data must be gone.
#[test]
fn a_quick_path_goal_zeroes_the_data() {
    let mut ai = AiState { locomotor_goal_data: Coord3D::new(5.0, 6.0, 7.0), ..Default::default() };
    let terrain = TerrainLogic::flat(100, 100, 0);
    ai.move_to(&Coord3D::new(10.0, 10.0, 0.0), &Coord3D::new(30.0, 30.0, 0.0), &terrain);
    assert!(!ai.path.is_empty(), "the quick path installed a path");
    assert_eq!(ai.locomotor_goal_type, LocoGoalType::PositionOnPath);
    assert_eq!(ai.locomotor_goal_data, Coord3D::ZERO, "setLocomotorGoalPositionOnPath zeroes m_locomotorGoalData");
}

/// Revert check: the old setter wrote `(angle, 0.0, 0.0)`, so the y/z assert fails.
#[test]
fn an_orientation_goal_writes_only_x() {
    let mut ai = AiState { locomotor_goal_data: Coord3D::new(0.0, 4.0, 5.0), ..Default::default() };
    ai.set_locomotor_goal_orientation(0.75);
    assert_eq!(ai.locomotor_goal_type, LocoGoalType::Angle);
    assert_eq!(ai.locomotor_goal_data, Coord3D::new(0.75, 4.0, 5.0), "setLocomotorGoalOrientation writes m_locomotorGoalData.x only");
}

#[test]
fn an_explicit_goal_writes_the_full_position() {
    let mut ai = AiState { locomotor_goal_data: Coord3D::new(1.0, 2.0, 3.0), ..Default::default() };
    ai.set_locomotor_goal_explicit(Coord3D::new(40.0, 50.0, 60.0));
    assert_eq!(ai.locomotor_goal_type, LocoGoalType::PositionExplicit);
    assert_eq!(ai.locomotor_goal_data, Coord3D::new(40.0, 50.0, 60.0));
}

/// `setLocomotorGoalNone` only changes the type; `doLocomotor` then maintains position and the stale
/// data stays for the next `gx` print and CRC.
#[test]
fn a_none_goal_leaves_the_data() {
    let mut ai = AiState::default();
    ai.set_locomotor_goal_position_on_path();
    assert_eq!(ai.locomotor_goal_data, Coord3D::ZERO);
    ai.set_locomotor_goal_explicit(Coord3D::new(9.0, 8.0, 7.0));
    ai.set_locomotor_goal_none();
    assert_eq!(ai.locomotor_goal_type, LocoGoalType::None);
    assert_eq!(ai.locomotor_goal_data, Coord3D::new(9.0, 8.0, 7.0), "setLocomotorGoalNone leaves m_locomotorGoalData");
}
