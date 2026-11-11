//! Water: `zr-water` plain data -> alpha-blended Bevy meshes with a gently animated texture.
//!
//! Ported look of `WaterRenderObjClass::update` / `drawTrapezoidWater`: the texture wobbles by
//! `0.02 * cos(11 t)` (areas) and rivers scroll along their length (`m_riverVOrigin`
//! advances 0.002 per 30 Hz frame). Client-only, so wall-clock time is fine here.
//! Reflections, bump map and sparkles of the original pixel shaders are not reproduced.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use zr_assets::Vfs;
use zr_ini::IniDb;
use zr_map::{GlobalLighting, MapFile};
use zr_water::WaterKind;

use crate::convert::{self, to_bevy, TextureUse};
use crate::lighting::edge_fade;
use crate::source::Ground;

/// Marks a water material so [`animate`] can move its texture.
#[derive(Component)]
struct WaterAnim {
    material: Handle<StandardMaterial>,
    kind: WaterKind,
}

/// Tags every entity [`spawn`] creates (water meshes and their animation driver). The shell map's
/// water is spawned before a game starts, so it must be removed by [`teardown`]; otherwise it stays
/// in the game's pre-game entity set and floats over the game's terrain.
#[derive(Component)]
pub struct WaterEntity;

/// Despawns the water of the previous world (`OnEnter(Loading)`, before the game's snapshot).
pub fn teardown(world: &mut World) {
    let mut q = world.query_filtered::<Entity, With<WaterEntity>>();
    let ids: Vec<Entity> = q.iter(world).collect();
    for e in ids {
        world.despawn(e);
    }
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

pub fn add_systems(app: &mut App) {
    app.add_systems(Update, animate);
}

/// Spawns the map's water (if any). Failures are logged, never fatal.
#[allow(clippy::too_many_arguments)]
pub fn spawn(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
    map: &MapFile,
    vfs: &Vfs,
    ini: &IniDb,
    ground: &Ground,
    lighting: &GlobalLighting,
) {
    let water = match zr_water::build_water(map, vfs, ini, lighting, &|x, y| ground.height(x, y)) {
        Ok(Some(w)) => w,
        Ok(None) => return,
        Err(e) => {
            warn!("water failed: {e:#}");
            return;
        }
    };
    let look = &water.look;
    // `WaterRenderObjClass::m_riverTexture` (`W3DWater.cpp:1133`) is a plain `Get_Texture`; the draw sets
    // `FILTER_TYPE_BEST` for min/mag/mip (`:218-220`, `:2965-2967`), i.e. the global `TextureFilter` mode, and
    // the `TextureFilterClass` default wrap (repeat). The blend uses the depth-derived alpha, not the texture's.
    let mut tex = convert::texture_image(&look.texture, TextureUse::Water);
    if let Some(d) = tex.data.as_mut() {
        d.chunks_exact_mut(4).for_each(|p| p[3] = 255);
    }
    let tex = images.add(tex);
    let mut mats: [Option<Handle<StandardMaterial>>; 2] = [None, None];
    let rgb = look.shade.map(srgb_to_linear);
    let (mut verts, mut tris) = (0, 0);
    for m in &water.meshes {
        let slot = usize::from(m.kind == WaterKind::River);
        let mat = mats[slot]
            .get_or_insert_with(|| {
                let h = materials.add(StandardMaterial {
                    base_color_texture: Some(tex.clone()),
                    alpha_mode: if look.additive { AlphaMode::Add } else { AlphaMode::Blend },
                    unlit: true,
                    cull_mode: None,
                    double_sided: true,
                    ..default()
                });
                commands.spawn((WaterAnim { material: h.clone(), kind: m.kind }, WaterEntity));
                h
            })
            .clone();
        // Black border shroud outside the playable area, as for the terrain.
        let (pw, ph) = ground.playable_size();
        let colors: Vec<[f32; 4]> = m
            .alpha
            .iter()
            .zip(&m.positions)
            .map(|(&a, p)| {
                let f = edge_fade(p[0], p[1], pw, ph);
                [rgb[0] * f, rgb[1] * f, rgb[2] * f, a]
            })
            .collect();
        let n = m.positions.len();
        let mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, m.positions.iter().map(|&p| to_bevy(p).to_array()).collect::<Vec<_>>())
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 1.0, 0.0]; n])
            .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, m.uvs.clone())
            .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors.clone())
            .with_inserted_indices(Indices::U32(m.indices.clone()));
        verts += n;
        tris += m.indices.len() / 3;
        let handle = meshes.add(mesh);
        let e = commands.spawn((Mesh3d(handle.clone()), MeshMaterial3d(mat), Transform::IDENTITY, WaterEntity)).id();
        // Shroud multiplies water too: rivers bake it per-vertex in retail
        // (`W3DWater::getRiverVertexDiffuse`); the viewer does the same for areas.
        commands.entity(e).insert(crate::terrain_shroud::ShroudVertices::new(handle, colors, m.positions.clone()));
    }
    info!("spawned {} water meshes ({verts} vertices, {tris} triangles)", water.meshes.len());
}

/// `m_riverVOrigin` grows by 0.002 per 30 Hz logic frame.
fn animate(time: Res<Time>, q: Query<&WaterAnim>, mut materials: ResMut<Assets<StandardMaterial>>) {
    let t = time.elapsed_secs() * 30.0 * 0.002;
    for a in &q {
        let Some(mut m) = materials.get_mut(&a.material) else { continue };
        let offset = match a.kind {
            // u1 = x/150 + 0.02 cos(11 t) sin(..): approximate the wobble as a global shift.
            WaterKind::Area => Vec2::new(0.02 * (11.0 * t).cos(), 0.02 * (5.0 * t).cos()),
            WaterKind::River => Vec2::new(0.0, -t),
        };
        m.uv_transform = bevy::math::Affine2::from_translation(offset);
    }
}
