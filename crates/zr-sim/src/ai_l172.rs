//! Lane 172: the old `stop_object_ai` shortcut is gone; its callers run the engine's `privateIdle`
//! (`AIUpdate.cpp:3092-3117`) — the machine's `clear()` (the current state's `onExit`, then
//! `internalClear()`) and `setState(AI_IDLE)` — which does not clear the unit's path. The path is dropped
//! by the completed-move block of the unit's next `AIUpdateInterface::update` (`AIUpdate.cpp:1035-1041`).

use crate::ai_states::{AiStateType, CommandSource};
use crate::body::DamageInfo;
use crate::damage::{DamageType, HUGE_DAMAGE_AMOUNT};
use crate::logic::GameLogic;
use crate::math::Coord3D;
use crate::object::ObjectId;
use crate::template::{kind_of, LocomotorKind, LocomotorParams, ObjectTemplate};
use crate::terrain::TerrainLogic;
use std::sync::Arc;

/// `HijackerUpdate::update` (`HijackerUpdate.cpp:122-126`): when the vehicle the hijacker is riding dies,
/// he is ejected and `ai->aiIdle(CMD_FROM_AI)` runs. With the old `stop_object_ai` the path was cleared
/// inside the call and the state was written directly; `privateIdle` leaves the path for the next
/// update's completed-move block and sets `m_movementComplete` through the move state's exit.
/// Revert check: clear `ai.path` inside the idle path (the old `stop_object_ai`); the last assertion
/// fails.
#[test]
fn an_ejected_hijacker_is_idled_through_private_idle() {
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    let mut inf = ObjectTemplate::new("Hijacker");
    inf.kind_of = kind_of::INFANTRY;
    inf.geometry.major_radius = 3.0;
    inf.max_health = 100.0;
    inf.initial_health = 100.0;
    inf.vision_range = 100.0;
    inf.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Legs, 30.0, 60.0, 120.0, 120.0));
    let hijacker = g.create_object(&Arc::new(inf), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    let mut car = ObjectTemplate::new("Car");
    car.kind_of = kind_of::VEHICLE;
    car.geometry.major_radius = 8.0;
    car.max_health = 100.0;
    car.initial_health = 100.0;
    let vehicle = g.create_object(&Arc::new(car), Coord3D::new(60.0, 50.0, 0.0), 0.0, 1);
    g.ai_move_to_position_cmd(hijacker, Coord3D::new(300.0, 50.0, 0.0), CommandSource::Player);
    g.run_frames(5);
    assert!(!g.find_object(hijacker).unwrap().ai.path.is_empty(), "the hijacker is walking somewhere");
    {
        let h = g.objects.get_mut(&hijacker).unwrap();
        h.behavior.hijacker_vehicle = Some(vehicle);
        h.behavior.hijacker_eject_pos = Coord3D::new(50.0, 50.0, 0.0);
        h.behavior.hijacker_was_airborne = false;
    }
    g.damage_object(vehicle, DamageInfo { source: ObjectId::INVALID, amount: HUGE_DAMAGE_AMOUNT, kind: DamageType::Unresistable });
    g.update();
    let h = g.find_object(hijacker).expect("the hijacker survives");
    assert_eq!(h.behavior.hijacker_vehicle, None, "the vehicle died and ejected him");
    assert_eq!(h.ai.machine.current, AiStateType::Idle, "privateIdle enters AI_IDLE");
    assert!(h.ai.movement_complete, "the move state's exit ran");
    assert!(!h.ai.path.is_empty(), "privateIdle does not clear the path; the next update does");
    g.update();
    assert!(g.find_object(hijacker).unwrap().ai.path.is_empty(), "the next update's completed-move block drops it");
}
