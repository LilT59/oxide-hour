//! Containment bones: resolve garrison `FIREPOINT` / `STATION` bones and `PassengersInTurret`
//! fire points from the install's W3D art, and hand them to the sim.
//!
//! Ported from `GarrisonContain::loadGarrisonPoints` / `loadStationGarrisonPoints` and
//! `OpenContain::putObjAtNextFirePoint`:
//!
//! * An enclosing garrison switches the model to `GARRISONED` + each body-damage state and reads
//!   `FIREPOINT01..99` (`Object::getMultiLogicalBonePosition`, stopping at the first missing bone).
//! * A non-enclosing garrison (firebase) reads `STATIONnn` from the `GARRISONED` pristine state
//!   only, up to `ContainMax` (`loadStationGarrisonPoints`).
//! * A `PassengersInTurret` container reads the default condition state's `FIREPOINT01..99` plus the
//!   `Turret` bone's rest position, the pivot the sim rotates the offset about
//!   (`Object::getSingleLogicalBonePositionOnTurret`).
//! * A tunnel network reads each exit path's `ExitStart` / `ExitEnd` pair (`OpenContain::exitObjectViaDoor`,
//!   `OpenContain.cpp:1038-1057`): `ExitStart` / `ExitEnd` for one exit path, `ExitStart01`.. for more.
//!
//! Here the model is selected with [`zr_w3d::states_for_template`] + [`DrawStates::find_best_state`],
//! and the bones come from the model's hierarchy rest pose in object-local space
//! ([`Hierarchy::bind_pose_trs`]); the sim transforms them by the structure's position and
//! orientation.
//!
//! Approximation (documented): the engine also folds time-of-day / weather condition bits into the
//! model choice (`W3DModelDrawModuleData::validateStuffForTimeAndWeather`); we resolve the base
//! `GARRISONED` + damage state only.

use std::collections::BTreeMap;

use zr_assets::Vfs;
use zr_ini::IniDb;
use zr_sim::contain::GarrisonFirePoints;
use zr_sim::Coord3D;
use zr_templates::{ContainKind, ModuleData, Templates};
use zr_w3d::{ConditionState, Hierarchy};

/// The condition sets `loadGarrisonPoints` toggles: `GARRISONED` plus each body-damage state.
/// Order matches `GARRISON_POINT_PRISTINE` / `DAMAGED` / `REALLY_DAMAGED`.
const GARRISON_CONDITIONS: [&[&str]; 3] = [&["GARRISONED"], &["GARRISONED", "DAMAGED"], &["GARRISONED", "REALLYDAMAGED"]];

/// `FIREPOINTnn` is scanned `01..99` and stops at the first missing bone.
const MAX_FIREPOINTS: usize = 99;
/// `GarrisonContain::MAX_GARRISON_POINTS` caps the cached `STATION` list.
const MAX_GARRISON_POINTS: usize = 40;

