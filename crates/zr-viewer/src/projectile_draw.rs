//! Client draw of projectile streams (`W3DProjectileStreamDraw`), laser beams (`W3DLaserDraw` with the
//! client `LaserUpdate`) and their maths. The sim provides read-only records
//! (`ProjectileStreamSnapshot`, `LaserBeam`); nothing here writes sim state.
//!
//! Both draws are `SegmentedLineClass` lines (`W3DDevice/GameClient/Drawable/Draw`), rendered by
//! `SegLineRendererClass::Render` (`WWVegas/WW3D2/seglinerenderer.cpp`): a strip camera-facing along the
//! line, `Width` across, with the texture's U running across the strip (0 on one edge, 1 on the other)
//! and V running along the points (`point index * TextureTileFactor`), offset by the scroll rate. The
//! ribbon here is the same silhouette without the noise and subdivision (both are off for these lines).
//! Mitred joins are approximated by the average tangent; the original intersects the edge planes.
//!
//! The tracer (`W3DTracerDraw`) is in this file too; `fx.rs` owns the FX nugget that spawns it.

use std::collections::{HashMap, HashSet};

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use crate::anim::AnimRig;
use crate::convert;
use crate::sim_view::{ModelCache, SimObject, SimRes};

/// `LOGICFRAMES_PER_SECOND`: the sim's frame rate, which the scroll rates are per second of.
const FRAMES_PER_SECOND: f32 = 30.0;
/// The clearance a laser arc keeps above the ground (`2.0 + getGroundHeight`, W3DLaserDraw.cpp:359-362).
const ARC_GROUND_CLEARANCE: f32 = 2.0;

// ---- template data (INI fields of the W3D draw modules) ------------------------------------------

/// `W3DProjectileStreamDrawModuleData`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StreamDraw {
    pub texture: String,
    pub width: f32,
    pub tile_factor: f32,
    /// `ScrollRate`: V units per second.
    pub scroll_rate: f32,
    /// `MaxSegments`: only the newest this many points are drawn (0 = all).
    pub max_segments: u32,
}

/// `W3DLaserDrawModuleData`.
#[derive(Clone, Debug, PartialEq)]
pub struct LaserDraw {
    pub texture: String,
    pub num_beams: u32,
    pub inner_width: f32,
    pub outer_width: f32,
    /// RGBA in 0..1 (the INI `A` channel, `GameGetColorComponentsReal`).
    pub inner_color: [f32; 4],
    pub outer_color: [f32; 4],
    pub tile: bool,
    pub scroll_rate: f32,
    pub segments: u32,
    pub segment_overlap_ratio: f32,
    pub tiling_scalar: f32,
    pub arc_height: f32,
}

impl Default for LaserDraw {
    /// The `W3DLaserDrawModuleData` constructor defaults (W3DLaserDraw.cpp:60-73).
    fn default() -> Self {
        LaserDraw {
            texture: String::new(),
            num_beams: 1,
            inner_width: 0.0,
            outer_width: 1.0,
            inner_color: [0.0; 4],
            outer_color: [0.0; 4],
            tile: false,
            scroll_rate: 0.0,
            segments: 1,
            segment_overlap_ratio: 0.0,
            tiling_scalar: 1.0,
            arc_height: 0.0,
        }
    }
}

/// `R:255 G:0 B:255 A:150` -> RGBA in 0..1 (`GameGetColorComponentsReal`). Missing channels are 0.
pub fn parse_rgba(s: &str) -> [f32; 4] {
    let mut out = [0.0f32; 4];
    for tok in s.split_whitespace() {
        let Some((k, v)) = tok.split_once(':') else { continue };
        let Ok(val) = v.trim().parse::<f32>() else { continue };
        let i = match k.to_ascii_uppercase().as_str() {
            "R" => 0,
            "G" => 1,
            "B" => 2,
            "A" => 3,
            _ => continue,
        };
        out[i] = val / 255.0;
    }
    out
}

/// INI booleans (`INI::parseBool`: `Yes` / `No`).
fn parse_bool(s: &str) -> bool {
    matches!(s.trim().to_ascii_lowercase().as_str(), "yes" | "true" | "1")
}

/// Reads `W3DProjectileStreamDraw` from a stream object's template.
pub fn stream_draw_from(fields: &[zr_templates::diag::RawItem]) -> StreamDraw {
    use zr_templates::diag::raw_value as get;
    let f = |k: &str| get(fields, k).and_then(|v| v.trim().parse::<f32>().ok());
    StreamDraw {
        texture: get(fields, "Texture").unwrap_or("").trim().to_owned(),
        width: f("Width").unwrap_or(0.0),
        tile_factor: f("TileFactor").unwrap_or(0.0),
        scroll_rate: f("ScrollRate").unwrap_or(0.0),
        max_segments: get(fields, "MaxSegments").and_then(|v| v.trim().parse::<u32>().ok()).unwrap_or(0),
    }
}

