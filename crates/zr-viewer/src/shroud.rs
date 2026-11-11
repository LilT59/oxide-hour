//! Terrain shroud overlay and fog-of-war unit hiding.
//!
//! Ports the client half of `W3DShroud` / `W3DDisplay::setShroudLevel` (`GameEngineDevice`): a
//! per-partition-cell texture drawn over the terrain with a multiply blend, plus
//! `GameClient.cpp`'s rule that an object whose `ObjectShroudStatus` is fogged or shrouded for the
//! local player is not drawn.
//!
//! `GlobalData` maps a cell's [`CellShroudStatus`] to an alpha (`ShroudColor`, `ClearAlpha`,
//! `FogAlpha`, `ShroudAlpha`); `W3DShroud` scales the shroud colour by that alpha and the result
//! multiplies the terrain. Bevy's `AlphaMode::Multiply` lerps by the texture alpha, so we keep the
//! alpha at 255 and bake `colour * level / 255` into the RGB instead, giving `dst * level/255`:
//! clear leaves the terrain, fog dims it, shroud blacks it out.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use std::collections::BTreeMap;

use zr_sim::shroud::{CellShroudStatus, ObjectShroudStatus};
use zr_sim::template::kind_of;
use zr_sim::{Coord3D, ObjectId};

/// A last-seen snapshot of a fogged enemy structure (`W3DGhostObject` +
/// `W3DRenderObjectSnapshot`). While an entity carries this marker its transform and animation are
/// frozen (`sim_sync` / `animate_rigs`) and it survives the underlying object's death, until the
/// shroud reveals or fully covers the cell again.
#[derive(Component)]
pub struct GhostSnapshot {
    /// The object's bounding-circle radius at snapshot time, used to classify the cell of a ghost
    /// whose object has since died (`W3DGhostObject::getShroudStatus`).
    pub radius: f32,
}

use crate::convert;
use crate::sim_view::{SimActive, SimObject, SimRes};
use crate::source::TerrainData;

/// `GameClient.cpp`: how long (logic frames) an object stays drawn after it was last clear.
const SHROUD_CLEAR_GRACE: u32 = 2 * zr_sim::LOGICFRAMES_PER_SECOND;

/// The live shroud overlay: its texture, grid shape and `GameData` colours.
#[derive(Resource)]
pub struct ShroudView {
    tex: Handle<Image>,
    count: (u32, u32),
    color: [u8; 3],
    clear_alpha: u8,
    fog_alpha: u8,
    shroud_alpha: u8,
    /// Last logic frame each object was visible (`Drawable::m_shroudClearFrame`), for the grace.
    last_clear: BTreeMap<ObjectId, u32>,
}

/// Build the terrain overlay mesh + material + shroud texture and spawn it. `count`/`cell_size`
/// come from the sim's partition manager; the colours from the `GameData` block.
#[allow(clippy::too_many_arguments)]
pub fn spawn_overlay(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
    terrain: &TerrainData,
    count: (i32, i32),
    cell_size: f32,
    color: [u8; 3],
    clear_alpha: u8,
    fog_alpha: u8,
    shroud_alpha: u8,
) -> ShroudView {
    let count = (count.0.max(1) as u32, count.1.max(1) as u32);
    let mesh = overlay_mesh(terrain, count, cell_size);
    let tex = images.add(shroud_image(count, color, shroud_alpha));
    let mat = StandardMaterial {
        base_color_texture: Some(tex.clone()),
        base_color: Color::WHITE,
        unlit: true,
        alpha_mode: AlphaMode::Multiply,
        ..default()
    };
    commands.spawn((Mesh3d(meshes.add(mesh)), MeshMaterial3d(materials.add(mat)), Transform::IDENTITY));
    ShroudView { tex, count, color, clear_alpha, fog_alpha, shroud_alpha, last_clear: BTreeMap::new() }
}

