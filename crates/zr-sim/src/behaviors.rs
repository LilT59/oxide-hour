//! Common gameplay behaviors: `FireWeaponWhenDeadBehavior`, `LifetimeUpdate`, `StealthUpdate` and
//! `StealthDetectorUpdate`, and the per-object state that the sleepy families keep.
//!
//! `AutoHealBehavior`, `FlammableUpdate` and `PoisonedBehavior` are sleepy-heap families now
//! (`sleepy_families.rs`): their updates run from the heap and their damage hooks wake them. The rest are
//! driven from [`GameLogic::update_behaviors`] and [`GameLogic::damage_object_with_death`]. Per-object state
//! lives in [`BehaviorState`] on the object and is hashed into the object CRC.

use std::sync::Arc;

use crate::body::DamageInfo;
use crate::damage::{DamageType, DeathType, HUGE_DAMAGE_AMOUNT};
use crate::logic::GameLogic;
use crate::module::{UPDATE_SLEEP_FOREVER, UPDATE_SLEEP_NONE};
use crate::object::{status, ObjectId};
use crate::weapon::WeaponTemplate;

/// `StealthUpdate::StealthForbiddenConditions` bits (`StealthUpdate.h`).
pub mod stealth_forbidden {
    pub const ATTACKING: u32 = 0x1;
    pub const MOVING: u32 = 0x2;
    pub const USING_ABILITY: u32 = 0x4;
    pub const FIRING_PRIMARY: u32 = 0x8;
    pub const FIRING_SECONDARY: u32 = 0x10;
    pub const FIRING_TERTIARY: u32 = 0x20;
    pub const NO_BLACK_MARKET: u32 = 0x40;
    pub const TAKING_DAMAGE: u32 = 0x80;
    pub const RIDERS_ATTACKING: u32 = 0x100;
    pub const FIRING_WEAPON: u32 = FIRING_PRIMARY | FIRING_SECONDARY | FIRING_TERTIARY;
}

/// `AutoHealBehaviorModuleData` (sim units).
#[derive(Clone, Debug, PartialEq)]
pub struct AutoHealTemplate {
    pub healing_amount: f32,
    /// Frames between heals (`u32::MAX` = never).
    pub healing_delay: u32,
    pub start_healing_delay: u32,
    pub radius: f32,
    pub affects_whole_player: bool,
    pub skip_self_for_healing: bool,
    pub single_burst: bool,
    pub kind_of: u64,
    pub forbidden_kind_of: u64,
    /// `UpgradeMuxData`: the module only runs while this upgrade is active.
    pub starts_active: bool,
    pub triggered_by: Vec<String>,
    pub conflicts_with: Vec<String>,
}

/// `FlammableUpdateModuleData` (sim units).
#[derive(Clone, Debug, PartialEq)]
pub struct FlammableTemplate {
    pub burned_delay: u32,
    pub aflame_duration: u32,
    pub aflame_damage_delay: u32,
    pub aflame_damage_amount: f32,
    pub flame_damage_limit: f32,
    pub flame_damage_expiration: u32,
}

/// `PoisonedBehaviorModuleData` (sim units).
#[derive(Clone, Debug, PartialEq)]
pub struct PoisonedTemplate {
    pub poison_damage_interval: u32,
    pub poison_duration: u32,
}

/// `StealthUpdateModuleData` (sim units).
#[derive(Clone, Debug, PartialEq)]
pub struct StealthTemplate {
    pub stealth_delay: u32,
    pub move_threshold_speed: f32,
    pub forbidden_conditions: u32,
    /// `RequiredStatus` / `ForbiddenStatus` as [`crate::object::status`] bits.
    pub required_status: u64,
    pub forbidden_status: u64,
    pub innate_stealth: bool,
    pub granted_by_special_power: bool,
    pub disguises_as_team: bool,
    pub reveal_distance_from_target: f32,
    pub order_idle_enemies_to_attack_on_reveal: bool,
    /// `UseRiderStealth`: the first rider is the stealth owner (`calcStealthOwner`, StealthUpdate.cpp:547).
    /// Its `CAN_STEALTH`, stealth level and `StealthDelay` decide; this module's own status and timing
    /// rules still apply to the container.
    pub use_rider_stealth: bool,
}

/// `StealthUpdate::m_enabled` for one object: `!m_teamDisguised` until `receiveGrant` sets it
/// (`StealthUpdate.cpp:132`, `:208`). `InnateStealth` does not decide it: it only sets
/// `OBJECT_STATUS_CAN_STEALTH` at construction (`:147-151`), which `allowedToStealth` tests on the
/// stealth owner (`:309-322`).
fn stealth_enabled(o: &crate::object::Object, t: &StealthTemplate) -> bool {
    match o.behavior.stealth_enabled {
        Some(granted) => granted,
        None => !t.disguises_as_team,
    }
}

/// `StealthDetectorUpdateModuleData` (sim units).
#[derive(Clone, Debug, PartialEq)]
pub struct StealthDetectorTemplate {
    pub detection_rate: u32,
    pub detection_range: f32,
    pub initially_disabled: bool,
    pub extra_required_kind_of: u64,
    pub extra_forbidden_kind_of: u64,
    pub can_detect_while_garrisoned: bool,
    pub can_detect_while_contained: bool,
}

/// One `AutoHealBehavior` module of an object (`ObjectTemplate::auto_heal[i]`). An object may carry several: the
/// retail GLA Scorpion Tank has the veterancy one inherited from `DefaultThingTemplate` and its own
/// (`ModuleTag_05`, `TriggeredBy = Upgrade_GLAJunkRepair`), two C++ modules with two heap slots (lane 178).
#[derive(Clone, Debug, Default)]
pub struct AutoHealState {
    /// `AutoHealBehavior::m_soonestHealFrame`: the frame the next pulse may come (`pulseHealObject`).
    pub next: u32,
    /// `UpgradeMux::m_upgradeExecuted` (`isUpgradeActive`): set at construction for StartsActive, when the
    /// TriggeredBy upgrade lands, and cleared by `undoUpgrade` (the pilot sniped, Object.cpp:2245).
    pub started: bool,
    /// This module's heap slot.
    pub handle: Option<crate::schedule::UpdateHandle>,
}

