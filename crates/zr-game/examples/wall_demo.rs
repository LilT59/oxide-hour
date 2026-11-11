//! Real-data wall layer (`LAYER_WALL`) and hierarchical path: loads `Iron Dragon` (116
//! `FortressWall*` pieces), reports the registered wall layer, walks a wall-layer path along the
//! wall, and reports a ground hierarchical-path corridor. Skips when no install is found.
//!
//! `cargo run --release -p zr-game --example wall_demo` (needs a Zero Hour install, or `ZR_ZEROHOUR_DIR`).

use std::sync::Arc;
use zr_assets::{Installs, Vfs};
use zr_game::{Game, GameData, PlayerSetup};
use zr_sim::pathfind::{surface, PathLayer, PathObject, Pathfinder};
use zr_sim::template::{Geometry as SimGeometry, GeometryShape};
use zr_sim::{Coord3D, ObjectId};
use zr_templates::{GeometryType, KindOf};

fn sim_geometry(t: &zr_templates::ThingTemplate) -> SimGeometry {
    SimGeometry {
        shape: match t.geometry.geometry_type {
            GeometryType::Sphere => GeometryShape::Sphere,
            GeometryType::Cylinder => GeometryShape::Cylinder,
            GeometryType::Box => GeometryShape::Box,
        },
        major_radius: t.geometry.major_radius,
        minor_radius: t.geometry.minor_radius,
        height: t.geometry.height,
        is_small: t.geometry.is_small,
    }
}

fn ground_obj(data: &GameData, name: &str) -> PathObject {
    let t = data.templates().thing(name).unwrap();
    PathObject {
        id: ObjectId::INVALID,
        surfaces: surface::GROUND,
        bounding_radius: sim_geometry(t).bounding_circle_radius(),
        layer: PathLayer::Ground,
        player: 0,
        infantry: false,
        crusher: false,
        ..Default::default()
    }
}

fn main() {
    let installs = Installs::detect();
    let Some(zh) = &installs.zero_hour else {
        eprintln!("no Zero Hour install found; set ZR_ZEROHOUR_DIR");
        return;
    };
    let vfs = Vfs::zero_hour(zh).expect("mounting Zero Hour");
    let data = Arc::new(GameData::load(&vfs));
    let wall_names: Vec<String> = data.templates().things().iter().filter(|t| t.is_kind_of(KindOf::WalkOnTopOfWall)).map(|t| t.name.clone()).collect();

    for map in ["Iron Dragon", "Fallen Empire"] {
        let setups = [PlayerSetup::new("FactionAmerica", 0), PlayerSetup::new("FactionChina", 1)];
        let g = match Game::new_skirmish_with(data.clone(), &vfs, map, &setups, 1) {
            Ok(g) => g,
            Err(e) => {
                println!("{map}: {e:#}");
                continue;
            }
        };
        let pf = g.logic().pathfinder();
        println!(
            "{map}: {} wall pieces, wall extent {:?}, wall height {:.1}",
            pf.wall_piece_count(),
            pf.wall_extent(),
            pf.wall_height()
        );
        if pf.wall_piece_count() == 0 {
            continue;
        }
        // Wall object id/name/position (id order is deterministic).
        let walls: Vec<(ObjectId, String, Coord3D, f32)> = g
            .objects()
            .filter(|o| wall_names.iter().any(|n| n == o.template))
            .map(|o| (o.id, o.template.to_string(), o.position, o.angle))
            .collect();
        let (_, first_name, first, _) = &walls[0];
        println!(
            "  first wall piece {first_name} at ({:.0}, {:.0}), on-wall {} (ground {:.1})",
            first.x,
            first.y,
            pf.is_point_on_wall(first),
            g.logic().terrain().ground_height(first.x, first.y)
        );

        // Walk on top of the wall: a wall-layer unit paths from the first piece to the last. Build a
        // fresh pathfinder and register every piece with its own template geometry (as `GameLogic`
        // does via `addWallPiece`).
        let wall_height = pf.wall_height();
        let mut wp = Pathfinder::new(g.logic().terrain());
        wp.set_wall_height(wall_height);
        for (id, name, p, angle) in &walls {
            if let Some(t) = data.templates().thing(name) {
                wp.add_wall_piece(*id, &sim_geometry(t), p, *angle);
            }
        }
        // Walk along one thick `Middle` piece: from 30 units before its centre to 30 units past,
        // along its orientation. The clear interior cells form a strip at the wall height.
        let mid = walls.iter().find(|w| w.1.contains("Middle")).unwrap_or(&walls[0]);
        let (mx, my, mang) = (mid.2.x, mid.2.y, mid.3);
        let start = Coord3D::new(mx - mang.cos() * 30.0, my - mang.sin() * 30.0, wall_height);
        let goal = Coord3D::new(mx + mang.cos() * 30.0, my + mang.sin() * 30.0, wall_height);
        let obj = {
            let t = data.templates().thing(first_name).unwrap();
            PathObject {
                id: ObjectId::INVALID,
                surfaces: surface::GROUND,
                bounding_radius: sim_geometry(t).bounding_circle_radius(),
                layer: PathLayer::Wall,
                player: 0,
                infantry: false,
                crusher: false,
                ..Default::default()
            }
        };
        let wps = wp.compute_move_path(g.logic().terrain(), &obj, &start, &goal);
        let on_wall = wps.iter().filter(|w| wp.is_point_on_wall(w)).count();
        println!(
            "  wall path: {} waypoints, {on_wall} on the wall, end ({:.0}, {:.0}) z {:.1}",
            wps.len(),
            wps.last().map_or(0.0, |w| w.x),
            wps.last().map_or(0.0, |w| w.y),
            wps.last().map_or(0.0, |w| w.z)
        );

        // Ground hierarchical path between the two player starts: the corridor blocks and the fine
        // path computed under the corridor gate.
        if let (Some(a), Some(b)) = (g.waypoint("Player_1_Start").map(|w| w.pos), g.waypoint("Player_2_Start").map(|w| w.pos)) {
            let mut hp = Pathfinder::new(g.logic().terrain());
            let route = hp.find_hierarchical_path(surface::GROUND, &a, &b);
            let n_blocks = route.as_ref().map_or(0, |r| r.len());
            let fine = hp.compute_move_path(g.logic().terrain(), &ground_obj(&data, "AmericaVehicleDozer"), &a, &b);
            println!(
                "  hierarchical corridor: {n_blocks} blocks, fine path {} waypoints, end {:?}",
                fine.len(),
                fine.last().map(|e| (e.x as i32, e.y as i32))
            );
        }
    }
}
