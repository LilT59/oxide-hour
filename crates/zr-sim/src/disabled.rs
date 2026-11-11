//! Per-object disabled state (`Common/DisabledTypes.h`; `Object::setDisabled` /
//! `setDisabledUntil` / `clearDisabled` / `isDisabledByType` / `getDisabledUntil` /
//! `checkDisabledStatus`, Object.cpp:2113-2401).
//!
//! The engine keeps `m_disabledMask` (which types are currently active) beside
//! `m_disabledTillFrame[type]` (when each expires). The sim used to carry only an ad-hoc
//! `Object::disabled_until` plus `Object::unmanned`; both are migrated onto the same
//! mask+till-frame model here. [`Object::disabled_until`] (the `DISABLED_ANY` aggregate) and
//! [`Object::unmanned`] remain as derived accessors so existing callers keep the same behaviour and
//! their CRCs are unchanged while nothing is disabled.

use crate::crc::Crc;
use crate::logic::GameLogic;
use crate::object::{Object, ObjectId, PlayerIndex};
use crate::template::kind_of;

/// `DISABLED_COUNT` (`DisabledTypes.h`).
pub const DISABLED_COUNT: usize = 13;
/// `FOREVER` (`Object.cpp`): a disabled type with no expiry (`setDisabled` uses this).
pub const FOREVER: u32 = u32::MAX;
/// `NEVER` (`Object.cpp`): the not-set / cleared expiry value.
pub const NEVER: u32 = 0;

/// `DisabledType` (`DisabledTypes.h`). The discriminants are the engine's enum order, which is also
/// the bit order of `DisabledMaskType` (`DisabledTypes.cpp` `s_bitNameList`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum DisabledType {
    Default = 0,
    Hacked = 1,
    Emp = 2,
    Held = 3,
    Paralyzed = 4,
    Unmanned = 5,
    Underpowered = 6,
    Freefall = 7,
    Awestruck = 8,
    Brainwashed = 9,
    Subdued = 10,
    ScriptDisabled = 11,
    ScriptUnderpowered = 12,
}

impl DisabledType {
    /// Every value, in engine order.
    pub const ALL: [DisabledType; DISABLED_COUNT] = [
        DisabledType::Default,
        DisabledType::Hacked,
        DisabledType::Emp,
        DisabledType::Held,
        DisabledType::Paralyzed,
        DisabledType::Unmanned,
        DisabledType::Underpowered,
        DisabledType::Freefall,
        DisabledType::Awestruck,
        DisabledType::Brainwashed,
        DisabledType::Subdued,
        DisabledType::ScriptDisabled,
        DisabledType::ScriptUnderpowered,
    ];

    /// The `DisabledMaskType` bit for this type.
    pub const fn bit(self) -> u16 {
        1u16 << (self as u8)
    }

    /// The type at an engine enum index (`DISABLED_COUNT` entries).
    pub fn from_index(i: usize) -> Option<Self> {
        Self::ALL.get(i).copied()
    }
}

/// `AIUpdate.cpp:1082` / `AIStates.cpp:1432`: a unit in any of these disabled states stops moving,
/// does not run its AI state machine and does not auto-acquire.
pub const MOVEMENT_DISABLED_MASK: u16 = DisabledType::Paralyzed.bit()
    | DisabledType::Unmanned.bit()
    | DisabledType::Emp.bit()
    | DisabledType::Subdued.bit()
    | DisabledType::Hacked.bit();

impl Object {
    /// `Object::m_disabledMask` (`Object::getDisabledFlags`, Object.h:586).
    pub fn disabled_flags(&self) -> u16 {
        self.disabled_mask
    }

    /// `Object::isDisabled` (`m_disabledMask.any()`).
    pub fn is_disabled(&self) -> bool {
        self.disabled_mask != 0
    }

    /// `Object::isDisabledByType`.
    pub fn is_disabled_by_type(&self, t: DisabledType) -> bool {
        self.disabled_mask & t.bit() != 0
    }

    /// `Object::getDisabledUntil(type)` when the type is active, else `NEVER`.
    pub fn get_disabled_until(&self, t: DisabledType) -> u32 {
        if self.is_disabled_by_type(t) {
            self.disabled_till_frame[t as usize]
        } else {
            NEVER
        }
    }

