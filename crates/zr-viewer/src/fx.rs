//! Client-side particle effects: a CPU port of `Particle::update` /
//! `ParticleSystem::update` (`GameClient/System/ParticleSys.cpp`) rendered as
//! camera-facing textured billboards, plus `FXList` playback.
//!
//! [`ParticleManager`] ports `ParticleSystemManager`'s global count and per-priority
//! oldest-first lists (`ParticleSys.cpp:1867-1921`, `:3465-3576`) and the dynamic-LOD
//! gate (`GameLOD.h:231-253`); the cap comes from the options' detail level
//! (`GameLOD.cpp:59`, `:257`).
//!
//! The simulation steps at the client's 30 Hz rate; randomness is client-only
//! (`zr_fx::ClientRng`), never the simulation RNG.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use zr_assets::{RgbaImage, Vfs};
use zr_fx::{
    ClientRng, EmissionVelocity, EmissionVolume, EmissionVolumeType, FxDatabase, FxNugget,
    ParticlePriority, ParticleShaderType, ParticleSystemTemplate, MAX_KEYFRAMES,
    NUM_PARTICLE_PRIORITIES,
};
use zr_sim::ObjectId;

use crate::anim::AnimRig;
use crate::convert::{self, TextureUse};
use crate::options::{CustomLod, Detail, GameSettings, InstallDefaults, LodPresets};
use crate::sim_view::{ModelCache, SimObject, SimRes};
use crate::source::Ground;

/// One client logic frame (`GameClient` runs at 30 Hz).
pub const STEP: f32 = 1.0 / 30.0;
const MAX_STEPS: u32 = 4;
const MAX_SIZE_BONUS: f32 = 50.0;

/// `TheGlobalData->m_maxParticleCount`'s value until the detail preset or `options.ini` is known:
/// `VeryHigh` is 5000 (`GameLOD.cpp:257`), the viewer's default detail.
const DEFAULT_MAX_PARTICLES: usize = 5000;
/// `GlobalData::m_maxFieldParticleCount`'s default (`GlobalData.cpp:837`).
const DEFAULT_MAX_FIELD_PARTICLES: usize = 30;

/// The parsed effect templates (inserted by the viewer setup).
#[derive(Resource)]
pub struct FxDb(pub Arc<FxDatabase>);

/// Texture / material caches for particle billboards.
#[derive(Resource, Default)]
pub struct FxAssets {
    textures: HashMap<String, Option<Handle<Image>>>,
    /// Additive copies of the textures (alpha forced to 1); see [`FxAssets::opaque_texture`].
    opaque_textures: HashMap<String, Option<Handle<Image>>>,
    materials: HashMap<(String, u8), Handle<StandardMaterial>>,
    fallback: Option<Handle<Image>>,
}

impl FxAssets {
    /// Texture for a `ParticleName`; falls back to a soft radial disc so a
    /// missing texture never renders as a hard white square.
    fn texture(&mut self, images: &mut Assets<Image>, vfs: &Vfs, name: &str) -> Handle<Image> {
        let key = name.to_ascii_lowercase();
        if let Some(h) = self.textures.get(&key) {
            return h.clone().unwrap_or_else(|| self.fallback(images));
        }
        let loaded = (!name.is_empty()).then(|| zr_w3d::load_texture(vfs, name).ok()).flatten();
        let handle = loaded.map(|img| images.add(convert::texture_image(&img, TextureUse::Particle)));
        self.textures.insert(key, handle.clone());
        handle.unwrap_or_else(|| self.fallback(images))
    }

    /// Texture for an additive particle. `PointGroupClass` draws these with `SRCBLEND_ONE,
    /// DSTBLEND_ONE` (`ShaderClass::_PresetAdditiveSpriteShader`, `Core/.../WW3D2/shader.cpp`), so
    /// the texture's alpha never weights the colour. Bevy's `AlphaMode::Add` multiplies by alpha,
    /// so this copy has alpha forced to 255.
    fn opaque_texture(&mut self, images: &mut Assets<Image>, vfs: &Vfs, name: &str) -> Handle<Image> {
        let key = name.to_ascii_lowercase();
        if let Some(h) = self.opaque_textures.get(&key) {
            return h.clone().unwrap_or_else(|| self.fallback(images));
        }
        let handle = (!name.is_empty()).then(|| zr_w3d::load_texture(vfs, name).ok()).flatten().map(|mut img| {
            for p in img.pixels.chunks_exact_mut(4) {
                p[3] = 255;
            }
            images.add(convert::texture_image(&img, TextureUse::Particle))
        });
        self.opaque_textures.insert(key, handle.clone());
        handle.unwrap_or_else(|| self.fallback(images))
    }

    fn fallback(&mut self, images: &mut Assets<Image>) -> Handle<Image> {
        if let Some(h) = &self.fallback {
            return h.clone();
        }
        const N: u32 = 32;
        let mut pixels = Vec::with_capacity((N * N * 4) as usize);
        for y in 0..N {
            for x in 0..N {
                let (dx, dy) = (x as f32 / (N - 1) as f32 * 2.0 - 1.0, y as f32 / (N - 1) as f32 * 2.0 - 1.0);
                let a = (1.0 - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0);
                pixels.extend_from_slice(&[255, 255, 255, (a * 255.0) as u8]);
            }
        }
        let img = RgbaImage { width: N, height: N, pixels, mips: Vec::new() };
        let h = images.add(convert::texture_image(&img, TextureUse::Particle));
        self.fallback = Some(h.clone());
        h
    }

    fn material(
        &mut self,
        images: &mut Assets<Image>,
        materials: &mut Assets<StandardMaterial>,
        vfs: &Vfs,
        name: &str,
        shader: ParticleShaderType,
    ) -> Handle<StandardMaterial> {
        let key = (name.to_ascii_lowercase(), shader as u8);
        if let Some(m) = self.materials.get(&key) {
            return m.clone();
        }
        let tex = if shader == ParticleShaderType::Additive { self.opaque_texture(images, vfs, name) } else { self.texture(images, vfs, name) };
        let alpha_mode = match shader {
            ParticleShaderType::Additive => AlphaMode::Add,
            ParticleShaderType::Alpha => AlphaMode::Blend,
            ParticleShaderType::AlphaTest => AlphaMode::Mask(0.5),
            ParticleShaderType::Multiply => AlphaMode::Multiply,
        };
        let mat = StandardMaterial {
            base_color: Color::WHITE,
            base_color_texture: Some(tex),
            alpha_mode,
            unlit: true,
            cull_mode: None,
            double_sided: true,
            perceptual_roughness: 1.0,
            ..default()
        };
        let h = materials.add(mat);
        self.materials.insert(key, h.clone());
        h
    }
}

/// A live particle's place in the engine's global lists: its system's entity and its serial id
/// inside that system.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ParticleKey {
    system: Entity,
    id: u64,
}

/// `ParticleSystemManager`'s global particle cap and dynamic-LOD gate
/// (`GameClient/System/ParticleSys.cpp:1867-1921`, `:3465-3576`).
///
/// The engine keeps one global count and a doubly-linked list per `ParticlePriorityType`
/// (`m_allParticlesHead` / `m_allParticlesTail`), appended on creation and read oldest-first by
/// `removeOldestParticles`. The viewer's particles live inside their systems, so a key names the
/// system entity and the particle's serial id (`Particle.id`); a system drops the keys the cap
/// removed from it.
#[derive(Resource)]
pub struct ParticleManager {
    /// `TheGlobalData->m_maxParticleCount`: the detail level's `MaxParticleCount`
    /// (`GameLOD.cpp:59`, VeryHigh 5000 at `:257`; `Custom` from `options.ini`).
    max_particle_count: usize,
    /// `TheGlobalData->m_maxFieldParticleCount` (`GlobalData.cpp:837`, default 30).
    max_field_particle_count: usize,
    /// `TheParticleSystemManager->m_particleCount`.
    count: usize,
    /// `m_allParticlesHead[priority]` .. tail: insertion order, oldest first (index 0 is unused).
    lists: [VecDeque<ParticleKey>; NUM_PARTICLE_PRIORITIES],
    /// `TheParticleSystemManager->m_fieldParticleCount`: the previous draw's field particles
    /// (`W3DParticleSys.cpp:141`, `:250`). The viewer counts every live particle of an
    /// `AREA_EFFECT` ground-aligned system; it does not cull particles that are off screen.
    field_particle_count: usize,
    /// `TheGameLODManager->m_minDynamicParticlePriority` / `m_minDynamicParticleSkipPriority`
    /// (`GameLOD.h:212-213`), from the current `DynamicGameLOD` block.
    min_dynamic_priority: ParticlePriority,
    min_dynamic_skip_priority: ParticlePriority,
    /// `m_dynamicParticleSkipMask` (`GameLOD.h:208`) and `m_numParticleGenerations`
    /// (`isParticleSkipped`, `GameLOD.h:231-234`).
    particle_skip_mask: u32,
    num_particle_generations: u32,
    /// `W3DDisplay::updateAverageFPS`'s 30-sample history (`W3DDisplay.cpp:1014-1049`).
    fps_history: [f32; 30],
    fps_index: usize,
    average_fps: f32,
    /// `m_currentDynamicLOD`, which starts `HIGH` (`GameLOD.cpp:215`).
    dynamic_level: usize,
    /// Serial ids for `ParticleKey`, so a key stays valid while its system swap-removes particles.
    next_id: u64,
    /// Particles the cap removed from systems other than the one being stepped; their systems drop
    /// them after the step loop.
    pending_removals: Vec<ParticleKey>,
    /// Systems a despawn was requested for in this frame (`sync_bone_fx`). Bevy may still run one
    /// more `step` before applying the despawn; a retired system must not emit particles whose keys
    /// nothing would drop. Entity generations make stale entries harmless.
    retired: BTreeSet<Entity>,
}

impl Default for ParticleManager {
    fn default() -> Self {
        Self {
            max_particle_count: DEFAULT_MAX_PARTICLES,
            max_field_particle_count: DEFAULT_MAX_FIELD_PARTICLES,
            count: 0,
            lists: std::array::from_fn(|_| VecDeque::new()),
            field_particle_count: 0,
            // `DynamicGameLODInfo::DynamicGameLODInfo` (`GameLOD.cpp:133-141`). In C++ the fields
            // are uninitialised until the first level change; the constructor's values are the
            // same "no gate" the first frame's LOW level applies anyway.
            min_dynamic_priority: ParticlePriority::WeaponExplosion,
            min_dynamic_skip_priority: ParticlePriority::WeaponExplosion,
            particle_skip_mask: 0,
            num_particle_generations: 0,
            fps_history: [0.0; 30],
            fps_index: 0,
            average_fps: 0.0,
            dynamic_level: 2,
            next_id: 0,
            pending_removals: Vec::new(),
            retired: BTreeSet::new(),
        }
    }
}

impl ParticleManager {
    /// The detail level's particle cap and the install's field-particle cap.
    fn set_limits(&mut self, max_particles: usize, max_field_particles: usize) {
        self.max_particle_count = max_particles;
        self.max_field_particle_count = max_field_particles;
    }

    /// Forgets every particle of a system that is being despawned (the engine would destroy the
    /// system and let its particles finish; the viewer's despawns are immediate).
    fn forget_system(&mut self, system: Entity) {
        for list in &mut self.lists {
            let before = list.len();
            list.retain(|k| k.system != system);
            self.count -= before - list.len();
        }
        self.pending_removals.retain(|k| k.system != system);
        self.retired.insert(system);
    }

    /// Whether the system's despawn was requested (it must not emit into the global lists again).
    fn is_retired(&self, system: Entity) -> bool {
        self.retired.contains(&system)
    }