/// The terrain vertices raised slightly and re-UV'd so one texel covers one partition cell.
fn overlay_mesh(terrain: &TerrainData, count: (u32, u32), cell_size: f32) -> Mesh {
    let positions: Vec<[f32; 3]> = terrain
        .positions
        .iter()
        .map(|p| convert::to_bevy([p[0], p[1], p[2] + 0.5]).to_array())
        .collect();
    let inv = 1.0 / (cell_size * count.0 as f32);
    let inv_y = 1.0 / (cell_size * count.1 as f32);
    let uvs: Vec<[f32; 2]> = terrain.positions.iter().map(|p| [p[0] * inv, p[1] * inv_y]).collect();
    let normals = vec![[0.0f32, 1.0, 0.0]; positions.len()];
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
        .with_inserted_indices(Indices::U32(terrain.indices.clone()))
}

/// A `count.x * count.y` `Rgba8` image, all cells at `alpha`, sampled like the engine's shroud texture
/// ([`convert::TextureUse::Shroud`]: one level, clamped, no mip filter).
fn shroud_image(count: (u32, u32), color: [u8; 3], alpha: u8) -> Image {
    let mut data = Vec::with_capacity((count.0 * count.1 * 4) as usize);
    for _ in 0..(count.0 * count.1) {
        data.push((u16::from(color[0]) * u16::from(alpha) / 255) as u8);
        data.push((u16::from(color[1]) * u16::from(alpha) / 255) as u8);
        data.push((u16::from(color[2]) * u16::from(alpha) / 255) as u8);
        data.push(255);
    }
    let mut image = Image::new(
        Extent3d { width: count.0, height: count.1, depth_or_array_layers: 1 },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.sampler = convert::texture_sampler(convert::TextureUse::Shroud);
    image
}

pub fn add_systems(app: &mut App) {
    app.add_systems(Update, shroud_update.after(crate::sim_view::sim_sync).run_if(resource_exists::<ShroudView>).run_if(resource_exists::<SimActive>));
}

/// Refresh the shroud texture from the local player's grid and hide fogged enemies. Enemy
/// structures seen at least once stay drawn as a frozen last-seen ghost while their cells are
/// fogged (`PartitionData::getShroudedStatus` + `W3DGhostObject`): the snapshot is released when
/// the cells clear again, or when they fall to full shroud (the ghost memory is forgotten).
#[allow(clippy::too_many_arguments)]
fn shroud_update(
    sim: NonSend<SimRes>,
    mut view: ResMut<ShroudView>,
    mut images: ResMut<Assets<Image>>,
    mut commands: Commands,
    mut objs: Query<(Entity, &SimObject, &mut Visibility, Option<&GhostSnapshot>, &Transform)>,
) {
    let shroud = sim.game.logic().shroud();
    let (cx, cy) = view.count;
    let mut next = Vec::with_capacity((cx * cy * 4) as usize);
    for y in 0..cy {
        for x in 0..cx {
            let level = match shroud.cell_status(sim.local, x as i32, y as i32) {
                CellShroudStatus::Clear => view.clear_alpha,
                CellShroudStatus::Fogged => view.fog_alpha,
                CellShroudStatus::Shrouded => view.shroud_alpha,
            };
            next.extend_from_slice(&[
                (u16::from(view.color[0]) * u16::from(level) / 255) as u8,
                (u16::from(view.color[1]) * u16::from(level) / 255) as u8,
                (u16::from(view.color[2]) * u16::from(level) / 255) as u8,
                255,
            ]);
        }
    }
    // Write the texels only when they change: `get_mut` marks the image changed, which re-uploads
    // the whole texture to the GPU every frame (the shroud rarely changes between two frames).
    if images.get(&view.tex).and_then(|img| img.data.as_deref()).is_some_and(|d| d.len() >= next.len() && d[..next.len()] != next[..]) {
        if let Some(mut img) = images.get_mut(&view.tex) {
            if let Some(data) = img.data.as_mut() {
                data[..next.len()].copy_from_slice(&next);
            }
        }
    }

    let frame = sim.game.frame();
    let logic = sim.game.logic();
    for (e, so, mut vis, ghost, transform) in &mut objs {
        // The possession camera owns the possessed unit's visibility (it hides it in first person).
        if sim.possessed() == Some(so.0) {
            continue;
        }
        match logic.find_object(so.0) {
            Some(o) => {
                let status = logic.object_shroud_status(sim.local, so.0);
                let grace = view.last_clear.get(&so.0).is_some_and(|f| frame.saturating_sub(*f) < SHROUD_CLEAR_GRACE);
                if status < ObjectShroudStatus::Fogged {
                    // Clear / partial-clear: the real object is drawn; drop any stale ghost.
                    view.last_clear.insert(so.0, frame);
                    if ghost.is_some() {
                        commands.entity(e).remove::<GhostSnapshot>();
                    }
                    set_changed!(*vis, Visibility::Inherited);
                } else {
                    // `PartitionData::getShroudedStatus`: a previously seen immobile enemy in
                    // fogged cells gets a ghost snapshot; a mobile one is hidden.
                    let ghostable = status == ObjectShroudStatus::Fogged
                        && o.is_kind_of(kind_of::IMMOBILE)
                        && view.last_clear.contains_key(&so.0);
                    if ghostable {
                        if ghost.is_none() {
                            debug!("ghost: snapshot {} ({})", so.0.0, o.template.name);
                            commands.entity(e).insert(GhostSnapshot { radius: o.template.geometry.bounding_circle_radius() });
                        }
                        set_changed!(*vis, Visibility::Inherited);
                    } else {
                        if ghost.is_some() {
                            commands.entity(e).remove::<GhostSnapshot>();
                        }
                        // Fully shrouded: forget the memory (`m_everSeenByPlayer = false`).
                        if status == ObjectShroudStatus::Shrouded {
                            view.last_clear.remove(&so.0);
                        }
                        set_changed!(*vis, if grace { Visibility::Inherited } else { Visibility::Hidden });
                    }
                }
            }
            None => {
                // The object is gone (killed while fogged): keep drawing the snapshot while the
                // cells still show fog, and drop it once they clear or are fully shrouded
                // (`W3DGhostObjectManager::updateOrphanedObjects`).
                if let Some(g) = ghost {
                    let p = convert::from_bevy(transform.translation);
                    let status = logic.shroud().object_status(sim.local, &Coord3D::new(p[0], p[1], p[2]), g.radius, true, false, false);
                    if status == ObjectShroudStatus::Fogged {
                        set_changed!(*vis, Visibility::Inherited);
                    } else {
                        commands.entity(e).despawn();
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler};

    #[test]
    fn the_shroud_overlay_samples_like_its_engine_texture_class() {
        // `W3DShroud::ReAcquireResources` (`W3DShroud.cpp:241-257`): `MIP_LEVELS_1`, U/V CLAMP,
        // `Set_Mip_Mapping(FILTER_TYPE_NONE)`; min/mag are the shroud's `m_shroudFilter = FILTER_TYPE_DEFAULT`
        // (`:88`, applied at `:683-686`), i.e. the engine's `TextureFilter` mode.
        let image = shroud_image((4, 3), [0, 0, 0], 255);
        let ImageSampler::Descriptor(d) = image.sampler else { panic!("expected a descriptor sampler") };
        assert_eq!((d.address_mode_u, d.address_mode_v), (ImageAddressMode::ClampToEdge, ImageAddressMode::ClampToEdge));
        assert_eq!((d.mipmap_filter, d.lod_max_clamp), (ImageFilterMode::Nearest, 0.0));
        assert_eq!((d.min_filter, d.mag_filter, d.anisotropy_clamp), (ImageFilterMode::Linear, ImageFilterMode::Linear, 1));
        assert_eq!(image.texture_descriptor.mip_level_count, 1);
    }
}
