//! Every `W3D*Draw` module of a live drawable, not just the first: the engine's `Drawable::draw` calls
//! `doDrawModule` on each module in `Draw` order (Drawable.cpp:2604-2653), and every `W3DModelDraw` selects its own
//! condition state, draws its own render object at the drawable transform plus its own adjustments and runs its own
//! animation, turret, recoil, muzzle-flash and bone-attached particle systems (`W3DModelDraw::doDrawModule`,
//! W3DModelDraw.cpp:2043-2115; `adjustTransformMtx`, :1989-2008). `sim_view` draws the first module; this system draws
//! the rest.
//!
//! Module data handled here:
//! * `AttachToBoneInAnotherModule`: the named bone's cached translation is added to the module transform
//!   (`CACHE_ATTACH_BONE`, `W3DModelDraw.cpp:3363-3388`; resolved by `zr_game::launch_bones::weapon_draw_modules`).
//! * `OkToChangeModelColor`: only those modules take the player colour (`replaceIndicatorColor`,
//!   W3DModelDraw.cpp:3206-3226); the others keep their own house-colour material, which the tint with white leaves
//!   unchanged.
//! * `MinLODRequired`: a module whose static LOD is above the game's is not created (`Drawable::setDrawModule`,
//!   Drawable.cpp:446).
//! * `ADJUST_HEIGHT_BY_CONSTRUCTION_PERCENT`: the state's lift is added to the module, less the main module's lift
//!   that the object root already carries (`adjustTransformMtx`, W3DModelDraw.cpp:2010-2020).
//! * Upgrade-gated parts are ordinary condition states (`WEAPONSET_*`, `PLAYER_UPGRADE`, ...): the sim's
//!   `model_conditions` select them, exactly like the first module.
//! * `TransitionState` / `WaitForStateToFinishIfPossible` / idle-animation restarts reuse `construct_modules`' state
//!   machine (the same one that drew the construction modules before this system took them over).
//! * `W3DDependencyModelDraw`'s `AttachToBoneInContainer` moves a contained rider's root onto its container's bone
//!   (`W3DDependencyModelDraw::adjustTransformMtx`, W3DDependencyModelDraw.cpp:120-155); the container's
//!   `W3DOverlord*Draw` clears the dependency each frame, which in the viewer is always true while the container
//!   exists.
//!
//! Not ported (documented): `ProjectileBoneFeedbackEnabledSlots`, `ReceivesDynamicLights` and `AnimationsRequirePower`
//! are parsed but no shipped module uses them (survey: 0 of 813 extra modules); the module's `AltTurret` is not
//! parsed by `zr-w3d`; `W3DTreeDraw`/`W3DPropDraw` objects are drawn by `source::object_model` instead (they have one
//! module and no condition states).

use std::collections::HashMap;
use std::sync::Arc;

use bevy::mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes};
use bevy::prelude::*;
use zr_assets::Vfs;
use zr_ini::IniDb;
use zr_sim::ObjectId;
use zr_w3d::{quat_about_axis, AnimMode, Animation, BoneControl, DrawStates, Hierarchy, IDENTITY_QUAT};

use crate::anim::{self, AnimCache, BarrelInfo, RecoilInfo, RecoilState};
use crate::construct_modules::{
    choose_anim, completion_secs, module_lift, request, rig_frame, shown_state, start_frame, ExtraCache, Gfx, Shown,
};
use crate::options::{Detail, GameSettings};
use crate::sim_view::{ModelCache, SimActive, SimObject, SimRes, STEP};
use crate::{construction_view, convert, team_color};

/// The extra draw modules of one live object, in `Draw` order after the module `sim_view` draws.
#[derive(Component)]
pub struct ExtraDraws {
    pub modules: Vec<ExtraModuleRig>,
}