    /// Clears the whole manager for a new game.
    fn reset(&mut self) {
        self.count = 0;
        for list in &mut self.lists {
            list.clear();
        }
        self.pending_removals.clear();
        self.retired.clear();
        self.field_particle_count = 0;
        self.next_id = 0;
    }

    fn new_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        id
    }

    /// `ParticleSystemManager::addParticle` (`ParticleSys.cpp:3465-3492`): append to the priority's
    /// list and count it.
    fn add_particle(&mut self, system: Entity, id: u64, priority: ParticlePriority) {
        self.lists[priority.index()].push_back(ParticleKey { system, id });
        self.count += 1;
    }

    /// `ParticleSystemManager::removeParticle` (`ParticleSys.cpp:3497-3522`): a particle died.
    fn remove_particle(&mut self, system: Entity, id: u64, priority: ParticlePriority) {
        let key = ParticleKey { system, id };
        let list = &mut self.lists[priority.index()];
        if let Some(i) = list.iter().position(|k| *k == key) {
            list.remove(i);
            self.count -= 1;
        }
    }

    /// `ParticleSystem::createParticle` (`ParticleSys.cpp:1867-1921`) without `forceCreate` (the
    /// viewer never loads saves). Returns whether the new particle may be created.
    fn create_particle(&mut self, system_particles: usize, priority: ParticlePriority, is_field: bool) -> bool {
        // `:1894-1897`: below the dynamic floor, or below the skip floor while the skip mask skips
        // this creation. C++ short-circuits, so `isParticleSkipped` advances only for the second
        // comparison.
        if priority < self.min_dynamic_priority
            || (priority < self.min_dynamic_skip_priority && self.is_particle_skipped())
        {
            return false;
        }
        // `:1899`: field particles are capped separately, and only once the system already has one
        // (strictly more than the max is refused).
        if system_particles > 0
            && priority == ParticlePriority::AreaEffect
            && is_field
            && self.field_particle_count > self.max_field_particle_count
        {
            return false;
        }
        // `:1902-1914`: ALWAYS_RENDER is exempt from every count limit. Otherwise remove the oldest
        // lower-priority particles (all of them, or the new particle is refused), and a cap of 0
        // refuses every particle.
        if priority != ParticlePriority::AlwaysRender {
            let excess = self.count as i64 - self.max_particle_count as i64;
            if excess > 0 && self.remove_oldest_particles(excess as u32, priority) != excess as i32 {
                return false;
            }
            if self.max_particle_count == 0 {
                return false;
            }
        }
        true
    }

    /// `ParticleSystemManager::removeOldestParticles` (`ParticleSys.cpp:3554-3576`): delete the
    /// oldest particle of the lowest non-empty priority below `priority_cap`, `count` times.
    ///
    /// The C++ loop's `count--` is an `UnsignedInt`, so when it ends because the counter reached 0
    /// it wraps, and `countToRemove - count` is `count + 1`. `createParticle` compares that with
    /// `numInExcess` (`:1908`) and therefore refuses the new particle even though the room was made;
    /// the port keeps the arithmetic. The population still settles at the cap: a refused creation
    /// has already removed one, and the next attempt sees no excess.
    fn remove_oldest_particles(&mut self, count: u32, priority_cap: ParticlePriority) -> i32 {
        let count_to_remove = count as i32;
        let mut remaining = count;
        loop {
            let old = remaining;
            remaining = remaining.wrapping_sub(1);
            if old == 0 || self.count == 0 {
                break;
            }
            for i in ParticlePriority::WeaponExplosion.index()..priority_cap.index() {
                if let Some(head) = self.lists[i].pop_front() {
                    self.count -= 1;
                    self.pending_removals.push(head);
                    break;
                }
            }
        }
        count_to_remove.wrapping_sub(remaining as i32)
    }

    /// `GameLODManager::isParticleSkipped` (`GameLOD.h:231-234`).
    fn is_particle_skipped(&mut self) -> bool {
        self.num_particle_generations = self.num_particle_generations.wrapping_add(1);
        (self.num_particle_generations & self.particle_skip_mask) != self.particle_skip_mask
    }

    /// The cap's removed keys of one system, which drops those particles from its vector.
    fn take_removals_for(&mut self, system: Entity) -> Vec<ParticleKey> {
        let mut mine = Vec::new();
        let mut rest = Vec::with_capacity(self.pending_removals.len());
        for key in self.pending_removals.drain(..) {
            if key.system == system {
                mine.push(key);
            } else {
                rest.push(key);
            }
        }
        self.pending_removals = rest;
        mine
    }

    fn take_all_removals(&mut self) -> Vec<ParticleKey> {
        std::mem::take(&mut self.pending_removals)
    }

    /// `W3DDisplay::updateAverageFPS` (`W3DDisplay.cpp:1014-1049`): a 30-sample history, averaged
    /// over the whole (zero-initialised) array.
    fn update_fps(&mut self, delta_secs: f32) -> f32 {
        let current = if delta_secs > 0.0 { 1.0 / delta_secs } else { 0.0 };
        self.fps_history[self.fps_index] = current;
        self.fps_index = (self.fps_index + 1) % self.fps_history.len();
        self.average_fps = self.fps_history.iter().sum::<f32>() / self.fps_history.len() as f32;
        self.average_fps
    }

    /// `W3DDisplay::draw` (`W3DDisplay.cpp:1857-1866`) and `GameLODManager::setDynamicLODLevel`
    /// (`GameLOD.cpp:700-724`): with dynamic LOD on, the level for the average FPS; otherwise
    /// `VeryHigh`. The gate values are applied when the level changes, and the generation counter
    /// restarts (`applyDynamicLODLevel`).
    fn set_dynamic_lod(&mut self, enabled: bool, presets: &LodPresets) {
        let level = if enabled { presets.dynamic_level_for_fps(self.average_fps) } else { 3 };
        if level == self.dynamic_level {
            return;
        }
        self.dynamic_level = level;
        let p = presets.dynamic_preset(level);
        self.min_dynamic_priority = p.min_particle_priority;
        self.min_dynamic_skip_priority = p.min_particle_skip_priority;
        self.particle_skip_mask = p.particle_skip_mask;
        self.num_particle_generations = 0;
    }
}

/// The fixed-step accumulator for particle updates.
#[derive(Resource, Default)]
pub struct FxClock {
    acc: f32,
}

/// FX events drained from the sim's per-frame `fx_events()` during the stepping loop, awaiting
/// playback (object, name, Generals-space position, shot direction, aim).
pub type FxPlay = (ObjectId, String, [f32; 3], [f32; 3], Aim);

#[derive(Resource, Default)]
pub struct FxQueue(pub Vec<FxPlay>);

/// The secondary point of a weapon FX (`FXList::doFXPos`'s `secondary`, the shot's target) and the weapon's
/// speed, which `Tracer` nuggets fly along (`TracerFXNugget::doFXPos`, FXList.cpp:169-215).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Aim {
    pub target: Option<[f32; 3]>,
    pub weapon_speed: f32,
}

/// The client-only RNG used for emission.
#[derive(Resource)]
pub struct FxRng(pub ClientRng);

/// A live particle system instance.
#[derive(Component)]
pub struct FxSystem {
    template: Arc<ParticleSystemTemplate>,
    particles: Vec<Particle>,
    origin: [f32; 3],
    rng: ClientRng,
    /// The billboard mesh, created by `update_fx` the first frame there is something to draw. An
    /// empty `Mesh` asset must never be added: bevy's mesh allocator skips zero-size meshes but
    /// still tries to copy their data, logging `Use-after-free` once per extraction.
    mesh: Option<Handle<Mesh>>,
    age: u32,
    delay_left: u32,
    burst_delay_left: u32,
    system_lifetime_left: u32,
    forever: bool,
    stopped: bool,
    destroyed: bool,
    accumulated_size_bonus: f32,
    ground_aligned: bool,
    shader: ParticleShaderType,
    drift: [f32; 3],
    color_scale: [f32; 3],
    /// Orientation that turns emission offsets and velocities (`orientation`). `None` for systems that
    /// do not follow an oriented object; a projectile's exhaust follows its full heading, pitch included.
    orient: Option<[[f32; 3]; 3]>,
}

/// A particle system attached to a live projectile: `MissileAIUpdate::doIgnitionState` creates it
/// (`createAttachedParticleSystemID`) and its origin follows the projectile. When the sim tosses the
/// exhaust or the projectile is gone, the component is removed and the system is destroyed
/// (`ParticleSystem::destroy`: stop emitting, let the live particles finish).
#[derive(Component)]
pub struct FxProjectile {
    projectile: u32,
}

/// A short-lived effect entity (`Tracer` / `RayEffect` beam, `LightPulse`) despawned after `life`.
#[derive(Component)]
struct FxBeam {
    life: f32,
}

/// A particle system attached to a model bone (`ParticleSysBone`); its origin follows the bone. `module` is the
/// object's draw module that owns the bone (`W3DModelDraw::m_particleSystemIDs`); 0 is the module `sim_view` draws.
#[derive(Component)]
pub struct FxBone {
    object: ObjectId,
    module: usize,
    bone: String,
    system: String,
}

struct Particle {
    /// Serial within the owning system: its key in [`ParticleManager`]'s priority lists.
    id: u64,
    pos: [f32; 3],
    vel: [f32; 3],
    accel: [f32; 3],
    size: f32,
    size_rate: f32,
    size_rate_damping: f32,
    alpha: f32,
    alpha_rate: f32,
    alpha_target: usize,
    alpha_keys: [(f32, u32); MAX_KEYFRAMES],
    color: [f32; 3],
    color_rate: [f32; 3],
    color_target: usize,
    color_keys: [([f32; 3], u32); MAX_KEYFRAMES],
    vel_damping: f32,
    angular_rate: f32,
    angular_damping: f32,
    angle: f32,
    lifetime_left: u32,
    age: u32,
}

/// The rotation (Generals space, row-major) that takes the object's X axis to `heading` (a unit
/// vector): `Rz(yaw) * Ry(-pitch)`, with no roll. Pitch is the missile's climb or dive, so an attached
/// exhaust follows the full transform as `ParticleSystem` does on an attached object.
fn orientation(heading: [f32; 3]) -> [[f32; 3]; 3] {
    let yaw = heading[1].atan2(heading[0]);
    let pitch = heading[2].atan2((heading[0] * heading[0] + heading[1] * heading[1]).sqrt());
    let (sy, cy) = yaw.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    [[cy * cp, -sy, -cy * sp], [sy * cp, cy, -sy * sp], [sp, 0.0, cp]]
}

/// Applies `orient` to `v`. `None` is the identity (bone-attached systems have no orientation).
fn rotate(orient: Option<&[[f32; 3]; 3]>, v: [f32; 3]) -> [f32; 3] {
    match orient {
        None => v,
        Some(m) => [0, 1, 2].map(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2]),
    }
}

fn norm(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 1e-9 {
        [v[0] / l, v[1] / l, v[2] / l]
    } else {
        [0.0, 0.0, 1.0]
    }
}

fn point_on_unit_sphere(rng: &mut ClientRng) -> [f32; 3] {
    loop {
        let v = [rng.real(-1.0, 1.0), rng.real(-1.0, 1.0), rng.real(-1.0, 1.0)];
        if v != [0.0, 0.0, 0.0] {
            return norm(v);
        }
    }
}

