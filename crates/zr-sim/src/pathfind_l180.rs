//! Lane 180 (R21-5, item 2): the shared attack view inside the pathfinder. `isAttackViewBlockedByObstacle`
//! (`AIPathfind.cpp:9459-9536`) runs for `findAttackPath` through the same `attack_view_blocked` as the line-of-sight
//! filter. The quick check (`:10750`) tests the view whatever `checkLOS` says; `checkLOS` (`:10844-10850`) gates
//! only the search loop (`:10869`).

use super::*;
use crate::los::{AttackView, VictimView};

fn flat() -> TerrainLogic {
    TerrainLogic::from_samples(62, 62, 0, vec![0u8; 62 * 62])
}

/// A view with no object behind it: a position target, nothing exempt, no terrain test.
fn view(attacker: u32) -> AttackView {
    AttackView { attacker: ObjectId(attacker), attacker_container: None, attacker_slaver: None, victim: None, terrain_test: false, attacker_eye_height: 0.0 }
}

fn ground(id: u32, radius: f32) -> PathObject {
    PathObject { id: ObjectId(id), surfaces: surface::GROUND, bounding_radius: radius, layer: PathLayer::Ground, player: 0, infantry: false, crusher: false, ..Default::default() }
}

/// Marks cell (x, y) as an obstacle owned by `id`.
fn set_obstacle(pf: &mut Pathfinder, x: i32, y: i32, id: u32, transparent: bool) {
    let ci = pf.idx(x, y);
    pf.cells[ci].ty = CellType::Obstacle;
    pf.cells[ci].obstacle = id;
    pf.cells[ci].obstacle_transparent = transparent;
}

/// Cells x 15..=19 of row 50 (world x 150..200, y 500): a wall across the line from x 100 to x 250.
fn wall_row(pf: &mut Pathfinder, transparent: bool) {
    for x in 15..=19 {
        set_obstacle(pf, x, 50, 7, transparent);
    }
}

/// A line along row 15 (world y 150) from x 35 to x 75 crosses cells (3,1)..(7,1). An obstacle at (4,1) is on it.
fn row_line() -> (Coord3D, Coord3D) {
    (Coord3D::new(35.0, 15.0, 0.0), Coord3D::new(75.0, 15.0, 0.0))
}

/// Off the grid the victim has no victim cell (`getCell(layer, &victimPos)` is null, `AIPathfind.h:479-481`), so
/// the obstacle the clamped edge cell holds is not exempt (`AIPathfind.cpp:9519`, `:9423`).
/// Revert check: `los_cell_obstacle_at` without the overflow test returns obstacle 9 and the view is clear.
#[test]
fn an_off_grid_victim_has_no_victim_cell() {
    let t = flat();
    let mut pf = Pathfinder::new(&t);
    set_obstacle(&mut pf, 0, 0, 9, false);
    assert_eq!(pf.los_cell_obstacle_at(&Coord3D::new(-20.0, 5.0, 0.0)), None, "off the grid: getCell is null");
    assert_eq!(pf.los_cell_obstacle_at(&Coord3D::new(5.0, 5.0, 0.0)), Some(9), "on the grid: the cell's obstacle");
    // The walk from (3,1) reaches the clamped edge cell (0,0), whose obstacle 9 is not the victim's.
    assert!(pf.attack_view_blocked(&t, &view(1), &Coord3D::new(35.0, 15.0, 0.0), &Coord3D::new(-20.0, 5.0, 0.0)));
}

/// The attacker's own container is not an obstacle to it (`AIPathfind.cpp:9409`). Revert check: remove the
/// `attacker_container` line in `attack_view_blocked` and the bunker blocks the unit inside it.
#[test]
fn the_attackers_container_does_not_block_its_view() {
    let t = flat();
    let mut pf = Pathfinder::new(&t);
    set_obstacle(&mut pf, 4, 1, 9, false);
    let (from, to) = row_line();
    assert!(pf.attack_view_blocked(&t, &view(1), &from, &to), "a bunker on the line blocks a unit outside it");
    let inside = AttackView { attacker_container: Some(9), ..view(1) };
    assert!(!pf.attack_view_blocked(&t, &inside, &from, &to), "the attacker's container is exempt");
}

