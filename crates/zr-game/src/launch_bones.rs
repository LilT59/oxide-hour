//! Projectile launch bones: resolve every object's `WeaponLaunchBone` rest positions per weapon slot and barrel, plus
//! the turret pivots, from the install's W3D art, and hand them to the sim.
//!
//! Ported from `Drawable::getProjectileLaunchOffset` (`GameEngine/Source/GameClient/Drawable.cpp:662-671`) over
//! `W3DModelDraw::getProjectileLaunchOffset` (`Core/GameEngineDevice/.../W3DModelDraw.cpp:3248-3395`) and
//! `ModelConditionInfo::validateWeaponBarrelInfo` (`:716-856`):
//!
//! * The drawable walks **every** draw module in `Draw` order and the first module whose `findBestInfo` state for the
//!   drawable's conditions has a barrel for the weapon slot answers; a module whose state has none is skipped
//!   (Drawable.cpp:662-671). Only modules whose class derives from `W3DModelDraw` implement `ObjectDrawInterface`
//!   (`W3DModelDraw::getObjectDrawInterface`, W3DModelDraw.h:421-422), so only those are asked. The GLA Technical's
//!   turret module (`ModuleTag_02`, a `W3DModelDraw`) carries the crate-upgrade `Muzzle02`/`Muzzle03` bones while its
//!   first module is the truck body; `LaunchBones::select_slot` merges the module tables per slot.
//! * A module's `findBestInfo` is its own (`W3DModelDrawModuleData::findBestInfo`), including its own
//!   `IgnoreConditionStates` (`:1711-1716`).
//! * `WeaponLaunchBone` is scanned `01..99` and stops at the first missing index (`validateWeaponBarrelInfo`:755-809);
//!   an empty list retries the unadorned name once (`:811-850`). The pristine bones are the state model posed by its
//!   **first animation at frame 0** (or its last frame with `PRISTINE_BONE_POS_IN_FINAL_FRAME`) — that is what
//!   `ModelConditionInfo::validateCachedBones` caches (`:590-713`: `Set_Animation(animToUse, whichFrame, MANUAL)` before
//!   reading the bones) — else the bind pose. The engine's barrel vec also holds entries whose only bones are the slot's
//!   fx/recoil/muzzle bones, with an identity launch matrix; see [`barrel_positions`] for why the sim keeps only the
//!   `WeaponLaunchBone` entries (the launch positions are the same for every shipped slot).
//! * The state's `Turret` / `TurretPitch` bone rest positions are the pivots `calcProjectileLaunchPosition` rotates
//!   about (`Weapon.cpp:3073-3098`), and the state's `TurretArtAngle` / `TurretArtPitch` are applied to its barrels
//!   (`W3DModelDraw.cpp:3336-3340`).
//! * `AttachToBoneInAnotherModule` adds the attach bone's translation to both turret pivots (`CACHE_ATTACH_BONE`,
//!   W3DModelDraw.h:277, W3DModelDraw.cpp:3363-3388); the offset is cached per module on its first launch query and
//!   resolved through `Drawable::getPristineBonePositions` (`:754-780`), i.e. from the first module whose selected
//!   state's model has the bone. This port resolves it at load from the empty condition set (the engine's first query),
//!   and documents the difference when a later state swaps the model that carries the attach bone.
//!
//! The sim selects the state at fire time with the object's model conditions (`LaunchBones::select_slot`, the engine's
//! `findBestInfo`), and the environment bits (`NIGHT`/`SNOW`, [`zr_sim::weapon::LaunchEnvironment`]) are part of the
//! query like every other condition.
//!
//! Known gaps (documented): `AltTurret` (`m_turrets[1]`) is not parsed by `zr-w3d`, so a state's second turret's bones
//! are not resolved (only the battleship uses an `AltTurret` block, and no launch bone depends on it in shipped data).

use std::collections::BTreeMap;

use zr_assets::Vfs;
use zr_ini::{Block, IniDb};
use zr_sim::weapon::{LaunchBarrel, LaunchBones, LaunchModule, LaunchState, TurretLaunch};
use zr_templates::{NameEnum, Templates, WeaponSlot};
use zr_w3d::{ConditionFlags, ConditionState, DrawStates, Hierarchy, Trs};

