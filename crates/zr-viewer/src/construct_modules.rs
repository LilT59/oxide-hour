//! Shared draw-module machinery: the condition-state transition/wait rules, animation start-frame/completion helpers,
//! the construction lift, the posed-model spawner used by the rally marker and the shell map, and their asset cache.
//!
//! The extra `W3DModelDraw` modules of every object (construction scaffold/fence/crane, upgrade parts, attached
//! turrets, supply boxes) are drawn by `weapon_modules`, which uses these helpers; this module no longer runs a
//! second extra-module draw pass. The engine's own model: each drawable owns one draw module per `Draw` block, and each
//! `W3DModelDraw` selects its own condition state (`W3DModelDraw::doStartOrSwitchState`,
//! `W3DModelDrawModuleData::findBestInfo`). A change between two states with `TransitionKey`s plays the transition
//! state first, and `WaitForStateToFinishIfPossible` holds a running `ONCE` animation until it ends. The rally marker
//! (`SCMRally`) and shell-map props are spawned as posed rigs here.

use std::collections::HashMap;
use std::sync::Arc;

use bevy::mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes};
use bevy::prelude::*;
use zr_assets::Vfs;
use zr_fx::ClientRng;
use zr_ini::IniDb;
use zr_sim::template::Geometry;
use zr_w3d::{AnimMode, Animation, ConditionFlags, ConditionState, DrawStates, Hierarchy};

use crate::anim::{self, AnimCache};
use crate::construction_view;
use crate::convert;
use crate::source::{self, ModelPart};
use crate::team_color;

/// One mesh part's render handles (`convert::model_part`).
type PartHandles = Vec<(Handle<Mesh>, Handle<StandardMaterial>)>;

/// Parsed draw-module tables and the models, skeletons and meshes the extra modules draw. Keyed by name.
#[derive(Resource, Default)]
pub(crate) struct ExtraCache {
    parts: HashMap<String, Option<Arc<Vec<ModelPart>>>>,
    skeleton: HashMap<String, Arc<Hierarchy>>,
    bind: HashMap<String, Handle<SkinnedMeshInverseBindposes>>,
    meshes: HashMap<(String, [u8; 3]), PartHandles>,
}

/// The asset stores a posed model's meshes are added to.
pub(crate) struct Gfx<'a> {
    pub meshes: &'a mut Assets<Mesh>,
    pub materials: &'a mut Assets<StandardMaterial>,
    pub images: &'a mut Assets<Image>,
    pub inv_bind: &'a mut Assets<SkinnedMeshInverseBindposes>,
}

/// One drawn model: its W3D model and clip, the mode it plays, and where it stands.
pub(crate) struct PosedSpec<'a> {
    pub model: &'a str,
    /// The state's chosen animation (`HIER.ANIM`), if any.
    pub clip: Option<&'a str>,
    pub mode: AnimMode,
    /// `START_FRAME_LAST`: a state with no explicit start frame starts at its last frame.
    pub start_last: bool,
    /// House colour of the owning player (applied to `HOUSECOLOR` meshes).
    pub rgb: [u8; 3],
    pub hide_show: &'a [(String, bool)],
    /// The model root's transform, relative to its parent.
    pub transform: Transform,
}

/// What an extra draw module shows: one of its condition states, or a `TransitionState` on the way to one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shown {
    State(usize),
    /// `transition` indexes `DrawStates::transitions`; `to` is the state the transition ends in.
    Transition { transition: usize, to: usize },
}

/// A posed model that animates: its skeleton and joint entities. A `LOOP` clip wraps; a `ONCE` or
/// `ONCE_BACKWARDS` clip runs from `start` to its end frame and holds there.
#[derive(Component)]
pub(crate) struct PosedRig {
    hierarchy: Arc<Hierarchy>,
    clip: Option<Arc<Animation>>,
    joints: Vec<Entity>,
    time: f32,
    mode: AnimMode,
    start: f32,
}