/// `ParticleSystem::computeParticlePosition` (local to the system origin).
fn emit_position(t: &ParticleSystemTemplate, rng: &mut ClientRng) -> [f32; 3] {
    match &t.volume {
        EmissionVolume::Cylinder { radius, length } => {
            let angle = rng.real(0.0, std::f32::consts::TAU);
            let r = if t.is_hollow { *radius } else { rng.real(0.0, *radius) };
            [r * angle.cos(), r * angle.sin(), rng.real(-length / 2.0, length / 2.0)]
        }
        EmissionVolume::Sphere { radius } => {
            let r = if t.is_hollow { *radius } else { rng.real(0.0, *radius) };
            let p = point_on_unit_sphere(rng);
            [p[0] * r, p[1] * r, p[2] * r]
        }
        EmissionVolume::Box { half } => {
            if t.is_hollow {
                let side = rng.int(0, 5);
                let (mut p, _) = ([0.0f32; 3], 0);
                match side % 3 {
                    0 => {
                        p[0] = rng.real(-half[0], half[0]);
                        p[1] = rng.real(-half[1], half[1]);
                        p[2] = if side == 0 { -half[2] } else { half[2] };
                    }
                    1 => {
                        p[1] = rng.real(-half[1], half[1]);
                        p[2] = rng.real(-half[2], half[2]);
                        p[0] = if side == 1 { -half[0] } else { half[1] };
                    }
                    _ => {
                        p[0] = rng.real(-half[0], half[0]);
                        p[2] = rng.real(-half[2], half[2]);
                        p[1] = if side == 2 { -half[1] } else { half[1] };
                    }
                }
                p
            } else {
                [rng.real(-half[0], half[0]), rng.real(-half[1], half[1]), rng.real(-half[2], half[2])]
            }
        }
        EmissionVolume::Line { start, end } => {
            let f = rng.real(0.0, 1.0);
            [start[0] + (end[0] - start[0]) * f, start[1] + (end[1] - start[1]) * f, start[2] + (end[2] - start[2]) * f]
        }
        EmissionVolume::Point => [0.0; 3],
    }
}

/// `ParticleSystem::computeParticleVelocity` (local, before the system transform).
fn emit_velocity(t: &ParticleSystemTemplate, pos: [f32; 3], rng: &mut ClientRng) -> [f32; 3] {
    match &t.velocity {
        EmissionVelocity::Ortho { x, y, z } => [x.get(rng), y.get(rng), z.get(rng)],
        EmissionVelocity::Cylindrical { radial, normal } => {
            let angle = rng.real(0.0, std::f32::consts::TAU);
            let r = radial.get(rng);
            [r * angle.cos(), r * angle.sin(), normal.get(rng)]
        }
        EmissionVelocity::Spherical { speed } => {
            let s = speed.get(rng);
            let v = point_on_unit_sphere(rng);
            [s * v[0], s * v[1], s * v[2]]
        }
        EmissionVelocity::Hemispherical { speed } => {
            let s = speed.get(rng);
            let v = loop {
                let v = [rng.real(-1.0, 1.0), rng.real(-1.0, 1.0), rng.real(0.0, 1.0)];
                if v != [0.0, 0.0, 0.0] {
                    break norm(v);
                }
            };
            [s * v[0], s * v[1], s * v[2]]
        }
        EmissionVelocity::Outward { speed, other } => {
            let s = speed.get(rng);
            let o = other.get(rng);
            match t.volume_type {
                EmissionVolumeType::Cylinder => {
                    let d = norm([pos[0], pos[1], 0.0]);
                    [s * d[0], s * d[1], o]
                }
                EmissionVolumeType::Line => {
                    let (start, end) = match &t.volume {
                        EmissionVolume::Line { start, end } => (start, end),
                        _ => (&[0.0; 3], &[0.0; 3]),
                    };
                    let along = norm([end[0] - start[0], end[1] - start[1], end[2] - start[2]]);
                    // perp = up x along, up = along x perp (ParticleSystem.cpp).
                    let perp = norm([-along[1], along[0], 0.0]);
                    let up = norm([
                        along[1] * perp[2] - along[2] * perp[1],
                        along[2] * perp[0] - along[0] * perp[2],
                        along[0] * perp[1] - along[1] * perp[0],
                    ]);
                    [s * perp[0] + o * up[0], s * perp[1] + o * up[1], s * perp[2] + o * up[2]]
                }
                EmissionVolumeType::Point => {
                    let v = point_on_unit_sphere(rng);
                    [s * v[0], s * v[1], s * v[2]]
                }
                // BOX and SPHERE: outward from the origin along the position.
                _ => {
                    let d = norm(pos);
                    [s * d[0], s * d[1], s * d[2]]
                }
            }
        }
        EmissionVelocity::None => [0.0; 3],
    }
}

impl FxSystem {
    fn new(template: Arc<ParticleSystemTemplate>, origin: [f32; 3], mut rng: ClientRng, delay: u32) -> Self {
        let cs = template.color_scale.get(&mut rng) / 255.0;
        let color_scale = [cs, cs, cs];
        let delay_left = (template.initial_delay.get(&mut rng) as u32).saturating_add(delay);
        let forever = template.system_lifetime == 0;
        Self {
            ground_aligned: template.ground_aligned,
            shader: template.shader,
            drift: template.drift_velocity,
            color_scale,
            system_lifetime_left: template.system_lifetime,
            forever,
            template,
            particles: Vec::new(),
            origin,
            rng,
            mesh: None,
            age: 0,
            delay_left,
            burst_delay_left: 0,
            stopped: false,
            destroyed: false,
            accumulated_size_bonus: 0.0,
            orient: None,
        }
    }

    /// `ParticleSystem::destroy`: stop emitting; the system is removed once its particles have died.
    fn destroy(&mut self) {
        self.destroyed = true;
    }

    /// `ParticleSystem::update`: delay, burst emission, then update all particles.
    ///
    /// `entity` is this system's entity, the owner recorded in the global particle lists; `manager`
    /// is the engine's `ParticleSystemManager` count, priority lists and dynamic-LOD gate.
    fn step(&mut self, entity: Entity, manager: &mut ParticleManager) {
        if manager.is_retired(entity) {
            // The system's keys are already out of the manager; it must not add new ones.
            self.particles.clear();
            return;
        }
        if self.delay_left > 0 {
            self.delay_left -= 1;
            return;
        }
        self.age += 1;
        let t = self.template.clone();
        if !self.destroyed && (self.forever || self.system_lifetime_left > 0) && !self.stopped {
            if self.burst_delay_left == 0 {
                let count = t.burst_count.get(&mut self.rng) as i32;
                for _ in 0..count.max(0) {
                    // `generateParticleInfo` draws the particle's random attributes first, also for
                    // a particle the cap then refuses (`ParticleSys.cpp:2104-2110`), and its
                    // constructor appends it to the global list on success. Emit first, so the RNG
                    // stays in step; drop the particle when the cap says no.
                    let mut p = self.emit(&t);
                    if manager.create_particle(self.particles.len(), t.priority, t.ground_aligned) {
                        p.id = manager.new_id();
                        let id = p.id;
                        self.particles.push(p);
                        manager.add_particle(entity, id, t.priority);
                    }
                    let removed = manager.take_removals_for(entity);
                    self.drop_particles(&removed);
                }
                self.burst_delay_left = t.burst_delay.get(&mut self.rng).max(0.0) as u32;
            } else {
                self.burst_delay_left -= 1;
            }
        }
        let (gravity, drift) = (t.gravity, self.drift);
        let shader = self.shader;
        let color_scale = self.color_scale;
        let priority = t.priority;
        let mut i = 0;
        while i < self.particles.len() {
            let p = &mut self.particles[i];
            p.accel[2] += gravity;
            if particle_step(p, shader, drift, color_scale) {
                i += 1;
            } else {
                let dead = self.particles.swap_remove(i);
                manager.remove_particle(entity, dead.id, priority);
            }
        }
        if self.destroyed && self.particles.is_empty() {
            return;
        }
        if !self.forever {
            if self.system_lifetime_left > 0 {
                self.system_lifetime_left -= 1;
            }
            if !self.particles.is_empty() {
                return;
            }
            if self.system_lifetime_left == 0 {
                self.destroyed = true;
            }
        }
    }

    /// Drops the particles the global cap removed (already out of the manager's lists).
    fn drop_particles(&mut self, removed: &[ParticleKey]) {
        if removed.is_empty() {
            return;
        }
        self.particles.retain(|p| !removed.iter().any(|k| k.id == p.id));
    }

    fn done(&self) -> bool {
        (self.destroyed || (!self.forever && self.system_lifetime_left == 0)) && self.particles.is_empty()
    }

    /// Draws one particle's random attributes (`ParticleSystem::generateParticleInfo`,
    /// `ParticleSys.cpp:1928-2045`); the caller assigns its id and appends it if the cap allows.
    fn emit(&mut self, t: &ParticleSystemTemplate) -> Particle {
        let unturned = emit_position(t, &mut self.rng);
        let vel = rotate(self.orient.as_ref(), emit_velocity(t, unturned, &mut self.rng));
        let local = rotate(self.orient.as_ref(), unturned);
        let pos = [self.origin[0] + local[0], self.origin[1] + local[1], self.origin[2] + local[2]];
        let mut alpha_keys = [(0.0f32, 0u32); MAX_KEYFRAMES];
        for (i, k) in t.alpha_keys.iter().enumerate() {
            alpha_keys[i] = (k.var.get(&mut self.rng), k.frame);
        }
        let mut color_keys = [([0.0f32; 3], 0u32); MAX_KEYFRAMES];
        for (i, k) in t.color_keys.iter().enumerate() {
            color_keys[i] = ([k.color.r, k.color.g, k.color.b], k.frame);
        }
        let size = t.start_size.get(&mut self.rng) + self.accumulated_size_bonus;
        self.accumulated_size_bonus = (self.accumulated_size_bonus + t.start_size_rate.get(&mut self.rng)).min(MAX_SIZE_BONUS);
        Particle {
            id: 0,
            pos,
            vel,
            accel: [0.0; 3],
            size,
            size_rate: t.size_rate.get(&mut self.rng),
            size_rate_damping: t.size_rate_damping.get(&mut self.rng),
            alpha: alpha_keys[0].0,
            alpha_rate: 0.0,
            alpha_target: 0,
            alpha_keys,
            color: color_keys[0].0,
            color_rate: [0.0; 3],
            color_target: 0,
            color_keys,
            vel_damping: t.velocity_damping.get(&mut self.rng),
            angular_rate: t.angular_rate_z.get(&mut self.rng),
            angular_damping: t.angular_damping.get(&mut self.rng),
            angle: t.angle_z.get(&mut self.rng),
            lifetime_left: t.lifetime.get(&mut self.rng).max(0.0) as u32,
            age: 0,
        }
    }

    fn visible(&self) -> bool {
        !self.particles.is_empty() || self.delay_left > 0
    }

