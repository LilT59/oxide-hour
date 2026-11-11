//! Data seam: everything the viewer draws comes through here as plain structs.
//!
//! Today these are self-written fallbacks (height-coloured terrain, coloured
//! primitives for objects). To switch to `zr-terrain` / `zr-w3d`, replace the bodies of
//! [`terrain`] and [`object_model`] (map `zr_terrain::TerrainMesh` -> [`TerrainData`] with
//! `atlas: Some(..)`, `zr_w3d::Model` -> [`ModelPart`]s); `convert.rs` does the Bevy side.

use zr_assets::{RgbaImage, Vfs};
use zr_ini::IniDb;
use zr_map::{HeightMap, MapFile, MapObject};

/// World units per height cell (MapObject.h `MAP_XY_FACTOR`).
pub const MAP_XY_FACTOR: f32 = 10.0;
/// World units per height byte (`MAP_HEIGHT_SCALE`).
pub const MAP_HEIGHT_SCALE: f32 = 10.0 / 16.0;

/// Terrain in Generals space (X east, Y north, Z up, world units), triangles CCW seen from +Z.
/// Mirrors `zr_terrain::TerrainMesh`, plus optional per-vertex colours for the fallback.
pub struct TerrainData {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// Base-layer UVs into `atlas` (empty only when `atlas` is `None`).
    pub uvs: Vec<[f32; 2]>,
    /// Blend-layer UVs, present when the map has a class atlas (the engine's second UV set).
    pub blend_uvs: Vec<[f32; 2]>,
    /// Per-vertex blend alpha (the engine's vertex diffuse alpha); `colors` carries it.
    pub blend_alpha: Vec<u8>,
    /// The 3-way blend pass, if the map has one.
    pub extra: Option<TerrainExtra>,
    pub colors: Option<Vec<[f32; 4]>>,
    pub indices: Vec<u32>,
    pub atlas: Option<RgbaImage>,
}

/// The extra (third) blend layer's quads (`HeightMap.cpp` `renderExtraBlendTiles`). The port
/// pre-composites the three layers per such cell into a block of the terrain atlas and draws the
/// quad opaque with the terrain, before roads and props as the engine does.
pub struct TerrainExtra {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    /// The pass' per-vertex colours, filled by [`crate::lighting::bake_terrain`].
    pub colors: Vec<[f32; 4]>,
}

/// One mesh of an object model, in model space (Z up). Mirrors `zr_w3d::ModelMesh`.
#[derive(Clone)]
pub struct ModelPart {
    /// W3D mesh name (`HOUSECOLOR*` meshes take the player colour as vertex-material colour).
    pub name: String,
    /// `container.name`, as the engine's render object names it (see `zr_w3d::ModelMesh::full_name`).
    pub full_name: String,
    /// Resolved texture path, for `ZHC*` house-colour texture remapping (see `team_color`).
    pub texture_name: Option<String>,
    /// The texture's sampling attributes (wrap, mip cap), from its W3D texture info chunk.
    pub texture_params: zr_w3d::TextureParams,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    pub color: [f32; 4],
    pub texture: Option<RgbaImage>,
    /// Shader classification (opaque / alpha-test / blend / additive) for the Bevy `AlphaMode`.
    pub blend: zr_w3d::BlendMode,
    pub two_sided: bool,
    /// Rigid bone binding (HLOD sub-object) for skeletal animation, if any.
    pub bone: Option<usize>,
    /// Skinned per-vertex bone indices (W3D stores one bone per vertex), if any.
    pub vertex_bones: Option<Vec<u16>>,
}

/// Terrain height sampler in Generals space (origin = playable-area corner, i.e. the
/// height grid is offset by `border_size` cells).
pub struct Ground<'a> {
    hm: &'a HeightMap,
}

impl<'a> Ground<'a> {
    pub fn new(hm: &'a HeightMap) -> Self {
        Self { hm }
    }

    fn cell(&self, x: i32, y: i32) -> f32 {
        let (w, h) = (self.hm.width, self.hm.height);
        let (x, y) = (x.clamp(0, w - 1) as usize, y.clamp(0, h - 1) as usize);
        f32::from(self.hm.data[y * w as usize + x]) * MAP_HEIGHT_SCALE
    }

