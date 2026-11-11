//! Roads and bridges for a Zero Hour map, as plain data in Generals space (X east, Y north,
//! Z up, world units; triangles CCW seen from +Z / outside), like `zr_terrain::TerrainMesh`
//! and `zr_w3d::ModelMesh`.
//!
//! * Roads (`W3DRoadBuffer.cpp`): consecutive map objects flagged `FLAG_ROAD_POINT1` /
//!   `FLAG_ROAD_POINT2` form segments of an INI `Road` type. [`build_roads`] links them,
//!   inserts curves, tees, Y / H / 4-way intersections and alpha joins, and drapes each piece
//!   over the terrain as a texture-mapped alpha strip.
//! * Bridges (`W3DBridgeBuffer.cpp`): `FLAG_BRIDGE_POINT1` / `FLAG_BRIDGE_POINT2` pairs name an
//!   INI `Bridge` template; [`build_bridges`] stretches its left / span / right model pieces
//!   between the two points.

mod bridge;
mod buffer;

use std::collections::HashMap;

use anyhow::Result;
use zr_assets::{RgbaImage, Vfs};
use zr_ini::IniDb;
use zr_map::MapFile;

pub use bridge::{build_bridges, BridgeInstance, BridgePiece};

use buffer::{RoadBuffer, RoadInput, DEFAULT_ROAD_SCALE, FLAG_ROAD_CORNER_ANGLED, FLAG_ROAD_CORNER_TIGHT, FLAG_ROAD_JOIN, FLAG_ROAD_POINT1, FLAG_ROAD_POINT2, MAP_HEIGHT_SCALE, MAP_XY_FACTOR};

/// `FLAG_BRIDGE_POINT1` / `FLAG_BRIDGE_POINT2`.
pub const FLAG_BRIDGE_POINT1: i32 = 0x10;
pub const FLAG_BRIDGE_POINT2: i32 = 0x20;
/// Every flag that makes a map object a road or bridge segment end (not a real object).
pub const SEGMENT_POINT_FLAGS: i32 = FLAG_ROAD_POINT1 | FLAG_ROAD_POINT2 | FLAG_BRIDGE_POINT1 | FLAG_BRIDGE_POINT2;

/// All road pieces of one road type, drawn with one alpha-blended texture.
#[derive(Debug, Clone)]
pub struct RoadMesh {
    /// INI road name (`TwoLane`, ...).
    pub name: String,
    pub positions: Vec<[f32; 3]>,
    /// Always +Z.
    pub normals: Vec<[f32; 3]>,
    /// Top-row-first image coordinates (V = 0 is the top row).
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    /// RGBA, alpha is the road edge / transparency.
    pub texture: RgbaImage,
    /// Draw order among road types (ascending; the engine's "stacking").
    pub order: i32,
}

struct RoadType {
    name: String,
    texture: String,
    width: f32,
    width_in_texture: f32,
}

fn road_types(ini: &IniDb) -> Vec<RoadType> {
    let f = |b: &zr_ini::Block, k: &str, d: f32| b.value(k).and_then(|v| zr_ini::tokens(v).next()?.parse().ok()).unwrap_or(d);
    ini.of_kind("Road")
        .filter_map(|b| {
            Some(RoadType {
                name: b.name()?.to_string(),
                texture: zr_ini::tokens(b.value("Texture")?).next()?.to_string(),
                width: f(b, "RoadWidth", DEFAULT_ROAD_SCALE),
                width_in_texture: f(b, "RoadWidthInTexture", 1.0),
            })
        })
        .collect()
}

/// `BaseHeightMapRenderObjClass::getMaxCellHeight`: the highest of the four corners of the
/// height cell containing `(x, y)` (Generals space, border-offset like `Ground`).
fn max_cell_height(map: &MapFile) -> impl Fn(f32, f32) -> f32 + '_ {
    move |x, y| {
        let Some(hm) = map.height_map.as_ref() else { return 0.0 };
        let (w, h) = (hm.width, hm.height);
        let ix = ((x / MAP_XY_FACTOR) as i32 + hm.border_size).clamp(0, (w - 2).max(0));
        let iy = ((y / MAP_XY_FACTOR) as i32 + hm.border_size).clamp(0, (h - 2).max(0));
        let at = |x: i32, y: i32| f32::from(hm.data.get((y * w + x) as usize).copied().unwrap_or(0)) * MAP_HEIGHT_SCALE;
        at(ix, iy).max(at(ix + 1, iy)).max(at(ix + 1, iy + 1)).max(at(ix, iy + 1))
    }
}

/// Builds every road of the map. Road types whose texture cannot be loaded are skipped.
pub fn build_roads(map: &MapFile, vfs: &Vfs, ini: &IniDb) -> Result<Vec<RoadMesh>> {
    let types = road_types(ini);
    let by_name: HashMap<String, usize> = types.iter().enumerate().map(|(i, t)| (t.name.to_ascii_lowercase(), i)).collect();

    // addMapObjects: a POINT1 object followed by a POINT2 object is one segment.
    let mut inputs = Vec::new();
    let objs = &map.objects;
    let mut i = 0;
    while i < objs.len() {
        let o = &objs[i];
        if o.flags & FLAG_ROAD_POINT1 != 0 {
            let Some(o2) = objs.get(i + 1) else { break };
            if o2.flags & FLAG_ROAD_POINT2 == 0 {
                i += 1;
                continue;
            }
            let (id, scale, wit) = match by_name.get(&o.template.to_ascii_lowercase()) {
                Some(&id) => (id, types[id].width, types[id].width_in_texture),
                None => (usize::MAX, DEFAULT_ROAD_SCALE, 1.0),
            };
            inputs.push(RoadInput {
                loc1: [o.pos[0], o.pos[1]],
                loc2: [o2.pos[0], o2.pos[1]],
                scale,
                width_in_texture: wit,
                id,
                angled: [o.flags & FLAG_ROAD_CORNER_ANGLED != 0, o2.flags & FLAG_ROAD_CORNER_ANGLED != 0],
                join: [o.flags & FLAG_ROAD_JOIN != 0, o2.flags & FLAG_ROAD_JOIN != 0],
                tight: o.flags & FLAG_ROAD_CORNER_TIGHT != 0,
            });
            i += 2;
        } else {
            i += 1;
        }
    }
    if inputs.is_empty() {
        return Ok(Vec::new());
    }

    let mh = max_cell_height(map);
    let mut buf = RoadBuffer::new(types.len(), &mh);
    buf.load_roads(&inputs);

    let mut order: Vec<usize> = (0..types.len()).collect();
    order.sort_by_key(|&i| (buf.stacking[i], i));
    let mut out = Vec::new();
    for (rank, &id) in order.iter().enumerate() {
        let mut m = RoadMesh {
            name: types[id].name.clone(),
            positions: Vec::new(),
            normals: Vec::new(),
            uvs: Vec::new(),
            indices: Vec::new(),
            texture: RgbaImage::new(1, 1),
            order: rank as i32,
        };
        for s in buf.segments_of(id) {
            let base = m.positions.len() as u32;
            for v in &s.verts {
                m.positions.push(v.pos);
                m.normals.push([0.0, 0.0, 1.0]);
                m.uvs.push(v.uv);
            }
            m.indices.extend(s.indices.iter().map(|&i| i + base));
        }
        if m.indices.is_empty() {
            continue;
        }
        match zr_w3d::load_texture(vfs, &types[id].texture) {
            Ok(t) => m.texture = t,
            Err(_) => continue,
        }
        out.push(m);
    }
    Ok(out)
}