/// One extra `W3D*Draw` module: its state table, its rendered skeleton/meshes and its per-barrel weapon state.
pub struct ExtraModuleRig {
    /// Position among the object's `Draw` blocks (the engine's draw-module order).
    pub index: usize,
    states: Arc<DrawStates>,
    /// `AttachToBoneInAnotherModule`'s cached translation (object space).
    attach: [f32; 3],
    /// The house colour for `HOUSECOLOR` sub-objects (`OkToChangeModelColor`; white leaves the art's own colour).
    rgb: [u8; 3],
    /// The state's model currently spawned, if any.
    model: Option<String>,
    hierarchy: Option<Arc<Hierarchy>>,
    joints: Vec<Entity>,
    /// Mesh sub-objects by base name (for hide/show and muzzle flash matching).
    parts: Vec<(String, Entity)>,
    /// Parts not bound to a joint; their transform carries the attach/lift offset.
    unbound: Vec<Entity>,
    muzzle_flash: Vec<Entity>,
    /// `ParticleSysBone = BONE ParticleSystem` of the current state.
    pub particle_sys_bones: Vec<(String, String)>,
    // State machine (`W3DModelDraw::setModelState` / `doDrawModule`).
    shown: Option<Shown>,
    pending: Option<usize>,
    anim: usize,
    elapsed: f32,
    duration: f32,
    clip: Option<Arc<Animation>>,
    mode: AnimMode,
    start: f32,
    /// The shown state's `ADJUST_HEIGHT_BY_CONSTRUCTION_PERCENT`.
    lift_adjusts: bool,
    // Client controls (`handleClientTurretPositioning`, `handleClientRecoil`).
    turret_bone: Option<usize>,
    turret_pitch_bone: Option<usize>,
    turret_art_angle: f32,
    turret_art_pitch: f32,
    barrels: [Vec<BarrelInfo>; 3],
    recoil: [Vec<RecoilInfo>; 3],
    recoil_acc: f32,
}

impl ExtraModuleRig {
    /// `W3DModelDraw::getBarrelCount` for this module's current state.
    pub fn barrel_count(&self, slot: usize) -> usize {
        self.barrels.get(slot).map_or(0, Vec::len)
    }

    /// `handleWeaponFireFX`'s recoil/muzzle-flash start for one barrel.
    pub fn start_shot(&mut self, slot: usize, barrel: usize) {
        if let Some(r) = self.recoil.get_mut(slot).and_then(|v| v.get_mut(barrel)) {
            r.state = RecoilState::RecoilStart;
            r.rate = 2.0 / 30.0;
        }
    }

    /// The `Joint` entity for a slot/barrel's `WeaponFireFXBone`, if this module's state has one.
    pub fn fire_fx_joint(&self, slot: usize, barrel: usize) -> Option<Entity> {
        self.barrels.get(slot)?.get(barrel)?.fx.and_then(|i| self.joints.get(i).copied())
    }

    /// The joint entities a `FXListAtBonePos` `BoneName` resolves to (unadorned, then `01`, `02`, ...).
    pub fn bone_indices_named(&self, name: &str) -> Vec<usize> {
        let Some(h) = &self.hierarchy else { return Vec::new() };
        let mut out = Vec::new();
        if let Some(i) = h.bone_index(name) {
            out.push(i);
        }
        for k in 1..=99u32 {
            match h.bone_index(&format!("{name}{k:02}")) {
                Some(i) => out.push(i),
                None => break,
            }
        }
        out
    }

    /// The joint entity for a named bone, for bone-attached particle systems and the container dependency.
    pub fn joint_named(&self, name: &str) -> Option<Entity> {
        self.hierarchy.as_ref()?.bone_index(name).and_then(|i| self.joints.get(i).copied())
    }

    /// The joint entity for a pivot index.
    pub fn joint(&self, index: usize) -> Option<Entity> {
        self.joints.get(index).copied()
    }

    fn part(&self, name: &str) -> Option<Entity> {
        let up = name.to_ascii_uppercase();
        self.parts
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .or_else(|| self.parts.iter().find(|(n, _)| n.to_ascii_uppercase().contains(&up)))
            .map(|(_, e)| *e)
    }

    /// Applies a state's barrel/turret/muzzle bindings (`W3DModelDraw::setModelState` -> `rebuildWeaponRecoilInfo`).
    fn set_state(&mut self, st: &zr_w3d::ConditionState) {
        self.turret_bone = self.hierarchy.as_ref().and_then(|h| st.turret.as_deref().and_then(|n| h.bone_index(n)));
        self.turret_pitch_bone = self.hierarchy.as_ref().and_then(|h| st.turret_pitch.as_deref().and_then(|n| h.bone_index(n)));
        self.turret_art_angle = st.turret_art_angle;
        self.turret_art_pitch = st.turret_art_pitch;
        self.muzzle_flash = st.weapon_muzzle_flash.iter().flatten().filter_map(|n| self.part(n)).collect();
        self.particle_sys_bones = st.particle_sys_bones.clone();
        for slot in 0..3 {
            self.barrels[slot] = self
                .hierarchy
                .as_ref()
                .map(|h| {
                    anim::find_barrels(
                        h,
                        st.weapon_fire_fx_bone[slot].as_deref(),
                        st.weapon_recoil_bone[slot].as_deref(),
                        st.weapon_muzzle_flash[slot].as_deref(),
                        st.weapon_launch_bone[slot].as_deref(),
                    )
                })
                .unwrap_or_default();
            self.recoil[slot] = vec![RecoilInfo::default(); self.barrels[slot].len()];
        }
    }

