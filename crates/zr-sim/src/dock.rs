//! `DockUpdate` (`Object/Update/DockUpdate/DockUpdate.cpp`) and the dock actions of
//! `SupplyWarehouseDockUpdate`, `SupplyCenterDockUpdate` and `RepairDockUpdate`.
//!
//! A dock owns a list of approach positions (`NumberApproachPositions`) that dockers reserve in
//! order, plus the single `m_activeDocker` that is cleared to enter. [`GameLogic::ai_dock`] drives a
//! docker through the `AIDockMachine` (`crate::ai_states`) which calls the dock functions here in
//! the same order as `AIDock.cpp`.
//!
//! Bones: the engine reads `DockStart` / `DockAction` / `DockEnd` / `DockWaiting01..` from the
//! drawable's pristine model. The sim carries them on [`ObjectTemplate::dock_bones`], object-local;
//! when the adapter has not resolved them they stay zero, which is exactly the C++ "no bones"
//! path: the enter/dock/exit positions are the docker's own position and the approach position is
//! the object origin biased toward the docker by half the major radius, moved to a clear spot by
//! `findPositionAround` (`computeApproachPosition`).

use crate::logic::GameLogic;
use crate::math::{cos, real_to_int_ceil, sin, Coord3D};
use crate::object::ObjectId;

/// `DYNAMIC_APPROACH_VECTOR_FLAG` (`DockUpdate.h:45`).
pub const DYNAMIC_APPROACH_VECTOR_FLAG: i32 = -1;
/// `DEFAULT_APPROACH_VECTOR_SIZE` (`DockUpdate.h:40`).
pub const DEFAULT_APPROACH_VECTOR_SIZE: usize = 10;
/// `PartitionManager::findPositionAround`'s `ringSpacing` (`PartitionManager.cpp:3944`).
const RING_SPACING: f32 = 5.0;
const TWO_PI: f32 = std::f32::consts::TAU;

/// The dock bones in object-local space (`DockUpdate::loadDockPositions`). Zero = not present.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DockBones {
    /// `DockStart` (`m_enterPosition`).
    pub enter: Coord3D,
    /// `DockAction` (`m_dockPosition`).
    pub dock: Coord3D,
    /// `DockEnd` (`m_exitPosition`).
    pub exit: Coord3D,
    /// `DockWaiting01..` (`m_approachPositions`); entries past the model's bones stay zero.
    pub approach: Vec<Coord3D>,
}

/// `DockUpdate`'s runtime fields (`DockUpdate.h:97`), plus `RepairDockUpdate`'s.
#[derive(Clone, Debug, PartialEq)]
pub struct DockState {
    /// `m_approachPositions` / `m_approachPositionOwners` / `m_approachPositionReached` are sized
    /// lazily (`sized`) from the module's `NumberApproachPositions`.
    sized: bool,
    owners: Vec<ObjectId>,
    reached: Vec<bool>,
    /// `m_activeDocker`.
    pub active_docker: ObjectId,
    /// `m_dockerInside`.
    pub docker_inside: bool,
    /// `m_dockCrippled`.
    pub crippled: bool,
    /// `m_dockOpen`.
    pub open: bool,
    /// `RepairDockUpdate::m_lastRepair` (`INVALID_ID` == 0 == "needs init", as in the C++).
    pub last_repair: ObjectId,
    /// `RepairDockUpdate::m_healthToAddPerFrame`.
    pub health_to_add_per_frame: f32,
}

impl Default for DockState {
    fn default() -> Self {
        Self {
            sized: false,
            owners: Vec::new(),
            reached: Vec::new(),
            active_docker: ObjectId::INVALID,
            docker_inside: false,
            crippled: false,
            open: true,
            last_repair: ObjectId::INVALID,
            health_to_add_per_frame: 0.0,
        }
    }
}