/// Per-object behavior state, defaulted on creation. The sleepy families (`sleepy_families.rs`) keep their C++
/// members here under the C++ meaning noted on each field.
#[derive(Clone, Debug, Default)]
pub struct BehaviorState {
    /// The object's AutoHeal modules, one per `ObjectTemplate::auto_heal` entry, in the template's module order.
    pub auto_heal: Vec<AutoHealState>,
    /// The heap slots of the Flammable and Poisoned families of this object.
    pub flame_handle: Option<crate::schedule::UpdateHandle>,
    pub poison_handle: Option<crate::schedule::UpdateHandle>,
    pub topple_handle: Option<crate::schedule::UpdateHandle>,
    /// The StealthUpdate and StealthDetectorUpdate heap slots of this object.
    pub stealth_handle: Option<crate::schedule::UpdateHandle>,
    pub detector_handle: Option<crate::schedule::UpdateHandle>,
    /// The `PhysicsBehavior` heap slot (physics.rs).
    pub physics_handle: Option<crate::schedule::UpdateHandle>,
    /// The `TechBuildingBehavior`, `BaseRegenerateUpdate` and `PowerPlantUpdate` heap slots of this object
    /// (their `setWakeFrame` callers: `onCapture`, `onDamage`, `extendRods`).
    pub tech_handle: Option<crate::schedule::UpdateHandle>,
    pub base_regen_handle: Option<crate::schedule::UpdateHandle>,
    pub power_plant_handle: Option<crate::schedule::UpdateHandle>,
    /// `OverchargeBehavior`'s heap slot (its `setWakeFrame` caller is `enable()`).
    pub overcharge_handle: Option<crate::schedule::UpdateHandle>,
    /// The SlowDeath modules of this object: the template's die-module index and the module's heap slot.
    pub slow_death_handles: Vec<(usize, crate::schedule::UpdateHandle)>,
    /// `FlammableUpdate::m_aflameEndFrame`: nonzero while the object is aflame (the AFLAME status).
    pub aflame_until: u32,
    /// `FlammableUpdate::m_burnedEndFrame`.
    pub burned_until: u32,
    /// `FlammableUpdate::m_damageEndFrame`: the next burn tick.
    pub flame_damage_next: u32,
    /// `FlammableUpdate::m_flameDamageLimit`: the flame damage still needed to ignite.
    pub flame_damage_accum: f32,
    /// `FlammableUpdate::m_lastFlameDamageDealt`.
    pub flame_last_damage: u32,
    /// `OBJECT_STATUS_BURNED`, set when the burn timer runs out (`FlammableUpdate::update`).
    pub flame_burned: bool,
    /// `PoisonedBehavior::m_poisonDamageFrame`: the next poison tick.
    pub poison_next: u32,
    /// `PoisonedBehavior::m_poisonOverallStopFrame`: the poison ends here (0 = not poisoned).
    pub poison_until: u32,
    /// `PoisonedBehavior::m_poisonDamageAmount`.
    pub poison_amount: f32,
    /// `PoisonedBehavior::m_deathType`: the death type of the poison hit (`None` = `DEATH_POISONED`).
    pub poison_death: Option<crate::damage::DeathType>,
    /// `LifetimeUpdate`: kill at this frame (0 = none).
    pub lifetime_die: u32,
    /// `DeletionUpdate`: destroy at this frame (0 = none).
    pub deletion_die: u32,
    /// `DynamicShroudClearingRangeUpdate::m_state` (`DSCRU_STATE`, DynamicShroudClearingRangeUpdate.h:104-112).
    pub ds_state: u8,
    /// `DynamicShroudClearingRangeUpdate::m_stateCountDown` (an `Int`; the `update` decrements it every frame).
    pub ds_state_countdown: i32,
    /// `DynamicShroudClearingRangeUpdate::m_totalFrames`.
    pub ds_total_frames: i32,
    /// `DynamicShroudClearingRangeUpdate::m_shrinkStartDeadline`.
    pub ds_shrink_start_deadline: u32,
    /// `DynamicShroudClearingRangeUpdate::m_growStartDeadline`.
    pub ds_grow_start_deadline: u32,
    /// `DynamicShroudClearingRangeUpdate::m_sustainDeadline`.
    pub ds_sustain_deadline: u32,
    /// `DynamicShroudClearingRangeUpdate::m_doneForeverFrame`.
    pub ds_done_forever_frame: u32,
    /// `DynamicShroudClearingRangeUpdate::m_changeIntervalCountdown`.
    pub ds_change_interval_countdown: u32,
    /// `DynamicShroudClearingRangeUpdate::m_nativeClearingRange`: the range captured at construction.
    pub ds_native_clearing_range: f32,
    /// `DynamicShroudClearingRangeUpdate::m_currentClearingRange`.
    pub ds_current_clearing_range: f32,
    /// `StealthUpdate`.
    pub stealth_allowed: u32,
    pub stealthed: bool,
    pub detected_until: u32,
    /// `StealthUpdate::m_enabled` once `receiveGrant` has run: `Some(active)`. `None` keeps the
    /// template's own rule (`InnateStealth` without `GrantedBySpecialPower`).
    pub stealth_enabled: Option<bool>,
    /// `StealthUpdate::m_framesGranted`: frames left on a temporary grant (0 = no countdown).
    pub stealth_frames_granted: u32,
    /// `StealthDetectorUpdate`: next sweep frame.
    pub detect_next: u32,
    /// `AutoDepositUpdate`: next deposit frame.
    pub deposit_next: u32,
    /// `AutoDepositUpdate::m_initialized`: the first due update has run.
    pub deposit_initialized: bool,
    /// `AutoDepositUpdate::m_awardInitialCaptureBonus`: armed by the first due update, paid (and cleared) when the
    /// building changes hands (`AutoDepositUpdate.cpp:129-151`).
    pub deposit_initial_bonus_armed: bool,
    /// `FireWeaponUpdate`: next fire frame.
    pub fire_next: u32,
    /// `OpenContain::m_whichExitPath` minus one: the exit path the next `exitObjectViaDoor` uses (cycles through
    /// `NumberOfExitPaths`; `OpenContain.cpp:1043-1050`). Only tunnel networks exit this way.
    pub open_exit_next: u32,
    /// `TransportContain::m_frameExitNotBusy`: the container is busy leaving passengers until this frame
    /// (`TransportContain::onRemoving` sets it to `now + ExitDelay`, TransportContain.cpp:410; `isExitBusy`, :649-655).
    pub frame_exit_not_busy: u32,
    /// `HordeUpdate::m_inHorde` (horde.rs).
    pub horde: bool,
    /// `HordeUpdate::m_trueHordeMember`: the member met `Count` on its own (`isTrueHordeMember` is this and `horde`).
    pub horde_true_member: bool,
    /// `HordeUpdate::m_lastHordeRefreshFrame`: the frame of the last horde test (set at construction).
    pub horde_last_refresh: u32,
    /// The frame the `HordeUpdate` constructor sets its first wake for: `now + GameLogicRandomValue(1, UpdateRate)`
    /// (HordeUpdate.cpp:169), drawn in `create_object_full`'s constructor pass.
    pub horde_wake: u32,
    /// `ToppleUpdate`.
    pub toppled: bool,
    pub topple_kill_frame: u32,
    /// `PropagandaTowerBehavior`: next aura tick.
    pub propaganda_next: u32,
    /// `RepairDockUpdate` / `HealContain`: next heal tick (kept for the CRC; `HealContain` heals every frame and
    /// `RepairDockUpdate` heals in its dock action, so nothing writes it any more).
    pub repair_next: u32,
    /// `TechBuildingBehavior`: `MODELCONDITION_CAPTURED` is on (the object has a playable controlling player).
    pub tech_captured: bool,
    /// `OverchargeBehavior`: overcharge is on.
    pub overcharge: bool,
    /// `PowerPlantUpdate::m_extended` (`PowerPlantUpdate.cpp:52-70`): the rods are extended (or extending), so a
    /// second `extendRods(TRUE)` does not re-wake the module.
    pub power_plant_extended: bool,
    /// `PowerPlantUpgrade` is active: adds the template's `EnergyBonus` to this object's power
    /// (`Object::onDisabledEdge`, Object.cpp:3900).
    pub power_plant_upgraded: bool,
    /// `ProneUpdate`: frames left prone.
    pub prone_frames: u32,
    /// `CountermeasuresBehavior`: reload ready frame.
    pub cm_reload_until: u32,
    /// `CountermeasuresBehavior`: live flares, oldest first.
    pub cm_flares: Vec<ObjectId>,
    /// `CountermeasuresBehavior`: counters (`m_availableCountermeasures` / `m_activeCountermeasures`).
    pub cm_available: u32,
    pub cm_active: u32,
    /// `CountermeasuresBehavior`: reaction / volley / reload frames.
    pub cm_reaction_frame: u32,
    pub cm_next_volley_frame: u32,
    pub cm_reload_frame: u32,
    /// `RebuildHoleBehavior`: the spawned worker, the reconstruction and the respawn countdown.
    pub hole_worker: Option<ObjectId>,
    pub hole_reconstructing: Option<ObjectId>,
    pub hole_worker_wait: u32,
    pub hole_spawner: Option<ObjectId>,
    /// `SlavedUpdate::m_framesToWait` (SlavedUpdate.h): the countdown that gates the slave's body to every
    /// `SLAVED_UPDATE_RATE` frames (SlavedUpdate.cpp:114-127). Starts at 0, so the first update runs the body.
    pub slaved_frames_to_wait: i32,
    /// `SlavedUpdate::m_guardPointOffset` (`SlavedUpdate.cpp:706-716`, `:255-278`): the offset from the
    /// master around which the slave guards, set when it is enslaved and re-rolled by `doGuardLogic`.
    pub slaved_guard_offset: crate::math::Coord3D,
    /// The template name the hole must rebuild.
    pub hole_rebuild_template: Option<String>,
    /// `DemoTrapUpdate`: proximity mode, the scan timer and the detonated latch.
    pub trap_proximity: bool,
    pub trap_next_scan: u32,
    pub trap_detonated: bool,
    /// `CleanupHazardUpdate`: scan timer, tracked target and in-range flag.
    pub cleanup_next_scan: u32,
    pub cleanup_target: Option<ObjectId>,
    pub cleanup_in_range: bool,
    /// `CleanupHazardUpdate`: the cleanup-area order `(position, move range)` (`m_pos`/`m_moveRange`).
    pub cleanup_area: Option<(crate::math::Coord3D, f32)>,
    /// `CommandButtonHuntUpdate`: the command to hunt with.
    pub hunt_command: Option<crate::modules4::HuntCommand>,
    /// `CommandButtonHuntUpdate`'s heap slot: `setCommandButton` wakes it (`CommandButtonHuntUpdate.cpp:137`).
    pub hunt_handle: Option<crate::schedule::UpdateHandle>,
    /// The `SpecialAbilityUpdate` modules of this object: each module's index in the template's
    /// `special_abilities` list and its heap slot (`initiateIntentToDoSpecialPower` wakes it,
    /// `SpecialAbilityUpdate.cpp:556`).
    pub special_ability_handles: Vec<(usize, crate::schedule::UpdateHandle)>,
    /// `AutoFindHealingUpdate::m_nextScanFrames`: the frames left before the next heal-pad scan.
    pub auto_find_scan_frames: u32,
    /// `GenerateMinefieldBehavior`: latch and the mines it created.
    pub minefield_generated: bool,
    pub minefield_upgraded: bool,
    pub minefield_mines: Vec<ObjectId>,
    /// `TransitionDamageFX`: the last body damage state seen (for the client FX hook).
    pub transition_state: u8,
    /// `FireWeaponCollide::m_everFired` (`FireOnce`).
    pub fire_collide_fired: bool,
    /// `HijackerUpdate`: the vehicle the hijacker is riding in (`m_targetID` / `m_isInVehicle`),
    /// the last eject position (`m_ejectPos`) and whether the target was airborne
    /// (`m_wasTargetAirborne`, parachute on eject).
    pub hijacker_vehicle: Option<ObjectId>,
    pub hijacker_eject_pos: crate::math::Coord3D,
    pub hijacker_was_airborne: bool,
    /// `RadarUpdate::m_extendDoneFrame` (`RadarUpdate.cpp:75`): the frame `extendRadar` finishes on
    /// (0 = no extension running). `RADAR_EXTENDING` is derived from it.
    pub radar_extend_done_frame: u32,
    /// `RadarUpdate::m_extendComplete`: the extension finished, so `RADAR_UPGRADED` is on.
    pub radar_extend_complete: bool,
}

