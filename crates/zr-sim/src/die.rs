//! Die modules (`GameLogic/Object/Die/*`, `GameLogic/Object/Behavior/SlowDeathBehavior.cpp`,
//! `Object::onDie`).
//!
//! `zr-sim` does not depend on the INI crate: the adapter converts `zr_templates::DieData` into
//! [`DieModuleTemplate`] values. On death, [`GameLogic::on_die`] walks the object's die modules in
//! definition order, filters each through its [`DieMux`] (`DieMuxData::isDieApplicable`) and runs it.
//!
//! Ported: `DestroyDie`, `InstantDeathBehavior` (FX/OCL/weapon pick by RNG, then destroy),
//! `CreateObjectDie` (run its OCL), `FXListDie` (recorded FX event), and `SlowDeathBehavior` (the
//! weighted selection among applicable SlowDeath modules, sink / midpoint / destruction delays with
//! variance, phase OCLs and weapons, then destroy). Fling physics, status/veterancy state and the
//! other die classes are not ported.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::crc::Crc;
use crate::damage::DeathType;
use crate::logic::GameLogic;
use crate::module::{UPDATE_SLEEP_FOREVER, UPDATE_SLEEP_NONE};
use crate::math::{cos, sin, PI};
use crate::object::ObjectId;
use crate::ocl::OclTemplate;
use crate::weapon::WeaponTemplate;

/// Lane 168 (the R18 review, R18-8): the sink keeps the transform in step (`die_l168.rs`).
#[cfg(test)]
#[path = "die_l168.rs"]
mod l168_tests;

/// `BEGIN_MIDPOINT_RATIO` / `END_MIDPOINT_RATIO` (`SlowDeathBehavior.cpp`).
const BEGIN_MIDPOINT_RATIO: f32 = 0.35;
const END_MIDPOINT_RATIO: f32 = 0.65;

/// Slow-death phases (`SlowDeathPhaseType`).
pub const PHASE_INITIAL: usize = 0;
pub const PHASE_MIDPOINT: usize = 1;
pub const PHASE_FINAL: usize = 2;

/// `DieMuxData`: which deaths a die module handles.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DieMux {
    /// `DeathTypes` bits, by `DeathType` index.
    pub death_types: u32,
    /// `VeterancyLevels` (engine flags; bit index is `level - 1`).
    pub veterancy_levels: u32,
    /// `ExemptStatus`: ignored if any of these status bits are set ([`crate::object::status`] bits).
    pub exempt_status: u64,
    /// `RequiredStatus`: ignored unless all of these status bits are set.
    pub required_status: u64,
}

impl DieMux {
    /// `DieMuxData::isDieApplicable`. `veterancy` is the object's level index (0 = `REGULAR`); the
    /// engine computes `1 << (level - 1)`, so `REGULAR` maps to bit 31. `status` is
    /// `Object::getStatusBits` ([`crate::object::Object::status_mask`]).
    pub fn is_applicable(&self, death: DeathType, veterancy: u8, status: u64) -> bool {
        if self.death_types & (1u32 << (death as u32)) == 0 {
            return false;
        }
        let bit = (veterancy as u32).wrapping_sub(1) & 31;
        if self.veterancy_levels & (1u32 << bit) == 0 {
            return false;
        }
        if status & self.exempt_status != 0 {
            return false;
        }
        if status & self.required_status != self.required_status {
            return false;
        }
        true
    }
}

/// `SlowDeathBehaviorModuleData` (without the embedded `DieMuxData`).
#[derive(Clone, Debug)]
pub struct SlowDeathTemplate {
    pub probability_modifier: i32,
    pub modifier_bonus_per_overkill_percent: f32,
    /// `SinkRate`, units per frame.
    pub sink_rate: f32,
    /// Frames.
    pub sink_delay: u32,
    pub sink_delay_variance: u32,
    pub destruction_delay: u32,
    pub destruction_delay_variance: u32,
    pub destruction_altitude: f32,
    /// FX/OCL/weapon names by phase (`INITIAL`, `MIDPOINT`, `FINAL`).
    pub fx: [Vec<String>; 3],
    pub ocl: [Vec<Arc<OclTemplate>>; 3],
    pub weapons: [Vec<Arc<WeaponTemplate>>; 3],
    pub fling_force: f32,
    pub fling_force_variance: f32,
    /// Radians.
    pub fling_pitch: f32,
    pub fling_pitch_variance: f32,
    /// Extra state for a slow-death subclass (`HelicopterSlowDeathBehavior`,
    /// `NeutronMissileSlowDeathBehavior`); `BattleBusSlowDeathBehavior`'s real death is a plain
    /// slow death.
    pub special: Option<SpecialDeathExtras>,
}

/// `HelicopterSlowDeathBehaviorModuleData` (sim units).
#[derive(Clone, Debug, Default)]
pub struct HelicopterExtras {
    pub spiral_orbit_turn_rate: f32,
    pub spiral_orbit_forward_speed: f32,
    pub spiral_orbit_forward_speed_damping: f32,
    pub min_self_spin: f32,
    pub max_self_spin: f32,
    /// Frames between self-spin uses (`SelfSpinUpdateDelay`).
    pub self_spin_update_delay: f32,
    /// Radians added per update (`SelfSpinUpdateAmount`).
    pub self_spin_update_amount: f32,
    /// Percent fraction (`FallHowFast`).
    pub fall_how_fast: f32,
    pub min_blade_fly_off_delay: f32,
    pub max_blade_fly_off_delay: f32,
    pub attach_particle: Option<String>,
    pub blade_object_name: String,
    pub ocl_eject_pilot: Option<Arc<OclTemplate>>,
    pub fx_blade: Option<String>,
    pub ocl_blade: Option<Arc<OclTemplate>>,
    pub fx_hit_ground: Option<String>,
    pub ocl_hit_ground: Option<Arc<OclTemplate>>,
    pub fx_final_blow_up: Option<String>,
    pub ocl_final_blow_up: Option<Arc<OclTemplate>>,
    /// Frames (Real in the INI).
    pub delay_from_ground_to_final_death: f32,
    pub final_rubble_object: String,
    pub max_braking: f32,
    pub death_loop_sound: Option<String>,
}

