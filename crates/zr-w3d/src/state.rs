//! W3D draw-module condition states.
//!
//! Parses the `ConditionState` / `DefaultConditionState` / `AliasConditionState`
//! / `TransitionState` entries of a `W3D*Draw` module into a table and ports the
//! best-match rule. Sources: `W3DModelDraw.cpp` (`parseConditionState`,
//! `W3DModelDrawModuleData::findBestInfo`), `SparseMatchFinder.h`
//! (`findBestInfoSlow`, `countConditionIntersection` / `countInverseIntersection`)
//! and `BitFlagsIO.h` (the `NONE` / `+` / `-` condition list syntax).

use std::collections::BTreeSet;
use zr_ini::{Block, Field, IniDb, Item};

/// A set of `ModelConditionFlags` bit names, kept as uppercase names in a
/// sorted set (matching the engine's fixed bit universe exactly).
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct ConditionFlags(BTreeSet<String>);

impl ConditionFlags {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parses an engine condition list: whitespace-separated names, `NONE`
    /// clears, `+NAME` adds and `-NAME` removes (case-insensitive).
    pub fn parse(s: &str) -> Self {
        let mut set = BTreeSet::new();
        let mut normal = false;
        for tok in s.split([' ', '\t', '=', '\r', '\n']).filter(|t| !t.is_empty()) {
            if tok.eq_ignore_ascii_case("NONE") {
                set.clear();
                normal = true;
                continue;
            }
            if let Some(rest) = tok.strip_prefix('+') {
                set.insert(rest.to_ascii_uppercase());
            } else if let Some(rest) = tok.strip_prefix('-') {
                set.remove(&rest.to_ascii_uppercase());
            } else {
                if !normal {
                    set.clear();
                    normal = true;
                }
                set.insert(tok.to_ascii_uppercase());
            }
        }
        ConditionFlags(set)
    }

    pub fn from_names<'a>(names: impl IntoIterator<Item = &'a str>) -> Self {
        ConditionFlags(names.into_iter().map(str::to_ascii_uppercase).collect())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn contains(&self, name: &str) -> bool {
        self.0.contains(&name.to_ascii_uppercase())
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(String::as_str)
    }

    /// Number of flags present in both sets (`countConditionIntersection`).
    fn intersection(&self, other: &ConditionFlags) -> usize {
        self.0.intersection(&other.0).count()
    }

    /// Number of flags present here but not in `query` (`countInverseIntersection`,
    /// the "irrelevant yes bits" the engine minimises).
    fn extras_over(&self, query: &ConditionFlags) -> usize {
        self.0.difference(&query.0).count()
    }

    fn without(&self, ignore: &ConditionFlags) -> ConditionFlags {
        ConditionFlags(self.0.difference(&ignore.0).cloned().collect())
    }
}

/// `RenderObjClass::AnimMode` (`rendobj.h`, `TheAnimModeNames`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AnimMode {
    Manual,
    Loop,
    #[default]
    Once,
    LoopPingPong,
    LoopBackwards,
    OnceBackwards,
}

impl AnimMode {
    fn parse(s: &str) -> Option<Self> {
        Some(match s.to_ascii_uppercase().as_str() {
            "MANUAL" => AnimMode::Manual,
            "LOOP" => AnimMode::Loop,
            "ONCE" => AnimMode::Once,
            "LOOP_PINGPONG" => AnimMode::LoopPingPong,
            "LOOP_BACKWARDS" => AnimMode::LoopBackwards,
            "ONCE_BACKWARDS" => AnimMode::OnceBackwards,
            _ => return None,
        })
    }
}

/// One `Animation` / `IdleAnimation` reference of a condition state.
#[derive(Debug, Clone, PartialEq)]
pub struct AnimationRef {
    pub name: String,
    pub idle: bool,
    pub distance_covered: f32,
    pub times_to_repeat: u32,
}

/// `WEAPONSLOT` index of a `Weapon*Bone` field (`TheWeaponSlotTypeNames`).
fn weapon_slot(name: &str) -> Option<usize> {
    Some(match name.to_ascii_uppercase().as_str() {
        "PRIMARY" => 0,
        "SECONDARY" => 1,
        "TERTIARY" => 2,
        _ => return None,
    })
}

