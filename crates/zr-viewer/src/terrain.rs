//! Terrain material: the engine's baked `doTheLight` vertex colours drawn unlit, with the
//! directional light's shadow map applied on top as a darkening factor (`terrain.wgsl`). A mesh
//! with the engine's second UV set (`u2`,`v2`, the blend layer) draws the two terrain passes from
//! the class atlas; meshes without it (roads, the extra blend pass, the heightfield fallback) draw
//! one layer, as the engine's `ST_ROAD_BASE` does.

use bevy::asset::{embedded_asset, embedded_path, AssetPath};
use bevy::pbr::{Material, MaterialPlugin};
use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;

/// `StandardMaterial`-like material for the terrain and road decals.
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct TerrainMaterial {
    #[uniform(0)]
    pub base_color: Vec4,
    #[texture(1)]
    #[sampler(2)]
    pub base_color_texture: Option<Handle<Image>>,
    /// `GlobalLighting::shadow_color` (linear RGBA); shadowed texels darken toward its RGB.
    #[uniform(3)]
    pub shadow_color: Vec4,
    /// `AlphaMode::Opaque` for the terrain, `Blend` for roads.
    pub alpha_mode: AlphaMode,
    /// Depth bias (roads stack; the terrain is 0).
    pub depth_bias: f32,
}

impl Default for TerrainMaterial {
    fn default() -> Self {
        Self { base_color: Vec4::ONE, base_color_texture: None, shadow_color: Vec4::new(0.0, 0.0, 0.0, 0.5), alpha_mode: AlphaMode::Opaque, depth_bias: 0.0 }
    }
}

impl Material for TerrainMaterial {
    fn fragment_shader() -> ShaderRef {
        ShaderRef::Path(AssetPath::from_path_buf(embedded_path!("terrain.wgsl")).with_source("embedded"))
    }
    fn alpha_mode(&self) -> AlphaMode {
        self.alpha_mode
    }
    fn depth_bias(&self) -> f32 {
        self.depth_bias
    }
}

pub struct TerrainMaterialPlugin;

impl Plugin for TerrainMaterialPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "terrain.wgsl");
        app.add_plugins(MaterialPlugin::<TerrainMaterial>::default());
    }
}
