//! Round-4 gameplay modules: GLA rebuild holes (`RebuildHoleBehavior` / `RebuildHoleExposeDie`),
//! aircraft countermeasures (`CountermeasuresBehavior`), demolition traps (`DemoTrapUpdate`),
//! hazard cleanup (`CleanupHazardUpdate`), `CommandButtonHuntUpdate` and minefield generation
//! (`GenerateMinefieldBehavior`).
//!
//! Ported from `GameLogic/Object/Behavior/{RebuildHoleBehavior,CountermeasuresBehavior,
//! GenerateMinefieldBehavior}.cpp`, `GameLogic/Object/Die/RebuildHoleExposeDie.cpp` and
//! `GameLogic/Object/Update/{DemoTrapUpdate,CleanupHazardUpdate,CommandButtonHuntUpdate}.cpp`.
//! `JetSlowDeathBehavior` lives with the other die modules in [`crate::die`].
//!
//! Reductions (documented, faithful where it matters):
//! * Countermeasure flares are real objects created from the aircraft's `FlareTemplateName`
//!   (lifetime and physics reuse the existing systems); `calculateCountermeasureToDivertTo` picks
//!   the nearest flare of the newest volley exactly like the `TheSuperHackers` fix.
//! * `DemoTrapUpdate`'s manual/external trigger needs command buttons (not ported): the trap runs
//!   its proximity mode, which is what the shipped GLA traps default to.
//! * `CommandButtonHuntUpdate` needs the command-button table (not ported): a hunt command is set
//!   through [`GameLogic::set_hunt_command`]. The weapon hunt drives the generic AI hunt. The
//!   special-power hunt scans for the closest enemy and uses the supplied power
//!   (`ActionManager::canDoSpecialPowerAtObject` and the button -> power lookup are not ported).
//!   The enter hunt (hijack / carbomb / sabotage) scans near targets and issues the enter command
//!   through the ported [`crate::enter_actions`] checks.
//! * `GenerateMinefieldBehavior` lays the border on a circle (the box/smart-border expansions are
//!   reduced to their bounding radius) and the footprint uses a circular random distribution.

use std::sync::Arc;

use crate::body::DamageInfo;
use crate::combat::AttackTarget;
use crate::damage::{DamageType, DeathType, Relationship, HUGE_DAMAGE_AMOUNT};
use crate::logic::GameLogic;
use crate::math::{ceil, cos, sin, sqrt, Coord3D, PI, TWO_PI};
use crate::module::{UPDATE_SLEEP_FOREVER, UPDATE_SLEEP_NONE};
use crate::object::ObjectId;
use crate::orders::GuardTarget;
use crate::template::{kind_of, Geometry, GeometryShape};
use crate::weapon::WeaponTemplate;

/// `RebuildHoleBehaviorModuleData` (sim units).
#[derive(Clone, Debug, PartialEq)]
pub struct RebuildHoleTemplate {
    pub worker_object_name: Option<String>,
    /// `WorkerRespawnDelay` in frames.
    pub worker_respawn_delay: u32,
    pub hole_health_regen_percent_per_second: f32,
}

/// `RebuildHoleExposeDieModuleData` (carried in the die module, not the template).
#[derive(Clone, Debug, PartialEq)]
pub struct RebuildHoleExposeTemplate {
    pub hole_name: Option<String>,
    pub hole_max_health: f32,
    pub transfer_attackers: bool,
}

/// `DemoTrapUpdateModuleData` (sim units).
#[derive(Clone, Debug)]
pub struct DemoTrapTemplate {
    pub detonation_weapon: Option<Arc<WeaponTemplate>>,
    pub trigger_detonation_range: f32,
    pub scan_frames: u32,
    pub ignore_kind_of: u64,
    pub friendly_detonation: bool,
    pub detonate_when_killed: bool,
    pub default_proximity_mode: bool,
}

/// `CleanupHazardUpdateModuleData` (sim units).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CleanupHazardTemplate {
    pub scan_frames: u32,
    pub scan_range: f32,
}

/// `CommandButtonHuntUpdateModuleData` (sim units).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CommandButtonHuntTemplate {
    pub scan_frames: u32,
    pub scan_range: f32,
}

/// The kind of command-button hunt (a reduction of the `CommandButton` command type).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HuntCommand {
    /// `GUI_COMMAND_SWITCH_WEAPON` / `GUI_COMMAND_FIRE_WEAPON`: idle units hunt.
    Weapon,
    /// `GUI_COMMAND_SPECIAL_POWER`: scan for a target and use the special power on it. The
    /// command-button -> power lookup and `ActionManager::canDoSpecialPowerAtObject` are not
    /// ported, so the caller supplies the resolved power name and an object-type filter.
    SpecialPower { power: String, object_type: String },
    /// `GUICOMMANDMODE_HIJACK_VEHICLE` / `CONVERT_TO_CARBOMB` / `SABOTAGE_BUILDING`: scan for a
    /// target the enter ability accepts and issue the enter command. The command-button ->
    /// `GUICOMMANDMODE_*` lookup is not ported, so `set_hunt_command` supplies the kind; the
    /// C++ uses the same target filter as [`crate::enter_actions`].
    Enter { kind: crate::enter_actions::EnterKind, object_type: String },
}

/// `GenerateMinefieldBehaviorModuleData` (sim units).
#[derive(Clone, Debug, PartialEq)]
pub struct GenerateMinefieldTemplate {
    pub mine_name: Option<String>,
    pub upgraded_mine_name: Option<String>,
    pub upgraded_trigger: Option<String>,
    pub distance_around_object: f32,
    pub mines_per_square_foot: f32,
    pub generate_only_on_death: bool,
    pub border_only: bool,
    pub smart_border: bool,
    pub smart_border_skip_interior: bool,
    pub always_circular: bool,
    pub upgradable: bool,
    pub random_jitter: f32,
    /// `SkipIfThisMuchUnderStructure` (fraction): a mine is skipped when a structure overlaps the
    /// central `skip` fraction of its footprint. Retail default 0.33; no shipped INI overrides it.
    pub skip_if_this_much_under_structure: f32,
    pub starts_active: bool,
    pub triggered_by: Vec<String>,
    pub conflicts_with: Vec<String>,
}

impl GameLogic {
    // ---- RebuildHoleBehavior ----------------------------------------------------------------

    /// Whether `id` is a GLA rebuild hole that is reconstructing its building.
    pub fn is_rebuild_hole(&self, id: ObjectId) -> bool {
        self.objects.get(&id).is_some_and(|o| o.template.rebuild_hole.is_some())
    }

    /// `RebuildHoleBehavior::startRebuildProcess`: record what to rebuild and start the worker
    /// respawn timer (killing any existing worker).
    pub fn start_rebuild_process(&mut self, hole: ObjectId, rebuild_template: String, spawner: ObjectId) {
        let Some(delay) = self.objects.get(&hole).and_then(|o| o.template.rebuild_hole.as_ref()).map(|t| t.worker_respawn_delay) else {
            return;
        };
        if let Some(worker) = self.objects.get(&hole).and_then(|o| o.behavior.hole_worker) {
            self.destroy_object(worker);
        }
        if let Some(o) = self.objects.get_mut(&hole) {
            o.behavior.hole_worker = None;
            o.behavior.hole_rebuild_template = Some(rebuild_template);
            o.behavior.hole_spawner = Some(spawner);
            o.behavior.hole_worker_wait = delay;
        }
    }

