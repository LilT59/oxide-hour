//! Dock bones: resolve every dock object's `DockStart` / `DockAction` / `DockEnd` /
//! `DockWaiting01..` rest positions from the install's W3D art and hand them to the sim.
//!
//! Ported from `DockUpdate::loadDockPositions` (`DockUpdate.cpp:600`), which reads the drawable's
//! pristine bone positions (`Drawable::getPristineBonePositions`): `DockStart`, `DockAction` and
//! `DockEnd` are the unadorned names; the approach positions are `DockWaiting01..NN` (up to
//! `NumberApproachPositions`, at most `DEFAULT_APPROACH_VECTOR_SIZE`). Objects with
//! `KINDOF_IGNORE_DOCKING_BONES` (the fortified GLA supply stash) ignore the bones. As in
//! `launch_bones` the default condition state's model is used.

use std::collections::BTreeMap;

use zr_assets::Vfs;
use zr_ini::IniDb;
use zr_sim::dock::{DockBones, DEFAULT_APPROACH_VECTOR_SIZE};
use zr_sim::Coord3D;
use zr_templates::{KindOf, ModuleData, Templates};
use zr_w3d::{ConditionState, Hierarchy};

/// The dock bones of every template with a dock module and at least one dock bone, keyed by name.
pub fn dock_bones(ini: &IniDb, vfs: &Vfs, templates: &Templates) -> BTreeMap<String, DockBones> {
    let mut out = BTreeMap::new();
    let mut cache: BTreeMap<String, Hierarchy> = BTreeMap::new();
    for t in templates.things() {
        let number = t.modules.iter().find_map(|m| match &m.data {
            ModuleData::Dock(d) => Some(d.number_approach_positions),
            ModuleData::RepairDock(d) => Some(d.number_approach_positions),
            _ => None,
        });
        let Some(number) = number else { continue };
        if t.is_kind_of(KindOf::IgnoreDockingBones) {
            continue;
        }
        let Some(states) = zr_w3d::states_for_template(ini, &t.name) else { continue };
        // The drawable starts with no condition flags: `findBestInfo(empty)` (the explicit
        // `DefaultConditionState`, or a `ConditionState = NONE` such as the supply centers').
        let Some(state) = states.default_state().or_else(|| states.find_best_state(&zr_w3d::ConditionFlags::default())) else { continue };
        let Some(model) = state.model.as_deref() else { continue };
        let Some(h) = model_hierarchy(vfs, model, state, &mut cache) else { continue };
        let bind = h.bind_pose_trs();
        let bone = |name: &str| h.bone_index(name).map(|i| Coord3D::new(bind[i].0[0], bind[i].0[1], bind[i].0[2]));
        let mut b = DockBones {
            enter: bone("DockStart").unwrap_or_default(),
            dock: bone("DockAction").unwrap_or_default(),
            exit: bone("DockEnd").unwrap_or_default(),
            approach: Vec::new(),
        };
        // `getPristineBonePositions("DockWaiting", 1, ..., NumberApproachPositions)`: dynamic docks
        // (`-1`) have no bones.
        if number > 0 {
            for i in 1..=(number as usize).min(DEFAULT_APPROACH_VECTOR_SIZE) {
                match bone(&format!("DockWaiting{i:02}")) {
                    Some(p) => b.approach.push(p),
                    None => break,
                }
            }
        }
        if b != DockBones::default() {
            out.insert(t.name.clone(), b);
        }
    }
    out
}

/// The model file's own hierarchy, then any skeleton named by the state's animations.
fn model_hierarchy<'a>(
    vfs: &Vfs,
    model: &str,
    state: &ConditionState,
    cache: &'a mut BTreeMap<String, Hierarchy>,
) -> Option<&'a Hierarchy> {
    if !cache.contains_key(model) {
        let mut candidates: Vec<String> = vec![model.to_string()];
        for anim in &state.animations {
            if let Ok(a) = zr_w3d::load_animation(vfs, &anim.name) {
                candidates.push(a.hierarchy);
            }
        }
        let mut loaded = None;
        for name in candidates {
            if let Ok(h) = zr_w3d::load_hierarchy(vfs, &name) {
                loaded = Some(h);
                break;
            }
        }
        if let Some(h) = loaded {
            cache.insert(model.to_string(), h);
        }
    }
    cache.get(model)
}