/// One `BlastInfo` of `NeutronMissileSlowDeathBehavior` (durations in Real frames).
#[derive(Clone, Copy, Debug, Default)]
pub struct NeutronBlast {
    pub enabled: bool,
    pub delay: f32,
    pub scorch_delay: f32,
    pub inner_radius: f32,
    pub outer_radius: f32,
    pub max_damage: f32,
    pub min_damage: f32,
    pub topple_speed: f32,
    pub push_force: f32,
}

/// `NeutronMissileSlowDeathBehaviorModuleData` (sim units): the falling warhead's ground blasts.
#[derive(Clone, Debug, Default)]
pub struct NeutronExtras {
    pub scorch_size: f32,
    pub fx_list: Option<String>,
    pub blasts: Vec<NeutronBlast>,
}

/// The subclass-specific extras a slow death carries.
#[derive(Clone, Debug)]
pub enum SpecialDeathExtras {
    Helicopter(HelicopterExtras),
    Neutron(NeutronExtras),
}

/// `JetSlowDeathBehaviorModuleData` (sim units): the base slow death plus the jet's phases.
#[derive(Clone, Debug)]
pub struct JetSlowDeathTemplate {
    pub slow: Arc<SlowDeathTemplate>,
    pub fx_on_ground_death: Vec<String>,
    pub ocl_on_ground_death: Vec<Arc<OclTemplate>>,
    pub fx_initial_death: Vec<String>,
    pub ocl_initial_death: Vec<Arc<OclTemplate>>,
    pub delay_secondary_from_initial_death: u32,
    pub fx_secondary: Vec<String>,
    pub ocl_secondary: Vec<Arc<OclTemplate>>,
    pub fx_hit_ground: Vec<String>,
    pub ocl_hit_ground: Vec<Arc<OclTemplate>>,
    pub delay_final_blow_up_from_hit_ground: u32,
    pub fx_final_blow_up: Vec<String>,
    pub ocl_final_blow_up: Vec<Arc<OclTemplate>>,
    pub roll_rate: f32,
    pub roll_rate_delta: f32,
    pub pitch_rate: f32,
    pub fall_how_fast: f32,
}

/// The class-specific data of a die module.
#[derive(Clone, Debug)]
pub enum DieTemplate {
    /// `DestroyDie`.
    Destroy,
    /// `InstantDeathBehavior`.
    InstantDeath {
        fx: Vec<String>,
        ocl: Vec<Arc<OclTemplate>>,
        weapons: Vec<Arc<WeaponTemplate>>,
    },
    /// `CreateObjectDie`.
    CreateObject {
        ocl: Option<Arc<OclTemplate>>,
        transfer_previous_health: bool,
        transfer_selection: bool,
    },
    /// `FXListDie` (FX are client side; the sim records an event).
    FxList {
        default_death_fx: Option<String>,
        orient_to_object: bool,
        initially_active: bool,
    },
    /// `SlowDeathBehavior`.
    SlowDeath(Arc<SlowDeathTemplate>),
    /// `JetSlowDeathBehavior`.
    JetSlowDeath(Arc<JetSlowDeathTemplate>),
    /// `RebuildHoleExposeDie`: leave a rebuild hole in place of the structure.
    RebuildHoleExpose(crate::modules4::RebuildHoleExposeTemplate),
    /// `RebuildHoleBehavior` as a die module: the hole takes its worker with it.
    RebuildHole,
    /// `GenerateMinefieldBehavior` as a die module: lay the field on death.
    GenerateMinefield,
    /// `UpgradeDie`: remove `upgrade` from the producer (`Object::getProducerID`).
    Upgrade { upgrade: Option<String> },
    /// `SpecialPowerCompletionDie`: tell the script engine the creator's power completed.
    SpecialPowerCompletion { power: String },
    /// `DamDie`: re-enable the map's water waves (`KINDOF_WAVEGUIDE`).
    Dam,
    /// `NeutronBlastBehavior`: kill infantry / unman vehicles in a radius on death.
    NeutronBlast(crate::superweapon::NeutronBlastTemplate),
}

/// One converted die module.
#[derive(Clone, Debug)]
pub struct DieModuleTemplate {
    pub mux: DieMux,
    pub kind: DieTemplate,
}

/// `JetSlowDeathBehavior`'s own timers (`m_timerDeathFrame` / `m_timerOnGroundFrame` / roll).
#[derive(Clone, Debug)]
pub(crate) struct JetDeathState {
    death_frame: u32,
    on_ground_frame: u32,
    roll_rate: f32,
}