pub fn add_systems(app: &mut App) {
    app.init_resource::<ExtraCache>();
    // The extra `W3DModelDraw` modules themselves are drawn by `weapon_modules` (now all of them, not only the
    // weapon-bearing ones); this keeps the rally marker/puck and shell-map posed rigs animating.
    app.add_systems(Update, animate_posed.run_if(resource_exists::<crate::sim_view::SimActive>));
}

impl ExtraCache {
    pub(crate) fn parts(&mut self, vfs: &Vfs, model: &str) -> Option<Arc<Vec<ModelPart>>> {
        self.parts.entry(model.to_owned()).or_insert_with(|| source::model_by_name(vfs, model).map(Arc::new)).clone()
    }

    pub(crate) fn skeleton(&mut self, vfs: &Vfs, model: &str) -> Arc<Hierarchy> {
        self.skeleton
            .entry(model.to_owned())
            .or_insert_with(|| Arc::new(zr_w3d::model_hierarchy(vfs, model).unwrap_or_else(Hierarchy::default_tree)))
            .clone()
    }

    pub(crate) fn bind(&mut self, inv_bind: &mut Assets<SkinnedMeshInverseBindposes>, model: &str, h: &Hierarchy) -> Handle<SkinnedMeshInverseBindposes> {
        self.bind
            .entry(model.to_owned())
            .or_insert_with(|| inv_bind.add(SkinnedMeshInverseBindposes::from(anim::inv_bind_mats(h))))
            .clone()
    }

    pub(crate) fn handles(&mut self, gfx: &mut Gfx, model: &str, rgb: [u8; 3], parts: &[ModelPart], pivots: usize) -> PartHandles {
        self.meshes
            .entry((model.to_owned(), rgb))
            .or_insert_with(|| {
                parts
                    .iter()
                    .map(|p| {
                        let tinted;
                        let p = if team_color::uses_house_color(p) {
                            tinted = team_color::tint(p, rgb);
                            &tinted
                        } else {
                            p
                        };
                        let (m, mat) = convert::model_part(p, gfx.images, Some(pivots));
                        (gfx.meshes.add(m), gfx.materials.add(mat))
                    })
                    .collect()
            })
            .clone()
    }
}

/// The model, clip and mode of a template's default condition state, for an effect drawn without a sim
/// object (the rally point marker's `DefaultConditionState`).
pub(crate) fn default_state_spec(anim_cache: &mut AnimCache, ini: &IniDb, template: &str) -> Option<(String, Option<String>, AnimMode, bool)> {
    let states = anim_cache.states(ini, template)?;
    let st = states.find_best_state(&ConditionFlags::new())?;
    Some((st.model.clone()?, st.animations.first().map(|a| a.name.clone()), st.animation_mode, st.flags.iter().any(|f| f == "START_FRAME_LAST")))
}

/// The start frame of a state's animation (`W3DModelDraw::adjustAnimation`): `RANDOMIZE_START_FRAME` picks one at
/// random (`GameClientRandomValue`), else `START_FRAME_FIRST` / `START_FRAME_LAST` by the flags, else a backwards
/// mode starts at its last frame and the others at their first.
pub(crate) fn start_frame(mode: AnimMode, flags: &[String], total: u32, rng: &mut ClientRng) -> f32 {
    let last = total.max(1) - 1;
    let has = |name: &str| flags.iter().any(|f| f == name);
    if has("RANDOMIZE_START_FRAME") {
        rng.int(0, last as i32) as f32
    } else if has("START_FRAME_FIRST") {
        0.0
    } else if has("START_FRAME_LAST") || matches!(mode, AnimMode::OnceBackwards | AnimMode::LoopBackwards) {
        last as f32
    } else {
        0.0
    }
}

/// Seconds a clip runs from `start` before `Is_Animation_Complete` (`animobj.cpp`: a `ONCE` completes at its last
/// frame, a `ONCE_BACKWARDS` at its first). Other modes never complete, so the result is infinite.
pub(crate) fn completion_secs(mode: AnimMode, start: f32, total: u32, rate: u32) -> f32 {
    let rate = rate.max(1) as f32;
    let last = total.saturating_sub(1) as f32;
    match mode {
        AnimMode::Once => (last - start).max(0.0) / rate,
        AnimMode::OnceBackwards => start.max(0.0) / rate,
        _ => f32::INFINITY,
    }
}