    /// Builds the camera-facing billboard mesh in Bevy world space.
    fn build_mesh(&self, cam_right: Vec3, cam_up: Vec3) -> Mesh {
        let mut positions: Vec<[f32; 3]> = Vec::with_capacity(self.particles.len() * 4);
        let mut uvs: Vec<[f32; 2]> = Vec::with_capacity(self.particles.len() * 4);
        let mut normals: Vec<[f32; 3]> = Vec::with_capacity(self.particles.len() * 4);
        let mut colors: Vec<[f32; 4]> = Vec::with_capacity(self.particles.len() * 4);
        let mut indices: Vec<u32> = Vec::with_capacity(self.particles.len() * 6);
        let (right, up) = if self.ground_aligned {
            (Vec3::X, Vec3::NEG_Z)
        } else {
            (cam_right, cam_up)
        };
        for p in &self.particles {
            if p.size <= 0.0 {
                continue;
            }
            let base = positions.len() as u32;
            let centre = convert::to_bevy(p.pos);
            // `m_angleZ` is in radians (`ParticleSystem::update`); the W3D point group turns it into
            // a 256-step orientation (`W3DParticleSys` `angleArray`), so the quad turns by the full angle.
            let (s, c) = p.angle.sin_cos();
            let r = right * c + up * s;
            let u = up * c - right * s;
            let h = p.size * 0.5;
            let corners = [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
            let alpha = if self.shader == ParticleShaderType::Additive { 1.0 } else { p.alpha };
            let col = [p.color[0], p.color[1], p.color[2], alpha];
            for (dx, dy) in corners {
                positions.push((centre + r * (dx * h) + u * (dy * h)).to_array());
                normals.push(up.to_array());
                uvs.push([(dx + 1.0) * 0.5, 1.0 - (dy + 1.0) * 0.5]);
                colors.push(col);
            }
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
            .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
            .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
            .with_inserted_indices(Indices::U32(indices))
    }
}

/// `Particle::update` + `Particle::draw` for one frame. Returns false to remove.
fn particle_step(p: &mut Particle, shader: ParticleShaderType, drift: [f32; 3], color_scale: [f32; 3]) -> bool {
    if p.lifetime_left > 0 {
        p.lifetime_left -= 1;
        if p.lifetime_left == 0 {
            return false;
        }
    }
    // integrate acceleration into velocity
    for i in 0..3 {
        p.vel[i] = (p.vel[i] + p.accel[i]) * p.vel_damping;
        p.accel[i] = 0.0;
    }
    if p.age > 0 {
        p.angular_rate *= p.angular_damping;
        p.size_rate *= p.size_rate_damping;
        if shader != ParticleShaderType::Additive {
            let target = p.alpha_target;
            if (target < MAX_KEYFRAMES && p.alpha_keys[target].1 != 0) || target == 0 {
                if p.age > p.alpha_keys[target].1 {
                    p.alpha_target = target + 1;
                    compute_alpha_rate(p);
                }
            } else {
                p.alpha_rate = 0.0;
            }
        }
        let target = p.color_target;
        if (target < MAX_KEYFRAMES && p.color_keys[target].1 != 0) || target == 0 {
            if p.age > p.color_keys[target].1 {
                p.color_target = target + 1;
                compute_color_rate(p);
            }
        } else {
            p.color_rate = [0.0; 3];
        }
        if invisible(p, shader) {
            return false;
        }
    }
    // draw: integrate position, size, alpha and color
    for (i, d) in drift.iter().enumerate() {
        p.pos[i] += p.vel[i] + d;
    }
    p.angle += p.angular_rate;
    p.size += p.size_rate;
    if shader != ParticleShaderType::Additive {
        p.alpha = (p.alpha + p.alpha_rate).clamp(0.0, 1.0);
    }
    for (i, cs) in color_scale.iter().enumerate() {
        p.color[i] = (p.color[i] + p.color_rate[i] + cs).clamp(0.0, 1.0);
    }
    p.age += 1;
    true
}

fn compute_alpha_rate(p: &mut Particle) {
    let t = p.alpha_target;
    if t == 0 || p.alpha_keys[t].1 == 0 {
        p.alpha_rate = 0.0;
        return;
    }
    let time = (p.alpha_keys[t].1 - p.alpha_keys[t - 1].1) as f32;
    if time <= 0.0 {
        p.alpha_rate = 0.0;
        return;
    }
    p.alpha_rate = (p.alpha_keys[t].0 - p.alpha_keys[t - 1].0) / time;
}

fn compute_color_rate(p: &mut Particle) {
    let t = p.color_target;
    if t == 0 || p.color_keys[t].1 == 0 {
        p.color_rate = [0.0; 3];
        return;
    }
    let time = (p.color_keys[t].1 - p.color_keys[t - 1].1) as f32;
    if time <= 0.0 {
        p.color_rate = [0.0; 3];
        return;
    }
    for i in 0..3 {
        p.color_rate[i] = (p.color_keys[t].0[i] - p.color_keys[t - 1].0[i]) / time;
    }
}

/// `Particle::isInvisible`.
fn invisible(p: &Particle, shader: ParticleShaderType) -> bool {
    match shader {
        ParticleShaderType::Additive => {
            if p.color_target >= MAX_KEYFRAMES || p.color_keys[p.color_target.min(MAX_KEYFRAMES - 1)].1 == 0 {
                p.color[0] < 0.01 && p.color[1] < 0.01 && p.color[2] < 0.01
            } else {
                false
            }
        }
        ParticleShaderType::Alpha => {
            if p.alpha_target >= MAX_KEYFRAMES || p.alpha_keys[p.alpha_target.min(MAX_KEYFRAMES - 1)].1 == 0 {
                p.alpha < 0.01
            } else {
                false
            }
        }
        ParticleShaderType::AlphaTest => false,
        ParticleShaderType::Multiply => {
            if p.color_target >= MAX_KEYFRAMES || p.color_keys[p.color_target.min(MAX_KEYFRAMES - 1)].1 == 0 {
                p.color[0] > 0.99 && p.color[1] > 0.99 && p.color[2] > 0.99
            } else {
                false
            }
        }
    }
}

/// Bone positions for `FXListAtBonePos` nuggets of one object: the `BoneName` (uppercased) to the
/// `(world position, X-axis direction)` of every matching bone (unadorned and `01`, `02`, ...),
/// in Generals space.
pub type BonePositions = BTreeMap<String, Vec<([f32; 3], [f32; 3])>>;

/// Spawns every nugget of an `FXList` at a Generals-space point. `dir` is the shot direction
/// (turret / body heading) used by `Tracer` and `RayEffect` beams. `bones` resolves
/// `FXListAtBonePos` nuggets by bone name.
#[allow(clippy::too_many_arguments)]
pub fn spawn_fx_list(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
    vfs: &Vfs,
    db: &FxDatabase,
    fx_assets: &mut FxAssets,
    rng: &mut ClientRng,
    name: &str,
    pos: [f32; 3],
    dir: [f32; 3],
    aim: Aim,
    ground: Option<&Ground>,
    bones: &BonePositions,
    scorches: &mut crate::scorch::Scorches,
) -> usize {
    let Some(fx) = db.fx_list(name) else {
        warn!("fx: FXList {name} not found");
        return 0;
    };
    let mut spawned = 0;
    for nugget in &fx.nuggets {
        match nugget {
            FxNugget::ParticleSystem {
                name: sys_name,
                count,
                offset,
                radius,
                height,
                initial_delay,
                create_at_ground_height,
                ..
            } => {
                let Some(template) = db.particle(sys_name).cloned() else {
                    warn!("fx: {name} references missing ParticleSystem {sys_name}");
                    continue;
                };
                // Drawable/SMUDGE particles are not billboards we can texture.
                if matches!(template.particle_type, zr_fx::ParticleType::Drawable | zr_fx::ParticleType::Smudge) {
                    continue;
                }
                let delay = if initial_delay.low >= 0.0 {
                    let msec = initial_delay.get(rng);
                    (msec * STEP).ceil() as u32
                } else {
                    0
                };
                let material = fx_assets.material(images, materials, vfs, &template.particle_name, template.shader);
                for _ in 0..(*count).max(0) {
                    let angle = rng.real(0.0, std::f32::consts::TAU);
                    let r = radius.get(rng);
                    let mut p = [pos[0] + offset[0] + r * angle.cos(), pos[1] + offset[1] + r * angle.sin(), pos[2] + offset[2]];
                    if *create_at_ground_height {
                        if let Some(g) = ground {
                            p[2] = g.height(p[0], p[1]);
                        }
                    } else {
                        p[2] += height.get(rng);
                    }
                    let seed = ((rng.next_u32() as u64) << 32) | rng.next_u32() as u64;
                    commands.spawn((
                        MeshMaterial3d(material.clone()),
                        Transform::IDENTITY,
                        Visibility::Hidden,
                        bevy::light::NotShadowCaster,
                        FxSystem::new(Arc::new(template.clone()), p, ClientRng::new(seed), delay),
                    ));
                    spawned += 1;
                }
            }
            // `TracerFXNugget::doFXPos` (FXList.cpp:169-215): a flying tracer from the muzzle toward the shot's
            // target, only when the nugget's `Probability` roll passes (client RNG).
            FxNugget::Tracer { speed, decay_at, length, width, color, probability, .. } => {
                if *probability <= rng.real(0.0, 1.0) {
                    continue;
                }
                let Some(target) = aim.target else { continue };
                crate::projectile_draw::spawn_tracer(commands, meshes, materials, pos, target, *speed, aim.weapon_speed, *decay_at, *length, *width, *color);
                spawned += 1;
            }
            FxNugget::RayEffect { name: tex, .. } => {
                spawn_beam(commands, meshes, materials, images, vfs, pos, dir, 200.0, 1.5, tex, zr_fx::Color { r: 1.0, g: 1.0, b: 1.0 });
                spawned += 1;
            }
            FxNugget::LightPulse { color, radius, .. } => {
                commands.spawn((
                    PointLight { color: Color::srgb(color.r, color.g, color.b), intensity: 3_000_000.0, range: radius * 4.0, shadow_maps_enabled: false, ..default() },
                    Transform::from_translation(convert::to_bevy(pos)),
                    FxBeam { life: 0.25 },
                ));
                spawned += 1;
            }
            // `FXListAtBonePosFXNugget::doFXObj`: play the nested FX at every bone that matches
            // `BoneName` (unadorned first, then `01`, `02`, ...).
            FxNugget::FxListAtBonePos { fx, bone_name, orient_to_bone } => {
                let key = bone_name.to_ascii_uppercase();
                if let Some(points) = bones.get(&key) {
                    for (p, d) in points {
                        let dir = if *orient_to_bone { *d } else { [0.0, 0.0, 0.0] };
                        spawned += spawn_fx_list(commands, meshes, materials, images, vfs, db, fx_assets, rng, fx, *p, dir, aim, ground, bones, scorches);
                    }
                }
            }
            // `TerrainScorchFXNugget::doFXPos`: append to the terrain scorch buffer.
            FxNugget::TerrainScorch { scorch, radius } => {
                let kind = match scorch.to_ascii_uppercase().as_str() {
                    "SCORCH_1" => Some(0),
                    "SCORCH_2" => Some(1),
                    "SCORCH_3" => Some(2),
                    "SCORCH_4" => Some(3),
                    "SHADOW_SCORCH" => Some(4),
                    _ => None,
                };
                scorches.add(pos, *radius, kind, rng);
                spawned += 1;
            }
            _ => {}
        }
    }
    spawned
}

/// A textured beam from `pos` along `dir` (`Tracer` / `RayEffect`), despawned by [`update_beams`].
#[allow(clippy::too_many_arguments)]
fn spawn_beam(commands: &mut Commands, meshes: &mut Assets<Mesh>, materials: &mut Assets<StandardMaterial>, images: &mut Assets<Image>, vfs: &Vfs, pos: [f32; 3], dir: [f32; 3], length: f32, width: f32, tex: &str, color: zr_fx::Color) {
    let n = convert::to_bevy(dir).normalize_or_zero();
    if n == Vec3::ZERO || length <= 0.0 {
        return;
    }
    let center = convert::to_bevy(pos) + n * (length * 0.5);
    let rot = Quat::from_rotation_arc(Vec3::Z, n);
    let texture = zr_w3d::load_texture(vfs, tex).ok().map(|t| images.add(convert::texture_image(&t, TextureUse::Laser)));
    let mat = StandardMaterial {
        base_color: Color::srgba(color.r, color.g, color.b, 0.9),
        base_color_texture: texture,
        unlit: true,
        alpha_mode: AlphaMode::Add,
        cull_mode: None,
        ..default()
    };
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(width.max(0.1), width.max(0.1), length.max(0.1)))),
        MeshMaterial3d(materials.add(mat)),
        Transform::from_translation(center).with_rotation(rot),
        Visibility::default(),
        bevy::light::NotShadowCaster,
        FxBeam { life: 0.1 },
    ));
}

