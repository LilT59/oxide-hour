//! Client-side skeletal animation, driven by the simulation's `ModelConditionFlags`.
//!
//! W3D stores exactly one bone per vertex (`W3dVertInfStruct`; there are no
//! weights), so a Bevy `SkinnedMesh` with a per-vertex joint index and unit
//! weight reproduces the engine's `MeshGeometryClass::get_deformed_vertices`
//! exactly. Each animated object owns one joint entity per skeleton pivot
//! (flat children of the object root, set to the model-space `pose_trs`); mesh
//! parts carry a `SkinnedMesh` that references the shared inverse-bind-poses
//! asset for their skeleton.
//!
//! The model's rest vertices are already in bind model space (`zr_w3d` bakes
//! the bind bone transform), which is what Bevy skinning expects: `joint *
//! inverse_bind * bind_vertex`.
//!
//! Condition states come from `zr_w3d::DrawStates` and are selected by the sim
//! flags (`GameLogic::model_conditions`). The chosen state supplies the model
//! (handled by `sim_sync`, which respawns on a model change), the animation,
//! its mode, and the hide/show sub-objects.

use std::collections::HashMap;
use std::sync::Arc;

use bevy::mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes};
use bevy::prelude::*;
use zr_assets::Vfs;
use zr_ini::IniDb;
use zr_w3d::{quat_about_axis, AnimMode, Animation, BoneControl, ConditionFlags, DrawStates, Hierarchy, IDENTITY_QUAT};

use crate::convert;
use crate::source::ModelPart;

/// Per-template animation assets (skeleton + condition states) and per-animation clips.
#[derive(Resource, Default)]
pub struct AnimCache {
    assets: HashMap<String, Option<Arc<AnimAsset>>>,
    states: HashMap<String, Option<Arc<DrawStates>>>,
    clips: HashMap<String, Option<Arc<Animation>>>,
}

/// A template's skeleton and its condition-state table.
pub struct AnimAsset {
    pub hierarchy: Hierarchy,
    pub inv_bind: Handle<SkinnedMeshInverseBindposes>,
    pub states: Arc<DrawStates>,
}

/// One weapon barrel's bones, resolved with the engine's `%02d` suffix scheme
/// (`ModelConditionInfo::validateWeaponBarrelInfo`). `muzzle` is the `WeaponMuzzleFlash` bone;
/// the muzzle-flash sub-objects are still matched by name in `set_state_bones`.
#[derive(Clone, Copy, Default)]
pub struct BarrelInfo {
    pub fx: Option<usize>,
    pub recoil: Option<usize>,
    pub muzzle: Option<usize>,
    pub launch: Option<usize>,
}

/// `WeaponRecoilInfo::RecoilState`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum RecoilState {
    #[default]
    Idle,
    RecoilStart,
    Settle,
}

/// `WeaponRecoilInfo`: per-barrel recoil shift and rate (`W3DModelDraw::handleClientRecoil`).
#[derive(Clone, Copy, Default)]
pub(crate) struct RecoilInfo {
    pub(crate) state: RecoilState,
    pub(crate) shift: f32,
    pub(crate) rate: f32,
}

/// `W3DModelDraw` barrel-resolution and recoil constants (`W3DModelDraw.cpp`):
/// `InitialRecoilSpeed` / `MaxRecoilDistance` / `RecoilDamping` / `RecoilSettleSpeed`. The two
/// `parseVelocityReal` fields are stored per frame (the parser divides by 30).
const INITIAL_RECOIL: f32 = 2.0 / 30.0;
const MAX_RECOIL: f32 = 3.0;
const RECOIL_DAMPING: f32 = 0.4;
const RECOIL_SETTLE: f32 = 0.065 / 30.0;
const TINY_RECOIL: f32 = 0.01;