/// A `ConditionState` block (also used for the default and transition states).
#[derive(Debug, Clone, Default)]
pub struct ConditionState {
    /// Condition sets that select this state; extra entries are aliases.
    pub conditions: Vec<ConditionFlags>,
    pub model: Option<String>,
    pub animations: Vec<AnimationRef>,
    pub animation_mode: AnimMode,
    /// `(sub-object, hide)` pairs, in first-mention order.
    pub hide_show: Vec<(String, bool)>,
    pub flags: Vec<String>,
    pub transition_key: Option<String>,
    pub allow_to_finish: Option<String>,
    /// `Turret`: the bone the turret yaw is applied to.
    pub turret: Option<String>,
    /// `TurretPitch`: the bone the turret pitch is applied to.
    pub turret_pitch: Option<String>,
    /// `TurretArtAngle` / `TurretArtPitch` (radians; `INI::parseAngleReal`): added to the AI's turret
    /// angle and pitch before they are applied (`W3DModelDraw::handleClientTurretPositioning`).
    pub turret_art_angle: f32,
    pub turret_art_pitch: f32,
    /// `WeaponFireFXBone` per weapon slot: where the muzzle `FireFX` is played.
    pub weapon_fire_fx_bone: [Option<String>; 3],
    /// `WeaponMuzzleFlash` per slot: the hidden sub-object shown while firing.
    pub weapon_muzzle_flash: [Option<String>; 3],
    /// `WeaponLaunchBone` per slot: where the projectile is launched.
    pub weapon_launch_bone: [Option<String>; 3],
    /// `WeaponRecoilBone` per slot.
    pub weapon_recoil_bone: [Option<String>; 3],
    /// `ParticleSysBone = BONE ParticleSystem` entries, in file order.
    pub particle_sys_bones: Vec<(String, String)>,
}

/// A `TransitionState = FROM TO` block (kept out of the condition-state table).
#[derive(Debug, Clone)]
pub struct TransitionState {
    pub from: String,
    pub to: String,
    pub state: ConditionState,
}

/// Parsed condition-state table of one `W3D*Draw` module.
#[derive(Debug, Clone)]
pub struct DrawStates {
    pub module: String,
    pub states: Vec<ConditionState>,
    pub default_state: Option<usize>,
    pub ignore: ConditionFlags,
    pub transitions: Vec<TransitionState>,
}

impl DrawStates {
    /// Selects the best condition state for `flags`, porting `findBestInfoSlow`:
    /// maximise matched "yes" bits, then minimise the candidate's bits that the
    /// query does not have. Ignored condition bits are cleared from the query
    /// first (`W3DModelDrawModuleData::findBestInfo`).
    pub fn find_best_state(&self, flags: &ConditionFlags) -> Option<&ConditionState> {
        self.best_state_index(flags).map(|i| &self.states[i])
    }

    /// Index into [`Self::states`] of the state [`Self::find_best_state`] selects.
    pub fn best_state_index(&self, flags: &ConditionFlags) -> Option<usize> {
        let bits = flags.without(&self.ignore);
        let mut best: Option<(usize, i32, i32)> = None;
        for (idx, st) in self.states.iter().enumerate() {
            for yes in st.conditions.iter().rev() {
                let yes_match = yes.intersection(&bits) as i32;
                let extras = yes.extras_over(&bits) as i32;
                let replace = match best {
                    None => true,
                    Some((_, bm, be)) => yes_match > bm || (yes_match == bm && extras < be),
                };
                if replace {
                    best = Some((idx, yes_match, extras));
                }
            }
        }
        best.map(|(idx, _, _)| idx)
    }

    /// The `DefaultConditionState`, if one was declared.
    pub fn default_state(&self) -> Option<&ConditionState> {
        self.default_state.and_then(|i| self.states.get(i))
    }
}

fn is_w3d_draw(draw: &Block) -> bool {
    let module = draw.header.first().map(String::as_str).unwrap_or("");
    module.starts_with("W3D") && module.ends_with("Draw")
}