pub fn add_systems(app: &mut App) {
    app.init_resource::<FxClock>();
    app.init_resource::<FxQueue>();
    app.init_resource::<ParticleManager>();
    // The options' cap / dynamic LOD and the per-game reset, before the systems that create
    // particles. This mirrors `W3DDisplay::draw` running `updateAverageFPS` every frame.
    app.add_systems(Update, (reset_particles_without_game, sync_particle_lod).chain().before(update_fx));
    app.add_systems(Update, update_fx.run_if(resource_exists::<FxAssets>));
    // `drain_fx` runs after `projectile_sync` so the projectile's launch bone uses the barrel of
    // the shot before `AnimRig::fire` advances the barrel cycle.
    app.add_systems(
        Update,
        drain_fx
            .after(crate::sim_view::projectile_sync)
            .run_if(resource_exists::<FxAssets>)
            .run_if(resource_exists::<crate::sim_view::ModelCache>),
    );
    app.add_systems(
        Update,
        sync_bone_fx
            .run_if(resource_exists::<FxAssets>)
            .run_if(resource_exists::<crate::sim_view::ModelCache>)
            .run_if(resource_exists::<crate::sim_view::SimActive>),
    );
    app.add_systems(Update, update_bone_fx.run_if(resource_exists::<FxAssets>).before(update_fx));
    app.add_systems(
        Update,
        sync_projectile_fx
            .run_if(resource_exists::<FxAssets>)
            .run_if(resource_exists::<crate::sim_view::ModelCache>)
            .run_if(resource_exists::<crate::sim_view::SimActive>)
            .before(update_fx),
    );
    app.add_systems(Update, update_beams);
}

/// Despawns short-lived `Tracer` / `RayEffect` beams and `LightPulse` lights.
fn update_beams(time: Res<Time>, mut commands: Commands, mut beams: Query<(Entity, &mut FxBeam)>) {
    let dt = time.delta_secs();
    for (e, mut b) in &mut beams {
        b.life -= dt;
        if b.life <= 0.0 {
            commands.entity(e).despawn();
        }
    }
}

#[allow(clippy::type_complexity)]
fn update_fx(
    time: Res<Time>,
    mut clock: ResMut<FxClock>,
    mut commands: Commands,
    mut systems: Query<(Entity, &mut FxSystem, &mut Visibility)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut manager: ResMut<ParticleManager>,
    cameras: Query<&GlobalTransform, With<Camera3d>>,
) {
    clock.acc += time.delta_secs().min(0.25);
    let mut steps = 0;
    while clock.acc >= STEP && steps < MAX_STEPS {
        clock.acc -= STEP;
        steps += 1;
        // The engine's `ParticleSystemManager` is global: a creation in one system can remove the
        // oldest lower-priority particle of another, so hold the systems at once.
        let mut all: Vec<(Entity, Mut<FxSystem>)> = systems.iter_mut().map(|(e, s, _)| (e, s)).collect();
        for (entity, sys) in all.iter_mut() {
            sys.step(*entity, &mut manager);
        }
        for key in manager.take_all_removals() {
            if let Some((_, sys)) = all.iter_mut().find(|(e, _)| *e == key.system) {
                sys.drop_particles(&[key]);
            }
        }
    }
    if steps == MAX_STEPS {
        clock.acc = 0.0;
    }
    // `m_fieldParticleCount` (`W3DParticleSys.cpp:141`, `:250`) is counted in the previous frame's
    // draw; `createParticle` reads it (`ParticleSys.cpp:1899`). Count every live particle of an
    // `AREA_EFFECT` ground-aligned system (the viewer does not cull off-screen particles).
    let field = systems
        .iter()
        .filter(|(_, s, _)| s.template.priority == ParticlePriority::AreaEffect && s.template.ground_aligned)
        .map(|(_, s, _)| s.particles.len())
        .sum();
    manager.field_particle_count = field;
    let cam = cameras.iter().next();
        let (right, up) = cam.map_or((Vec3::X, Vec3::Y), |t| (*t.right(), *t.up()));
    for (e, mut sys, mut vis) in &mut systems {
        let mesh = sys.build_mesh(right, up);
        if mesh.count_vertices() == 0 {
            // Nothing to draw this frame (delayed start / all particles invisible): hide the
            // entity and keep the old mesh rather than uploading an empty one.
            *vis = Visibility::Hidden;
        } else {
            match sys.mesh.clone() {
                Some(h) => {
                    if let Some(mut m) = meshes.get_mut(&h) {
                        *m = mesh;
                    }
                }
                None => {
                    let h = meshes.add(mesh);
                    commands.entity(e).insert(Mesh3d(h.clone()));
                    sys.mesh = Some(h);
                }
            }
            *vis = Visibility::Inherited;
        }
        // A forever system would otherwise emit unboundedly; cap its lifetime.
        if (sys.done() && !sys.visible()) || sys.age > 60 * 30 {
            manager.forget_system(e);
            commands.entity(e).despawn();
        }
    }
}

/// The detail level's particle cap (`MaxParticleCount`, `GameLOD.cpp:59`, `:257`; `Custom` from
/// `options.ini`) and the install's `MaxFieldParticleCount` (`GlobalData.cpp:402`, `:837`).
fn particle_limits(settings: Option<&GameSettings>, presets: &LodPresets, install: Option<&InstallDefaults>) -> (usize, usize) {
    let lod = settings.map_or_else(|| presets.effective(Detail::VeryHigh, &CustomLod::default()), |s| s.lod(presets));
    let fields = install.map_or(DEFAULT_MAX_FIELD_PARTICLES, |i| i.max_field_particle_count.max(0) as usize);
    (lod.max_particles.max(0) as usize, fields)
}

/// Applies the options' cap and the FPS-driven dynamic LOD level every frame, like
/// `W3DDisplay::draw`'s `updateAverageFPS` / `setDynamicLODLevel` (`W3DDisplay.cpp:1857-1866`).
fn sync_particle_lod(
    time: Res<Time>,
    settings: Option<Res<GameSettings>>,
    presets: Option<Res<LodPresets>>,
    install: Option<Res<InstallDefaults>>,
    mut manager: ResMut<ParticleManager>,
) {
    let fallback = LodPresets::default();
    let presets = presets.as_deref().unwrap_or(&fallback);
    let (max_particles, fields) = particle_limits(settings.as_deref(), presets, install.as_deref());
    manager.set_limits(max_particles, fields);
    manager.update_fps(time.delta_secs());
    let dynamic = settings.as_deref().map_or_else(|| presets.effective(Detail::VeryHigh, &CustomLod::default()).dynamic_lod, |s| s.lod(presets).dynamic_lod);
    manager.set_dynamic_lod(dynamic, presets);
}

/// A game teardown removes `FxAssets`: the global particle count belongs to a game.
fn reset_particles_without_game(assets: Option<Res<FxAssets>>, mut manager: ResMut<ParticleManager>) {
    if assets.is_none() {
        manager.reset();
    }
}

/// Where an FX event plays. A weapon's muzzle `FireFX` (`is_muzzle`) plays at the firing barrel's
/// `WeaponFireFXBone` when the model has one (`W3DModelDraw::handleWeaponFireFX`), else at the drawable's
/// position (`Weapon::fireWeaponTemplate`, Weapon.cpp:949-955), the object's origin. The sim's recorded point
/// (the half-height muzzle approximation) is only the fallback when the drawable is unknown. Other events
/// (death, detonation) keep the point they were recorded at.
/// Where a weapon's `FireFX` starts: the barrel's `WeaponFireFXBone` if the drawable handles it
/// (`W3DModelDraw::handleWeaponFireFX`, W3DModelDraw.cpp:3693-), else, for a contact weapon, the shot's target
/// point, else the drawable's position (`Weapon::fireWeaponTemplate`, Weapon.cpp:961-965). `contact_target` is
/// `Some` only for a contact weapon (`WeaponTemplate::isContactWeapon`).
pub fn muzzle_fx_start(recorded: [f32; 3], is_muzzle: bool, drawable: Option<[f32; 3]>, bone: Option<[f32; 3]>, contact_target: Option<[f32; 3]>) -> [f32; 3] {
    if !is_muzzle {
        return recorded;
    }
    bone.or(contact_target).or(drawable).unwrap_or(recorded)
}