/// A live animated object: joint entities plus the mesh entities (for hide/show).
#[derive(Component)]
pub struct AnimRig {
    pub asset: Arc<AnimAsset>,
    pub states: Arc<DrawStates>,
    time: f32,
    /// The current state's clip; `None` plays the bind pose (`Base_Update`).
    clip: Option<Arc<Animation>>,
    /// Index of the current condition state in `states.states` (`None` before the first one).
    state_index: Option<usize>,
    mode: AnimMode,
    hide_show: Vec<(String, bool)>,
    joints: Vec<Entity>,
    mesh_parts: Vec<(String, Entity)>,
    /// Each mesh part's `container.name` (the engine's render-object name), by entity.
    full_names: Vec<(Entity, String)>,
    /// `Turret` / `TurretPitch` bones rotated from the sim's turret state.
    turret_bone: Option<usize>,
    turret_pitch_bone: Option<usize>,
    /// `TurretArtAngle` / `TurretArtPitch` of the current state (radians).
    turret_art_angle: f32,
    turret_art_pitch: f32,
    /// `WeaponMuzzleFlash` sub-object entities, shown while their barrel is in `RECOIL_START`.
    muzzle_flash: Vec<Entity>,
    /// `ParticleSysBone = BONE ParticleSystem` for the current state, attached by `fx::sync_bone_fx`.
    pub particle_sys_bones: Vec<(String, String)>,
    /// Per-weapon-slot barrels (fire FX / recoil / muzzle-flash / launch bones).
    barrels: [Vec<BarrelInfo>; 3],
    /// Runtime recoil state, one entry per barrel (`m_weaponRecoilInfoVec`).
    recoil: [Vec<RecoilInfo>; 3],
    /// `Weapon::m_curBarrel` / `m_numShotsForCurBarrel`, tracked client-side for barrel cycling.
    cur_barrel: [usize; 3],
    num_shots_for_cur_barrel: [i32; 3],
    /// 30 Hz accumulator for the recoil state machine (`handleClientRecoil` runs per client frame).
    recoil_acc: f32,
    /// Tire-spin bone controls from `vehicle_draw`, composed with the turret and recoil controls.
    pub vehicle_controls: Vec<BoneControl>,
}

impl AnimRig {
    /// The joint entity for pivot `index` (for the first-person bone camera).
    pub fn joint(&self, index: usize) -> Option<Entity> {
        self.joints.get(index).copied()
    }

    /// Each mesh part's `container.name` with its entity (`vehicle_draw` tests these for `TREADS`).
    pub fn full_names(&self) -> &[(Entity, String)] {
        &self.full_names
    }

    /// The joint entity for a named bone (`HEAD`, `TURRET`, ...), for bone-attached FX.
    pub fn joint_named(&self, name: &str) -> Option<Entity> {
        self.asset.hierarchy.bone_index(name).and_then(|i| self.joint(i))
    }

    /// The mesh part entity for a sub-object name: exact case-insensitive, then a substring match
    /// (the model's sub-objects carry LOD suffixes, e.g. `WeaponMuzzleFlash = TurretFX` matches
    /// `TURRETFX01`).
    fn part(&self, name: &str) -> Option<Entity> {
        let up = name.to_ascii_uppercase();
        self.mesh_parts
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .or_else(|| self.mesh_parts.iter().find(|(n, _)| n.to_ascii_uppercase().contains(&up)))
            .map(|(_, e)| *e)
    }

    /// Applies a condition state's turret / muzzle-flash / weapon-barrel bindings.
    fn set_state_bones(&mut self, sc: &StateClip) {
        // Exact bone names, as `findPristineBone` looks them up.
        self.turret_bone = sc.turret.as_deref().and_then(|n| self.asset.hierarchy.bone_index(n));
        self.turret_pitch_bone = sc.turret_pitch.as_deref().and_then(|n| self.asset.hierarchy.bone_index(n));
        self.turret_art_angle = sc.turret_art_angle;
        self.turret_art_pitch = sc.turret_art_pitch;
        self.muzzle_flash = sc.muzzle_flash.iter().flatten().filter_map(|n| self.part(n)).collect();
        self.particle_sys_bones = sc.particle_sys_bones.clone();
        // `rebuildWeaponRecoilInfo`: reset each slot's barrels and recoil state on a state change.
        for slot in 0..3 {
            self.barrels[slot] = find_barrels(
                &self.asset.hierarchy,
                sc.weapon_fire_fx_bone[slot].as_deref(),
                sc.weapon_recoil_bone[slot].as_deref(),
                sc.muzzle_flash[slot].as_deref(),
                sc.weapon_launch_bone[slot].as_deref(),
            );
            self.recoil[slot] = vec![RecoilInfo::default(); self.barrels[slot].len()];
            self.cur_barrel[slot] = 0;
            // 0 means "not yet initialised"; `fire` fills in `ShotsPerBarrel`.
            self.num_shots_for_cur_barrel[slot] = 0;
        }
    }