/// The frame of a rig `t` seconds after it started: `LOOP` wraps, `ONCE` runs forward from `start` and
/// holds its last frame, `ONCE_BACKWARDS` runs back from `start` and holds the first.
pub(crate) fn rig_frame(mode: AnimMode, start: f32, t: f32, rate: u32, total: u32) -> f32 {
    let rate = rate.max(1) as f32;
    let total = total.max(1) as f32;
    let d = t * rate;
    match mode {
        AnimMode::Once => (start + d).min(total - 1.0),
        AnimMode::OnceBackwards => (start - d).max(0.0),
        _ => d % total,
    }
}

/// The animation index a state picks among `n` clips (`W3DModelDraw::adjustAnimation`): with several, a random
/// one; with `avoid` set (the same state again, after its idle clip completed), one other than the current.
pub(crate) fn choose_anim(n: usize, avoid: Option<usize>, rng: &mut ClientRng) -> usize {
    if n <= 1 {
        return 0;
    }
    match avoid {
        Some(a) => loop {
            let r = rng.int(0, n as i32 - 1) as usize;
            if r != a {
                return r;
            }
        },
        None => rng.int(0, n as i32 - 1) as usize,
    }
}

/// The construction lift of a module whose state flags ask for it (`W3DModelDraw::adjustTransformMtx`, per state
/// and per frame). Zero for every other state and once the object is complete.
pub(crate) fn module_lift(adjusts: bool, geometry: &Geometry, percent: f32, under_construction: bool) -> f32 {
    if adjusts {
        construction_view::lift_at_percent(geometry, percent, under_construction)
    } else {
        0.0
    }
}

/// Spawns a posed model under `parent`: the skeleton's joints and the model's meshes, bound to the
/// joints, at the clip's pose. A clip that is not `MANUAL` also gets a [`PosedRig`] that keeps it moving.
pub(crate) fn spawn_posed(
    commands: &mut Commands,
    parent: Entity,
    cx: &mut ExtraCache,
    anim_cache: &mut AnimCache,
    vfs: &Vfs,
    gfx: &mut Gfx,
    spec: &PosedSpec,
) -> Option<Entity> {
    spawn_clip(commands, parent, cx, anim_cache, vfs, gfx, spec, None)
}

#[allow(clippy::too_many_arguments)]
fn spawn_clip(
    commands: &mut Commands,
    parent: Entity,
    cx: &mut ExtraCache,
    anim_cache: &mut AnimCache,
    vfs: &Vfs,
    gfx: &mut Gfx,
    spec: &PosedSpec,
    start: Option<f32>,
) -> Option<Entity> {
    let parts = cx.parts(vfs, spec.model)?;
    let hierarchy = cx.skeleton(vfs, spec.model);
    let pivots = hierarchy.pivot_count();
    let bind = cx.bind(gfx.inv_bind, spec.model, &hierarchy);
    let handles = cx.handles(gfx, spec.model, spec.rgb, &parts, pivots);
    let clip = spec.clip.and_then(|n| anim_cache.clip(vfs, n));
    // Without an explicit start, a state starts at its last frame under `START_FRAME_LAST`, else its first.
    let start = start.unwrap_or_else(|| match clip.as_deref() {
        Some(c) if spec.start_last => c.frame_count.saturating_sub(1) as f32,
        _ => 0.0,
    });
    let animated = clip.is_some() && spec.mode != AnimMode::Manual;
    let frame = if clip.is_some() { start } else { 0.0 };
    let root = commands.spawn((spec.transform, Visibility::default())).id();
    commands.entity(parent).add_child(root);
    let poses = hierarchy.pose_trs_controlled(clip.as_deref(), frame, &[]);
    let mut joints = Vec::with_capacity(poses.len());
    commands.entity(root).with_children(|c| {
        for &(t, q) in &poses {
            joints.push(c.spawn((anim::bevy_trs(t, q), Visibility::default())).id());
        }
    });
    commands.entity(root).with_children(|c| {
        for (p, (m, mat)) in parts.iter().zip(&handles) {
            let mut e = c.spawn((Mesh3d(m.clone()), MeshMaterial3d(mat.clone()), Transform::IDENTITY, Visibility::default()));
            if anim::part_bound(p, pivots) {
                e.insert(SkinnedMesh { joints: joints.clone(), inverse_bindposes: bind.clone() });
            }
            if let Some(hide) = hidden(&p.name, spec.hide_show) {
                e.insert(if hide { Visibility::Hidden } else { Visibility::Inherited });
            }
        }
    });
    if animated {
        commands.entity(root).insert(PosedRig { hierarchy, clip, joints, time: 0.0, mode: spec.mode, start });
    }
    Some(root)
}