impl DockState {
    pub(crate) fn crc(&self, c: &mut crate::crc::Crc) {
        c.add_u32(self.owners.len() as u32);
        for (o, r) in self.owners.iter().zip(&self.reached) {
            c.add_u32(o.0);
            c.add_bool(*r);
        }
        c.add_u32(self.active_docker.0);
        c.add_bool(self.docker_inside);
        c.add_bool(self.crippled);
        c.add_bool(self.open);
        c.add_u32(self.last_repair.0);
        c.add_f32(self.health_to_add_per_frame);
    }
}

/// `FindPositionOptions` (`PartitionManager.h:123`) for the callers in this crate.
#[derive(Clone, Copy, Debug)]
pub struct FindPositionOptions {
    pub min_radius: f32,
    pub max_radius: f32,
    /// `ignoreObject`.
    pub ignore_object: ObjectId,
    /// `sourceToPathToDest`.
    pub source_to_path_to_dest: ObjectId,
    /// `maxZDelta`: the largest height difference from the center a spot may have.
    pub max_z_delta: f32,
    /// `startAngle` (`PartitionManager.h:139`): `None` is `RANDOM_START_ANGLE` (`PartitionManager.h:121`, the search
    /// starts at a random angle drawn from the logic RNG, `PartitionManager.cpp:3996-3998`); `Some` starts at that angle.
    pub start_angle: Option<f32>,
}

impl Default for FindPositionOptions {
    fn default() -> Self {
        Self {
            min_radius: 0.0,
            max_radius: 0.0,
            ignore_object: ObjectId::INVALID,
            source_to_path_to_dest: ObjectId::INVALID,
            // `FindPositionOptions()` (`PartitionManager.h:129-131`): any z delta.
            max_z_delta: 1e10,
            start_angle: None,
        }
    }
}

/// Which dock module an object carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DockClass {
    SupplyWarehouse,
    SupplyCenter,
    Repair,
}

impl DockClass {
    /// The C++ module class name, as the `ZR_L99_HEAP` dump prints it.
    pub(crate) fn class_name(self) -> &'static str {
        match self {
            DockClass::SupplyWarehouse => "SupplyWarehouseDockUpdate",
            DockClass::SupplyCenter => "SupplyCenterDockUpdate",
            DockClass::Repair => "RepairDockUpdate",
        }
    }
}

impl GameLogic {
    /// `Object::getDockUpdateInterface`: the dock class and `DockUpdateModuleData` fields
    /// (`NumberApproachPositions`, `AllowsPassthrough`).
    pub(crate) fn dock_module(&self, dock: ObjectId) -> Option<(DockClass, i32, bool)> {
        let o = self.objects.get(&dock)?;
        if o.destroyed {
            return None;
        }
        if let Some(d) = &o.template.dock {
            let class = match d.kind {
                crate::supply::DockKind::Warehouse => DockClass::SupplyWarehouse,
                crate::supply::DockKind::Center => DockClass::SupplyCenter,
            };
            return Some((class, d.number_approach_positions, d.allows_passthrough));
        }
        o.template.repair_dock.map(|r| (DockClass::Repair, r.number_approach_positions, r.allows_passthrough))
    }

    pub(crate) fn has_dock_interface(&self, dock: ObjectId) -> bool {
        self.dock_module(dock).is_some()
    }

    /// `DockUpdate::DockUpdate` sizing, done on first use.
    fn dock_ensure_sized(&mut self, dock: ObjectId) {
        let Some((_, n, _)) = self.dock_module(dock) else { return };
        let Some(o) = self.objects.get_mut(&dock) else { return };
        let st = &mut o.dock_state;
        if st.sized {
            return;
        }
        st.sized = true;
        let len = if n == DYNAMIC_APPROACH_VECTOR_FLAG { DEFAULT_APPROACH_VECTOR_SIZE } else { n.max(0) as usize };
        st.owners = vec![ObjectId::INVALID; len];
        st.reached = vec![false; len];
    }

    /// `DockUpdateInterface::isDockOpen`.
    pub(crate) fn dock_is_open(&self, dock: ObjectId) -> bool {
        self.objects.get(&dock).is_some_and(|o| o.dock_state.open)
    }