/// Reads `W3DLaserDraw` from a laser object's template.
pub fn laser_draw_from(fields: &[zr_templates::diag::RawItem]) -> LaserDraw {
    use zr_templates::diag::raw_value as get;
    let d = LaserDraw::default();
    let f = |k: &str, def: f32| get(fields, k).and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(def);
    LaserDraw {
        texture: get(fields, "Texture").unwrap_or("").trim().to_owned(),
        num_beams: get(fields, "NumBeams").and_then(|v| v.trim().parse::<u32>().ok()).unwrap_or(d.num_beams),
        inner_width: f("InnerBeamWidth", d.inner_width),
        outer_width: f("OuterBeamWidth", d.outer_width),
        inner_color: get(fields, "InnerColor").map(parse_rgba).unwrap_or(d.inner_color),
        outer_color: get(fields, "OuterColor").map(parse_rgba).unwrap_or(d.outer_color),
        tile: get(fields, "Tile").is_some_and(parse_bool),
        scroll_rate: f("ScrollRate", d.scroll_rate),
        segments: get(fields, "Segments").and_then(|v| v.trim().parse::<u32>().ok()).unwrap_or(d.segments).max(1),
        segment_overlap_ratio: f("SegmentOverlapRatio", d.segment_overlap_ratio),
        tiling_scalar: f("TilingScalar", d.tiling_scalar),
        arc_height: f("ArcHeight", d.arc_height),
    }
}

// ---- geometry maths (pure; unit-tested) ----------------------------------------------------------

/// `W3DProjectileStreamDraw::doDrawModule` (W3DProjectileStreamDraw.cpp:151-180): the stream's points
/// (oldest first, holes and dead projectiles as the zero vector) become the lines to draw. Only the
/// newest `max_segments` points are considered; a hole breaks the line; a line needs two points.
pub fn stream_lines(points: &[[f32; 3]], max_segments: u32) -> Vec<Vec<[f32; 3]>> {
    let used = points.len();
    let mut current = if max_segments > 0 { used.saturating_sub(max_segments as usize) } else { 0 };
    let mut lines = Vec::new();
    let mut staging: Vec<[f32; 3]> = Vec::new();
    while current < used {
        while current < used && points[current] != [0.0, 0.0, 0.0] {
            staging.push(points[current]);
            current += 1;
        }
        if staging.len() > 1 {
            lines.push(std::mem::take(&mut staging));
        }
        // The point we stopped at is a hole (or past the end): skip it.
        current += 1;
        staging.clear();
    }
    lines
}

/// The per-point texture V of a tiled line: `point index * TextureTileFactor` plus the scroll offset
/// (`SegLineRendererClass::Render`, seglinerenderer.cpp:232-239 and 316-321).
pub fn tiled_v(count: usize, tile: f32, offset: f32) -> Vec<f32> {
    (0..count).map(|i| i as f32 * tile + offset).collect()
}

/// The scroll offset after `seconds` of `rate` V units per second, wrapped to `[0, 1)`: the segline keeps
/// `CurrentUVOffset + UVOffsetDeltaPerMS * delta` and takes its fraction (seglinerenderer.cpp:232-239).
pub fn scroll_offset(rate: f32, seconds: f32) -> f32 {
    let x = rate * seconds;
    x - x.floor()
}

/// `SegLineRendererClass::Set_Texture_Tile_Factor` (seglinerenderer.cpp:187-199): the factor is clamped to
/// `[0, 50]`, so a negative `TilingScalar` (the Avenger and Lazr beams use -3) gives no tiling at all.
pub fn segline_tile_factor(f: f32) -> f32 {
    if f > 50.0 {
        50.0
    } else {
        f.max(0.0)
    }
}

/// `W3DLaserDraw`'s tile factor for a beam of `length` and `width`: the texture is not stretched
/// (W3DLaserDraw.cpp:409-410). A zero width is clamped so the factor stays finite.
pub fn laser_tile_factor(length: f32, width: f32, texture_aspect: f32, tiling_scalar: f32) -> f32 {
    length / width.max(1e-4) * texture_aspect * tiling_scalar
}

/// The line's segments (W3DLaserDraw.cpp:274-370). With an arc (`ArcHeight > 0` and more than one
/// segment) each segment is lifted along a cosine from the line's middle, never below the ground plus
/// 2; otherwise every segment is the whole line. `ground(x, y)` is the terrain height.
pub fn laser_segments(start: [f32; 3], end: [f32; 3], segments: u32, arc_height: f32, overlap: f32, ground: impl Fn(f32, f32) -> f32) -> Vec<[[f32; 3]; 2]> {
    let segments = segments.max(1);
    if !(arc_height > 0.0 && segments > 1) {
        return (0..segments).map(|_| [start, end]).collect();
    }
    let v = [end[0] - start[0], end[1] - start[1], end[2] - start[2]];
    let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    let middle = [(start[0] + end[0]) * 0.5, (start[1] + end[1]) * 0.5, (start[2] + end[2]) * 0.5];
    let half = length * 0.5;
    let n = segments as f32;
    let lift = |p: [f32; 3]| -> [f32; 3] {
        let d = [middle[0] - p[0], middle[1] - p[1], middle[2] - p[2]];
        let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        // A zero-length line has no middle to lift from: it stays level.
        let h = if half > 0.0 { (dist / half * std::f32::consts::PI * 0.5).cos() * arc_height } else { 0.0 };
        [p[0], p[1], p[2] + h]
    };
    (0..segments)
        .map(|s| {
            let s = s as f32;
            let mut r0 = s / n;
            let mut r1 = (s + 1.0) / n;
            if s > 0.0 {
                r0 -= overlap;
            }
            if s < n - 1.0 {
                r1 += overlap;
            }
            let at = |r: f32| [start[0] + v[0] * r, start[1] + v[1] * r, start[2] + v[2] * r];
            let a = lift(at(r0));
            let b = lift(at(r1));
            let clamp = |p: [f32; 3]| [p[0], p[1], p[2].max(ARC_GROUND_CLEARANCE + ground(p[0], p[1]))];
            [clamp(a), clamp(b)]
        })
        .collect()
}