    /// `RebuildHoleBehavior::newWorkerRespawnProcess`: kill the worker and restart the timer.
    fn new_worker_respawn_process(&mut self, hole: ObjectId, existing_worker: Option<ObjectId>) {
        if let Some(worker) = existing_worker {
            self.destroy_object(worker);
        }
        let delay = self.objects.get(&hole).and_then(|o| o.template.rebuild_hole.as_ref()).map_or(0, |t| t.worker_respawn_delay);
        if let Some(o) = self.objects.get_mut(&hole) {
            o.behavior.hole_worker = None;
            o.behavior.hole_worker_wait = delay;
        }
    }

    /// `RebuildHoleBehavior::update`: respawn the worker, restart lost reconstructions and heal the
    /// hole; when the building is complete, remove the hole and worker.
    fn update_rebuild_hole(&mut self, id: ObjectId, now: u32) {
        let Some(t) = self.objects.get(&id).and_then(|o| o.template.rebuild_hole.clone()) else { return };
        // A hole that was never told what to rebuild (no `RebuildHoleExposeDie` fired) is inert.
        if self.objects.get(&id).and_then(|o| o.behavior.hole_rebuild_template.clone()).is_none() {
            return;
        }
        if self.objects.get(&id).is_some_and(|o| o.is_dying()) {
            return;
        }
        let mut worker = self.objects.get(&id).and_then(|o| o.behavior.hole_worker).filter(|w| self.find_object(*w).is_some());
        if self.objects.get(&id).and_then(|o| o.behavior.hole_worker).is_some() && worker.is_none() {
            self.new_worker_respawn_process(id, None);
        }
        let mut reconstructing = self.objects.get(&id).and_then(|o| o.behavior.hole_reconstructing).filter(|r| self.find_object(*r).is_some());
        if self.objects.get(&id).and_then(|o| o.behavior.hole_reconstructing).is_some() && reconstructing.is_none() {
            self.new_worker_respawn_process(id, worker);
            worker = None;
            if let Some(o) = self.objects.get_mut(&id) {
                o.behavior.hole_reconstructing = None;
            }
        }

        if worker.is_none() {
            let wait = self.objects.get(&id).map_or(0, |o| o.behavior.hole_worker_wait);
            if wait > 0 {
                if let Some(o) = self.objects.get_mut(&id) {
                    o.behavior.hole_worker_wait = wait - 1;
                }
                if wait - 1 == 0 {
                    self.spawn_rebuild_worker(id, reconstructing, &t, now);
                    worker = self.objects.get(&id).and_then(|o| o.behavior.hole_worker);
                    reconstructing = self.objects.get(&id).and_then(|o| o.behavior.hole_reconstructing);
                }
            }
        }

        // Holes auto-heal (`HoleHealthRegen%PerSecond` of their max health).
        if t.hole_health_regen_percent_per_second > 0.0 {
            let (max, cur) = match self.objects.get(&id).and_then(|o| o.body.as_ref()) {
                Some(b) => (b.max_health(), b.health()),
                None => (0.0, 0.0),
            };
            if cur < max {
                self.heal_object(id, max * t.hole_health_regen_percent_per_second / 30.0);
            }
        }

        // When reconstruction completes, the hole and its worker go away.
        if reconstructing.is_some_and(|r| self.find_object(r).is_some_and(|o| !o.under_construction)) {
            if let Some(w) = worker {
                self.destroy_object(w);
            }
            self.destroy_object(id);
        }
    }

    /// Spawn the worker and (starting or resuming) the reconstruction.
    fn spawn_rebuild_worker(&mut self, hole: ObjectId, reconstructing: Option<ObjectId>, t: &RebuildHoleTemplate, _now: u32) {
        let Some(name) = t.worker_object_name.clone() else { return };
        let Some(tt) = self.find_template(&name) else { return };
        let (pos, angle, player) = match self.objects.get(&hole) {
            Some(o) => (o.position, o.angle, o.player),
            None => return,
        };
        let rebuild = self.objects.get(&hole).and_then(|o| o.behavior.hole_rebuild_template.clone());
        let Some(rebuild) = rebuild else { return };
        let worker = self.create_object(&tt, pos, angle, player);
        let building = match reconstructing {
            Some(r) => {
                // `aiResumeConstruction`: re-issue the build task on the same site.
                r
            }
            None => match self.construct_rebuild(worker, &rebuild, pos, angle, player) {
                Some(b) => b,
                None => {
                    self.destroy_object(worker);
                    return;
                }
            },
        };
        if let Some(o) = self.objects.get_mut(&worker) {
            o.build_target = Some(building);
        }
        // Transfer attackers from the hole to the replacement building.
        self.transfer_attackers(hole, building);
        if let Some(o) = self.objects.get_mut(&hole) {
            o.behavior.hole_worker = Some(worker);
            o.behavior.hole_reconstructing = Some(building);
        }
    }

    /// `DozerAIUpdate::construct` / `WorkerAIUpdate::construct` with `isRebuild = TRUE`: create the
    /// building under construction at 1 hp, no cost and no location checks.
    pub(crate) fn construct_rebuild(&mut self, worker: ObjectId, template_name: &str, pos: Coord3D, angle: f32, player: crate::object::PlayerIndex) -> Option<ObjectId> {
        let t = self.find_template(template_name)?;
        let site = Coord3D::new(pos.x, pos.y, self.terrain.ground_height(pos.x, pos.y));
        let id = self.create_object_under_construction(&t, site, angle, player);
        if let Some(o) = self.objects.get_mut(&id) {
            o.construction_percent = 0.0;
            o.builder = Some(worker);
        }
        if let Some(body) = self.objects.get_mut(&id).and_then(|o| o.body.as_mut()) {
            let max = body.max_health();
            let _ = body.attempt_damage(&DamageInfo { source: ObjectId::INVALID, amount: max - 1.0, kind: DamageType::Unresistable });
        }
        let _ = worker;
        Some(id)
    }

    /// `RebuildHoleBehavior::onDie`: destroy the worker, then the hole.
    pub(crate) fn rebuild_hole_on_die(&mut self, id: ObjectId) {
        if let Some(worker) = self.objects.get(&id).and_then(|o| o.behavior.hole_worker) {
            self.destroy_object(worker);
        }
        if let Some(o) = self.objects.get_mut(&id) {
            o.behavior.hole_worker = None;
        }
        self.destroy_object(id);
    }

    /// `Object::transferAttack`: retarget every object attacking `from` at `to`.
    pub(crate) fn transfer_attackers(&mut self, from: ObjectId, to: ObjectId) {
        for o in self.objects.values_mut() {
            if let Some(a) = &mut o.combat.attack {
                if let AttackTarget::Object(id) = a.target {
                    if id == from {
                        a.target = AttackTarget::Object(to);
                    }
                }
            }
            if let Some(GuardTarget::Object(id)) = &mut o.combat.guard {
                if *id == from {
                    *id = to;
                }
            }
        }
    }

