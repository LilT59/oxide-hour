//! Map lighting and the black border shroud.
//!
//! The engine lights the two worlds differently, and so do we:
//!
//! * **Terrain** is lit on the CPU into per-vertex colours
//!   (`BaseHeightMapRenderObjClass::doTheLight`): `ambient[0] + sum_i diffuse[i] * max(0, N . -dir[i])`
//!   over the 3 terrain lights, clamped to 0..1, then modulated with the texture by the fixed-function
//!   pipeline. We do exactly that ([`bake_terrain`]) and render the terrain material `unlit`, so the
//!   baked value is the final shade (no shadow-map, no tone-mapping, as in the original).
//! * **Objects** use the scene ambient (`terrainObjectsLighting[tod][0].ambient`,
//!   `W3DDisplay::setTimeOfDay`) plus up to 3 directional lights from `terrainObjectsLighting[tod][0..3]`
//!   ([`spawn_object_lights`]). The camera gets `Tonemapping::None` so the result is not remapped.
//!
//! The time of day comes from the map's `GlobalLighting` chunk (`m_timeOfDay`).
//!
//! The original works in gamma space (D3D8 fixed function). Bevy lights in linear space, so light
//! colours are converted sRGB -> linear: pure ambient or fully-lit surfaces then come out identical to
//! the original after the final sRGB encode; partially lit ones are an approximation.
//!
//! **Border**: the original covers everything outside the playable rectangle with black shroud. We fade
//! terrain vertex colours to black over [`FADE_WIDTH`] world units past the playable edge ([`edge_fade`]).

use bevy::light::{CascadeShadowConfigBuilder, GlobalAmbientLight};
use bevy::prelude::*;
use zr_map::{GlobalLight, GlobalLighting, MapFile};

use crate::convert::to_bevy;
use crate::source::{Ground, TerrainData};

/// World units over which the terrain fades to black outside the playable rectangle.
pub const FADE_WIDTH: f32 = 30.0;

/// Fallback when the map has no `GlobalLighting` chunk: a plain afternoon sun.
fn fallback() -> GlobalLighting {
    let mut l = GlobalLighting {
        version: 3,
        time_of_day: 2,
        terrain: [[GlobalLight::default(); 3]; 4],
        objects: [[GlobalLight::default(); 3]; 4],
        shadow_color: None,
    };
    let sun = GlobalLight { ambient: [0.5; 3], diffuse: [1.0, 1.0, 0.8], direction: [0.7, 0.2, -0.7] };
    l.terrain[1][0] = sun;
    l.objects[1][0] = sun;
    l
}

