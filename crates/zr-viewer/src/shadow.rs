//! Projected decal shadows (`W3DProjectedShadow`), simplified.
//!
//! The engine uses stencil shadow volumes for most vehicles and structures (`Shadow =
//! SHADOW_VOLUME`) and textured decals for infantry (`SHADOW_DECAL`). Only the decals are ported
//! (the volume pass is a stencil effect, out of scope): a quad on the terrain, sized from
//! `ShadowSizeX/Y`, textured with `ShadowTexture` (default `shadow` / `shadows`,
//! `ThingTemplate::validate`), rotated with the object and pushed along the map's sun direction by
//! its height. `W3DProjectedShadow` projects the texture onto the terrain; the viewer uses a quad
//! (a documented approximation). Volume-typed objects are skipped rather than given a wrong blob.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use std::collections::BTreeMap;
use zr_assets::Vfs;
use zr_sim::ObjectId;
use zr_templates::{GeometryType, ShadowType};

use crate::convert;
use crate::sim_view::{ModelCache, SimRes, STEP};
use crate::MapRes;

#[derive(Resource)]
pub struct ShadowAssets {
    quad: Handle<Mesh>,
    materials: BTreeMap<String, Handle<StandardMaterial>>,
    /// Sun direction in Bevy space (points from the light toward the scene).
    sun: Vec3,
    /// `GlobalLighting::shadow_color` (ARGB), the shadow tint.
    shadow_color: [u8; 3],
}

#[derive(Component)]
struct ObjectShadow(ObjectId);

pub fn spawn_assets(meshes: &mut Assets<Mesh>, sun: Vec3, shadow_color: Option<u32>) -> ShadowAssets {
    // `GlobalLighting::shadow_color` is ARGB; the shipped maps use a purple-grey shadow.
    let shadow_color = shadow_color.map_or([0, 0, 0], |v| [((v >> 16) & 0xff) as u8, ((v >> 8) & 0xff) as u8, (v & 0xff) as u8]);
    ShadowAssets { quad: meshes.add(quad_mesh()), materials: BTreeMap::new(), sun, shadow_color }
}

/// Unit quad in the XZ plane, centred, UV (0,0) at the -x/-z corner.
fn quad_mesh() -> Mesh {
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, vec![[-0.5f32, 0.0, -0.5], [0.5, 0.0, -0.5], [0.5, 0.0, 0.5], [-0.5, 0.0, 0.5]])
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0f32, 1.0, 0.0]; 4])
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0f32, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]])
        .with_inserted_indices(Indices::U32(vec![0, 1, 2, 0, 2, 3]))
}

fn shadow_material(name: &str, color: [u8; 3], vfs: &Vfs, images: &mut Assets<Image>, materials: &mut Assets<StandardMaterial>, cache: &mut BTreeMap<String, Handle<StandardMaterial>>) -> Handle<StandardMaterial> {
    if let Some(h) = cache.get(name) {
        return h.clone();
    }
    // `W3DProjectedShadow::addDecal` / `addShadow` (`W3DProjectedShadow.cpp:1516-1519`): the shadow texture is
    // clamped and its mip mapping is disabled (`TextureUse::DecalShadow`).
    let texture = zr_w3d::load_texture(vfs, name).ok().map(|t| images.add(convert::texture_image(&t, convert::TextureUse::DecalShadow)));
    if texture.is_none() {
        warn!("shadow: texture `{name}` not found");
    }
    let mat = StandardMaterial {
        // The shadow textures are alpha masks; the map's shadow colour tints the decal and `Blend`
        // lets the texture's alpha shape it.
        base_color: Color::srgba(color[0] as f32 / 255.0, color[1] as f32 / 255.0, color[2] as f32 / 255.0, 0.65),
        base_color_texture: texture,
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        cull_mode: None,
        double_sided: true,
        ..default()
    };
    let h = materials.add(mat);
    cache.insert(name.to_owned(), h.clone());
    h
}

pub fn add_systems(app: &mut App) {
    app.add_systems(
        Update,
        shadow_sync
            .after(crate::sim_view::sim_sync)
            .run_if(in_state(crate::shell::ShellState::InGame))
            .run_if(resource_exists::<crate::sim_view::SimActive>),
    );
}