    /// `RebuildHoleExposeDie::onDie`: spawn the hole, set its health and start the rebuild.
    pub(crate) fn rebuild_hole_expose_die(&mut self, id: ObjectId, expose: &RebuildHoleExposeTemplate) {
        let Some(hole_name) = expose.hole_name.clone() else { return };
        let Some(hole_t) = self.find_template(&hole_name) else { return };
        let (pos, angle, player, structure_name, under_construction) = match self.objects.get(&id) {
            Some(o) => (o.position, o.angle, o.player, o.template.name.clone(), o.under_construction),
            None => return,
        };
        // `RebuildHoleExposeDie.cpp:127-129`: do not leave a hole when the structure is still being
        // constructed (its first build or a hole reconstruction), when it belongs to the neutral
        // player, or when its player is inactive. A hole reconstruction creates an under-construction
        // replacement at 1 hp; without the under-construction guard, killing that replacement leaves
        // another hole (and the old hole re-queues its worker), so holes multiply every rebuild —
        // the `GLAHole*` structure runaway.
        if under_construction || self.neutral_player() == Some(player) || !self.is_player_active(player) {
            return;
        }
        let hole = self.create_object(&hole_t, pos, angle, player);
        if expose.hole_max_health > 0.0 {
            if let Some(body) = self.objects.get_mut(&hole).and_then(|o| o.body.as_mut()) {
                let add = expose.hole_max_health - body.max_health();
                body.change_max_health(add, crate::body::MaxHealthChangeType::SameCurrentHealth);
            }
        }
        self.start_rebuild_process(hole, structure_name, id);
        if expose.transfer_attackers {
            self.transfer_attackers(id, hole);
        }
    }

    // ---- CountermeasuresBehavior ------------------------------------------------------------

    /// `Object::reportMissileForCountermeasures` reduced: roll `EvasionRate` and, if the missile is
    /// diverted, return the decoy delay (frames) and start the reaction timer.
    pub(crate) fn countermeasure_decoy_report(&mut self, victim: ObjectId) -> Option<u32> {
        let cm = self.objects.get(&victim)?.template.countermeasures.clone()?;
        if !self.behavior_upgrade_active(victim, cm.starts_active, &cm.triggered_by, &cm.conflicts_with) {
            return None;
        }
        let o = self.objects.get(&victim)?;
        let (available, active, reaction) = (o.behavior.cm_available, o.behavior.cm_active, o.behavior.cm_reaction_frame);
        if available + active == 0 {
            return None;
        }
        if self.rng.value_real(0.0, 1.0) < cm.evasion_rate {
            if active == 0 && reaction == 0 {
                let now = self.frame;
                if let Some(o) = self.objects.get_mut(&victim) {
                    o.behavior.cm_reaction_frame = now + cm.countermeasure_reaction_frames;
                }
            }
            Some(cm.missile_decoy_delay)
        } else {
            None
        }
    }

    /// `CountermeasuresBehavior::calculateCountermeasureToDivertTo`: nearest flare of the newest
    /// volley (the `TheSuperHackers` fix iterates the whole volley).
    pub(crate) fn calculate_countermeasure_to_divert_to(&mut self, victim: ObjectId) -> Option<ObjectId> {
        let cm = self.objects.get(&victim)?.template.countermeasures.clone()?;
        let flares = self.objects.get(&victim)?.behavior.cm_flares.clone();
        let pos = self.objects.get(&victim)?.position;
        let volley = cm.volley_size.max(1) as usize;
        let mut best: Option<ObjectId> = None;
        let mut best_d = f32::MAX;
        for &fid in flares.iter().rev().take(volley) {
            let Some(f) = self.find_object(fid) else { continue };
            let d = f.position.dist_2d(&pos);
            if d < best_d {
                best_d = d;
                best = Some(fid);
            }
        }
        best
    }

    /// `CountermeasuresBehavior::reloadCountermeasures` (also called from the airfield reload).
    pub fn reload_countermeasures(&mut self, id: ObjectId) {
        let Some(cm) = self.objects.get(&id).and_then(|o| o.template.countermeasures.clone()) else { return };
        if let Some(o) = self.objects.get_mut(&id) {
            o.behavior.cm_available = cm.number_of_volleys * cm.volley_size;
            o.behavior.cm_reload_frame = 0;
        }
    }

    /// `CountermeasuresBehavior::update`.
    fn update_countermeasures(&mut self, id: ObjectId, now: u32) {
        let Some(cm) = self.objects.get(&id).and_then(|o| o.template.countermeasures.clone()) else { return };
        if self.objects.get(&id).is_some_and(|o| o.is_dying()) {
            return;
        }
        if !self.behavior_upgrade_active(id, cm.starts_active, &cm.triggered_by, &cm.conflicts_with) {
            return;
        }
        // Validate the flare list, dropping expired entries.
        let flares = self.objects.get(&id).map_or(Vec::new(), |o| o.behavior.cm_flares.clone());
        let alive: Vec<ObjectId> = flares.iter().copied().filter(|f| self.find_object(*f).is_some()).collect();
        if alive.len() != flares.len() {
            if let Some(o) = self.objects.get_mut(&id) {
                o.behavior.cm_flares = alive;
            }
        }
        let active = self.objects.get(&id).map_or(0, |o| o.behavior.cm_flares.len() as u32);
        if let Some(o) = self.objects.get_mut(&id) {
            o.behavior.cm_active = active;
        }

        if self.airborne_target(id) {
            let available = self.objects.get(&id).map_or(0, |o| o.behavior.cm_available);
            if available > 0 {
                let reaction = self.objects.get(&id).map_or(0, |o| o.behavior.cm_reaction_frame);
                if reaction != 0 && reaction == now {
                    self.launch_countermeasure_volley(id, &cm);
                    if let Some(o) = self.objects.get_mut(&id) {
                        o.behavior.cm_next_volley_frame = now + cm.delay_between_volleys;
                        o.behavior.cm_reaction_frame = 0;
                    }
                }
                let next = self.objects.get(&id).map_or(0, |o| o.behavior.cm_next_volley_frame);
                if next == now {
                    self.launch_countermeasure_volley(id, &cm);
                    if let Some(o) = self.objects.get_mut(&id) {
                        o.behavior.cm_next_volley_frame = now + cm.delay_between_volleys;
                    }
                }
            }
        }

        // Auto-reload (`ReloadTime == 0` means the aircraft must land at an airfield instead).
        let available = self.objects.get(&id).map_or(0, |o| o.behavior.cm_available);
        if available == 0 && cm.reload_time > 0 {
            let reload = self.objects.get(&id).map_or(0, |o| o.behavior.cm_reload_frame);
            if reload != 0 {
                if reload <= now {
                    self.reload_countermeasures(id);
                }
            } else if let Some(o) = self.objects.get_mut(&id) {
                o.behavior.cm_reload_frame = now + cm.reload_time;
            }
        }
    }

    /// `CountermeasuresBehavior::launchVolley`.
    fn launch_countermeasure_volley(&mut self, id: ObjectId, cm: &crate::modules2::CountermeasuresTemplate) {
        let Some(flare_name) = cm.flare_template_name.clone() else { return };
        let Some(flare_t) = self.find_template(&flare_name) else { return };
        let (pos, angle, player, velocity) = match self.objects.get(&id) {
            Some(o) => (o.position, o.angle, o.player, o.physics.as_ref().map(|p| p.velocity)),
            None => return,
        };
        let volley = cm.volley_size.max(1) as f32;
        for i in 0..cm.volley_size {
            let ratio = if volley != 1.0 { i as f32 / (volley - 1.0) * 2.0 - 1.0 } else { 0.0 };
            let rot = ratio * cm.volley_arc_angle;
            // `getUnitDirectionVector3D` then `Rotate(angle)`.
            let (fx, fy) = (cos(angle), sin(angle));
            let (rx, ry) = (fx * cos(rot) - fy * sin(rot), fx * sin(rot) + fy * cos(rot));
            let mut speed = velocity.map_or(0.0, |v| sqrt(v.x * v.x + v.y * v.y + v.z * v.z));
            if speed < 1.0 {
                speed = -10.0;
            }
            let force = Coord3D::new(rx * speed * cm.volley_velocity_factor, ry * speed * cm.volley_velocity_factor, 0.0);
            let fid = self.create_object(&flare_t, pos, angle, player);
            if let Some(o) = self.objects.get_mut(&fid) {
                o.set_position(pos);
                if let (Some(fp), Some(v)) = (o.physics.as_mut(), velocity) {
                    fp.velocity = v;
                }
            }
            self.apply_motive_force(fid, force);
            if let Some(o) = self.objects.get_mut(&id) {
                o.behavior.cm_flares.push(fid);
                o.behavior.cm_active = o.behavior.cm_active.saturating_add(1);
                o.behavior.cm_available = o.behavior.cm_available.saturating_sub(1);
            }
        }
    }