    /// `W3DModelDraw::handleClientTurretPositioning` (W3DModelDraw.cpp:2440-2468): the turret bone turns by the
    /// TurretAI's body-relative yaw plus the state's `TurretArtAngle`; the pitch is `TurretArtPitch` before
    /// `Rotate_Y(-pitch)`.
    fn pose(&self, turret: (f32, f32), lift: f32) -> Vec<([f32; 3], [f32; 4])> {
        let Some(h) = &self.hierarchy else { return Vec::new() };
        let mut controls = Vec::new();
        if let Some(b) = self.turret_bone {
            controls.push(BoneControl {
                bone: b,
                translation: [0.0; 3],
                rotation: quat_about_axis([0.0, 0.0, 1.0], turret_yaw(turret.0, self.turret_art_angle)),
            });
        }
        if let Some(b) = self.turret_pitch_bone {
            controls.push(BoneControl {
                bone: b,
                translation: [0.0; 3],
                rotation: quat_about_axis([0.0, 1.0, 0.0], -turret_pitch(turret.1, self.turret_art_pitch)),
            });
        }
        for slot in 0..3 {
            for (barrel, info) in self.barrels[slot].iter().enumerate() {
                let shift = self.recoil[slot].get(barrel).map_or(0.0, |r| r.shift);
                if shift > 0.0 {
                    if let Some(b) = info.recoil {
                        controls.push(BoneControl { bone: b, translation: [-shift, 0.0, 0.0], rotation: IDENTITY_QUAT });
                    }
                }
            }
        }
        let frame = self.clip.as_ref().map_or(0.0, |c| rig_frame(self.mode, self.start, self.elapsed, c.frame_rate, c.frame_count));
        let mut pose = h.pose_trs_controlled(self.clip.as_deref(), frame, &controls);
        for (t, _) in &mut pose {
            t[0] += self.attach[0];
            t[1] += self.attach[1];
            t[2] += self.attach[2] + lift;
        }
        pose
    }
}

/// `W3DModelDraw::handleClientTurretPositioning` (W3DModelDraw.cpp:2440-2451): the turret bone turns by the TurretAI's
/// body-relative yaw plus the state's `TurretArtAngle` (the art is often modelled 180 degrees backward, e.g. the GLA
/// Technical's 20Cal). The pitch is the same with `TurretArtPitch` but negated for `Rotate_Y(-pitch)` (:2454-2468).
pub(crate) fn turret_yaw(relative_angle: f32, art_angle: f32) -> f32 {
    relative_angle + art_angle
}

/// The turret pitch before `Rotate_Y(-pitch)` (`handleClientTurretPositioning`, W3DModelDraw.cpp:2454-2468).
pub(crate) fn turret_pitch(relative_pitch: f32, art_pitch: f32) -> f32 {
    relative_pitch + art_pitch
}

/// Caches per template the module list, and per template the first module's `OkToChangeModelColor`.
#[derive(Resource, Default)]
pub(crate) struct WeaponModuleCache {
    templates: HashMap<String, Arc<Vec<zr_game::launch_bones::WeaponDrawModule>>>,
}

impl WeaponModuleCache {
    fn modules(&mut self, ini: &IniDb, vfs: &Vfs, template: &str) -> Arc<Vec<zr_game::launch_bones::WeaponDrawModule>> {
        self.templates
            .entry(template.to_owned())
            .or_insert_with(|| Arc::new(zr_game::launch_bones::weapon_draw_modules(ini, vfs, template)))
            .clone()
    }

    /// `OkToChangeModelColor` of the module `sim_view` draws (the first `W3D*Draw` with states).
    pub(crate) fn first_ok_to_change_model_color(&mut self, ini: &IniDb, vfs: &Vfs, template: &str) -> bool {
        self.modules(ini, vfs, template).first().is_some_and(|m| m.ok_to_change_model_color)
    }

    /// The first module's `AttachToBoneInContainer` (a `W3DDependencyModelDraw` rider attached to its carrier).
    pub(crate) fn first_container_bone(&mut self, ini: &IniDb, vfs: &Vfs, template: &str) -> Option<String> {
        self.modules(ini, vfs, template).first().and_then(|m| m.attach_container_bone.clone())
    }
}

/// The static game LOD in force (`GameSettings::detail`; the viewer default is `VeryHigh`).
fn static_lod(settings: Option<&GameSettings>) -> usize {
    settings.map_or(Detail::VeryHigh.index(), |s| s.detail.index())
}