/// Whether `hide_show` hides a mesh part (its base name, before any `#` suffix).
fn hidden(name: &str, hide_show: &[(String, bool)]) -> Option<bool> {
    let base = name.split('#').next().unwrap_or(name);
    hide_show.iter().find(|(n, _)| n.eq_ignore_ascii_case(base)).map(|(_, h)| *h)
}

/// The transition `TransitionState from -> to` between two states' `TransitionKey`s
/// (`W3DModelDraw::findTransitionForSig`), as an index into `DrawStates::transitions`.
fn find_transition(states: &DrawStates, from: usize, to: usize) -> Option<usize> {
    let from_key = states.states[from].transition_key.as_deref()?;
    let to_key = states.states[to].transition_key.as_deref()?;
    states.transitions.iter().position(|t| t.from == from_key && t.to == to_key)
}

/// `W3DModelDraw::setModelState` for a request of state `want` while showing `cur` with `pending` set:
/// returns what the module shows and its pending state. `complete` says whether the shown animation has ended.
/// - a request for what is shown (or for the transition's own target) changes nothing;
/// - a running state whose clip is not complete keeps playing when the new state's `WaitForStateToFinishIfPossible`
///   names that state's `TransitionKey`, and the request waits as pending;
/// - a change between two states with transition keys plays their transition;
/// - anything else (including a request during a transition) shows `want` at once and drops the pending state.
pub(crate) fn request(states: &DrawStates, cur: Option<Shown>, pending: Option<usize>, want: usize, complete: bool) -> (Shown, Option<usize>) {
    if let Some(s @ Shown::Transition { to, .. }) = cur {
        if to == want {
            return (s, None);
        }
    }
    if let Some(Shown::State(c)) = cur {
        if c == want && pending.is_none() {
            return (Shown::State(c), None);
        }
        if pending == Some(want) {
            return (Shown::State(c), pending);
        }
        if c != want && !complete {
            let wait = states.states[want].allow_to_finish.as_deref();
            if wait.is_some() && wait == states.states[c].transition_key.as_deref() {
                return (Shown::State(c), Some(want));
            }
        }
        if c != want {
            if let Some(t) = find_transition(states, c, want) {
                return (Shown::Transition { transition: t, to: want }, None);
            }
        }
    }
    (Shown::State(want), None)
}

/// The condition state a shown value draws (for its flags and hide/show list).
pub(crate) fn shown_state(states: &DrawStates, shown: Shown) -> &ConditionState {
    match shown {
        Shown::State(k) => &states.states[k],
        Shown::Transition { transition, .. } => &states.transitions[transition].state,
    }
}

