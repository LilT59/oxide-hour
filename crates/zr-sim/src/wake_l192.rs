//! `AIUpdateInterface::wakeUpNow` (lane 192, wave 22): the forward-only pull of a sleeping AI module.
//!
//! C++ (AIUpdate.cpp:973-988): `if (getWakeFrame() > UPDATE_SLEEP_NONE) { if (m_isInUpdate) {} else
//! setWakeFrame(getObject(), UPDATE_SLEEP_NONE); }`. `getWakeFrame` (UpdateModule.cpp:68-76) is `UPDATE_SLEEP(next - now)`
//! while the next call is ahead, else NONE, so the pull happens only for a module asleep past `now + 1`. Only a state
//! change clears the machine's sleep (`StateMachine::internalSetState`, StateMachine.cpp:567); a bare wake does not.

use std::sync::Arc;

use crate::logic::GameLogic;
use crate::math::Coord3D;
use crate::module::UPDATE_SLEEP_NONE;
use crate::object::ObjectId;
use crate::template::ObjectTemplate;
use crate::terrain::TerrainLogic;

fn logic() -> GameLogic {
    GameLogic::new(TerrainLogic::flat(100, 100, 0), 1)
}

fn ai_unit() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Wanderer");
    t.kind_of = crate::template::kind_of::VEHICLE | crate::template::kind_of::SELECTABLE;
    t.geometry.major_radius = 5.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.ai_update = Some(0);
    t
}

/// The heap key's frame of the AI module of `id`.
fn ai_key_frame(g: &GameLogic, id: ObjectId) -> u32 {
    let h = g.objects[&id].ai_handle.expect("an AI module");
    g.schedule.slot(h.0).next_call_and_phase >> 2
}