/// `WeaponLaunchBone` is scanned `01..99` and stops at the first missing index (`validateWeaponBarrelInfo`).
const MAX_BARRELS: u32 = 99;

/// The `W3D*Draw` classes whose module data derives from `W3DModelDrawModuleData` (`Core/GameEngineDevice/Include/
/// W3DDevice/GameClient/Module/*.h`). Only they implement `ObjectDrawInterface`, so `Drawable::getProjectileLaunchOffset`
/// can only be answered by them; `W3DPropDraw`, `W3DDefaultDraw`, `W3DTreeDraw`, `W3DDebrisDraw`, `W3DRopeDraw`,
/// `W3DTracerDraw`, `W3DLaserDraw` and `W3DProjectileStreamDraw` have no launch bones.
const LAUNCH_DRAW_MODULES: &[&str] = &[
    "W3DModelDraw",
    "W3DDependencyModelDraw",
    "W3DScienceModelDraw",
    "W3DSupplyDraw",
    "W3DTankDraw",
    "W3DTankTruckDraw",
    "W3DTruckDraw",
    "W3DPoliceCarDraw",
    "W3DOverlordTankDraw",
    "W3DOverlordTruckDraw",
    "W3DOverlordAircraftDraw",
];

/// One `W3DModelDraw`-family `Draw` block of an object.
struct LaunchDraw {
    /// Position among the object's `Draw` blocks (0-based, counting every `Draw` block).
    index: usize,
    states: DrawStates,
    /// `AttachToBoneInAnotherModule` (`W3DModelDrawModuleData::m_attachToDrawableBone`).
    attach_bone: Option<String>,
    /// `AttachToBoneInContainer` (`W3DDependencyModelDrawModuleData::m_attachToDrawableBoneInContainer`).
    attach_container_bone: Option<String>,
    /// `W3DModelDrawModuleData::m_okToChangeModelColor`: the module honours the player colour.
    ok_to_change_model_color: bool,
    /// `W3DModelDrawModuleData::m_minLODRequired` (`StaticGameLODLevel` name, `Low`..`VeryHigh`).
    min_lod: Option<String>,
    /// `W3DModelDrawModuleData::m_receivesDynamicLights`.
    receives_dynamic_lights: bool,
    /// `W3DModelDrawModuleData::m_animationsRequirePower`: animations pause while underpowered.
    animations_require_power: bool,
    /// `W3DModelDrawModuleData::m_projectileBoneFeedbackEnabledSlots` (`WeaponSlotType` names).
    projectile_feedback_slots: u8,
}

/// Parse a `Yes`/`No` INI value (the engine's `INI::parseBool`).
fn parse_yes(v: Option<&str>) -> bool {
    v.is_some_and(|s| s.trim().eq_ignore_ascii_case("yes") || s.trim() == "1" || s.trim().eq_ignore_ascii_case("true"))
}

/// `INI::parseBitString32` with `TheWeaponSlotTypeNames`: a list of `PRIMARY` / `SECONDARY` / `TERTIARY`.
fn parse_weapon_slot_bits(v: &str) -> u8 {
    v.split(|c: char| c.is_whitespace() || c == ',' || c == ';')
        .filter_map(|w| match w.trim().to_ascii_uppercase().as_str() {
            "PRIMARY" => Some(1u8),
            "SECONDARY" => Some(2),
            "TERTIARY" => Some(4),
            _ => None,
        })
        .fold(0, |b, x| b | x)
}

/// `INI::parseStaticGameLODLevel`: `Low`..`VeryHigh`, normalised to the `StaticGameLODNames` spelling.
fn parse_static_lod(v: &str) -> Option<String> {
    match v.trim().to_ascii_lowercase().as_str() {
        "low" => Some("Low".into()),
        "medium" => Some("Medium".into()),
        "high" => Some("High".into()),
        "veryhigh" | "very_high" | "very high" => Some("VeryHigh".into()),
        _ => None,
    }
}