    /// `Object::isAirborneTarget` (`Object.h:211`, `CountermeasuresBehavior.cpp:240`): the `OBJECT_STATUS_AIRBORNE_TARGET`
    /// bit that `doLocomotor` maintains (`AIUpdate.cpp:2294-2300`).
    fn airborne_target(&self, id: ObjectId) -> bool {
        self.objects.get(&id).is_some_and(|o| o.status_bits & crate::object::status::AIRBORNE_TARGET != 0)
    }

    // ---- DemoTrapUpdate ---------------------------------------------------------------------

    /// `DemoTrapUpdate::detonate`: fire the detonation weapon (if any) then kill the trap.
    fn detonate_demo_trap(&mut self, id: ObjectId) {
        if self.objects.get(&id).is_none_or(|o| o.behavior.trap_detonated) {
            return;
        }
        if let Some(w) = self.objects.get(&id).and_then(|o| o.template.demo_trap.as_ref()).and_then(|t| t.detonation_weapon.clone()) {
            self.fire_temp_weapon(id, w);
        }
        if let Some(o) = self.objects.get_mut(&id) {
            o.behavior.trap_detonated = true;
        }
        self.kill_object(id);
    }

    /// `DemoTrapUpdate::update` (DemoTrapUpdate.cpp:131-...), from its heap slot: proximity mode or the
    /// detonate-when-killed path. Always returns `UPDATE_SLEEP_NONE`.
    pub(crate) fn demo_trap_update(&mut self, id: ObjectId) {
        let Some(t) = self.objects.get(&id).and_then(|o| o.template.demo_trap.clone()) else { return };
        if self.objects.get(&id).is_some_and(|o| o.behavior.trap_detonated) {
            return;
        }
        if self.objects.get(&id).is_some_and(|o| o.under_construction) {
            return;
        }
        if self.objects.get(&id).is_some_and(|o| o.is_dying()) {
            if t.detonate_when_killed {
                self.detonate_demo_trap(id);
            }
            return;
        }
        if !self.objects.get(&id).is_some_and(|o| o.behavior.trap_proximity) {
            return;
        }
        let next = self.objects.get(&id).map_or(0, |o| o.behavior.trap_next_scan);
        if next > 0 {
            if let Some(o) = self.objects.get_mut(&id) {
                o.behavior.trap_next_scan = next - 1;
            }
            return;
        }
        if let Some(o) = self.objects.get_mut(&id) {
            o.behavior.trap_next_scan = t.scan_frames;
        }
        let (pos, player) = match self.objects.get(&id) {
            Some(o) => (o.position, o.player),
            None => return,
        };
        let mut detonate = false;
        for cid in self.partition.objects_in_range(&pos, t.trigger_detonation_range, crate::partition::DistanceMode::FromBoundingSphere2D) {
            if cid == id {
                continue;
            }
            let Some(v) = self.objects.get(&cid) else { continue };
            if v.destroyed || v.is_dying() {
                continue;
            }
            if v.template.kind_of & t.ignore_kind_of != 0 {
                continue;
            }
            if self.relationship(player, v.player) != crate::damage::Relationship::Enemies {
                if !t.friendly_detonation {
                    return; // a friend is nearby, don't proximity-detonate
                }
                continue;
            }
            // Never detonate on anything airborne.
            if v.position.z - self.terrain.ground_height(v.position.x, v.position.y) > 9.0 {
                continue;
            }
            if v.position.dist_2d(&pos) <= t.trigger_detonation_range {
                detonate = true;
                if t.friendly_detonation {
                    break;
                }
            }
        }
        if detonate {
            self.detonate_demo_trap(id);
        }
    }

    /// `Object::kill` reduced: force the die pipeline (a bodyless projectile such as the neutron
    /// warhead has no `ActiveBody` to damage, so `on_die` runs directly).
    ///
    /// `Object::kill` (`Object.cpp:2010-2024`) builds `m_sourceID = INVALID_ID` and `m_kill = TRUE` with the body's max
    /// health as the amount; the port passes the max health directly (`DamageInfo` has no kill flag) and has no source,
    /// so no kill is credited to anyone.
    pub(crate) fn kill_object(&mut self, id: ObjectId) {
        if self.objects.get(&id).is_none_or(|o| o.destroyed || o.effectively_dead) {
            return;
        }
        let has_body = self.objects.get(&id).is_some_and(|o| o.body.is_some());
        if has_body {
            let _ = self.damage_object_with_death(
                id,
                DamageInfo { source: ObjectId::INVALID, amount: HUGE_DAMAGE_AMOUNT, kind: DamageType::Unresistable },
                DeathType::Normal,
            );
        }
        if self.objects.get(&id).is_some_and(|o| !o.destroyed && !o.effectively_dead && !o.is_dying()) {
            self.on_die(id, DeathType::Normal, 0.0, ObjectId::INVALID);
        }
    }

    // ---- CleanupHazardUpdate ----------------------------------------------------------------

    /// `CleanupHazardUpdate::setCleanupAreaParameters`: clean an area until empty.
    pub fn set_cleanup_area(&mut self, id: ObjectId, pos: crate::math::Coord3D, range: f32) {
        if self.objects.get(&id).is_none_or(|o| o.template.cleanup_hazard.is_none()) {
            return;
        }
        if let Some(o) = self.objects.get_mut(&id) {
            // `m_moveRange = range; m_pos = *pos` (CleanupHazardUpdate.cpp:300-301). Only a positive range is an area:
            // every `m_moveRange` test in the update is `> 0` or truthy (:133, :168, :271). A zero range still moves.
            o.behavior.cleanup_area = (range > 0.0).then_some((pos, range));
            o.behavior.cleanup_target = None;
            o.behavior.cleanup_next_scan = 0;
        }
        // `ai->aiMoveToPosition(pos, CMD_FROM_AI)` (setCleanupAreaParameters, CleanupHazardUpdate.cpp:308).
        self.ai_move_to_position_cmd(id, pos, crate::ai_states::CommandSource::Ai);
    }

