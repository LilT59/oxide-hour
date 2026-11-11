//! Lane 125: `AutoDepositUpdate::InitialCaptureBonus` (`AutoDepositUpdate.cpp:113-151`).
//!
//! The constructor only sets `m_depositOnFrame` (`:102-107`). The first update that reaches it arms
//! `m_awardInitialCaptureBonus` and `m_initialized` (`:143-150`); a later team change runs
//! `Player::becomingTeamMember` -> `awardInitialCaptureBonus` (`Player.cpp:1036-1049`), which re-arms the deposit
//! timer and pays `InitialCaptureBonus` once to the new, non-neutral player.

use std::sync::Arc;
use zr_sim::modules2::AutoDepositTemplate;
use zr_sim::template::kind_of;
use zr_sim::{Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

fn derrick(name: &str, bonus: i32) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::STRUCTURE;
    t.max_health = 500.0;
    t.initial_health = 500.0;
    t.auto_deposit = Some(AutoDepositTemplate {
        deposit_timing: 5,
        deposit_amount: 0,
        initial_capture_bonus: bonus,
        upgraded_boost: Vec::new(),
    });
    t
}

fn logic() -> GameLogic {
    let mut g = GameLogic::new(TerrainLogic::flat(80, 80, 0), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g
}

/// Before the first due update the bonus is not armed, so a team change only re-arms the deposit timer
/// (`AutoDepositUpdate.cpp:143-150`). Revert check: arming in the constructor pays it here.
#[test]
fn the_capture_bonus_is_not_paid_before_the_first_deposit_tick() {
    let t = Arc::new(derrick("Derrick", 250));
    let mut g = logic();
    let id = g.create_object(&t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    assert!(!g.find_object(id).unwrap().behavior.deposit_initial_bonus_armed);
    g.set_object_team(id, 1);
    assert_eq!(g.player_money(1), 0, "no bonus before the module has had its first due update");
}

/// After the first due tick the flag is armed; capturing the derrick pays the new owner once
/// (`AutoDepositUpdate.cpp:129-151`) and re-arms `m_depositOnFrame` on every team change. Revert check: without the
/// payout the money stays 0; without the clearing the second capture pays again.
#[test]
fn the_initial_capture_bonus_is_paid_once_after_the_first_deposit_tick() {
    let t = Arc::new(derrick("Derrick", 250));
    let mut g = logic();
    let id = g.create_object(&t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    g.run_frames(6);
    let o = g.find_object(id).unwrap();
    assert!(o.behavior.deposit_initialized, "the first due update ran");
    assert!(o.behavior.deposit_initial_bonus_armed, "and armed the capture bonus");
    g.set_object_team(id, 1);
    assert_eq!(g.player_money(1), 250, "the new owner is paid the capture bonus");
    assert!(!g.find_object(id).unwrap().behavior.deposit_initial_bonus_armed, "paid once");
    // A second capture pays nothing, but the deposit timer is re-armed each time (`:117-124`).
    let before = g.find_object(id).unwrap().behavior.deposit_next;
    g.run_frames(3);
    g.set_object_team(id, 0);
    assert_eq!(g.player_money(0), 0, "the bonus is not paid twice");
    assert_ne!(g.find_object(id).unwrap().behavior.deposit_next, before, "the deposit timer re-armed");
}