/// One `W3DModelDraw`-family `Draw` module of an object, as the client's weapon-FX walk needs it: its condition
/// states, its `AttachToBoneInAnotherModule` and that bone's cached translation.
#[derive(Clone, Debug)]
pub struct WeaponDrawModule {
    /// Position among the object's `Draw` blocks (0-based, counting every `Draw` block).
    pub index: usize,
    /// The module's condition-state table (`W3DModelDrawModuleData::m_conditionStates`).
    pub states: DrawStates,
    /// `AttachToBoneInAnotherModule` (`W3DModelDrawModuleData::m_attachToDrawableBone`).
    pub attach_bone: Option<String>,
    /// `AttachToBoneInContainer` (`W3DDependencyModelDrawModuleData::m_attachToDrawableBoneInContainer`).
    pub attach_container_bone: Option<String>,
    /// `W3DModelDrawModuleData::m_okToChangeModelColor`: only these modules take the player colour.
    pub ok_to_change_model_color: bool,
    /// `W3DModelDrawModuleData::m_minLODRequired` (`Low`..`VeryHigh`); modules above the static game LOD are not
    /// created (`Drawable::setDrawModule`, Drawable.cpp:446).
    pub min_lod: Option<String>,
    /// `W3DModelDrawModuleData::m_receivesDynamicLights`.
    pub receives_dynamic_lights: bool,
    /// `W3DModelDrawModuleData::m_animationsRequirePower`.
    pub animations_require_power: bool,
    /// `W3DModelDrawModuleData::m_projectileBoneFeedbackEnabledSlots` as a three-slot bitmask.
    pub projectile_feedback_slots: u8,
    /// The cached attach translation (`getAttachToDrawableBoneOffset`, W3DModelDraw.cpp:1147-1174): the
    /// `AttachToBoneInAnotherModule` bone from the empty condition state's first module that has it.
    pub attach: Option<[f32; 3]>,
}

/// Every `W3DModelDraw`-family draw module of `template` in `Draw` order, with its attach-bone offset, for the
/// client's fire-FX / recoil / muzzle-flash walk (`Drawable::handleWeaponFireFX`, Drawable.cpp:4191-4214) and its
/// bone-attached particle systems (`W3DModelDraw::updateBonesForClientParticleSystems`). The viewer draws the first
/// module itself; this lists the others (the GLA Technical's turret attach module, the Strategy Center's cannon).
pub fn weapon_draw_modules(ini: &IniDb, vfs: &Vfs, template: &str) -> Vec<WeaponDrawModule> {
    let draws = launch_draws(ini, template);
    if draws.is_empty() {
        return Vec::new();
    }
    let mut hierarchies = BTreeMap::new();
    let mut animations = BTreeMap::new();
    let attach = attach_offsets(vfs, &draws, &mut hierarchies, &mut animations);
    draws
        .into_iter()
        .map(|d| WeaponDrawModule {
            index: d.index,
            attach: attach.get(&d.index).copied().flatten(),
            attach_bone: d.attach_bone,
            attach_container_bone: d.attach_container_bone,
            ok_to_change_model_color: d.ok_to_change_model_color,
            min_lod: d.min_lod,
            receives_dynamic_lights: d.receives_dynamic_lights,
            animations_require_power: d.animations_require_power,
            projectile_feedback_slots: d.projectile_feedback_slots,
            states: d.states,
        })
        .collect()
}