/// Per-object slow-death state (`SlowDeathBehavior::beginSlowDeath`).
#[derive(Clone, Debug)]
pub(crate) struct SlowDeathState {
    data: Arc<SlowDeathTemplate>,
    sink_frame: u32,
    midpoint_frame: u32,
    destruction_frame: u32,
    midpoint_executed: bool,
    /// `SlowDeathBehavior::m_flags` `FLUNG_INTO_AIR`: the object is flying; the timers are pushed
    /// out each frame until it lands.
    flung_into_air: bool,
    /// `BOUNCED`: the flung object has touched down.
    bounced: bool,
    /// `JetSlowDeathBehavior`'s extra timers, when this is a jet death.
    jet: Option<JetDeathState>,
    /// The jet module's FX/OCL lists and delays, when this is a jet death.
    jet_template: Option<Arc<JetSlowDeathTemplate>>,
    /// Subclass runtime state (`HelicopterSlowDeathBehavior` / `NeutronMissileSlowDeathBehavior`).
    special: Option<SpecialDeathRuntime>,
}

/// Per-object runtime for a slow-death subclass.
#[derive(Clone, Debug)]
pub(crate) enum SpecialDeathRuntime {
    Helicopter {
        blade_frame: u32,
        hit_ground_frame: u32,
        forward_speed: f32,
        self_spin: f32,
        last_spin_frame: u32,
        forward_angle: f32,
        dead: bool,
    },
    Neutron {
        next_blast: usize,
        /// Frame the slow death activated (0 until the first update).
        activation: u32,
    },
}

/// An FX event the client could play later (`FXList::doFXObj`), recorded instead of run.
/// `pos` is the world position to play it at (death position, muzzle, or detonation point).
#[derive(Clone, Debug, PartialEq)]
pub struct FxEvent {
    pub object: ObjectId,
    pub name: String,
    pub pos: crate::math::Coord3D,
    /// The FX's secondary position (`FXList::doFXPos`'s `secondary`): a weapon's shot target, which a
    /// `Tracer` nugget flies toward. `None` for death, detonation and other FX.
    pub target: Option<crate::math::Coord3D>,
    /// The weapon's speed (`getWeaponSpeed`, distance per frame): a `Tracer` with no `Speed` uses it
    /// (FXList.cpp:179-181). Zero when there is no weapon.
    pub weapon_speed: f32,
}

impl GameLogic {
    /// `Object::onDie`: run each die module in definition order, filtered by its `DieMux`. `source` is the damage's
    /// attacker (`damageInfo->in.m_sourceID`).
    pub(crate) fn on_die(&mut self, id: ObjectId, death: DeathType, overkill: f32, source: ObjectId) {
        let Some(obj) = self.objects.get(&id) else { return };
        let template = obj.template.clone();
        let veterancy = 0u8; // veterancy is not tracked yet; every object is REGULAR
        // `FireWeaponWhenDeadBehavior::onDie` (a die module; fired before the death effects).
        self.fire_weapon_when_dead(id, death, overkill);
        // `CreateCrateDie` / `EjectPilotDie` (spawn on death).
        self.die_spawns(id, death);
        for (die_index, dm) in template.die.iter().enumerate() {
            // `DieMuxData::isDieApplicable` reads the live status, which an earlier module can change
            // (`DestroyDie` sets `DESTROYED`).
            let status = self.objects.get(&id).map_or(0, |o| o.status_mask(self.frame));
            if !dm.mux.is_applicable(death, veterancy, status) {
                continue;
            }
            match &dm.kind {
                DieTemplate::Destroy => self.destroy_object(id),
                DieTemplate::InstantDeath { fx, ocl, weapons } => {
                    if self.is_effectively_dead(id) {
                        continue;
                    }
                    self.mark_effectively_dead(id);
                    if !fx.is_empty() {
                        let idx = self.rng.value(0, fx.len() as i32 - 1) as usize;
                        self.record_fx(id, fx[idx].clone());
                    }
                    if !ocl.is_empty() {
                        let idx = self.rng.value(0, ocl.len() as i32 - 1) as usize;
                        let o = ocl[idx].clone();
                        self.run_ocl(&o, id);
                    }
                    if !weapons.is_empty() {
                        let idx = self.rng.value(0, weapons.len() as i32 - 1) as usize;
                        let w = weapons[idx].clone();
                        self.fire_temp_weapon(id, w);
                    }
                    self.destroy_object(id);
                }
                DieTemplate::CreateObject { ocl, .. } => {
                    if let Some(o) = ocl {
                        let o = o.clone();
                        self.run_ocl(&o, id);
                    }
                }
                DieTemplate::FxList { default_death_fx, .. } => {
                    if let Some(name) = default_death_fx {
                        self.record_fx(id, name.clone());
                    }
                }
                DieTemplate::SlowDeath(_) => {
                    if self.is_effectively_dead(id) {
                        continue;
                    }
                    self.mark_effectively_dead(id);
                    self.start_slow_death(id, death, overkill);
                }
                DieTemplate::JetSlowDeath(j) => {
                    if self.is_effectively_dead(id) {
                        continue;
                    }
                    self.mark_effectively_dead(id);
                    self.start_jet_slow_death(id, j.clone(), die_index);
                }
                DieTemplate::RebuildHoleExpose(expose) => {
                    self.rebuild_hole_expose_die(id, &expose.clone());
                }
                DieTemplate::RebuildHole => self.rebuild_hole_on_die(id),
                DieTemplate::GenerateMinefield => self.generate_minefield_on_die(id),
                DieTemplate::Upgrade { upgrade } => {
                    if let Some(name) = upgrade {
                        let producer = self.objects.get(&id).map(|o| o.producer_id).unwrap_or(ObjectId::INVALID);
                        self.remove_upgrade_from_producer(producer, name);
                    }
                }
                DieTemplate::SpecialPowerCompletion { power } => {
                    // `SpecialPowerCompletionDie::notifyScriptEngine`: only when the object was
                    // given a creator; the power belongs to the dying object's controlling player.
                    let creator = self.objects.get(&id).map(|o| o.sp_creator).unwrap_or(ObjectId::INVALID);
                    if creator != ObjectId::INVALID {
                        let player = self.objects.get(&id).map(|o| o.player).unwrap_or(0);
                        self.record_completed_special_power(player, power, creator);
                    }
                }
                DieTemplate::Dam => self.dam_die_reenable_waves(),
                DieTemplate::NeutronBlast(b) => self.apply_neutron_blast(id, b),
            }
        }
        // `Object::onDie`'s tail (Object.cpp:4657-4665): the spawner hears that this spawn died.
        self.spawn_notify_spawner_of_death(id, source);
        // Fallback for a template with no die modules at all (synthetic data): the engine's
        // `DefaultThingTemplate` always carries a `DestroyDie`, so shipped objects never get here.
        // A template whose only die module is non-destroying (`FXListDie`) keeps the object alive
        // (the bridge object must persist in `BODY_RUBBLE` so it can be repaired).
        if template.die.is_empty() && self.objects.get(&id).is_some_and(|o| !o.destroyed) && !self.slow_deaths.contains_key(&id) {
            self.destroy_object(id);
        }
    }