/// One beam's width and colour (W3DLaserDraw.cpp:159-198). `i` is the beam's index as the constructor
/// loops it (`0..num_beams`); beams are spread from the inner width and colour to the outer ones.
/// The colour is premultiplied by the inner alpha, as the constructor does (it is additive).
pub fn beam_look(i: u32, num_beams: u32, inner_width: f32, outer_width: f32, inner: [f32; 4], outer: [f32; 4]) -> (f32, [f32; 3]) {
    if num_beams <= 1 {
        return (inner_width, [inner[0] * inner[3], inner[1] * inner[3], inner[2] * inner[3]]);
    }
    let scale = i as f32 / (num_beams as f32 - 1.0);
    let width = inner_width + scale * (outer_width - inner_width);
    let c = |k: usize| inner[k] + scale * (outer[k] - inner[k]) * inner[3];
    (width, [c(0), c(1), c(2)])
}

// ---- strips ------------------------------------------------------------------------------------

/// A camera-facing textured strip, ready for a Bevy mesh.
#[derive(Default, Debug, PartialEq)]
pub struct Strip {
    pub positions: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub colors: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
}

/// The strip along `pts` (Bevy space), `width` across as seen from `eye`, with the texture V of point `i`
/// given by `v[i]` (`SegLineRendererClass::Render`: the edge planes are the silhouette of a cylinder of
/// radius `width / 2` from the eye, so the side vector is `tangent x (eye - point)`).
pub fn ribbon(pts: &[Vec3], v: &[f32], width: f32, eye: Vec3, color: [f32; 4]) -> Strip {
    let n = pts.len();
    let mut out = Strip::default();
    if n < 2 || v.len() < n {
        return out;
    }
    let mut prev_t = Vec3::X;
    for i in 0..n {
        let t = match i {
            0 => pts[1] - pts[0],
            _ if i == n - 1 => pts[n - 1] - pts[n - 2],
            _ => pts[i + 1] - pts[i - 1],
        };
        let t = if t.length_squared() > 0.0 { t.normalize() } else { prev_t };
        prev_t = t;
        let mut side = t.cross(eye - pts[i]);
        if side.length_squared() <= 1e-12 {
            // Looking straight down the line: any perpendicular will do.
            side = t.cross(Vec3::Y);
            if side.length_squared() <= 1e-12 {
                side = t.cross(Vec3::Z);
            }
        }
        let side = side.normalize_or_zero() * (width * 0.5);
        for (u, p) in [(0.0f32, pts[i] + side), (1.0, pts[i] - side)] {
            out.positions.push(p.to_array());
            out.uvs.push([u, v[i]]);
            out.colors.push(color);
        }
    }
    for i in 0..(n - 1) as u32 {
        let a = 2 * i;
        out.indices.extend_from_slice(&[a, a + 1, a + 2, a + 1, a + 3, a + 2]);
    }
    out
}

fn strip_mesh(s: &Strip) -> Mesh {
    let normals = vec![[0.0f32, 1.0, 0.0]; s.positions.len()];
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, s.positions.clone())
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, s.uvs.clone())
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, s.colors.clone())
        .with_inserted_indices(Indices::U32(s.indices.clone()))
}

// ---- tracers -------------------------------------------------------------------------------------

/// `W3DTracerDraw`'s state for one tracer drawable: the line from `origin` along `dir`, `length` long,
/// moving `speed` per frame, and fading to nothing at `expire` (the drawable's expiration date, relative
/// to its creation).
#[derive(Component, Clone, Debug, PartialEq)]
pub struct TracerView {
    pub origin: [f32; 3],
    pub dir: [f32; 3],
    /// Distance per logic frame (`m_speedInDistPerFrame`).
    pub speed: f32,
    pub length: f32,
    pub width: f32,
    pub color: [f32; 3],
    /// `m_opacity`: starts at 1.
    pub opacity: f32,
    pub age: u32,
    pub expire: u32,
    /// The logic frame this tracer has been stepped to (`None` until the first sync).
    pub synced: Option<u32>,
}

impl TracerView {
    /// Catches the tracer up to the sim's logic `frame`, one [`step`](Self::step) per logic frame that has
    /// passed. C++ `W3DTracerDraw::doDrawModule` runs once per draw, and `GameEngine::update` draws and runs
    /// the logic once per loop (GameEngine.cpp:745-770), so the two rates match there. A render frame with no
    /// new logic frame (a paused sim, or the sim idle between frames) does not move the tracer. A new tracer
    /// takes one step on its first sync, as the original's first draw does (it moves in the same draw).
    /// Deviation: C++ still draws while the game is paused (`GameEngine::update` skips only the logic), so a
    /// paused original tracer keeps flying; here it holds with the sim (the logic clock is the only clock).
    pub fn advance_to(&mut self, frame: u32) {
        let from = self.synced.unwrap_or(frame.saturating_sub(1));
        for _ in from..frame {
            if self.expired() {
                break;
            }
            self.step();
        }
        self.synced = Some(frame.max(from));
    }

    /// One logic frame of `W3DTracerDraw::doDrawModule` (W3DTracerDraw.cpp:136-150): the opacity drops by
    /// `opacity / frames left` (so it reaches zero at the expiry), then the line moves `speed` along its
    /// direction.
    pub fn step(&mut self) {
        let left = self.expire.saturating_sub(self.age);
        if left > 0 {
            self.opacity -= self.opacity / left as f32;
        } else {
            self.opacity = 0.0;
        }
        if self.speed != 0.0 {
            for k in 0..3 {
                self.origin[k] += self.dir[k] * self.speed;
            }
        }
        self.age += 1;
    }