/// The launch bones of every template whose model has a `WeaponLaunchBone` in some condition state, keyed by name.
pub fn launch_bones(ini: &IniDb, vfs: &Vfs, templates: &Templates) -> BTreeMap<String, LaunchBones> {
    let mut out = BTreeMap::new();
    // A model is shared by many templates; load each hierarchy and animation once.
    let mut hierarchies: BTreeMap<String, Hierarchy> = BTreeMap::new();
    let mut animations: BTreeMap<String, zr_w3d::Animation> = BTreeMap::new();
    for t in templates.things() {
        let draws: Vec<LaunchDraw> = launch_draws(ini, &t.name);
        if draws.is_empty() {
            continue;
        }
        // The weapon slots `TurretAI` controls (`ControlledWeaponSlots`).
        let controlled = t.ai().and_then(|ai| ai.turret.as_ref()).map_or(0, |tu| weapon_slot_bits(tu.controlled_weapon_slots));
        let attach = attach_offsets(vfs, &draws, &mut hierarchies, &mut animations);
        let mut modules: Vec<LaunchModule> = Vec::new();
        for d in &draws {
            let mut states = Vec::new();
            for state in &d.states.states {
                states.push(resolve_state(vfs, state, controlled, attach.get(&d.index).copied().flatten(), &mut hierarchies, &mut animations));
            }
            // Only a module with at least one barrel list can ever answer `getProjectileLaunchOffset`.
            if states.iter().any(|s| s.slots.iter().any(|b| !b.is_empty())) {
                modules.push(LaunchModule { ignore: flags_bits(&d.states.ignore), states });
            }
        }
        if modules.is_empty() {
            continue;
        }
        let first = modules.remove(0);
        let mut lb = LaunchBones { ignore: first.ignore, states: first.states, extra_modules: modules, ..LaunchBones::default() };
        // The state the empty query resolves (the engine's `getProjectileLaunchOffset` with no conditions): per slot the
        // first module that has a barrel, and the turret of the module that answers slot 0.
        for slot in 0..3 {
            let (barrels, turret) = {
                let sel = lb.select_slot(slot, 0);
                (sel.barrels.to_vec(), sel.turret.cloned())
            };
            lb.slots[slot] = barrels;
            if lb.turret.is_none() {
                lb.turret = turret;
            }
        }
        out.insert(t.name.clone(), lb);
    }
    out
}

/// The object's `W3DModelDraw`-family `Draw` blocks, following `ObjectReskin` to its parent (the same resolution as
/// `zr_w3d::draw_states_by_module`) and keeping the module-level fields `zr-w3d` does not store.
fn launch_draws(ini: &IniDb, template: &str) -> Vec<LaunchDraw> {
    let parsed: BTreeMap<usize, DrawStates> = zr_w3d::draw_states_by_module(ini, template).into_iter().collect();
    let Some(&first) = parsed.keys().next() else { return Vec::new() };
    let mut name = template.to_string();
    for _ in 0..16 {
        let Some(obj) = ini.resolved("Object", &name) else { return Vec::new() };
        let draws: Vec<&Block> = obj.children_named("Draw").collect();
        // `draw_states_by_module` may have followed the reskin parent: only use this object's Draw list when the parsed
        // module at `first` is the block at that index.
        let matches = draws
            .get(first)
            .and_then(|d| d.header.first())
            .zip(parsed.get(&first))
            .is_some_and(|(header, states)| *header == states.module);
        if matches {
            return parsed
                .into_iter()
                .filter(|(_, states)| LAUNCH_DRAW_MODULES.contains(&states.module.as_str()))
                .map(|(index, states)| {
                    let block = draws[index];
                    let attach_bone = block
                        .value("AttachToBoneInAnotherModule")
                        .map(str::trim)
                        .filter(|v| !v.is_empty() && !v.eq_ignore_ascii_case("none"))
                        .map(str::to_string);
                    let attach_container_bone = block
                        .value("AttachToBoneInContainer")
                        .map(str::trim)
                        .filter(|v| !v.is_empty() && !v.eq_ignore_ascii_case("none"))
                        .map(str::to_string);
                    LaunchDraw {
                        index,
                        states,
                        attach_bone,
                        attach_container_bone,
                        ok_to_change_model_color: parse_yes(block.value("OkToChangeModelColor")),
                        min_lod: block.value("MinLODRequired").and_then(parse_static_lod),
                        receives_dynamic_lights: parse_yes(block.value("ReceivesDynamicLights")),
                        animations_require_power: parse_yes(block.value("AnimationsRequirePower")),
                        projectile_feedback_slots: block.value("ProjectileBoneFeedbackEnabledSlots").map_or(0, parse_weapon_slot_bits),
                    }
                })
                .collect();
        }
        let Some(parent) = ini
            .definitions("Object", &name)
            .into_iter()
            .find(|d| d.kind == "ObjectReskin")
            .and_then(|d| d.header.get(1).cloned())
        else {
            return Vec::new();
        };
        name = parent;
    }
    Vec::new()
}