/// `Drawable::setDrawModule` (Drawable.cpp:446): a module whose `MinLODRequired` is above the static game LOD is not
/// created.
fn lod_allows(min_lod: &Option<String>, static_lod: usize) -> bool {
    match min_lod.as_deref() {
        None => true,
        Some(name) => Detail::from_key(name).is_none_or(|d| d.index() <= static_lod),
    }
}

/// Whether the extra-module system draws a module: it has a model in some state, and its `MinLODRequired` is not
/// above the static game LOD (`Drawable::setDrawModule`, Drawable.cpp:446). The first module is drawn by `sim_view`.
pub(crate) fn should_draw(m: &zr_game::launch_bones::WeaponDrawModule, static_lod: usize) -> bool {
    lod_allows(&m.min_lod, static_lod) && has_model(&m.states)
}
/// Whether any state of a module names a model (otherwise it can never draw).
fn has_model(states: &DrawStates) -> bool {
    states.states.iter().any(|s| s.model.is_some()) || states.transitions.iter().any(|t| t.state.model.is_some())
}

/// Whether the module's shown animation has completed: a module with no model has nothing to wait for, and only a
/// clip that can complete (a `ONCE` / `ONCE_BACKWARDS` one) ever ends.
fn is_complete(m: &ExtraModuleRig) -> bool {
    match m.shown {
        None => true,
        Some(_) if m.model.is_none() => true,
        Some(_) => m.duration.is_finite() && m.elapsed >= m.duration,
    }
}

struct Visual {
    model: String,
    hierarchy: Arc<Hierarchy>,
    joints: Vec<Entity>,
    parts: Vec<(String, Entity)>,
    unbound: Vec<Entity>,
}

/// Spawns a module's skeleton joints and mesh parts under the object root, posed at `frame`.
#[allow(clippy::too_many_arguments)]
fn spawn_visual(
    commands: &mut Commands,
    root: Entity,
    cx: &mut ExtraCache,
    vfs: &Vfs,
    gfx: &mut Gfx,
    model: &str,
    attach: [f32; 3],
    lift: f32,
    rgb: [u8; 3],
    clip: Option<&Animation>,
    frame: f32,
) -> Option<Visual> {
    let parts = cx.parts(vfs, model)?;
    let hierarchy = cx.skeleton(vfs, model);
    let pivots = hierarchy.pivot_count();
    let bind = cx.bind(gfx.inv_bind, model, &hierarchy);
    let handles = cx.handles(gfx, model, rgb, &parts, pivots);
    let mut pose = hierarchy.pose_trs_controlled(clip, frame, &[]);
    for (t, _) in &mut pose {
        t[0] += attach[0];
        t[1] += attach[1];
        t[2] += attach[2] + lift;
    }
    let mut joints = Vec::with_capacity(pose.len());
    commands.entity(root).with_children(|c| {
        for &(t, q) in &pose {
            joints.push(c.spawn((anim::bevy_trs(t, q), Visibility::default())).id());
        }
    });
    let mut out = Vec::with_capacity(parts.len());
    let mut unbound = Vec::new();
    commands.entity(root).with_children(|c| {
        for (p, (m, mat)) in parts.iter().zip(handles) {
            let bound = anim::part_bound(p, pivots);
            let local = if bound { Transform::IDENTITY } else { Transform::from_translation(convert::to_bevy([attach[0], attach[1], attach[2] + lift])) };
            let mut e = c.spawn((Mesh3d(m), MeshMaterial3d(mat), local, Visibility::default()));
            if bound {
                e.insert(SkinnedMesh { joints: joints.clone(), inverse_bindposes: bind.clone() });
            } else {
                unbound.push(e.id());
            }
            out.push((p.name.split('#').next().unwrap_or(&p.name).to_owned(), e.id()));
        }
    });
    Some(Visual { model: model.to_owned(), hierarchy, joints, parts: out, unbound })
}

/// Removes a module's current skeleton joints and mesh parts.
fn despawn_visual(commands: &mut Commands, m: &mut ExtraModuleRig) {
    for &e in &m.joints {
        commands.entity(e).try_despawn();
    }
    for &(_, e) in &m.parts {
        commands.entity(e).try_despawn();
    }
    m.joints.clear();
    m.parts.clear();
    m.unbound.clear();
    m.model = None;
    m.hierarchy = None;
}