    /// `DockUpdate::isAllowPassthroughType`.
    pub(crate) fn dock_allows_passthrough(&self, dock: ObjectId) -> bool {
        self.dock_module(dock).is_some_and(|(_, _, p)| p)
    }

    /// `DockUpdate::isClearToApproach` (`DockUpdate.cpp:93`).
    pub(crate) fn dock_is_clear_to_approach(&mut self, dock: ObjectId, docker: ObjectId) -> bool {
        self.dock_ensure_sized(dock);
        let Some((_, n, _)) = self.dock_module(dock) else { return false };
        if n == DYNAMIC_APPROACH_VECTOR_FLAG {
            return true;
        }
        let Some(o) = self.objects.get(&dock) else { return false };
        for owner in &o.dock_state.owners {
            if *owner == ObjectId::INVALID || *owner == docker {
                return true;
            }
        }
        false
    }

    /// `DockUpdate::reserveApproachPosition` (`DockUpdate.cpp:116`): the position and index.
    pub(crate) fn dock_reserve_approach(&mut self, dock: ObjectId, docker: ObjectId) -> Option<(Coord3D, i32)> {
        self.dock_ensure_sized(dock);
        let (_, n, _) = self.dock_module(dock)?;
        let found = {
            let st = &mut self.objects.get_mut(&dock)?.dock_state;
            let mut hit = None;
            for i in 0..st.owners.len() {
                if st.owners[i] == docker {
                    hit = Some(i);
                    break;
                }
                if st.owners[i] == ObjectId::INVALID {
                    st.owners[i] = docker;
                    hit = Some(i);
                    break;
                }
            }
            hit
        };
        if let Some(i) = found {
            return Some((self.dock_compute_approach_position(dock, i, docker), i as i32));
        }
        // Full: dynamic docks append a free spot instead of saying no (`DockUpdate.cpp:149`).
        if n == DYNAMIC_APPROACH_VECTOR_FLAG {
            let i = {
                let st = &mut self.objects.get_mut(&dock)?.dock_state;
                st.owners.push(docker);
                st.reached.push(false);
                st.owners.len() - 1
            };
            return Some((self.dock_compute_approach_position(dock, i, docker), i as i32));
        }
        None
    }

    /// `DockUpdate::advanceApproachPosition` (`DockUpdate.cpp:181`).
    pub(crate) fn dock_advance_approach(&mut self, dock: ObjectId, docker: ObjectId, index: i32) -> Option<(Coord3D, i32)> {
        self.dock_ensure_sized(dock);
        if index <= 0 {
            return None;
        }
        let prev = (index - 1) as usize;
        {
            let st = &mut self.objects.get_mut(&dock)?.dock_state;
            if prev >= st.owners.len() || st.owners[prev] != ObjectId::INVALID {
                return None;
            }
            st.owners[prev] = docker;
            st.reached[prev] = false;
            let his = index as usize;
            if his < st.owners.len() {
                st.owners[his] = ObjectId::INVALID;
                st.reached[his] = false;
            }
        }
        Some((self.dock_compute_approach_position(dock, prev, docker), prev as i32))
    }

    /// `DockUpdate::isClearToEnter`.
    pub(crate) fn dock_is_clear_to_enter(&self, dock: ObjectId, docker: ObjectId) -> bool {
        self.objects.get(&dock).is_some_and(|o| o.dock_state.active_docker == docker)
    }

    /// `DockUpdate::isClearToAdvance` (`DockUpdate.cpp:220`).
    pub(crate) fn dock_is_clear_to_advance(&self, dock: ObjectId, docker: ObjectId, index: i32) -> bool {
        if index < 0 {
            return false;
        }
        let Some(o) = self.objects.get(&dock) else { return false };
        let st = &o.dock_state;
        let i = index as usize;
        if i >= st.owners.len() {
            return false;
        }
        let correct = st.owners[i] == docker;
        let reached = st.reached[i];
        let next_free = i > 0 && st.owners[i - 1] == ObjectId::INVALID;
        correct && reached && next_free
    }