    /// `CleanupHazardUpdate::update` (`CleanupHazardUpdate.cpp:126-...`), from its heap slot: the scan and the
    /// attack order. Always returns `UPDATE_SLEEP_NONE`.
    pub(crate) fn cleanup_hazard_update(&mut self, id: ObjectId) {
        let Some(t) = self.objects.get(&id).and_then(|o| o.template.cleanup_hazard) else { return };
        // `if (m_moveRange > 0.0f)` (CleanupHazardUpdate.cpp:133-149): an area cleanup keeps its unit busy while it is
        // idle, so scripts do not advance on it, and a player's or a script's new order abandons the area.
        if self.objects.get(&id).is_some_and(|o| o.behavior.cleanup_area.is_some()) {
            if self.is_ai_idle(id) {
                // `ai->aiBusy(CMD_FROM_AI)` (:138).
                self.ai_busy(id, crate::ai_states::CommandSource::Ai);
            } else if self.objects.get(&id).is_some_and(|o| o.ai.machine.last_command_source != crate::ai_states::CommandSource::Ai) {
                // `m_moveRange = 0.0f; return UPDATE_SLEEP_NONE;` (:142-146).
                if let Some(o) = self.objects.get_mut(&id) {
                    o.behavior.cleanup_area = None;
                }
                return;
            }
        }
        let next = self.objects.get(&id).map_or(0, |o| o.behavior.cleanup_next_scan);
        if next > 0 {
            if let Some(o) = self.objects.get_mut(&id) {
                o.behavior.cleanup_next_scan = next - 1;
            }
            self.cleanup_fire_when_ready(id);
            return;
        }
        if let Some(o) = self.objects.get_mut(&id) {
            o.behavior.cleanup_next_scan = t.scan_frames;
        }
        if self.cleanup_scan_closest_target(id, t) {
            self.cleanup_fire_when_ready(id);
        } else if let Some((area, _)) = self.objects.get(&id).and_then(|o| o.behavior.cleanup_area) {
            // `else if (m_moveRange)` (CleanupHazardUpdate.cpp:168-185): nothing left near the area. An idle or busy unit
            // (`isIdle() || isBusy()`, :170-171) either ends the area when it is within 25 of the area's centre
            // (:175-179), or moves back to the area (`aiMoveToPosition(&m_pos, CMD_FROM_AI)`, :184).
            let busy = self.objects.get(&id).is_some_and(|o| o.ai.machine.current == crate::ai_states::AiStateType::Busy);
            if self.is_ai_idle(id) || busy {
                let pos = self.objects.get(&id).map_or(area, |o| o.position);
                if pos.dist_2d(&area) < 25.0 {
                    if let Some(o) = self.objects.get_mut(&id) {
                        o.behavior.cleanup_area = None;
                    }
                } else {
                    self.ai_move_to_position_cmd(id, area, crate::ai_states::CommandSource::Ai);
                }
            }
        }
    }

    /// `CleanupHazardUpdate::scanClosestTarget`: the closest `KINDOF_CLEANUP_HAZARD` object.
    fn cleanup_scan_closest_target(&mut self, id: ObjectId, t: CleanupHazardTemplate) -> bool {
        let (pos, player, area) = match self.objects.get(&id) {
            Some(o) => (o.position, o.player, o.behavior.cleanup_area),
            None => return false,
        };
        let (center, range) = match area {
            Some((area, move_range)) => (area, t.scan_range + move_range),
            None => (pos, t.scan_range),
        };
        let target = self
            .objects
            .values()
            .filter(|o| o.id != id && !o.destroyed && !o.is_dying())
            .filter(|o| o.is_kind_of(kind_of::CLEANUP_HAZARD))
            .filter(|o| o.position.dist_2d(&center) <= range)
            .min_by(|a, b| a.position.dist_2d(&center).total_cmp(&b.position.dist_2d(&center)))
            .map(|o| o.id);
        let _ = player;
        match target {
            Some(t) => {
                if let Some(o) = self.objects.get_mut(&id) {
                    o.behavior.cleanup_target = Some(t);
                }
                true
            }
            None => {
                if let Some(o) = self.objects.get_mut(&id) {
                    o.behavior.cleanup_target = None;
                }
                false
            }
        }
    }

    /// `CleanupHazardUpdate::fireWhenReady` (`CleanupHazardUpdate.cpp:233-258`): when the AI is idle or busy, lock
    /// the cleanup weapon and order the attack. The `aiAttackObject` call is a state change, which wakes the AI
    /// (`AIUpdate.cpp:616`, `:973-985`); the port's attack-order adoption runs in the AI update, so it must be
    /// woken here.
    fn cleanup_fire_when_ready(&mut self, id: ObjectId) {
        let Some(target) = self.objects.get(&id).and_then(|o| o.behavior.cleanup_target).filter(|t| self.find_object(*t).is_some()) else {
            return;
        };
        let can_order = self.objects.get(&id).is_some_and(|o| {
            matches!(o.ai.machine.current, crate::ai_states::AiStateType::Idle | crate::ai_states::AiStateType::Busy)
        });
        if !can_order {
            return;
        }
        if let Some(o) = self.objects.get_mut(&id) {
            o.combat.attack = Some(crate::combat::AttackOrder::new(AttackTarget::Object(target)));
        }
        self.wake_up_now(id);
    }

    // ---- CommandButtonHuntUpdate ------------------------------------------------------------

    /// `CommandButtonHuntUpdate::setCommandButton` (`CommandButtonHuntUpdate.cpp:102-138`): store the hunt command,
    /// idle the AI, run the update once immediately (its return is discarded, the engine calls `update()` directly
    /// at `:136`) and wake the module for the next frame (`setWakeFrame(obj, UPDATE_SLEEP_NONE)`, `:137`).
    /// The command-button table is not ported, so scripts call this directly (with the resolved special-power name /
    /// object-type filter); a command the object's template has no `CommandButtonHuntUpdate` for returns `false`.
    pub fn set_hunt_command(&mut self, id: ObjectId, command: HuntCommand) -> bool {
        if self.objects.get(&id).is_none_or(|o| o.template.command_button_hunt.is_none()) {
            return false;
        }
        if let Some(o) = self.objects.get_mut(&id) {
            o.behavior.hunt_command = Some(command);
        }
        self.ai_idle(id, crate::ai_states::CommandSource::Ai);
        let now = self.frame;
        let _ = self.command_button_hunt_update(id, now);
        self.wake_command_button_hunt(id);
        true
    }