/// Shows `shown` on module `m`: `W3DModelDraw::setModelState` (model swap, clip/start frame, hide/show, barrels).
#[allow(clippy::too_many_arguments)]
fn show(
    commands: &mut Commands,
    root: Entity,
    m: &mut ExtraModuleRig,
    shown: Shown,
    avoid: Option<usize>,
    cx: &mut ExtraCache,
    anim_cache: &mut AnimCache,
    vfs: &Vfs,
    gfx: &mut Gfx,
    lift: f32,
    rng: &mut zr_fx::ClientRng,
) {
    let st = shown_state(&m.states, shown).clone();
    m.shown = Some(shown);
    m.elapsed = 0.0;
    m.duration = 0.0;
    m.anim = choose_anim(st.animations.len(), avoid, rng);
    let clip_name = st.animations.get(m.anim).map(|a| a.name.as_str());
    let clip = clip_name.and_then(|n| anim_cache.clip(vfs, n));
    m.start = clip.as_deref().map_or(0.0, |c| {
        let start = start_frame(st.animation_mode, &st.flags, c.frame_count, rng);
        m.duration = completion_secs(st.animation_mode, start, c.frame_count, c.frame_rate);
        start
    });
    if clip.is_none() {
        m.duration = f32::INFINITY;
    }
    m.clip = clip;
    m.mode = st.animation_mode;
    m.lift_adjusts = construction_view::adjusts_height(&st.flags);
    let model = st.model.clone();
    if model != m.model {
        despawn_visual(commands, m);
        if let Some(model) = &model {
            let frame = m.clip.as_ref().map_or(0.0, |c| rig_frame(m.mode, m.start, 0.0, c.frame_rate, c.frame_count));
            if let Some(v) = spawn_visual(commands, root, cx, vfs, gfx, model, m.attach, lift, m.rgb, m.clip.as_deref(), frame) {
                m.model = Some(v.model);
                m.hierarchy = Some(v.hierarchy);
                m.joints = v.joints;
                m.parts = v.parts;
                m.unbound = v.unbound;
            }
        }
    }
    m.set_state(&st);
    anim::apply_hide_show(commands, &m.parts, &st.hide_show);
}

/// Spawns the extra draw modules of each object (`sim_sync` has just created the root).
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn spawn_extra_draws(
    mut commands: Commands,
    sim: Option<NonSend<SimRes>>,
    cache: Res<ModelCache>,
    settings: Option<Res<GameSettings>>,
    mut modules: ResMut<WeaponModuleCache>,
    mut anim_cache: ResMut<AnimCache>,
    mut cx: ResMut<ExtraCache>,
    mut inv_bind: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut fx_rng: Option<ResMut<crate::fx::FxRng>>,
    roots: Query<(Entity, &SimObject, &anim::AnimRig, Option<&ExtraDraws>, Option<&crate::shroud::GhostSnapshot>)>,
) {
    let Some(sim) = &sim else { return };
    let lod = static_lod(settings.as_deref());
    let mut fallback = zr_fx::ClientRng::new(0x5eed_0138);
    for (root, so, _, have, ghost) in &roots {
        if have.is_some() || ghost.is_some() {
            continue;
        }
        let Some(o) = sim.game.logic().find_object(so.0) else { continue };
        let list = modules.modules(&cache.ini, &cache.vfs, &o.template.name);
        if list.len() <= 1 {
            continue;
        }
        let flags = anim::view_conditions(sim, so.0, o.model_conditions);
        let player_rgb = cache.color_of(o.player);
        let mut rigs = Vec::new();
        for m in list.iter().skip(1) {
            if !should_draw(m, lod) {
                continue;
            }
            let rgb = if m.ok_to_change_model_color { player_rgb } else { team_color::NEUTRAL };
            let Some(state) = m.states.best_state_index(&flags) else { continue };
            let mut rig = ExtraModuleRig {
                index: m.index,
                states: Arc::new(m.states.clone()),
                attach: m.attach.unwrap_or([0.0; 3]),
                rgb,
                model: None,
                hierarchy: None,
                joints: Vec::new(),
                parts: Vec::new(),
                unbound: Vec::new(),
                muzzle_flash: Vec::new(),
                particle_sys_bones: Vec::new(),
                shown: None,
                pending: None,
                anim: 0,
                elapsed: 0.0,
                duration: 0.0,
                clip: None,
                mode: AnimMode::Loop,
                start: 0.0,
                lift_adjusts: false,
                turret_bone: None,
                turret_pitch_bone: None,
                turret_art_angle: 0.0,
                turret_art_pitch: 0.0,
                barrels: Default::default(),
                recoil: Default::default(),
                recoil_acc: 0.0,
            };
            let rng: &mut zr_fx::ClientRng = match fx_rng.as_deref_mut() {
                Some(r) => &mut r.0,
                None => &mut fallback,
            };
            let mut gfx = Gfx { meshes: &mut meshes, materials: &mut materials, images: &mut images, inv_bind: &mut inv_bind };
            show(&mut commands, root, &mut rig, Shown::State(state), None, &mut cx, &mut anim_cache, &cache.vfs, &mut gfx, 0.0, rng);
            rigs.push(rig);
        }
        if !rigs.is_empty() {
            commands.entity(root).insert(ExtraDraws { modules: rigs });
        }
    }
}