impl BehaviorState {
    pub(crate) fn crc(&self, c: &mut crate::crc::Crc) {
        // The first AutoHeal module's bytes are the single-module ones (0 and false without a module), so an object
        // with at most one module hashes as before; each further module adds its own pair. This is a port-internal
        // digest: the C++ `GameLogic::crc` snapshots each object (GameLogic.cpp:4186), and `Object::crc` has no
        // behaviour-module loop (Object.cpp:3935-4043). The save format is `AutoHealBehavior::xfer` (AutoHealBehavior.cpp:
        // 325-348, reached from Object.cpp:4327-4345), which this digest does not follow. Lane 185 (R22-7).
        let (next, started) = self.auto_heal.first().map_or((0, false), |a| (a.next, a.started));
        c.add_u32(next);
        c.add_bool(started);
        for a in self.auto_heal.iter().skip(1) {
            c.add_u32(a.next);
            c.add_bool(a.started);
        }
        c.add_u32(self.aflame_until);
        c.add_u32(self.burned_until);
        c.add_u32(self.flame_damage_next);
        c.add_f32(self.flame_damage_accum);
        c.add_u32(self.flame_last_damage);
        c.add_u32(self.poison_until);
        c.add_u32(self.poison_next);
        c.add_f32(self.poison_amount);
        // Only hashed when set, so objects that never burned or were poisoned keep their historical CRC.
        if self.flame_burned {
            c.add_bool(true);
        }
        if let Some(d) = self.poison_death {
            c.add_u32(u32::from(d as u8) + 1);
        }
        c.add_u32(self.lifetime_die);
        c.add_u32(self.deletion_die);
        // Only hashed for an object with a DynamicShroudClearingRangeUpdate (its constructor leaves the state at
        // DSCRU_NOT_STARTED_YET and the countdown at `shrinkDelay + shrinkTime`; every other object keeps 0), so
        // objects without the module keep their historical CRC.
        if self.ds_state != 0 || self.ds_state_countdown != 0 {
            c.add_u32(u32::from(self.ds_state));
            c.add_u32(self.ds_state_countdown as u32);
            c.add_u32(self.ds_total_frames as u32);
            c.add_u32(self.ds_shrink_start_deadline);
            c.add_u32(self.ds_grow_start_deadline);
            c.add_u32(self.ds_sustain_deadline);
            c.add_u32(self.ds_done_forever_frame);
            c.add_u32(self.ds_change_interval_countdown);
            c.add_f32(self.ds_native_clearing_range);
            c.add_f32(self.ds_current_clearing_range);
        }
        c.add_u32(self.stealth_allowed);
        c.add_bool(self.stealthed);
        c.add_u32(self.detected_until);
        // Only hashed once a grant has run, so objects never granted keep their historical CRC.
        if let Some(enabled) = self.stealth_enabled {
            c.add_bool(true);
            c.add_bool(enabled);
            c.add_u32(self.stealth_frames_granted);
        }
        c.add_u32(self.detect_next);
        c.add_u32(self.deposit_next);
        // Only hashed while it is armed, so every other object keeps its historical CRC. `m_initialized` only
        // changes together with the arm.
        if self.deposit_initial_bonus_armed {
            c.add_bool(true);
        }
        c.add_u32(self.fire_next);
        c.add_bool(self.horde);
        // Only hashed for a `HordeUpdate` object (its constructor always sets a non-zero wake), so every other
        // object keeps its historical CRC.
        if self.horde_wake != 0 {
            c.add_bool(self.horde_true_member);
            c.add_u32(self.horde_last_refresh);
        }
        c.add_u32(self.horde_wake);
        c.add_bool(self.toppled);
        c.add_u32(self.topple_kill_frame);
        c.add_u32(self.propaganda_next);
        c.add_u32(self.repair_next);
        // Only hashed when set, so uncaptured tech buildings keep their historical CRC.
        if self.tech_captured {
            c.add_bool(true);
        }
        c.add_bool(self.overcharge);
        // Only hashed when set, so unupgraded power plants keep their historical CRC.
        if self.power_plant_extended {
            c.add_bool(true);
        }
        // Only hashed when set, so objects without the upgrade keep their historical CRC.
        if self.power_plant_upgraded {
            c.add_bool(true);
        }
        c.add_u32(self.prone_frames);
        c.add_u32(self.cm_reload_until);
        c.add_u32(self.cm_flares.len() as u32);
        for f in &self.cm_flares {
            c.add_u32(f.0);
        }
        c.add_u32(self.cm_available);
        c.add_u32(self.cm_active);
        c.add_u32(self.cm_reaction_frame);
        c.add_u32(self.cm_next_volley_frame);
        c.add_u32(self.cm_reload_frame);
        c.add_u32(self.hole_worker.map_or(0xffff_ffff, |x| x.0));
        c.add_u32(self.hole_reconstructing.map_or(0xffff_ffff, |x| x.0));
        c.add_u32(self.hole_worker_wait);
        c.add_u32(self.hole_spawner.map_or(0xffff_ffff, |x| x.0));
        match &self.hole_rebuild_template {
            Some(n) => {
                c.add_bool(true);
                c.add_str(n);
            }
            None => c.add_bool(false),
        }
        c.add_bool(self.trap_proximity);
        c.add_u32(self.trap_next_scan);
        c.add_bool(self.trap_detonated);
        c.add_u32(self.cleanup_next_scan);
        c.add_u32(self.cleanup_target.map_or(0xffff_ffff, |x| x.0));
        c.add_bool(self.cleanup_in_range);
        match &self.cleanup_area {
            Some((p, r)) => {
                c.add_bool(true);
                c.add_coord(p);
                c.add_f32(*r);
            }
            None => c.add_bool(false),
        }
        match &self.hunt_command {
            Some(crate::modules4::HuntCommand::Weapon) => c.add_u32(1),
            Some(crate::modules4::HuntCommand::SpecialPower { power, object_type }) => {
                c.add_u32(2);
                c.add_str(power);
                c.add_str(object_type);
            }
            Some(crate::modules4::HuntCommand::Enter { kind, object_type }) => {
                c.add_u32(3);
                let k = match kind {
                    crate::enter_actions::EnterKind::HijackVehicle => 1u32,
                    crate::enter_actions::EnterKind::ConvertToCarBomb => 2u32,
                    crate::enter_actions::EnterKind::SabotageBuilding => 3u32,
                };
                c.add_u32(k);
                c.add_str(object_type);
            }
            None => c.add_u32(0),
        }
        c.add_u32(self.auto_find_scan_frames);
        c.add_bool(self.minefield_generated);
        c.add_bool(self.minefield_upgraded);
        c.add_u32(self.minefield_mines.len() as u32);
        for m in &self.minefield_mines {
            c.add_u32(m.0);
        }
        c.add_u32(u32::from(self.transition_state));
        if self.fire_collide_fired {
            c.add_bool(true);
        }
        // Only hashed when set, so objects without a hijacker keep their historical CRC.
        if let Some(v) = self.hijacker_vehicle {
            c.add_bool(true);
            c.add_u32(v.0);
            c.add_coord(&self.hijacker_eject_pos);
            c.add_bool(self.hijacker_was_airborne);
        }
        // Only hashed once a `RadarUpgrade` has extended the dish, so every other object keeps its
        // historical CRC.
        if self.radar_extend_done_frame != 0 || self.radar_extend_complete {
            c.add_u32(self.radar_extend_done_frame);
            c.add_bool(self.radar_extend_complete);
        }
    }
}