/// Advances the animated rigs (`LOOP` clips wrap; a `ONCE` clip runs to its end frame and holds) and writes their
/// joint poses.
fn animate_posed(time: Res<Time>, mut rigs: Query<&mut PosedRig>, mut joints: Query<&mut Transform>) {
    let dt = time.delta_secs();
    for mut rig in &mut rigs {
        let PosedRig { hierarchy, clip, joints: ids, time: t, mode, start } = &mut *rig;
        let Some(clip) = clip.clone() else { continue };
        *t += dt;
        let frame = rig_frame(*mode, *start, *t, clip.frame_rate, clip.frame_count);
        let poses = hierarchy.pose_trs_controlled(Some(&clip), frame, &[]);
        for (&j, (pt, pq)) in ids.iter().zip(poses) {
            if let Ok(mut tf) = joints.get_mut(j) {
                *tf = anim::bevy_trs(pt, pq);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zr_sim::template::GeometryShape;
    use zr_w3d::{TransitionState};

    /// A state with the given `TransitionKey` and `WaitForStateToFinishIfPossible` key.
    fn state(transition_key: Option<&str>, allow: Option<&str>) -> ConditionState {
        ConditionState {
            transition_key: transition_key.map(str::to_string),
            allow_to_finish: allow.map(str::to_string),
            ..Default::default()
        }
    }

    fn draw_states(states: Vec<ConditionState>, transitions: Vec<TransitionState>) -> DrawStates {
        DrawStates { module: "W3DModelDraw".to_string(), states, default_state: None, ignore: ConditionFlags::new(), transitions }
    }

    /// States 0 (`down_default`) and 1 (`up_day`) with a transition 0 between them, state 2 unrelated.
    fn scaffold_like() -> DrawStates {
        let s = vec![state(Some("down_default"), None), state(Some("up_day"), None), state(None, None)];
        let t = vec![TransitionState { from: "down_default".into(), to: "up_day".into(), state: ConditionState::default() }];
        draw_states(s, t)
    }

    #[test]
    fn a_state_change_plays_the_transition_between_the_states() {
        let d = scaffold_like();
        assert_eq!(request(&d, None, None, 1, true), (Shown::State(1), None), "a first state is shown directly");
        assert_eq!(request(&d, Some(Shown::State(0)), None, 1, true), (Shown::Transition { transition: 0, to: 1 }, None));
        assert_eq!(request(&d, Some(Shown::State(0)), None, 2, true), (Shown::State(2), None), "no transition, no wait");
        assert_eq!(request(&d, Some(Shown::State(1)), None, 1, true), (Shown::State(1), None), "no change");
    }

    #[test]
    fn a_request_during_a_transition_replaces_it_unless_it_targets_it() {
        let d = scaffold_like();
        let running = Shown::Transition { transition: 0, to: 1 };
        assert_eq!(request(&d, Some(running), None, 1, false), (running, None), "the same target keeps the transition");
        assert_eq!(request(&d, Some(running), None, 0, false), (Shown::State(0), None), "a new target takes effect at once");
    }

    #[test]
    fn wait_for_state_to_finish_holds_the_running_state_until_its_clip_ends() {
        // State 1 waits for state 0's transition key (`down_default`); state 0 is running with `up_day` (state 1).
        let d = draw_states(vec![state(Some("up_day"), None), state(Some("down_default"), Some("up_day"))], vec![]);
        // Running and not complete: the request waits, the running state keeps showing.
        assert_eq!(request(&d, Some(Shown::State(0)), None, 1, false), (Shown::State(0), Some(1)));
        // Complete: the engine clears the pending state and re-requests it, which now applies at once (the
        // states have no transition between them).
        assert_eq!(request(&d, Some(Shown::State(0)), None, 1, true), (Shown::State(1), None));
        // A wait-key that does not match the running state's key does not hold it.
        let other = draw_states(vec![state(Some("up_day"), None), state(None, Some("up_night"))], vec![]);
        assert_eq!(request(&other, Some(Shown::State(0)), None, 1, false), (Shown::State(1), None));
    }

    #[test]
    fn a_pending_state_is_dropped_by_a_return_to_the_shown_state() {
        let d = scaffold_like();
        assert_eq!(request(&d, Some(Shown::State(0)), Some(2), 0, false), (Shown::State(0), None));
        assert_eq!(request(&d, Some(Shown::State(0)), Some(1), 1, false), (Shown::State(0), Some(1)), "the same pending target keeps waiting");
    }

    #[test]
    fn start_frames_follow_the_flags_and_the_engine_default() {
        let mut rng = ClientRng::new(7);
        let f = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(start_frame(AnimMode::Manual, &[], 40, &mut rng), 0.0);
        assert_eq!(start_frame(AnimMode::Manual, &f(&["START_FRAME_LAST"]), 40, &mut rng), 39.0);
        assert_eq!(start_frame(AnimMode::Once, &f(&["START_FRAME_FIRST"]), 40, &mut rng), 0.0);
        assert_eq!(start_frame(AnimMode::OnceBackwards, &[], 40, &mut rng), 39.0, "a backwards mode starts at its last frame");
        let random: Vec<f32> = (0..64).map(|_| start_frame(AnimMode::Manual, &f(&["RANDOMIZE_START_FRAME"]), 40, &mut rng)).collect();
        assert!(random.iter().all(|&x| (0.0..=39.0).contains(&x)));
        assert!(random.iter().any(|&x| x > 0.0), "a random start is not the first frame every time");
    }

    #[test]
    fn a_repeated_idle_clip_picks_another_animation_and_one_clip_is_always_chosen() {
        let mut rng = ClientRng::new(11);
        assert_eq!(choose_anim(1, Some(0), &mut rng), 0);
        for _ in 0..64 {
            assert_ne!(choose_anim(3, Some(1), &mut rng), 1, "the current animation is avoided");
        }
        let picks: Vec<usize> = (0..64).map(|_| choose_anim(3, None, &mut rng)).collect();
        assert!(picks.contains(&0) && picks.contains(&2), "all clips are picked at random");
    }

    #[test]
    fn transitions_start_and_end_at_the_frames_the_engine_uses() {
        // ONCE from its first frame: frames 0..=125 at 30 fps.
        assert!((completion_secs(AnimMode::Once, 0.0, 126, 30) - 125.0 / 30.0).abs() < 1e-6);
        // ONCE_BACKWARDS from its last frame runs back to the first (the sinking).
        assert!((completion_secs(AnimMode::OnceBackwards, 125.0, 126, 30) - 125.0 / 30.0).abs() < 1e-6);
        // Modes that never complete.
        assert!(completion_secs(AnimMode::Manual, 0.0, 126, 30).is_infinite());
        assert!(completion_secs(AnimMode::Loop, 0.0, 126, 30).is_infinite());
        // The rig holds the end frame of a ONCE and the first of a ONCE_BACKWARDS.
        assert_eq!(rig_frame(AnimMode::Once, 0.0, 100.0, 30, 126), 125.0);
        assert_eq!(rig_frame(AnimMode::OnceBackwards, 125.0, 100.0, 30, 126), 0.0);
        assert!((rig_frame(AnimMode::Once, 0.0, 10.0 / 30.0, 30, 126) - 10.0).abs() < 1e-3);
        // LOOP still wraps.
        assert!((rig_frame(AnimMode::Loop, 0.0, 130.0 / 30.0, 30, 126) - 4.0).abs() < 1e-3);
    }

    #[test]
    fn the_construction_lift_follows_the_state_flag_and_the_percent() {
        let g = Geometry { shape: GeometryShape::Box, major_radius: 15.0, minor_radius: 15.0, height: 20.0, is_small: false };
        assert_eq!(module_lift(false, &g, 50.0, true), 0.0, "a state without the flag is not lifted");
        assert_eq!(module_lift(true, &g, 0.0, true), -20.0);
        assert_eq!(module_lift(true, &g, 50.0, true), -10.0);
        assert_eq!(module_lift(true, &g, 100.0, false), 0.0, "a complete object is not lifted");
    }

    #[test]
    fn hide_show_matches_the_base_name() {
        let hs = vec![("Turret".to_string(), true)];
        assert_eq!(hidden("turret#2", &hs), Some(true));
        assert_eq!(hidden("body", &hs), None);
    }
}
