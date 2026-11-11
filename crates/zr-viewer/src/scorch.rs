//! Terrain scorch decals (`W3DScorch` / `TheTerrainRenderObject->addScorch`).
//!
//! Ported from `W3DDevice/GameClient/W3DScorch.cpp`: an `FXList`'s `TerrainScorch` nugget calls
//! `TheGameClient->addScorch(pos, radius, type)` (`FXList.cpp:448`), which appends to a deque capped
//! at `MAX_SCORCH_MARKS` (oldest popped) after the `isDuplicate` test. The marks are drawn as
//! terrain-grid-aligned quads textured from `EXScorch01.tga` (a 3x3 sheet of marks, `SCORCH_PER_ROW`
//! = 3), alpha-blended and modulated by the terrain ambient/diffuse average
//! (`W3DScorch::updateScorches`). The viewer rebuilds the same terrain-cell grid mesh on the CPU.
//!
//! The sim records the `TerrainScorch` FXList (e.g. a tank's `FX_GenericTankDeathExplosion`, the
//! particle uplink cannon's `GroundHitFX`); the FX player in [`crate::fx`] feeds it here.

use std::collections::VecDeque;

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use crate::convert;
use crate::source::{Ground, MAP_HEIGHT_SCALE, MAP_XY_FACTOR};
use crate::MapRes;

/// `W3DScorch.h`: `MAX_SCORCH_MARKS` / `SCORCH_PER_ROW` / `SCORCH_MARKS_IN_TEXTURE`.
const MAX_SCORCH_MARKS: usize = 500;
const SCORCH_PER_ROW: i32 = 3;
/// `ScorchTextureClass` (`TerrainTex.cpp:1092`).
const SCORCH_TEXTURE: &str = "EXScorch01.tga";
/// `writeScorchToBuffer`'s `amtToFloat = MAP_HEIGHT_SCALE / 10`, the z lift over the terrain.
const SCORCH_Z_LIFT: f32 = MAP_HEIGHT_SCALE / 10.0;
/// One scorch's grid is capped to keep a bad radius from allocating without bound.
const MAX_SCORCH_GRID: i32 = 256;

/// `TheGlobalData->m_bilinearTerrainTex` / `m_trilinearTerrainTex` (`GlobalData.cpp:647-648`, defaults FALSE),
/// which `ScorchTextureClass::Apply` reads to override the global texture filter (`TerrainTex.cpp:1111-1122`).
/// The shipped `Data\INI\gamedata.ini` sets both to `Yes`.
fn terrain_texture_filter(ini: &zr_ini::IniDb) -> (bool, bool) {
    let gd = ini.resolved("GameData", "");
    let flag = |key: &str| {
        gd.as_ref().and_then(|b| b.value(key)).is_some_and(|v| {
            matches!(v.trim().to_ascii_lowercase().as_str(), "yes" | "true" | "1")
        })
    };
    (flag("BilinearTerrainTex"), flag("TrilinearTerrainTex"))
}

#[derive(Clone, Copy)]
struct Mark {
    pos: [f32; 3],
    radius: f32,
    kind: i32,
}

/// The live scorch buffer (`W3DScorch::m_scorches`).
#[derive(Resource, Default)]
pub struct Scorches {
    marks: VecDeque<Mark>,
    dirty: bool,
    texture: Option<Handle<Image>>,
    material: Option<Handle<StandardMaterial>>,
}

impl Scorches {
    /// `W3DScorch::addScorch`: type `-1` (RANDOM) picks `SCORCH_1..SCORCH_4`; duplicates within
    /// `radius/4` are dropped; the deque is capped at `MAX_SCORCH_MARKS`.
    pub fn add(&mut self, pos: [f32; 3], radius: f32, kind: Option<i32>, rng: &mut zr_fx::ClientRng) {
        let kind = kind.unwrap_or_else(|| (rng.next_u32() % 4) as i32);
        if radius <= 0.0 {
            return;
        }
        let limit = radius / 4.0;
        if self.marks.iter().any(|m| {
            m.kind == kind
                && (m.pos[0] - pos[0]).abs() < limit
                && (m.pos[1] - pos[1]).abs() < limit
                && (m.radius - radius).abs() < limit
        }) {
            return;
        }
        if self.marks.len() >= MAX_SCORCH_MARKS {
            self.marks.pop_front();
        }
        self.marks.push_back(Mark { pos, radius, kind });
        self.dirty = true;
    }
}

/// Marker for the single mesh entity the scorch buffer draws into.
#[derive(Component)]
struct ScorchMesh;

