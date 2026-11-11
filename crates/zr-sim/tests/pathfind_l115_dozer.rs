//! Lane 115: the dozer exception in `examineNeighboringCells` (`AIPathfind.cpp:6384-6399`).
//!
//! A pathfind that starts inside an obstacle is "tunneling" (`m_isTunneling`): until the search
//! reaches a cell the unit may stand on, every neighbour is costed with `costRemaining = 0` (the
//! engine's "find the closest valid cell" hack, `AIPathfind.cpp:6505-6507`). The dozer exception
//! makes a cell of an obstacle that is *not an enemy* of the dozer a valid movement position, so a
//! worker starting inside its own supply stash clears the tunneling state on its first obstacle
//! neighbour and the rest of the search is costed normally (`AIPathfind.cpp:6384-6399`,
//! `:6421-6424`). Without it the early search keeps its underestimates and the route differs.

use zr_sim::pathfind::{surface, PathObject, Pathfinder};
use zr_sim::template::{kind_of, Geometry, GeometryShape, ObjectTemplate};
use zr_sim::{Coord3D, ObjectId, TerrainLogic};

fn flat() -> TerrainLogic {
    TerrainLogic::flat(120, 120, 0)
}

/// A 10-unit bounding radius gives `radius_and_center` = (1, false): a 2×2 footprint, the shape the
/// GLA worker has (`getRadiusAndCenter`, `AIPathfind.cpp:9776`).
fn dozer(id: u32, player: u8) -> PathObject {
    PathObject {
        id: ObjectId(id),
        surfaces: surface::GROUND | surface::RUBBLE,
        bounding_radius: 10.0,
        layer: zr_sim::pathfind::PathLayer::Ground,
        player,
        infantry: true,
        crusher: false,
        dozer: true,
        ..Default::default()
    }
}

/// A structure footprint: `classify_object_footprint` marks its cells `CELL_OBSTACLE` with `id`.
fn stash(half_size: f32) -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Stash");
    t.kind_of = kind_of::STRUCTURE;
    t.geometry = Geometry { shape: GeometryShape::Box, major_radius: half_size, minor_radius: half_size, height: 20.0, is_small: false };
    t
}

/// The first waypoint of a path, in `(x*10, y*10)` cell-centre form.
fn first_waypoint(p: &zr_sim::pathfind::Path) -> (i32, i32) {
    let w = p.waypoints();
    let p = w.first().expect("a waypoint");
    ((p.x * 10.0).round() as i32, (p.y * 10.0).round() as i32)
}

/// A dozer standing inside a structure's footprint (`CELL_OBSTACLE` under the start cell) tunnels
/// out. The obstacle's owner decides whether the dozer exception applies:
/// * not an enemy (`Player::getRelationship(...) != ENEMIES`): the exception clears the tunneling
///   state on the first obstacle neighbour, so the search leaves the stash to the east (cell
///   (51,50) is the first neighbour in the engine's `delta[]` order, `AIPathfind.cpp:6315-6319`);
/// * an enemy: no exception, the tunneling underestimates stay and the route leaves to the south.
#[test]
fn a_dozers_own_stash_clears_the_tunneling_state_but_an_enemy_one_does_not() {
    let t = flat();
    let mut pf = Pathfinder::new(&t);
    let from = Coord3D::new(500.0, 500.0, 0.0);
    let to = Coord3D::new(900.0, 800.0, 0.0);
    // The stash covers cells (49..50, 49..50): exactly the dozer's own 2×2 footprint, so the search
    // starts inside an obstacle (`m_isTunneling`).
    pf.classify_object_footprint(&t, ObjectId(7), &stash(10.0), &from, 0.0, true);
    // `internalFindPath`'s zone screen (`AIPathfind.cpp:6684-6715`) rejects an obstacle start whose
    // cells a just-finished classification left at `UNINITIALIZED_ZONE` (the terrain remap of zone 0 is
    // still 0), so the cells need zones: a `calculateZones` like the one the engine runs between
    // classifications (`Pathfinder::calculateZones`, `AIPathfind.cpp:5859`) makes the start's zone
    // equal the goal's through the terrain table, which is what lets the search run.
    pf.calculate_zones();
    let mover = dozer(238, 0);
    let mut a = [0u32; 32];
    a[0] |= 1 << 0; // player 0 is allied with itself
    pf.set_allies(a);
    pf.update_unit_pos(&mover, &from, false);

    // The stash belongs to the dozer's own player: `getRelationship` is ALLIES, not ENEMIES.
    pf.set_enemies([0u32; 32]);
    pf.set_obstacle_owners([(7u32, 0u8)]);
    let own = pf.find_path(&t, &mover, &from, &to).expect("a path over the own stash");
    assert_eq!(first_waypoint(&own), (5305, 5005), "the own stash's cells are valid for a dozer, so the search runs normally");

    // The same cells, but owned by a player the dozer is at war with: `getRelationship == ENEMIES`.
    let mut enemies = [0u32; 32];
    enemies[0] |= 1 << 1; // the dozer's player (0) is at war with the stash's owner (1)
    pf.set_enemies(enemies);
    pf.set_obstacle_owners([(7u32, 1u8)]);
    let enemy = pf.find_path(&t, &mover, &from, &to).expect("a path out of the enemy stash");
    assert_eq!(first_waypoint(&enemy), (5005, 5305), "an enemy obstacle gets no exception, so the tunneling route stays");

    assert_ne!(own.waypoints(), enemy.waypoints(), "the dozer exception must change the route");
}