impl GameLogic {
    /// Whether `id` is logically stealthed (`OBJECT_STATUS_STEALTHED` and not detected).
    pub fn is_stealthed(&self, id: ObjectId) -> bool {
        self.objects.get(&id).is_some_and(|o| o.behavior.stealthed && self.frame >= o.behavior.detected_until)
    }

    /// Mark `id` detected for `frames` (`StealthUpdate::markAsDetected`, `StealthUpdate.cpp:866-932`). `frames == 0`
    /// uses the stealth owner's `StealthDelay`; a positive value keeps the longer expiry. With
    /// `OrderIdleEnemiesToAttackMeUponReveal`, every enemy object that can see the victim (3D centre distance within
    /// its own vision range) is woken (`setWakeupIfInRange`, `:837-862`) so an idle observer checks for a target
    /// now (`wakeUpAndAttemptToTarget`, `AIUpdate.cpp:4483`).
    pub fn mark_detected(&mut self, id: ObjectId, frames: u32) {
        let now = self.frame;
        let Some((player, pos, own)) = self.objects.get(&id).map(|o| (o.player, o.position, o.template.stealth.clone())) else {
            return;
        };
        // `calcStealthOwner` (`StealthUpdate.cpp:547`) picks the rider with `UseRiderStealth`; the reveal rules are
        // its module data (`:883-888`). A rider without a stealth module leaves the C++ values uninitialised; the
        // object's own module is used instead.
        let owner = self.stealth_owner(id);
        let rules = self.objects.get(&owner).and_then(|o| o.template.stealth.clone()).or(own);
        let Some(rules) = rules else { return };
        if let Some(o) = self.objects.get_mut(&id) {
            let until = now.saturating_add(if frames == 0 { rules.stealth_delay } else { frames });
            if frames == 0 || o.behavior.detected_until < until {
                o.behavior.detected_until = until;
            }
        }
        if !rules.order_idle_enemies_to_attack_on_reveal {
            return;
        }
        // "walk the play list, and for each player that considers us an enemy, check if any of their units can see
        // us" (`:914-930`): `iterateObjects(setWakeupIfInRange)`.
        let woken: Vec<ObjectId> = self
            .objects
            .values()
            .filter(|o| {
                !o.destroyed
                    && o.id != id
                    // `obj->getAI()` (`StealthUpdate.cpp:839-843`): the object has an
                    // `AIUpdateInterface`-derived module, which `Object::Object` records from the
                    // template's modules (`Object.cpp:439-446`). A weapon alone is not enough.
                    && o.template.ai_update.is_some()
                    && self.relationship(o.player, player) == crate::damage::Relationship::Enemies
            })
            .filter(|o| {
                // `Real vision = obj->getVisionRange(); srcpos.sub(dstpos); if (srcpos.length() > vision) return;`
                let (dx, dy, dz) = (o.position.x - pos.x, o.position.y - pos.y, o.position.z - pos.z);
                crate::math::sqrt(dx * dx + dy * dy + dz * dz) <= o.template.vision_range
            })
            .map(|o| o.id)
            .collect();
        for w in woken {
            self.wake_up_and_attempt_to_target(w);
        }
    }

