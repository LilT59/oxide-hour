//! Lane 172: `AIUpdateInterface::privateIdle` (`AIUpdate.cpp:3092-3117`) instead of the port's old
//! `stop_object_ai` shortcut.
//!
//! `privateIdle` runs the state machine's `clear()` — the current state's `onExit(EXIT_RESET)`, then
//! `internalClear()` — and then `setState(AI_IDLE)`. It does **not** clear the unit's path: the move
//! state's exit only sets `m_movementComplete` (`AIStates.cpp:1736-1747`), and the completed-move block of
//! the unit's next `AIUpdateInterface::update` drops the path (`AIUpdate.cpp:1035-1041`). The old
//! `stop_object_ai` wrote `current = Idle` without an exit and cleared the path immediately, so a hijacked
//! vehicle or an ejected hijacker (`ConvertToHijackedVehicleCrateCollide.cpp:167-168`, `:234`,
//! `HijackerUpdate.cpp:122-126`) lost its move without the state's exit running.

use std::sync::Arc;

use zr_sim::ai_states::{AiStateType, CommandSource};
use zr_sim::modules2::CrateCollideTemplate;
use zr_sim::modules2::CrateEffect;
use zr_sim::template::{kind_of, HijackerTemplate, LocomotorKind, LocomotorParams};
use zr_sim::{Command, Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

fn walker() -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new("Walker");
    t.kind_of = kind_of::INFANTRY | kind_of::SELECTABLE;
    t.geometry.major_radius = 3.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.vision_range = 200.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Legs, 30.0, 60.0, 120.0, 120.0));
    Arc::new(t)
}

/// `privateIdle` keeps the path and leaves the exit's `m_movementComplete` set, so the next update's
/// completed-move block drops the path. Revert check: clear `o.ai.path` inside the idle path (the old
/// `stop_object_ai`); the first assertion fails.
#[test]
fn an_idle_keeps_the_path_until_the_next_update_drops_it() {
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 0);
    let u = g.create_object(&walker(), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    g.ai_move_to_position_cmd(u, Coord3D::new(300.0, 50.0, 0.0), CommandSource::Player);
    g.run_frames(5);
    assert!(!g.find_object(u).unwrap().ai.path.is_empty(), "the move has a path to run");
    g.ai_idle(u, CommandSource::Ai);
    let o = g.find_object(u).unwrap();
    assert_eq!(o.ai.machine.current, AiStateType::Idle, "privateIdle enters AI_IDLE");
    assert!(!o.ai.path.is_empty(), "privateIdle does not clear the path");
    assert!(o.ai.movement_complete, "the move state's exit ran (m_movementComplete)");
    g.update();
    assert!(g.find_object(u).unwrap().ai.path.is_empty(), "the completed-move block drops the path on the next update");
}