#[allow(clippy::too_many_arguments)]
fn shadow_sync(
    mut commands: Commands,
    sim: NonSend<SimRes>,
    map: Res<MapRes>,
    cache: Res<ModelCache>,
    mut assets: ResMut<ShadowAssets>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut existing: Query<(Entity, &ObjectShadow, &mut Transform, &mut MeshMaterial3d<StandardMaterial>)>,
    settings: Option<Res<crate::options::GameSettings>>,
    presets: Option<Res<crate::options::LodPresets>>,
) {
    let alpha = (sim.acc / STEP).clamp(0.0, 1.0);
    let templates = sim.game.data().templates();
    let ground = map.ground();
    let sun = assets.sun;
    // `UseShadowDecals` of the applied detail level (`OptionsMenu.cpp`); off removes the decals.
    let decals_on = match (settings.as_deref(), presets.as_deref()) {
        (Some(s), Some(p)) => s.lod(p).shadow_decals,
        (Some(s), None) => s.lod(&Default::default()).shadow_decals,
        _ => true,
    };
    let mut desired: BTreeMap<ObjectId, (Handle<StandardMaterial>, Transform)> = BTreeMap::new();
    for o in sim.game.objects() {
        if o.is_drawable_hidden() {
            continue;
        }
        if !decals_on {
            break;
        }
        let Some(thing) = templates.thing(o.template) else { continue };
        if thing.shadow_type.is_empty() {
            continue;
        }
        let decal = thing.shadow_type.contains(ShadowType::ShadowDecal)
            || thing.shadow_type.contains(ShadowType::ShadowAlphaDecal)
            || thing.shadow_type.contains(ShadowType::ShadowAdditiveDecal);
        if !decal {
            continue; // shadow volumes are not ported (see the module docs)
        }
        let radius = sim.game.object_radius(o.id).unwrap_or(5.0);
        let (sx, sy) = if thing.shadow_size_x > 0.0 {
            (thing.shadow_size_x, if thing.shadow_size_y > 0.0 { thing.shadow_size_y } else { thing.shadow_size_x })
        } else {
            (radius * 2.4, radius * 2.4)
        };
        let tex = if !thing.shadow_texture.is_empty() {
            thing.shadow_texture.clone()
        } else if thing.geometry.geometry_type == GeometryType::Box {
            "shadows".to_owned()
        } else {
            "shadow".to_owned()
        };
        let color = assets.shadow_color;
        let mat = shadow_material(&tex, color, &cache.vfs, &mut images, &mut materials, &mut assets.materials);
        let (pos, angle) = crate::sim_view::interpolated(&sim, o.id, alpha).unwrap_or(([o.position.x, o.position.y, o.position.z], o.angle));
        let p = convert::to_bevy(pos);
        let h = thing.geometry.height.max(2.0);
        let off = if sun.y < -0.05 {
            let t = h / -sun.y;
            Vec3::new(sun.x, 0.0, sun.z) * t
        } else {
            Vec3::ZERO
        };
        let y = ground.height(p.x, -p.z) + 0.35;
        let transform = Transform::from_translation(Vec3::new(p.x, y, p.z) + off)
            .with_rotation(Quat::from_rotation_y(angle))
            .with_scale(Vec3::new(sx, 1.0, sy));
        desired.insert(o.id, (mat, transform));
    }

    let mut have = std::collections::BTreeSet::new();
    for (e, os, mut t, mut mat) in &mut existing {
        match desired.get(&os.0) {
            Some((m, tr)) => {
                *t = *tr;
                if mat.0 != *m {
                    mat.0 = m.clone();
                }
                have.insert(os.0);
            }
            None => {
                commands.entity(e).despawn();
            }
        }
    }
    let quad = assets.quad.clone();
    for (id, (m, tr)) in desired {
        if have.contains(&id) {
            continue;
        }
        commands.spawn((ObjectShadow(id), Mesh3d(quad.clone()), MeshMaterial3d(m), tr, Visibility::default()));
    }
}