    /// Number of barrels of `slot` (`W3DModelDraw::getBarrelCount`).
    pub fn barrel_count(&self, slot: usize) -> usize {
        self.barrels.get(slot).map_or(0, Vec::len)
    }

    /// The barrel index the engine would use next (`privateFireWeapon`'s reset when
    /// `m_curBarrel >= barrelCount`).
    #[allow(dead_code)] // the sim now owns the launch barrel; kept for client barrel parity
    pub fn current_barrel(&self, slot: usize) -> usize {
        let n = self.barrel_count(slot);
        if n == 0 || self.cur_barrel[slot] >= n {
            0
        } else {
            self.cur_barrel[slot]
        }
    }

    /// Records a shot of `slot`: sets that barrel's recoil and advances the barrel cycle. Returns
    /// the barrel used (`Weapon::m_curBarrel`). The cycle uses this module's own barrel count; the
    /// drawable-wide walk is [`AnimRig::advance_barrel`].
    #[allow(dead_code)] // kept as the single-module form of the walk (unit tests use it)
    pub fn fire(&mut self, slot: usize, shots_per_barrel: i32) -> usize {
        if self.barrel_count(slot) == 0 {
            return 0;
        }
        let barrel = self.advance_barrel(slot, shots_per_barrel, self.barrel_count(slot));
        self.start_recoil(slot, barrel);
        barrel
    }

    /// `Weapon::privateFireWeapon`'s barrel cycle (`Weapon.cpp`): advance `m_curBarrel` after
    /// `ShotsPerBarrel` shots. `count` is the drawable's barrel count (`Drawable::getBarrelCount`,
    /// the first draw module with barrels for the slot); the cycle itself lives on the weapon.
    pub fn advance_barrel(&mut self, slot: usize, shots_per_barrel: i32, count: usize) -> usize {
        if count == 0 {
            return 0;
        }
        let pieces = shots_per_barrel.max(1);
        if self.cur_barrel[slot] >= count {
            self.cur_barrel[slot] = 0;
            self.num_shots_for_cur_barrel[slot] = pieces;
        }
        if self.num_shots_for_cur_barrel[slot] == 0 {
            self.num_shots_for_cur_barrel[slot] = pieces;
        }
        let barrel = self.cur_barrel[slot].min(count - 1);
        self.num_shots_for_cur_barrel[slot] -= 1;
        if self.num_shots_for_cur_barrel[slot] <= 0 {
            self.cur_barrel[slot] = (self.cur_barrel[slot] + 1) % count;
            self.num_shots_for_cur_barrel[slot] = pieces;
        }
        barrel
    }

    /// `handleWeaponFireFX`'s `recoil.m_state = RECOIL_START` (W3DModelDraw.cpp:3749-3764). The
    /// muzzle flash shows while this barrel is in `RECOIL_START` (`handleClientRecoil`, :2510-2530).
    pub fn start_recoil(&mut self, slot: usize, barrel: usize) {
        if let Some(r) = self.recoil.get_mut(slot).and_then(|v| v.get_mut(barrel)) {
            r.state = RecoilState::RecoilStart;
            r.rate = INITIAL_RECOIL;
        }
    }

    /// The `Joint` entity for a slot/barrel's `WeaponFireFXBone`.
    pub fn fire_fx_joint(&self, slot: usize, barrel: usize) -> Option<Entity> {
        self.barrels.get(slot)?.get(barrel)?.fx.and_then(|i| self.joint(i))
    }

    /// The `Joint` entity for a slot/barrel's `WeaponLaunchBone`.
    #[allow(dead_code)] // the sim now owns the launch position; the bone is still resolved
    pub fn launch_joint(&self, slot: usize, barrel: usize) -> Option<Entity> {
        self.barrels.get(slot)?.get(barrel)?.launch.and_then(|i| self.joint(i))
    }