/// A sleeping AI module is pulled to the next frame, and its machine's sleep is left alone (wakeUpNow does not reset
/// `m_sleepTill`). Revert check: without the forward pull (`wake_up_now` does nothing) the key stays at `now + 50`.
#[test]
fn a_sleeping_ai_module_is_pulled_to_the_next_frame_and_keeps_its_machine_sleep() {
    let mut g = logic();
    g.add_player(0, 0);
    let id = g.create_object(&Arc::new(ai_unit()), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let now = g.frame();
    let h = g.objects[&id].ai_handle.expect("an AI module");
    g.schedule.awaken(h, now, now + 50);
    g.objects.get_mut(&id).expect("exists").ai.machine.sleep_till = now + 50;
    g.wake_up_now(id);
    assert_eq!(ai_key_frame(&g, id), now + UPDATE_SLEEP_NONE, "pulled to the next frame");
    assert_eq!(g.objects[&id].ai.machine.sleep_till, now + 50, "a bare wake leaves the machine asleep");
}

/// A module due this frame keeps its slot: wakeUpNow never delays it (`getWakeFrame()` is NONE, not `> NONE`,
/// AIUpdate.cpp:977). The test runs at frame 0, where `schedule.awaken`'s own "already awake this frame" guard
/// (`now > 0`) is off; at a later frame that guard would hide an unconditional pull, so it cannot discriminate there.
/// Revert check: an unconditional `awaken(now + 1)` moves the key from 0 to 1 and fails the assert.
#[test]
fn a_module_due_this_frame_is_not_delayed() {
    let mut g = logic();
    g.add_player(0, 0);
    let id = g.create_object(&Arc::new(ai_unit()), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let now = g.frame();
    assert_eq!(now, 0, "frame 0: the awaken guard is off");
    let h = g.objects[&id].ai_handle.expect("an AI module");
    g.schedule.awaken(h, now, now);
    g.wake_up_now(id);
    assert_eq!(ai_key_frame(&g, id), now, "still due this frame");
}

/// `setQueueForPathTime(frames)` (AIUpdate.cpp:955-968) pulls a module asleep past `frames` to `now + frames`, and never
/// delays one due sooner. Revert check: without the call in `wake_up_for_queue_time` the key stays at `now + 50`.
#[test]
fn a_queued_path_pulls_a_long_sleeper_to_its_queue_time_only() {
    let mut g = logic();
    g.add_player(0, 0);
    let id = g.create_object(&Arc::new(ai_unit()), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let now = g.frame();
    let h = g.objects[&id].ai_handle.expect("an AI module");
    g.schedule.awaken(h, now, now + 50);
    g.wake_up_for_queue_time(id, 10);
    assert_eq!(ai_key_frame(&g, id), now + 10, "pulled to the queue time");
    g.wake_up_for_queue_time(id, 30);
    assert_eq!(ai_key_frame(&g, id), now + 10, "a later queue time does not delay the module");
}

/// `doPathfind`'s tail: `if (!m_waitingForPath) wakeUpNow()` (AIUpdate.cpp:466-472). A computed path wakes a sleeper;
/// a path still queued does not. Revert check: without the wake in `ai_pathfind_tail` the first key stays at `now + 50`.
#[test]
fn a_computed_path_wakes_the_module_and_a_queued_one_does_not() {
    let mut g = logic();
    g.add_player(0, 0);
    let id = g.create_object(&Arc::new(ai_unit()), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let now = g.frame();
    let h = g.objects[&id].ai_handle.expect("an AI module");
    g.schedule.awaken(h, now, now + 50);
    g.objects.get_mut(&id).expect("exists").ai.waiting_for_path = false;
    g.ai_pathfind_tail(id);
    assert_eq!(ai_key_frame(&g, id), now + UPDATE_SLEEP_NONE, "a computed path wakes the module");
    g.schedule.awaken(h, now, now + 50);
    g.objects.get_mut(&id).expect("exists").ai.queue_for_path_frame = now + 20;
    g.ai_pathfind_tail(id);
    assert_eq!(ai_key_frame(&g, id), now + 50, "a queued path leaves it asleep");
}

/// `markAsDead` (AIUpdate.cpp:1193-1198) wakes the unit so its death update runs promptly. Revert check: without the
/// wake in `mark_effectively_dead` the key stays at `now + 50`.
#[test]
fn marking_a_unit_dead_wakes_its_module() {
    let mut g = logic();
    g.add_player(0, 0);
    let id = g.create_object(&Arc::new(ai_unit()), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let now = g.frame();
    let h = g.objects[&id].ai_handle.expect("an AI module");
    g.schedule.awaken(h, now, now + 50);
    g.mark_effectively_dead(id);
    assert_eq!(ai_key_frame(&g, id), now + UPDATE_SLEEP_NONE, "woken for the death update");
}

/// `aiIdle` (`privateIdle`, AIUpdate.cpp:3097, `getStateMachine()->clear()`) notifies the AI and wakes it, so
/// `end_attack` does. The attack's own internal exit (`AIAttackState::onExit`, AIStates.cpp:5714-5741, no notify, an
/// internal transition on `AttackStateMachine`) does not, so `end_attack_internal` leaves the module asleep. Revert
/// check: a wake added to `end_attack_internal` fails the second assert; removing it from `end_attack` fails the first.
#[test]
fn an_idle_command_wakes_but_an_attack_internal_exit_does_not() {
    let mut g = logic();
    g.add_player(0, 0);
    let id = g.create_object(&Arc::new(ai_unit()), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let now = g.frame();
    let h = g.objects[&id].ai_handle.expect("an AI module");
    g.schedule.awaken(h, now, now + 50);
    g.end_attack(id);
    assert_eq!(ai_key_frame(&g, id), now + UPDATE_SLEEP_NONE, "aiIdle wakes the module");
    g.schedule.awaken(h, now, now + 50);
    g.end_attack_internal(id);
    assert_eq!(ai_key_frame(&g, id), now + 50, "the internal exit does not wake it");
}

/// A mobile unit with two locomotor sets (the second one a copy of the first), so a set switch has a target.
fn mover() -> ObjectTemplate {
    let loco = crate::template::LocomotorParams::from_ini(crate::template::LocomotorKind::Treads, 60.0, 100.0, 100.0, 360.0);
    let mut t = ai_unit();
    t.locomotor = Some(loco);
    t.locomotor_sets.insert(crate::template::LocomotorSet::Normal, loco);
    t.locomotor_sets.insert(crate::template::LocomotorSet::Sluggish, loco);
    t
}

/// `setLocomotorUpgrade`, `chooseLocomotorSet` and `chooseGoodLocomotorFromCurrentSet` never wake the module
/// (AIUpdate.cpp:801-872): a set switch leaves a sleeping AI asleep (R28 F1). Revert check: re-adding the wake to
/// `set_locomotor_set` pulls the key to `now + 1` and fails the assert.
#[test]
fn a_locomotor_set_switch_does_not_wake_the_module() {
    let mut g = logic();
    g.add_player(0, 0);
    let id = g.create_object(&Arc::new(mover()), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let now = g.frame();
    let h = g.objects[&id].ai_handle.expect("an AI module");
    g.schedule.awaken(h, now, now + 50);
    assert!(g.set_locomotor_set(id, crate::template::LocomotorSet::Sluggish), "the template has that set");
    assert_eq!(ai_key_frame(&g, id), now + 50, "the set switch does not wake the module");
}

/// `ai_stop_and_wait` queues its path for a second and pulls a sleeper to that queue time (`setQueueForPathTime`,
/// AIUpdate.cpp:1754 per R28 F3). Revert check: without the call the key stays at `now + 50`.
#[test]
fn a_stop_and_wait_pulls_a_sleeper_to_the_one_second_queue() {
    let mut g = logic();
    g.add_player(0, 0);
    let id = g.create_object(&Arc::new(mover()), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let now = g.frame();
    let h = g.objects[&id].ai_handle.expect("an AI module");
    g.schedule.awaken(h, now, now + 50);
    let po = crate::pathfind::PathObject::for_object(&g.objects[&id]).expect("a mover has a path object");
    g.ai_stop_and_wait(id, &po);
    assert_eq!(ai_key_frame(&g, id), now + crate::math::LOGICFRAMES_PER_SECOND, "pulled to the one-second queue");
}

/// A locked machine refuses a state change, but `AIStateMachine::clear` still drops its goal path and notifies
/// (AIStates.cpp:1017-1033), and `AIStateMachine::setState` notifies whenever the requested ID differs from the current
/// one, locked or not (AIStates.cpp:1050-1058). Revert check: removing the wake from the locked branch of `ai_clear`
/// leaves the first key at `now + 50` and fails the first assert.
#[test]
fn a_locked_machine_still_notifies_on_clear_and_on_a_different_state() {
    let mut g = logic();
    g.add_player(0, 0);
    let id = g.create_object(&Arc::new(ai_unit()), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let now = g.frame();
    let h = g.objects[&id].ai_handle.expect("an AI module");
    g.objects.get_mut(&id).expect("exists").ai.machine.locked = true;
    g.schedule.awaken(h, now, now + 50);
    g.ai_clear(id);
    assert_eq!(ai_key_frame(&g, id), now + UPDATE_SLEEP_NONE, "clear notifies a locked machine");
    g.schedule.awaken(h, now, now + 50);
    g.ai_set_state(id, crate::ai_states::AiStateType::Busy);
    assert_eq!(ai_key_frame(&g, id), now + UPDATE_SLEEP_NONE, "a refused change to another state still notifies");
    g.schedule.awaken(h, now, now + 50);
    g.ai_set_state(id, crate::ai_states::AiStateType::Idle);
    assert_eq!(ai_key_frame(&g, id), now + 50, "a refused set to the current state does not notify");
}