    /// `SlowDeathBehavior::onDie`: weighted choice among all applicable SlowDeath modules.
    fn start_slow_death(&mut self, id: ObjectId, death: DeathType, overkill: f32) {
        let Some(obj) = self.objects.get(&id) else { return };
        let template = obj.template.clone();
        let status = obj.status_mask(self.frame);
        let max_health = obj.body.as_ref().map_or(0.0, |b| b.max_health());

        let mut total = 0i32;
        for dm in &template.die {
            if let DieTemplate::SlowDeath(sd) = &dm.kind {
                if dm.mux.is_applicable(death, 0, status) {
                    total += probability_modifier(sd, overkill, max_health);
                }
            }
        }
        if total <= 0 {
            return;
        }
        let mut roll = self.rng.value(1, total);
        let mut chosen: Option<(Arc<SlowDeathTemplate>, usize)> = None;
        for (index, dm) in template.die.iter().enumerate() {
            if let DieTemplate::SlowDeath(sd) = &dm.kind {
                if dm.mux.is_applicable(death, 0, status) {
                    roll -= probability_modifier(sd, overkill, max_health);
                    if roll <= 0 {
                        chosen = Some((sd.clone(), index));
                        break;
                    }
                }
            }
        }
        let Some((sd, die_index)) = chosen else { return };
        self.schedule_slow_death(id, &sd, overkill, None, die_index);
    }

    /// `JetSlowDeathBehavior::onDie`: on the ground only the ground FX/OCL run; in the air the
    /// slow death begins with the jet's roll/timers.
    fn start_jet_slow_death(&mut self, id: ObjectId, j: Arc<JetSlowDeathTemplate>, die_index: usize) {
        let (pos, on_ground) = match self.objects.get(&id) {
            Some(o) => (o.position, o.position.z - self.terrain.ground_height(o.position.x, o.position.y) <= 9.0),
            None => return,
        };
        let _ = pos;
        if on_ground {
            for name in j.fx_on_ground_death.clone() {
                self.record_fx(id, name);
            }
            for ocl in j.ocl_on_ground_death.clone() {
                self.run_ocl(&ocl, id);
            }
            self.destroy_object(id);
            return;
        }
        let now = self.frame;
        let jet = JetDeathState { death_frame: now, on_ground_frame: 0, roll_rate: j.roll_rate };
        // `beginSlowDeath`: `locomotor->setMaxLift(-gravity * (1 - FallHowFast))`; the sim has no
        // per-object lift override, so gravity takes the jet down (`allow_to_fall`).
        if let Some(p) = self.objects.get_mut(&id).and_then(|o| o.physics.as_mut()) {
            p.allow_to_fall = true;
        }
        self.schedule_slow_death(id, &j.slow, 0.0, Some((j.clone(), jet)), die_index);
        for name in j.fx_initial_death.clone() {
            self.record_fx(id, name);
        }
        for ocl in j.ocl_initial_death.clone() {
            self.run_ocl(&ocl, id);
        }
    }