    /// The bone indices an `FXListAtBonePos` `BoneName` resolves to: the unadorned name, then the
    /// `01`, `02`, ... suffixed bones (`getCurrentClientBonePositions` with `start` 0 then 1).
    pub fn bone_indices_named(&self, name: &str) -> Vec<usize> {
        let h = &self.asset.hierarchy;
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
}

/// `ModelConditionInfo::validateWeaponBarrelInfo`: find `%02d`-suffixed barrels, then fall back to
/// the unadorned names.
pub(crate) fn find_barrels(h: &Hierarchy, fx: Option<&str>, recoil: Option<&str>, muzzle: Option<&str>, launch: Option<&str>) -> Vec<BarrelInfo> {
    let at = |name: Option<&str>, suffix: Option<u32>| -> Option<usize> {
        let n = name?;
        match suffix {
            Some(i) => h.bone_index(&format!("{n}{i:02}")),
            None => h.bone_index(n),
        }
    };
    let mut out = Vec::new();
    for i in 1..=99u32 {
        let info = BarrelInfo { fx: at(fx, Some(i)), recoil: at(recoil, Some(i)), muzzle: at(muzzle, Some(i)), launch: at(launch, Some(i)) };
        if info.fx.is_none() && info.recoil.is_none() && info.muzzle.is_none() && info.launch.is_none() {
            break;
        }
        out.push(info);
    }
    if out.is_empty() {
        let info = BarrelInfo { fx: at(fx, None), recoil: at(recoil, None), muzzle: at(muzzle, None), launch: at(launch, None) };
        if info.fx.is_some() || info.recoil.is_some() || info.muzzle.is_some() || info.launch.is_some() {
            out.push(info);
        }
    }
    out
}

/// `W3DModelDraw::handleClientRecoil` for one 30 Hz client frame. A barrel without a
/// `WeaponRecoilBone` does not run the shift machine; its state resets to `IDLE` (`:2554-2561`),
/// so a muzzle-flash-only barrel blinks for one client frame.
fn step_recoil(rig: &mut AnimRig) {
    for slot in 0..3 {
        for (barrel, r) in rig.recoil[slot].iter_mut().enumerate() {
            let has_recoil = rig.barrels[slot].get(barrel).is_some_and(|b| b.recoil.is_some());
            step_recoil_one(r, has_recoil);
        }
    }
}

/// One barrel's recoil state machine (`W3DModelDraw::handleClientRecoil`, W3DModelDraw.cpp:2496-2565).
pub(crate) fn step_recoil_one(r: &mut RecoilInfo, has_recoil_bone: bool) {
    if !has_recoil_bone {
        r.state = RecoilState::Idle;
        return;
    }
    match r.state {
        RecoilState::Idle => {}
        RecoilState::RecoilStart => {
            r.shift += r.rate;
            r.rate *= RECOIL_DAMPING;
            if r.shift >= MAX_RECOIL {
                r.shift = MAX_RECOIL;
                r.state = RecoilState::Settle;
            } else if r.rate.abs() < TINY_RECOIL {
                r.state = RecoilState::Settle;
            }
        }
        RecoilState::Settle => {
            r.shift -= RECOIL_SETTLE;
            if r.shift <= 0.0 {
                r.shift = 0.0;
                r.state = RecoilState::Idle;
            }
        }
    }
}

fn bevy_quat(q: [f32; 4]) -> Quat {
    Quat::from_xyzw(q[0], q[2], -q[1], q[3]).normalize()
}

pub(crate) fn bevy_trs(t: [f32; 3], q: [f32; 4]) -> Transform {
    Transform::from_translation(convert::to_bevy(t)).with_rotation(bevy_quat(q))
}

pub(crate) fn inv_bind_mats(h: &Hierarchy) -> Vec<Mat4> {
    h.bind_pose_trs()
        .iter()
        .map(|&(t, q)| {
            let tr = bevy_trs(t, q);
            Mat4::from_scale_rotation_translation(Vec3::ONE, tr.rotation, tr.translation).inverse()
        })
        .collect()
}

impl AnimCache {
    /// The animation asset for `template` (skeleton + condition states), or `None` when it has no
    /// W3D draw states or no loadable skeleton.
    pub fn asset(
        &mut self,
        ini: &IniDb,
        vfs: &Vfs,
        inv_bind: &mut Assets<SkinnedMeshInverseBindposes>,
        template: &str,
    ) -> Option<Arc<AnimAsset>> {
        if let Some(a) = self.assets.get(template) {
            return a.clone();
        }
        let a = self.build(ini, vfs, inv_bind, template);
        self.assets.insert(template.to_owned(), a.clone());
        a
    }