    /// The object-local bone -> world (`Object::convertBonePosToWorldPos` with no transform).
    fn dock_bone_to_world(&self, dock: ObjectId, local: Coord3D) -> Coord3D {
        let Some(o) = self.objects.get(&dock) else { return local };
        let (s, c) = (sin(o.angle), cos(o.angle));
        Coord3D::new(o.position.x + local.x * c - local.y * s, o.position.y + local.x * s + local.y * c, o.position.z + local.z)
    }

    /// `DockUpdate::getEnterPosition` (`DockUpdate.cpp:247`).
    pub(crate) fn dock_enter_position(&self, dock: ObjectId, docker: ObjectId) -> Coord3D {
        let bones = self.objects.get(&dock).map(|o| o.template.dock_bones.clone()).unwrap_or_default();
        let dpos = self.objects.get(&docker).map_or(Coord3D::ZERO, |o| o.position);
        if bones.enter == Coord3D::ZERO {
            // "If I don't have a bone, you are fine where you are, unless you fly."
            if self.is_airborne_locomotor(docker) {
                return self.objects.get(&dock).map_or(dpos, |o| o.position);
            }
            return dpos;
        }
        self.dock_bone_to_world(dock, bones.enter)
    }

    /// `DockUpdate::getDockPosition` (`DockUpdate.cpp:279`; keys off `m_enterPosition`).
    pub(crate) fn dock_dock_position(&self, dock: ObjectId, docker: ObjectId) -> Coord3D {
        let bones = self.objects.get(&dock).map(|o| o.template.dock_bones.clone()).unwrap_or_default();
        if bones.enter == Coord3D::ZERO {
            return self.objects.get(&docker).map_or(Coord3D::ZERO, |o| o.position);
        }
        self.dock_bone_to_world(dock, bones.dock)
    }

    /// `DockUpdate::getExitPosition` (`DockUpdate.cpp:304`; keys off `m_enterPosition`).
    pub(crate) fn dock_exit_position(&self, dock: ObjectId, docker: ObjectId) -> Coord3D {
        let bones = self.objects.get(&dock).map(|o| o.template.dock_bones.clone()).unwrap_or_default();
        if bones.enter == Coord3D::ZERO {
            return self.objects.get(&docker).map_or(Coord3D::ZERO, |o| o.position);
        }
        self.dock_bone_to_world(dock, bones.exit)
    }

    /// `Object::isUsingAirborneLocomotor`.
    pub(crate) fn is_airborne_locomotor(&self, id: ObjectId) -> bool {
        self.objects.get(&id).and_then(|o| o.locomotor()).is_some_and(|l| l.surfaces_air)
    }

    /// `DockUpdate::onApproachReached` (`DockUpdate.cpp:329`).
    pub(crate) fn dock_on_approach_reached(&mut self, dock: ObjectId, docker: ObjectId) {
        if let Some(o) = self.objects.get_mut(&dock) {
            let st = &mut o.dock_state;
            for i in 0..st.owners.len() {
                if st.owners[i] == docker {
                    st.reached[i] = true;
                    return;
                }
            }
        }
    }

    /// `DockUpdate::onEnterReached` (`DockUpdate.cpp:342`). The `MODELCONDITION_DOCKING_*` flags
    /// are a client concern and are not derived.
    pub(crate) fn dock_on_enter_reached(&mut self, dock: ObjectId, docker: ObjectId) {
        if let Some(o) = self.objects.get_mut(&dock) {
            let st = &mut o.dock_state;
            st.docker_inside = true;
            for i in 0..st.owners.len() {
                if st.owners[i] == docker {
                    st.owners[i] = ObjectId::INVALID;
                    st.reached[i] = false;
                    return;
                }
            }
        }
    }