pub fn add_systems(app: &mut App) {
    app.init_resource::<Scorches>();
    app.add_systems(Update, render_scorches.run_if(resource_exists::<MapRes>).run_if(resource_exists::<crate::sim_view::ModelCache>));
}

/// `W3DScorch::updateScorches` + `drawScorches`: rebuild the grid mesh and draw it over the terrain.
#[allow(clippy::too_many_arguments)]
fn render_scorches(
    mut commands: Commands,
    mut scorches: ResMut<Scorches>,
    map: Res<MapRes>,
    cache: Res<crate::sim_view::ModelCache>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    existing: Query<Entity, With<ScorchMesh>>,
) {
    if !scorches.dirty {
        return;
    }
    scorches.dirty = false;
    if scorches.marks.is_empty() {
        return;
    }
    let ground = map.ground();
    // Terrain ambient + diffuse average (`W3DScorch.cpp:157`), the fixed diffuse the draw modulates by.
    let light = crate::lighting::map_lighting(&map.0);
    let tod = light.tod_index();
    let g = light.terrain[tod][0];
    let shade = [
        (g.ambient[0] + g.diffuse[0]) * 0.5,
        (g.ambient[1] + g.diffuse[1]) * 0.5,
        (g.ambient[2] + g.diffuse[2]) * 0.5,
    ];
    if scorches.texture.is_none() {
        let (bilinear, trilinear) = terrain_texture_filter(&cache.ini);
        match zr_w3d::load_texture(&cache.vfs, SCORCH_TEXTURE) {
            Ok(t) => {
                let family = convert::TextureUse::Scorch { bilinear, trilinear };
                scorches.texture = Some(images.add(convert::texture_image(&t, family)));
            }
            Err(e) => warn!("scorch: texture load failed: {e}"),
        }
    }
    let Some(tex) = scorches.texture.clone() else { return };
    // `ScorchTextureClass::Apply` (`TerrainTex.cpp:1106`): color = texture * diffuse (the terrain
    // shade), alpha = texture alpha, standard src-alpha blending (no alpha test). `unlit` keeps the
    // scene's lights from lighting the decal a second time; `base_color` carries the shade.
    let material = scorches.material.clone().unwrap_or_else(|| {
        let m = materials.add(StandardMaterial {
            base_color: Color::linear_rgb(shade[0], shade[1], shade[2]),
            base_color_texture: Some(tex),
            alpha_mode: AlphaMode::Blend,
            unlit: true,
            cull_mode: None,
            double_sided: true,
            // The grid follows `Ground::height`, which can disagree with the terrain mesh's per-cell
            // diagonal; bias the decal toward the camera so it never z-fights the terrain.
            depth_bias: 4.0,
            ..default()
        });
        scorches.material = Some(m.clone());
        m
    });
    let mesh = build_mesh(&scorches.marks, &ground);
    let handle = meshes.add(mesh);
    if let Ok(e) = existing.single() {
        commands.entity(e).insert(Mesh3d(handle));
    } else {
        commands.spawn((
            ScorchMesh,
            Mesh3d(handle),
            MeshMaterial3d(material),
            Transform::IDENTITY,
            Visibility::default(),
            bevy::light::NotShadowCaster,
        ));
    }
}