    /// The drawable is destroyed once its expiration frame is reached.
    pub fn expired(&self) -> bool {
        self.age >= self.expire
    }

    /// The tracer's segment: from `origin` along `dir` for `length`, in Generals space.
    pub fn segment(&self) -> [[f32; 3]; 2] {
        let end = [0, 1, 2].map(|k| self.origin[k] + self.dir[k] * self.length);
        [self.origin, end]
    }
}

/// `TracerFXNugget::doFXPos` (FXList.cpp:169-215) and `W3DTracerDraw::setTracerParms`: a tracer from `pos`
/// toward `target`, fired at `speed` (the nugget's `Speed` in distance per second, or the weapon's speed
/// per frame when that is zero). Its expiry is the frames to reach the target less the tracer's length,
/// times `DecayAt`, as the nugget computes it.
#[allow(clippy::too_many_arguments)]
pub fn spawn_tracer(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    pos: [f32; 3],
    target: [f32; 3],
    speed_per_sec: f32,
    weapon_speed: f32,
    decay_at: f32,
    length: f32,
    width: f32,
    color: zr_fx::Color,
) {
    let d = [target[0] - pos[0], target[1] - pos[1], target[2] - pos[2]];
    let dist3 = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    let dir = if dist3 > 0.0 { [d[0] / dist3, d[1] / dist3, d[2] / dist3] } else { [1.0, 0.0, 0.0] };
    let (speed, expire) = tracer_timing(dist3, length, speed_per_sec, weapon_speed, decay_at);
    let view = TracerView { origin: pos, dir, speed, length, width, color: [color.r, color.g, color.b], opacity: 1.0, age: 0, expire, synced: None };
    // The first strip is non-empty: an empty `Mesh` asset logs bevy's slab-allocator "Use-after-free" once per
    // extraction (see `FxSystem::mesh` in fx.rs). `sync_tracers` redraws it camera-facing on the next frame.
    let seg = view.segment();
    let world = [convert::to_bevy(seg[0]), convert::to_bevy(seg[1])];
    let first = ribbon(&world, &[0.0, 0.0], width, world[0] + Vec3::Y, [view.color[0], view.color[1], view.color[2], 1.0]);
    let material = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        unlit: true,
        alpha_mode: AlphaMode::Add,
        cull_mode: None,
        double_sided: true,
        ..default()
    });
    commands.spawn((view, Mesh3d(meshes.add(strip_mesh(&first))), MeshMaterial3d(material), Transform::IDENTITY, Visibility::default(), bevy::light::NotShadowCaster));
}

/// The tracer's speed per frame and its expiry in frames (FXList.cpp:179-202). The INI `Speed` is per
/// second (`ConvertVelocityInSecsToFrames`); a zero speed takes the weapon's (`primarySpeed`). The flight is
/// `dist - length` frames at that speed, scaled by `DecayAt` and rounded up.
pub fn tracer_timing(dist3: f32, length: f32, speed_per_sec: f32, weapon_speed: f32, decay_at: f32) -> (f32, u32) {
    let speed = if speed_per_sec == 0.0 { weapon_speed } else { speed_per_sec / FRAMES_PER_SECOND };
    let dist = dist3 - length;
    let frames = if dist >= 0.0 && speed >= 0.0 { dist / speed } else { 1.0 };
    let expire = (frames * decay_at).ceil();
    // A non-finite expiry (zero speed) is capped; the original's integer conversion is undefined there.
    let expire = if expire.is_finite() { expire.clamp(0.0, 1.0e6) as u32 } else { 0 };
    (speed, expire)
}

/// Steps every tracer to the sim's logic frame, removes the expired ones and redraws the rest camera-facing.
/// The tracer clock is the sim's frame counter, not wall time, so a paused or stepped-by-frame sim moves
/// the tracers exactly as many times as it advances.
fn sync_tracers(
    mut commands: Commands,
    sim: Option<NonSend<SimRes>>,
    cameras: Query<&GlobalTransform, With<Camera3d>>,
    mut tracers: Query<(Entity, &mut TracerView, &Mesh3d)>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let frame = sim.as_ref().map_or(0, |s| s.game.frame());
    for (_, mut t, _) in &mut tracers {
        t.advance_to(frame);
    }
    let Ok(cam) = cameras.single() else { return };
    let eye = cam.translation();
    for (e, t, m) in &tracers {
        if t.expired() {
            commands.entity(e).despawn();
            continue;
        }
        let seg = t.segment();
        let world = [convert::to_bevy(seg[0]), convert::to_bevy(seg[1])];
        let o = t.opacity;
        let color = [t.color[0] * o, t.color[1] * o, t.color[2] * o, 1.0];
        let strip = ribbon(&world, &[0.0, 0.0], t.width, eye, color);
        if let Some(mut mesh) = meshes.get_mut(&m.0) {
            *mesh = strip_mesh(&strip);
        }
    }
}

// ---- Bevy systems --------------------------------------------------------------------------------

/// A stream line entity: `(stream id, line index)`.
#[derive(Component)]
struct StreamLineView {
    stream: u32,
    line: usize,
}

/// A laser line entity: `(beam id, segment * num_beams + beam index)`, as `W3DLaserDraw` indexes `m_line3D`.
#[derive(Component)]
struct LaserLineView {
    beam: u32,
    index: usize,
}

