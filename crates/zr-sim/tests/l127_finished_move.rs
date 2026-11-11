//! Lane 127 (wave 16), ROADMAP step 1(a): the finished-move semantics of `AIInternalMoveToState`
//! (`AIStates.cpp:1929-1943`, `AIUpdate.cpp:1035-1061`), `doLocomotor`'s goal switch
//! (`AIUpdate.cpp:2158-2286`) and the dock machine's interruption rules (`AIDock.cpp:70-86`,
//! `StateMachine.cpp:753-763`).

use std::sync::Arc;
use zr_sim::ai_states::{AiStateType, DockSub};
use zr_sim::math::Coord3D;
use zr_sim::supply::{DockKind, DockTemplate};
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::weapon::{CombatTemplate, WeaponSetTemplate, WeaponTemplate};
use zr_sim::{Command, GameLogic, ObjectTemplate, TerrainLogic};

fn unit(name: &str) -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
    t.geometry.major_radius = 8.0;
    t.geometry.height = 10.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.vision_range = 300.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    let mut w = WeaponTemplate::new("Gun");
    w.primary_damage = 20.0;
    w.primary_damage_radius = 5.0;
    w.secondary_damage_radius = 5.0;
    w.attack_range = 100.0;
    w.min_delay_between_shots = 10;
    w.max_delay_between_shots = 10;
    let mut set = WeaponSetTemplate::default();
    set.weapons[0] = Some(Arc::new(w));
    t.combat = CombatTemplate { weapon_set: Some(set), ..CombatTemplate::default() };
    Arc::new(t)
}

fn dock_template() -> ObjectTemplate {
    let mut dock_t = ObjectTemplate::new("Dock");
    dock_t.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;
    dock_t.geometry.major_radius = 12.0;
    dock_t.max_health = 500.0;
    dock_t.initial_health = 500.0;
    dock_t.dock = Some(DockTemplate {
        kind: DockKind::Warehouse,
        starting_boxes: 5,
        delete_when_empty: false,
        number_approach_positions: 5,
        allows_passthrough: false,
    });
    dock_t
}

/// `AIInternalMoveToState::update`'s arrival (`AIStates.cpp:1929-1940`) does not clear the state's
/// `m_goalPosition`; only `if (getAdjustsDestination()) ai->setLocomotorGoalNone()` runs. The port's
/// `ai.move_goal` is that member, so after a completed move it still holds the (adjusted) destination,
/// and the locomotor goal is `NONE`.
/// Revert check: the old `ai_finish_move` (`move_goal = None`) makes `move_goal` `None` here.
#[test]
fn a_finished_move_keeps_its_destination() {
    let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
    g.add_player(0, 10000);
    let id = g.create_object(&unit("Tank"), Coord3D::new(20.0, 20.0, 0.0), 0.0, 0);
    g.submit(0, Command::MoveTo { ids: vec![id], pos: Coord3D::new(60.0, 20.0, 0.0) });
    g.update();
    let destination = g.find_object(id).unwrap().ai.move_goal.expect("the move began with a goal");
    g.run_frames(300);
    let o = g.find_object(id).unwrap();
    assert_eq!(o.ai.machine.current, AiStateType::Idle, "the move completed");
    assert_eq!(o.ai.move_goal, Some(destination), "m_goalPosition outlives the arrival");
    assert_eq!(o.ai.locomotor_goal_type, zr_sim::ai::LocoGoalType::None, "the arrival cleared the locomotor goal");
    assert!(
        o.position.dist_2d(&destination) < 2.0,
        "the unit stopped at its destination ({:.2},{:.2} vs {:.2},{:.2})",
        o.position.x,
        o.position.y,
        destination.x,
        destination.y
    );
}

/// Step until the docker's dock machine is in `AI_DOCK_MOVE_TO_DOCK` (`AIDock.cpp:57`).
fn step_to_move_to_dock(g: &mut GameLogic, docker: zr_sim::ObjectId) -> bool {
    for _ in 0..600 {
        g.update();
        if g.find_object(docker).is_some_and(|o| o.ai.machine.dock_sub == DockSub::MoveToDock) {
            return true;
        }
    }
    false
}

/// `AIDockMoveToDockState::onEnter`'s `getMachine()->lock(...)` (`AIDock.cpp:515`) locks the *dock*
/// state machine, not the owner's `AIStateMachine`. A player's stop therefore interrupts the dock move
/// (`AIUpdateInterface::privateIdle` -> `getStateMachine()->clear()`), as it does in C++.
/// Revert check: with the port's old `ai_set_machine_locked(id, true)` the owner's machine is locked,
/// `ai_clear` refuses, and the unit stays in `Dock`.
#[test]
fn a_stop_order_interrupts_a_dock_move() {
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 0);
    let docker = g.create_object(&unit("Truck"), Coord3D::new(50.0, 100.0, 0.0), 0.0, 0);
    let dock = g.create_object(&Arc::new(dock_template()), Coord3D::new(160.0, 100.0, 0.0), 0.0, 0);
    g.ai_dock(docker, dock);
    assert!(step_to_move_to_dock(&mut g, docker), "the docker reached AIDockMoveToDockState");
    g.submit(0, Command::Stop { ids: vec![docker] });
    g.update();
    assert_eq!(g.find_object(docker).unwrap().ai.machine.current, AiStateType::Idle, "the stop was accepted");
}

/// `AIDockMachine::halt` (`AIDock.cpp:70-86`) only cancels the reservation; `StateMachine::halt`
/// (`StateMachine.cpp:753-763`) clears the current sub-state **without running its `onExit`**, so an
/// interrupted dock does not call `AIInternalMoveToState::onExit` (`friend_endingMove`) and the move is
/// not marked complete. The docker is idle, keeps no `movement_complete` flag and the dock's spot is
/// free again.
/// Revert check: the old `ai_dock_exit` called `ai_internal_move_exit`, so `movement_complete` is set.
#[test]
fn an_interrupted_dock_does_not_complete_the_move() {
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 0);
    let docker = g.create_object(&unit("Truck"), Coord3D::new(50.0, 100.0, 0.0), 0.0, 0);
    let dock = g.create_object(&Arc::new(dock_template()), Coord3D::new(160.0, 100.0, 0.0), 0.0, 0);
    g.ai_dock(docker, dock);
    assert!(step_to_move_to_dock(&mut g, docker), "the docker reached AIDockMoveToDockState");
    g.submit(0, Command::Stop { ids: vec![docker] });
    g.update();
    let o = g.find_object(docker).unwrap();
    assert_eq!(o.ai.machine.current, AiStateType::Idle);
    assert!(!o.ai.movement_complete, "no sub-state onExit ran, so no friend_endingMove");
    assert!(o.ai.machine.dock_sub == DockSub::Approach, "the dock machine's sub-state is reset after the halt");
    // `DockUpdate::cancelDock` (`DockUpdate.cpp:381`) left no trace of the docker on the dock either:
    // the dock is open again for the next docker.
    assert_eq!(g.find_object(dock).unwrap().dock_state.active_docker, zr_sim::ObjectId::INVALID);
}