    /// `CommandButtonHuntUpdate::update` (`CommandButtonHuntUpdate.cpp:143-174`), the module's heap slot: the hunt
    /// command's kind decides the work and the return is the module's next wake frame. A unit whose last command came
    /// from a script or the player quits hunting (`:151-157`); a weapon hunt keeps the module awake (`:190`), the two
    /// scanning hunts sleep `ScanRate` (`:202`, `:219`, `:231`, `:240`) and a scan with nothing to hunt with sleeps
    /// forever (`:149`, `:208`).
    pub(crate) fn command_button_hunt_update(&mut self, id: ObjectId, _now: u32) -> u32 {
        let Some(t) = self.objects.get(&id).and_then(|o| o.template.command_button_hunt) else {
            return crate::module::UPDATE_SLEEP_FOREVER;
        };
        let Some(command) = self.objects.get(&id).and_then(|o| o.behavior.hunt_command.clone()) else {
            return crate::module::UPDATE_SLEEP_FOREVER;
        };
        // "If a script or the player (in this case should only be script, but either way) we quit hunting."
        let last = self.objects.get(&id).map(|o| o.ai.machine.last_command_source);
        if last.is_some_and(|s| s != crate::ai_states::CommandSource::Ai) {
            if let Some(o) = self.objects.get_mut(&id) {
                o.behavior.hunt_command = None;
            }
            return crate::module::UPDATE_SLEEP_FOREVER;
        }
        let idle = self.objects.get(&id).is_some_and(|o| o.ai.machine.current == crate::ai_states::AiStateType::Idle);
        let scan_sleep = t.scan_frames.max(1);
        match command {
            HuntCommand::Weapon => {
                // `huntWeapon` (`:179-191`): an idle mobile unit hunts (`ai->aiHunt(CMD_FROM_AI)` -> `privateHunt`,
                // `AIUpdate.cpp:3648-3665`: clear the machine, record the source, enter `AI_HUNT`); the C++ also locks
                // the button's weapon slot `LOCKED_TEMPORARILY`, which the command-button table (not ported) carries.
                if idle && self.is_mobile(id) {
                    if let Some(o) = self.objects.get_mut(&id) {
                        o.combat.hunt = true;
                    }
                    self.ai_clear(id);
                    if let Some(o) = self.objects.get_mut(&id) {
                        o.ai.machine.last_command_source = crate::ai_states::CommandSource::Ai;
                    }
                    self.ai_set_state(id, crate::ai_states::AiStateType::Hunt);
                }
                crate::module::UPDATE_SLEEP_NONE
            }
            HuntCommand::SpecialPower { power, object_type } => {
                // `huntSpecialPower` (`:196-220`): not idle -> sleep `ScanRate`; no `SpecialAbilityUpdate` for the
                // power -> sleep forever; the ability already active -> sleep `ScanRate`; else scan and use it.
                if !idle {
                    return scan_sleep;
                }
                if !self.has_special_ability_update(id, &power) {
                    return crate::module::UPDATE_SLEEP_FOREVER;
                }
                if self.objects.get(&id).is_some_and(|o| o.ability.as_ref().is_some_and(|a| a.power == power)) {
                    return scan_sleep;
                }
                // A capture power scans for `canCaptureBuilding` targets (neutral capturable
                // structures included); other powers keep the enemy scan (`isValidToUseOn`, not ported).
                let capture = self
                    .special_powers
                    .get(&power)
                    .is_some_and(|t| crate::special_power::is_capture_power_kind(&t.kind));
                let scanned = if capture {
                    self.hunt_scan_capture(id, t.scan_range)
                } else {
                    self.hunt_scan_target(id, &object_type, t.scan_range)
                };
                if let Some(target) = scanned {
                    let player = self.objects.get(&id).map_or(0, |o| o.player);
                    // `doCommandButtonAtObject` -> `doSpecialPowerAtObject` (the `RadiusDecalUpdate`
                    // is client-only).
                    self.command_do_special_power(player, id, &power, crate::special_power::SpecialPowerTarget::Object(target));
                }
                scan_sleep
            }
            HuntCommand::Enter { kind, object_type } => {
                // `huntEnter` (`:225-241`): sleep `ScanRate` while not idle, then scan and issue the enter command
                // with `CMD_FROM_AI`.
                if !idle {
                    return scan_sleep;
                }
                if let Some(target) = self.hunt_scan_enter(id, kind, &object_type, t.scan_range) {
                    let player = self.objects.get(&id).map_or(0, |o| o.player);
                    // `doCommandButtonAtObject` -> `createEnterMessage` -> `MSG_ENTER`, `CMD_FROM_AI`.
                    self.command_enter_from(player, &[id], target, crate::ai_states::CommandSource::Ai);
                }
                scan_sleep
            }
        }
    }

    /// `Object::findSpecialAbilityUpdate` (`Object.cpp:6082`) is non-null: the object carries a
    /// `SpecialAbilityUpdate` module (not just the `SpecialAbility` power module) for `power`.
    fn has_special_ability_update(&self, id: ObjectId, power: &str) -> bool {
        self.objects
            .get(&id)
            .is_some_and(|o| o.template.special_abilities.iter().any(|a| !a.power_module && a.power == power))
    }