/// Plays the FX events the stepping loop drained from the sim (death, muzzle, detonation).
#[allow(clippy::too_many_arguments)]
pub(crate) fn drain_fx(
    mut commands: Commands,
    sim: Option<NonSend<SimRes>>,
    cache: Res<crate::sim_view::ModelCache>,
    db: Res<FxDb>,
    map: Res<crate::MapRes>,
    mut rigs: Query<(Entity, &SimObject, &mut AnimRig, Option<&mut crate::weapon_modules::ExtraDraws>, &Transform)>,
    joints: Query<&Transform, (Without<SimObject>, Without<Camera3d>)>,
    mut queue: ResMut<FxQueue>,
    mut fx_assets: ResMut<FxAssets>,
    mut rng: ResMut<FxRng>,
    mut audio: ResMut<crate::audio::AudioQueue>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut scorches: ResMut<crate::scorch::Scorches>,
) {
    if queue.0.is_empty() {
        return;
    }
    let ground = map.ground();
    for (object, name, pos, dir, aim) in std::mem::take(&mut queue.0) {
        // Which weapon fired this `FireFX` (`Weapon::fireWeaponTemplate` passes the slot and the
        // specific barrel to `handleWeaponFireFX`): the slot whose `FireFX` matches the name.
        let fire_slot = sim.as_ref().and_then(|s| {
            s.game.logic().find_object(object).and_then(|o| {
                o.combat.weapons.weapons.iter().position(|w| w.as_ref().is_some_and(|w| w.template.fire_fx.as_deref() == Some(name.as_str())))
            })
        });
        let mut bones = BonePositions::new();
        // The drawable's position (the object's origin) is where C++ plays a muzzle FX with no barrel bone, unless the
        // weapon is a contact weapon (then the shot's target, `Weapon.cpp:963`).
        let drawable = sim.as_ref().and_then(|s| s.game.logic().find_object(object)).map(|o| [o.position.x, o.position.y, o.position.z]);
        // `WeaponTemplate::isContactWeapon`: such a weapon's un-handled `FireFX` plays at the shot's target
        // (`Weapon.cpp:963`), which `Aim::target` carries for a weapon FX.
        let contact_target = if sim.as_ref().is_some_and(|s| {
            s.game.logic().find_object(object).is_some_and(|o| {
                fire_slot.is_some_and(|slot| o.combat.weapons.weapons.get(slot).and_then(Option::as_ref).is_some_and(|w| w.template.is_contact_weapon()))
            })
        }) {
            aim.target
        } else {
            None
        };
        let mut bone = None;
        // Relocate the muzzle FX to the firing barrel's `WeaponFireFXBone`, cycle the barrel and
        // kick the `WeaponRecoilBone`; collect any `FXListAtBonePos` bone positions.
        if let Some((_, _, mut rig, mut modules, root)) = rigs.iter_mut().find(|(_, so, _, _, _)| so.0 == object) {
            if let Some(slot) = fire_slot {
                let shots = sim
                    .as_ref()
                    .and_then(|s| s.game.logic().find_object(object))
                    .and_then(|o| o.combat.weapons.weapons.get(slot).and_then(Option::as_ref).map(|w| w.template.shots_per_barrel))
                    .unwrap_or(1);
                // `Drawable::getBarrelCount` walks the draw modules and the first one with barrels for the slot
                // gives `Weapon::m_curBarrel`'s cycle length; `Drawable::handleWeaponFireFX` then walks the modules
                // again and the first whose barrel has a `WeaponFireFXBone` plays the FX (Drawable.cpp:4191-4214,
                // W3DModelDraw.cpp:3693-3764). Recoil/muzzle flashes start on every module with that barrel
                // (W3DModelDraw.cpp:3749-3764).
                let root_count = rig.barrel_count(slot);
                let extra: Vec<usize> = modules.as_ref().map_or_else(Vec::new, |m| m.modules.iter().map(|m| m.barrel_count(slot)).collect());
                let count = barrel_cycle_count(root_count, extra.iter().copied());
                let barrel = rig.advance_barrel(slot, shots, count);
                let mut walk = Vec::with_capacity(1 + extra.len());
                walk.push((root_count, root_count > 0 && rig.fire_fx_joint(slot, barrel.min(root_count.max(1) - 1)).is_some()));
                if let Some(wm) = modules.as_ref() {
                    for m in &wm.modules {
                        let c = m.barrel_count(slot);
                        walk.push((c, c > 0 && m.fire_fx_joint(slot, barrel.min(c - 1)).is_some()));
                    }
                }
                let (_, started, fx) = fire_walk(&walk);
                if let Some(wm) = modules.as_mut() {
                    for &module in &started {
                        if module == 0 {
                            rig.start_recoil(slot, barrel.min(root_count.max(1) - 1));
                        } else if let Some(m) = wm.modules.get_mut(module - 1) {
                            let c = m.barrel_count(slot);
                            m.start_shot(slot, barrel.min(c.max(1) - 1));
                        }
                    }
                } else if started.contains(&0) {
                    rig.start_recoil(slot, barrel.min(root_count.max(1) - 1));
                }
                if let Some(i) = fx {
                    let e = if i == 0 {
                        rig.fire_fx_joint(slot, barrel.min(root_count.max(1) - 1))
                    } else {
                        modules.as_ref().and_then(|wm| wm.modules.get(i - 1)).and_then(|m| m.fire_fx_joint(slot, barrel.min(m.barrel_count(slot).max(1) - 1)))
                    };
                    if let Some(e) = e {
                        if let Ok(t) = joints.get(e) {
                            bone = Some(convert::from_bevy(root.mul_transform(*t).translation));
                        }
                    }
                }
            }
            if let Some(list) = db.0.fx_list(&name) {
                for n in &list.nuggets {
                    if let zr_fx::FxNugget::FxListAtBonePos { bone_name, .. } = n {
                        let key = bone_name.to_ascii_uppercase();
                        if bones.contains_key(&key) {
                            continue;
                        }
                        let mut points = Vec::new();
                        for idx in rig.bone_indices_named(bone_name) {
                            if let Some(e) = rig.joint(idx) {
                                if let Ok(t) = joints.get(e) {
                                    let w = root.mul_transform(*t);
                                    points.push((convert::from_bevy(w.translation), convert::from_bevy(w.rotation * Vec3::X)));
                                }
                            }
                        }
                        // `getCurrentClientBonePositions` concatenates every draw module's matches (Drawable.cpp:783).
                        if let Some(wm) = modules.as_ref() {
                            for m in &wm.modules {
                                for idx in m.bone_indices_named(bone_name) {
                                    if let Some(e) = m.joint(idx) {
                                        if let Ok(t) = joints.get(e) {
                                            let w = root.mul_transform(*t);
                                            points.push((convert::from_bevy(w.translation), convert::from_bevy(w.rotation * Vec3::X)));
                                        }
                                    }
                                }
                            }
                        }
                        if !points.is_empty() {
                            bones.insert(key, points);
                        }
                    }
                }
            }
        }
        let pos = muzzle_fx_start(pos, fire_slot.is_some(), drawable, bone, contact_target);
        // `FXList` `Sound` nuggets are `AudioEventRTS` names played at the effect position.
        if let Some(list) = db.0.fx_list(&name) {
            for n in &list.nuggets {
                if let zr_fx::FxNugget::Sound { name } = n {
                    audio.push(name.clone(), None, Some(pos));
                }
            }
        }
        spawn_fx_list(
            &mut commands,
            &mut meshes,
            &mut materials,
            &mut images,
            &cache.vfs,
            &db.0,
            &mut fx_assets,
            &mut rng.0,
            &name,
            pos,
            dir,
            aim,
            Some(&ground),
            &bones,
            &mut scorches,
        );
    }
}

/// `FxBone`'s module for a bone of the draw module `sim_view` draws (the engine's module order starts at it).
pub(crate) const MAIN_DRAW_MODULE: usize = usize::MAX;

/// Spawns / despawns the `ParticleSysBone` systems of each object's current condition state
/// (damaged smoke/fire, exhaust, dust trails). Attached systems are keyed by
/// `(object, module, bone, particle system)`.
#[allow(clippy::too_many_arguments)]
fn sync_bone_fx(
    mut commands: Commands,
    rigs: Query<(&SimObject, &AnimRig, Option<&crate::weapon_modules::ExtraDraws>)>,
    db: Res<FxDb>,
    cache: Res<ModelCache>,
    mut fx_assets: ResMut<FxAssets>,
    mut rng: ResMut<FxRng>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    existing: Query<(Entity, &FxBone)>,
    mut manager: ResMut<ParticleManager>,
) {
    let mut desired: BTreeSet<(ObjectId, usize, String, String)> = BTreeSet::new();
    for (so, rig, modules) in &rigs {
        for (bone, sys) in &rig.particle_sys_bones {
            desired.insert((so.0, MAIN_DRAW_MODULE, bone.clone(), sys.clone()));
        }
        if let Some(m) = modules {
            for module in &m.modules {
                for (bone, sys) in &module.particle_sys_bones {
                    desired.insert((so.0, module.index, bone.clone(), sys.clone()));
                }
            }
        }
    }
    let mut have: BTreeSet<(ObjectId, usize, String, String)> = BTreeSet::new();
    for (e, fb) in &existing {
        let key = (fb.object, fb.module, fb.bone.clone(), fb.system.clone());
        if desired.contains(&key) {
            have.insert(key);
        } else {
            manager.forget_system(e);
            commands.entity(e).despawn();
        }
    }
    for (object, module, bone, sys) in desired {
        if have.contains(&(object, module, bone.clone(), sys.clone())) {
            continue;
        }
        let Some(template) = db.0.particle(&sys).cloned() else {
            continue;
        };
        if matches!(template.particle_type, zr_fx::ParticleType::Drawable | zr_fx::ParticleType::Smudge) {
            continue;
        }
        let material = fx_assets.material(&mut images, &mut materials, &cache.vfs, &template.particle_name, template.shader);
        let seed = ((rng.0.next_u32() as u64) << 32) | rng.0.next_u32() as u64;
        commands.spawn((
            MeshMaterial3d(material),
            Transform::IDENTITY,
            Visibility::Hidden,
            bevy::light::NotShadowCaster,
            FxSystem::new(Arc::new(template), [0.0; 3], ClientRng::new(seed), 0),
            FxBone { object, module, bone, system: sys },
        ));
    }
}

/// Moves bone-attached systems to their bone's world position (converted to Generals space).
fn update_bone_fx(
    rigs: Query<(&SimObject, &AnimRig, Option<&crate::weapon_modules::ExtraDraws>, &Transform)>,
    joints: Query<&Transform, (Without<SimObject>, Without<Camera3d>)>,
    mut systems: Query<(&FxBone, &mut FxSystem)>,
) {
    for (fb, mut sys) in &mut systems {
        let Some((_, rig, modules, root)) = rigs.iter().find(|(so, _, _, _)| so.0 == fb.object) else { continue };
        let joint = if fb.module == MAIN_DRAW_MODULE {
            rig.joint_named(&fb.bone)
        } else {
            modules.and_then(|m| m.modules.iter().find(|m| m.index == fb.module)).and_then(|m| m.joint_named(&fb.bone))
        };
        let world = if fb.bone.eq_ignore_ascii_case("none") {
            root.translation
        } else {
            joint.and_then(|e| joints.get(e).ok()).map_or(root.translation, |t| root.mul_transform(*t).translation)
        };
        sys.origin = convert::from_bevy(world);
    }
}

/// Attaches each missile's exhaust particle system at ignition (`ProjectileExhaust`, the firer's level)
/// and keeps it on the projectile's interpolated position and heading. When the sim tosses the exhaust
/// (`exhaust_lit` false) or the projectile is gone, the system is destroyed: it stops emitting and its
/// live particles finish, as `ParticleSystem::destroy` does when the parent object is removed.
#[allow(clippy::too_many_arguments)]
fn sync_projectile_fx(
    mut commands: Commands,
    sim: Option<NonSend<SimRes>>,
    db: Option<Res<FxDb>>,
    cache: Option<Res<ModelCache>>,
    fx_assets: Option<ResMut<FxAssets>>,
    rng: Option<ResMut<FxRng>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut attached: Query<(Entity, &FxProjectile, &mut FxSystem)>,
) {
    let (Some(sim), Some(db), Some(cache), Some(mut fx_assets), Some(mut rng)) = (sim, db, cache, fx_assets, rng) else {
        return;
    };
    let alpha = (sim.acc / STEP).clamp(0.0, 1.0);
    let snapshots = sim.game.projectiles();
    let mut have = BTreeSet::new();
    for (e, fp, mut sys) in &mut attached {
        match snapshots.iter().find(|s| s.id == fp.projectile && s.exhaust_lit) {
            Some(s) => {
                // Interpolated like the projectile's model (`sim_view::projectile_sync`).
                sys.origin = [
                    s.prev_pos.x + (s.pos.x - s.prev_pos.x) * alpha,
                    s.prev_pos.y + (s.pos.y - s.prev_pos.y) * alpha,
                    s.prev_pos.z + (s.pos.z - s.prev_pos.z) * alpha,
                ];
                sys.orient = Some(orientation([s.heading.x, s.heading.y, s.heading.z]));
                have.insert(fp.projectile);
            }
            None => {
                sys.destroy();
                commands.entity(e).remove::<FxProjectile>();
            }
        }
    }
    for s in &snapshots {
        if !s.exhaust_lit || have.contains(&s.id) {
            continue;
        }
        let Some(template) = s.exhaust.as_deref().and_then(|n| db.0.particle(n)).cloned() else {
            continue;
        };
        if matches!(template.particle_type, zr_fx::ParticleType::Drawable | zr_fx::ParticleType::Smudge) {
            continue;
        }
        let material = fx_assets.material(&mut images, &mut materials, &cache.vfs, &template.particle_name, template.shader);
        let seed = ((rng.0.next_u32() as u64) << 32) | rng.0.next_u32() as u64;
        let mut sys = FxSystem::new(Arc::new(template), [s.pos.x, s.pos.y, s.pos.z], ClientRng::new(seed), 0);
        sys.orient = Some(orientation([s.heading.x, s.heading.y, s.heading.z]));
        commands.spawn((
            MeshMaterial3d(material),
            Transform::IDENTITY,
            Visibility::Hidden,
            bevy::light::NotShadowCaster,
            sys,
            FxProjectile { projectile: s.id },
        ));
    }
}

/// `Drawable::getBarrelCount` (Drawable.cpp:4217-4230): the draw modules are asked in order and the first with
/// barrels for the slot gives `Weapon::m_curBarrel`'s cycle length. The root module is asked first, then the
/// object's later weapon modules.
pub(crate) fn barrel_cycle_count(root: usize, extras: impl IntoIterator<Item = usize>) -> usize {
    std::iter::once(root).chain(extras).find(|&count| count > 0).unwrap_or(0)
}

