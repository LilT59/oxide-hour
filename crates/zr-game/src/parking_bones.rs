//! Airfield bones: resolve every `ParkingPlaceBehavior` object's parking-space and runway bones from
//! the install's W3D art and hand them to the sim.
//!
//! Ported from `ParkingPlaceBehavior::buildInfo` (`ParkingPlaceBehavior.cpp:68`), which reads each
//! bone with `Object::getSingleLogicalBonePosition` (the drawable's pristine bone positions):
//! `Runway{col}Park{row}Han` (hangar start), `Runway{col}Parking{row}` (parking location),
//! `Runway{col}Prep{row}` (the runway prep spot), and, for `HasRunways`, `RunwayStart{col}` /
//! `RunwayEnd{col}`; plus `HeliPark01` (`exitObjectViaDoor` / `getExitPosition` for helipads). The
//! orientation is the bone's Z rotation. A bone the model lacks reads back as the object's own
//! position and orientation, i.e. the zero local bone. As in [`crate::dock_bones`] the default
//! condition state's model is used.

use std::collections::BTreeMap;

use zr_assets::Vfs;
use zr_ini::IniDb;
use zr_sim::jet_ai::{Bone, ParkingBones, ParkingSpaceBones, RunwayBones};
use zr_sim::Coord3D;
use zr_templates::{ModuleData, Templates};
use zr_w3d::{ConditionState, Hierarchy};

/// The parking bones of every template with a `ParkingPlaceBehavior`, keyed by name.
pub fn parking_bones(ini: &IniDb, vfs: &Vfs, templates: &Templates) -> BTreeMap<String, ParkingBones> {
    let mut out = BTreeMap::new();
    let mut cache: BTreeMap<String, Hierarchy> = BTreeMap::new();
    for t in templates.things() {
        let Some(pp) = t.modules.iter().find_map(|m| match &m.data {
            ModuleData::ParkingPlace(d) => Some(d.clone()),
            _ => None,
        }) else {
            continue;
        };
        let Some(states) = zr_w3d::states_for_template(ini, &t.name) else { continue };
        let Some(state) = states.default_state().or_else(|| states.find_best_state(&zr_w3d::ConditionFlags::default())) else { continue };
        let Some(model) = state.model.as_deref() else { continue };
        let Some(h) = model_hierarchy(vfs, model, state, &mut cache) else { continue };
        let bind = h.bind_pose_trs();
        let find = |name: &str| h.bone_index(name).map(|i| bind[i]);
        let pos = |name: &str| find(name).map(|b| Coord3D::new(b.0[0], b.0[1], b.0[2])).unwrap_or_default();
        let bone = |name: &str| {
            find(name).map(|b| Bone { pos: Coord3D::new(b.0[0], b.0[1], b.0[2]), orient: z_rotation(b.1) }).unwrap_or_default()
        };
        let mut b = ParkingBones { heli_park: bone("HeliPark01"), ..Default::default() };
        // `buildInfo`: rows outer, columns inner.
        for row in 0..pp.num_rows.max(0) {
            for col in 0..pp.num_cols.max(0) {
                b.spaces.push(ParkingSpaceBones {
                    hangar: bone(&format!("Runway{}Park{}Han", col + 1, row + 1)),
                    location: bone(&format!("Runway{}Parking{}", col + 1, row + 1)),
                    prep: pos(&format!("Runway{}Prep{}", col + 1, row + 1)),
                });
            }
        }
        if pp.has_runways {
            for col in 0..pp.num_cols.max(0) {
                b.runways.push(RunwayBones { start: pos(&format!("RunwayStart{}", col + 1)), end: pos(&format!("RunwayEnd{}", col + 1)) });
            }
        }
        out.insert(t.name.clone(), b);
    }
    out
}

/// `Matrix3D::Get_Z_Rotation` of a rotation quaternion `[x, y, z, w]`: the yaw of its X axis.
pub(crate) fn z_rotation(q: [f32; 4]) -> f32 {
    let (x, y, z, w) = (q[0], q[1], q[2], q[3]);
    zr_sim::math::atan2(2.0 * (w * z + x * y), 1.0 - 2.0 * (y * y + z * z))
}

/// The model file's own hierarchy, then any skeleton named by the state's animations.
pub(crate) fn model_hierarchy<'a>(
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
