//! Spawn point bones: resolve every `SpawnPointProductionExitUpdate` object's `SpawnPointBoneName`
//! bones from the install's W3D art and hand them to the sim.
//!
//! Ported from `SpawnPointProductionExitUpdate::initializeBonePositions`
//! (`SpawnPointProductionExitUpdate.cpp:143`): the drawable's pristine bones `<name>01`, `<name>02`, ...
//! are read from index 1 up to `MAX_SPAWN_POINTS` (10), stopping at the first missing bone
//! (`W3DModelDraw::getPristineBonePositionsForConditionState`). Each bone is kept as its object-space
//! position and Z rotation; the sim places it with the building's position and heading. As in
//! [`crate::parking_bones`] the default condition state's model is used.

use std::collections::BTreeMap;

use zr_assets::Vfs;
use zr_ini::IniDb;
use zr_sim::production::SpawnPointBone;
use zr_templates::{ModuleData, ProductionExitKind, Templates};
use zr_w3d::Hierarchy;

use crate::parking_bones::{model_hierarchy, z_rotation};

/// `SpawnPointProductionExitUpdate::MAX_SPAWN_POINTS`: the most bones read.
const MAX_SPAWN_POINTS: usize = 10;

/// The spawn point bones of every template with a `SpawnPointProductionExitUpdate`, keyed by name.
pub fn spawn_bones(ini: &IniDb, vfs: &Vfs, templates: &Templates) -> BTreeMap<String, Vec<SpawnPointBone>> {
    let mut out = BTreeMap::new();
    let mut cache: BTreeMap<String, Hierarchy> = BTreeMap::new();
    for t in templates.things() {
        let Some(prefix) = t.modules.iter().find_map(|m| match &m.data {
            ModuleData::ProductionExit(e) if e.kind == ProductionExitKind::SpawnPoint => Some(e.spawn_point_bone_name.clone()),
            _ => None,
        }) else {
            continue;
        };
        let Some(states) = zr_w3d::states_for_template(ini, &t.name) else { continue };
        let Some(state) = states.default_state().or_else(|| states.find_best_state(&zr_w3d::ConditionFlags::default())) else {
            continue;
        };
        let Some(model) = state.model.as_deref() else { continue };
        let Some(h) = model_hierarchy(vfs, model, state, &mut cache) else { continue };
        let bind = h.bind_pose_trs();
        let mut bones = Vec::new();
        for i in 1..=MAX_SPAWN_POINTS {
            let Some(index) = h.bone_index(&format!("{prefix}{i:02}")) else { break };
            let b = bind[index];
            bones.push(SpawnPointBone { pos: (b.0[0], b.0[1], b.0[2]), angle: z_rotation(b.1) });
        }
        out.insert(t.name.clone(), bones);
    }
    out
}