/// The map's lighting (or [`fallback`]).
pub fn map_lighting(map: &MapFile) -> GlobalLighting {
    map.lighting.clone().unwrap_or_else(fallback)
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Distance outside the playable rectangle `[0, pw] x [0, ph]` (0 inside).
fn outside_distance(x: f32, y: f32, pw: f32, ph: f32) -> f32 {
    let dx = (-x).max(x - pw).max(0.0);
    let dy = (-y).max(y - ph).max(0.0);
    (dx * dx + dy * dy).sqrt()
}

/// 1.0 inside the playable area, falling to 0.0 (black shroud) `FADE_WIDTH` units outside it.
pub fn edge_fade(x: f32, y: f32, pw: f32, ph: f32) -> f32 {
    let t = 1.0 - (outside_distance(x, y, pw, ph) / FADE_WIDTH).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// `doTheLight` without dynamic lights / water depth fade. Result is the gamma-space shade (0..1).
pub fn terrain_shade(normal: [f32; 3], lights: &[GlobalLight; 3]) -> [f32; 3] {
    let mut s = lights[0].ambient; // only the first terrain light contributes ambient
    for l in lights {
        let ray = [-l.direction[0], -l.direction[1], -l.direction[2]];
        let k = dot(ray, normal).clamp(0.0, 1.0);
        for (sc, d) in s.iter_mut().zip(l.diffuse) {
            *sc += k * d;
        }
    }
    s.map(|v| v.clamp(0.0, 1.0))
}

/// Bakes the map's terrain lighting (`doTheLight`) and the border shroud into `t.colors`
/// (multiplying any existing colours, e.g. the heightfield fallback's). The terrain is drawn
/// **unlit** with these colours, exactly as before; shadow maps are applied on top by the terrain
/// material (`terrain.wgsl`) so unshadowed pixels stay byte-identical. The vertex alpha is the
/// blend layer's per-vertex alpha: `doTheLight` packs it into the diffuse alpha of the engine's
/// terrain vertices (`BaseHeightMap.cpp:618-635`), and the blend pass multiplies its texture alpha
/// by it (`W3DShaderManager.cpp` `TerrainShader2Stage::set` pass 1).
pub fn bake_terrain(t: &mut TerrainData, lighting: &GlobalLighting, playable: (f32, f32)) {
    let lights = &lighting.terrain[lighting.tod_index()];
    let (pw, ph) = playable;
    let old = t.colors.take();
    let blend = &t.blend_alpha;
    let colors = t
        .positions
        .iter()
        .zip(&t.normals)
        .enumerate()
        .map(|(i, (p, n))| {
            let shade = terrain_shade(*n, lights);
            let fade = edge_fade(p[0], p[1], pw, ph);
            let base = old.as_ref().map_or([1.0; 4], |c| c[i]);
            let alpha = blend.get(i).map_or(1.0, |&a| f32::from(a) / 255.0);
            [
                srgb_to_linear(shade[0]) * fade * base[0],
                srgb_to_linear(shade[1]) * fade * base[1],
                srgb_to_linear(shade[2]) * fade * base[2],
                base[3] * alpha,
            ]
        })
        .collect();
    t.colors = Some(colors);
    if let Some(e) = t.extra.as_mut() {
        // The extra pass' `getStaticDiffuse` has alpha 255; the cell's three-layer composite is
        // already in the atlas block, so the quad itself is opaque.
        e.colors = e
            .positions
            .iter()
            .zip(&e.normals)
            .map(|(p, n)| {
                let shade = terrain_shade(*n, lights);
                let fade = edge_fade(p[0], p[1], pw, ph);
                [srgb_to_linear(shade[0]) * fade, srgb_to_linear(shade[1]) * fade, srgb_to_linear(shade[2]) * fade, 1.0]
            })
            .collect();
    }
}

/// Per-vertex colours for decals lying on the terrain (roads): the terrain's baked shade at
/// each point, using the heightfield normal there, times the border fade. The original colours
/// road vertices with the terrain's static diffuse (`RoadSegment::updateSegLighting`).
pub fn ground_decal_colors(positions: &[[f32; 3]], ground: &Ground, lighting: &GlobalLighting) -> Vec<[f32; 4]> {
    const D: f32 = 5.0;
    let lights = &lighting.terrain[lighting.tod_index()];
    let (pw, ph) = ground.playable_size();
    positions
        .iter()
        .map(|&[x, y, _]| {
            let dx = (ground.height(x + D, y) - ground.height(x - D, y)) / (2.0 * D);
            let dy = (ground.height(x, y + D) - ground.height(x, y - D)) / (2.0 * D);
            let len = (dx * dx + dy * dy + 1.0).sqrt();
            let shade = terrain_shade([-dx / len, -dy / len, 1.0 / len], lights);
            let fade = edge_fade(x, y, pw, ph);
            [srgb_to_linear(shade[0]) * fade, srgb_to_linear(shade[1]) * fade, srgb_to_linear(shade[2]) * fade, 1.0]
        })
        .collect()
}

/// Bevy's ambient term for a diffuse surface is attenuated by `EnvBRDFApprox` at roughness 1
/// (`F_AB.x` = 0.4324), so ambient brightness is scaled up to compensate.
const AMBIENT_COMPENSATION: f32 = 1.0 / 0.4324;

/// The map's `GlobalLighting::shadow_color` (ARGB) as a linear RGBA for `TerrainMaterial`.
/// Defaults to the engine's `W3DShadowManager` grey when the chunk carries none.
pub fn shadow_color_linear(lighting: &GlobalLighting) -> Vec4 {
    // `ZR_NO_SHADOW=1` disables the terrain shadow darkening (alpha 0) so the baked `doTheLight`
    // render can be captured for the before/after pixel diff.
    if std::env::var_os("ZR_NO_SHADOW").is_some() {
        return Vec4::ZERO;
    }
    let v = lighting.shadow_color.unwrap_or(0x7fa0a0a0);
    let a = ((v >> 24) & 0xff) as f32 / 255.0;
    Vec4::new(
        srgb_to_linear(((v >> 16) & 0xff) as f32 / 255.0),
        srgb_to_linear(((v >> 8) & 0xff) as f32 / 255.0),
        srgb_to_linear((v & 0xff) as f32 / 255.0),
        a,
    )
}

fn linear(c: [f32; 3]) -> Vec3 {
    Vec3::new(srgb_to_linear(c[0]), srgb_to_linear(c[1]), srgb_to_linear(c[2]))
}

/// Inverse of the camera exposure, to convert "final radiance" into Bevy light units.
fn inv_exposure() -> f32 {
    1.0 / bevy::camera::Exposure::default().exposure()
}

/// Sets the scene ambient and spawns the object directional lights for the map's time of day.
///
/// Object colours are unchanged from before the shadow work: the ambient is the object light's own
/// ambient at `AMBIENT_COMPENSATION`. The first light still casts shadow maps; the terrain samples
/// them in `shadow_terrain.wgsl` (darkening by `shadow_color`), and the shadowed object surfaces are
/// darkened by the map's ambient as before.
pub fn spawn_object_lights(commands: &mut Commands, ambient: &mut GlobalAmbientLight, lighting: &GlobalLighting) {
    let lights = &lighting.objects[lighting.tod_index()];
    let amb = linear(lights[0].ambient);
    let k = inv_exposure();
    ambient.color = Color::linear_rgb(amb.x, amb.y, amb.z);
    ambient.brightness = k * AMBIENT_COMPENSATION;
    let mut first = true;
    for l in lights {
        let d = linear(l.diffuse);
        let strength = d.max_element();
        if strength <= 0.0 {
            continue;
        }
        let dir = to_bevy(l.direction).normalize_or_zero();
        if dir == Vec3::ZERO {
            continue;
        }
        let c = d / strength;
        // Lambert: radiance = albedo / pi * E * cos * exposure  ->  E = diffuse * pi / exposure.
        let mut e = commands.spawn((
            DirectionalLight {
                illuminance: strength * std::f32::consts::PI * k,
                color: Color::linear_rgb(c.x, c.y, c.z),
                shadow_maps_enabled: first,
                shadow_depth_bias: 0.02,
                shadow_normal_bias: 1.4,
                ..default()
            },
            Transform::IDENTITY.looking_to(dir, if dir.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y }),
        ));
        if first {
            // Crisp near the possession camera; the engine's volumes are sharp, not soft.
            e.insert(CascadeShadowConfigBuilder { num_cascades: 4, maximum_distance: 1500.0, first_cascade_far_bound: 120.0, ..default() }.build());
            first = false;
        }
    }
}