/// Runs every extra module: state selection, transitions, model swaps, animation, turret/recoil controls, construction
/// lift, muzzle-flash visibility and bone-attached particle-system bindings (`W3DModelDraw::doDrawModule`,
/// `setModelState`, `handleClientRecoil`).
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_extra_draws(
    mut commands: Commands,
    time: Res<Time>,
    sim: Option<NonSend<SimRes>>,
    cache: Res<ModelCache>,
    mut anim_cache: ResMut<AnimCache>,
    mut cx: ResMut<ExtraCache>,
    mut inv_bind: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut fx_rng: Option<ResMut<crate::fx::FxRng>>,
    mut roots: Query<(Entity, &SimObject, &mut ExtraDraws, &Transform, Option<&crate::shroud::GhostSnapshot>)>,
    mut transforms: Query<(&mut Transform, Option<&mut Visibility>), Without<ExtraDraws>>,
) {
    let Some(sim) = &sim else { return };
    let dt = time.delta_secs();
    let mut fallback = zr_fx::ClientRng::new(0x5eed_0139);
    for (root, so, mut extra, root_tf, ghost) in &mut roots {
        if ghost.is_some() {
            continue;
        }
        let Some(o) = sim.game.logic().find_object(so.0) else { continue };
        let flags = anim::view_conditions(sim, so.0, o.model_conditions);
        let turret = (zr_sim::math::angle_diff(o.combat.turret_angle, o.angle), o.combat.turret_pitch);
        // The main module's lift already in the root transform (`sim_sync` adds it to the object position).
        let root_lift = root_tf.translation.y - convert::to_bevy([o.position.x, o.position.y, o.position.z]).y;
        for m in &mut extra.modules {
            m.elapsed += dt;
            let complete = is_complete(m);
            if complete {
                match m.shown {
                    Some(Shown::Transition { to, .. }) => {
                        let rng: &mut zr_fx::ClientRng = match fx_rng.as_deref_mut() {
                            Some(r) => &mut r.0,
                            None => &mut fallback,
                        };
                        let mut gfx = Gfx { meshes: &mut meshes, materials: &mut materials, images: &mut images, inv_bind: &mut inv_bind };
                        let lift = module_lift(m.lift_adjusts, &o.template.geometry, o.construction_percent, o.under_construction) - root_lift;
                        show(&mut commands, root, m, Shown::State(to), None, &mut cx, &mut anim_cache, &cache.vfs, &mut gfx, lift, rng);
                    }
                    Some(Shown::State(c)) => {
                        if let Some(p) = m.pending.take() {
                            let (next, pend) = request(&m.states, Some(Shown::State(c)), None, p, true);
                            m.pending = pend;
                            if Some(next) != m.shown {
                                let rng: &mut zr_fx::ClientRng = match fx_rng.as_deref_mut() {
                                    Some(r) => &mut r.0,
                                    None => &mut fallback,
                                };
                                let mut gfx = Gfx { meshes: &mut meshes, materials: &mut materials, images: &mut images, inv_bind: &mut inv_bind };
                                let lift = module_lift(m.lift_adjusts, &o.template.geometry, o.construction_percent, o.under_construction) - root_lift;
                                show(&mut commands, root, m, next, None, &mut cx, &mut anim_cache, &cache.vfs, &mut gfx, lift, rng);
                            }
                        } else if m.model.is_some() && m.duration.is_finite() && m.duration > 0.0 {
                            // An idle clip, or `RESTART_ANIM_WHEN_COMPLETE`, plays again (`adjustAnimation(curState, -1)`).
                            let st = &m.states.states[c];
                            let restart = st.animations.get(m.anim).is_some_and(|a| a.idle) || st.flags.iter().any(|f| f == "RESTART_ANIM_WHEN_COMPLETE");
                            if restart {
                                let avoid = Some(m.anim);
                                let rng: &mut zr_fx::ClientRng = match fx_rng.as_deref_mut() {
                                    Some(r) => &mut r.0,
                                    None => &mut fallback,
                                };
                                let mut gfx = Gfx { meshes: &mut meshes, materials: &mut materials, images: &mut images, inv_bind: &mut inv_bind };
                                let lift = module_lift(m.lift_adjusts, &o.template.geometry, o.construction_percent, o.under_construction) - root_lift;
                                show(&mut commands, root, m, Shown::State(c), avoid, &mut cx, &mut anim_cache, &cache.vfs, &mut gfx, lift, rng);
                            }
                        }
                    }
                    None => {}
                }
            }
            if let Some(want) = m.states.best_state_index(&flags) {
                let (next, pend) = request(&m.states, m.shown, m.pending, want, is_complete(m));
                m.pending = pend;
                if Some(next) != m.shown {
                    let rng: &mut zr_fx::ClientRng = match fx_rng.as_deref_mut() {
                        Some(r) => &mut r.0,
                        None => &mut fallback,
                    };
                    let mut gfx = Gfx { meshes: &mut meshes, materials: &mut materials, images: &mut images, inv_bind: &mut inv_bind };
                    let lift = module_lift(m.lift_adjusts, &o.template.geometry, o.construction_percent, o.under_construction) - root_lift;
                    show(&mut commands, root, m, next, None, &mut cx, &mut anim_cache, &cache.vfs, &mut gfx, lift, rng);
                }
            }
            // Muzzle flashes show while a barrel is in `RECOIL_START`; read before the state machine advances.
            let muzzle_shown = m.recoil.iter().flatten().any(|r| r.state == RecoilState::RecoilStart);
            m.recoil_acc += dt;
            while m.recoil_acc >= STEP {
                m.recoil_acc -= STEP;
                for slot in 0..3 {
                    for (barrel, r) in m.recoil[slot].iter_mut().enumerate() {
                        let has_recoil = m.barrels[slot].get(barrel).is_some_and(|b| b.recoil.is_some());
                        anim::step_recoil_one(r, has_recoil);
                    }
                }
            }
            if m.model.is_some() {
                let lift = module_lift(m.lift_adjusts, &o.template.geometry, o.construction_percent, o.under_construction) - root_lift;
                let pose = m.pose(turret, lift);
                for (j, &e) in m.joints.iter().enumerate() {
                    if let (Some(&(t, q)), Ok((mut tr, _))) = (pose.get(j), transforms.get_mut(e)) {
                        *tr = anim::bevy_trs(t, q);
                    }
                }
                let local = convert::to_bevy([m.attach[0], m.attach[1], m.attach[2] + lift]);
                for &e in &m.unbound {
                    if let Ok((mut tr, _)) = transforms.get_mut(e) {
                        tr.translation = local;
                    }
                }
            }
            for &e in &m.muzzle_flash {
                if let Ok((_, Some(mut v))) = transforms.get_mut(e) {
                    *v = if muzzle_shown { Visibility::Inherited } else { Visibility::Hidden };
                }
            }
        }
    }
}