    /// `SlowDeathBehavior::beginSlowDeath` (timeScale = 1, no fling physics): the RNG order is sink,
    /// destruction, midpoint, then the fling draws, then the INITIAL phase.
    fn schedule_slow_death(
        &mut self,
        id: ObjectId,
        sd: &Arc<SlowDeathTemplate>,
        _overkill: f32,
        jet: Option<(Arc<JetSlowDeathTemplate>, JetDeathState)>,
        die_index: usize,
    ) {
        let now = self.frame;
        let sink = sd.sink_delay + self.rng.value(0, sd.sink_delay_variance as i32) as u32;
        let destruction = sd.destruction_delay + self.rng.value(0, sd.destruction_delay_variance as i32) as u32;
        let midpoint =
            self.rng.value((BEGIN_MIDPOINT_RATIO * destruction as f32) as i32, (END_MIDPOINT_RATIO * destruction as f32) as i32)
                as u32;
        // `SlowDeathBehavior::beginSlowDeath`'s fling block (`SlowDeathBehavior.cpp:265`): free the
        // object, lift it a little above the ground, apply the random force, reduce its ground
        // friction, allow bouncing and face it along the force.
        let mut flung = false;
        if sd.fling_force > 0.0 {
            let angle = self.rng.value_real(-PI, PI);
            let pitch = self.rng.value_real(sd.fling_pitch, sd.fling_pitch + sd.fling_pitch_variance);
            let mag = self.rng.value_real(sd.fling_force, sd.fling_force + sd.fling_force_variance);
            let force = crate::ocl::calc_random_force(angle, pitch, mag);
            let ground = self.objects.get(&id).map_or(0.0, |o| self.terrain.ground_height(o.position.x, o.position.y));
            if let Some(o) = self.objects.get_mut(&id) {
                if o.position.z - ground < 1.0 {
                    o.set_position(crate::math::Coord3D::new(o.position.x, o.position.y, o.position.z + 1.0));
                    o.mark_partition_dirty();
                }
            }
            self.set_allow_to_fall(id, true);
            self.apply_force(id, crate::math::Coord3D::new(force.0, force.1, force.2));
            // `setExtraBounciness(-1)` / `setExtraFriction(-3 * SECONDS_PER_LOGICFRAME_REAL)`.
            self.set_extra_bounciness(id, -1.0);
            self.set_extra_friction(id, -3.0 / crate::math::LOGICFRAMES_PER_SECOND as f32);
            self.set_allow_bouncing(id, true);
            let orientation = crate::math::atan2(force.1, force.0);
            self.set_angles(id, orientation, 0.0, 0.0);
            flung = true;
        }
        let (jet_template, jet_state) = match jet {
            Some((t, s)) => (Some(t), Some(s)),
            None => (None, None),
        };
        // `HelicopterSlowDeathBehavior::beginSlowDeath`: after the base's draws, pick the blade
        // fly-off frame (the engine draws `GameLogicRandomValueReal(min, max)` and ceil's it).
        let special = match &sd.special {
            Some(SpecialDeathExtras::Helicopter(h)) => {
                let blade = if h.max_blade_fly_off_delay > h.min_blade_fly_off_delay {
                    self.rng.value_real(h.min_blade_fly_off_delay, h.max_blade_fly_off_delay)
                } else {
                    h.min_blade_fly_off_delay
                };
                Some(SpecialDeathRuntime::Helicopter {
                    blade_frame: now + blade.max(0.0) as u32,
                    hit_ground_frame: 0,
                    forward_speed: h.spiral_orbit_forward_speed,
                    self_spin: 0.0,
                    last_spin_frame: now,
                    forward_angle: self.objects.get(&id).map_or(0.0, |o| o.angle),
                    dead: false,
                })
            }
            Some(SpecialDeathExtras::Neutron(_)) => {
                Some(SpecialDeathRuntime::Neutron { next_blast: 0, activation: 0 })
            }
            None => None,
        };
        self.slow_deaths.insert(
            id,
            SlowDeathState {
                data: sd.clone(),
                sink_frame: now + sink,
                midpoint_frame: now + midpoint,
                destruction_frame: now + destruction,
                midpoint_executed: false,
                flung_into_air: flung,
                bounced: false,
                jet: jet_state,
                jet_template,
                special,
            },
        );
        // `beginSlowDeath`'s `setWakeFrame`: a fling wakes next frame (SlowDeathBehavior.cpp:291); otherwise at the
        // earliest of the three relative timers (`whenToWakeTime`, SlowDeathBehavior.cpp:322-330).
        let wake = if flung { now + 1 } else { now + sink.min(destruction).min(midpoint) };
        self.wake_die_module(id, die_index, wake);
        // The base class's last statement (`SlowDeathBehavior.cpp:329-330`). `JetSlowDeathBehavior::beginSlowDeath`
        // (`JetSlowDeathBehavior.cpp:180-184`) calls the base first and only then runs its own initial FX/OCL
        // (:189-193), so the base INITIAL phase (and its draws) happens for a jet too.
        self.do_slow_death_phase(id, sd, PHASE_INITIAL);
    }

    /// `SlowDeathBehavior::doPhaseStuff`.
    fn do_slow_death_phase(&mut self, id: ObjectId, sd: &Arc<SlowDeathTemplate>, phase: usize) {
        if !sd.fx[phase].is_empty() {
            let idx = self.rng.value(0, sd.fx[phase].len() as i32 - 1) as usize;
            let name = sd.fx[phase][idx].clone();
            self.record_fx(id, name);
        }
        if !sd.ocl[phase].is_empty() {
            let idx = self.rng.value(0, sd.ocl[phase].len() as i32 - 1) as usize;
            let o = sd.ocl[phase][idx].clone();
            self.run_ocl(&o, id);
        }
        if !sd.weapons[phase].is_empty() {
            let idx = self.rng.value(0, sd.weapons[phase].len() as i32 - 1) as usize;
            let w = sd.weapons[phase][idx].clone();
            self.fire_temp_weapon(id, w);
        }
    }

