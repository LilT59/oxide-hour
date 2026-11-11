//! Lane 170 (wave 20): AN1 s1 draw 4356 (f2704) — the guard idle state must sleep its AI module.
//!
//! `AIGuardIdleState::update` returns `STATE_SLEEP(m_nextEnemyScanTime - now)` both while it waits for
//! its next scan (`AIGuard.cpp:731`) and after a scan (`:777`), and `AIGuardState::update` returns the
//! guard machine's result (`AIStates.cpp:6793-6798`). `AIUpdateInterface::update` turns that into
//! `UPDATE_SLEEP(frames)` (`AIUpdate.cpp:1011-1037`), the module's sleepy-heap key — so an idle guard
//! is asleep between scans, as the C++ heap dump shows (AN1 s1 f2704: C++ wakes Red Guard 243 at 2702
//! and 2717, the port woke it every frame). The port dropped the sub-state's return and returned
//! CONTINUE for the wait, so every idle guard ran (and drew) every frame.

use std::sync::Arc;
use zr_sim::ai::AiTunables;
use zr_sim::ai_states::AiStateType;
use zr_sim::orders::{GuardMode, GuardTarget};
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams, ObjectTemplate};
use zr_sim::{Command, Coord3D, GameLogic, TerrainLogic};

fn armed() -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new("Gun");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
    t.geometry.major_radius = 8.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.vision_range = 100.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 60.0, 120.0, 120.0, 360.0));
    Arc::new(t)
}

fn world() -> GameLogic {
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    g
}

/// Order `id` to guard the point it stands on.
fn guard_post(g: &mut GameLogic, id: zr_sim::ObjectId) {
    let pos = g.find_object(id).expect("object").position;
    g.submit(0, Command::Guard { ids: vec![id], target: GuardTarget::Point(pos), mode: GuardMode::Normal });
}

/// The frame after a guard enters `AI_GUARD_IDLE`, its AI module sleeps until the next scan: the
/// machine's `sleep_till` is `m_nextEnemyScanTime` (`AIGuard.cpp:731`, `:777`), which is what the
/// module's sleepy-heap key is built from (`AIUpdate.cpp:1011-1037`).
///
/// Revert check (either half): `ai_guard_update` dropping the sub-state's return, or
/// `ai_guard_idle_update`'s wait branch returning CONTINUE, leaves `sleep_till` at 0 and this fails.
#[test]
fn an_idle_guard_sleeps_its_ai_module_until_its_next_scan() {
    let mut g = world();
    g.set_ai_tunables(AiTunables { guard_enemy_scan_rate: 9, ..AiTunables::default() });
    let id = g.create_object(&armed(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    guard_post(&mut g, id);
    let mut entered = false;
    for _ in 0..300 {
        g.update();
        if g.find_object(id).expect("alive").ai.machine.guard_sub == AiStateType::GuardIdle {
            entered = true;
            break;
        }
    }
    assert!(entered, "the guard reaches its post");
    // The entry frame clears `sleep_till` (`internalSetState`); the next update is the idle update,
    // which must sleep until the scan it entered with (or the one it just took).
    g.update();
    let o = g.find_object(id).expect("alive");
    assert_eq!(o.ai.machine.guard_sub, AiStateType::GuardIdle, "still idle");
    let next = o.ai.machine.guard.next_scan_time;
    assert_eq!(o.ai.machine.sleep_till, next, "`STATE_SLEEP(m_nextEnemyScanTime - now)` (`AIGuard.cpp:731`)");
    assert!(o.ai.machine.sleep_till > g.frame(), "the sleep is in the future, not a one-frame wake");
}

/// A sleeping guard is not called every frame: over an idle window it sleeps for most of the frames
/// and runs its scan on the rest. Revert check: dropping the sub-state's return in `ai_guard_update`
/// makes every frame a scan frame (`0 sleeping, 60 awake`).
#[test]
fn a_sleeping_guard_does_not_run_its_mood_scan_between_scans() {
    let mut g = world();
    g.set_ai_tunables(AiTunables { guard_enemy_scan_rate: 12, ..AiTunables::default() });
    let id = g.create_object(&armed(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    guard_post(&mut g, id);
    for _ in 0..300 {
        g.update();
        if g.find_object(id).expect("alive").ai.machine.guard_sub == AiStateType::GuardIdle {
            break;
        }
    }
    let mut scans = 0;
    let mut sleeping = 0;
    for _ in 0..60 {
        g.update();
        let o = g.find_object(id).expect("alive");
        if o.ai.machine.guard_sub != AiStateType::GuardIdle {
            continue;
        }
        if o.ai.machine.sleep_till > g.frame() {
            sleeping += 1;
        } else {
            scans += 1;
        }
    }
    assert!(sleeping > scans * 3, "an idle guard sleeps most frames: {sleeping} sleeping, {scans} awake");
}