    /// The `update()` pass for the behavior modules that are still id walks. AutoHeal, Flammable, Poisoned and
    /// `DeletionUpdate` are sleepy-heap families now (`sleepy_families.rs`); `LifetimeUpdate` is still this sweep.
    pub(crate) fn update_behaviors(&mut self) {
        let now = self.frame;
        let ids: Vec<ObjectId> = self.objects.values().filter(|o| !o.destroyed).map(|o| o.id).collect();
        for id in ids {
            self.update_lifetime(id, now);
        }
        self.update_hijackers(now);
    }

    /// `HijackerUpdate::update` (`HijackerUpdate.cpp:71`): while in a vehicle, mirror the hijacker
    /// to it and share veterancy; when the vehicle dies, eject the hijacker (under a parachute if
    /// the vehicle was airborne).
    fn update_hijackers(&mut self, _now: u32) {
        let ids: Vec<ObjectId> = self
            .objects
            .values()
            .filter(|o| !o.destroyed && o.behavior.hijacker_vehicle.is_some())
            .map(|o| o.id)
            .collect();
        for id in ids {
            let Some(target_id) = self.objects.get(&id).and_then(|o| o.behavior.hijacker_vehicle) else { continue };
            let target_alive = self.objects.get(&target_id).is_some_and(|o| !o.destroyed);
            if target_alive {
                let (tpos, airborne) = {
                    let Some(t) = self.objects.get(&target_id) else { continue };
                    let ground = self.terrain.surface_height(t.position.x, t.position.y, t.position.z);
                    (t.position, t.position.z - ground > -(9.0) * self.rules.gravity)
                };
                if let Some(o) = self.objects.get_mut(&id) {
                    o.set_position(tpos);
                    o.behavior.hijacker_eject_pos = tpos;
                    o.behavior.hijacker_was_airborne = airborne;
                }
                // `ExperienceTracker`: the higher of the two veterancy levels is kept.
                let target_level = self.objects.get(&target_id).map(|o| o.experience.level);
                let jacker_level = self.objects.get(&id).map(|o| o.experience.level);
                if let (Some(t), Some(j)) = (target_level, jacker_level) {
                    let max = crate::experience::VeterancyLevel::from_index(t.index().max(j.index()));
                    self.set_veterancy_level(target_id, max);
                    self.set_veterancy_level(id, max);
                }
            } else {
                let (eject_pos, was_airborne, parachute, player) = match self.objects.get(&id) {
                    Some(o) => (
                        o.behavior.hijacker_eject_pos,
                        o.behavior.hijacker_was_airborne,
                        o.template.hijacker.as_ref().and_then(|h| h.parachute.clone()),
                        o.player,
                    ),
                    None => continue,
                };
                if let Some(o) = self.objects.get_mut(&id) {
                    o.behavior.hijacker_vehicle = None;
                    o.behavior.hijacker_was_airborne = false;
                    o.set_position(eject_pos);
                }
                // `ai->aiIdle(CMD_FROM_AI)` (`HijackerUpdate.cpp:122-126`): the hijacker becomes a pedestrian
                // again through the engine's `privateIdle` (`AIUpdate.cpp:3092-3117`).
                self.ai_idle(id, crate::ai_states::CommandSource::Ai);
                if was_airborne {
                    if let Some(p) = parachute {
                        let container = self.create_object(&p, eject_pos, 0.0, player);
                        if self.objects.contains_key(&container) {
                            self.add_to_contain(container, id);
                        }
                    }
                }
            }
        }
    }

    /// `LifetimeUpdate::update`: kill the object when its lifetime (rolled at creation) elapses.
    fn update_lifetime(&mut self, id: ObjectId, now: u32) {
        let die = match self.objects.get(&id) {
            Some(o) if o.behavior.lifetime_die != 0 && !o.is_dying() => o.behavior.lifetime_die,
            _ => return,
        };
        if now >= die {
            self.damage_object_with_death(
                id,
                DamageInfo { source: ObjectId::INVALID, amount: HUGE_DAMAGE_AMOUNT, kind: DamageType::Unresistable },
                DeathType::Normal,
            );
        }
    }

    /// `StealthUpdate::calcStealthOwner` (`StealthUpdate.cpp:547`): with `UseRiderStealth` and a rider
    /// aboard, the first rider owns the stealth rules; otherwise `id` itself.
    pub(crate) fn stealth_owner(&self, id: ObjectId) -> ObjectId {
        let use_rider = self.objects.get(&id).and_then(|o| o.template.stealth.as_ref()).is_some_and(|t| t.use_rider_stealth);
        if use_rider {
            if let Some(&rider) = self.contained_units(id).first() {
                return rider;
            }
        }
        id
    }