    /// Terrain height at world `(x, y)`; each cell is two triangles with the diagonal from
    /// (0,0) to (1,1), matching [`terrain`].
    pub fn height(&self, x: f32, y: f32) -> f32 {
        let b = self.hm.border_size as f32;
        let (gx, gy) = (x / MAP_XY_FACTOR + b, y / MAP_XY_FACTOR + b);
        let (ix, iy) = (gx.floor(), gy.floor());
        let (fx, fy) = (gx - ix, gy - iy);
        let (ix, iy) = (ix as i32, iy as i32);
        let (h00, h10, h01, h11) = (self.cell(ix, iy), self.cell(ix + 1, iy), self.cell(ix, iy + 1), self.cell(ix + 1, iy + 1));
        if fx >= fy {
            h00 + (h10 - h00) * fx + (h11 - h10) * fy
        } else {
            h00 + (h11 - h01) * fx + (h01 - h00) * fy
        }
    }

    /// Playable area `(width, height)` in world units.
    pub fn playable_size(&self) -> (f32, f32) {
        let (w, h) = self.hm.boundaries.first().copied().unwrap_or((self.hm.width - 2 * self.hm.border_size, self.hm.height - 2 * self.hm.border_size));
        ((w as f32) * MAP_XY_FACTOR, (h as f32) * MAP_XY_FACTOR)
    }
}

/// Builds the terrain. Fallback: heightfield with vertex colours by height / slope.
/// Textured terrain from zr-terrain; falls back to a height-coloured heightfield.
pub fn terrain(map: &MapFile, vfs: &Vfs, ini: &IniDb) -> anyhow::Result<TerrainData> {
    match zr_terrain::build_terrain(map, vfs, ini) {
        Ok(t) => {
            if !t.stats.missing_classes.is_empty() {
                bevy::log::warn!("missing terrain textures: {:?}", t.stats.missing_classes);
            }
            Ok(TerrainData {
                positions: t.positions,
                normals: t.normals,
                uvs: t.uvs,
                blend_uvs: t.blend_uvs,
                blend_alpha: t.blend_alpha,
                extra: t.extra.map(|e| TerrainExtra {
                    positions: e.positions,
                    normals: e.normals,
                    uvs: e.uvs,
                    indices: e.indices,
                    colors: Vec::new(),
                }),
                colors: None,
                indices: t.indices,
                atlas: Some(t.atlas),
            })
        }
        Err(e) => {
            bevy::log::warn!("textured terrain failed ({e}); using heightfield");
            heightfield_terrain(map)
        }
    }
}

fn heightfield_terrain(map: &MapFile) -> anyhow::Result<TerrainData> {
    let hm = map.height_map.as_ref().ok_or_else(|| anyhow::anyhow!("map has no height map"))?;
    let g = Ground::new(hm);
    let (w, h) = (hm.width as usize, hm.height as usize);
    let b = hm.border_size as f32;
    let mut positions = Vec::with_capacity(w * h);
    let mut normals = Vec::with_capacity(w * h);
    let mut colors = Vec::with_capacity(w * h);
    let mut uvs = Vec::with_capacity(w * h);
    for j in 0..h {
        for i in 0..w {
            let (ci, cj) = (i as i32, j as i32);
            let z = g.cell(ci, cj);
            positions.push([(i as f32 - b) * MAP_XY_FACTOR, (j as f32 - b) * MAP_XY_FACTOR, z]);
            let dx = (g.cell(ci + 1, cj) - g.cell(ci - 1, cj)) / (2.0 * MAP_XY_FACTOR);
            let dy = (g.cell(ci, cj + 1) - g.cell(ci, cj - 1)) / (2.0 * MAP_XY_FACTOR);
            let n = normalize([-dx, -dy, 1.0]);
            normals.push(n);
            colors.push(height_color(z, n[2]));
            uvs.push([i as f32 / (w - 1) as f32, j as f32 / (h - 1) as f32]);
        }
    }
    let mut indices = Vec::with_capacity((w - 1) * (h - 1) * 6);
    for j in 0..h - 1 {
        for i in 0..w - 1 {
            let a = (j * w + i) as u32;
            let (b, c, d) = (a + 1, a + w as u32, a + w as u32 + 1);
            // Diagonal a-d, both triangles CCW seen from +Z.
            indices.extend_from_slice(&[a, b, d, a, d, c]);
        }
    }
    Ok(TerrainData {
        positions,
        normals,
        uvs,
        blend_uvs: Vec::new(),
        blend_alpha: Vec::new(),
        extra: None,
        colors: Some(colors),
        indices,
        atlas: None,
    })
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    [v[0] / l, v[1] / l, v[2] / l]
}