    /// The template's condition-state table, cached (used to pick a state's model without loading
    /// the skeleton).
    pub fn states(&mut self, ini: &IniDb, template: &str) -> Option<Arc<DrawStates>> {
        if let Some(s) = self.states.get(template) {
            return s.clone();
        }
        let s = zr_w3d::states_for_template(ini, template).map(Arc::new);
        self.states.insert(template.to_owned(), s.clone());
        s
    }

    fn build(
        &mut self,
        ini: &IniDb,
        vfs: &Vfs,
        inv_bind: &mut Assets<SkinnedMeshInverseBindposes>,
        template: &str,
    ) -> Option<Arc<AnimAsset>> {
        let states = self.states(ini, template)?;
        // The skeleton is the default model's own (the one its sub-objects are bound to, as
        // `zr_w3d::load_model` bakes them). Clips are applied to it by pivot index
        // (`HTreeClass::Anim_Update`), never to a clip's own hierarchy. A model without one gets
        // the one-pivot default tree (`HTreeClass::Init_Default`).
        let model = zr_w3d::model_for_template(ini, template)?;
        let hierarchy = zr_w3d::model_hierarchy(vfs, &model).unwrap_or_else(zr_w3d::Hierarchy::default_tree);
        let handle = inv_bind.add(SkinnedMeshInverseBindposes::from(inv_bind_mats(&hierarchy)));
        info!("anim: {template} -> {model} ({} pivots, {} states)", hierarchy.pivot_count(), states.states.len());
        Some(Arc::new(AnimAsset { hierarchy, inv_bind: handle, states }))
    }