    /// `SlowDeathBehavior::update` (SlowDeathBehavior.cpp:369-466): sink, midpoint and destruction checks, once per
    /// frame. Returns `UPDATE_SLEEP_NONE` while the death runs (the update returns NONE), and forever once it ends.
    pub(crate) fn slow_death_update(&mut self, id: ObjectId, now: u32) -> u32 {
        {
            let Some(state) = self.slow_deaths.get(&id) else { return UPDATE_SLEEP_FOREVER };
            let sd = state.data.clone();
            let (mut sink_frame, mut midpoint_frame, mut destruction_frame, mid_done) =
                (state.sink_frame, state.midpoint_frame, state.destruction_frame, state.midpoint_executed);
            let (flung_into_air, mut bounced) = (state.flung_into_air, state.bounced);
            let (jet, jet_template) = (state.jet.clone(), state.jet_template.clone());

            // `SlowDeathBehavior::update`'s `FLUNG_INTO_AIR` block (`SlowDeathBehavior.cpp:408`):
            // while the body flies, push the sink/midpoint/destruction timers out each frame so it
            // does not sink or explode mid-air; mark it bounced once it lands.
            if flung_into_air && !bounced {
                sink_frame += 1;
                midpoint_frame += 1;
                destruction_frame += 1;
                let above = self.objects.get(&id).is_some_and(|o| {
                    o.position.z - self.terrain.ground_height(o.position.x, o.position.y) > 0.0
                });
                if !above {
                    bounced = true;
                }
            }
            if let Some(s) = self.slow_deaths.get_mut(&id) {
                s.sink_frame = sink_frame;
                s.midpoint_frame = midpoint_frame;
                s.destruction_frame = destruction_frame;
                s.bounced = bounced;
            }
            if now >= sink_frame && sd.sink_rate > 0.0 {
                if let Some(o) = self.objects.get_mut(&id) {
                    // The sink goes through `set_position`, so the transform's translation follows it (R18-8): a turn
                    // on a sinking object then keeps the sunk height (`Thing::setPosition`, `Thing.cpp:177-205`).
                    let p = o.position;
                    o.set_position(crate::math::Coord3D::new(p.x, p.y, p.z - sd.sink_rate));
                    o.mark_partition_dirty();
                }
            }
            if now >= midpoint_frame && !mid_done {
                self.do_slow_death_phase(id, &sd, PHASE_MIDPOINT);
                if let Some(s) = self.slow_deaths.get_mut(&id) {
                    s.midpoint_executed = true;
                }
            }
            // `JetSlowDeathBehavior::update`: hit-ground / secondary / final blow-up timers.
            if let (Some(mut js), Some(jt)) = (jet, jet_template) {
                let mut remove = false;
                if js.on_ground_frame == 0 {
                    let height = self.objects.get(&id).map_or(0.0, |o| o.position.z - self.terrain.ground_height(o.position.x, o.position.y));
                    if height <= 0.0 {
                        for name in jt.fx_hit_ground.clone() {
                            self.record_fx(id, name);
                        }
                        for ocl in jt.ocl_hit_ground.clone() {
                            self.run_ocl(&ocl, id);
                        }
                        js.on_ground_frame = now;
                    }
                    if js.death_frame != 0 && now.saturating_sub(js.death_frame) >= jt.delay_secondary_from_initial_death {
                        for name in jt.fx_secondary.clone() {
                            self.record_fx(id, name);
                        }
                        for ocl in jt.ocl_secondary.clone() {
                            self.run_ocl(&ocl, id);
                        }
                        js.death_frame = 0;
                    }
                } else if now.saturating_sub(js.on_ground_frame) >= jt.delay_final_blow_up_from_hit_ground {
                    for name in jt.fx_final_blow_up.clone() {
                        self.record_fx(id, name);
                    }
                    for ocl in jt.ocl_final_blow_up.clone() {
                        self.run_ocl(&ocl, id);
                    }
                    self.destroy_object(id);
                    remove = true;
                }
                if remove {
                    self.slow_deaths.remove(&id);
                    return UPDATE_SLEEP_FOREVER;
                }
                if let Some(s) = self.slow_deaths.get_mut(&id) {
                    s.jet = Some(js);
                }
            }
            // `HelicopterSlowDeathBehavior::update`: spiral down, fling the blade, eject the pilot,
            // hit the ground, then the final blow-up (the base destruction delay is enormous, so the
            // helicopter subclass owns the actual destruction).
            let heli = match self.slow_deaths.get(&id).and_then(|s| s.special.clone()) {
                Some(SpecialDeathRuntime::Helicopter {
                    blade_frame,
                    hit_ground_frame,
                    forward_speed,
                    self_spin,
                    last_spin_frame,
                    forward_angle,
                    dead,
                }) => Some((blade_frame, hit_ground_frame, forward_speed, self_spin, last_spin_frame, forward_angle, dead)),
                _ => None,
            };
            if let (Some((blade_frame, mut hit_ground_frame, forward_speed, self_spin, last_spin_frame, forward_angle, dead)), Some(SpecialDeathExtras::Helicopter(h))) =
                (heli, sd.special.as_ref())
            {
                if !dead {
                    // Move along the heading, orbit-turn and sink toward the ground.
                    let (px, py, pz) = self.objects.get(&id).map_or((0.0, 0.0, 0.0), |o| (o.position.x, o.position.y, o.position.z));
                    let na = forward_angle + h.spiral_orbit_turn_rate;
                    let nx = px + cos(forward_angle) * forward_speed;
                    let ny = py + sin(forward_angle) * forward_speed;
                    let ground = self.terrain.ground_height(nx, ny);
                    let fall = 30.0 * h.fall_how_fast;
                    let nz = (pz - fall).max(ground);
                    if let Some(o) = self.objects.get_mut(&id) {
                        o.set_position(crate::math::Coord3D::new(nx, ny, nz));
                        o.set_orientation(na + self_spin);
                    }
                    // Self-spin: every `SelfSpinUpdateDelay` frames add `SelfSpinUpdateAmount` until
                    // `MaxSelfSpin` is reached.
                    let mut spin = self_spin;
                    let mut last_spin = last_spin_frame;
                    if now.saturating_sub(last_spin_frame) as f32 >= h.self_spin_update_delay.max(1.0) {
                        spin = (spin + h.self_spin_update_amount).min(h.max_self_spin);
                        last_spin = now;
                    }
                    // The blade flies off once.
                    let mut blade_frame = blade_frame;
                    if blade_frame != 0 && now >= blade_frame {
                        if let Some(name) = &h.fx_blade {
                            self.record_fx(id, name.clone());
                        }
                        if let Some(ocl) = h.ocl_blade.clone() {
                            self.run_ocl(&ocl, id);
                        }
                        if let Some(ocl) = h.ocl_eject_pilot.clone() {
                            self.run_ocl(&ocl, id);
                        }
                        blade_frame = 0;
                    }
                    // Hit the ground, then the final blow-up after the delay.
                    let height = self.objects.get(&id).map_or(0.0, |o| o.position.z - ground);
                    if hit_ground_frame == 0 && height <= 1.0 {
                        hit_ground_frame = now;
                        if let Some(name) = &h.fx_hit_ground {
                            self.record_fx(id, name.clone());
                        }
                        if let Some(ocl) = h.ocl_hit_ground.clone() {
                            self.run_ocl(&ocl, id);
                        }
                        let g = self.terrain.ground_height(nx, ny);
                        if let Some(o) = self.objects.get_mut(&id) {
                            let mut p = o.position;
                            p.z = g;
                            o.set_position(p);
                        }
                    }
                    if hit_ground_frame != 0 && now.saturating_sub(hit_ground_frame) as f32 >= h.delay_from_ground_to_final_death {
                        if let Some(name) = &h.fx_final_blow_up {
                            self.record_fx(id, name.clone());
                        }
                        if let Some(ocl) = h.ocl_final_blow_up.clone() {
                            self.run_ocl(&ocl, id);
                        }
                        self.destroy_object(id);
                        self.slow_deaths.remove(&id);
                        return UPDATE_SLEEP_FOREVER;
                    }
                    if let Some(s) = self.slow_deaths.get_mut(&id) {
                        if let Some(SpecialDeathRuntime::Helicopter {
                            blade_frame: bf,
                            hit_ground_frame: hg,
                            forward_angle: fa,
                            self_spin: ss,
                            last_spin_frame: ls,
                            ..
                        }) = s.special.as_mut()
                        {
                            *bf = blade_frame;
                            *hg = hit_ground_frame;
                            *fa = na;
                            *ss = spin;
                            *ls = last_spin;
                        }
                    }
                }
            }
            // `NeutronMissileSlowDeathBehavior::update`: once activated, play the ground FX and run
            // the timed blasts at the missile's position.
            if let Some(SpecialDeathExtras::Neutron(n)) = sd.special.as_ref() {
                let activation = self
                    .slow_deaths
                    .get(&id)
                    .and_then(|s| match &s.special {
                        Some(SpecialDeathRuntime::Neutron { activation, .. }) => Some(*activation),
                        _ => None,
                    });
                if activation == Some(0) {
                    if let Some(fx) = &n.fx_list {
                        self.record_fx(id, fx.clone());
                    }
                    if let Some(s) = self.slow_deaths.get_mut(&id) {
                        if let Some(SpecialDeathRuntime::Neutron { activation, .. }) = s.special.as_mut() {
                            *activation = now;
                        }
                    }
                }
                let activation = self
                    .slow_deaths
                    .get(&id)
                    .and_then(|s| match &s.special {
                        Some(SpecialDeathRuntime::Neutron { activation, .. }) => Some(*activation),
                        _ => None,
                    })
                    .unwrap_or(now);
                let mut next = self
                    .slow_deaths
                    .get(&id)
                    .and_then(|s| match &s.special {
                        Some(SpecialDeathRuntime::Neutron { next_blast, .. }) => Some(*next_blast),
                        _ => None,
                    })
                    .unwrap_or(0);
                while next < n.blasts.len() {
                    let b = n.blasts[next];
                    if !b.enabled {
                        next += 1;
                        continue;
                    }
                    if (now.saturating_sub(activation)) as f32 > b.delay {
                        self.neutron_blast(id, &b);
                        next += 1;
                    } else {
                        break;
                    }
                }
                if let Some(s) = self.slow_deaths.get_mut(&id) {
                    if let Some(SpecialDeathRuntime::Neutron { next_blast, .. }) = s.special.as_mut() {
                        *next_blast = next;
                    }
                }
            }
            if now >= destruction_frame {
                self.do_slow_death_phase(id, &sd, PHASE_FINAL);
                self.destroy_object(id);
                self.slow_deaths.remove(&id);
                return UPDATE_SLEEP_FOREVER;
            }
        }
        // `SlowDeathBehavior::update` returns `UPDATE_SLEEP_NONE` (SlowDeathBehavior.cpp:465) while it is still running.
        UPDATE_SLEEP_NONE
    }