    /// `StealthUpdate::receiveGrant` (`StealthUpdate.cpp:193-245`): set `m_enabled` to `active` and turn
    /// `CAN_STEALTH` and `STEALTHED` on or off with it (`:208-229`), for `frames` (0 = until switched off).
    /// A `RiderChangeContain` bike passes the grant to its first rider (`:231-241`), so the rider's stealth
    /// follows the bike's. Called by the supply centre's temporary grant (`SupplyCenterProductionExitUpdate.cpp:132-145`);
    /// `GrantStealthBehavior`, `HelixContain`, `OverlordContain` and `SlavedUpdate` are not ported.
    pub fn stealth_receive_grant(&mut self, id: ObjectId, active: bool, frames: u32) {
        let Some(t) = self.objects.get(&id).and_then(|o| o.template.stealth.clone()) else { return };
        // `canDisguise()`: a team-disguised unit (bomb truck) is not foiled by a grant.
        if t.disguises_as_team {
            return;
        }
        let now = self.frame;
        if let Some(o) = self.objects.get_mut(&id) {
            o.behavior.stealth_enabled = Some(active);
            if active {
                o.status_bits |= status::CAN_STEALTH;
                o.behavior.stealthed = true;
                o.behavior.stealth_allowed = now;
                o.behavior.stealth_frames_granted = frames;
            } else {
                o.status_bits &= !status::CAN_STEALTH;
                o.behavior.stealthed = false;
                // `m_stealthAllowedFrame = FOREVER`.
                o.behavior.stealth_allowed = u32::MAX;
                o.behavior.stealth_frames_granted = 0;
            }
        }
        // `setWakeFrame(obj, UPDATE_SLEEP_NONE)` for an active grant (StealthUpdate.cpp:215).
        if active {
            self.wake_stealth_update(id);
        }
        // `friend_getRider` (the first rider in the list) on a `RiderChangeContain`.
        let rider = if self.is_rider_change(id) { self.contained_units(id).first().copied() } else { None };
        if let Some(rider) = rider {
            self.stealth_receive_grant(rider, active, frames);
        }
    }

    /// `StealthUpgrade::upgradeImplementation` (`StealthUpgrade.cpp:51-70`): set
    /// `OBJECT_STATUS_CAN_STEALTH` on the upgraded object, and on every spawn of a
    /// `KINDOF_SPAWNS_ARE_THE_WEAPONS` spawner (`SpawnBehavior::giveSlavesStealthUpgrade`,
    /// SpawnBehavior.cpp:478-488, which iterates the module's own `m_spawnIDs`). "The logic that does the
    /// stealthupdate will notice this and start stealthing" — the awake module reads the owner's bit in
    /// `allowedToStealth`, so nothing else has to wake. Called once per object when its `StealthUpgrade`
    /// module first activates.
    pub(crate) fn stealth_upgrade_activate(&mut self, id: ObjectId) {
        let Some(o) = self.objects.get(&id) else { return };
        let spawner = o.is_kind_of(crate::template::kind_of::SPAWNS_ARE_THE_WEAPONS);
        let spawned = o.spawned.clone();
        if let Some(o) = self.objects.get_mut(&id) {
            o.status_bits |= status::CAN_STEALTH;
        }
        if spawner {
            for slave in spawned {
                if self.objects.get(&slave).is_some_and(|o| !o.destroyed) {
                    if let Some(o) = self.objects.get_mut(&slave) {
                        o.status_bits |= status::CAN_STEALTH;
                    }
                }
            }
        }
    }