#[derive(Resource, Default)]
struct DrawAssets {
    streams: HashMap<String, Option<StreamDraw>>,
    lasers: HashMap<String, Option<LaserDraw>>,
    /// Texture name -> (image handle, width / height).
    textures: HashMap<String, Option<(Handle<Image>, f32)>>,
    /// Texture name -> additive material.
    materials: HashMap<String, Handle<StandardMaterial>>,
    /// Last known start of each beam (`LaserUpdate::m_startPos` keeps its value when the parent is gone).
    last_start: HashMap<u32, [f32; 3]>,
}

pub fn add_systems(app: &mut App) {
    app.init_resource::<DrawAssets>();
    app.add_systems(Update, sync_projectile_draws.run_if(resource_exists::<crate::sim_view::SimActive>));
    app.add_systems(Update, sync_tracers);
}

/// Loads the texture with its family's sampler ([`TextureUse::Laser`]: the engine's default filter and repeat
/// addressing, `W3DLaserDraw.cpp:124` / `W3DProjectileStreamDraw.cpp:94` `Get_Texture(name)`), returning its
/// handle and aspect ratio.
fn texture(assets: &mut DrawAssets, vfs: &zr_assets::Vfs, images: &mut Assets<Image>, name: &str) -> Option<(Handle<Image>, f32)> {
    if name.is_empty() {
        return None;
    }
    assets
        .textures
        .entry(name.to_owned())
        .or_insert_with(|| {
            let t = zr_w3d::load_texture(vfs, name).ok()?;
            let aspect = t.width as f32 / t.height.max(1) as f32;
            let img = convert::texture_image(&t, convert::TextureUse::Laser);
            Some((images.add(img), aspect))
        })
        .clone()
}

/// The additive, unlit, double-sided material of a line (`_PresetAdditiveSpriteShader`): the vertex colour
/// multiplies the texture, and additive blending means no alpha is needed.
fn material(assets: &mut DrawAssets, materials: &mut Assets<StandardMaterial>, name: &str, tex: Option<Handle<Image>>) -> Handle<StandardMaterial> {
    assets
        .materials
        .entry(name.to_owned())
        .or_insert_with(|| {
            materials.add(StandardMaterial {
                base_color: Color::WHITE,
                base_color_texture: tex,
                unlit: true,
                alpha_mode: AlphaMode::Add,
                cull_mode: None,
                double_sided: true,
                ..default()
            })
        })
        .clone()
}

/// The fields of the first `Unparsed` module of class `module` on template `thing`.
fn module_fields(sim: &SimRes, thing: &str, module: &str) -> Option<Vec<zr_templates::diag::RawItem>> {
    let t = sim.game.data().templates().thing(thing)?;
    t.modules.iter().find_map(|m| match &m.data {
        zr_templates::ModuleData::Unparsed { module_type, fields, .. } if module_type.eq_ignore_ascii_case(module) => Some(fields.clone()),
        _ => None,
    })
}