fn height_color(z: f32, nz: f32) -> [f32; 4] {
    let lerp = |a: [f32; 3], b: [f32; 3], t: f32| {
        let t = t.clamp(0.0, 1.0);
        [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
    };
    let grass = [0.28, 0.40, 0.18];
    let dirt = [0.42, 0.33, 0.22];
    let rock = [0.45, 0.44, 0.43];
    let snow = [0.95, 0.96, 0.98];
    let mut c = lerp(grass, dirt, (z - 10.0) / 50.0);
    c = lerp(c, rock, (z - 60.0) / 60.0);
    c = lerp(c, snow, (z - 110.0) / 40.0);
    c = lerp(c, rock, (0.85 - nz) / 0.2); // steep -> grey
    [c[0], c[1], c[2], 1.0]
}

/// Object categories for fallback primitives.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Tree,
    Rock,
    Building,
    Other,
}

fn classify(template: &str) -> (Kind, [f32; 4]) {
    let t = template.to_ascii_lowercase();
    let side = if t.starts_with("america") {
        Some([0.2, 0.4, 0.95, 1.0])
    } else if t.starts_with("china") {
        Some([0.9, 0.15, 0.15, 1.0])
    } else if t.starts_with("gla") {
        Some([0.9, 0.75, 0.15, 1.0])
    } else if ["airf", "lazr", "infa", "nuke", "supw", "boss"].iter().any(|p| t.starts_with(p)) {
        Some([0.5, 0.2, 0.7, 1.0])
    } else {
        None
    };
    if let Some(c) = side {
        return (Kind::Building, c);
    }
    if ["tree", "pine", "fir", "bush", "palm"].iter().any(|k| t.contains(k)) {
        (Kind::Tree, [0.1, 0.45, 0.15, 1.0])
    } else if ["rock", "boulder", "stone", "cliff"].iter().any(|k| t.contains(k)) {
        (Kind::Rock, [0.5, 0.5, 0.52, 1.0])
    } else if ["oil", "supply", "tech", "hospital"].iter().any(|k| t.contains(k)) {
        (Kind::Building, [0.9, 0.5, 0.1, 1.0])
    } else {
        (Kind::Other, [0.7, 0.65, 0.55, 1.0])
    }
}

/// Whether the map object should be drawn at all (skips waypoints, lights, roads, ...).
pub fn is_visible(o: &MapObject) -> bool {
    /// `FLAG_ROAD_FLAGS | FLAG_BRIDGE_FLAGS` (MapObject.h): road and bridge points are
    /// segment endpoints rendered as terrain decals / bridges, not as objects. Rendered by `roads.rs` (zr-roads).
    const SEGMENT_POINT_FLAGS: i32 = 0x02 | 0x04 | 0x10 | 0x20;
    let t = o.template.to_ascii_lowercase();
    o.flags & SEGMENT_POINT_FLAGS == 0 && !t.starts_with('*') && !t.starts_with("amb_") && !t.contains("waypoint")
}

/// Waypoints named `Player_N_Start`: `(N, [x, y, z])` in Generals space (debug markers).
pub fn player_starts(map: &MapFile) -> Vec<(u32, [f32; 3])> {
    map.objects
        .iter()
        .filter_map(|o| {
            let n = o.props.str("waypointName")?.to_ascii_lowercase();
            let n = n.strip_prefix("player_")?.strip_suffix("_start")?.parse().ok()?;
            Some((n, o.pos))
        })
        .collect()
}

/// A tall thin pillar used to mark spots (player starts).
pub fn marker(color: [f32; 4]) -> ModelPart {
    cuboid([3.0, 3.0, 120.0], color)
}

/// Model for a map object's template: the real W3D model when it resolves and loads,
/// else a coloured primitive. Return `None` to skip.
pub fn object_model(ini: &IniDb, vfs: &Vfs, template: &str) -> Option<Vec<ModelPart>> {
    match w3d_model(ini, vfs, template) {
        Ok(Some(parts)) => return Some(parts),
        // A template without a resolvable model has no render object in the engine
        // (`W3DModelDraw` with no `Model`, e.g. the `Amb_*` ambient-sound emitters): draw nothing
        // instead of a placeholder box.
        Ok(None) => return None,
        Err(e) => bevy::log::warn!("{template}: {e}; using placeholder"),
    }
    let (kind, color) = classify(template);
    Some(match kind {
        Kind::Tree => vec![cone([5.0, 5.0, 30.0], color)],
        Kind::Rock => vec![cuboid([6.0, 5.0, 8.0], color)],
        Kind::Building => vec![cuboid([18.0, 15.0, 24.0], color)],
        Kind::Other => vec![cuboid([6.0, 6.0, 10.0], color)],
    })
}