    /// `Object::getDisabledUntil(DISABLED_ANY)`: the latest expiry among the active types (0 when
    /// nothing is disabled).
    pub fn disabled_until(&self) -> u32 {
        let mut highest = NEVER;
        for t in DisabledType::ALL {
            if self.is_disabled_by_type(t) && self.disabled_till_frame[t as usize] > highest {
                highest = self.disabled_till_frame[t as usize];
            }
        }
        highest
    }

    /// `Object::isDisabledByType(DISABLED_UNMANNED)`.
    pub fn unmanned(&self) -> bool {
        self.is_disabled_by_type(DisabledType::Unmanned)
    }

    /// Whether any of the movement/AI-blocking types is active (see [`MOVEMENT_DISABLED_MASK`]).
    pub fn is_movement_disabled(&self) -> bool {
        self.disabled_mask & MOVEMENT_DISABLED_MASK != 0
    }

    /// `Object::setDisabled` calls `pauseAllSpecialPowers` for every type except `DISABLED_HELD`
    /// (Object.cpp:2166, 2410). A disabled type other than `HELD` is set, so the special powers are
    /// paused; `HELD`-only units keep them.
    pub fn special_powers_paused(&self) -> bool {
        self.disabled_mask & !DisabledType::Held.bit() != 0
    }

    /// The retail disabled-to-process test for a module (`GameLogic.cpp:3803`):
    /// `!dis.any() || dis.anyIntersectionWith(types_to_process)`. The SuperHackers exclusive
    /// variant is intentionally not used because it changes retail gameplay (AGENTS.md).
    pub fn update_module_processes(&self, types_to_process: u16) -> bool {
        self.disabled_mask == 0 || self.disabled_mask & types_to_process != 0
    }

    /// `ProductionUpdate`'s module mask is `MAKE_DISABLED_MASK(DISABLED_HELD)`
    /// (ProductionUpdate.cpp:105): a factory disabled by anything else pauses its queue, but a
    /// `HELD` (contained) factory keeps producing.
    pub fn production_module_active(&self) -> bool {
        self.template
            .production
            .as_ref()
            .is_some_and(|p| self.update_module_processes(p.disabled_types_to_process))
    }

    /// `ThingTemplate::findWeaponTemplateSet(WEAPONSET_CARBOMB)` (`Object.cpp:2219`): a sniped
    /// carbomb's dead-man's trigger detonates it instead of leaving it unmanned.
    pub fn has_carbomb_weapon_set(&self) -> bool {
        self.template.weapon_sets.iter().any(|(cond, _)| cond & crate::weapon::WEAPONSET_CARBOMB != 0)
    }

    /// `Object::friend_adjustPowerForPlayer` guard (Object.cpp:3845): a disabled producer
    /// contributes no power. Used by [`GameLogic::energy_tally`].
    pub fn is_disabled_producer(&self) -> bool {
        self.template.energy_production > 0 && self.is_disabled()
    }

    /// The `EnergyBonus` this object currently contributes (`Object.cpp:3895-3928`): once for an
    /// active `PowerPlantUpgrade`, once for active `OverchargeBehavior`.
    pub fn active_energy_bonus(&self) -> i32 {
        let mut sources = 0;
        if self.behavior.power_plant_upgraded {
            sources += 1;
        }
        if self.behavior.overcharge && self.template.overcharge.is_some() {
            sources += 1;
        }
        self.template.energy_bonus * sources
    }

    /// The hash contribution of the disabled state. Only added when a type is active, so games
    /// whose objects are never disabled keep their historical CRC (the derived
    /// [`disabled_until`](Self::disabled_until) / [`unmanned`](Self::unmanned) accessors are hashed
    /// in their place by `Object::crc`).
    pub(crate) fn disabled_crc(&self, c: &mut Crc) {
        if self.disabled_mask == 0 {
            return;
        }
        c.add_u32(u32::from(self.disabled_mask));
        for t in DisabledType::ALL {
            if self.is_disabled_by_type(t) {
                c.add_u32(self.disabled_till_frame[t as usize]);
            }
        }
    }
}