    /// `DockUpdate::onExitReached` (`DockUpdate.cpp:379`).
    pub(crate) fn dock_on_exit_reached(&mut self, dock: ObjectId, docker: ObjectId) {
        if let Some(o) = self.objects.get_mut(&dock) {
            let st = &mut o.dock_state;
            st.docker_inside = false;
            if st.active_docker == docker {
                st.active_docker = ObjectId::INVALID;
            }
        }
    }

    /// `DockUpdate::cancelDock` (`DockUpdate.cpp:403`).
    pub(crate) fn dock_cancel(&mut self, dock: ObjectId, docker: ObjectId) {
        if let Some(o) = self.objects.get_mut(&dock) {
            let st = &mut o.dock_state;
            for i in 0..st.owners.len() {
                if st.owners[i] == docker {
                    st.owners[i] = ObjectId::INVALID;
                    st.reached[i] = false;
                }
            }
            if st.active_docker == docker {
                st.active_docker = ObjectId::INVALID;
                st.docker_inside = false;
            }
        }
    }

    /// `DockUpdate::setDockCrippled` + `SupplyWarehouseDockUpdate::setDockCrippled`.
    pub fn set_dock_crippled(&mut self, dock: ObjectId, crippled: bool) {
        let class = self.dock_module(dock).map(|(c, _, _)| c);
        if class == Some(DockClass::SupplyWarehouse) && crippled {
            let (active, inside) = self.objects.get(&dock).map_or((ObjectId::INVALID, false), |o| (o.dock_state.active_docker, o.dock_state.docker_inside));
            if active != ObjectId::INVALID && self.objects.contains_key(&active) {
                if inside {
                    if !self.is_airborne_locomotor(active) {
                        self.kill_object(active);
                    }
                } else if self.objects.get(&active).is_some_and(|o| o.template.supply_truck.is_some()) {
                    // He was between Approach and Enter: stop, and remind him to try again later.
                    self.ai_idle(active, crate::ai_states::CommandSource::Ai);
                    if let Some(o) = self.objects.get_mut(&active) {
                        o.supply_ai.force_wanting = true;
                    }
                }
            }
        }
        if let Some(o) = self.objects.get_mut(&dock) {
            o.dock_state.crippled = crippled;
        }
    }

    /// `DockUpdate::update` for one dock, run from the sleepy heap (`DockUpdate.cpp:416`). It returns
    /// `UPDATE_SLEEP_NONE` on every path, so a dock is awake every frame. The supply-source half of the C++
    /// body clears a docker's drawable `MOVING` condition; the sim has no drawables, so only the entry
    /// grant remains.
    pub(crate) fn dock_update(&mut self, dock: ObjectId) -> u32 {
        let Some(o) = self.objects.get_mut(&dock) else { return crate::module::UPDATE_SLEEP_NONE };
        let st = &mut o.dock_state;
        if st.active_docker == ObjectId::INVALID && !st.crippled {
            for i in 0..st.reached.len() {
                if st.reached[i] {
                    st.active_docker = st.owners[i];
                    break;
                }
            }
        }
        crate::module::UPDATE_SLEEP_NONE
    }

    /// `DockUpdate::computeApproachPosition` (`DockUpdate.cpp:487`).
    fn dock_compute_approach_position(&mut self, dock: ObjectId, index: usize, for_whom: ObjectId) -> Coord3D {
        let Some((_, n, _)) = self.dock_module(dock) else { return Coord3D::ZERO };
        let (bones, dpos, major) = match self.objects.get(&dock) {
            Some(o) => (o.template.dock_bones.clone(), o.position, o.template.geometry.major_radius),
            None => return Coord3D::ZERO,
        };
        // `m_numberApproachPositionBones`: dynamic docks have none; otherwise the bones found.
        let n_bones = if n == DYNAMIC_APPROACH_VECTOR_FLAG { 0 } else { bones.approach.len().min(n.max(0) as usize) };
        let local = if index < n_bones { bones.approach[index] } else { Coord3D::ZERO };
        let mut working = self.dock_bone_to_world(dock, local);
        if n_bones == 0 {
            // A boneless building biases toward the caller (`DockUpdate.cpp:504`).
            let their = self.objects.get(&for_whom).map_or(dpos, |o| o.position);
            let (dx, dy, dz) = (their.x - dpos.x, their.y - dpos.y, their.z - dpos.z);
            let l = (dx * dx + dy * dy + dz * dz).sqrt();
            if l > 0.0 {
                let k = major / 2.0;
                working.x += dx / l * k;
                working.y += dy / l * k;
                working.z += dz / l * k;
            }
        }
        let mut opts = FindPositionOptions { min_radius: 0.0, max_radius: 100.0, source_to_path_to_dest: for_whom, ..Default::default() };
        if self.is_airborne_locomotor(for_whom) {
            opts.ignore_object = dock; // Flyers can approach right over the dock.
        }
        self.find_position_around(&working, &opts).unwrap_or(working)
    }