    /// `Object::onDie` marks the AI dead so later die modules skip (`isAiInDeadState`).
    pub(crate) fn mark_effectively_dead(&mut self, id: ObjectId) {
        if let Some(o) = self.objects.get_mut(&id) {
            o.effectively_dead = true;
            o.combat.attack = None;
            o.ai.stop();
        }
        // `AIUpdateInterface::markAsDead` (AIUpdate.cpp:1193-1198): `wakeUpNow()` so the dying unit's update runs
        // promptly. The instant- and slow-death callers (InstantDeathBehavior.cpp:134, :191; SlowDeathBehavior.cpp:487)
        // reach it through `ai->markAsDead()`.
        self.wake_up_now(id);
    }

    pub(crate) fn is_effectively_dead(&self, id: ObjectId) -> bool {
        self.objects.get(&id).is_some_and(|o| o.effectively_dead)
    }

    pub(crate) fn record_fx(&mut self, object: ObjectId, name: String) {
        let pos = self.objects.get(&object).map_or(crate::math::Coord3D::new(0.0, 0.0, 0.0), |o| o.position);
        self.record_fx_at(object, name, pos);
    }

    /// Record an FX event at an explicit world position (weapon fire / projectile detonation).
    pub(crate) fn record_fx_at(&mut self, object: ObjectId, name: String, pos: crate::math::Coord3D) {
        self.fx_events.push(FxEvent { object, name, pos, target: None, weapon_speed: 0.0 });
    }