/// `Drawable::handleWeaponFireFX` (Drawable.cpp:4191-4214) over modules in `Draw` order: `modules` is
/// `(barrel_count_for_slot, barrel_has_weapon_fire_fx_bone)`. Every module that has the barrel starts its recoil and
/// muzzle flash (`W3DModelDraw::handleWeaponFireFX`, W3DModelDraw.cpp:3749-3764); the first whose barrel also has a
/// `WeaponFireFXBone` handles the FX and ends the walk. Returns `(cycle_count, started_modules, fx_module)`;
/// `fx_module` is `None` when no module has the bone (the FX then plays at the drawable, `Weapon.cpp:961-965`).
pub(crate) fn fire_walk(modules: &[(usize, bool)]) -> (usize, Vec<usize>, Option<usize>) {
    let count = modules.iter().map(|(c, _)| *c).find(|&c| c > 0).unwrap_or(0);
    let mut started = Vec::new();
    let mut fx = None;
    for (i, (module_count, has_fx)) in modules.iter().enumerate() {
        if *module_count == 0 {
            continue;
        }
        started.push(i);
        if *has_fx {
            fx = Some(i);
            break;
        }
    }
    (count, started, fx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zr_fx::{AlphaKeyframe, ColorKeyframe, RandomValue};

    fn smoke_template() -> ParticleSystemTemplate {
        let mut t = ParticleSystemTemplate {
            name: "T".into(),
            particle_name: "x.tga".into(),
            shader: ParticleShaderType::Alpha,
            lifetime: RandomValue::constant(50.0),
            start_size: RandomValue::constant(2.0),
            size_rate: RandomValue::constant(1.0),
            size_rate_damping: RandomValue::constant(1.0),
            velocity_damping: RandomValue::constant(1.0),
            burst_delay: RandomValue::constant(5.0),
            burst_count: RandomValue::constant(2.0),
            system_lifetime: 100,
            ..Default::default()
        };
        t.alpha_keys[0] = AlphaKeyframe { var: RandomValue::constant(0.0), frame: 0 };
        t.alpha_keys[1] = AlphaKeyframe { var: RandomValue::constant(1.0), frame: 10 };
        t.color_keys[0] = ColorKeyframe { color: zr_fx::Color { r: 1.0, g: 0.5, b: 0.0 }, frame: 0 };
        t
    }

    fn system(t: ParticleSystemTemplate) -> FxSystem {
        FxSystem::new(Arc::new(t), [0.0; 3], ClientRng::new(7), 0)
    }

    fn test_entity(index: u32) -> Entity {
        Entity::from_raw_u32(index).expect("valid entity index")
    }

    /// Steps one system through the global manager (the entity is the key's owner).
    fn step(s: &mut FxSystem, m: &mut ParticleManager) {
        s.step(test_entity(1), m);
    }

    #[test]
    fn burst_timing_and_count() {
        let mut m = ParticleManager::default();
        let mut s = system(smoke_template());
        step(&mut s, &mut m); // first burst
        assert_eq!(s.particles.len(), 2);
        for _ in 0..5 {
            step(&mut s, &mut m); // burst_delay counts down to zero
        }
        assert_eq!(s.particles.len(), 2);
        step(&mut s, &mut m); // delay is zero -> second burst
        assert_eq!(s.particles.len(), 4);
    }

    #[test]
    fn alpha_ramps_to_keyframe() {
        let mut m = ParticleManager::default();
        let mut s = system(smoke_template());
        step(&mut s, &mut m);
        assert_eq!(s.particles[0].alpha, 0.0);
        for _ in 0..11 {
            step(&mut s, &mut m);
        }
        // Alpha reached ~1.0 at frame 10 (small float drift).
        assert!(s.particles[0].alpha > 0.9, "{}", s.particles[0].alpha);
    }

    #[test]
    fn gravity_and_damping_change_velocity() {
        let mut t = smoke_template();
        t.velocity_type = zr_fx::EmissionVelocityType::Ortho;
        t.velocity = EmissionVelocity::Ortho {
            x: RandomValue::constant(0.0),
            y: RandomValue::constant(0.0),
            z: RandomValue::constant(0.0),
        };
        t.gravity = 1.0;
        t.velocity_damping = RandomValue::constant(0.5);
        let mut m = ParticleManager::default();
        let mut s = system(t);
        step(&mut s, &mut m);
        let z0 = s.particles[0].pos[2];
        step(&mut s, &mut m);
        assert!(s.particles[0].vel[2] > 0.0);
        assert!(s.particles[0].pos[2] > z0);
    }

    #[test]
    fn destroyed_system_stops_emitting_and_finishes_its_particles() {
        // `ParticleSystem::destroy` (a missile's exhaust once it is tossed or the missile is gone).
        let mut m = ParticleManager::default();
        let mut s = system(smoke_template());
        step(&mut s, &mut m);
        assert_eq!(s.particles.len(), 2);
        s.destroy();
        let mut prev = s.particles.len();
        for _ in 0..200 {
            step(&mut s, &mut m);
            assert!(s.particles.len() <= prev, "a destroyed system emits nothing");
            prev = s.particles.len();
        }
        assert!(s.done(), "the live particles have died and the system can be removed");
        assert_eq!(m.count, 0, "every dead particle left the global list");
    }

    fn emit_x_with_heading(heading: [f32; 3]) -> [f32; 3] {
        let mut t = smoke_template();
        t.velocity_type = zr_fx::EmissionVelocityType::Ortho;
        t.velocity = EmissionVelocity::Ortho {
            x: RandomValue::constant(10.0),
            y: RandomValue::constant(0.0),
            z: RandomValue::constant(0.0),
        };
        let mut m = ParticleManager::default();
        let mut s = system(t);
        s.orient = Some(orientation(heading));
        step(&mut s, &mut m);
        s.particles[0].vel
    }

    #[test]
    fn heading_turns_the_emission_velocity() {
        // A missile's exhaust turns with its heading: a +X emission with heading +Y points along +Y.
        let v = emit_x_with_heading([0.0, 1.0, 0.0]);
        assert!(v[0].abs() < 1e-4 && (v[1] - 10.0).abs() < 1e-4 && v[2].abs() < 1e-6, "{v:?}");
    }

    #[test]
    fn pitched_heading_tilts_the_emission_velocity() {
        // A missile climbing straight up: the exhaust points up as well (the full transform, not yaw alone).
        let v = emit_x_with_heading([0.0, 0.0, 1.0]);
        assert!(v[0].abs() < 1e-4 && v[1].abs() < 1e-4 && (v[2] - 10.0).abs() < 1e-4, "{v:?}");
        // A 45-degree dive at heading (1,0,-1)/sqrt2: the +X emission stays unit-length and points down-range.
        let s2 = std::f32::consts::FRAC_1_SQRT_2;
        let d = emit_x_with_heading([s2, 0.0, -s2]);
        assert!((d[0] - 10.0 * s2).abs() < 1e-4 && d[1].abs() < 1e-4 && (d[2] + 10.0 * s2).abs() < 1e-4, "{d:?}");
    }

    #[test]
    fn identity_orientation_leaves_bone_systems_unchanged() {
        assert_eq!(rotate(None, [1.0, 2.0, 3.0]), [1.0, 2.0, 3.0]);
    }

    /// `Weapon::fireWeaponTemplate` (Weapon.cpp:949-955): a muzzle FX on a model with no `WeaponFireFXBone` is
    /// played at the drawable's position, not at the sim's half-height muzzle point. Reverting the `drawable`
    /// fallback (using `recorded` instead) fails this test.
    #[test]
    fn muzzle_fx_without_a_fire_bone_plays_at_the_drawable() {
        let recorded = [10.0, 20.0, 7.0];
        let drawable = [10.0, 20.0, 0.0];
        assert_eq!(muzzle_fx_start(recorded, true, Some(drawable), None, None), drawable);
    }

    /// `WeaponTemplate::isContactWeapon` + `Weapon.cpp:963`: a contact weapon's `FireFX` with no barrel bone plays at
    /// the shot's target (the victim's centre), not at the firer. Reverting to the drawable fails this test.
    #[test]
    fn contact_weapon_muzzle_fx_without_a_fire_bone_plays_at_the_target() {
        let recorded = [10.0, 20.0, 7.0];
        let drawable = [10.0, 20.0, 0.0];
        let target = [40.0, 22.0, 5.0];
        assert_eq!(muzzle_fx_start(recorded, true, Some(drawable), None, Some(target)), target);
    }

    /// `W3DModelDraw::handleWeaponFireFX` (W3DModelDraw.cpp:3693-): a muzzle FX with a barrel's `WeaponFireFXBone`
    /// plays at that bone, whatever the drawable's position. Reverting to the drawable fails this test.
    #[test]
    fn muzzle_fx_with_a_fire_bone_plays_at_the_bone() {
        let bone = [11.0, 19.5, 3.0];
        assert_eq!(muzzle_fx_start([10.0, 20.0, 7.0], true, Some([10.0, 20.0, 0.0]), Some(bone), None), bone);
    }

    /// A contact weapon with a fire bone still plays at the bone: `handleWeaponFireFX` returning true skips the
    /// contact fallback (`Weapon.cpp:961-965`).
    #[test]
    fn a_contact_weapon_with_a_fire_bone_keeps_the_bone() {
        let bone = [11.0, 19.5, 3.0];
        assert_eq!(muzzle_fx_start([10.0, 20.0, 7.0], true, Some([10.0, 20.0, 0.0]), Some(bone), Some([40.0, 22.0, 5.0])), bone);
    }

    /// Death and detonation FX keep the point they were recorded at (no barrel, not a muzzle).
    #[test]
    fn non_muzzle_fx_keeps_its_recorded_point() {
        let recorded = [1.0, 2.0, 3.0];
        assert_eq!(muzzle_fx_start(recorded, false, Some([9.0, 9.0, 9.0]), Some([8.0, 8.0, 8.0]), None), recorded);
    }

    /// A template that fires `count` particles in one burst and keeps them alive for the test.
    fn burst_template(priority: ParticlePriority, count: f32) -> ParticleSystemTemplate {
        let mut t = smoke_template();
        t.priority = priority;
        t.burst_count = RandomValue::constant(count);
        t.burst_delay = RandomValue::constant(1000.0);
        t.lifetime = RandomValue::constant(1000.0);
        t.system_lifetime = 1000;
        t
    }

    /// `ParticleSystemManager::removeOldestParticles` (`ParticleSys.cpp:3554-3576`): the oldest
    /// particle of the lowest non-empty priority below the cap goes first. Reverting the priority
    /// scan to highest-first fails this test.
    #[test]
    fn oldest_particles_go_first_from_the_lowest_priority() {
        let mut m = ParticleManager::default();
        let a = test_entity(1);
        let b = test_entity(2);
        m.add_particle(a, 10, ParticlePriority::DustTrail);
        m.add_particle(a, 11, ParticlePriority::Buildup);
        m.add_particle(b, 12, ParticlePriority::DustTrail);
        assert_eq!(m.count, 3);
        // `count` is an UnsignedInt in C++: ending on the counter's post-decrement wraps it, so
        // `countToRemove - count` is one more than the requested count (`:3574`).
        assert_eq!(m.remove_oldest_particles(3, ParticlePriority::Constant), 4);
        let removed = m.take_all_removals();
        assert_eq!(
            removed,
            [
                ParticleKey { system: a, id: 10 },
                ParticleKey { system: b, id: 12 },
                ParticleKey { system: a, id: 11 },
            ],
            "both DustTrail particles oldest-first, then the older-of-one Buildup"
        );
        assert_eq!(m.count, 0);
    }

    /// At the cap, `createParticle` (`ParticleSys.cpp:1905-1910`) removes the oldest lower-priority
    /// particle and then refuses the new one: `removeOldestParticles` returns `numInExcess + 1`.
    /// Reverting the removal call (no `removeOldestParticles`) fails the removal assertion.
    #[test]
    fn a_full_cap_removes_a_lower_priority_particle_then_refuses() {
        let mut m = ParticleManager::default();
        m.set_limits(1, 30);
        let s = test_entity(1);
        m.add_particle(s, 1, ParticlePriority::DustTrail);
        m.add_particle(s, 2, ParticlePriority::DustTrail);
        assert!(!m.create_particle(0, ParticlePriority::Constant, false), "the C++ return refuses the new particle");
        let removed = m.take_all_removals();
        assert_eq!(removed, vec![ParticleKey { system: s, id: 1 }], "the oldest lower-priority particle was removed anyway");
        assert_eq!(m.count, 1);
        assert!(m.create_particle(0, ParticlePriority::Constant, false), "with room again the next creation succeeds");
    }

    /// `createParticle` refuses when nothing below its priority can be removed (`:1908`).
    /// Reverting the `!= numInExcess` check (ignoring the return) fails this test.
    #[test]
    fn a_full_cap_refuses_when_nothing_lower_can_be_removed() {
        let mut m = ParticleManager::default();
        m.set_limits(1, 30);
        m.add_particle(test_entity(1), 1, ParticlePriority::Critical);
        m.add_particle(test_entity(1), 2, ParticlePriority::Critical);
        assert!(!m.create_particle(0, ParticlePriority::DustTrail, false));
        assert!(m.take_all_removals().is_empty(), "no particle of an equal or higher priority may be removed");
        assert_eq!(m.count, 2);
    }

    /// `ALWAYS_RENDER` is exempt from every count limit (`ParticleSys.cpp:1902-1914`), including a
    /// cap of 0. Reverting the exemption fails this test.
    #[test]
    fn always_render_is_exempt_from_the_cap() {
        let mut m = ParticleManager::default();
        m.set_limits(1, 30);
        m.add_particle(test_entity(1), 1, ParticlePriority::Critical);
        m.add_particle(test_entity(1), 2, ParticlePriority::Critical);
        assert!(m.create_particle(0, ParticlePriority::AlwaysRender, false), "over the cap and still created");
        m.add_particle(test_entity(2), 3, ParticlePriority::AlwaysRender);
        assert_eq!(m.count, 3);
        assert!(m.take_all_removals().is_empty(), "the exemption removes nothing");
        m.set_limits(0, 30);
        assert!(m.create_particle(0, ParticlePriority::AlwaysRender, false), "a cap of 0 does not stop ALWAYS_RENDER");
    }

    /// `createParticle`'s `m_maxParticleCount == 0` check (`ParticleSys.cpp:1912-1913`) refuses
    /// every other particle. Reverting the check fails this test.
    #[test]
    fn a_cap_of_zero_refuses_every_normal_particle() {
        let mut m = ParticleManager::default();
        m.set_limits(0, 30);
        assert!(!m.create_particle(0, ParticlePriority::Constant, false));
        // With particles in the way, the excess removal runs first; they are removed and the
        // creation is still refused.
        m.set_limits(1, 30);
        m.add_particle(test_entity(1), 1, ParticlePriority::DustTrail);
        m.add_particle(test_entity(1), 2, ParticlePriority::DustTrail);
        m.set_limits(0, 30);
        assert!(!m.create_particle(0, ParticlePriority::Constant, false));
        assert_eq!(m.count, 0);
    }

    /// `m_maxFieldParticleCount` (`ParticleSys.cpp:1899`, default 30 `GlobalData.cpp:837`): once
    /// the previous draw counted more than 30 field particles, an `AREA_EFFECT` ground-aligned
    /// system with particles already stops creating; a non-field particle and the first particle of
    /// a system are not checked. Reverting the field check fails this test.
    #[test]
    fn field_particles_stop_at_thirty() {
        let mut m = ParticleManager::default();
        m.set_limits(5000, 30);
        m.field_particle_count = 31;
        assert!(!m.create_particle(1, ParticlePriority::AreaEffect, true), "31 > m_maxFieldParticleCount");
        assert!(m.create_particle(0, ParticlePriority::AreaEffect, true), "the system's first particle is not checked");
        assert!(m.create_particle(1, ParticlePriority::AreaEffect, false), "a billboard particle is not a field particle");
        m.field_particle_count = 30;
        assert!(m.create_particle(1, ParticlePriority::AreaEffect, true), "the check is strictly greater than");
    }

    /// The dynamic-LOD gate (`ParticleSys.cpp:1894-1897`): below `MinParticlePriority` nothing is
    /// created; below `MinParticleSkipPriority` every `ParticleSkipMask`-th creation is let through
    /// (`isParticleSkipped`, `GameLOD.h:231-234`). Reverting the first comparison fails this test.
    #[test]
    fn dynamic_lod_gates_the_priority_and_the_skip_mask() {
        let mut m = ParticleManager {
            min_dynamic_priority: ParticlePriority::UnitDamageFx,
            min_dynamic_skip_priority: ParticlePriority::Critical,
            particle_skip_mask: 1,
            ..Default::default()
        };
        assert!(!m.create_particle(0, ParticlePriority::DustTrail, false), "below MinParticlePriority");
        assert!(m.create_particle(0, ParticlePriority::DeathExplosion, false), "above MinParticlePriority");
        // Generation 1: (1 & 1) != 1 -> skipped. Generation 2: (2 & 1) == 1 -> created.
        assert!(!m.create_particle(0, ParticlePriority::WeaponTrail, false), "below MinParticleSkipPriority, skipped on this generation");
        assert!(m.create_particle(0, ParticlePriority::WeaponTrail, false), "the mask lets every second one through");
        assert!(m.create_particle(0, ParticlePriority::AlwaysRender, false), "at/above MinParticleSkipPriority is never skipped");
    }

    /// The cap comes from the detail preset's `MaxParticleCount` (`GameLOD.cpp:59`, VeryHigh 5000
    /// at `:257`) or `options.ini` at Custom, and the field cap from the install's GameData.
    /// Reverting the cap to the old fixed `MAX_PARTICLES` fails the preset/Custom assertions.
    #[test]
    fn the_cap_comes_from_the_detail_preset_and_the_options() {
        let mut db = zr_ini::IniDb::new();
        db.extend(
            zr_ini::parse(
                "GameLOD.ini",
                "StaticGameLOD Low\n MaxParticleCount = 500\n TextureReductionFactor = 1\n UseShadowVolumes = No\n\
                 UseShadowDecals = No\n UseCloudMap = No\n UseLightMap = No\n ShowSoftWaterEdge = No\n\
                 UseBuildupScaffolds = No\n UseHeatEffects = No\nEnd\n",
            )
            .expect("parses"),
        );
        let presets = LodPresets::from_ini(&db);
        let low = GameSettings { detail: Detail::Low, ..GameSettings::default() };
        assert_eq!(particle_limits(Some(&low), &presets, None).0, 500, "the install's Low block");
        let very_high = GameSettings { detail: Detail::VeryHigh, ..GameSettings::default() };
        assert_eq!(particle_limits(Some(&very_high), &presets, None).0, 5000, "GameLOD.cpp:257");
        let custom = GameSettings {
            detail: Detail::Custom,
            custom: CustomLod { max_particles: 1234, ..CustomLod::default() },
            ..GameSettings::default()
        };
        assert_eq!(particle_limits(Some(&custom), &presets, None).0, 1234, "options.ini's MaxParticleCount");
        let install = InstallDefaults { keyboard_scroll_factor: 50, max_field_particle_count: 12 };
        assert_eq!(particle_limits(Some(&very_high), &presets, Some(&install)), (5000, 12), "GameData's MaxFieldParticleCount");
    }

    /// A creation in a higher-priority system removes the oldest particle of another system
    /// (`ParticleSysManager::removeOldestParticles`); the owner drops it from its vector.
    #[test]
    fn the_cap_removes_another_systems_particle() {
        let mut m = ParticleManager::default();
        m.set_limits(1, 30);
        let a = test_entity(1);
        let b = test_entity(2);
        let mut low = system(burst_template(ParticlePriority::DustTrail, 2.0));
        let mut high = system(burst_template(ParticlePriority::Constant, 1.0));
        low.step(a, &mut m);
        assert_eq!(low.particles.len(), 2);
        high.step(b, &mut m);
        assert!(high.particles.is_empty(), "the creation at the cap is refused");
        let removed = m.take_all_removals();
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].system, a, "the removed particle belongs to the other system");
        let removed_id = removed[0].id;
        assert!(low.particles.iter().any(|p| p.id == removed_id));
        low.drop_particles(&removed);
        assert_eq!(low.particles.len(), 1, "the owner dropped the capped particle");
        assert_eq!(m.count, 1);
    }

    /// `sync_bone_fx` forgets a system before its despawn is applied; a step in between must not
    /// emit keys that nothing would drop. Reverting the retired guard fails this test.
    #[test]
    fn a_retired_system_stops_emitting_into_the_global_lists() {
        let mut m = ParticleManager::default();
        let e = test_entity(3);
        let mut s = system(burst_template(ParticlePriority::DustTrail, 1.0));
        m.forget_system(e);
        s.step(e, &mut m);
        assert!(s.particles.is_empty(), "a retired system emits nothing");
        assert_eq!(m.count, 0);
        assert!(m.take_all_removals().is_empty());
    }

    /// `Drawable::getBarrelCount` (Drawable.cpp:4217-4230) takes the first draw module with barrels for the slot.
    /// The GLA Technical's truck body has none; its turret module (`Draw[1]`) has one, so the weapon's barrel
    /// cycle must be one, not zero. Reverting the multi-module walk (root only) fails this.
    #[test]
    fn the_barrel_cycle_comes_from_the_first_module_with_barrels() {
        assert_eq!(barrel_cycle_count(0, [1]), 1, "the Technical's turret module gives the cycle");
        assert_eq!(barrel_cycle_count(0, [0, 2]), 2, "the first later module with barrels");
        assert_eq!(barrel_cycle_count(2, [1]), 2, "the root module wins when it has barrels");
        assert_eq!(barrel_cycle_count(0, [0, 0]), 0, "no module has a barrel");
    }

    /// `Drawable::handleWeaponFireFX` (Drawable.cpp:4191-4214): every module with the barrel starts its recoil and
    /// muzzle flash, and the first one with a `WeaponFireFXBone` plays the FX. The Technical's body (`0, false`)
    /// is skipped and its turret module (`1, true`) answers. Reverting the walk to the root module only fails this
    /// (it would return no FX module for the Technical).
    #[test]
    fn the_fire_walk_reaches_a_later_draw_module() {
        let (count, started, fx) = fire_walk(&[(0, false), (1, true)]);
        assert_eq!(count, 1, "the turret module's barrel");
        assert_eq!(started, vec![1], "only the module with the barrel starts recoil");
        assert_eq!(fx, Some(1), "the turret module's WeaponFireFXBone handles the FX");

        // A module with the barrel but no FX bone starts recoil and the walk continues (`Drawable.cpp:4206-4212`).
        let (_, started, fx) = fire_walk(&[(1, false), (1, true), (1, true)]);
        assert_eq!(started, vec![0, 1]);
        assert_eq!(fx, Some(1), "the first module with an FX bone ends the walk");

        // The CombatBike's body has the FX bone: the rider module (`1, true`) is never asked to fire.
        let (_, started, fx) = fire_walk(&[(1, true), (1, true)]);
        assert_eq!(started, vec![0]);
        assert_eq!(fx, Some(0));

        // No module has a FireFX bone: the FX plays at the drawable (`Weapon.cpp:961-965`).
        let (_, started, fx) = fire_walk(&[(1, false), (1, false)]);
        assert_eq!(started, vec![0, 1]);
        assert_eq!(fx, None);
    }
}