/// The `AttachToBoneInAnotherModule` translation of each module that has one, resolved like
/// `W3DModelDrawModuleData::getAttachToDrawableBoneOffset` (`W3DModelDraw.cpp:1147-1174`): the bone comes from the
/// first module in `Draw` order whose condition-selected model has it (`Drawable::getPristineBonePositions`,
/// Drawable.cpp:754-780). The engine caches it on the module's first launch query; this resolves it from the empty
/// condition set, i.e. the object's rest/default models.
fn attach_offsets(
    vfs: &Vfs,
    draws: &[LaunchDraw],
    hierarchies: &mut BTreeMap<String, Hierarchy>,
    animations: &mut BTreeMap<String, zr_w3d::Animation>,
) -> BTreeMap<usize, Option<[f32; 3]>> {
    let mut out = BTreeMap::new();
    for d in draws {
        let Some(bone) = &d.attach_bone else { continue };
        let mut offset = None;
        for m in draws {
            let Some(state) = m.states.find_best_state(&ConditionFlags::new()) else { continue };
            let Some(h) = hierarchy(vfs, state, hierarchies) else { continue };
            let Some(idx) = h.bone_index(bone) else { continue };
            offset = Some(state_pose(vfs, h, state, animations)[idx].0);
            break;
        }
        out.insert(d.index, offset);
    }
    out
}

/// One condition state's `LaunchState`: the state's `WeaponLaunchBone` matrices (with the turret art applied) and its
/// turret. Empty when the state declares none of the weapon bones or has no model (it then never answers
/// `getProjectileLaunchOffset`'s barrel lookup).
fn resolve_state(
    vfs: &Vfs,
    state: &ConditionState,
    controlled: u8,
    attach: Option<[f32; 3]>,
    hierarchies: &mut BTreeMap<String, Hierarchy>,
    animations: &mut BTreeMap<String, zr_w3d::Animation>,
) -> LaunchState {
    let mut out = LaunchState { conditions: state.conditions.iter().map(flags_bits).collect(), ..LaunchState::default() };
    // `validateWeaponBarrelInfo` is only run for a state that names one of the four bones; a state without them has no
    // barrels (its `m_weaponBarrelInfoVec` stays empty), so it never answers `getProjectileLaunchOffset`.
    let declares = (0..3).any(|slot| {
        state.weapon_fire_fx_bone[slot].is_some()
            || state.weapon_recoil_bone[slot].is_some()
            || state.weapon_muzzle_flash[slot].is_some()
            || state.weapon_launch_bone[slot].is_some()
    });
    if !declares {
        return out;
    }
    // A state without a `WeaponLaunchBone` name never answers (`m_weaponBarrelInfoVec` would have identity matrices
    // only, which the sim does not model; see `barrel_positions`), so its turret pivots are unused: skip posing it.
    if (0..3).all(|slot| state.weapon_launch_bone[slot].is_none()) {
        return out;
    }
    let Some(h) = hierarchy(vfs, state, hierarchies) else { return out };
    let pose = state_pose(vfs, h, state, animations);
    for slot in 0..3 {
        let mut barrels = barrel_positions(h, &pose, state, slot);
        // `W3DModelDraw::getProjectileLaunchOffset` (W3DModelDraw.cpp:3336-3340): a turret slot's barrel first takes the
        // state's `TurretArtAngle` / `TurretArtPitch` (`Pre_Rotate_Z(angle)`, `Pre_Rotate_Y(-pitch)`), before the
        // turret rotates it.
        if controlled & (1 << slot) != 0 && (state.turret_art_angle != 0.0 || state.turret_art_pitch != 0.0) {
            for b in &mut barrels {
                *b = turret_art(*b, state.turret_art_angle, state.turret_art_pitch);
            }
        }
        out.slots[slot] = barrels;
    }
    // The state's turret pivots (`getProjectileLaunchOffset`:3350-3393). `CACHE_ATTACH_BONE` adds the module's attach
    // offset to both, and the pitch pivot exists even without a `TurretPitch` bone (the engine then rotates about the
    // attach point / model origin).
    let pivot = |name: Option<&str>| -> [f32; 3] {
        let mut p = name.and_then(|n| h.bone_index(n)).map_or([0.0; 3], |i| pose[i].0);
        if controlled != 0 {
            if let Some(a) = attach {
                p[0] += a[0];
                p[1] += a[1];
                p[2] += a[2];
            }
        }
        p
    };
    if controlled != 0 || state.turret.is_some() || state.turret_pitch.is_some() {
        out.turret = Some(TurretLaunch {
            slots: controlled,
            yaw_pivot: pivot(state.turret.as_deref()),
            pitch_pivot: Some(pivot(state.turret_pitch.as_deref())),
        });
    }
    out
}