    /// `PartitionManager::findPositionAround` (`PartitionManager.cpp:3973`): a ring search for a
    /// legal spot (`tryPosition`, `:3797`). `FPF_NONE` flags: cliff, impassable and underwater cells
    /// and overlaps with other objects are rejected, and `sourceToPathToDest` must be able to reach
    /// the spot. The start angle is random (`RANDOM_START_ANGLE`), drawn from the logic RNG.
    pub(crate) fn find_position_around(&mut self, center: &Coord3D, opts: &FindPositionOptions) -> Option<Coord3D> {
        let (w, h) = self.terrain.world_size();
        // Off the map: a scripted setup, so just use the center.
        if center.x < 0.0 || center.y < 0.0 || center.x > w || center.y > h {
            return Some(*center);
        }
        let start_angle = match opts.start_angle {
            Some(angle) => angle,
            None => self.rng.value_real(0.0, TWO_PI),
        };
        let mut dist = opts.min_radius;
        while dist <= opts.max_radius {
            let spacing = if dist == opts.min_radius { TWO_PI } else { (RING_SPACING / (dist + 1.0)) * (TWO_PI / 6.0) };
            let samples = real_to_int_ceil((TWO_PI / spacing) / 2.0);
            for i in 0..samples {
                if let Some(p) = self.try_position(center, dist, start_angle + spacing * i as f32, opts) {
                    return Some(p);
                }
                if i != 0 {
                    if let Some(p) = self.try_position(center, dist, start_angle - spacing * i as f32, opts) {
                        return Some(p);
                    }
                }
            }
            dist += RING_SPACING;
        }
        None
    }

    /// `PartitionManager::tryPosition` (`PartitionManager.cpp:3797`).
    fn try_position(&mut self, center: &Coord3D, dist: f32, angle: f32, opts: &FindPositionOptions) -> Option<Coord3D> {
        let (x, y) = (dist * cos(angle) + center.x, dist * sin(angle) + center.y);
        let pos = Coord3D::new(x, y, self.terrain.ground_height(x, y));
        // `fabs(pos.z - center->z) > options->maxZDelta` (`PartitionManager.cpp:3823`).
        if (pos.z - center.z).abs() > opts.max_z_delta {
            return None;
        }
        // The path source decides which surfaces count as standable; with none, ground. The C++
        // `tryPosition` asks `clientSafeQuickDoesPathExist(ai->getLocomotorSet(), ...)` of the live
        // source object (`PartitionManager.cpp:4046-4052`).
        let po = if opts.source_to_path_to_dest != ObjectId::INVALID {
            self.objects.get(&opts.source_to_path_to_dest).and_then(crate::pathfind::PathObject::for_object)
        } else {
            None
        };
        let surfaces = po.map_or(crate::pathfind::surface::GROUND, |p| p.surfaces);
        if !self.pathfinder.can_place(surfaces, &pos) {
            return None;
        }
        // A very small sphere (radius 5) against the potential collisions (`PartitionFilterWouldCollide`),
        // turned by the sample's angle (`tryPosition`'s `iteratePotentialCollisions(&pos, geometry, angle, true)`).
        if !self.small_sphere_clear_at(&pos, angle, &[opts.ignore_object, opts.source_to_path_to_dest]) {
            return None;
        }
        if let (Some(po), Some(src)) = (po, self.objects.get(&opts.source_to_path_to_dest).map(|o| o.position)) {
            if !self.pathfinder.quick_does_path_exist(&po, &src, &pos) {
                return None;
            }
        }
        Some(pos)
    }

