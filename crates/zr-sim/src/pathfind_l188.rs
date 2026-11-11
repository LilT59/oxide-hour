//! Lane 188 (AN5 s1 crawler 263, f3138): `Weapon::isGoalPosWithinAttackRange` (`Weapon.cpp:2357-2400`) undersizes the
//! weapon's range by a quarter cell again on top of `getAttackRange` (`Weapon.cpp:2364`), and oversizes the minimum
//! by a quarter cell (`:2394`). The port's `goal_in_range` used the envelope as given, so a goal 171.3 from the victim
//! was accepted where C++ (limit 170) rejects it and finds the goal at 21 units.

use super::*;
use crate::los::AttackView;

fn flat() -> TerrainLogic {
    TerrainLogic::from_samples(62, 62, 0, vec![0u8; 62 * 62])
}

fn ground(id: u32, radius: f32) -> PathObject {
    PathObject { id: ObjectId(id), surfaces: surface::GROUND, bounding_radius: radius, layer: PathLayer::Ground, player: 0, infantry: false, crusher: false, ..Default::default() }
}

/// The goal test with no radii: `d2` is the squared distance from the victim's centre. The envelope is
/// `getAttackRange` (175 - 2.5 = 172.5), and the goal limit is 172.5 - 2.5 = 170 (`Weapon.cpp:2364`). Revert check:
/// `goal_in_range` as `d2 <= attack_range^2` accepts 171.3.
#[test]
fn the_goal_range_is_undersized_by_a_quarter_cell_again() {
    let t = flat();
    let pf = Pathfinder::new(&t);
    let victim = Coord3D::new(400.0, 500.0, 0.0);
    let at = |d: f32| Coord3D::new(400.0 - d, 500.0, 0.0);
    assert!(pf.goal_in_range(&at(169.9), 0.0, &victim, 0.0, 172.5, 0.0), "169.9 is inside 170");
    assert!(!pf.goal_in_range(&at(171.3), 0.0, &victim, 0.0, 172.5, 0.0), "171.3 is outside 170 (C++ limit)");
}

/// `getMinimumAttackRange() + 2.5` (`Weapon.cpp:2394`), and no `-0.5` under `RATIONALIZE_ATTACK_RANGE` (`:2396-2400`).
/// With a minimum of 0 a goal closer than 2.5 to the boundary is rejected. Revert check: `d2 >= min_range^2` accepts 2.0.
#[test]
fn the_goal_minimum_is_oversized_by_a_quarter_cell() {
    let t = flat();
    let pf = Pathfinder::new(&t);
    let victim = Coord3D::new(400.0, 500.0, 0.0);
    let at = |d: f32| Coord3D::new(400.0 - d, 500.0, 0.0);
    assert!(!pf.goal_in_range(&at(2.0), 0.0, &victim, 0.0, 175.0, 0.0), "2.0 is under the 2.5 minimum");
    assert!(pf.goal_in_range(&at(3.0), 0.0, &victim, 0.0, 175.0, 0.0), "3.0 is over it");
}

/// A unit that stands still: its position cell is also its goal, so its flags are `UNIT_PRESENT_FIXED`
/// (`PathfindCell::setGoalUnit`, `AIPathfind.cpp:1420-1455`), which the strip-back's `checkForMovement` counts.
fn fixed_unit(pf: &mut Pathfinder, obj: &PathObject, at: &Coord3D) {
    pf.update_unit_pos(obj, at, false);
    pf.update_goal(obj, at);
}

/// The chain of parents the strip-back walks: (20,50) is the start, (21,50) its child, (22,50) the accepted cell.
fn chain(pf: &mut Pathfinder) -> (usize, usize, usize) {
    let (c0, c1, c2) = (pf.idx(20, 50), pf.idx(21, 50), pf.idx(22, 50));
    for c in [c0, c1, c2] {
        pf.allocate_info(c);
    }
    pf.info[c1].parent = c0 as u32;
    pf.info[c2].parent = c1 as u32;
    (c0, c1, c2)
}