fn apply_fields<'a>(state: &mut ConditionState, fields: impl Iterator<Item = &'a Field>, inherited: bool) {
    let mut copied_from_default = inherited;
    for f in fields {
        match f.key.as_str() {
            "Model" => {
                let v = f.tokens().first().copied().unwrap_or("");
                state.model = (!v.is_empty() && !v.eq_ignore_ascii_case("none")).then(|| v.to_string());
            }
            "Animation" | "IdleAnimation" => {
                if copied_from_default {
                    state.animations.clear();
                    copied_from_default = false;
                }
                let toks = f.tokens();
                let Some(name) = toks.first() else { continue };
                if name.is_empty() || name.eq_ignore_ascii_case("none") {
                    continue;
                }
                let distance = toks.get(1).and_then(|t| t.parse::<f32>().ok()).unwrap_or(0.0);
                let repeat = toks.get(2).and_then(|t| t.parse::<u32>().ok()).unwrap_or(1).max(1);
                for _ in 0..repeat {
                    state.animations.push(AnimationRef {
                        name: (*name).to_string(),
                        idle: f.key == "IdleAnimation",
                        distance_covered: distance,
                        times_to_repeat: repeat,
                    });
                }
            }
            "AnimationMode" => {
                if let Some(m) = f.tokens().first().and_then(|t| AnimMode::parse(t)) {
                    state.animation_mode = m;
                }
            }
            "ShowSubObject" | "HideSubObject" => {
                let hide = f.key == "HideSubObject";
                for name in f.tokens() {
                    if name.eq_ignore_ascii_case("none") {
                        state.hide_show.clear();
                        break;
                    }
                    let name = name.to_string();
                    match state.hide_show.iter_mut().find(|(n, _)| n.eq_ignore_ascii_case(&name)) {
                        Some(entry) => entry.1 = hide,
                        None => state.hide_show.push((name, hide)),
                    }
                }
            }
            "Flags" => {
                for name in f.tokens() {
                    state.flags.push(name.to_ascii_uppercase());
                }
            }
            "TransitionKey" => {
                state.transition_key = f.tokens().first().map(|t| t.to_ascii_lowercase());
            }
            "WaitForStateToFinishIfPossible" => {
                state.allow_to_finish = f.tokens().first().map(|t| t.to_ascii_lowercase());
            }
            "Turret" | "TurretPitch" => {
                let v = f.tokens().first().copied().unwrap_or("");
                let bone = (!v.is_empty() && !v.eq_ignore_ascii_case("none")).then(|| v.to_string());
                if f.key == "Turret" {
                    state.turret = bone;
                } else {
                    state.turret_pitch = bone;
                }
            }
            "TurretArtAngle" | "TurretArtPitch" => {
                // `INI::parseAngleReal`: degrees in the file, radians in the engine.
                let deg = f.tokens().first().and_then(|t| t.parse::<f32>().ok()).unwrap_or(0.0);
                if f.key == "TurretArtAngle" {
                    state.turret_art_angle = deg.to_radians();
                } else {
                    state.turret_art_pitch = deg.to_radians();
                }
            }
            "WeaponFireFXBone" | "WeaponMuzzleFlash" | "WeaponLaunchBone" | "WeaponRecoilBone" => {
                let toks = f.tokens();
                let Some(slot) = toks.first().and_then(|s| weapon_slot(s)) else { continue };
                let v = toks.get(1).copied().unwrap_or("");
                let bone = (!v.is_empty() && !v.eq_ignore_ascii_case("none")).then(|| v.to_string());
                let arr = match f.key.as_str() {
                    "WeaponFireFXBone" => &mut state.weapon_fire_fx_bone,
                    "WeaponMuzzleFlash" => &mut state.weapon_muzzle_flash,
                    "WeaponLaunchBone" => &mut state.weapon_launch_bone,
                    _ => &mut state.weapon_recoil_bone,
                };
                arr[slot] = bone;
            }
            "ParticleSysBone" => {
                let toks = f.tokens();
                if let (Some(bone), Some(sys)) = (toks.first(), toks.get(1)) {
                    state.particle_sys_bones.push(((*bone).to_string(), (*sys).to_string()));
                }
            }
            _ => {}
        }
    }
}

fn parse_draw(draw: &Block) -> DrawStates {
    let mut states: Vec<ConditionState> = Vec::new();
    let mut default_state = None;
    let mut ignore = ConditionFlags::new();
    let mut transitions = Vec::new();
    for item in &draw.items {
        match item {
            Item::Field(f) => match f.key.as_str() {
                "IgnoreConditionStates" => ignore = ConditionFlags::parse(&f.value),
                "AliasConditionState" => {
                    if let Some(last) = states.last_mut() {
                        last.conditions.push(ConditionFlags::parse(&f.value));
                    }
                }
                _ => {}
            },
            Item::Block(b) => match b.kind.as_str() {
                "DefaultConditionState" => {
                    if default_state.is_none() {
                        default_state = Some(states.len());
                        let mut st = ConditionState::default();
                        st.conditions.push(ConditionFlags::new());
                        apply_fields(&mut st, b.fields(), false);
                        states.push(st);
                    }
                }
                "ConditionState" => {
                    let inherited = default_state.is_some();
                    let mut st = default_state
                        .and_then(|d| states.get(d))
                        .cloned()
                        .unwrap_or_default();
                    st.conditions = vec![ConditionFlags::from_names(b.header.iter().map(String::as_str))];
                    apply_fields(&mut st, b.fields(), inherited);
                    states.push(st);
                }
                "TransitionState" => {
                    let inherited = default_state.is_some();
                    let mut st = default_state
                        .and_then(|d| states.get(d))
                        .cloned()
                        .unwrap_or_default();
                    st.conditions = vec![ConditionFlags::new()];
                    apply_fields(&mut st, b.fields(), inherited);
                    let from = b.header.first().map(|s| s.to_ascii_lowercase()).unwrap_or_default();
                    let to = b.header.get(1).map(|s| s.to_ascii_lowercase()).unwrap_or_default();
                    transitions.push(TransitionState { from, to, state: st });
                }
                _ => {}
            },
        }
    }
    DrawStates {
        module: draw.header.first().cloned().unwrap_or_default(),
        states,
        default_state,
        ignore,
        transitions,
    }
}