/// A `zr-w3d` condition set as the sim's `ModelConditionFlags` bits. Names the sim does not model are dropped.
fn flags_bits(f: &ConditionFlags) -> u128 {
    f.names().filter_map(zr_sim::condition::bit_for_name).fold(0u128, |bits, b| bits | (1u128 << b))
}

/// `W3DModelDraw::getProjectileLaunchOffset`'s turret art (W3DModelDraw.cpp:3336-3340): `Pre_Rotate_Z(angle)` then
/// `Pre_Rotate_Y(-pitch)` on the barrel's bone matrix, i.e. `q = Ry(-pitch) * Rz(angle)` (`Pre_Rotate_*` pre-multiplies,
/// so the Y rotation is applied last). The translation and the rest rotation both turn.
fn turret_art(b: LaunchBarrel, angle: f32, pitch: f32) -> LaunchBarrel {
    let qz = zr_w3d::quat_about_axis([0.0, 0.0, 1.0], angle);
    let qy = zr_w3d::quat_about_axis([0.0, 1.0, 0.0], -pitch);
    let q = zr_w3d::quat_mul(&qy, &qz);
    LaunchBarrel { pos: zr_w3d::quat_rotate(&q, b.pos), rot: zr_w3d::quat_mul(&q, &b.rot) }
}

/// The `ControlledWeaponSlots` mask as the sim's per-slot bitmask.
fn weapon_slot_bits(m: zr_templates::Mask<WeaponSlot>) -> u8 {
    WeaponSlot::ALL.iter().fold(0u8, |b, &s| if m.contains(s) { b | (1 << s.index()) } else { b })
}

/// The `WeaponLaunchBone` rest transforms of a model (model space), stopping at the first missing
/// `WeaponLaunchBone01..NN` index, else the unadorned bone (`validateWeaponBarrelInfo`:787-794, :826-830). Each keeps
/// the bone's posed rotation as well as its translation: the launch matrix is the whole bone matrix
/// (`projectileOffsetMtx`).
///
/// Note: the engine's barrel vec (`m_weaponBarrelInfoVec`) also holds an entry for an index whose only bones are the
/// slot's `WeaponRecoilBone` / `WeaponMuzzleFlash` / `WeaponFireFXBone`, with an identity launch matrix (the shot starts
/// at the object's origin). The sim keeps only launch-bone entries: for every shipped slot with a launch bone the launch
/// chain is the longest chain of the four names (survey: 0 exceptions), so the launch position/heading of every barrel
/// index is the same; the 2964 slots with no launch bone are not counted, so their (identity) barrel cycle is not
/// modelled. The unadorned fallback is only reached in C++ when the whole list is empty, which here is the launch chain
/// being empty too (survey: 0 states where another bone would have blocked it).
fn barrel_positions(h: &Hierarchy, pose: &[Trs], state: &ConditionState, slot: usize) -> Vec<LaunchBarrel> {
    let Some(name) = state.weapon_launch_bone[slot].as_deref() else { return Vec::new() };
    let barrel = |idx: usize| LaunchBarrel { pos: pose[idx].0, rot: pose[idx].1 };
    let mut out = Vec::new();
    for i in 1..=MAX_BARRELS {
        match h.bone_index(&format!("{name}{i:02}")) {
            Some(idx) => out.push(barrel(idx)),
            None => break,
        }
    }
    if out.is_empty() {
        if let Some(idx) = h.bone_index(name) {
            out.push(barrel(idx));
        }
    }
    out
}