/// `findAttackPath`'s strip-back (`AIPathfind.cpp:10919-10972`): a fixed enemy on (21,50) blocks the path there, so
/// the vehicle's path ends at the parent of that cell, (20,50). Revert check: make `blocked` false (the helper
/// returns the accepted cell (22,50)).
#[test]
fn a_vehicle_strips_back_from_a_fixed_enemy_on_its_path() {
    let t = flat();
    let mut pf = Pathfinder::new(&t);
    let (c0, _c1, c2) = chain(&mut pf);
    let enemy = PathObject { player: 1, ..ground(9, 0.5) };
    let at = Coord3D::new(210.0, 500.0, 0.0);
    fixed_unit(&mut pf, &enemy, &at);
    let attacker = ground(1, 8.0);
    let (r, center) = attacker.radius_and_center();
    assert_eq!(pf.attack_strip_back(&attacker, c2, r, center), c0, "the enemy on (21,50) ends the path at (20,50)");
}

/// The `unitIdle` exemption (`AIPathfind.cpp:10948-10949`): a fixed ally on the path blocks the strip-back, but an
/// idle one does not. Revert check: drop the `!unit_idle &&` term and the idle case strips back to (20,50).
#[test]
fn a_vehicle_strip_back_passes_an_idle_ally() {
    let t = flat();
    let mut pf = Pathfinder::new(&t);
    let (c0, _c1, c2) = chain(&mut pf);
    let ally = ground(9, 0.5);
    fixed_unit(&mut pf, &ally, &Coord3D::new(210.0, 500.0, 0.0));
    let attacker = ground(1, 8.0);
    let (r, center) = attacker.radius_and_center();
    assert_eq!(pf.attack_strip_back(&attacker, c2, r, center), c0, "a busy fixed ally blocks");
    pf.set_unit_kinds([], [9]);
    assert_eq!(pf.attack_strip_back(&attacker, c2, r, center), c2, "an idle ally does not block");
}

/// `findAttackPath` allocates at most `ATTACK_CELL_LIMIT` (2500) new cells before it stops expanding
/// (`AIPathfind.cpp:10765`, `:11014-11018`; `cellCount` counts `allocateInfo` in `examineNeighboringCells`,
/// `:6344-6349`). An attacker 800 units from a victim behind a short wall needs between 5000 and 10000 new cells
/// (measured by lane 188 with the limit raised), so with the limit the search fails. Revert check: a limit of 10000
/// finds the path, and this test fails.
#[test]
fn the_attack_search_gives_up_after_2500_new_cells() {
    // 120 x 120 cells: the victim at x 900 must be on the grid (the helper `flat` is 62 x 62).
    let t = TerrainLogic::flat(120, 120, 0);
    let mut pf = Pathfinder::new(&t);
    let attacker = ground(1, 8.0);
    let mut wall = ObjectTemplate::new("Wall");
    wall.kind_of = kind_of::STRUCTURE;
    wall.geometry = Geometry { shape: GeometryShape::Box, major_radius: 20.0, minor_radius: 60.0, height: 30.0, is_small: false };
    pf.classify_object_footprint(&t, ObjectId(7), &wall, &Coord3D::new(500.0, 500.0, 0.0), 0.0, true);
    let from = Coord3D::new(100.0, 500.0, 0.0);
    let victim = Coord3D::new(900.0, 500.0, 0.0);
    let v = AttackView { attacker: attacker.id, attacker_container: None, attacker_slaver: None, victim: None, terrain_test: false, attacker_eye_height: 0.0 };
    let path = pf.find_attack_path(&t, &attacker, &from, &victim, Some(&v), 200.0, 0.0, 8.0, 8.0, true);
    assert!(path.is_none(), "the search exceeds 2500 new cells and gives up");
}

/// The quick check (`AIPathfind.cpp:10730-10762`) steps 5 units a time toward the victim. From 201 away, the sixth step
/// is 171 away: the C++ goal limit (170) rejects it, so the seventh step (166) is the quick path's end. Revert check: the
/// old envelope accepts the sixth step (x 229) and the quick path ends there.
#[test]
fn the_quick_check_stops_at_the_undersized_goal_range() {
    let t = flat();
    let mut pf = Pathfinder::new(&t);
    let attacker = ground(1, 8.0);
    let from = Coord3D::new(199.0, 500.0, 0.0);
    let victim = Coord3D::new(400.0, 500.0, 0.0);
    let path = pf.find_attack_path(&t, &attacker, &from, &victim, None::<&AttackView>, 172.5, 0.0, 0.0, 0.0, true).expect("a quick attack path");
    assert_eq!(path.nodes.len(), 2, "the quick check's two nodes: {:?}", path.nodes);
    assert!((path.nodes[1].pos.x - 234.0).abs() < 0.01, "the seventh step: {:?}", path.nodes[1].pos);
}