    /// A clip by its INI name (`HIER.ANIM`), cached.
    pub fn clip(&mut self, vfs: &Vfs, name: &str) -> Option<Arc<Animation>> {
        self.clips.entry(name.to_owned()).or_insert_with(|| zr_w3d::load_animation(vfs, name).ok().map(Arc::new)).clone()
    }
}

/// The sim's condition bits plus the client-side overlay conditions. The Chinook drop is the only
/// one: the sim tracks a rappeller's descent (`GameLogic::rappel_state`) but its `condition::derive`
/// does not set `RAPPELLING`, so the client adds it from that exposed state (the engine's
/// `AIRappelState` sets `MODELCONDITION_RAPPELLING` itself).
pub fn view_conditions(sim: &crate::sim_view::SimRes, id: zr_sim::ObjectId, flags: zr_sim::ModelConditionFlags) -> ConditionFlags {
    let mut names = flags.names();
    if crate::chinook_view::rappelling(sim, id) {
        names.push("RAPPELLING");
    }
    ConditionFlags::from_names(names)
}

fn base_name(name: &str) -> &str {
    name.split('#').next().unwrap_or(name)
}

pub(crate) fn apply_hide_show(commands: &mut Commands, parts: &[(String, Entity)], hide_show: &[(String, bool)]) {
    let hidden = |name: &str| hide_show.iter().find(|(n, _)| n.eq_ignore_ascii_case(base_name(name))).map(|(_, h)| *h);
    for (name, e) in parts {
        if let Some(hide) = hidden(name) {
            commands.entity(*e).insert(if hide { Visibility::Hidden } else { Visibility::Inherited });
        }
    }
}

pub(crate) fn part_bound(p: &ModelPart, pivots: usize) -> bool {
    match (&p.vertex_bones, p.bone) {
        (Some(v), _) => !v.is_empty() && v.iter().all(|&b| (b as usize) < pivots),
        (None, Some(b)) => b < pivots,
        _ => false,
    }
}

/// The animation chosen for a condition state, plus the state's model-attached bone names
/// (`W3DModelDraw` `ModelConditionInfo`: turret, muzzle flash).
pub struct StateClip {
    /// The state's first animation; `None` when the state has none (the bind pose plays).
    pub clip: Option<Arc<Animation>>,
    /// Index of the chosen state in `DrawStates::states`.
    pub index: usize,
    pub mode: AnimMode,
    pub hide_show: Vec<(String, bool)>,
    pub turret: Option<String>,
    pub turret_pitch: Option<String>,
    pub turret_art_angle: f32,
    pub turret_art_pitch: f32,
    pub muzzle_flash: [Option<String>; 3],
    pub weapon_fire_fx_bone: [Option<String>; 3],
    pub weapon_recoil_bone: [Option<String>; 3],
    pub weapon_launch_bone: [Option<String>; 3],
    pub particle_sys_bones: Vec<(String, String)>,
}

/// The state chosen for `flags`: its first animation (if any), mode, hide/show list and bindings.
pub fn state_clip(anim_cache: &mut AnimCache, vfs: &Vfs, states: &DrawStates, flags: &ConditionFlags) -> Option<StateClip> {
    let index = states.best_state_index(flags)?;
    let state = &states.states[index];
    let clip = state.animations.first().and_then(|a| anim_cache.clip(vfs, &a.name));
    Some(StateClip {
        clip,
        index,
        mode: state.animation_mode,
        hide_show: state.hide_show.clone(),
        turret: state.turret.clone(),
        turret_pitch: state.turret_pitch.clone(),
        turret_art_angle: state.turret_art_angle,
        turret_art_pitch: state.turret_art_pitch,
        muzzle_flash: state.weapon_muzzle_flash.clone(),
        weapon_fire_fx_bone: state.weapon_fire_fx_bone.clone(),
        weapon_recoil_bone: state.weapon_recoil_bone.clone(),
        weapon_launch_bone: state.weapon_launch_bone.clone(),
        particle_sys_bones: state.particle_sys_bones.clone(),
    })
}

/// Spawns mesh children for `root`; when `asset` is present, joints and `SkinnedMesh` bindings
/// are added too (otherwise the parts render in rest pose).
#[allow(clippy::too_many_arguments)]
pub fn populate(
    commands: &mut Commands,
    root: Entity,
    parts: &[ModelPart],
    handles: &[(Handle<Mesh>, Handle<StandardMaterial>)],
    asset: Option<&Arc<AnimAsset>>,
    flags: &ConditionFlags,
    anim_cache: &mut AnimCache,
    vfs: &Vfs,
) -> Vec<Entity> {
    let Some(asset) = asset else { return populate_rest(commands, root, handles) };
    let pivots = asset.hierarchy.pivot_count();
    if pivots == 0 || parts.iter().zip(handles).all(|(p, _)| !part_bound(p, pivots)) {
        return populate_rest(commands, root, handles);
    }

    // The chosen state's first animation gives the initial pose and clip.
    let initial = state_clip(anim_cache, vfs, &asset.states, flags);
    // Joints start at the state's pose at frame 0, or at the bind pose when the state has no clip.
    // A joint left at identity would pull every skinned part back toward the bone's origin.
    let start = asset.hierarchy.pose_trs_controlled(initial.as_ref().and_then(|s| s.clip.as_deref()), 0.0, &[]);
    let mut joints = Vec::with_capacity(pivots);
    commands.entity(root).with_children(|c| {
        for &(t, q) in &start {
            joints.push(c.spawn((bevy_trs(t, q), Visibility::default())).id());
        }
    });

    let mut mesh_parts = Vec::with_capacity(handles.len());
    let mut full_names = Vec::with_capacity(handles.len());
    commands.entity(root).with_children(|c| {
        for (p, (m, mat)) in parts.iter().zip(handles) {
            let mut e = c.spawn((
                Mesh3d(m.clone()),
                MeshMaterial3d(mat.clone()),
                Transform::IDENTITY,
                Visibility::default(),
            ));
            if part_bound(p, pivots) {
                e.insert(SkinnedMesh { joints: joints.clone(), inverse_bindposes: asset.inv_bind.clone() });
            }
            mesh_parts.push((base_name(&p.name).to_owned(), e.id()));
            full_names.push((e.id(), p.full_name.clone()));
        }
    });

    let (clip, state_index, mode, hide_show) = match initial.as_ref() {
        Some(s) => (s.clip.clone(), Some(s.index), s.mode, s.hide_show.clone()),
        None => (None, None, AnimMode::Once, Vec::new()),
    };
    apply_hide_show(commands, &mesh_parts, &hide_show);
    let part_entities: Vec<Entity> = mesh_parts.iter().map(|(_, e)| *e).collect();
    let mut rig = AnimRig {
        asset: asset.clone(),
        states: asset.states.clone(),
        time: 0.0,
        clip,
        state_index,
        mode,
        hide_show,
        joints,
        mesh_parts,
        full_names,
        turret_bone: None,
        turret_pitch_bone: None,
        turret_art_angle: 0.0,
        turret_art_pitch: 0.0,
        muzzle_flash: Vec::new(),
        particle_sys_bones: Vec::new(),
        barrels: Default::default(),
        recoil: Default::default(),
        cur_barrel: [0; 3],
        num_shots_for_cur_barrel: [1; 3],
        recoil_acc: 0.0,
        vehicle_controls: Vec::new(),
    };
    if let Some(sc) = &initial {
        rig.set_state_bones(sc);
    }
    commands.entity(root).insert(rig);
    part_entities
}

fn populate_rest(
    commands: &mut Commands,
    root: Entity,
    handles: &[(Handle<Mesh>, Handle<StandardMaterial>)],
) -> Vec<Entity> {
    let mut out = Vec::with_capacity(handles.len());
    commands.entity(root).with_children(|c| {
        for (m, mat) in handles {
            out.push(c.spawn((Mesh3d(m.clone()), MeshMaterial3d(mat.clone()), Transform::IDENTITY)).id());
        }
    });
    out
}

/// Advances animation time at render rate and writes the joint poses. LOOP wraps within a clip;
/// ONCE holds the last frame.
pub(crate) fn frame_for(a: &Animation, mode: AnimMode, time: f32) -> f32 {
    let rate = a.frame_rate.max(1) as f32;
    let total = a.frame_count.max(1) as f32;
    let t = time * rate;
    match mode {
        AnimMode::Loop | AnimMode::LoopPingPong | AnimMode::Manual => t % total,
        AnimMode::Once => t.min(total - 1.0),
        AnimMode::LoopBackwards => (total - 1.0) - t % total,
        AnimMode::OnceBackwards => (total - 1.0 - t).max(0.0),
    }
}

pub fn add_systems(app: &mut App) {
    app.add_systems(Update, animate_rigs);
    app.add_systems(
        Update,
        update_anim_state
            .run_if(resource_exists::<crate::sim_view::SimActive>)
            .before(animate_rigs),
    );
}

/// Picks each sim object's condition state from the simulation flags and switches the clip.
fn update_anim_state(
    sim: NonSend<crate::sim_view::SimRes>,
    cache: Res<crate::sim_view::ModelCache>,
    mut anim_cache: ResMut<AnimCache>,
    mut commands: Commands,
    mut rigs: Query<(&crate::sim_view::SimObject, &mut AnimRig, Option<&crate::shroud::GhostSnapshot>)>,
) {
    for (so, mut rig, ghost) in &mut rigs {
        // A ghost snapshot keeps its last-seen model / hide-show state.
        if ghost.is_some() {
            continue;
        }
        let Some(flags) = sim.game.logic().model_conditions(so.0) else { continue };
        let flags = view_conditions(&sim, so.0, flags);
        let states = rig.states.clone();
        let Some(sc) = state_clip(&mut anim_cache, &cache.vfs, &states, &flags) else { continue };
        if Some(sc.index) != rig.state_index {
            rig.clip = sc.clip.clone();
            rig.state_index = Some(sc.index);
            rig.mode = sc.mode;
            rig.hide_show = sc.hide_show.clone();
            rig.time = 0.0;
            let parts = rig.mesh_parts.clone();
            apply_hide_show(&mut commands, &parts, &rig.hide_show);
            rig.set_state_bones(&sc);
        }
    }
}

#[allow(clippy::type_complexity)]
pub(crate) fn animate_rigs(
    time: Res<Time>,
    sim: Option<NonSend<crate::sim_view::SimRes>>,
    mut rigs: Query<(&crate::sim_view::SimObject, &mut AnimRig, Option<&crate::shroud::GhostSnapshot>)>,
    mut transforms: Query<&mut Transform, Without<AnimRig>>,
    mut vis: Query<&mut Visibility, Without<AnimRig>>,
) {
    let dt = time.delta_secs();
    for (so, mut rig, ghost) in &mut rigs {
        // A ghost snapshot freezes its animation at the last-seen frame (`W3DRenderObjectSnapshot`
        // copies the render object and stops updating it).
        if ghost.is_some() {
            continue;
        }
        // `handleClientRecoil` shows a barrel's `WeaponMuzzleFlash` sub-objects while that barrel is in
        // `RECOIL_START` (W3DModelDraw.cpp:2510-2530); the state machine then advances once per 30 Hz frame, and
        // a barrel with no `WeaponRecoilBone` resets to `IDLE` (so a flash-only barrel blinks one frame).
        let muzzle_shown = rig.recoil.iter().flatten().any(|r| r.state == RecoilState::RecoilStart);
        rig.recoil_acc += dt;
        while rig.recoil_acc >= crate::sim_view::STEP {
            rig.recoil_acc -= crate::sim_view::STEP;
            step_recoil(&mut rig);
        }
        let (clip, frame) = match rig.clip.clone() {
            Some(clip) => {
                rig.time += dt;
                let frame = frame_for(&clip, rig.mode, rig.time);
                (Some(clip), frame)
            }
            None => (None, 0.0),
        };
        // Bone controls, composed into the pose so a child bone follows its controlled parent
        // (`HTreeClass::Control_Bone`). Turret yaw is `Rotate_Z`, pitch `Rotate_Y(-pitch)`
        // (`W3DModelDraw::handleClientTurretPositioning`), both in the bone's own frame.
        let mut controls = Vec::new();
        if let Some(sim) = &sim {
            if let Some(o) = sim.game.logic().find_object(so.0) {
                if let Some(b) = rig.turret_bone {
                    let yaw = zr_sim::math::angle_diff(o.combat.turret_angle, o.angle) + rig.turret_art_angle;
                    controls.push(BoneControl { bone: b, translation: [0.0; 3], rotation: quat_about_axis([0.0, 0.0, 1.0], yaw) });
                }
                if let Some(b) = rig.turret_pitch_bone {
                    let pitch = o.combat.turret_pitch + rig.turret_art_pitch;
                    controls.push(BoneControl { bone: b, translation: [0.0; 3], rotation: quat_about_axis([0.0, 1.0, 0.0], -pitch) });
                }
            }
        }
        // Recoiling barrels: `Translate_X(-shift)` on the recoil bone (`W3DModelDraw` recoil).
        for slot in 0..3 {
            for barrel in 0..rig.barrel_count(slot) {
                let shift = rig.recoil[slot][barrel].shift;
                if shift <= 0.0 {
                    continue;
                }
                if let Some(b) = rig.barrels[slot][barrel].recoil {
                    controls.push(BoneControl { bone: b, translation: [-shift, 0.0, 0.0], rotation: IDENTITY_QUAT });
                }
            }
        }
        controls.extend_from_slice(&rig.vehicle_controls);
        let pose: Vec<([f32; 3], [f32; 4])> = rig.asset.hierarchy.pose_trs_controlled(clip.as_deref(), frame, &controls);
        for (j, &e) in rig.joints.iter().enumerate() {
            if let (Some(&(t, q)), Ok(mut tr)) = (pose.get(j), transforms.get_mut(e)) {
                *tr = bevy_trs(t, q);
            }
        }
        // Muzzle flash sub-objects: shown while the barrel is in `RECOIL_START`, hidden otherwise.
        for &e in &rig.muzzle_flash {
            if let Ok(mut v) = vis.get_mut(e) {
                *v = if muzzle_shown { Visibility::Inherited } else { Visibility::Hidden };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `handleClientRecoil` (W3DModelDraw.cpp:2496-2565): `handleWeaponFireFX` sets a barrel to `RECOIL_START`, the
    /// muzzle flash is shown while it is there, and a barrel with no `WeaponRecoilBone` is reset to `IDLE` by the
    /// next client frame (its flash blinks once). Reverting the no-recoil-bone reset to the old shared machine (which
    /// leaves the state at `RECOIL_START` for several frames) fails this.
    #[test]
    fn a_muzzle_flash_only_barrel_blinks_one_client_frame() {
        let mut r = RecoilInfo { state: RecoilState::RecoilStart, shift: 0.0, rate: INITIAL_RECOIL };
        assert_eq!(r.state, RecoilState::RecoilStart, "the shot's frame shows the flash");
        step_recoil_one(&mut r, false);
        assert_eq!(r.state, RecoilState::Idle, "no recoil bone: reset after one client frame");
        step_recoil_one(&mut r, false);
        assert_eq!(r.state, RecoilState::Idle);

        r.state = RecoilState::RecoilStart;
        r.rate = INITIAL_RECOIL;
        step_recoil_one(&mut r, true);
        assert!(r.shift > 0.0, "a recoil bone gets the shift");
        for _ in 0..1000 {
            step_recoil_one(&mut r, true);
            if r.state == RecoilState::Idle {
                break;
            }
        }
        assert_eq!((r.state, r.shift), (RecoilState::Idle, 0.0), "the shift settles back to zero");
    }
}
