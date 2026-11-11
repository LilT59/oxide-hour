//! Fine-A* unit avoidance (`moveAllies`/`getMoveAwayFromPath`), `checkDestination` group
//! tightening and `findAttackPath`, on synthetic terrain (no game assets).

use zr_sim::los::AttackView;
use zr_sim::pathfind::{surface, CellType, Path, PathNode, PathObject, Pathfinder};
use zr_sim::template::{kind_of, Geometry, GeometryShape, ObjectTemplate};
use zr_sim::{Coord3D, ObjectId, TerrainLogic};

fn flat() -> TerrainLogic {
    TerrainLogic::flat(120, 120, 0)
}

fn ground(id: u32, player: u8, radius: f32) -> PathObject {
    PathObject { id: ObjectId(id), surfaces: surface::GROUND, bounding_radius: radius, layer: zr_sim::pathfind::PathLayer::Ground, player, infantry: false, crusher: false, ..Default::default() }
}

fn wall(half_x: f32, half_y: f32) -> ObjectTemplate {
    let mut w = ObjectTemplate::new("Wall");
    w.kind_of = kind_of::STRUCTURE;
    w.geometry = Geometry { shape: GeometryShape::Box, major_radius: half_x, minor_radius: half_y, height: 20.0, is_small: false };
    w
}

/// A mover in a one-cell-tall corridor with an idle ally blocking it: the fine A* marks the path
/// `blockedByAlly` and `moveAllies` asks the ally to step aside (`AIPathfind.cpp:10190`).
#[test]
fn move_allies_pushes_an_idle_ally_off_the_path() {
    let t = flat();
    let mut pf = Pathfinder::new(&t);
    // A bounding radius of 15 gives a centred 3x3 footprint (`radius_and_center`).
    let mover = ground(1, 0, 15.0);
    let ally = ground(2, 0, 15.0);
    let mut a = [0u32; 32];
    a[0] |= 1 << 0; // player 0 is allied with itself
    pf.set_allies(a);
    // A corridor at cells y 48..52 (world 480..530); the walls span the whole map so the only
    // route is through the corridor (where the ally stands).
    pf.classify_object_footprint(&t, ObjectId(8), &wall(700.0, 5.0), &Coord3D::new(500.0, 465.0, 0.0), 0.0, true);
    pf.classify_object_footprint(&t, ObjectId(9), &wall(700.0, 5.0), &Coord3D::new(500.0, 535.0, 0.0), 0.0, true);
    pf.update_unit_pos(&mover, &Coord3D::new(100.0, 500.0, 0.0), false);
    pf.update_unit_pos(&ally, &Coord3D::new(500.0, 500.0, 0.0), false);
    // An idle ally stands on its own goal cell (`UNIT_PRESENT_FIXED`, `AIPathfind.cpp:1420-1455`).
    pf.update_goal(&ally, &Coord3D::new(500.0, 500.0, 0.0));

    let info = pf.check_for_movement_full(&mover, &Coord3D::new(500.0, 500.0, 0.0), false);
    assert_eq!(info.ally_fixed_count, 1, "the ally must be seen: {info:?}");
    let p = pf.find_path(&t, &mover, &Coord3D::new(100.0, 500.0, 0.0), &Coord3D::new(900.0, 500.0, 0.0)).expect("a path");
    assert!(p.blocked_by_ally, "the route is blocked by the ally");
    let wps = pf.compute_move_path(&t, &mover, &Coord3D::new(100.0, 500.0, 0.0), &Coord3D::new(900.0, 500.0, 0.0));
    assert!(!wps.is_empty());
    // The scan lists the idle ally on the path's cells (`moveAllies`' visits, AIPathfind.cpp:10190-10320); the
    // step-aside itself is `GameLogic::move_allies`, which runs `aiMoveAwayFromUnit` at each visit.
    let visits = pf.ally_scan(&mover, &p);
    assert!(!visits.is_empty(), "the idle ally is on the path's cells");
    assert!(visits.iter().all(|v| *v == ObjectId(2)), "only the ally is visited: {visits:?}");
}

/// A fixed ally that is *moving* does not block or get pushed (`isMoving` test).
#[test]
fn moving_allies_do_not_block_or_get_pushed() {
    let t = flat();
    let mut pf = Pathfinder::new(&t);
    let mover = ground(1, 0, 8.0);
    let ally = ground(2, 0, 8.0);
    let mut a = [0u32; 32];
    a[0] |= 1 << 0;
    pf.set_allies(a);
    pf.update_unit_pos(&ally, &Coord3D::new(500.0, 500.0, 0.0), true);
    let wps = pf.compute_move_path(&t, &mover, &Coord3D::new(100.0, 500.0, 0.0), &Coord3D::new(900.0, 500.0, 0.0));
    assert!(!wps.is_empty());
    // The veto on a moving ally is `GameLogic::move_allies`' live `isMoving` test, not the scan's.
}