    /// Record a weapon's FX at its muzzle, with the shot's target and the weapon's speed
    /// (`WeaponTemplate::fireWeaponTemplate` -> `FXList::doFXPos(fx, where, mtx, speed, &targetPos, radius)`).
    pub(crate) fn record_fx_toward(&mut self, object: ObjectId, name: String, pos: crate::math::Coord3D, target: crate::math::Coord3D, weapon_speed: f32) {
        self.fx_events.push(FxEvent { object, name, pos, target: Some(target), weapon_speed });
    }

    /// FX events recorded since the last call, for the client (`FXListDie`, death FX).
    pub fn fx_events(&self) -> &[FxEvent] {
        &self.fx_events
    }

    /// Clear the recorded FX events (the viewer drains them once it has played them).
    pub fn clear_fx_events(&mut self) {
        self.fx_events.clear();
    }

    /// Slow-death state for hashing.
    pub(crate) fn slow_deaths_crc(&self, c: &mut Crc) {
        c.add_u32(self.slow_deaths.len() as u32);
        for (id, s) in &self.slow_deaths {
            c.add_u32(id.0);
            c.add_u32(s.sink_frame);
            c.add_u32(s.midpoint_frame);
            c.add_u32(s.destruction_frame);
            c.add_bool(s.midpoint_executed);
            match &s.jet {
                Some(j) => {
                    c.add_bool(true);
                    c.add_u32(j.death_frame);
                    c.add_u32(j.on_ground_frame);
                    c.add_f32(j.roll_rate);
                }
                None => c.add_bool(false),
            }
        }
    }

    /// `UpgradeDie::onDie`: if the producer still has the upgrade, remove it (and re-run its
    /// upgrade modules).
    fn remove_upgrade_from_producer(&mut self, producer: ObjectId, name: &str) {
        if producer == ObjectId::INVALID || !self.objects.contains_key(&producer) {
            return;
        }
        let has = self.objects.get(&producer).is_some_and(|o| o.upgrades.contains(name));
        if !has {
            return;
        }
        if let Some(o) = self.objects.get_mut(&producer) {
            o.upgrades.remove(name);
        }
        self.update_upgrade_modules(producer);
    }

    /// `DamDie::onDie`: clear `DISABLED_DEFAULT` on every `KINDOF_WAVEGUIDE` object, enabling the
    /// map's water waves.
    fn dam_die_reenable_waves(&mut self) {
        let ids: Vec<ObjectId> = self
            .objects
            .values()
            .filter(|o| o.is_kind_of(crate::template::kind_of::WAVEGUIDE))
            .map(|o| o.id)
            .collect();
        for id in ids {
            self.clear_disabled(id, crate::disabled::DisabledType::Default);
        }
    }

    /// `NeutronMissileSlowDeathBehavior::doBlast`: damage every object within `outer_radius` (full
    /// damage inside `inner_radius`, linear falloff to `minDamage` at the rim) and push it away.
    fn neutron_blast(&mut self, id: ObjectId, b: &NeutronBlast) {
        if b.outer_radius <= 0.0 {
            return;
        }
        let Some((pos, source)) = self.objects.get(&id).map(|o| (o.position, o.id)) else { return };
        let victims = self.partition.objects_in_range(&pos, b.outer_radius, crate::partition::DistanceMode::FromBoundingSphere2D);
        for cid in victims {
            let Some(cpos) = self.objects.get(&cid).map(|o| o.position) else { continue };
            if cid == id {
                continue;
            }
            let d = pos.dist_2d(&cpos);
            let amount = if d <= b.inner_radius {
                b.max_damage
            } else if d >= b.outer_radius {
                b.min_damage
            } else {
                let t = (d - b.inner_radius) / (b.outer_radius - b.inner_radius);
                b.max_damage + (b.min_damage - b.max_damage) * t
            };
            if amount > 0.0 {
                self.damage_object_with_death(
                    cid,
                    crate::body::DamageInfo { source, amount, kind: crate::damage::DamageType::Explosion },
                    crate::damage::DeathType::Exploded,
                );
            }
            if b.push_force != 0.0 {
                let len = d.max(1e-3);
                let f = crate::math::Coord3D::new((cpos.x - pos.x) / len * b.push_force, (cpos.y - pos.y) / len * b.push_force, b.push_force * 0.1);
                self.apply_force(cid, f);
            }
        }
    }
}

/// `SlowDeathBehavior::getProbabilityModifier`: the base modifier plus an overkill bonus, at least 1.
fn probability_modifier(sd: &SlowDeathTemplate, overkill: f32, max_health: f32) -> i32 {
    let overkill_percent = if max_health > 0.0 { overkill / max_health } else { 0.0 };
    let bonus = overkill_percent * sd.modifier_bonus_per_overkill_percent;
    let value = sd.probability_modifier as f32 + bonus;
    if value < 1.0 {
        1
    } else {
        value as i32
    }
}

/// A `BTreeMap` of slow-death states keyed by object, for the `GameLogic` field.
pub(crate) type SlowDeathMap = BTreeMap<ObjectId, SlowDeathState>;