/// `W3DDependencyModelDraw::adjustTransformMtx` (W3DDependencyModelDraw.cpp:120-155): a rider carried by a
/// non-enclosing container is drawn at the `AttachToBoneInContainer` bone of its carrier (the carrier's
/// `W3DOverlord*Draw` clears the dependency every frame, so the rider always draws while the carrier exists). The
/// carrier's bone may be in its first module or in one of its extra modules.
#[allow(clippy::type_complexity)]
fn attach_dependency_roots(
    sim: Option<NonSend<SimRes>>,
    mut module_cache: ResMut<WeaponModuleCache>,
    cache: Res<ModelCache>,
    mut riders: ParamSet<(Query<(&SimObject, &mut Transform)>, Query<(&SimObject, &Transform, &anim::AnimRig, Option<&ExtraDraws>)>)>,
    joints: Query<&Transform, (Without<SimObject>, Without<Camera3d>)>,
) {
    let Some(sim) = &sim else { return };
    let mut wanted: HashMap<ObjectId, Transform> = HashMap::new();
    {
        let roots = riders.p1();
        for (so, root, rig, extras) in roots.iter() {
            let Some(carrier) = sim.game.logic().find_object(so.0) else { continue };
            let Some(ct) = carrier.template.contain.as_ref() else { continue };
            // Only a non-enclosing container uses the dependency transform (`isEnclosingContainerFor`).
            if ct.is_enclosing_container && ct.kind == zr_sim::contain::ContainKind::Garrison {
                continue;
            }
            for &rider in &carrier.contained {
                let Some(rider_obj) = sim.game.logic().find_object(rider) else { continue };
                // `AttachToBoneInContainer` is on the rider's own `W3DDependencyModelDraw`, naming a bone of the carrier.
                let Some(bone) = module_cache.first_container_bone(&cache.ini, &cache.vfs, &rider_obj.template.name) else { continue };
                let world = rig
                    .joint_named(&bone)
                    .or_else(|| extras.and_then(|e| e.modules.iter().find_map(|m| m.joint_named(&bone))))
                    .and_then(|e| joints.get(e).ok())
                    .map_or(*root, |t| root.mul_transform(*t));
                wanted.insert(rider, world);
            }
        }
    }
    for (so, mut tf) in riders.p0().iter_mut() {
        if let Some(world) = wanted.get(&so.0) {
            *tf = *world;
        }
    }
}

