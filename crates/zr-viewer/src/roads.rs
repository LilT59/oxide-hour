//! Roads and bridges: `zr-roads` plain data -> Bevy meshes.
//!
//! Roads are alpha-blended strips floating a hair above the terrain (depth bias keeps them on
//! top at distance); bridges are ordinary textured meshes already placed in world space.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::Face;
use zr_assets::Vfs;
use zr_ini::IniDb;
use zr_map::{GlobalLighting, MapFile};

use crate::convert::{bridge_image, road_image, to_bevy};
use crate::lighting::ground_decal_colors;
use crate::source::Ground;

fn bevy_mesh(positions: &[[f32; 3]], normals: &[[f32; 3]], uvs: &[[f32; 2]], indices: &[u32]) -> Mesh {
    let conv = |v: &[[f32; 3]]| -> Vec<[f32; 3]> { v.iter().map(|&p| to_bevy(p).to_array()).collect() };
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, conv(positions))
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, conv(normals))
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs.to_vec())
        .with_inserted_indices(Indices::U32(indices.to_vec()))
}

/// Tags the road and bridge entities. The shell map's roads are spawned before a game starts, so
/// [`teardown`] must remove them; otherwise they stay in the game's pre-game entity set.
#[derive(Component)]
pub struct RoadEntity;

/// Despawns the roads and bridges of the previous world (`OnEnter(Loading)`, before the snapshot).
pub fn teardown(world: &mut World) {
    let mut q = world.query_filtered::<Entity, With<RoadEntity>>();
    let ids: Vec<Entity> = q.iter(world).collect();
    for e in ids {
        world.despawn(e);
    }
}

/// Spawns all roads and bridges of `map`. Failures are logged, never fatal.
#[allow(clippy::too_many_arguments)]
pub fn spawn(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    terrain_materials: &mut Assets<crate::terrain::TerrainMaterial>,
    images: &mut Assets<Image>,
    map: &MapFile,
    vfs: &Vfs,
    ini: &IniDb,
    ground: &Ground,
    lighting: &GlobalLighting,
) {
    let shadow_color = crate::lighting::shadow_color_linear(lighting);
    match zr_roads::build_roads(map, vfs, ini) {
        Ok(roads) => {
            for r in &roads {
                // Baked per-vertex like the terrain, so roads match the ground under them; the
                // terrain material darkens them by the shadow map too.
                let mat = crate::terrain::TerrainMaterial {
                    base_color: Vec4::ONE,
                    base_color_texture: Some(images.add(road_image(&r.texture))),
                    shadow_color,
                    alpha_mode: AlphaMode::Blend,
                    // Later road types stack on top of earlier ones.
                    depth_bias: 4.0 + r.order as f32,
                };
                let base = ground_decal_colors(&r.positions, ground, lighting);
                let mesh = bevy_mesh(&r.positions, &r.normals, &r.uvs, &r.indices)
                    .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, base.clone());
                let handle = meshes.add(mesh);
                let e = commands
                    .spawn((
                        Mesh3d(handle.clone()),
                        MeshMaterial3d(terrain_materials.add(mat)),
                        Transform::IDENTITY,
                        RoadEntity,
                    ))
                    .id();
                // Roads are darkened by the terrain shroud pass in retail; the viewer multiplies
                // the cell level into the decal's per-vertex colours each frame.
                commands.entity(e).insert(crate::terrain_shroud::ShroudVertices::new(handle, base, r.positions.clone()));
            }
            info!("spawned {} road meshes ({} triangles)", roads.len(), roads.iter().map(|r| r.indices.len() / 3).sum::<usize>());
        }
        Err(e) => warn!("roads failed: {e:#}"),
    }
    match zr_roads::build_bridges(map, vfs, ini, &|x, y| ground.height(x, y)) {
        Ok(bridges) => {
            for b in &bridges {
                for p in &b.pieces {
                    let mat = StandardMaterial {
                        base_color_texture: p.texture.as_ref().map(|t| images.add(bridge_image(t))),
                        alpha_mode: if p.alpha_blend { AlphaMode::Mask(0.5) } else { AlphaMode::Opaque },
                        cull_mode: if p.two_sided { None } else { Some(Face::Back) },
                        double_sided: p.two_sided,
                        perceptual_roughness: 0.9,
                        ..default()
                    };
                    let handle = materials.add(mat);
                    let e = commands
                        .spawn((
                            Mesh3d(meshes.add(bevy_mesh(&p.positions, &p.normals, &p.uvs, &p.indices))),
                            MeshMaterial3d(handle.clone()),
                            Transform::IDENTITY,
                            RoadEntity,
                        ))
                        .id();
                    // Bridges get their own shroud pass in retail (`W3DBridgeBuffer::drawBridges`).
                    let mut pos = [0.0f32; 3];
                    for v in &p.positions {
                        pos[0] += v[0];
                        pos[1] += v[1];
                        pos[2] += v[2];
                    }
                    let n = p.positions.len().max(1) as f32;
                    let pos = [pos[0] / n, pos[1] / n, pos[2] / n];
                    commands.entity(e).insert(crate::terrain_shroud::ShroudProp::new(pos, vec![(e, handle)]));
                }
                info!("bridge {} from {:?} to {:?}: {} pieces", b.template, b.from, b.to, b.pieces.len());
            }
        }
        Err(e) => warn!("bridges failed: {e:#}"),
    }
}