fn w3d_model(ini: &IniDb, vfs: &Vfs, template: &str) -> anyhow::Result<Option<Vec<ModelPart>>> {
    let Some(name) = zr_w3d::model_for_template(ini, template) else { return Ok(None) };
    // Possession LOD: `zr_w3d::load_model` always assembles the highest-detail HLOD LOD
    // (`model.rs::top_lod`, the largest-`MaxScreenSize` array, `WW3D2/hlod.cpp`). The engine picks
    // a coarser LOD by screen size and a possession camera could force `Set_LOD_Level(0)`, but
    // there is no coarser LOD here to force away from: the close camera already gets the top one.
    let model = zr_w3d::load_model(vfs, &name)?;
    Ok(Some(parts_of(model, vfs)))
}

/// Real W3D parts for a model file name. Used when a condition state selects a different model
/// (damaged / rubble / dying) than the template's default.
pub fn model_by_name(vfs: &Vfs, name: &str) -> Option<Vec<ModelPart>> {
    zr_w3d::load_model(vfs, name).ok().map(|m| parts_of(m, vfs))
}

fn parts_of(model: zr_w3d::Model, vfs: &Vfs) -> Vec<ModelPart> {
    model
        .meshes
        .into_iter()
        .filter(|m| !m.hidden)
        .map(|m| ModelPart {
            texture: m.texture.as_deref().and_then(|t| zr_w3d::load_texture(vfs, t).ok()),
            name: m.name,
            full_name: m.full_name,
            texture_name: m.texture,
            texture_params: m.texture_params,
            positions: m.positions,
            normals: m.normals,
            uvs: m.uvs,
            indices: m.indices,
            color: [1.0; 4],
            blend: m.blend,
            two_sided: m.two_sided,
            bone: m.bone,
            vertex_bones: m.vertex_bones,
        })
        .collect()
}

fn empty_part(color: [f32; 4]) -> ModelPart {
    ModelPart { name: String::new(), full_name: String::new(), texture_name: None, texture_params: zr_w3d::TextureParams::default(), positions: vec![], normals: vec![], uvs: vec![], indices: vec![], color, texture: None, blend: zr_w3d::BlendMode::Opaque, two_sided: false, bone: None, vertex_bones: None }
}

/// Box with its base on z=0; `h` = half-extents in x/y and full height in z.
fn cuboid(h: [f32; 3], color: [f32; 4]) -> ModelPart {
    let mut p = empty_part(color);
    let (x, y, z) = (h[0], h[1], h[2]);
    // (normal, four corners CCW seen from outside)
    let faces: [([f32; 3], [[f32; 3]; 4]); 6] = [
        ([0.0, 0.0, 1.0], [[-x, -y, z], [x, -y, z], [x, y, z], [-x, y, z]]),
        ([0.0, 0.0, -1.0], [[-x, y, 0.0], [x, y, 0.0], [x, -y, 0.0], [-x, -y, 0.0]]),
        ([0.0, -1.0, 0.0], [[-x, -y, 0.0], [x, -y, 0.0], [x, -y, z], [-x, -y, z]]),
        ([0.0, 1.0, 0.0], [[x, y, 0.0], [-x, y, 0.0], [-x, y, z], [x, y, z]]),
        ([1.0, 0.0, 0.0], [[x, -y, 0.0], [x, y, 0.0], [x, y, z], [x, -y, z]]),
        ([-1.0, 0.0, 0.0], [[-x, y, 0.0], [-x, -y, 0.0], [-x, -y, z], [-x, y, z]]),
    ];
    for (n, c) in faces {
        let base = p.positions.len() as u32;
        for (k, v) in c.iter().enumerate() {
            p.positions.push(*v);
            p.normals.push(n);
            p.uvs.push([f32::from(k == 1 || k == 2), f32::from(k >= 2)]);
        }
        p.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    p
}

/// Cone with base radius `s[0]` on z=0 and height `s[2]`, 8 sides, smooth normals.
fn cone(s: [f32; 3], color: [f32; 4]) -> ModelPart {
    let mut p = empty_part(color);
    const N: u32 = 8;
    for k in 0..N {
        let a = k as f32 / N as f32 * std::f32::consts::TAU;
        let (c, sn) = (a.cos(), a.sin());
        p.positions.push([c * s[0], sn * s[0], 0.0]);
        p.normals.push(normalize([c * s[2], sn * s[2], s[0]]));
        p.uvs.push([k as f32 / N as f32, 0.0]);
    }
    p.positions.push([0.0, 0.0, s[2]]);
    p.normals.push([0.0, 0.0, 1.0]);
    p.uvs.push([0.5, 1.0]);
    for k in 0..N {
        p.indices.extend_from_slice(&[k, (k + 1) % N, N]);
    }
    p
}