/// Parses the condition states of `template`'s first `W3D*Draw` module,
/// following `ObjectReskin` to its parent when needed.
pub fn states_for_template(ini: &IniDb, template: &str) -> Option<DrawStates> {
    let mut name = template.to_string();
    for _ in 0..16 {
        let obj = ini.resolved("Object", &name)?;
        for draw in obj.children_named("Draw").filter(|d| is_w3d_draw(d)) {
            let states = parse_draw(draw);
            if states.default_state.is_some() || !states.states.is_empty() {
                return Some(states);
            }
        }
        let parent = ini
            .definitions("Object", &name)
            .into_iter()
            .find(|d| d.kind == "ObjectReskin")
            .and_then(|d| d.header.get(1).cloned())?;
        name = parent;
    }
    None
}

/// Every `W3D*Draw` module of `template` that has condition states, in `Draw` order. The first entry is
/// the module [`states_for_template`] returns; the others are the object's further draw modules (the
/// construction scaffold, fence and crane of a structure, `ModuleTag_02`..). The index is the module's
/// position among the object's `Draw` blocks (0-based, counting every `Draw` block).
pub fn draw_states_by_module(ini: &IniDb, template: &str) -> Vec<(usize, DrawStates)> {
    let mut name = template.to_string();
    for _ in 0..16 {
        let Some(obj) = ini.resolved("Object", &name) else { return Vec::new() };
        let out: Vec<(usize, DrawStates)> = obj
            .children_named("Draw")
            .enumerate()
            .filter(|(_, d)| is_w3d_draw(d))
            .map(|(i, d)| (i, parse_draw(d)))
            .filter(|(_, s)| s.default_state.is_some() || !s.states.is_empty())
            .collect();
        if !out.is_empty() {
            return out;
        }
        let Some(parent) = ini.definitions("Object", &name).into_iter().find(|d| d.kind == "ObjectReskin").and_then(|d| d.header.get(1).cloned()) else {
            return Vec::new();
        };
        name = parent;
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db(text: &str) -> IniDb {
        let mut db = IniDb::new();
        db.extend(zr_ini::parse("test.ini", text).unwrap());
        db
    }

    const TANK: &str = "Object Tank\n  Draw = W3DModelDraw ModuleTag_01\n    IgnoreConditionStates = PREATTACK_A FIRING_A\n    DefaultConditionState\n      Model = TankA\n      IdleAnimation = Tank_SKL.Tank_IDA\n      AnimationMode = ONCE\n      ShowSubObject = Turret\n    End\n    ConditionState = DAMAGED\n      Model = TankD\n      HideSubObject = Turret\n      Animation = Tank_SKL.Tank_MVA\n      AnimationMode = LOOP\n    End\n    AliasConditionState = DAMAGED WEAPONSET_VETERAN\n    ConditionState = WEAPONSET_VETERAN\n      Animation = Tank_SKL.Tank_VET\n    End\n    TransitionState = TRANS_Stand TRANS_Move\n      Animation = Tank_SKL.Tank_TR\n      AnimationMode = ONCE\n    End\n  End\nEnd\nObjectReskin TankSkin Tank\n  Side = X\nEnd\n";

    #[test]
    fn parses_states_and_default() {
        let states = states_for_template(&db(TANK), "Tank").unwrap();
        assert_eq!(states.module, "W3DModelDraw");
        assert_eq!(states.states.len(), 3);
        assert_eq!(states.default_state().unwrap().model.as_deref(), Some("TankA"));
        assert_eq!(states.default_state().unwrap().animations[0].name, "Tank_SKL.Tank_IDA");
        assert!(states.default_state().unwrap().animations[0].idle);
        assert_eq!(states.transitions.len(), 1);
        assert_eq!(states.transitions[0].from, "trans_stand");
        // DAMAGED inherits the IdleAnimation then replaces it with its own Animation.
        let dmg = &states.states[1];
        assert_eq!(dmg.animations.len(), 1);
        assert_eq!(dmg.animations[0].name, "Tank_SKL.Tank_MVA");
        assert_eq!(dmg.animation_mode, AnimMode::Loop);
        assert_eq!(dmg.hide_show, vec![("Turret".to_string(), true)]);
    }

    #[test]
    fn draw_states_by_module_keeps_every_w3d_draw_in_order() {
        let ini = db("Object O\n  Draw = W3DModelDraw ModuleTag_01\n    DefaultConditionState\n      Model = Main\n    End\n  End\n  Draw = W3DTankDraw ModuleTag_02\n  End\n  Draw = W3DModelDraw ModuleTag_03\n    DefaultConditionState\n      Model = None\n    End\n    ConditionState = PARTIALLY_CONSTRUCTED\n      Model = Scaffold\n    End\n  End\nEnd\n");
        let mods = draw_states_by_module(&ini, "O");
        // The `Draw` index counts every Draw block; the first entry is the main module.
        assert_eq!(mods.iter().map(|(i, _)| *i).collect::<Vec<_>>(), vec![0, 2]);
        assert_eq!(mods[0].1.module, "W3DModelDraw");
        let flags = ConditionFlags::from_names(["PARTIALLY_CONSTRUCTED"]);
        assert_eq!(mods[1].1.find_best_state(&flags).unwrap().model.as_deref(), Some("Scaffold"));
        assert_eq!(mods[1].1.find_best_state(&ConditionFlags::new()).unwrap().model, None);
    }

    #[test]
    fn find_best_state_uses_default_and_flags() {
        let states = states_for_template(&db(TANK), "Tank").unwrap();
        assert_eq!(states.find_best_state(&ConditionFlags::new()).unwrap().model.as_deref(), Some("TankA"));
        assert_eq!(
            states.find_best_state(&ConditionFlags::from_names(["DAMAGED"])).unwrap().model.as_deref(),
            Some("TankD")
        );
        // Ignored bits don't influence the match.
        assert_eq!(
            states.find_best_state(&ConditionFlags::from_names(["FIRING_A"])).unwrap().model.as_deref(),
            Some("TankA")
        );
        // Alias condition set selects the same state.
        let alias = states.find_best_state(&ConditionFlags::from_names(["DAMAGED", "WEAPONSET_VETERAN"]));
        assert!(alias.unwrap().animations[0].name == "Tank_SKL.Tank_MVA");
    }

    #[test]
    fn prefer_fewer_extraneous_yes_bits() {
        let text = "Object T\n  Draw = W3DModelDraw M\n    DefaultConditionState\n      Model = Base\n    End\n    ConditionState = FRONTCRUSHED\n      Model = Front\n    End\n    ConditionState = FRONTCRUSHED BACKCRUSHED\n      Model = Both\n    End\n  End\nEnd\n";
        let states = states_for_template(&db(text), "T").unwrap();
        let q = ConditionFlags::from_names(["FRONTCRUSHED"]);
        assert_eq!(states.find_best_state(&q).unwrap().model.as_deref(), Some("Front"));
    }

    #[test]
    fn reskin_follows_parent() {
        let states = states_for_template(&db(TANK), "TankSkin").unwrap();
        assert_eq!(states.default_state().unwrap().model.as_deref(), Some("TankA"));
    }

    #[test]
    fn parses_turret_weapon_bones_and_particle_bones() {
        let text = "Object Tank\n  Draw = W3DTankDraw M\n    DefaultConditionState\n      Model = AVLeopard\n      Turret = Turret\n      TurretPitch = Barrel\n      WeaponFireFXBone = PRIMARY TurretMS\n      WeaponMuzzleFlash = PRIMARY TurretFX\n      WeaponLaunchBone = PRIMARY TurretMS\n      ParticleSysBone = Smoke01 TankSmoke\n      ParticleSysBone = None InfantryDustTrails\n    End\n  End\nEnd\n";
        let states = states_for_template(&db(text), "Tank").unwrap();
        let st = states.default_state().unwrap();
        assert_eq!(st.turret.as_deref(), Some("Turret"));
        assert_eq!(st.turret_pitch.as_deref(), Some("Barrel"));
        assert_eq!(st.weapon_fire_fx_bone[0].as_deref(), Some("TurretMS"));
        assert_eq!(st.weapon_muzzle_flash[0].as_deref(), Some("TurretFX"));
        assert_eq!(st.weapon_launch_bone[0].as_deref(), Some("TurretMS"));
        assert_eq!(st.particle_sys_bones, vec![("Smoke01".to_string(), "TankSmoke".to_string()), ("None".to_string(), "InfantryDustTrails".to_string())]);
    }
}