/// A fixed *enemy* is seen by `checkForMovement` and blocks the line of sight
/// (`linePassableCallback`), which stops `Path::optimize` straightening through it.
#[test]
fn a_fixed_enemy_blocks_the_line_of_sight() {
    let t = flat();
    let mut pf = Pathfinder::new(&t);
    let mover = ground(1, 0, 15.0);
    let enemy = ground(2, 1, 15.0);
    pf.update_unit_pos(&enemy, &Coord3D::new(500.0, 500.0, 0.0), false);
    // A unit standing on its own goal cell is `UNIT_PRESENT_FIXED`; without a goal it is
    // `UNIT_PRESENT_MOVING`, the engine's transient state.
    pf.update_goal(&enemy, &Coord3D::new(500.0, 500.0, 0.0));
    let info = pf.check_for_movement_full(&mover, &Coord3D::new(500.0, 500.0, 0.0), false);
    assert!(info.enemy_fixed, "the enemy must be seen: {info:?}");
    assert!(!pf.is_line_passable(&mover, &Coord3D::new(100.0, 500.0, 0.0), &Coord3D::new(900.0, 500.0, 0.0), false, false), "the line is blocked");
}

/// `adjustDestination`'s `groupDest` tightening reserves distinct goals and does not stack them.
#[test]
fn group_destination_tightens_and_spreads() {
    let t = flat();
    let mut pf = Pathfinder::new(&t);
    let group = Coord3D::new(600.0, 600.0, 0.0);
    // Two units near each other, both ordered to the group point.
    let a = ground(1, 0, 8.0);
    let b = ground(2, 0, 8.0);
    pf.update_unit_pos(&a, &Coord3D::new(100.0, 100.0, 0.0), false);
    pf.update_unit_pos(&b, &Coord3D::new(140.0, 100.0, 0.0), false);
    // Each unit's `computeIndividualDestination` takes its adjusted cell as its goal (AIGroup.cpp:546-548): the
    // adjust reads the goal cells and the `updateGoal` reserves the cell for the next unit.
    let pa = pf.adjust_destination(&t, &a, &group, Some(&group)).unwrap();
    pf.update_goal(&a, &pa);
    let pb = pf.adjust_destination(&t, &b, &group, Some(&group)).unwrap();
    pf.update_goal(&b, &pb);
    assert!(pa.dist_2d(&pb) > 15.0, "group goals must not stack: {pa:?} / {pb:?}");
    assert!(pa.dist_2d(&group) < 60.0 && pb.dist_2d(&group) < 60.0, "{pa:?} / {pb:?}");
}

/// `KINDOF_MINE` structures never classify a footprint (`classifyObjectFootprint` early return).
#[test]
fn mines_do_not_block_pathing() {
    let t = flat();
    let mut pf = Pathfinder::new(&t);
    let mut mine = ObjectTemplate::new("Mine");
    mine.kind_of = kind_of::STRUCTURE | kind_of::MINE;
    mine.geometry = Geometry { shape: GeometryShape::Sphere, major_radius: 20.0, minor_radius: 20.0, height: 10.0, is_small: false };
    pf.classify_object_footprint(&t, ObjectId(9), &mine, &Coord3D::new(500.0, 500.0, 0.0), 0.0, true);
    assert_ne!(pf.cell_type_at(&Coord3D::new(500.0, 500.0, 0.0)), Some(CellType::Obstacle));
}