/// The containment bones of every template that needs them, keyed by template name.
pub fn garrison_fire_points(ini: &IniDb, vfs: &Vfs, templates: &Templates) -> BTreeMap<String, GarrisonFirePoints> {
    let mut out = BTreeMap::new();
    // A model is shared by many templates; load each hierarchy once.
    let mut cache: BTreeMap<String, Hierarchy> = BTreeMap::new();
    for t in templates.things() {
        let Some(d) = t.modules.iter().find_map(|m| match &m.data {
            ModuleData::Contain(c) => Some(c.as_ref()),
            _ => None,
        }) else {
            continue;
        };
        let enclosing = d.kind == ContainKind::Garrison && d.is_enclosing_container;
        let non_enclosing = d.kind == ContainKind::Garrison && !d.is_enclosing_container;
        let in_turret = d.passengers_in_turret;
        // Every container that leaves through `OpenContain::exitObjectViaDoor` reads its exit bones: all but a garrison
        // (its own exit, GarrisonContain.cpp:1498) and a railed transport (its dock, RailedTransportContain.cpp:108).
        let open_exit = d.number_of_exit_paths > 0 && !matches!(d.kind, ContainKind::Garrison | ContainKind::RailedTransport);
        if !enclosing && !non_enclosing && !in_turret && !open_exit {
            continue;
        }
        let Some(states) = zr_w3d::states_for_template(ini, &t.name) else { continue };
        let mut points = GarrisonFirePoints::default();
        if open_exit {
            // `OpenContain::exitObjectViaDoor` reads the exit bones of the model's current state
            // (`getSingleLogicalBonePosition` -> the drawable's pristine bones), default state first.
            if let Some(state) = states.default_state().or_else(|| states.find_best_state(&zr_w3d::ConditionFlags::default())) {
                let paths = d.number_of_exit_paths.max(0) as usize;
                let h = hierarchy(vfs, state, &mut cache);
                if let Some(h) = h {
                    let bind = h.bind_pose_trs();
                    let at = |name: &str| h.bone_index(name).map(|i| Coord3D::new(bind[i].0[0], bind[i].0[1], bind[i].0[2]));
                    for k in 1..=paths {
                        let suffix = if paths > 1 { format!("{k:02}") } else { String::new() };
                        points.exit_bones.push((at(&format!("ExitStart{suffix}")), at(&format!("ExitEnd{suffix}"))));
                    }
                }
            }
        }
        if enclosing {
            for (condition, names) in GARRISON_CONDITIONS.iter().enumerate() {
                let flags = zr_w3d::ConditionFlags::from_names(names.iter().copied());
                let Some(state) = states.find_best_state(&flags) else { continue };
                points.conditions[condition] = bones(vfs, state, "FIREPOINT", MAX_FIREPOINTS, &mut cache);
            }
        }
        if non_enclosing {
            // `loadStationGarrisonPoints`: only the `GARRISONED` pristine state, up to `ContainMax`.
            let flags = zr_w3d::ConditionFlags::from_names(["GARRISONED"]);
            if let Some(state) = states.find_best_state(&flags) {
                let cap = if d.contain_max > 0 { d.contain_max as usize } else { MAX_GARRISON_POINTS };
                points.station_points = bones(vfs, state, "STATION", cap.min(MAX_GARRISON_POINTS), &mut cache);
            }
        }
        if in_turret {
            // `putObjAtNextFirePoint` reads the current (unconditioned) state's `FIREPOINT` bones;
            // the turret pivot is the `Turret` bone of the same state. `ConditionState = NONE` is
            // common for vehicles, so select with empty flags rather than only the default state.
            if let Some(state) = states.find_best_state(&zr_w3d::ConditionFlags::new()) {
                points.turret_points = bones(vfs, state, "FIREPOINT", MAX_FIREPOINTS, &mut cache);
                points.turret_pivot = state
                    .turret
                    .as_deref()
                    .and_then(|name| hierarchy(vfs, state, &mut cache).and_then(|h| h.bone_index(name).map(|i| h.bind_pose_trs()[i].0)))
                    .map(|p| Coord3D::new(p[0], p[1], p[2]));
            }
        }
        if points.conditions.iter().any(|c| !c.is_empty())
            || !points.station_points.is_empty()
            || !points.turret_points.is_empty()
            || !points.exit_bones.is_empty()
        {
            out.insert(t.name.clone(), points);
        }
    }
    out
}

/// The `PREFIX01..NN` rest-pose bone translations of a state's model, in object-local space,
/// stopping at the first missing index. Tries the model file's own hierarchy first, then any
/// skeleton named by the state's animations (some models keep the hierarchy in a separate file).
fn bones(vfs: &Vfs, state: &ConditionState, prefix: &str, max: usize, cache: &mut BTreeMap<String, Hierarchy>) -> Vec<Coord3D> {
    let Some(h) = hierarchy(vfs, state, cache) else { return Vec::new() };
    let bind = h.bind_pose_trs();
    let mut out = Vec::new();
    for i in 1..=max {
        match h.bone_index(&format!("{prefix}{i:02}")) {
            Some(index) => out.push(Coord3D::new(bind[index].0[0], bind[index].0[1], bind[index].0[2])),
            None => break,
        }
    }
    out
}

/// The model file's own hierarchy, then any skeleton named by the state's animations. Memoised per
/// model (a model is shared by many templates).
fn hierarchy<'a>(vfs: &Vfs, state: &ConditionState, cache: &'a mut BTreeMap<String, Hierarchy>) -> Option<&'a Hierarchy> {
    let model = state.model.as_deref()?;
    if !cache.contains_key(model) {
        let mut candidates: Vec<String> = vec![model.to_string()];
        for anim in &state.animations {
            if let Ok(a) = zr_w3d::load_animation(vfs, &anim.name) {
                candidates.push(a.hierarchy);
            }
        }
        for name in candidates {
            if let Ok(h) = zr_w3d::load_hierarchy(vfs, &name) {
                cache.insert(model.to_string(), h);
                break;
            }
        }
    }
    cache.get(model)
}