pub fn add_systems(app: &mut App) {
    app.init_resource::<WeaponModuleCache>();
    app.add_systems(
        Update,
        spawn_extra_draws
            .after(crate::sim_view::sim_sync)
            .before(update_extra_draws)
            .run_if(resource_exists::<SimActive>)
            .run_if(resource_exists::<ModelCache>)
            .run_if(resource_exists::<AnimCache>),
    );
    app.add_systems(
        Update,
        update_extra_draws
            .after(crate::sim_view::sim_sync)
            .before(crate::fx::drain_fx)
            .run_if(resource_exists::<SimActive>)
            .run_if(resource_exists::<ModelCache>)
            .run_if(resource_exists::<AnimCache>),
    );
    app.add_systems(
        Update,
        attach_dependency_roots
            .after(update_extra_draws)
            .before(crate::fx::drain_fx)
            .run_if(resource_exists::<SimActive>)
            .run_if(resource_exists::<ModelCache>),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `handleClientTurretPositioning` (W3DModelDraw.cpp:2440-2451) adds the state's `TurretArtAngle` to the
    /// TurretAI's body-relative angle. The Technical's art is modelled 180 degrees backward, so at relative 0 the
    /// model's rest muzzle (-X) must turn to +X (toward the target). Reverting the art term (plain `relative`) leaves
    /// the muzzle at -X and fails the second assertion.
    #[test]
    fn the_turret_art_angle_turns_the_backwards_art_forward() {
        use std::f32::consts::PI;
        assert_eq!(turret_yaw(0.0, PI), PI, "relative 0 + art 180");
        assert_eq!(turret_yaw(1.0, 0.5), 1.5, "an ordinary angle is added unchanged");
        let muzzle = zr_w3d::quat_rotate(&quat_about_axis([0.0, 0.0, 1.0], turret_yaw(0.0, PI)), [-1.0, 0.0, 0.0]);
        assert!(muzzle[0] > 0.999 && muzzle[1].abs() < 1e-4, "the 20Cal's backward muzzle now points +X: {muzzle:?}");
        assert_eq!(turret_pitch(0.1, 0.2), 0.3);
    }

    /// `Drawable::setDrawModule` (Drawable.cpp:446): `MinLODRequired` above the static game LOD excludes the module.
    #[test]
    fn a_modules_min_lod_follows_the_static_game_lod() {
        assert!(lod_allows(&None, Detail::Low.index()));
        assert!(!lod_allows(&Some("Medium".into()), Detail::Low.index()), "Medium is hidden at Low");
        assert!(lod_allows(&Some("Medium".into()), Detail::Medium.index()));
        assert!(lod_allows(&Some("Medium".into()), Detail::VeryHigh.index()));
        assert!(!lod_allows(&Some("VeryHigh".into()), Detail::High.index()));
    }

    /// Every extra module with a model is drawn, not only the weapon-bearing ones; `MinLODRequired` filters. Reverting
    /// `should_draw` to a weapon-bone test would drop the structure parts (the extra AmericaCommandCenter modules),
    /// which this covers.
    #[test]
    fn every_extra_module_with_a_model_is_drawn() {
        let mut states = DrawStates { module: "W3DModelDraw".into(), states: vec![], default_state: None, ignore: zr_w3d::ConditionFlags::new(), transitions: vec![] };
        states.states.push(zr_w3d::ConditionState { model: Some("Part".into()), ..Default::default() });
        let weaponless = zr_game::launch_bones::WeaponDrawModule {
            index: 3,
            states: states.clone(),
            attach_bone: None,
            attach_container_bone: None,
            ok_to_change_model_color: false,
            min_lod: None,
            receives_dynamic_lights: false,
            animations_require_power: false,
            projectile_feedback_slots: 0,
            attach: None,
        };
        assert!(should_draw(&weaponless, Detail::VeryHigh.index()), "a non-weapon extra module is drawn");
        let mut low = weaponless.clone();
        low.min_lod = Some("Medium".into());
        assert!(!should_draw(&low, Detail::Low.index()), "Medium is not created at Low");
        assert!(should_draw(&low, Detail::Medium.index()));
        let mut empty = weaponless.clone();
        empty.states.states[0].model = None;
        assert!(!should_draw(&empty, Detail::VeryHigh.index()), "a module with no model is not drawn");
    }
}
