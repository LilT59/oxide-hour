//! Wall layer (`LAYER_WALL`) through `GameLogic`: `addWallPiece`/`removeWallPiece` registration and
//! wall-walking paths. Synthetic data only (`Pathfinder::addWallPiece`, `PathfindLayer` wall cells).

use std::sync::Arc;
use zr_sim::pathfind::{surface, PathLayer, PathObject, Pathfinder};
use zr_sim::template::{Geometry, GeometryShape};
use zr_sim::{Command, Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

fn flat() -> TerrainLogic {
    TerrainLogic::flat(100, 100, 0)
}

fn tank(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = zr_sim::template::kind_of::VEHICLE | zr_sim::template::kind_of::SELECTABLE;
    t.geometry.major_radius = 8.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.locomotor = Some(zr_sim::template::LocomotorParams::from_ini(zr_sim::template::LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    t
}

/// A `KINDOF_WALK_ON_TOP_OF_WALL` structure: a 200x40 box (four clear cell rows, so the inner two
/// stay walkable after the edge tightening).
fn wall(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = zr_sim::template::kind_of::STRUCTURE | zr_sim::template::kind_of::WALK_ON_TOP_OF_WALL;
    t.geometry = Geometry { shape: GeometryShape::Box, major_radius: 100.0, minor_radius: 20.0, height: 40.0, is_small: false };
    t
}

#[test]
fn game_logic_registers_and_removes_wall_pieces() {
    let mut g = GameLogic::new(flat(), 1);
    g.set_wall_height(43.0);
    let w = Arc::new(wall("Wall"));
    let id = g.create_object(&w, Coord3D::new(500.0, 500.0, 0.0), 0.0, 0);
    g.rebuild_wall_layer();
    assert_eq!(g.pathfinder().wall_piece_count(), 1);
    assert!(g.pathfinder().is_point_on_wall(&Coord3D::new(500.0, 500.0, 0.0)));
    assert!(g.pathfinder().wall_extent().is_some());
    // The wall piece is also a ground obstacle.
    assert_eq!(g.pathfinder().cell_type_at(&Coord3D::new(500.0, 500.0, 0.0)), Some(zr_sim::pathfind::CellType::Obstacle));

    // A ground unit cannot cross it.
    let t = Arc::new(tank("T"));
    let u = g.create_object(&t, Coord3D::new(500.0, 300.0, 0.0), 0.0, 0);
    g.submit(0, Command::MoveTo { ids: vec![u], pos: Coord3D::new(500.0, 700.0, 0.0) });
    g.run_frames(1);
    let path = g.find_object(u).unwrap().ai.path.clone();
    assert!(!path.is_empty());
    let mut prev = Coord3D::new(500.0, 300.0, 0.0);
    for w in &path {
        let steps = 40;
        for s in 0..=steps {
            let f = s as f32 / steps as f32;
            let (x, y) = (prev.x + (w.x - prev.x) * f, prev.y + (w.y - prev.y) * f);
            assert!(!(x > 410.0 && x < 590.0 && y > 480.0 && y < 520.0), "({x},{y}) inside the wall");
        }
        prev = *w;
    }

    // Destroying the wall removes the layer.
    g.destroy_object(id);
    g.run_frames(1);
    g.rebuild_wall_layer();
    assert_eq!(g.pathfinder().wall_piece_count(), 0);
    assert!(g.pathfinder().wall_extent().is_none());
    assert!(!g.pathfinder().is_point_on_wall(&Coord3D::new(500.0, 500.0, 0.0)));
}

#[test]
fn wall_layer_paths_along_the_wall_at_wall_height() {
    let t = flat();
    let mut pf = Pathfinder::new(&t);
    pf.set_wall_height(43.0);
    let geom = wall("Wall").geometry;
    pf.add_wall_piece(zr_sim::ObjectId(7), &geom, &Coord3D::new(500.0, 500.0, 0.0), 0.0);
    let obj = PathObject {
        id: zr_sim::ObjectId::INVALID,
        surfaces: surface::GROUND,
        bounding_radius: 6.0,
        layer: PathLayer::Wall,
        player: 0,
        infantry: false,
        crusher: false,
        ..Default::default()
    };
    let from = Coord3D::new(450.0, 500.0, 43.0);
    let to = Coord3D::new(550.0, 500.0, 43.0);
    let wps = pf.compute_move_path(&t, &obj, &from, &to);
    assert!(!wps.is_empty());
    assert!(wps.last().unwrap().dist_2d(&to) < 8.0, "{wps:?}");
    for w in &wps {
        assert!((w.z - 43.0).abs() < 0.01, "waypoint at the wall height: {w:?}");
        assert!(pf.is_point_on_wall(w), "waypoint stays on the wall: {w:?}");
    }
}