    /// `StealthUpdate::update`: engage/break stealth (detection is applied by the detector pass).
    /// `StealthUpdate::update` (StealthUpdate.cpp:584-840) for one object, run from the sleepy heap. Returns
    /// `calcSleepTime()`: `UPDATE_SLEEP_NONE` while `m_enabled`, `UPDATE_SLEEP_FOREVER` once it is off
    /// (`:576-579`).
    pub(crate) fn stealth_update(&mut self, id: ObjectId, now: u32) -> u32 {
        {
            let Some(t) = self.objects.get(&id).and_then(|o| o.template.stealth.clone()) else { return UPDATE_SLEEP_FOREVER };
            // `if (!m_enabled) return calcSleepTime();` (`:633-636`): a `GrantedBySpecialPower` module
            // sleeps at FOREVER from its constructor (`:154-155`) until a grant wakes it (`:215`), and a
            // team-disguised one starts off (`:132`).
            let self_enabled = self.objects.get(&id).is_some_and(|o| stealth_enabled(o, &t));
            if !self_enabled {
                return UPDATE_SLEEP_FOREVER;
            }
            // `getRevealDistanceFromTarget()` (`:694-713`): close enough to the current victim, the unit
            // reveals itself (`markAsDetected()` with no frame count = the stealth owner's `StealthDelay`).
            // `AIUpdateInterface::getCurrentVictim` is the attack order's target in the port.
            if t.reveal_distance_from_target > 0.0 {
                let victim = self.objects.get(&id).and_then(|o| match o.combat.attack.as_ref().map(|a| a.target) {
                    Some(crate::combat::AttackTarget::Object(v)) => Some(v),
                    _ => None,
                });
                let pair = victim.and_then(|v| self.objects.get(&id).zip(self.objects.get(&v)).map(|(self_o, target)| (self_o.position, target.position)));
                if let Some((self_pos, target_pos)) = pair {
                    let (dx, dy) = (self_pos.x - target_pos.x, self_pos.y - target_pos.y);
                    if dx * dx + dy * dy <= t.reveal_distance_from_target * t.reveal_distance_from_target {
                        self.mark_detected(id, 0);
                        return UPDATE_SLEEP_NONE;
                    }
                }
            }
            // Temporary grant (`:715-734`): the countdown, and the player-ordered unit loses it at once.
            let left = self.objects.get(&id).map_or(0, |o| o.behavior.stealth_frames_granted);
            if left > 0 {
                if let Some(o) = self.objects.get_mut(&id) {
                    o.behavior.stealth_frames_granted = left - 1;
                }
                // "If the last AI command given was by the player... then LOSE the stealth now!" (`:720-729`).
                if self.objects.get(&id).is_some_and(|o| o.ai.machine.last_command_source == crate::ai_states::CommandSource::Player) {
                    self.stealth_receive_grant(id, false, 0);
                }
                if left == 1 {
                    // `if (m_framesGranted == 0) receiveGrant(FALSE);` (`:730-734`).
                    self.stealth_receive_grant(id, false, 0);
                }
                // The C++ runs on through `allowedToStealth` after that turning-off grant, whose
                // `CAN_STEALTH` test only repeats the clear; its final `calcSleepTime()` is FOREVER.
                if self.objects.get(&id).is_some_and(|o| !stealth_enabled(o, &t)) {
                    return UPDATE_SLEEP_FOREVER;
                }
            }
            // `calcStealthOwner` (StealthUpdate.cpp:547): with `UseRiderStealth` the first rider is the
            // owner. Its `CAN_STEALTH` (`allowedToStealth`, StealthUpdate.cpp:309), stealth level and
            // `StealthDelay` decide; otherwise this object's own rules do. No stealth on the owner means
            // no stealth at all.
            let owner = self.stealth_owner(id);
            // `UnsignedInt flags = data->m_stealthLevel;` stays this module's when the owner has no
            // stealth module (`:255-266`); the delay is read the same way (`:610-626`).
            let owner_rules = self
                .objects
                .get(&owner)
                .and_then(|o| o.template.stealth.as_ref().map(|s| (s.stealth_delay, s.forbidden_conditions)));
            let (owner_delay, owner_forbidden) = owner_rules.unwrap_or((t.stealth_delay, t.forbidden_conditions));
            // `allowedToStealth`'s `stealthOwner->getStatusBits().test(OBJECT_STATUS_CAN_STEALTH)`
            // (`:309-312`): the innate constructor sets the bit (`:147-151`), `receiveGrant` sets and
            // clears it with `STEALTHED` (`:213`, `:221`). Without it the else branch clears `STEALTHED`
            // and sets `m_stealthAllowedFrame = now + stealthDelay` (`:755-759`).
            let owner_can = self.objects.get(&owner).is_some_and(|o| o.status_bits & status::CAN_STEALTH != 0);
            if !owner_can {
                if let Some(o) = self.objects.get_mut(&id) {
                    o.behavior.stealthed = false;
                    o.behavior.stealth_allowed = now.saturating_add(owner_delay);
                }
                return UPDATE_SLEEP_NONE;
            }
            let (allowed, status_bits) = match self.objects.get(&id) {
                Some(o) if !o.is_dying() => (o.behavior.stealth_allowed, o.status_mask(now)),
                _ => return UPDATE_SLEEP_NONE,
            };
            let mut ok = !t.disguises_as_team;
            if t.required_status != 0 && status_bits & t.required_status != t.required_status {
                ok = false;
            }
            if t.forbidden_status != 0 && status_bits & t.forbidden_status != 0 {
                ok = false;
            }
            let (velocity, firing) = match self.objects.get(&id) {
                Some(o) => (
                    o.physics.as_ref().map_or(0.0, |p| p.velocity_magnitude()),
                    o.status_bits & status::IS_FIRING_WEAPON != 0,
                ),
                None => (0.0, false),
            };
            // The stealth level (`flags`) is the owner's (`allowedToStealth`, StealthUpdate.cpp:255).
            // `STEALTH_NOT_WHILE_MOVING` reads the physics velocity against this module's
            // `MoveThresholdSpeed` (`:399-402`), not the AI's move order.
            if owner_forbidden & stealth_forbidden::MOVING != 0 && velocity > t.move_threshold_speed {
                ok = false;
            }
            // `STEALTH_NOT_WHILE_ATTACKING` tests `OBJECT_STATUS_IS_FIRING_WEAPON` (`StealthUpdate.cpp:283`),
            // which the attack sub-machine sets in `AIAttackFireWeaponState::onEnter` and clears in its
            // `onExit` (`AIStates.cpp:5203`, `:5347`) — an attack order alone is not enough.
            if owner_forbidden & stealth_forbidden::ATTACKING != 0 && firing {
                ok = false;
            }
            // `STEALTH_NOT_WHILE_FIRING_WEAPON` (`StealthUpdate.cpp:340-397`): the all-three case tests the
            // firing bit alone; otherwise each weapon slot's `getLastShotFrame() >= now - 1`.
            if owner_forbidden & stealth_forbidden::FIRING_WEAPON != 0 && firing {
                if owner_forbidden & stealth_forbidden::FIRING_WEAPON == stealth_forbidden::FIRING_WEAPON {
                    ok = false;
                } else {
                    let last_frame = now.saturating_sub(1);
                    let shot_recently = [
                        (stealth_forbidden::FIRING_PRIMARY, 0usize),
                        (stealth_forbidden::FIRING_SECONDARY, 1),
                        (stealth_forbidden::FIRING_TERTIARY, 2),
                    ]
                    .into_iter()
                    .filter(|(bit, _)| owner_forbidden & bit != 0)
                    .any(|(_, slot)| {
                        self.objects
                            .get(&id)
                            .and_then(|o| o.combat.weapons.weapons.get(slot))
                            .and_then(|w| w.as_ref())
                            .is_some_and(|w| w.last_fire_frame() >= last_frame)
                    });
                    if shot_recently {
                        ok = false;
                    }
                }
            }
            let engaged = ok && now >= allowed;
            if let Some(o) = self.objects.get_mut(&id) {
                o.behavior.stealthed = engaged;
                if !ok {
                    // `m_stealthAllowedFrame = now + stealthDelay` (the owner's delay, StealthUpdate.cpp:759).
                    o.behavior.stealth_allowed = now.saturating_add(owner_delay);
                }
            }
        }
        // `calcSleepTime()` with `m_enabled` on (StealthUpdate.cpp:578).
        UPDATE_SLEEP_NONE
    }