/// A bone's world position for a laser's start: the source's rig joint, as `drain_fx` resolves
/// `FXListAtBonePos` bones (the first match of the unadorned or `01`-suffixed name).
fn bone_world(rig: &AnimRig, root: &Transform, joints: &Query<&Transform, (Without<SimObject>, Without<Camera3d>)>, bone: &str) -> Option<[f32; 3]> {
    let i = *rig.bone_indices_named(bone).first()?;
    let e = rig.joint(i)?;
    let t = joints.get(e).ok()?;
    Some(convert::from_bevy(root.mul_transform(*t).translation))
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn sync_projectile_draws(
    mut commands: Commands,
    sim: NonSend<SimRes>,
    cache: Res<ModelCache>,
    map: Res<crate::MapRes>,
    mut assets: ResMut<DrawAssets>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    cameras: Query<&GlobalTransform, With<Camera3d>>,
    rigs: Query<(&SimObject, &Transform, Option<&AnimRig>)>,
    joints: Query<&Transform, (Without<SimObject>, Without<Camera3d>)>,
    stream_views: Query<(Entity, &StreamLineView, &Mesh3d)>,
    laser_views: Query<(Entity, &LaserLineView, &Mesh3d)>,
) {
    let Ok(cam) = cameras.single() else { return };
    let eye = cam.translation();
    let seconds = sim.game.frame() as f32 / FRAMES_PER_SECOND;

    // ---- projectile streams -------------------------------------------------------------------
    let mut have_stream: HashMap<(u32, usize), (Entity, Handle<Mesh>)> = HashMap::new();
    for (e, v, m) in &stream_views {
        have_stream.insert((v.stream, v.line), (e, m.0.clone()));
    }
    let mut want_stream: HashSet<(u32, usize)> = HashSet::new();
    for s in sim.game.projectile_streams() {
        let draw = match assets.streams.get(&s.template_name).cloned() {
            Some(d) => d,
            None => {
                let d = module_fields(&sim, &s.template_name, "W3DProjectileStreamDraw").map(|f| stream_draw_from(&f));
                assets.streams.insert(s.template_name.clone(), d.clone());
                d
            }
        };
        let Some(draw) = draw else { continue };
        let pts: Vec<[f32; 3]> = s.points.iter().map(|p| [p.x, p.y, p.z]).collect();
        let offset = scroll_offset(draw.scroll_rate, seconds);
        let tex = texture(&mut assets, &cache.vfs, &mut images, &draw.texture);
        let mat = material(&mut assets, &mut materials, &draw.texture, tex.map(|t| t.0));
        for (li, line) in stream_lines(&pts, draw.max_segments).into_iter().enumerate() {
            let world: Vec<Vec3> = line.iter().map(|&p| convert::to_bevy(p)).collect();
            let v = tiled_v(line.len(), segline_tile_factor(draw.tile_factor), offset);
            let strip = ribbon(&world, &v, draw.width, eye, [1.0; 4]);
            let key = (s.id, li);
            want_stream.insert(key);
            match have_stream.get(&key) {
                Some((e, handle)) => {
                    if let Some(mut mesh) = meshes.get_mut(handle) {
                        *mesh = strip_mesh(&strip);
                    }
                    commands.entity(*e).insert(MeshMaterial3d(mat.clone()));
                }
                None => {
                    commands.spawn((
                        StreamLineView { stream: s.id, line: li },
                        Mesh3d(meshes.add(strip_mesh(&strip))),
                        MeshMaterial3d(mat.clone()),
                        Transform::IDENTITY,
                        Visibility::default(),
                        bevy::light::NotShadowCaster,
                    ));
                }
            }
        }
    }
    for (e, v, _) in &stream_views {
        if !want_stream.contains(&(v.stream, v.line)) {
            commands.entity(e).despawn();
        }
    }

    // ---- laser beams --------------------------------------------------------------------------
    let mut have_laser: HashMap<(u32, usize), (Entity, Handle<Mesh>)> = HashMap::new();
    for (e, v, m) in &laser_views {
        have_laser.insert((v.beam, v.index), (e, m.0.clone()));
    }
    let mut want_laser: HashSet<(u32, usize)> = HashSet::new();
    let ground = map.ground();
    for beam in sim.game.lasers() {
        let draw = match assets.lasers.get(&beam.template_name).cloned() {
            Some(d) => d,
            None => {
                let d = module_fields(&sim, &beam.template_name, "W3DLaserDraw").map(|f| laser_draw_from(&f));
                assets.lasers.insert(beam.template_name.clone(), d.clone());
                d
            }
        };
        let Some(draw) = draw else { continue };
        // `LaserUpdate::updateStartPos`: the source's bone every frame, else its position; the last start
        // is kept once the source is gone.
        // A source with no rig (no skeleton) still has a drawable: the position is its transform. A bone that is
        // not found falls back to the drawable's position too (LaserUpdate.cpp:134).
        let source = rigs.iter().find(|(so, _, _)| so.0 == beam.source);
        let live_start = match source {
            Some((_, root, rig)) if !beam.bone_name.is_empty() => rig
                .and_then(|r| bone_world(r, root, &joints, &beam.bone_name))
                .or_else(|| Some(convert::from_bevy(root.translation))),
            Some((_, root, _)) => Some(convert::from_bevy(root.translation)),
            None => None,
        };
        let start = live_start.or_else(|| assets.last_start.get(&beam.id).copied()).unwrap_or([beam.start.x, beam.start.y, beam.start.z]);
        assets.last_start.insert(beam.id, start);
        let end = [beam.end.x, beam.end.y, beam.end.z];
        let tex = texture(&mut assets, &cache.vfs, &mut images, &draw.texture);
        let aspect = tex.as_ref().map_or(1.0, |t| t.1);
        let mat = material(&mut assets, &mut materials, &draw.texture, tex.map(|t| t.0));
        let segs = laser_segments(start, end, draw.segments, draw.arc_height, draw.segment_overlap_ratio, |x, y| ground.height(x, y));
        for (si, seg) in segs.iter().enumerate() {
            let len = {
                let d = [seg[1][0] - seg[0][0], seg[1][1] - seg[0][1], seg[1][2] - seg[0][2]];
                (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
            };
            for i in (0..draw.num_beams.max(1)).rev() {
                let (width, color) = beam_look(i, draw.num_beams.max(1), draw.inner_width, draw.outer_width, draw.inner_color, draw.outer_color);
                // Constructor scale: the beam's own alpha is premultiplied into the colour (additive).
                let tile = if draw.tile { segline_tile_factor(laser_tile_factor(len, width, aspect, draw.tiling_scalar)) } else { 1.0 };
                let offset = scroll_offset(draw.scroll_rate, seconds);
                let index = si * draw.num_beams.max(1) as usize + i as usize;
                let world = [convert::to_bevy(seg[0]), convert::to_bevy(seg[1])];
                let v = tiled_v(2, tile, offset);
                let strip = ribbon(&world, &v, width, eye, [color[0], color[1], color[2], 1.0]);
                let key = (beam.id, index);
                want_laser.insert(key);
                match have_laser.get(&key) {
                    Some((e, handle)) => {
                        if let Some(mut mesh) = meshes.get_mut(handle) {
                            *mesh = strip_mesh(&strip);
                        }
                        commands.entity(*e).insert(MeshMaterial3d(mat.clone()));
                    }
                    None => {
                        commands.spawn((
                            LaserLineView { beam: beam.id, index },
                            Mesh3d(meshes.add(strip_mesh(&strip))),
                            MeshMaterial3d(mat.clone()),
                            Transform::IDENTITY,
                            Visibility::default(),
                            bevy::light::NotShadowCaster,
                        ));
                    }
                }
            }
        }
    }
    for (e, v, _) in &laser_views {
        if !want_laser.contains(&(v.beam, v.index)) {
            commands.entity(e).despawn();
        }
    }
    let live: HashSet<u32> = sim.game.lasers().iter().map(|b| b.id).collect();
    assets.last_start.retain(|id, _| live.contains(id));
}

#[cfg(test)]
mod tests {
    use super::*;

    const ZERO: [f32; 3] = [0.0, 0.0, 0.0];

    fn p(x: f32) -> [f32; 3] {
        [x, 0.0, 1.0]
    }

    /// `doDrawModule` (W3DProjectileStreamDraw.cpp:151-180): with `MaxSegments` the newest points only.
    /// Reverting the slicing (`used - max`) draws the whole stream and fails the first assertion.
    #[test]
    fn max_segments_keeps_only_the_newest_points() {
        let pts: Vec<[f32; 3]> = (0..6).map(|i| p(i as f32)).collect();
        let lines = stream_lines(&pts, 3);
        assert_eq!(lines, vec![vec![p(3.0), p(4.0), p(5.0)]]);
        assert_eq!(stream_lines(&pts, 0)[0].len(), 6, "0 draws everything");
    }

    /// A zero point (hole or dead projectile) breaks the line; a single point is not drawn
    /// (W3DProjectileStreamDraw.cpp:172-177). Reverting the hole test merges the two strands.
    #[test]
    fn holes_break_the_strand_and_single_points_are_dropped() {
        let pts = vec![ZERO, p(1.0), p(2.0), ZERO, p(4.0), ZERO, p(6.0), p(7.0)];
        let lines = stream_lines(&pts, 0);
        assert_eq!(lines, vec![vec![p(1.0), p(2.0)], vec![p(6.0), p(7.0)]]);
    }

    #[test]
    fn stream_v_runs_by_point_index_and_scrolls() {
        // `TileFactor` 2 over four points: V 0, 2, 4, 6 plus the offset.
        assert_eq!(tiled_v(4, 2.0, 0.25), vec![0.25, 2.25, 4.25, 6.25]);
    }

    /// `ScrollRate` is V units per second, kept in [0, 1) (seglinerenderer.cpp:232-239).
    #[test]
    fn scroll_offset_is_a_fraction_of_rate_times_seconds() {
        assert!((scroll_offset(6.0, 0.5) - 0.0).abs() < 1e-6);
        assert!((scroll_offset(6.0, 0.25) - 0.5).abs() < 1e-6);
        // A negative rate wraps into the same range: -1.75 V/s for one second is 0.25.
        assert!((scroll_offset(-1.75, 1.0) - 0.25).abs() < 1e-6);
    }

    /// `W3DLaserDraw.cpp:159-198`: one beam takes the inner width and colour (premultiplied by alpha); several
    /// spread from inner to outer. Reverting the spread (scale) fails the outer-beam assertions.
    #[test]
    fn beams_spread_from_inner_to_outer() {
        let inner = [1.0, 1.0, 1.0, 0.5];
        let outer = [0.0, 0.0, 1.0, 1.0];
        let (w, c) = beam_look(0, 1, 0.4, 2.0, inner, outer);
        assert_eq!(w, 0.4);
        assert_eq!(c, [0.5, 0.5, 0.5], "a single beam is the inner colour times its alpha");
        let (w0, c0) = beam_look(0, 3, 0.4, 2.0, inner, outer);
        assert_eq!(w0, 0.4);
        assert_eq!(c0, [1.0, 1.0, 1.0]);
        let (w2, c2) = beam_look(2, 3, 0.4, 2.0, inner, outer);
        assert!((w2 - 2.0).abs() < 1e-6);
        // colour = inner + scale * (outer - inner) * inner_alpha with scale 1: red 1 + (0 - 1) * 0.5, blue 1 + 0.
        assert!((c2[0] - 0.5).abs() < 1e-6 && (c2[2] - 1.0).abs() < 1e-6, "{c2:?}");
    }

    /// `laser_tile_factor`: the texture is tiled, not stretched (W3DLaserDraw.cpp:409-410).
    #[test]
    fn tile_factor_keeps_the_texture_square() {
        assert!((laser_tile_factor(10.0, 2.0, 2.0, 0.5) - 5.0).abs() < 1e-6);
    }

    /// `SegLineRendererClass::Set_Texture_Tile_Factor` (seglinerenderer.cpp:187-199) clamps to [0, 50]: the
    /// Avenger and Lazr beams' `TilingScalar = -3` gives 0. Reverting the clamp makes V run negative and fails.
    #[test]
    fn segline_tile_factor_is_clamped_to_zero_to_fifty() {
        assert_eq!(segline_tile_factor(-7.5), 0.0);
        assert_eq!(segline_tile_factor(2.0), 2.0);
        assert_eq!(segline_tile_factor(-22417.0), 0.0);
        assert_eq!(segline_tile_factor(120.0), 50.0);
    }

    /// `W3DLaserDraw.cpp:274-370`: with an arc the middle segments are highest; without one every segment is
    /// the whole line. Reverting the cosine lift fails the arc assertion.
    #[test]
    fn arc_is_highest_in_the_middle() {
        let start = [0.0, 0.0, 0.0];
        let end = [100.0, 0.0, 0.0];
        let segs = laser_segments(start, end, 4, 10.0, 0.0, |_, _| 0.0);
        assert_eq!(segs.len(), 4);
        // Segment 1 starts at a quarter of the line: cos(pi/4) of the arc. Segment 2 starts at the middle.
        assert!((segs[1][0][2] - 10.0 * std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3, "{:?}", segs[1][0]);
        assert!((segs[2][0][2] - 10.0).abs() < 1e-4, "{:?}", segs[2][0]);
        let flat = laser_segments(start, end, 4, 0.0, 0.0, |_, _| 0.0);
        assert!(flat.iter().all(|s| *s == [start, end]));
    }

    /// The arc never dips below the ground plus the 2 clearance (W3DLaserDraw.cpp:358-362).
    #[test]
    fn arc_stays_above_the_ground() {
        let segs = laser_segments([0.0, 0.0, 0.0], [100.0, 0.0, 0.0], 4, 1.0, 0.0, |_, _| 50.0);
        for s in &segs {
            assert!(s[0][2] >= 52.0 && s[1][2] >= 52.0);
        }
    }

    /// The ribbon is `width` across as seen from the eye (`tangent x (eye - point)`), with U 0 and 1 on the
    /// two edges and V per point. Reverting the half-width fails the span check.
    #[test]
    fn ribbon_spans_the_width_from_the_eye() {
        let pts = [Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0)];
        let s = ribbon(&pts, &[0.0, 1.0], 4.0, Vec3::new(5.0, 100.0, 0.0), [1.0; 4]);
        assert_eq!(s.positions.len(), 4);
        // The eye is above the line: `x cross up` is along z, so the two edges sit at z = +-2.
        let spread = s.positions[0][2] - s.positions[1][2];
        assert!((spread.abs() - 4.0).abs() < 1e-4, "{:?}", s.positions);
        assert_eq!(s.uvs, vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]]);
        assert_eq!(s.indices.len(), 6);
    }

    fn tracer(expire: u32) -> TracerView {
        TracerView { origin: [0.0; 3], dir: [1.0, 0.0, 0.0], speed: 2.0, length: 10.0, width: 0.3, color: [1.0; 3], opacity: 1.0, age: 0, expire, synced: None }
    }

    /// The tracer steps once per logic frame, not per render frame: five render frames at logic frame 10
    /// move it one step (the first sync counts as the first draw), and a render frame with no new logic frame
    /// moves nothing. Reverting `advance_to` to one step per call fails the repeated-render-frame assertions.
    #[test]
    fn tracer_steps_once_per_logic_frame() {
        let mut t = tracer(100);
        for _ in 0..5 {
            t.advance_to(10);
        }
        assert_eq!(t.age, 1);
        assert_eq!(t.origin, [2.0, 0.0, 0.0]);
        t.advance_to(13);
        assert_eq!(t.age, 4);
        assert_eq!(t.origin, [8.0, 0.0, 0.0]);
        t.advance_to(13);
        assert_eq!(t.age, 4, "no new logic frame, no step");
    }

    /// A paused sim (no logic frames) holds the tracer, and it is removed at its expiry frame count, not
    /// after a wall-clock time. Reverting the `expired` break in `advance_to` steps past the expiry.
    #[test]
    fn tracer_holds_while_paused_and_stops_at_its_expiry() {
        let mut t = tracer(3);
        t.advance_to(5);
        let held = t.origin;
        for _ in 0..100 {
            t.advance_to(5);
        }
        assert_eq!(t.origin, held);
        t.advance_to(50);
        assert_eq!(t.age, 3);
        assert!(t.expired());
        assert_eq!(t.origin, [6.0, 0.0, 0.0]);
    }

    /// `W3DTracerDraw::doDrawModule` (W3DTracerDraw.cpp:136-150): the opacity falls linearly to zero at the
    /// expiry (`opacity / frames left` each frame). Reverting the decay leaves it at 1 and fails.
    #[test]
    fn tracer_fades_linearly_to_zero_at_its_expiry() {
        let mut t = tracer(10);
        for _ in 0..5 {
            t.step();
        }
        assert!((t.opacity - 0.5).abs() < 1e-5, "{}", t.opacity);
        for _ in 0..5 {
            t.step();
        }
        assert!(t.opacity.abs() < 1e-5);
        assert!(t.expired());
        assert!(!tracer(10).expired());
    }

    /// The line moves `speed` per frame along its direction (W3DTracerDraw.cpp:145-149).
    #[test]
    fn tracer_moves_along_its_direction() {
        let mut t = tracer(100);
        t.dir = [0.0, 1.0, 0.0];
        for _ in 0..3 {
            t.step();
        }
        assert_eq!(t.origin, [0.0, 6.0, 0.0]);
        assert_eq!(t.segment(), [[0.0, 6.0, 0.0], [0.0, 16.0, 0.0]]);
    }

    /// `TracerFXNugget::doFXPos` (FXList.cpp:179-202): 90 to fly at 300/s (10 per frame) with `DecayAt` 1 expires
    /// after 9 frames; `DecayAt` 0.5 rounds 4.5 up to 5. Reverting the `dist - length` term gives 10.
    #[test]
    fn tracer_expiry_is_the_flight_time_scaled_and_rounded_up() {
        let (speed, expire) = tracer_timing(100.0, 10.0, 300.0, 999.0, 1.0);
        assert_eq!(speed, 10.0, "300 per second is 10 per frame");
        assert_eq!(expire, 9);
        assert_eq!(tracer_timing(100.0, 10.0, 300.0, 999.0, 0.5).1, 5);
    }

    /// A zero `Speed` takes the weapon's speed per frame (FXList.cpp:179-181): 90 at 20 per frame is 4.5 frames.
    #[test]
    fn a_zero_speed_takes_the_weapon_speed() {
        let (speed, expire) = tracer_timing(100.0, 10.0, 0.0, 20.0, 1.0);
        assert_eq!(speed, 20.0);
        assert_eq!(expire, 5);
    }

    #[test]
    fn colours_parse_with_alpha() {
        let c = parse_rgba("R:255 G:0 B:255  A:150");
        assert_eq!(c[0], 1.0);
        assert_eq!(c[1], 0.0);
        assert!((c[3] - 150.0 / 255.0).abs() < 1e-6);
    }
}