/// The victim object and its slaver are not obstacles to the attack (`AIPathfind.cpp:9399-9406`). Revert check:
/// remove the victim-slaver test and the second case blocks.
#[test]
fn the_victim_and_its_slaver_do_not_block_the_attack() {
    let t = flat();
    let mut pf = Pathfinder::new(&t);
    set_obstacle(&mut pf, 4, 1, 9, false);
    let (from, to) = row_line();
    let as_victim = |id: u32, slaver: Option<u32>| AttackView { victim: Some(VictimView { id: ObjectId(id), slaver, center: to }), ..view(1) };
    assert!(!pf.attack_view_blocked(&t, &as_victim(9, None), &from, &to), "the victim itself is exempt");
    assert!(!pf.attack_view_blocked(&t, &as_victim(5, Some(9)), &from, &to), "the victim's slaver is exempt");
    assert!(pf.attack_view_blocked(&t, &as_victim(5, None), &from, &to), "an unrelated obstacle blocks");
}

/// `findAttackPath`'s quick check (`AIPathfind.cpp:10730-10762`) takes the first step whose view is clear. A
/// transparent obstacle does not block (`attackBlockedByObstacleCallback`, `:9418`), so the step at 5 units from
/// the attacker is accepted: the two-node quick path. Revert check: without the transparent rule the quick check
/// rejects every step and the path is not the quick path.
#[test]
fn a_transparent_obstacle_does_not_block_the_quick_attack_path() {
    let t = flat();
    let mut pf = Pathfinder::new(&t);
    wall_row(&mut pf, true);
    let attacker = ground(1, 8.0);
    let from = Coord3D::new(100.0, 500.0, 0.0);
    let victim = Coord3D::new(250.0, 500.0, 0.0);
    let v = AttackView { victim: Some(VictimView { id: ObjectId(2), slaver: None, center: victim }), ..view(1) };
    let path = pf.find_attack_path(&t, &attacker, &from, &victim, Some(&v), 150.0, 0.0, 8.0, 8.0, true).expect("an attack path");
    assert_eq!(path.nodes.len(), 2, "the quick check's two nodes: {:?}", path.nodes);
    assert!((path.nodes[1].pos.x - 105.0).abs() < 0.01, "one step toward the victim: {:?}", path.nodes[1].pos);
}

/// The quick check tests the view whatever `checkLOS` says (`AIPathfind.cpp:10750` has no `checkLOS` gate), so an
/// opaque wall stops the quick path even for a victim significantly above the terrain, where `checkLOS` is off
/// (`:10848`). Revert check: gate the quick check by `check_los` and the two-node quick path is returned.
#[test]
fn the_quick_check_tests_the_view_even_when_check_los_is_off() {
    let t = flat();
    let mut pf = Pathfinder::new(&t);
    wall_row(&mut pf, false);
    let attacker = ground(1, 8.0);
    let from = Coord3D::new(100.0, 500.0, 0.0);
    let victim = Coord3D::new(250.0, 500.0, 0.0);
    let v = AttackView { victim: Some(VictimView { id: ObjectId(2), slaver: None, center: victim }), ..view(1) };
    let path = pf.find_attack_path(&t, &attacker, &from, &victim, Some(&v), 150.0, 0.0, 8.0, 8.0, false);
    let quick = path.as_ref().is_some_and(|p| p.nodes.len() == 2 && (p.nodes[1].pos.x - 105.0).abs() < 0.01);
    assert!(!quick, "the opaque wall blocks the quick check whatever checkLOS says: {:?}", path.map(|p| p.nodes.len()));
}
