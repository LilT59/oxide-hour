//! `AIUpdateInterface` movement machinery: the async pathfind queue, blocked-and-stuck detection
//! and repathing. Synthetic data only.

use std::sync::Arc;
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::{Command, Coord3D, GameLogic, ObjectTemplate, PhysicsTemplate, TerrainLogic};

const LOGIC_FPS: u32 = 30;

fn flat() -> TerrainLogic {
    TerrainLogic::flat(120, 120, 0)
}

/// A vehicle with its `PhysicsBehavior`. The engine asks the AI about a contact only from that module's `onCollide`
/// (`PhysicsUpdate.cpp:1278`), so a unit without one is never blocked by a contact (lane 181).
fn tank(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 8.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.physics = Some(PhysicsTemplate::default());
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    t
}

/// `requestPath` -> `queueForPath` -> `processPathfindQueue`: the move order is serviced by the
/// async queue (`AIUpdate.cpp:479`, `AIPathfind.cpp:5989`).
#[test]
fn a_move_order_is_serviced_by_the_pathfind_queue() {
    let mut g = GameLogic::new(flat(), 1);
    let t = Arc::new(tank("T"));
    let id = g.create_object(&t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    g.submit(0, Command::MoveTo { ids: vec![id], pos: Coord3D::new(600.0, 100.0, 0.0) });
    g.update();
    let o = g.find_object(id).unwrap();
    assert!(o.ai.is_moving(), "the move state is active");
    assert!(g.stats().pathfind_requests >= 1, "the request was serviced this frame");
    assert!(!o.ai.path.is_empty(), "the queue produced a path: {:?}", o.ai.path);
    assert!(!o.ai.waiting_for_path, "the request is complete");
}

/// `processCollision` + `blockedBy`: a moving unit overlapped by a fixed enemy on its route is
/// marked blocked; with the enemy fixed it becomes blocked-and-stuck (`AIUpdate.cpp:1503-1511`).
#[test]
fn a_fixed_enemy_in_the_way_marks_the_mover_blocked_and_stuck() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    let t = Arc::new(tank("T"));
    let mover = g.create_object(&t, Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    // Parked enemy in the mover's path (one cell ahead, so the bounding circles overlap).
    let _enemy = g.create_object(&t, Coord3D::new(215.0, 200.0, 0.0), 0.0, 1);
    g.submit(0, Command::MoveTo { ids: vec![mover], pos: Coord3D::new(800.0, 200.0, 0.0) });
    let mut saw_blocked = false;
    let mut saw_stuck = false;
    for _ in 0..2 * LOGIC_FPS {
        g.update();
        if let Some(o) = g.find_object(mover) {
            saw_blocked |= o.ai.is_blocked || o.ai.blocked_frames > 0;
            saw_stuck |= o.ai.is_blocked_and_stuck;
        }
    }
    assert!(saw_blocked, "the mover was seen blocked by the parked enemy");
    assert!(saw_stuck, "a fixed enemy makes it blocked-and-stuck");
}

/// The blocked move re-requests a path through the queue (a `findClosestPath`/`patchPath`
/// repath), so a blocked unit does not sit on a stale path forever.
#[test]
fn a_blocked_unit_repaths_through_the_queue() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    let t = Arc::new(tank("T"));
    let mover = g.create_object(&t, Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let _enemy = g.create_object(&t, Coord3D::new(215.0, 200.0, 0.0), 0.0, 1);
    g.submit(0, Command::MoveTo { ids: vec![mover], pos: Coord3D::new(800.0, 200.0, 0.0) });
    // First frame: the initial path.
    g.update();
    let after_initial = g.stats().pathfind_requests;
    // The blocked repath is anti-spun (`setQueueForPathTime`) but must arrive within a few seconds.
    g.run_frames(4 * LOGIC_FPS);
    assert!(g.stats().pathfind_requests > after_initial, "a repath was requested (was {after_initial}, now {})", g.stats().pathfind_requests);
}

/// Two identical synthetic runs agree frame by frame (the queue, collision scan and repath are
/// deterministic).
#[test]
fn movement_machinery_is_deterministic() {
    let run = || {
        let mut g = GameLogic::new(flat(), 7);
        g.add_player(0, 0);
        g.add_player(1, 0);
        let t = Arc::new(tank("T"));
        let ids: Vec<_> = (0..6)
            .map(|i| g.create_object(&t, Coord3D::new(100.0 + (i % 3) as f32 * 20.0, 300.0 + (i / 3) as f32 * 20.0, 0.0), 0.0, 0))
            .collect();
        let _enemy = g.create_object(&t, Coord3D::new(320.0, 310.0, 0.0), 0.0, 1);
        g.submit(0, Command::MoveTo { ids, pos: Coord3D::new(600.0, 320.0, 0.0) });
        g.run_frames(6 * LOGIC_FPS);
        g.crc()
    };
    assert_eq!(run(), run(), "two runs from the same setup must agree");
}
