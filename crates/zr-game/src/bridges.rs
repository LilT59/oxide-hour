//! Bridge rectangles for the simulation (`TerrainLogic::addBridgeToLogic`).
//!
//! Ported from `Core/.../Map/TerrainLogic.cpp` (`Bridge::Bridge(BridgeInfo&)`,
//! `Bridge::Bridge(Object*)`, `addBridgeToLogic`) and
//! `GameEngineDevice/.../W3DBridgeBuffer.cpp` (`W3DBridge::getBridgeInfo`, `loadBridges`). Two
//! sources, mirroring the engine (the client builds the geometry, then hands it to `TerrainLogic`):
//!
//! * `FLAG_BRIDGE_POINT1` / `FLAG_BRIDGE_POINT2` pairs name an INI `Bridge` template; its model
//!   sets the rectangle width. [`zr_roads::build_bridges`] places that geometry, and the width is
//!   measured from its vertices (the engine's `(m_maxY - m_minY) * m_scale`).
//! * `IsBridge` map objects are boxes; `Bridge::Bridge(Object*)` uses the box's major/minor radii.

use std::collections::BTreeMap;

use zr_assets::Vfs;
use zr_ini::IniDb;
use zr_map::MapFile;
use zr_sim::bridge::Bridge;
use zr_sim::{Coord3D, ObjectId};
use zr_templates::GeometryType;

use crate::data::GameData;

/// First non-ground `PathfindLayer` (`LAYER_GROUND + 1`).
pub const FIRST_BRIDGE_LAYER: u32 = 2;

/// Bridge rectangles from the map's `IsBridge` objects (the object-transform formula). Their
/// `layer` is provisional; [`crate::game`] reassigns layers after appending the point bridges.
pub fn object_bridges(data: &GameData, map: &MapFile, ground: &dyn Fn(f32, f32) -> f32) -> Vec<Bridge> {
    let mut out = Vec::new();
    for o in &map.objects {
        if o.props.str("waypointName").is_some() || o.flags & zr_roads::SEGMENT_POINT_FLAGS != 0 {
            continue;
        }
        let Some(t) = data.templates().thing(&o.template) else { continue };
        if !t.is_bridge || t.geometry.geometry_type != GeometryType::Box {
            continue;
        }
        let layer = FIRST_BRIDGE_LAYER + out.len() as u32;
        let pos = Coord3D::new(o.pos[0], o.pos[1], o.pos[2] + ground(o.pos[0], o.pos[1]));
        out.push(Bridge::from_object(pos, o.angle, t.geometry.major_radius, t.geometry.minor_radius, layer, ObjectId::INVALID));
    }
    out
}

/// Bridge rectangles from `FLAG_BRIDGE_POINT1/2` pairs. The width is measured from the placed
/// model geometry (matching `W3DBridge::getBridgeInfo`); bridges whose model is missing are
/// skipped, exactly as the engine skips them.
pub fn point_bridges(map: &MapFile, vfs: &Vfs, ini: &IniDb, ground: &dyn Fn(f32, f32) -> f32) -> Vec<Bridge> {
    let Ok(instances) = zr_roads::build_bridges(map, vfs, ini, ground) else { return Vec::new() };
    let mut out = Vec::new();
    for b in &instances {
        let (dx, dy) = (b.to[0] - b.from[0], b.to[1] - b.from[1]);
        let len = (dx * dx + dy * dy).sqrt();
        let (nx, ny) = if len > 1e-3 { (-dy / len, dx / len) } else { (0.0, 1.0) };
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for p in &b.pieces {
            for v in &p.positions {
                let t = (v[0] - b.from[0]) * nx + (v[1] - b.from[1]) * ny;
                lo = lo.min(t);
                hi = hi.max(t);
            }
        }
        let width = hi - lo;
        if width <= 0.0 {
            continue;
        }
        let from = Coord3D::new(b.from[0], b.from[1], b.from[2]);
        let to = Coord3D::new(b.to[0], b.to[1], b.to[2]);
        out.push(Bridge::new(from, to, width, FIRST_BRIDGE_LAYER + out.len() as u32));
    }
    out
}

/// `TerrainRoads` `TowerObjectName{FromLeft,FromRight,ToLeft,ToRight}` per `Bridge` template
/// (`Bridge::Bridge(Object*)` looks each corner up on its own, `TerrainLogic.cpp:388-394`). A
/// template that names only some corners gets `None` for the others; a `Bridge` template with no
/// tower names at all is absent from the map.
pub fn tower_names(ini: &IniDb) -> BTreeMap<String, [Option<String>; 4]> {
    let mut out = BTreeMap::new();
    for b in ini.of_kind("Bridge") {
        let Some(name) = b.name() else { continue };
        let get = |k: &str| b.value(k).and_then(|v| zr_ini::tokens(v).next().map(str::to_string));
        let names = [
            get("TowerObjectNameFromLeft"),
            get("TowerObjectNameFromRight"),
            get("TowerObjectNameToLeft"),
            get("TowerObjectNameToRight"),
        ];
        if names.iter().any(Option::is_some) {
            out.insert(name.to_owned(), names);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Bridge::Bridge(Object*)` reads each corner's name separately (`TerrainLogic.cpp:388-394`), so
    /// a `Bridge` template that names only some of them still gets the corners it does name (the old
    /// port required all four keys and created nothing otherwise).
    #[test]
    fn tower_names_are_read_per_corner() {
        let mut db = IniDb::new();
        db.extend(
            zr_ini::parse(
                "t.ini",
                "
Bridge Partial
  TowerObjectNameFromLeft = TowerA
  TowerObjectNameToRight = TowerD
End
Bridge NoTowers
  BridgeModelName = CBBridgeSt
End
",
            )
            .unwrap(),
        );
        let names = tower_names(&db);
        assert_eq!(
            names.get("Partial"),
            Some(&[Some("TowerA".to_owned()), None, None, Some("TowerD".to_owned())]),
            "the named corners only, in BRIDGE_MAX_TOWERS order"
        );
        assert!(!names.contains_key("NoTowers"), "a Bridge template without tower names has none");
    }
}
