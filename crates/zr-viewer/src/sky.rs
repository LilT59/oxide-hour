//! Time-of-day skybox.
//!
//! Ports `W3DWater`'s sky (`Water/W3DWater.cpp`): a `new_skybox` W3D model, scaled by `SkyBoxScale`
//! and centred on the camera at `SkyBoxPositionZ`, drawn when `GlobalData.m_drawSkyBox` is set
//! (`GameData.ini` ships `DrawSkyBox = Yes`). The shipped `new_skybox.w3d` carries its own
//! time-of-day face textures (`TSMorningE/N/S/T/W.tga`), so no separate texture selection is
//! needed. The engine draws it in the water render object; the viewer spawns it once, follows the
//! camera horizontally, and renders it unlit with no fog so it stays a clean backdrop.

use bevy::prelude::*;
use zr_assets::Vfs;
use zr_ini::IniDb;

use crate::convert;
use crate::source;

/// `GameData` sky settings (`GlobalData.cpp` field table).
#[derive(Clone, Copy, Debug)]
pub struct SkySettings {
    /// `DrawSkyBox`.
    pub draw: bool,
    /// `SkyBoxPositionZ` (world Z; lowered under the terrain so the horizon sits right).
    pub position_z: f32,
    /// `SkyBoxScale` (tuned per map size in the shipped INI).
    pub scale: f32,
}

impl SkySettings {
    pub fn from_ini(ini: &IniDb) -> Self {
        // `GameData` is split across `Default\GameData.ini` and `Data\INI\GameData.ini`; the engine
        // applies every block in load order, so take the last value set.
        let mut s = SkySettings { draw: false, position_z: -100.0, scale: 8.4 };
        for b in ini.iter().filter(|b| b.kind == "GameData") {
            if let Some(v) = b.value("DrawSkyBox") {
                s.draw = v.trim().eq_ignore_ascii_case("yes") || v.trim() == "1" || v.trim().eq_ignore_ascii_case("true");
            }
            if let Some(v) = b.value("SkyBoxPositionZ").and_then(|v| v.trim().parse().ok()) {
                s.position_z = v;
            }
            if let Some(v) = b.value("SkyBoxScale").and_then(|v| v.trim().parse().ok()) {
                s.scale = v;
            }
        }
        s
    }
}

/// The skybox root (moved to the camera each frame).
#[derive(Component)]
pub struct SkyBoxRoot {
    position_z: f32,
}

/// Loads `new_skybox` and spawns it. Returns `false` when the model is missing (the sky is then the
/// clear colour, as before). `scene_radius` is half the map's playable diagonal.
pub fn spawn(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
    vfs: &Vfs,
    settings: SkySettings,
    scene_radius: f32,
) -> bool {
    if !settings.draw {
        return false;
    }
    let Some(parts) = source::model_by_name(vfs, "new_skybox") else {
        warn!("sky: new_skybox model not found; leaving the sky as the clear colour");
        return false;
    };
    let model_radius = parts
        .iter()
        .flat_map(|p| p.positions.iter())
        .fold(0.0f32, |m, p| m.max(convert::to_bevy(*p).length()));
    if model_radius <= 0.0 {
        return false;
    }
    // The engine's `SkyBoxScale` is tuned for a 96x96 map; compute a scale that encloses this map
    // (and never exceeds the camera far plane, 20000).
    let scale = settings.scale.max(scene_radius * 1.15 / model_radius).min(18000.0 / model_radius);
    let root = commands
        .spawn((SkyBoxRoot { position_z: settings.position_z }, Transform::from_scale(Vec3::splat(scale)), Visibility::default()))
        .id();
    let mut n = 0;
    commands.entity(root).with_children(|c| {
        for p in &parts {
            if p.indices.is_empty() {
                continue;
            }
            let (mesh, mut mat) = convert::model_part(p, images, None);
            mat.unlit = true;
            mat.cull_mode = None;
            mat.double_sided = true;
            mat.fog_enabled = false;
            mat.perceptual_roughness = 1.0;
            c.spawn((Mesh3d(meshes.add(mesh)), MeshMaterial3d(materials.add(mat)), Transform::IDENTITY, bevy::light::NotShadowCaster));
            n += 1;
        }
    });
    info!("sky: new_skybox, {n} parts, model radius {model_radius:.1}, scale {scale:.1}");
    n > 0
}

pub fn add_systems(app: &mut App) {
    app.add_systems(Update, follow_camera);
}

/// Despawns the skybox of the previous world (`OnEnter(Loading)`, before the game's snapshot). The
/// shell map's sky would otherwise stay in the game's pre-game entity set. Its parts are children
/// of the root, so they go with it.
pub fn teardown(world: &mut World) {
    let mut q = world.query_filtered::<Entity, With<SkyBoxRoot>>();
    let ids: Vec<Entity> = q.iter(world).collect();
    for e in ids {
        world.despawn(e);
    }
}

/// Centre the skybox on the camera horizontally (the engine only follows X/Y and pins Z).
fn follow_camera(cam: Query<&Transform, With<Camera3d>>, mut sky: Query<(&SkyBoxRoot, &mut Transform), Without<Camera3d>>) {
    let Ok(cam) = cam.single() else { return };
    for (root, mut t) in &mut sky {
        t.translation = Vec3::new(cam.translation.x, root.position_z, cam.translation.z);
    }
}