/// The state model's skeleton: the model's own hierarchy, else the skeleton its HLOD names (`zr_w3d::model_hierarchy`,
/// `HLodDefClass::Load_W3D` / `WW3DAssetManager::Get_HTree`), else a skeleton named by the state's animations (some
/// models keep the skeleton in a separate file). Memoised per model.
fn hierarchy<'a>(vfs: &Vfs, state: &ConditionState, cache: &'a mut BTreeMap<String, Hierarchy>) -> Option<&'a Hierarchy> {
    let model = state.model.as_deref()?;
    if !cache.contains_key(model) {
        let mut loaded = zr_w3d::model_hierarchy(vfs, model);
        if loaded.is_none() {
            for anim in &state.animations {
                if let Ok(a) = zr_w3d::load_animation(vfs, &anim.name) {
                    loaded = zr_w3d::load_hierarchy(vfs, &a.hierarchy).ok();
                    if loaded.is_some() {
                        break;
                    }
                }
            }
        }
        if let Some(h) = loaded {
            cache.insert(model.to_string(), h);
        }
    }
    cache.get(model)
}

/// The bone transforms `ModelConditionInfo::validateCachedBones` caches for the state (W3DModelDraw.cpp:590-713): the
/// model posed by the state's **first animation** at frame 0 (the last frame with `PRISTINE_BONE_POS_IN_FINAL_FRAME`),
/// else the bind pose. The engine plays that animation on a freshly created render object before reading
/// `Get_Bone_Transform`; launch/turret/attach bones therefore follow the animation's first frame, not the bind pose
/// (the GLA Technical's turret skeleton has its turret bone at the origin only at frame 0).
fn state_pose(vfs: &Vfs, h: &Hierarchy, state: &ConditionState, animations: &mut BTreeMap<String, zr_w3d::Animation>) -> Vec<Trs> {
    let Some(first) = state.animations.first() else { return h.bind_pose_trs() };
    if !animations.contains_key(&first.name) {
        if let Ok(a) = zr_w3d::load_animation(vfs, &first.name) {
            animations.insert(first.name.clone(), a);
        }
    }
    let Some(anim) = animations.get(&first.name) else { return h.bind_pose_trs() };
    let final_frame = state.flags.iter().any(|f| f.eq_ignore_ascii_case("PRISTINE_BONE_POS_IN_FINAL_FRAME"));
    let frame = if final_frame { anim.frame_count.saturating_sub(1) as f32 } else { 0.0 };
    h.pose_trs(anim, frame)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Pre_Rotate_Z` then `Pre_Rotate_Y` (W3DModelDraw.cpp:3336-3340) is `q = Ry(-pitch) * Rz(angle)`: the Y rotation
    /// is applied last. Each axis alone turns as before; both together must not commute the other way.
    #[test]
    fn turret_art_turns_the_barrel_position_and_rotation_before_the_turret() {
        // A barrel 10 units out on +X with the art angle at 90 degrees points at +Y (and its rest rotation turns with it).
        let b = LaunchBarrel { pos: [10.0, 0.0, 2.0], rot: [0.0, 0.0, 0.0, 1.0] };
        let t = turret_art(b, std::f32::consts::FRAC_PI_2, 0.0);
        assert!(t.pos[0].abs() < 1e-5 && (t.pos[1] - 10.0).abs() < 1e-5 && (t.pos[2] - 2.0).abs() < 1e-5, "{:?}", t.pos);
        let x = zr_w3d::quat_rotate(&t.rot, [1.0, 0.0, 0.0]);
        assert!(x[1] > 0.999, "the rest rotation turns with the art: {x:?}");
        // A pitch of 90 degrees is `Pre_Rotate_Y(-90)`: (10, 0, 2) -> (-2, 0, 10), so the barrel lifts toward +Z.
        let p = turret_art(b, 0.0, std::f32::consts::FRAC_PI_2);
        assert!((p.pos[0] + 2.0).abs() < 1e-4 && p.pos[1].abs() < 1e-5 && (p.pos[2] - 10.0).abs() < 1e-4, "{:?}", p.pos);
        // Both: Rz(90) first gives (0, 10, 2), then Ry(-90) gives (-2, 10, 0). The other order would be (0, -2, 10).
        let both = turret_art(b, std::f32::consts::FRAC_PI_2, std::f32::consts::FRAC_PI_2);
        assert!((both.pos[0] + 2.0).abs() < 1e-4 && (both.pos[1] - 10.0).abs() < 1e-4 && both.pos[2].abs() < 1e-4, "{:?}", both.pos);
    }
}