impl GameLogic {
    /// `Object::setDisabled`: disable for [`FOREVER`].
    pub fn set_disabled(&mut self, id: ObjectId, t: DisabledType) {
        self.set_disabled_until(id, t, FOREVER);
    }

    /// `Object::setDisabledUntil` (Object.cpp:2119). The mask bit is only set while the expiry is in
    /// the future (`frame > TheGameLogic->getFrame()`). The disabled edge fires when the object
    /// crosses between "not disabled" and "disabled"; a spawner's slaves (`SPAWNS_ARE_THE_WEAPONS`,
    /// `SpawnBehavior::orderSlavesDisabledUntil`) get the same type and frame.
    pub fn set_disabled_until(&mut self, id: ObjectId, t: DisabledType, frame: u32) {
        let now = self.frame;
        let (changed, now_disabled, pause_edge) = {
            let Some(o) = self.objects.get_mut(&id) else { return };
            if o.destroyed {
                return;
            }
            let was = o.is_disabled();
            let was_pausing = o.special_powers_paused();
            let idx = t as usize;
            if o.disabled_till_frame[idx] != frame {
                o.disabled_till_frame[idx] = frame;
                if frame > now {
                    o.disabled_mask |= t.bit();
                } else {
                    o.disabled_mask &= !t.bit();
                }
            }
            let pausing = o.special_powers_paused();
            (was != o.is_disabled(), o.is_disabled(), (was_pausing != pausing).then_some(pausing))
        };
        // `Object::setDisabledUntil`'s `pauseAllSpecialPowers(TRUE)` edge (Object.cpp:2166).
        if let Some(pausing) = pause_edge {
            self.pause_all_special_powers(id, pausing);
        }
        if changed {
            self.propagate_disabled_to_slaves(id, t, frame);
        }
        // `Object::setDisabledUntil`'s UNMANNED side effects run outside the frame guard
        // (Object.cpp:2214).
        if t == DisabledType::Unmanned && self.objects.get(&id).is_some_and(|o| !o.is_kind_of(kind_of::DRONE)) {
            self.on_unmanned_disabled(id);
        }
        if changed {
            self.on_disabled_edge(id, now_disabled);
        }
    }

    /// `Object::clearDisabled` (Object.cpp:2284): clear `t` if it was set. Returns whether anything
    /// changed. A spawner's slaves are cleared too (`orderSlavesToClearDisabled`).
    pub fn clear_disabled(&mut self, id: ObjectId, t: DisabledType) -> bool {
        let (now_disabled, pause_edge) = {
            let Some(o) = self.objects.get_mut(&id) else { return false };
            if !o.is_disabled_by_type(t) {
                return false;
            }
            let was_pausing = o.special_powers_paused();
            o.disabled_till_frame[t as usize] = NEVER;
            o.disabled_mask &= !t.bit();
            let pausing = o.special_powers_paused();
            (o.is_disabled(), (was_pausing != pausing).then_some(pausing))
        };
        // `Object::setDisabledUntil`'s `pauseAllSpecialPowers(FALSE)` edge (Object.cpp:2325).
        if let Some(pausing) = pause_edge {
            self.pause_all_special_powers(id, pausing);
        }
        self.clear_slaves_disabled(id, t);
        if !now_disabled {
            self.on_disabled_edge(id, false);
        }
        true
    }

    /// `Object::checkDisabledStatus` (Object.cpp:2385), run at the end of the frame: clear every
    /// type whose expiry the current frame has reached, firing the re-enabled edge for each object.
    pub(crate) fn check_disabled_status(&mut self) {
        let now = self.frame;
        let mut re_enabled: Vec<ObjectId> = Vec::new();
        let mut unpaused: Vec<ObjectId> = Vec::new();
        for o in self.objects.values_mut() {
            if o.disabled_mask == 0 {
                continue;
            }
            let was_pausing = o.special_powers_paused();
            for t in DisabledType::ALL {
                if o.is_disabled_by_type(t) && now >= o.disabled_till_frame[t as usize] {
                    o.disabled_mask &= !t.bit();
                    o.disabled_till_frame[t as usize] = NEVER;
                }
            }
            if was_pausing && !o.special_powers_paused() {
                unpaused.push(o.id);
            }
            if !o.is_disabled() {
                re_enabled.push(o.id);
            }
        }
        // `Object::checkDisabledStatus` -> `clearDisabled` -> `pauseAllSpecialPowers(FALSE)`.
        for id in unpaused {
            self.pause_all_special_powers(id, false);
        }
        for id in re_enabled {
            self.on_disabled_edge(id, false);
        }
    }