    /// `StealthDetectorUpdate::update` (StealthDetectorUpdate.cpp:143-411) for one detector, run from the sleepy
    /// heap: the module gates of `:143-184`, the sweep over the `PartitionFilterStealthedOrStealthGarrisoned` set
    /// (`:184-352`), then `UPDATE_SLEEP(updateRate)` (`:411`). `fauna`/ping FX, radar events and the EVA messages
    /// are client-only.
    pub(crate) fn stealth_detector_update(&mut self, id: ObjectId, now: u32) -> u32 {
        let Some(d) = self.objects.get(&id).and_then(|o| o.template.stealth_detector.clone()) else {
            return UPDATE_SLEEP_FOREVER;
        };
        // `if (self->isEffectivelyDead()) return UPDATE_SLEEP_FOREVER;` (`:148-149`).
        if self.is_effectively_dead(id) {
            return UPDATE_SLEEP_FOREVER;
        }
        // `if (self->testStatus(OBJECT_STATUS_UNDER_CONSTRUCTION)) return UPDATE_SLEEP_NONE;` (`:152-153`):
        // "we will detect the moment we finish".
        if self.objects.get(&id).is_some_and(|o| o.under_construction) {
            return UPDATE_SLEEP_NONE;
        }
        // `if (self->testStatus(OBJECT_STATUS_SOLD)) return UPDATE_SLEEP_FOREVER;` (`:156-157`).
        if self.objects.get(&id).is_some_and(|o| o.status_mask(now) & status::SOLD != 0) {
            return UPDATE_SLEEP_FOREVER;
        }
        // A contained detector only detects with its matching flag (`:159-182`): in a garrisonable
        // container it needs `CanDetectWhileGarrisoned`, in any other container `CanDetectWhileContained`.
        if let Some(container) = self.objects.get(&id).and_then(|o| o.contained_by) {
            let garrisonable = self
                .objects
                .get(&container)
                .and_then(|c| c.template.contain.as_ref())
                .is_some_and(|c| c.is_garrison());
            if garrisonable {
                if !d.can_detect_while_garrisoned {
                    return d.detection_rate.max(1);
                }
            } else if !d.can_detect_while_contained {
                return d.detection_rate.max(1);
            }
        }
        {
            // The C++ relies on the heap's wake (`:411`); this keeps an early wake from sweeping.
            let next = self.objects.get(&id).map_or(0, |o| o.behavior.detect_next);
            if next != 0 && now < next {
                return next - now;
            }
            let (pos, player, vision) = match self.objects.get(&id) {
                Some(o) => (o.position, o.player, o.template.vision_range),
                None => return UPDATE_SLEEP_FOREVER,
            };
            let range = if d.detection_range > 0.0 { d.detection_range } else { vision };
            // `filters[] = { &filterStealthOrStealthGarrisoned, &filterTeam, &filterKindof, &filterMapStatus }`
            // (`:184-190`). The relationship filter allows enemies and neutrals only; the port has a single map,
            // so every object passes the map-status test. Iteration is in object-id order, not the partition's
            // spatial order (the sweep's effects are per victim).
            let mut victims: Vec<ObjectId> = Vec::new();
            for o in self.objects.values() {
                if o.destroyed || o.id == id {
                    continue;
                }
                // `PartitionFilterStealthedOrStealthGarrisoned` (`:125-138`): `OBJECT_STATUS_STEALTHED`, or a
                // garrisonable container with stealth-garrison riders.
                let stealth_garrison = o.template.contain.as_ref().is_some_and(|c| c.is_garrison())
                    && self
                        .contained_units(o.id)
                        .iter()
                        .any(|r| self.objects.get(r).is_some_and(|r| r.template.is_kind_of(crate::template::kind_of::STEALTH_GARRISON)));
                if !o.behavior.stealthed && !stealth_garrison {
                    continue;
                }
                // `PartitionFilterRelationship(ALLOW_ENEMIES | ALLOW_NEUTRAL)` (`:187`).
                if self.relationship(player, o.player) == crate::damage::Relationship::Allies {
                    continue;
                }
                // `PartitionFilterAcceptByKindOf(m_extraDetectKindof, m_extraDetectKindofNot)` (`:188`).
                if d.extra_required_kind_of != 0 && !o.template.is_kind_of(d.extra_required_kind_of) {
                    continue;
                }
                if d.extra_forbidden_kind_of != 0 && o.template.is_kind_of(d.extra_forbidden_kind_of) {
                    continue;
                }
                if o.position.dist_2d(&pos) > range {
                    continue;
                }
                victims.push(o.id);
            }
            for victim in victims {
                // `if (them->isEffectivelyDead()) continue;` (`:204-205`).
                if self.is_effectively_dead(victim) {
                    continue;
                }
                // `StealthUpdate *stealth = them->getStealth(); if (stealth) { ... }` (`:207-327`): "updateRate
                // PLUS 1 is necessary to ensure it stays detected 'till we are called again" (`:301-302`).
                if self.objects.get(&victim).is_some_and(|o| o.template.stealth.is_some()) {
                    self.mark_detected(victim, d.detection_rate.saturating_add(1));
                    continue;
                }
                // `else // perhaps they are garrisoning something stealthy, eh?` (`:328-351`): the garrison's
                // own stealth riders are revealed for two frames more (`:345`), unless the rider is the
                // detector's player or an ally (`:343`).
                let is_garrison = self
                    .objects
                    .get(&victim)
                    .and_then(|o| o.template.contain.as_ref())
                    .is_some_and(|c| c.is_garrison());
                if !is_garrison {
                    continue;
                }
                let riders: Vec<ObjectId> = self.contained_units(victim).to_vec();
                for rider in riders {
                    if self.objects.get(&rider).and_then(|o| o.template.stealth.as_ref()).is_none() {
                        continue;
                    }
                    let hits = self.objects.get(&rider).is_some_and(|o| {
                        o.player != player && self.relationship(player, o.player) != crate::damage::Relationship::Allies
                    });
                    if hits {
                        self.mark_detected(rider, d.detection_rate.saturating_add(2));
                    }
                }
            }
            if let Some(o) = self.objects.get_mut(&id) {
                o.behavior.detect_next = now.saturating_add(d.detection_rate.max(1));
            }
        }
        // `UPDATE_SLEEP(data->m_updateRate)` (StealthDetectorUpdate.cpp:411).
        d.detection_rate.max(1)
    }

    /// `FireWeaponWhenDeadBehavior::onDie`: fire the death weapon when applicable.
    pub(crate) fn fire_weapon_when_dead(&mut self, id: ObjectId, death: DeathType, _overkill: f32) {
        let Some(t) = self.objects.get(&id).and_then(|o| o.template.fire_weapon_when_dead.clone()) else { return };
        if !self.behavior_upgrade_active(id, t.starts_active, &t.triggered_by, &t.conflicts_with) {
            return;
        }
        let Some(obj) = self.objects.get(&id) else { return };
        if obj.under_construction || t.death_types & (1u32 << (death as u32)) == 0 {
            return;
        }
        self.fire_temp_weapon(id, t.weapon);
    }

    /// `UpgradeMux::isUpgradeActive` as the port evaluates it: active when `starts_active` (the constructor's
    /// `giveSelfUpgrade`), or when a `TriggeredBy` upgrade is owned and no `ConflictsWith` one is. With neither, the
    /// module never activates: `UpgradeModule::wouldUpgrade` needs an activation mask (UpgradeModule.cpp:124-150,
    /// `activation.any()` at :130), and FireWeaponWhenDeadBehavior.h:50 / CountermeasuresBehavior.cpp:204 test
    /// `isUpgradeActive()` only.
    pub(crate) fn behavior_upgrade_active(&self, id: ObjectId, starts_active: bool, triggered_by: &[String], conflicts_with: &[String]) -> bool {
        if starts_active {
            return true;
        }
        if triggered_by.is_empty() {
            return false;
        }
        let Some(o) = self.objects.get(&id) else { return false };
        let player = o.player;
        let player_set = self.player_upgrade_set(player);
        let owned = |n: &String| o.upgrades.contains(n) || player_set.is_some_and(|s| s.contains(n));
        triggered_by.iter().any(&owned) && !conflicts_with.iter().any(owned)
    }
}

/// A converted death weapon plus its `DieMuxData` mask and `UpgradeMuxData` gate.
#[derive(Clone, Debug)]
pub struct DeathWeaponTemplate {
    pub weapon: Arc<WeaponTemplate>,
    pub death_types: u32,
    pub starts_active: bool,
    pub triggered_by: Vec<String>,
    pub conflicts_with: Vec<String>,
}

/// Alias kept for the template field type.
pub type DeathWeapon = DeathWeaponTemplate;

/// Lane 172: the hijacker's eject runs `privateIdle`, not the old `stop_object_ai` shortcut (`ai_l172.rs`).
#[cfg(test)]
#[path = "ai_l172.rs"]
mod l172_tests;