/// `ConvertToHijackedVehicleCrateCollide::executeCrateBehavior` (`:166-168`): the stolen vehicle gets
/// `targetAI->aiMoveToPosition(other->getPosition(), CMD_FROM_AI)` — for a busy vehicle the 20-second
/// temporary `AI_MOVE_TO` (`AIUpdate.cpp:2900-2909`) — and then `targetAI->aiIdle(CMD_FROM_AI)`, which
/// clears the machine but keeps the temporary state in the retail build. Revert check: put the old
/// `stop_object_ai` back (clear the path, write `current = Idle`); there is no temporary move.
#[test]
fn a_hijacked_vehicle_gets_the_ai_move_and_then_the_idle() {
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    let mut hijacker = ObjectTemplate::new("Hijacker");
    hijacker.kind_of = kind_of::INFANTRY;
    hijacker.geometry.major_radius = 3.0;
    hijacker.max_health = 100.0;
    hijacker.initial_health = 100.0;
    hijacker.vision_range = 100.0;
    hijacker.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Legs, 30.0, 60.0, 120.0, 120.0));
    // A weapon: the hijack only runs when the hijacker's AI goal is the vehicle, which the attack order
    // needs (`CrateCollide::isValidToExecute`, `ai->getGoalObject() == other`, `:149`).
    let mut gun = zr_sim::weapon::WeaponTemplate::new("Gun");
    gun.primary_damage = 1.0;
    gun.attack_range = 100.0;
    gun.min_delay_between_shots = 30;
    gun.max_delay_between_shots = 30;
    let mut set = zr_sim::weapon::WeaponSetTemplate::default();
    set.weapons[0] = Some(Arc::new(gun));
    hijacker.combat.weapon_set = Some(set);
    hijacker.crate_collides = vec![CrateCollideTemplate { effect: CrateEffect::ConvertToHijackedVehicle, ..Default::default() }];
    hijacker.hijacker = Some(HijackerTemplate { attach_to_bone: String::new(), parachute: None });
    let hid = g.create_object(&Arc::new(hijacker), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let mut vehicle = ObjectTemplate::new("Vehicle");
    vehicle.kind_of = kind_of::VEHICLE;
    vehicle.geometry.major_radius = 8.0;
    vehicle.max_health = 100.0;
    vehicle.initial_health = 100.0;
    // Slow, so it is still next to the hijacker when the collision runs.
    vehicle.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 1.0, 60.0, 120.0, 120.0));
    vehicle.eject_pilot = Some(zr_sim::modules2::EjectPilotTemplate { air_ocl: None, ground_ocl: None });
    let vid = g.create_object(&Arc::new(vehicle), Coord3D::new(103.0, 100.0, 0.0), 0.0, 1);
    // A busy vehicle: its `aiMoveToPosition` is the temporary branch.
    g.ai_move_to_position_cmd(vid, Coord3D::new(300.0, 100.0, 0.0), CommandSource::Player);
    g.update();
    assert_ne!(g.find_object(vid).unwrap().ai.machine.current, AiStateType::Idle, "the vehicle is busy");
    g.submit(0, Command::Attack { ids: vec![hid], target: vid });
    for _ in 0..6 {
        g.update();
        if g.find_object(hid).is_some_and(|h| h.behavior.hijacker_vehicle == Some(vid)) {
            break;
        }
    }
    let v = g.find_object(vid).expect("the vehicle survived the hijack");
    assert_eq!(v.player, 0, "the vehicle defected");
    assert_eq!(v.ai.machine.current, AiStateType::Idle, "privateIdle enters AI_IDLE");
    assert_eq!(v.ai.machine.temporary, Some(AiStateType::MoveTo), "the aiMoveToPosition's temporary move survives the idle");
}

/// The hijack's own sequence (`ConvertToHijackedVehicleCrateCollide.cpp:167-168`): a `CMD_FROM_AI` move of
/// a busy unit becomes the 20-second temporary `AI_MOVE_TO` (`AIUpdate.cpp:2900-2909`), and the
/// `privateIdle` after it clears the machine but — in the retail build, where the
/// `#if !RETAIL_COMPATIBLE_CRC` block of `AIStateMachine::clear()` is off (`AIStates.cpp:1017-1027`) — does
/// not drop the temporary state. It therefore runs on the unit's next update, then the main `AI_IDLE`
/// resumes. This pins the engine's behaviour; it is not a revert check.
#[test]
fn an_idle_after_an_ai_move_keeps_the_temporary_move() {
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 0);
    let u = g.create_object(&walker(), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    g.ai_move_to_position_cmd(u, Coord3D::new(300.0, 50.0, 0.0), CommandSource::Player);
    g.run_frames(5);
    assert_eq!(g.find_object(u).unwrap().ai.machine.current, AiStateType::MoveTo);
    g.ai_move_to_position_cmd(u, Coord3D::new(50.0, 50.0, 0.0), CommandSource::Ai);
    assert_eq!(g.find_object(u).unwrap().ai.machine.temporary, Some(AiStateType::MoveTo), "the busy unit takes a temporary move");
    g.ai_idle(u, CommandSource::Ai);
    let o = g.find_object(u).unwrap();
    assert_eq!(o.ai.machine.current, AiStateType::Idle, "the machine is cleared and idled");
    assert_eq!(o.ai.machine.temporary, Some(AiStateType::MoveTo), "the retail clear keeps the temporary state");
}