    /// `SpawnBehavior::orderSlavesDisabledUntil` (SpawnBehavior.cpp:371-386): a `SPAWNS_ARE_THE_WEAPONS`
    /// object disables every spawn it made (the module's own `m_spawnIDs`, `:373`), idling it first.
    pub(crate) fn propagate_disabled_to_slaves(&mut self, id: ObjectId, t: DisabledType, frame: u32) {
        let Some(spawned) = self.objects.get(&id).filter(|o| o.template.spawn.is_some()).map(|o| o.spawned.clone()) else {
            return;
        };
        for s in spawned {
            if !self.objects.get(&s).is_some_and(|o| !o.destroyed) {
                continue;
            }
            // `if (ai) ai->aiIdle(CMD_FROM_AI);` (`SpawnBehavior.cpp:377-381`): `privateIdle` clears the
            // state machine and enters `AI_IDLE`; it does not drop the path (the next update's
            // completed-move block does) and it does not run `ai.stop()`'s shortcut.
            self.ai_idle(s, crate::ai_states::CommandSource::Ai);
            self.set_disabled_until(s, t, frame);
        }
    }

    /// `SpawnBehavior::orderSlavesToClearDisabled` (SpawnBehavior.cpp:389-400).
    fn clear_slaves_disabled(&mut self, id: ObjectId, t: DisabledType) {
        let Some(spawned) = self.objects.get(&id).filter(|o| o.template.spawn.is_some()).map(|o| o.spawned.clone()) else {
            return;
        };
        for s in spawned {
            if self.objects.get(&s).is_some_and(|o| !o.destroyed) {
                self.clear_disabled(s, t);
            }
        }
    }

    /// `Object::onDisabledEdge` (Object.cpp:3865): the object crossed between "not disabled" and
    /// "disabled". Behavior modules get the edge (`DozerAIUpdate` / `WorkerAIUpdate` cancel their
    /// build/repair task; `SpyVisionUpdate` is not ported), then the player's power is adjusted for
    /// a producer. The sim computes power on demand ([`GameLogic::energy_tally`]), which excludes a
    /// disabled producer and adds the active `EnergyBonus`, so the power branch is realized there
    /// rather than by an incremental `Energy`; the radar half of `Player::onPowerBrownOutChange`
    /// runs in [`GameLogic::update_brownout_disabled`].
    pub(crate) fn on_disabled_edge(&mut self, id: ObjectId, becoming_disabled: bool) {
        if becoming_disabled {
            let busy = self.objects.get(&id).is_some_and(|o| o.build_target.is_some() || o.repair_target.is_some());
            if busy {
                if let Some(o) = self.objects.get_mut(&id) {
                    // `DozerAIUpdate::onDisabledEdge` `cancelTask` (retail does not resume it).
                    o.build_target = None;
                    o.repair_target = None;
                }
            }
        }
    }

    /// `Object::setDisabledUntil`'s `DISABLED_UNMANNED` branch (Object.cpp:2214): reset veterancy to
    /// Regular and undo the `AutoHealBehavior` upgrade. The carbomb detonation (which needs the
    /// sniper's id) is handled where the damage source is known
    /// ([`GameLogic::damage_object_with_death`]).
    fn on_unmanned_disabled(&mut self, id: ObjectId) {
        use crate::experience::VeterancyLevel;
        // `setExperienceAndLevel(0, FALSE)` only runs for a trainable unit; the engine's version is a no-op otherwise
        // (`ExperienceTracker.cpp:207-228`). The AutoHeal undo below runs for every unit (Object.cpp:2242-2246), so the
        // early exit comes after it, not before.
        let old = self.objects.get(&id).filter(|o| o.experience.is_trainable).map(|o| o.experience.level);
        if let Some(o) = self.objects.get_mut(&id) {
            if old.is_some() {
                // `ExperienceTracker::setExperienceAndLevel(0, FALSE)`.
                o.experience.level = VeterancyLevel::Regular;
                o.experience.current = 0;
            }
            // `AutoHealBehavior::undoUpgrade` on the first AutoHeal module only: `findUpdateModule(key_AutoHealBehavior)`
            // returns the first module of that name and `findModule` stops there (Object.cpp:2243-2246, :2916-2938). The
            // sim has no self-granted heal bonus, so the undo resets that module's start latch and next heal frame.
            if let Some(a) = o.behavior.auto_heal.first_mut() {
                a.started = false;
                a.next = 0;
            }
        }
        if let Some(old) = old.filter(|old| *old != VeterancyLevel::Regular) {
            self.on_veterancy_level_changed(id, old, VeterancyLevel::Regular);
        }
    }