    /// `ThePartitionManager->getDistanceSquared(a, b, FROM_BOUNDINGSPHERE_2D)`
    /// (`distCalcProc_BoundaryAndBoundary_2D`, `PartitionManager.cpp:806`).
    pub(crate) fn boundary_dist_sqr_2d(&self, a: ObjectId, b: ObjectId) -> f32 {
        let (Some(oa), Some(ob)) = (self.objects.get(&a), self.objects.get(&b)) else { return f32::MAX };
        let diff = oa.position.dist_2d(&ob.position);
        let rad = oa.template.geometry.bounding_circle_radius() + ob.template.geometry.bounding_circle_radius();
        if rad > 0.0 {
            let shrunk = diff - rad;
            if shrunk <= 0.0 {
                return 0.0;
            }
            return shrunk * shrunk;
        }
        diff * diff
    }

    /// `DockUpdateInterface::action` for whichever dock module `dock` carries. `true` = keep
    /// docking; `false` = done (`AIDockProcessDockState::update`).
    pub(crate) fn dock_action(&mut self, docker: ObjectId, dock: ObjectId, drone: Option<ObjectId>) -> bool {
        match self.dock_module(dock).map(|(c, _, _)| c) {
            Some(DockClass::SupplyWarehouse) => self.supply_warehouse_action(docker, dock),
            Some(DockClass::SupplyCenter) => self.supply_center_action(docker, dock),
            Some(DockClass::Repair) => self.repair_dock_action(docker, dock, drone),
            None => false,
        }
    }

    /// `RepairDockUpdate::action` (`RepairDockUpdate.cpp:80`). Healing is `attemptHealing` with
    /// `DAMAGE_HEALING`.
    fn repair_dock_action(&mut self, docker: ObjectId, dock: ObjectId, drone: Option<ObjectId>) -> bool {
        let Some(frames) = self.objects.get(&dock).and_then(|d| d.template.repair_dock).map(|r| r.frames_for_full_heal) else { return false };
        let Some((max, cur)) = self.objects.get(&docker).and_then(|o| o.body.as_ref()).map(|b| (b.max_health(), b.health())) else { return false };
        if self.objects.get(&dock).is_some_and(|d| d.dock_state.last_repair == ObjectId::INVALID) {
            // `m_lastRepair == 0`: first action for this docker.
            if let Some(d) = self.objects.get_mut(&dock) {
                d.dock_state.last_repair = docker;
                d.dock_state.health_to_add_per_frame = (max - cur) / frames;
            }
        }
        if cur >= max {
            if let Some(d) = self.objects.get_mut(&dock) {
                d.dock_state.last_repair = ObjectId::INVALID;
            }
            return false;
        }
        let amount = self.objects.get(&dock).map_or(0.0, |d| d.dock_state.health_to_add_per_frame);
        self.heal_object(docker, amount);
        if let Some(dr) = drone {
            let dmax = self.objects.get(&dr).and_then(|o| o.body.as_ref()).map_or(0.0, |b| b.max_health());
            self.heal_object(dr, dmax);
        }
        true
    }

    /// `AIDockProcessDockState::findMyDrone` (`AIDock.cpp:597`): a `KINDOF_DRONE` this docker
    /// produced.
    pub(crate) fn dock_find_drone(&self, docker: ObjectId) -> Option<ObjectId> {
        let player = self.objects.get(&docker)?.player;
        self.objects
            .values()
            .find(|o| !o.destroyed && o.player == player && o.producer_id == docker && o.is_kind_of(crate::template::kind_of::DRONE))
            .map(|o| o.id)
    }
}