    /// `CommandButtonHuntUpdate::scanClosestTarget`'s enter branch
    /// (`CommandButtonHuntUpdate.cpp:366`): the nearest object the enter ability accepts. Hijack /
    /// sabotage consider enemies, convert-to-carbomb only neutrals; the optional `object_type`
    /// stands in for the not-ported command-button target-type filter.
    fn hunt_scan_enter(
        &self,
        id: ObjectId,
        kind: crate::enter_actions::EnterKind,
        object_type: &str,
        scan_range: f32,
    ) -> Option<ObjectId> {
        let o = self.objects.get(&id)?;
        let (pos, player) = (o.position, o.player);
        let wants_neutral = matches!(kind, crate::enter_actions::EnterKind::ConvertToCarBomb);
        let mut best: Option<(f32, ObjectId)> = None;
        for v in self.objects.values() {
            if v.id == id || v.destroyed || v.is_dying() {
                continue;
            }
            let rel = self.relationship(player, v.player);
            let wanted = if wants_neutral { Relationship::Neutral } else { Relationship::Enemies };
            if rel != wanted {
                continue;
            }
            if !object_type.is_empty() && !v.template.name.eq_ignore_ascii_case(object_type) {
                continue;
            }
            let d = pos.dist_2d(&v.position);
            if d > scan_range {
                continue;
            }
            if !self.can_enter_kind(id, v.id, kind) {
                continue;
            }
            if best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, v.id));
            }
        }
        best.map(|(_, id)| id)
    }

    /// `CommandButtonHuntUpdate::scanClosestTarget` (the relationship/kind-of half; the
    /// `canDoSpecialPowerAtObject` targeting-type and attack-priority checks are not ported).
    fn hunt_scan_target(&self, id: ObjectId, object_type: &str, scan_range: f32) -> Option<ObjectId> {
        let o = self.objects.get(&id)?;
        let (pos, player) = (o.position, o.player);
        let mut best: Option<(f32, ObjectId)> = None;
        for v in self.objects.values() {
            if v.id == id || v.destroyed || v.is_dying() {
                continue;
            }
            // `PartitionFilterRelationship(ALLOW_ENEMIES)`: only enemies.
            if self.relationship(player, v.player) != crate::damage::Relationship::Enemies {
                continue;
            }
            if !object_type.is_empty() && !v.template.name.eq_ignore_ascii_case(object_type) {
                continue;
            }
            let d = pos.dist_2d(&v.position);
            if d > scan_range {
                continue;
            }
            if best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, v.id));
            }
        }
        best.map(|(_, id)| id)
    }

    /// `CommandButtonHuntUpdate::scanClosestTarget` for a capture power: the nearest object within
    /// `scan_range` that `ActionManager::canCaptureBuilding` accepts for this unit (`PartitionFilter-
    /// ValidCommandButtonTarget`, `CommandButtonHuntUpdate.cpp:366`).
    fn hunt_scan_capture(&self, id: ObjectId, scan_range: f32) -> Option<ObjectId> {
        let o = self.objects.get(&id)?;
        let pos = o.position;
        let mut best: Option<(f32, ObjectId)> = None;
        for v in self.objects.values() {
            if v.id == id || v.destroyed || v.is_dying() {
                continue;
            }
            let d = pos.dist_2d(&v.position);
            if d > scan_range || best.is_some_and(|(bd, _)| d >= bd) {
                continue;
            }
            if self.can_capture_building(id, v.id) {
                best = Some((d, v.id));
            }
        }
        best.map(|(_, id)| id)
    }

    // ---- GenerateMinefieldBehavior ----------------------------------------------------------

    /// `GenerateMinefieldBehavior::placeMines`: lay the border minefield.
    pub fn generate_minefield(&mut self, id: ObjectId) {
        let Some(t) = self.objects.get(&id).and_then(|o| o.template.generate_minefield.clone()) else { return };
        if t.generate_only_on_death {
            return;
        }
        self.place_mines(id, &t);
    }

    /// The activation of `GenerateMinefieldBehavior` (`upgradeImplementation` and `onDie` in C++; the port checks it
    /// once a frame): lay the field when it is active and not yet laid, unless it is `GenerateOnlyOnDeath`. The upgrade
    /// swap is the module's update, [`generate_minefield_update`](Self::generate_minefield_update).
    fn update_generate_minefield(&mut self, id: ObjectId, _now: u32) {
        let Some(t) = self.objects.get(&id).and_then(|o| o.template.generate_minefield.clone()) else { return };
        if t.generate_only_on_death {
            return;
        }
        let active = self.behavior_upgrade_active(id, t.starts_active, &t.triggered_by, &t.conflicts_with);
        let generated = self.objects.get(&id).is_some_and(|o| o.behavior.minefield_generated);
        if !generated && active {
            self.place_mines(id, &t);
        }
    }

    /// `GenerateMinefieldBehavior::update` (GenerateMinefieldBehavior.cpp:457-496), the module's sleepy update. While the
    /// field is upgradable and not yet upgraded it returns `UPDATE_SLEEP_NONE` (awake every frame, :492); otherwise
    /// `UPDATE_SLEEP_FOREVER` (:495). A laid field whose object has completed `Upgrade_ChinaEMPMines` (the object's own
    /// upgrade mask, :463-468) is swapped for the upgraded mines: the old mines are destroyed and `placeMines` runs again
    /// with `MineNameUpgraded` (:474-487).
    pub(crate) fn generate_minefield_update(&mut self, id: ObjectId) -> u32 {
        let Some(o) = self.objects.get(&id) else { return UPDATE_SLEEP_FOREVER };
        let Some(t) = o.template.generate_minefield.clone() else { return UPDATE_SLEEP_FOREVER };
        if o.behavior.minefield_upgraded || !t.upgradable {
            return UPDATE_SLEEP_FOREVER;
        }
        if o.behavior.minefield_generated && o.upgrades.contains("Upgrade_ChinaEMPMines") {
            let mines = o.behavior.minefield_mines.clone();
            for m in mines {
                self.destroy_object(m);
            }
            if let Some(o) = self.objects.get_mut(&id) {
                o.behavior.minefield_mines.clear();
                o.behavior.minefield_generated = false;
                o.behavior.minefield_upgraded = true;
            }
            self.place_mines(id, &t);
        }
        UPDATE_SLEEP_NONE
    }

    /// `GenerateMinefieldBehavior::placeMines`: pick the border or footprint layout from the
    /// object's real `GeometryInfo` (box vs cylinder), faithful to the original branch order.
    fn place_mines(&mut self, id: ObjectId, t: &GenerateMinefieldTemplate) {
        let (target, player, obj_geom, obj_angle) = match self.objects.get(&id) {
            Some(o) => (o.position, o.player, o.template.geometry, o.angle),
            None => return,
        };
        let upgraded = self.objects.get(&id).is_some_and(|o| o.behavior.minefield_upgraded);
        let name = if upgraded { t.upgraded_mine_name.clone() } else { t.mine_name.clone() };
        let Some(name) = name else { return };
        let Some(mine_t) = self.find_template(&name) else { return };
        let skip = t.skip_if_this_much_under_structure.clamp(0.0, 1.0);
        let mut placed: Vec<ObjectId> = Vec::new();

        if t.smart_border {
            let mut geom = obj_geom;
            if !t.smart_border_skip_interior {
                // Seed geometry becomes the mine's own footprint, then one mine on the target.
                geom = mine_t.geometry;
                if let Some(m) = self.place_mine_at(target, &mine_t, player, skip) {
                    placed.push(m);
                }
            }
            if t.always_circular {
                geom = cylinder_geometry(geom.bounding_circle_radius());
            }
            let mine_radius = mine_t.geometry.bounding_circle_radius();
            let mine_diameter = mine_radius * 2.0;
            geom.expand_footprint(mine_radius);
            loop {
                if geom.shape == GeometryShape::Box && !t.always_circular {
                    let (major, minor) = (geom.major_radius, geom.minor_radius);
                    self.place_mines_around_rect(target, major, minor, obj_angle, &mine_t, player, t, skip, &mut placed);
                } else {
                    self.place_mines_around_circle(target, geom.major_radius, &mine_t, player, t, skip, &mut placed);
                }
                geom.expand_footprint(mine_diameter);
                // `while (geom.getBoundingCircleRadius() < distanceAroundObject)`.
                if geom.bounding_circle_radius() >= t.distance_around_object {
                    break;
                }
            }
        } else if t.border_only {
            let mut geom = obj_geom;
            geom.expand_footprint(t.distance_around_object);
            if geom.shape == GeometryShape::Box && !t.always_circular {
                let (major, minor) = (geom.major_radius, geom.minor_radius);
                self.place_mines_around_rect(target, major, minor, obj_angle, &mine_t, player, t, skip, &mut placed);
            } else {
                self.place_mines_around_circle(target, geom.major_radius, &mine_t, player, t, skip, &mut placed);
            }
        } else {
            let mut geom = obj_geom;
            geom.expand_footprint(t.distance_around_object);
            if t.always_circular {
                geom = cylinder_geometry(geom.bounding_circle_radius());
            }
            self.place_mines_in_footprint(&geom, obj_geom, target, &mine_t, player, t, skip, &mut placed);
        }
        if let Some(o) = self.objects.get_mut(&id) {
            o.behavior.minefield_generated = true;
            o.behavior.minefield_mines = placed;
        }
    }

    /// `placeMinesAlongLine`: space mines along a line, skipping the first point at the start.
    #[allow(clippy::too_many_arguments)]
    fn place_mines_along_line(
        &mut self,
        start: Coord3D,
        end: Coord3D,
        mine_t: &Arc<crate::template::ObjectTemplate>,
        player: crate::object::PlayerIndex,
        t: &GenerateMinefieldTemplate,
        skip: f32,
        placed: &mut Vec<ObjectId>,
    ) {
        let (dx, dy) = (end.x - start.x, end.y - start.y);
        let len = sqrt(dx * dx + dy * dy);
        if len <= 0.0 {
            return;
        }
        let mine_radius = mine_t.geometry.bounding_circle_radius();
        let mine_diameter = (mine_radius * 2.0).max(0.001);
        let mine_jitter = mine_radius * t.random_jitter;
        let num = (ceil(len / mine_diameter) as i32).max(1);
        let inc = len / num as f32;
        let mut place = inc; // `skipOneAtStart` is always true for the rect's edges.
        while place <= len {
            let mut pt = Coord3D::new(start.x + place * dx / len, start.y + place * dy / len, 0.0);
            pt.z = self.terrain.ground_height(pt.x, pt.y);
            self.offset_by_small_random(&mut pt, mine_jitter);
            if let Some(m) = self.place_mine_at(pt, mine_t, player, skip) {
                placed.push(m);
            }
            place += inc;
        }
    }

    /// `placeMinesAroundRect`: four edges of the rotated `major x minor` footprint.
    #[allow(clippy::too_many_arguments)]
    fn place_mines_around_rect(
        &mut self,
        pos: Coord3D,
        major: f32,
        minor: f32,
        angle: f32,
        mine_t: &Arc<crate::template::ObjectTemplate>,
        player: crate::object::PlayerIndex,
        t: &GenerateMinefieldTemplate,
        skip: f32,
        placed: &mut Vec<ObjectId>,
    ) {
        // `makeCorner` transforms the local corner by the object's transform (translation + yaw).
        let corner = |mx: f32, my: f32| {
            let (c, s) = (cos(angle), sin(angle));
            Coord3D::new(pos.x + mx * c - my * s, pos.y + mx * s + my * c, 0.0)
        };
        let pt = [corner(major, minor), corner(-major, minor), corner(-major, -minor), corner(major, -minor)];
        self.place_mines_along_line(pt[0], pt[1], mine_t, player, t, skip, placed);
        self.place_mines_along_line(pt[1], pt[2], mine_t, player, t, skip, placed);
        self.place_mines_along_line(pt[2], pt[3], mine_t, player, t, skip, placed);
        self.place_mines_along_line(pt[3], pt[0], mine_t, player, t, skip, placed);
    }

    /// `placeMinesAroundCircle`: mines around a circle at `radius`.
    #[allow(clippy::too_many_arguments)]
    fn place_mines_around_circle(
        &mut self,
        pos: Coord3D,
        radius: f32,
        mine_t: &Arc<crate::template::ObjectTemplate>,
        player: crate::object::PlayerIndex,
        t: &GenerateMinefieldTemplate,
        skip: f32,
        placed: &mut Vec<ObjectId>,
    ) {
        let mine_radius = mine_t.geometry.bounding_circle_radius();
        let mine_diameter = (mine_radius * 2.0).max(0.001);
        let mine_jitter = mine_radius * t.random_jitter;
        let circum = TWO_PI * radius;
        let num = (ceil(circum / mine_diameter) as i32).max(1);
        let angle_inc = TWO_PI / num as f32;
        let angle_lim = TWO_PI - angle_inc * 0.5;
        let mut angle = 0.0f32;
        while angle < angle_lim {
            let mut pt = Coord3D::new(pos.x + radius * cos(angle), pos.y + radius * sin(angle), 0.0);
            pt.z = self.terrain.ground_height(pt.x, pt.y);
            self.offset_by_small_random(&mut pt, mine_jitter);
            if let Some(m) = self.place_mine_at(pt, mine_t, player, skip) {
                placed.push(m);
            }
            angle += angle_inc;
        }
    }

    /// `placeMinesInFootprint`: `MinesPerSquareFoot * area` random points inside the footprint,
    /// retrying to keep mines at least two radii apart and skipping the generator's own footprint.
    #[allow(clippy::too_many_arguments)]
    fn place_mines_in_footprint(
        &mut self,
        geom: &Geometry,
        obj_geom: Geometry,
        target: Coord3D,
        mine_t: &Arc<crate::template::ObjectTemplate>,
        player: crate::object::PlayerIndex,
        t: &GenerateMinefieldTemplate,
        skip: f32,
        placed: &mut Vec<ObjectId>,
    ) {
        let area = geom.footprint_area();
        let num = (ceil(t.mines_per_square_foot * area) as i32).max(1);
        let mine_radius = mine_t.geometry.bounding_circle_radius();
        let min_dist_sq = (mine_radius * 2.0) * (mine_radius * 2.0);
        let mut created: Vec<Coord3D> = Vec::new();
        for _ in 0..num {
            let mut retry = 100;
            let pt = loop {
                let mut candidate = self.random_offset_within_footprint(geom);
                candidate.x += target.x;
                candidate.y += target.y;
                candidate.z += target.z;
                retry -= 1;
                if !anything_too_close(&created, &candidate, min_dist_sq) || retry <= 0 {
                    break candidate;
                }
            };
            // `if (getObject()->getGeometryInfo().isPointInFootprint(*target, pt)) continue;`
            if obj_geom.is_point_in_footprint(&target, &pt) {
                continue;
            }
            if let Some(m) = self.place_mine_at(pt, mine_t, player, skip) {
                created.push(pt);
                placed.push(m);
            }
        }
    }

    /// `GeometryInfo::makeRandomOffsetWithinFootprint` (cylinder rejection sampling / box uniform).
    fn random_offset_within_footprint(&mut self, geom: &Geometry) -> Coord3D {
        match geom.shape {
            GeometryShape::Box => Coord3D::new(
                self.rng.value_real(-geom.major_radius, geom.major_radius),
                self.rng.value_real(-geom.minor_radius, geom.minor_radius),
                0.0,
            ),
            _ => {
                let max_sq = geom.major_radius * geom.major_radius;
                loop {
                    let x = self.rng.value_real(-geom.major_radius, geom.major_radius);
                    let y = self.rng.value_real(-geom.major_radius, geom.major_radius);
                    if x * x + y * y <= max_sq {
                        return Coord3D::new(x, y, 0.0);
                    }
                }
            }
        }
    }

    /// `offsetBySmallRandomAmount`: always two draws, even when `max_amt` is 0.
    fn offset_by_small_random(&mut self, pt: &mut Coord3D, max_amt: f32) {
        pt.x += self.rng.value_real(-max_amt, max_amt);
        pt.y += self.rng.value_real(-max_amt, max_amt);
    }

    /// `GenerateMinefieldBehavior::placeMineAt`: create one mine, skipping water/cliff/structures.
    fn place_mine_at(
        &mut self,
        pt: Coord3D,
        mine_t: &Arc<crate::template::ObjectTemplate>,
        player: crate::object::PlayerIndex,
        skip: f32,
    ) -> Option<ObjectId> {
        // The orientation is drawn before the layer/overlap checks, matching the engine's RNG order.
        let angle = self.rng.value_real(-PI, PI);
        if self.terrain.is_underwater(pt.x, pt.y) || self.terrain.is_cliff_cell(pt.x, pt.y) {
            return None;
        }
        let mine_radius = mine_t.geometry.bounding_circle_radius();
        // `geom.expandFootprint(-(1 - skip) * mineRadius)`: a structure overlapping the central
        // `skip` fraction of the mine's footprint blocks placement.
        let probe = mine_radius * skip;
        let blocked = self.objects.values().any(|o| {
            !o.destroyed && o.is_kind_of(kind_of::STRUCTURE) && o.position.dist_2d(&pt) < probe + o.template.geometry.bounding_circle_radius()
        });
        if blocked {
            return None;
        }
        let z = self.terrain.ground_height(pt.x, pt.y);
        Some(self.create_object(mine_t, Coord3D::new(pt.x, pt.y, z), angle, player))
    }

    /// `GenerateMinefieldBehavior::onDie`: lay the field when `GenerateOnlyOnDeath`.
    pub(crate) fn generate_minefield_on_die(&mut self, id: ObjectId) {
        let Some(t) = self.objects.get(&id).and_then(|o| o.template.generate_minefield.clone()) else { return };
        if t.generate_only_on_death {
            self.place_mines(id, &t);
        }
    }

    /// The round-4 periodic pass. `DemoTrapUpdate`, `CleanupHazardUpdate` and `CommandButtonHuntUpdate` are heap
    /// families (`sleepy_families.rs`) now, so they are not swept here.
    pub(crate) fn update_modules4(&mut self) {
        let now = self.frame;
        let ids: Vec<ObjectId> = self.objects.values().filter(|o| !o.destroyed).map(|o| o.id).collect();
        for id in ids {
            self.update_rebuild_hole(id, now);
            self.update_countermeasures(id, now);
            self.update_generate_minefield(id, now);
            self.update_height_die(id, now);
            self.update_neutron_missile(id, now);
            self.update_spy_vision(id, now);
            self.update_particle_uplink(id, now);
            self.update_spectre(id, now);
            self.update_missile_launcher(id, now);
        }
    }
}

/// `GeometryInfo::set(GEOMETRY_CYLINDER, false, 1, radius, radius)` (`AlwaysCircular`).
fn cylinder_geometry(radius: f32) -> Geometry {
    Geometry {
        shape: GeometryShape::Cylinder,
        major_radius: radius,
        minor_radius: radius,
        height: 1.0,
        is_small: false,
    }
}

/// `isAnythingTooClose2D`: any point whose squared 2D distance to `pos` is below `min_dist_sq`.
fn anything_too_close(points: &[Coord3D], pos: &Coord3D, min_dist_sq: f32) -> bool {
    points.iter().any(|p| {
        let (dx, dy) = (p.x - pos.x, p.y - pos.y);
        dx * dx + dy * dy < min_dist_sq
    })
}

/// Lane 173 (wave 20): `Object::kill`'s source is `INVALID_ID`, R19-8 (`modules4_l173.rs`).
#[cfg(test)]
#[path = "modules4_l173.rs"]
mod l173_tests;