    /// `Object::isAbleToAttack` (Object.cpp:3231), reduced to the disabled/status conditions the sim
    /// models: `OBJECT_STATUS_NO_ATTACK` (the first test, Object.cpp:3239-3241, which `ProneUpdate`
    /// sets while a unit is prone), a contained unit needs its container's permission, a subdued unit
    /// cannot attack, and a portable structure / `SPAWNS_ARE_THE_WEAPONS` unit is blocked by
    /// `HACKED`/`EMP` (plus a slave whose slaver is subdued).
    pub fn is_able_to_attack(&self, id: ObjectId) -> bool {
        let Some(o) = self.find_object(id) else { return false };
        // "For things that may or may not be able to normally attack, but are under a status condition"
        // (`Object.cpp:3236-3241`).
        if o.status_bits & crate::object::status::NO_ATTACK != 0 {
            return false;
        }
        if o.is_dying() || o.under_construction {
            return false;
        }
        if let Some(container) = o.contained_by {
            if !self.is_passenger_allowed_to_fire(container) {
                return false;
            }
        }
        if o.is_disabled_by_type(DisabledType::Subdued) {
            return false;
        }
        if o.is_kind_of(kind_of::PORTABLE_STRUCTURE) || o.is_kind_of(kind_of::SPAWNS_ARE_THE_WEAPONS) {
            if o.is_disabled_by_type(DisabledType::Hacked) || o.is_disabled_by_type(DisabledType::Emp) {
                return false;
            }
            if o.is_kind_of(kind_of::INFANTRY) {
                if let Some(master) = o.master {
                    if self.objects.get(&master).is_some_and(|s| s.is_disabled_by_type(DisabledType::Subdued)) {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// `doPowerDisable` (Player.cpp:3281): while the owning player is in brownout, every
    /// `KINDOF_POWERED` object of that player is `DISABLED_UNDERPOWERED`; otherwise the type is
    /// cleared. The original runs this when the energy tally changes (`Energy::addProduction` /
    /// `addConsumption` -> `Player::onPowerBrownOutChange`); applying the idempotent set/clear once
    /// per frame reaches the same state.
    pub(crate) fn update_brownout_disabled(&mut self) {
        let now = self.frame;
        let brownouts: Vec<(PlayerIndex, bool)> = self
            .players
            .keys()
            .map(|&p| (p, self.power_sabotaged_till.get(&p).is_some_and(|&f| now < f) || !self.has_sufficient_power(p)))
            .collect();
        for (player, brownout) in brownouts {
            // `Player::onPowerBrownOutChange` (Player.cpp:3295-3302) also runs `disableRadar` /
            // `enableRadar`: a brownout hides the player's radar unless a `DisableProof` facility is
            // online (`Player::hasRadar`, :3270-3278). The engine calls this on every energy change;
            // the port's energy is derived, so the idempotent set runs once per frame like the
            // `doPowerDisable` half above.
            if let Some(p) = self.players.get_mut(&player) {
                if brownout {
                    p.disable_radar();
                } else {
                    p.enable_radar();
                }
            }
            let ids: Vec<ObjectId> = self
                .objects
                .values()
                .filter(|o| !o.destroyed && o.player == player && o.is_kind_of(kind_of::POWERED))
                .map(|o| o.id)
                .collect();
            for id in ids {
                if brownout {
                    self.set_disabled(id, DisabledType::Underpowered);
                } else {
                    self.clear_disabled(id, DisabledType::Underpowered);
                }
            }
        }
    }
}