/// The engine's `writeScorchToBuffer` grid: every map cell corner in the mark's bounding rectangle
/// carries a vertex at its terrain height with a UV derived from its world position, so the decal
/// conforms to the ground. Cells are split along the `(0,0)-(1,1)` diagonal, matching
/// [`Ground::height`].
fn build_mesh(marks: &VecDeque<Mark>, ground: &Ground) -> Mesh {
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    for m in marks {
        let (lx, ly, r) = (m.pos[0], m.pos[1], m.radius);
        if r.is_nan() || r <= 0.0 {
            continue;
        }
        let u_offset = (m.kind % SCORCH_PER_ROW) as f32 * 1.5;
        let v_offset = (m.kind / SCORCH_PER_ROW) as f32 * 1.5;
        // `W3DScorch.cpp:182-199`: `floor((loc - radius)/factor)` .. `ceil((loc + radius)/factor) + 1`.
        let min_x = ((lx - r) / MAP_XY_FACTOR).floor() as i32;
        let min_y = ((ly - r) / MAP_XY_FACTOR).floor() as i32;
        let max_x = ((lx + r) / MAP_XY_FACTOR).ceil() as i32 + 1;
        let max_y = ((ly + r) / MAP_XY_FACTOR).ceil() as i32 + 1;
        let (nx, ny) = (max_x - min_x, max_y - min_y);
        if nx <= 1 || ny <= 1 || nx > MAX_SCORCH_GRID || ny > MAX_SCORCH_GRID {
            continue;
        }
        let base = positions.len() as u32;
        for j in min_y..max_y {
            for i in min_x..max_x {
                let (x, y) = (i as f32 * MAP_XY_FACTOR, j as f32 * MAP_XY_FACTOR);
                let z = ground.height(x, y) + SCORCH_Z_LIFT;
                positions.push(convert::to_bevy([x, y, z]).to_array());
                normals.push([0.0, 1.0, 0.0]);
                // `W3DScorch.cpp:229`: UVs spaced by 1.5 per mark, one mark tile per `+1` divisor.
                uvs.push([
                    (u_offset + 0.5 + (x - lx) / (2.0 * r)) / (SCORCH_PER_ROW as f32 + 1.0),
                    (v_offset + 0.5 + (y - ly) / (2.0 * r)) / (SCORCH_PER_ROW as f32 + 1.0),
                ]);
            }
        }
        let row = nx as u32;
        for j in 0..(ny - 1) as u32 {
            for i in 0..(nx - 1) as u32 {
                let a = base + j * row + i;
                let b = a + 1;
                let c = a + row + 1;
                let d = a + row;
                indices.extend_from_slice(&[a, b, c, a, c, d]);
            }
        }
    }
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
        .with_inserted_indices(Indices::U32(indices))
}

#[cfg(test)]
mod tests {
    use super::*;
    use zr_map::HeightMap;

    fn flat_ground(size: i32) -> HeightMap {
        HeightMap {
            version: 4,
            width: size,
            height: size,
            border_size: 0,
            boundaries: vec![(size, size)],
            data: vec![0; (size * size) as usize],
        }
    }

    /// The grid spans the mark's cell bounding rectangle (`writeScorchToBuffer`'s min/max).
    #[test]
    fn grid_covers_bounding_cells() {
        let hm = flat_ground(32);
        let ground = Ground::new(&hm);
        let mut marks = VecDeque::new();
        marks.push_back(Mark { pos: [50.0, 50.0, 0.0], radius: 25.0, kind: 0 });
        let mesh = build_mesh(&marks, &ground);
        // min = floor((50-25)/10) = 2, max = ceil((50+25)/10)+1 = 9 -> 7x7 vertices, 6x6x2 tris.
        assert_eq!(mesh.count_vertices(), 49);
        let indices = mesh.indices().map_or(0, |i| i.len());
        assert_eq!(indices, 6 * 6 * 2 * 3);
    }

    /// The centre vertex lands on the centre of the mark's tile (`u = v = 0.5/4` for SCORCH_1).
    #[test]
    fn centre_uv_is_tile_centre() {
        let hm = flat_ground(32);
        let ground = Ground::new(&hm);
        let mut marks = VecDeque::new();
        // Even vertex count keeps the mark centred on a grid vertex at (50, 50).
        marks.push_back(Mark { pos: [50.0, 50.0, 0.0], radius: 25.0, kind: 0 });
        let mesh = build_mesh(&marks, &ground);
        let bevy::mesh::VertexAttributeValues::Float32x2(uvs) = mesh.attribute(Mesh::ATTRIBUTE_UV_0).unwrap() else {
            panic!("UVs should be Float32x2");
        };
        // Vertex (50, 50) is index 3*7+3 = 24 in the row-major grid.
        let uv = uvs[24];
        assert!((uv[0] - 0.125).abs() < 1e-5, "{uv:?}");
        assert!((uv[1] - 0.125).abs() < 1e-5, "{uv:?}");
    }

    /// `isDuplicate` drops a same-kind mark within `radius/4`, and the deque caps at 500.
    #[test]
    fn dedup_and_cap() {
        let mut s = Scorches::default();
        let mut rng = zr_fx::ClientRng::new(7);
        s.add([0.0, 0.0, 0.0], 40.0, Some(0), &mut rng);
        s.add([2.0, 2.0, 0.0], 40.0, Some(0), &mut rng);
        assert_eq!(s.marks.len(), 1, "duplicate within radius/4 dropped");
        for i in 0..600 {
            s.add([(i % 20) as f32 * 50.0, (i / 20) as f32 * 50.0, 0.0], 100.0, Some(1), &mut rng);
        }
        assert_eq!(s.marks.len(), MAX_SCORCH_MARKS);
    }
}