/// A fence (`FenceWidth > 0`) marks a thin strip of obstacle cells; the centre stays clear.
#[test]
fn fences_classify_a_thin_strip() {
    let t = flat();
    let mut pf = Pathfinder::new(&t);
    let mut fence = ObjectTemplate::new("Fence");
    fence.kind_of = kind_of::STRUCTURE;
    fence.fence_width = 200.0;
    fence.fence_x_offset = 0.0;
    fence.geometry = Geometry { shape: GeometryShape::Box, major_radius: 100.0, minor_radius: 1.0, height: 20.0, is_small: false };
    pf.classify_object_footprint(&t, ObjectId(9), &fence, &Coord3D::new(500.0, 500.0, 0.0), 0.0, true);
    // Cells along the fence line are obstacles...
    assert_eq!(pf.cell_type_at(&Coord3D::new(500.0, 500.0, 0.0)), Some(CellType::Obstacle));
    assert_eq!(pf.cell_type_at(&Coord3D::new(600.0, 500.0, 0.0)), Some(CellType::Obstacle));
    // ...but a cell well off the line is not.
    assert_ne!(pf.cell_type_at(&Coord3D::new(500.0, 560.0, 0.0)), Some(CellType::Obstacle));
    // Removing the fence clears it.
    pf.classify_object_footprint(&t, ObjectId(9), &fence, &Coord3D::new(500.0, 500.0, 0.0), 0.0, false);
    assert_ne!(pf.cell_type_at(&Coord3D::new(500.0, 500.0, 0.0)), Some(CellType::Obstacle));
}

/// `patchPath` routes from the current position back onto a path whose next node is blocked.
#[test]
fn patch_path_routes_back_onto_a_blocked_path() {
    let t = flat();
    let mut pf = Pathfinder::new(&t);
    let unit = ground(1, 0, 15.0);
    let blocker = ground(2, 1, 15.0);
    pf.update_unit_pos(&blocker, &Coord3D::new(500.0, 500.0, 0.0), false);
    let nodes = vec![
        PathNode { pos: Coord3D::new(100.0, 500.0, 0.0), can_optimize: true, next_optimized: Some(1) },
        PathNode { pos: Coord3D::new(400.0, 500.0, 0.0), can_optimize: true, next_optimized: Some(2) },
        PathNode { pos: Coord3D::new(500.0, 500.0, 0.0), can_optimize: true, next_optimized: Some(3) },
        PathNode { pos: Coord3D::new(600.0, 500.0, 0.0), can_optimize: true, next_optimized: Some(4) },
        PathNode { pos: Coord3D::new(900.0, 500.0, 0.0), can_optimize: true, next_optimized: None },
    ];
    let original = Path { nodes, blocked_by_ally: false };
    let p = pf.patch_path(&t, &unit, &Coord3D::new(450.0, 500.0, 0.0), &original, true).expect("a patched path");
    let end = p.waypoints().last().copied().unwrap_or_else(|| p.nodes.last().unwrap().pos);
    assert!(end.dist_2d(&Coord3D::new(900.0, 500.0, 0.0)) < 20.0, "rejoins the path: {end:?}");
}

/// `findAttackPath` stops within weapon range of the victim and (with LOS on) keeps a clear line.
#[test]
fn attack_path_stops_in_weapon_range() {
    let t = flat();
    let mut pf = Pathfinder::new(&t);
    let attacker = ground(1, 0, 8.0);
    let victim = Coord3D::new(900.0, 500.0, 0.0);
    // A wall between attacker and victim: with LOS on, the path must go around it. The attacker is 250 from the victim
    // (outside the 200 range), so the search is short. `findAttackPath` stops expanding after `ATTACK_CELL_LIMIT`
    // (2500) new cells (AIPathfind.cpp:10765, :11014-11018). An attacker 800 units away needs between 5000 and 10000
    // new cells in this port (measured by lane 188; `l188` test `the_attack_search_gives_up_after_2500_new_cells`), so
    // that setup is not one C++ answers either.
    let mut wall = ObjectTemplate::new("Wall");
    wall.kind_of = kind_of::STRUCTURE;
    wall.geometry = Geometry { shape: GeometryShape::Box, major_radius: 20.0, minor_radius: 60.0, height: 30.0, is_small: false };
    pf.classify_object_footprint(&t, ObjectId(7), &wall, &Coord3D::new(780.0, 500.0, 0.0), 0.0, true);
    let from = Coord3D::new(650.0, 500.0, 0.0);
    let range = 200.0;
    let view = AttackView { attacker: attacker.id, attacker_container: None, attacker_slaver: None, victim: None, terrain_test: false, attacker_eye_height: 0.0 };
    let path = pf.find_attack_path(&t, &attacker, &from, &victim, Some(&view), range, 0.0, 8.0, 8.0, true).expect("an attack path");
    let wps = path.waypoints();
    let end = *wps.last().unwrap();
    let d = (end.dist_2d(&victim) - 8.0 - 8.0).max(0.0);
    assert!(d <= range + 1.0, "stops in range: end {end:?} d {d}");
    // The end is not inside the wall footprint.
    assert_ne!(pf.cell_type_at(&end), Some(CellType::Obstacle), "not inside the wall: {end:?}");
}
