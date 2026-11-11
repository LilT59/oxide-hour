//! The `ObjectHelper` update modules that the C++ `Object` constructor creates for every object, before its
//! behaviour modules (`Object::Object`, `Object.cpp:320-395`), and that sit in the sleepy-update heap.
//!
//! They are registered at creation in the constructor's order, so the heap layout (and with it the order
//! of the AI modules that share a wake frame) matches the original: the constructor's `*curB++` order is
//! `ObjectSMCHelper`, then `StatusDamageHelper` and `SubdualDamageHelper` (unless the template has an
//! `InactiveBody`), then `ObjectDefectionHelper` (unless the template is a shrubbery), then
//! `ObjectWeaponStatusHelper`, `FiringTracker` and `TempWeaponBonusHelper` (when the template can have a
//! weapon, `ThingTemplate::canPossiblyHaveAnyWeapon`, `ThingTemplate.cpp:1410`). `registerObject` pushes the
//! behaviour array in that order (`GameLogic.cpp:4165-4200`).
//!
//! Each module's C++ key (lane 99): the `ObjectHelper` base constructor calls
//! `setWakeFrame(getObject(), UPDATE_SLEEP_FOREVER)` (`ObjectHelper.h:58`), so every helper is created asleep, and the
//! ones that want an early call set their own wake afterwards: `ObjectWeaponStatusHelper` calls
//! `setWakeFrame(NONE)` (`ObjectWeaponStatusHelper.h:66`, so `now + 1`). `ObjectSMCHelper`
//! (`ObjectSMCHelper.h:52`) and `ObjectDefectionHelper` (`ObjectDefectionHelper.h:62`) add no wake of their own, so
//! they stay at `FOREVER`; so do `StatusDamageHelper` (`StatusDamageHelper.cpp:46`), `SubdualDamageHelper` and
//! `TempWeaponBonusHelper` (`UpdateModule.h:199-206` clamps `now + FOREVER`). Their phases are `PHASE_NORMAL`,
//! except `ObjectWeaponStatusHelper` and `FiringTracker`, which are `PHASE_FINAL`
//! (`ObjectWeaponStatusHelper.h:59`, `FiringTracker.h:71`).
//!
//! What each helper does in the port (lane 105, wave 14; callers completed in wave 15 lane 116):
//! - `FiringTracker` (`FiringTracker.cpp`): the consecutive-shot count, the continuous-fire bonus and the
//!   `CONTINUOUS_FIRE_*` model conditions, the auto-reload and cooldown timers, and the spin-down `update`.
//!   `GameLogic::firing_shot_fired` is `FiringTracker::shotFired`; the weapon's numbers come in as
//!   [`FiringParams`]. `combat.rs` `fire_weapon` calls it for every `Object::fireCurrentWeapon` path (the AI
//!   attack and turret fire states, `DeliverPayload`'s strafe, `FireWeaponPower`/`MissileDefender`, possession).
//! - `TempWeaponBonusHelper`: `GameLogic::do_temp_weapon_bonus` and its timed clear. Its only C++ caller is
//!   `WeaponBonusUpdate`, which the sim does not run yet, so no game path reaches it (tests only).
//! - `StatusDamageHelper`: `GameLogic::do_status_damage` and its timed clear, reached from
//!   `combat.rs` `apply_status_damage` for every `DAMAGE_STATUS` weapon hit.
//! - `SubdualDamageHelper`: `notify_subdual_damage`, the heal-over-time `update` (through the body's subdual
//!   accounting in `body.rs`) and `after_subdual_hit`, the game-side half of a subdual hit (`DISABLED_SUBDUED`).
//!   The heal's own `DISABLED_SUBDUED` clear is the sleepy loop's `on_subdual_change` after the helper's
//!   update (`logic.rs`).
//! - `ObjectSMCHelper`, `ObjectDefectionHelper`, `ObjectWeaponStatusHelper`: unchanged idle updates (the
//!   defection timer is not ported).
//!
//! The helpers' state lives on `ObjectCombat::helpers` (not on the helper modules, which the scheduler owns), so an
//! update reads and writes it through the `Object` it is given. Wake calls go through `GameLogic::wake_helper`,
//! which finds the helper's handle by its class name in `Object::update_handles` and awakens it at `now + delay`
//! (`UpdateModule::setWakeFrame`, `UpdateModule.cpp:80-84`).

use crate::bonus::condition as bonus_cond;
use crate::body::{DamageInfo, DamageType};
use crate::disabled::DisabledType;
use crate::logic::GameLogic;
use crate::math::LOGICFRAMES_PER_SECOND;
use crate::module::{SleepyUpdatePhase, UpdateContext, UpdateModule, UpdateSleepTime, UPDATE_SLEEP_FOREVER, UPDATE_SLEEP_NONE};
use crate::object::{status, Object, ObjectId};
use crate::schedule::UpdateHandle;
use crate::template::{kind_of, ObjectTemplate};
use crate::weapon::Weapon;

/// `WEAPONBONUSCONDITION_CONTINUOUS_FIRE_MEAN` / `_FAST` / `TARGET_FAERIE_FIRE` as bits of
/// `ObjectCombat::weapon_bonus_conditions`.
const CONTINUOUS_FIRE_MEAN: u32 = 1 << bonus_cond::CONTINUOUS_FIRE_MEAN;
const CONTINUOUS_FIRE_FAST: u32 = 1 << bonus_cond::CONTINUOUS_FIRE_FAST;
const TARGET_FAERIE_FIRE: u32 = 1 << bonus_cond::TARGET_FAERIE_FIRE;

/// `DEFECTION_DETECTION_TIME_MAX` (`ObjectDefectionHelper.h:36`): `LOGICFRAMES_PER_SECOND * 10`.
const DEFECTION_DETECTION_TIME_MAX: u32 = LOGICFRAMES_PER_SECOND * 10;

/// The per-object state of the helper modules (the C++ helpers' own members), kept on `ObjectCombat`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HelperState {
    /// `FiringTracker`'s members (`FiringTracker.h:61-68`).
    pub firing: FiringState,
    /// `TempWeaponBonusHelper::m_currentBonus` (the condition bit) and `m_frameToRemove`; `None` is `WEAPONBONUSCONDITION_INVALID`.
    pub temp_bonus: Option<(u8, u32)>,
    /// `StatusDamageHelper::m_statusToHeal` (the status mask) and `m_frameToHeal`; `None` is `OBJECT_STATUS_NONE`.
    pub status_damage: Option<(u64, u32)>,
    /// `SubdualDamageHelper::m_healingStepCountdown` (an `UnsignedInt`: a zero countdown wraps on the next step).
    pub subdual_countdown: u32,
    /// `ObjectDefectionHelper`'s timers (`ObjectDefectionHelper.h:56-61`).
    pub defection: DefectionState,
}

/// `ObjectDefectionHelper`'s members (`ObjectDefectionHelper.h:52-62`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DefectionState {
    /// `m_defectionDetectionStart`: the frame the timer started.
    pub start: u32,
    /// `m_defectionDetectionEnd`: the frame it runs out.
    pub end: u32,
    /// `m_defectionDetectionFlashPhase`: the logarithmic flash curve.
    pub flash_phase: f32,
    /// `m_doDefectorFX`: play the flashing/ding FX (a capture uses one frame, `goInvulnerable` none).
    pub do_fx: bool,
}

impl HelperState {
    /// Hashed only where it differs from the default, so an object without these effects keeps its CRC.
    pub(crate) fn crc(&self, c: &mut crate::crc::Crc) {
        if self == &HelperState::default() {
            return;
        }
        self.firing.crc(c);
        match self.temp_bonus {
            Some((bonus, until)) => {
                c.add_u32(u32::from(bonus));
                c.add_u32(until);
            }
            None => c.add_u32(u32::MAX),
        }
        match self.status_damage {
            Some((mask, until)) => {
                c.add_u32(mask as u32);
                c.add_u32((mask >> 32) as u32);
                c.add_u32(until);
            }
            None => c.add_u32(u32::MAX),
        }
        c.add_u32(self.subdual_countdown);
        // Only hashed while defection is/was running, so ordinary objects keep their historical CRC.
        if self.defection != DefectionState::default() {
            c.add_u32(self.defection.start);
            c.add_u32(self.defection.end);
            c.add_f32(self.defection.flash_phase);
            c.add_bool(self.defection.do_fx);
        }
    }
}

/// `FiringTracker`'s members (`FiringTracker.h:61-68`): the shot count at the current victim, the cooldown and
/// auto-reload timers, and the looping-sound timer. `slow` is `MODELCONDITION_CONTINUOUS_FIRE_SLOW`, the one
/// continuous-fire model flag that is not also a bonus flag.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FiringState {
    /// `m_consecutiveShots`.
    pub consecutive_shots: i32,
    /// `m_victimID`: `INVALID` for a ground shot or after a cooldown.
    pub victim: ObjectId,
    /// `m_frameToStartCooldown`: pushed back by each shot to `possible next shot + coast`.
    pub frame_to_start_cooldown: u32,
    /// `m_frameToForceReload`: pushed back by each shot to `now + AutoReloadWhenIdle`.
    pub frame_to_force_reload: u32,
    /// `m_frameToStopLoopingSound` (the sound itself is not played by the sim).
    pub frame_to_stop_looping_sound: u32,
    /// `MODELCONDITION_CONTINUOUS_FIRE_SLOW`.
    pub slow: bool,
}

impl FiringState {
    fn crc(&self, c: &mut crate::crc::Crc) {
        c.add_u32(self.consecutive_shots as u32);
        c.add_u32(self.victim.0);
        c.add_u32(self.frame_to_start_cooldown);
        c.add_u32(self.frame_to_force_reload);
        c.add_u32(self.frame_to_stop_looping_sound);
        c.add_bool(self.slow);
    }
}

/// The numbers `FiringTracker::shotFired` reads from the weapon that fired (`Weapon.h` accessors): the
/// auto-reload delay, the coast frames, the next shot frame and the two continuous-fire thresholds. Built by
/// `combat.rs` `fire_weapon` after `fireWeapon` (`Object::fireCurrentWeapon`, Object.cpp:1510).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FiringParams {
    /// `getAutoReloadWhenIdleFrames`.
    pub auto_reload_when_idle_frames: u32,
    /// `getContinuousFireCoastFrames`.
    pub continuous_fire_coast_frames: u32,
    /// `getPossibleNextShotFrame` (the weapon's `m_whenWeCanFireAgain`, read after `fireWeapon`).
    pub possible_next_shot_frame: u32,
    /// `getContinuousFireOneShotsNeeded` / `getContinuousFireTwoShotsNeeded`.
    pub continuous_fire_one_shots_needed: i32,
    pub continuous_fire_two_shots_needed: i32,
    /// `getFireSoundLoopTime` (frames; 0 for a fire-and-forget sound).
    pub fire_sound_loop_time: u32,
}

impl FiringParams {
    /// The params of `weapon` just after it fired (its next shot frame is already set).
    pub fn of(weapon: &Weapon) -> Self {
        let t = &weapon.template;
        FiringParams {
            auto_reload_when_idle_frames: t.auto_reload_when_idle_frames,
            continuous_fire_coast_frames: t.continuous_fire_coast_frames,
            possible_next_shot_frame: weapon.when_we_can_fire_again(),
            continuous_fire_one_shots_needed: t.continuous_fire_one_shots_needed,
            continuous_fire_two_shots_needed: t.continuous_fire_two_shots_needed,
            fire_sound_loop_time: t.fire_sound_loop_time,
        }
    }
}

/// Which C++ helper a module is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HelperKind {
    /// `ObjectSMCHelper` (`ObjectSMCHelper.cpp:45-52`): clears the special model-condition states, then sleeps forever.
    SmcHelper,
    /// `StatusDamageHelper` (`StatusDamageHelper.cpp:58-65`): clears the status condition, then sleeps forever.
    StatusDamage,
    /// `SubdualDamageHelper`: heals subdual damage, sleeps forever once none is left.
    SubdualDamage,
    /// `ObjectDefectionHelper` (`ObjectDefectionHelper.cpp:48-59`): sleeps forever unless a defection is running.
    Defection,
    /// `ObjectWeaponStatusHelper::update` (`ObjectWeaponStatusHelper.h:66-70`): runs every frame, PHASE_FINAL.
    WeaponStatus,
    /// `FiringTracker::update` (`FiringTracker.cpp:182`): spin-down timer; asleep forever at rest, PHASE_FINAL.
    FiringTracker,
    /// `TempWeaponBonusHelper::update` (`TempWeaponBonusHelper.cpp:60-66`): clears the bonus, then sleeps forever.
    TempWeaponBonus,
}

/// One helper module of an object. `wake` is the frame of its first update (`initial_wake_frame`): the
/// registration frame for a helper with no constructor wake, or the constructor's `setWakeFrame` target.
pub(crate) struct ObjectHelper {
    pub kind: HelperKind,
    pub wake: u32,
}

/// The helper modules of an object built from `template` at frame `frame`, in the constructor's order, with
/// each one's first wake frame (see the module doc for the C++ source of each).
pub(crate) fn object_helper_wakes(template: &ObjectTemplate, frame: u32) -> Vec<(HelperKind, u32)> {
    // `ObjectHelper`'s constructor: `setWakeFrame(getObject(), UPDATE_SLEEP_FOREVER)` (`ObjectHelper.h:58`).
    let mut out = vec![(HelperKind::SmcHelper, UPDATE_SLEEP_FOREVER)];
    // `isInactiveBody`: a template without an `ActiveBody` (`max_health` 0) has no status or subdual helper.
    if template.max_health > 0.0 {
        out.push((HelperKind::StatusDamage, UPDATE_SLEEP_FOREVER));
        out.push((HelperKind::SubdualDamage, UPDATE_SLEEP_FOREVER));
    }
    if !template.is_kind_of(kind_of::SHRUBBERY) {
        // The defection helper's own constructor sets no wake (`ObjectDefectionHelper.h:62`): it stays asleep.
        out.push((HelperKind::Defection, UPDATE_SLEEP_FOREVER));
    }
    // `canPossiblyHaveAnyWeapon` (ThingTemplate.cpp:1410): some weapon set holds at least one weapon.
    if template.weapon_sets.iter().any(|(_, ws)| ws.weapons.iter().any(Option::is_some)) {
        // `ObjectWeaponStatusHelper`'s constructor: `setWakeFrame(getObject(), UPDATE_SLEEP_NONE)` at the creation frame.
        out.push((HelperKind::WeaponStatus, frame.saturating_add(1)));
        out.push((HelperKind::FiringTracker, UPDATE_SLEEP_FOREVER));
        out.push((HelperKind::TempWeaponBonus, UPDATE_SLEEP_FOREVER));
    }
    out
}

impl ObjectHelper {
    /// The helper's C++ class name, as the `ZR_L99_HEAP` dump prints it.
    pub(crate) fn class_name(kind: HelperKind) -> &'static str {
        match kind {
            HelperKind::SmcHelper => "ObjectSMCHelper",
            HelperKind::StatusDamage => "StatusDamageHelper",
            HelperKind::SubdualDamage => "SubdualDamageHelper",
            HelperKind::Defection => "ObjectDefectionHelper",
            HelperKind::WeaponStatus => "ObjectWeaponStatusHelper",
            HelperKind::FiringTracker => "FiringTracker",
            HelperKind::TempWeaponBonus => "TempWeaponBonusHelper",
        }
    }
}

/// `TempWeaponBonusHelper::clearTempWeaponBonus` (TempWeaponBonusHelper.cpp:70-86): drop the bonus this helper set.
/// The frenzy tint is client-side and not kept here.
fn clear_temp_weapon_bonus(o: &mut Object) {
    if let Some((bonus, _)) = o.combat.helpers.temp_bonus.take() {
        o.combat.weapon_bonus_conditions &= !(1u32 << bonus);
    }
}

/// `StatusDamageHelper::clearStatusCondition` (StatusDamageHelper.cpp:68-76): clear the status it set.
fn clear_status_damage(o: &mut Object) {
    if let Some((mask, _)) = o.combat.helpers.status_damage.take() {
        o.status_bits &= !mask;
    }
}

/// `FiringTracker::speedUp` (FiringTracker.cpp:243-285). The `VoiceRapidFire` sound is not played.
fn firing_speed_up(o: &mut Object) {
    let bits = &mut o.combat.weapon_bonus_conditions;
    let slow = &mut o.combat.helpers.firing.slow;
    if *bits & CONTINUOUS_FIRE_FAST != 0 {
        // Already at the top: the exclusive flags stay as they are.
    } else if *bits & CONTINUOUS_FIRE_MEAN != 0 {
        *bits = (*bits & !CONTINUOUS_FIRE_MEAN) | CONTINUOUS_FIRE_FAST;
        *slow = false;
    } else {
        *bits = (*bits & !CONTINUOUS_FIRE_FAST) | CONTINUOUS_FIRE_MEAN;
        *slow = false;
    }
}

/// `FiringTracker::coolDown` (FiringTracker.cpp:291-328): from MEAN or FAST the object drops straight to SLOW (the
/// cooldown timer is left as it is); otherwise it stops and the cooldown timer is cleared. Either way the shot count
/// and victim restart.
fn firing_cool_down(o: &mut Object) {
    let bits = &mut o.combat.weapon_bonus_conditions;
    let f = &mut o.combat.helpers.firing;
    if *bits & (CONTINUOUS_FIRE_FAST | CONTINUOUS_FIRE_MEAN) != 0 {
        f.slow = true;
        *bits &= !(CONTINUOUS_FIRE_FAST | CONTINUOUS_FIRE_MEAN);
    } else {
        f.slow = false;
        f.frame_to_start_cooldown = 0;
    }
    f.consecutive_shots = 0;
    f.victim = ObjectId::INVALID;
}

/// `FiringTracker::calcTimeToSleep` (FiringTracker.cpp:333-364): the sleep until the nearest timer, `FOREVER` when
/// none is running. A timer that is already due gives `UPDATE_SLEEP_NONE`.
fn firing_time_to_sleep(f: &FiringState, now: u32) -> UpdateSleepTime {
    if f.frame_to_stop_looping_sound == 0 && f.frame_to_start_cooldown == 0 && f.frame_to_force_reload == 0 {
        return UPDATE_SLEEP_FOREVER;
    }
    let mut sleep = UPDATE_SLEEP_FOREVER;
    for timer in [f.frame_to_stop_looping_sound, f.frame_to_start_cooldown, f.frame_to_force_reload] {
        if timer != 0 {
            if timer <= now {
                sleep = UPDATE_SLEEP_NONE;
            } else if timer - now < sleep {
                sleep = timer - now;
            }
        }
    }
    sleep
}

/// `FiringTracker::update` (FiringTracker.cpp:182-227).
fn firing_update(o: &mut Object, now: u32) -> UpdateSleepTime {
    // Idle long enough that it should reload, so it does not hang around with a near-empty clip (`reloadAllAmmo(TRUE)`).
    if o.combat.helpers.firing.frame_to_force_reload != 0 && now >= o.combat.helpers.firing.frame_to_force_reload {
        for w in o.combat.weapons.weapons.iter_mut().flatten() {
            w.load_ammo_now(now);
        }
        o.combat.helpers.firing.frame_to_force_reload = 0;
    }
    {
        let f = &mut o.combat.helpers.firing;
        if f.frame_to_stop_looping_sound != 0 && now >= f.frame_to_stop_looping_sound {
            // `TheAudio->removeAudioEvent` (no audio in the sim).
            f.frame_to_stop_looping_sound = 0;
        }
        if f.frame_to_start_cooldown != 0 && now > f.frame_to_start_cooldown {
            // Too long since the last shot: spin down, and look again in a second.
            f.frame_to_start_cooldown = now + LOGICFRAMES_PER_SECOND;
            firing_cool_down(o);
            return LOGICFRAMES_PER_SECOND;
        }
    }
    firing_time_to_sleep(&o.combat.helpers.firing, now)
}

/// `ObjectDefectionHelper::update` (`ObjectDefectionHelper.cpp:48-102`): while the object is an undetected defector,
/// clear the flag when the timer runs out (the "you are now vulnerable" moment), or when it is dead or firing; the
/// flash/sound effects are client-only. The helper sleeps forever otherwise.
fn defection_update(o: &mut Object, now: u32) -> UpdateSleepTime {
    if !o.undetected_defector {
        return UPDATE_SLEEP_FOREVER;
    }
    let (end, do_fx) = (o.combat.helpers.defection.end, o.combat.helpers.defection.do_fx);
    if now >= end {
        o.undetected_defector = false;
        return UPDATE_SLEEP_FOREVER;
    }
    // "dead or attacking... our cover is blown."
    if o.effectively_dead || o.status_mask(now) & crate::object::status::IS_FIRING_WEAPON != 0 {
        o.undetected_defector = false;
        return UPDATE_SLEEP_FOREVER;
    }
    // "if (draw && m_doDefectorFX)": the logarithmic flash curve. The phase change flashes and ticks (client-only), so
    // only the curve itself is simulated; every logic object has a drawable in the engine.
    if do_fx {
        let d = &mut o.combat.helpers.defection;
        let time_left = d.end - now;
        d.flash_phase += 0.5 * (1.0 - time_left as f32 / DEFECTION_DETECTION_TIME_MAX as f32);
    }
    UPDATE_SLEEP_NONE
}

/// `SubdualDamageHelper::update` (SubdualDamageHelper.cpp:56-75): one heal step per `SubdualDamageHealRate` frames,
/// through the body's subdual accounting (a `SUBDUAL_UNRESISTABLE` hit of minus the heal amount).
fn subdual_update(o: &mut Object) -> UpdateSleepTime {
    // `m_healingStepCountdown--` on an `UnsignedInt`: a zero countdown wraps and does not step.
    let countdown = &mut o.combat.helpers.subdual_countdown;
    *countdown = countdown.wrapping_sub(1);
    if *countdown > 0 {
        return UPDATE_SLEEP_NONE;
    }
    let Some(body) = o.body.as_mut() else { return UPDATE_SLEEP_FOREVER };
    *countdown = body.subdual_heal_rate();
    let heal = DamageInfo { source: ObjectId::INVALID, amount: -body.subdual_heal_amount(), kind: DamageType::SubdualUnresistable };
    body.attempt_damage(&heal);
    if body.has_any_subdual_damage() {
        UPDATE_SLEEP_NONE
    } else {
        UPDATE_SLEEP_FOREVER
    }
}

impl UpdateModule for ObjectHelper {
    fn update(&mut self, ctx: &mut UpdateContext) -> UpdateSleepTime {
        match self.kind {
            HelperKind::WeaponStatus => UPDATE_SLEEP_NONE,
            HelperKind::FiringTracker => firing_update(ctx.object, ctx.frame),
            HelperKind::TempWeaponBonus => {
                clear_temp_weapon_bonus(ctx.object);
                UPDATE_SLEEP_FOREVER
            }
            HelperKind::StatusDamage => {
                clear_status_damage(ctx.object);
                UPDATE_SLEEP_FOREVER
            }
            HelperKind::SubdualDamage => subdual_update(ctx.object),
            HelperKind::Defection => defection_update(ctx.object, ctx.frame),
            _ => UPDATE_SLEEP_FOREVER,
        }
    }

    fn phase(&self) -> SleepyUpdatePhase {
        match self.kind {
            HelperKind::WeaponStatus | HelperKind::FiringTracker => SleepyUpdatePhase::Final,
            _ => SleepyUpdatePhase::Normal,
        }
    }

    fn initial_wake_frame(&self) -> u32 {
        self.wake
    }

    fn name(&self) -> &'static str {
        Self::class_name(self.kind)
    }
}

// `Object::doTempWeaponBonus`'s only engine caller is `WeaponBonusUpdate::update` (WeaponBonusUpdate.cpp), whose
// module the sim does not run yet, so `do_temp_weapon_bonus` is reached by tests only. The rest of this block is
// used by the fire, damage and sleepy-update paths. The allowance comes off with `WeaponBonusUpdate`.
#[allow(dead_code)]
impl GameLogic {
    /// The helper's handle on `id`, found by its class name among `Object::update_handles`. A helper that is
    /// running has no module in its slot, and then it cannot be woken anyway (`UpdateSchedule::awaken`).
    pub(crate) fn helper_handle(&self, id: ObjectId, kind: HelperKind) -> Option<UpdateHandle> {
        let name = ObjectHelper::class_name(kind);
        self.objects.get(&id)?.update_handles.iter().copied().find(|h| self.schedule.slot(h.0).module.as_ref().is_some_and(|m| m.name() == name))
    }

    /// `UpdateModule::setWakeFrame(obj, delay)` (UpdateModule.cpp:80-84): awaken the helper at `now + delay`, which
    /// `friend_setNextCallFrame` clamps to `FOREVER` (UpdateModule.h:199-206).
    pub(crate) fn wake_helper(&mut self, id: ObjectId, kind: HelperKind, delay: u32) {
        let now = self.frame;
        let Some(h) = self.helper_handle(id, kind) else { return };
        let when = now.saturating_add(delay).min(UPDATE_SLEEP_FOREVER);
        self.schedule.awaken(h, now, when);
    }

    /// `Object::friend_setUndetectedDefector` (`Object.cpp:819`), the `defect`/`goInvulnerable` entry point.
    pub fn set_undetected_defector(&mut self, id: ObjectId, undetected: bool) {
        if let Some(o) = self.objects.get_mut(&id) {
            o.set_undetected_defector(undetected);
        }
    }

    /// `ObjectDefectionHelper::startDefectionTimer` (`ObjectDefectionHelper.cpp:108-127`): a running defection tick
    /// every frame until `frames` have passed; an object that is not an undetected defector leaves the helper asleep.
    /// The C++ callers are `Object::defect(newTeam, detectionTime)` (`Object.cpp:6281-6284`) and
    /// `Object::goInvulnerable(time)` (`Object.cpp:6353-6361`). The port's `defect` is `object_defect` (defect_l193.rs),
    /// called by the capture trigger (`special_power.rs`) and the car-bomb crate (`collide.rs`), with `detectionTime`
    /// 1 and 0 respectively. `goInvulnerable` has no port caller yet.
    pub fn start_defection_timer(&mut self, id: ObjectId, frames: u32, with_defector_fx: bool) {
        let now = self.frame;
        let undetected = match self.objects.get_mut(&id) {
            Some(o) => {
                if !o.undetected_defector {
                    false
                } else {
                    o.combat.helpers.defection = DefectionState {
                        start: now,
                        end: now.saturating_add(frames),
                        flash_phase: 0.0,
                        do_fx: with_defector_fx,
                    };
                    true
                }
            }
            None => return,
        };
        if undetected {
            self.wake_helper(id, HelperKind::Defection, UPDATE_SLEEP_NONE);
        } else {
            self.wake_helper(id, HelperKind::Defection, UPDATE_SLEEP_FOREVER);
        }
    }

    /// `FiringTracker::getNumConsecutiveShotsAtVictim` (FiringTracker.cpp:53-62), via `Object::getNumConsecutiveShotsFiredAtTarget`
    /// (Object.cpp:6108). A null victim (`INVALID`) counts zero.
    pub fn firing_consecutive_shots(&self, id: ObjectId, victim: ObjectId) -> i32 {
        if victim == ObjectId::INVALID {
            return 0;
        }
        match self.objects.get(&id) {
            Some(o) if o.combat.helpers.firing.victim == victim => o.combat.helpers.firing.consecutive_shots,
            _ => 0,
        }
    }

    /// `FiringTracker::shotFired` (FiringTracker.cpp:86-191): the owner has just fired a weapon at `victim`
    /// (`INVALID` for a ground shot). Called by `Object::fireCurrentWeapon` (Object.cpp:1510, :1532) and
    /// `Object::notifyFiringTrackerShotFired` (AIStates.cpp:5324), after the weapon has fired.
    pub(crate) fn firing_shot_fired(&mut self, id: ObjectId, victim: ObjectId, p: FiringParams) {
        let now = self.frame;
        // A faerie-fire target makes the owner shoot faster; any other target (or a ground shot) clears it.
        let victim_faerie = self.objects.get(&victim).is_some_and(|v| v.status_mask(now) & status::FAERIE_FIRE != 0);
        let Some(o) = self.objects.get_mut(&id) else { return };
        if victim_faerie {
            o.combat.weapon_bonus_conditions |= TARGET_FAERIE_FIRE;
        } else {
            o.combat.weapon_bonus_conditions &= !TARGET_FAERIE_FIRE;
        }
        {
            let f = &mut o.combat.helpers.firing;
            // Same victim, or a new one within the coast time, keeps counting; otherwise the count restarts.
            if victim == f.victim || now < f.frame_to_start_cooldown {
                f.consecutive_shots += 1;
                f.victim = victim;
            } else {
                f.consecutive_shots = 1;
                f.victim = victim;
            }
            // Each shot pushes the idle reload back.
            if p.auto_reload_when_idle_frames > 0 {
                f.frame_to_force_reload = now + p.auto_reload_when_idle_frames;
            }
            f.frame_to_start_cooldown = if p.continuous_fire_coast_frames != 0 { p.possible_next_shot_frame + p.continuous_fire_coast_frames } else { 0 };
        }
        // Speed up or slow down by the shot count, from the current state (FiringTracker.cpp:136-158).
        let shots = o.combat.helpers.firing.consecutive_shots;
        let (one, two) = (p.continuous_fire_one_shots_needed, p.continuous_fire_two_shots_needed);
        let bits = o.combat.weapon_bonus_conditions;
        if bits & CONTINUOUS_FIRE_MEAN != 0 {
            // Can go either way from MEAN.
            if shots < one {
                firing_cool_down(o);
            } else if shots > two {
                firing_speed_up(o);
            }
        } else if bits & CONTINUOUS_FIRE_FAST != 0 {
            // From FAST the only way is all the way down.
            if shots < two {
                firing_cool_down(o);
            }
        } else if shots > one {
            firing_speed_up(o);
        }
        o.combat.helpers.firing.frame_to_stop_looping_sound =
            if p.fire_sound_loop_time != 0 { now + p.fire_sound_loop_time } else { 0 };
        let sleep = firing_time_to_sleep(&o.combat.helpers.firing, now);
        self.wake_helper(id, HelperKind::FiringTracker, sleep);
    }

    /// `Object::doTempWeaponBonus` -> `TempWeaponBonusHelper::doTempWeaponBonus` (TempWeaponBonusHelper.cpp:88-108):
    /// set the bonus condition `cond` for `duration` frames. A different bonus replaces the current one; the same one
    /// restarts the timer.
    pub(crate) fn do_temp_weapon_bonus(&mut self, id: ObjectId, cond: u8, duration: u32) {
        let now = self.frame;
        {
            let Some(o) = self.objects.get_mut(&id) else { return };
            if o.combat.helpers.temp_bonus.is_some_and(|(current, _)| current != cond) {
                clear_temp_weapon_bonus(o);
            }
            o.combat.weapon_bonus_conditions |= 1u32 << cond;
            o.combat.helpers.temp_bonus = Some((cond, now.saturating_add(duration)));
        }
        self.wake_helper(id, HelperKind::TempWeaponBonus, duration);
    }

    /// `Object::doStatusDamage` -> `StatusDamageHelper::doStatusDamage` (StatusDamageHelper.cpp:80-92): set the status
    /// `status_mask` for `duration` frames. The caller rounds the duration up and does not drop a zero
    /// (`ActiveBody.cpp:487-488`, `REAL_TO_INT_CEIL`); the helper's own `REAL_TO_INT_FLOOR`
    /// (StatusDamageHelper.cpp:82) then floors an already-integral value.
    pub(crate) fn do_status_damage(&mut self, id: ObjectId, status_mask: u64, duration: u32) {
        let now = self.frame;
        {
            let Some(o) = self.objects.get_mut(&id) else { return };
            if o.combat.helpers.status_damage.is_some_and(|(current, _)| current != status_mask) {
                clear_status_damage(o);
            }
            o.status_bits |= status_mask;
            o.combat.helpers.status_damage = Some((status_mask, now.saturating_add(duration)));
        }
        self.wake_helper(id, HelperKind::StatusDamage, duration);
    }

    /// `SubdualDamageHelper::notifySubdualDamage` (SubdualDamageHelper.cpp:79-86): a subdual hit that does damage
    /// restarts the heal countdown and wakes the helper for the next frame (`UPDATE_SLEEP_NONE`).
    pub(crate) fn notify_subdual_damage(&mut self, id: ObjectId, amount: f32) {
        if amount <= 0.0 {
            return;
        }
        let Some(o) = self.objects.get_mut(&id) else { return };
        o.combat.helpers.subdual_countdown = o.body.as_ref().map_or(0, |b| b.subdual_heal_rate());
        self.wake_helper(id, HelperKind::SubdualDamage, UPDATE_SLEEP_NONE);
    }

    /// The game-side half of a subdual hit (`ActiveBody::attemptDamage`, ActiveBody.cpp:495-513): `onSubdualChange`
    /// (ActiveBody.cpp:1285-1321: `DISABLED_SUBDUED` and the passenger orders on a change of state, not for
    /// projectiles), then `Object::notifySubdualDamage`. `was_subdued` is `is_subdued()` read before the
    /// `attempt_damage` call. A heal (`SubdualDamageHelper::update`) runs the same two halves, but only
    /// `onSubdualChange` (it must not re-arm the countdown).
    pub(crate) fn after_subdual_hit(&mut self, id: ObjectId, was_subdued: bool) {
        let Some(o) = self.objects.get(&id) else { return };
        let Some(body) = o.body.as_ref() else { return };
        let amount = body.last_subdual_hit();
        self.on_subdual_change(id, was_subdued);
        self.notify_subdual_damage(id, amount);
    }

    /// `ActiveBody::onSubdualChange` (ActiveBody.cpp:1285-1321), run when the subdued state differs from `was_subdued`.
    /// A unit gets or loses `DISABLED_SUBDUED`; a newly subdued container sends its passengers to idle, and a container
    /// that stops being subdued orders its money hackers to hack again if it is an internet center. A projectile is
    /// jammed instead (see below). The heal path calls this too, after `SubdualDamageHelper::update` has healed (so
    /// it must not re-arm the countdown, which `after_subdual_hit` does).
    pub(crate) fn on_subdual_change(&mut self, id: ObjectId, was_subdued: bool) {
        let Some(o) = self.objects.get(&id) else { return };
        let Some(body) = o.body.as_ref() else { return };
        let now_subdued = body.is_subdued();
        if was_subdued == now_subdued {
            return;
        }
        let is_projectile = o.is_kind_of(kind_of::PROJECTILE);
        let internet_center = o.is_kind_of(kind_of::FS_INTERNET_CENTER);
        if is_projectile {
            // `ActiveBody.cpp:1315-1321`: a projectile that becomes subdued is jammed
            // (`ProjectileUpdateInterface::projectileNowJammed`). `MissileAIUpdate::projectileNowJammed`
            // (MissileAIUpdate.cpp:811-840) drops the target, scatters the goal point by
            // `DistanceScatterWhenJammed` (two `GameLogicRandomValue` draws) and flies there;
            // `DumbProjectileBehavior` has an empty override (DumbProjectileBehavior.h:96). The sim's projectiles are
            // not `Object`s and have no `ActiveBody` (`projectile.rs`), so no projectile can reach this branch; an
            // object template flagged `KINDOF_PROJECTILE` would need the jam in its projectile update.
            return;
        }
        if now_subdued {
            self.set_disabled(id, DisabledType::Subdued);
            // `ActiveBody.cpp:1290-1292`: `contain->orderAllPassengersToIdle(CMD_FROM_AI)`.
            self.order_all_passengers_to_idle(id);
        } else {
            self.clear_disabled(id, DisabledType::Subdued);
            if internet_center {
                // `ActiveBody.cpp:1301-1311`, patch 1.01: the center's hackers start hacking again.
                self.order_all_passengers_to_hack_internet(id);
            }
        }
    }

    /// `OpenContain::orderAllPassengersToIdle` (OpenContain.cpp:1479-1491): every contained object that has an AI is
    /// ordered idle.
    fn order_all_passengers_to_idle(&mut self, container: ObjectId) {
        let passengers = self.contained_units(container).to_vec();
        for p in passengers {
            if self.objects.get(&p).is_some_and(|o| o.ai_handle.is_some()) {
                self.ai_idle(p, crate::ai_states::CommandSource::Ai);
            }
        }
    }

    /// `OpenContain::orderAllPassengersToHackInternet` (OpenContain.cpp:1494-1509): every contained `MONEY_HACKER`
    /// with an AI is ordered to hack again.
    fn order_all_passengers_to_hack_internet(&mut self, container: ObjectId) {
        let passengers = self.contained_units(container).to_vec();
        for p in passengers {
            if self.objects.get(&p).is_some_and(|o| o.is_kind_of(kind_of::MONEY_HACKER)) {
                self.ai_hack_internet(p);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::{ActiveBody, BodyModule};
    use crate::math::Coord3D;
    use crate::template::ObjectTemplate;
    use crate::terrain::TerrainLogic;
    use crate::weapon::{WeaponSetTemplate, WeaponTemplate};
    use std::sync::Arc;

    /// A unit with a body and one weapon: every helper of the constructor applies.
    fn armed_infantry() -> ObjectTemplate {
        let mut t = ObjectTemplate::new("ArmedInfantry");
        t.kind_of = kind_of::INFANTRY | kind_of::SELECTABLE;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        let mut w = WeaponTemplate::new("Gun");
        w.primary_damage = 1.0;
        w.attack_range = 100.0;
        let mut set = WeaponSetTemplate::default();
        set.weapons[0] = Some(Arc::new(w));
        t.weapon_sets.push((0, set));
        t
    }

    /// The `Object` constructor's order (Object.cpp:320-395): SMC, status and subdual (a body), defection (not a
    /// shrubbery), then weapon status, firing tracker and temporary bonus (a weapon).
    #[test]
    fn an_armed_body_gets_the_constructor_helpers_in_order() {
        let kinds: Vec<HelperKind> = object_helper_wakes(&armed_infantry(), 5).into_iter().map(|(k, _)| k).collect();
        assert_eq!(
            kinds,
            vec![
                HelperKind::SmcHelper,
                HelperKind::StatusDamage,
                HelperKind::SubdualDamage,
                HelperKind::Defection,
                HelperKind::WeaponStatus,
                HelperKind::FiringTracker,
                HelperKind::TempWeaponBonus,
            ]
        );
    }

    /// No body (`InactiveBody`), a shrubbery and no weapon: only the SMC helper (Object.cpp:330, :341, :377, :386).
    #[test]
    fn a_bodiless_unarmed_shrubbery_gets_only_the_smc_helper() {
        let mut t = ObjectTemplate::new("Bush");
        t.kind_of = kind_of::SHRUBBERY;
        t.max_health = 0.0; // no `ActiveBody`: the default `InactiveBody`
        assert_eq!(object_helper_wakes(&t, 5), vec![(HelperKind::SmcHelper, UPDATE_SLEEP_FOREVER)]);
    }

    /// The first wake of each helper: every helper starts at `FOREVER` from the `ObjectHelper` constructor, and only
    /// `ObjectWeaponStatusHelper` moves itself to the creation frame + 1 (`ObjectWeaponStatusHelper.h:66`,
    /// `setWakeFrame(NONE)`); the SMC (`ObjectSMCHelper.h:52`) and defection (`ObjectDefectionHelper.h:62`)
    /// constructors add no wake, so they stay asleep.
    #[test]
    fn the_helper_wake_frames_follow_their_constructors() {
        let wakes: Vec<u32> = object_helper_wakes(&armed_infantry(), 5).into_iter().map(|(_, w)| w).collect();
        assert_eq!(
            wakes,
            vec![
                UPDATE_SLEEP_FOREVER,
                UPDATE_SLEEP_FOREVER,
                UPDATE_SLEEP_FOREVER,
                UPDATE_SLEEP_FOREVER,
                6,
                UPDATE_SLEEP_FOREVER,
                UPDATE_SLEEP_FOREVER
            ]
        );
    }

    /// The heap keys after registration (frame 0, so `now` is 1): `(wake << 2) | phase`, with `FOREVER` clamped
    /// (`friend_setNextCallFrame`, UpdateModule.h:203). The weapon status helper is phase Final (3).
    #[test]
    fn the_helpers_are_in_the_heap_with_their_keys_and_phases() {
        let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 1);
        let id = g.create_object(&Arc::new(armed_infantry()), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let forever_normal = (UPDATE_SLEEP_FOREVER << 2) | 2;
        let forever_final = (UPDATE_SLEEP_FOREVER << 2) | 3;
        let keys: Vec<u32> = g.objects[&id].update_handles.iter().map(|h| g.schedule.slot(h.0).next_call_and_phase).collect();
        assert_eq!(keys, vec![forever_normal, forever_normal, forever_normal, forever_normal, 7, forever_final, forever_normal]);
    }

    /// Continuous-fire parameters: a 30-frame coast, MEAN after more than `one` shots, FAST after more than `two`.
    fn fire_params(next_shot: u32, one: i32, two: i32) -> FiringParams {
        FiringParams {
            auto_reload_when_idle_frames: 0,
            continuous_fire_coast_frames: 30,
            possible_next_shot_frame: next_shot,
            continuous_fire_one_shots_needed: one,
            continuous_fire_two_shots_needed: two,
            fire_sound_loop_time: 0,
        }
    }

    fn bonus_bits(g: &GameLogic, id: ObjectId) -> u32 {
        g.objects[&id].combat.weapon_bonus_conditions
    }

    /// `FiringTracker::shotFired` (FiringTracker.cpp:136-158): with `one` 3 and `two` 6, the fourth shot at one
    /// victim speeds the object up to MEAN and the seventh to FAST. The model conditions follow the bonus flags.
    /// Without `speedUp` (the reverted branch) the bits never change.
    #[test]
    fn a_run_of_shots_speeds_up_to_mean_then_fast() {
        let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 1);
        let id = g.create_object(&Arc::new(armed_infantry()), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let victim = ObjectId(4242);
        for shot in 1..=7 {
            g.firing_shot_fired(id, victim, fire_params(g.frame, 3, 6));
            assert_eq!(g.firing_consecutive_shots(id, victim), shot);
            let expect = match shot {
                0..=3 => 0,
                4..=6 => CONTINUOUS_FIRE_MEAN,
                _ => CONTINUOUS_FIRE_FAST,
            };
            assert_eq!(bonus_bits(&g, id) & (CONTINUOUS_FIRE_MEAN | CONTINUOUS_FIRE_FAST), expect, "after shot {shot}");
            g.run_frames(1);
        }
        let model = g.model_conditions(id).unwrap();
        assert!(model.test(crate::condition::CONTINUOUS_FIRE_FAST));
        assert!(!model.test(crate::condition::CONTINUOUS_FIRE_MEAN));
        assert!(!model.test(crate::condition::CONTINUOUS_FIRE_SLOW));
        // Another victim restarts the count (not within the coast time, and the victim differs).
        g.run_frames(40);
        g.firing_shot_fired(id, ObjectId(4243), fire_params(g.frame, 3, 6));
        assert_eq!(g.firing_consecutive_shots(id, ObjectId(4243)), 1);
        assert_eq!(g.firing_consecutive_shots(id, victim), 0);
    }

    /// A shot wakes the tracker for its next timer (`setWakeFrame(me, calcTimeToSleep())`, FiringTracker.cpp:223): the
    /// cooldown is 30 frames out, so the tracker's next call is then. Without `wake_helper` it sleeps forever.
    #[test]
    fn a_shot_wakes_the_firing_tracker_for_its_cooldown() {
        let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 1);
        let id = g.create_object(&Arc::new(armed_infantry()), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let h = g.helper_handle(id, HelperKind::FiringTracker).unwrap();
        assert_eq!(g.schedule.next_call_frame(h), UPDATE_SLEEP_FOREVER);
        g.firing_shot_fired(id, ObjectId(4242), fire_params(g.frame, 3, 6));
        assert_eq!(g.schedule.next_call_frame(h), g.frame + 30);
    }

    /// `FiringTracker::update` spin-down (FiringTracker.cpp:182-227): when the coast runs out a FAST object drops to
    /// SLOW (the cooldown is renewed for a second), and a second later with no shot it stops and sleeps forever.
    #[test]
    fn an_idle_firing_tracker_spins_down_to_slow_then_stops() {
        let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 1);
        let id = g.create_object(&Arc::new(armed_infantry()), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let victim = ObjectId(4242);
        for _ in 0..7 {
            g.firing_shot_fired(id, victim, fire_params(g.frame, 3, 6));
            g.run_frames(1);
        }
        assert_ne!(bonus_bits(&g, id) & CONTINUOUS_FIRE_FAST, 0);
        // The last shot was at frame 6, so the coast ends at 6 + 30 = 36. The tracker's wake at 36 finds the timer
        // not yet past, and its next call at 37 spins down (the cooldown is then renewed to 67).
        g.run_frames(31);
        assert_eq!(bonus_bits(&g, id) & (CONTINUOUS_FIRE_MEAN | CONTINUOUS_FIRE_FAST), 0, "the speed flags clear");
        let model = g.model_conditions(id).unwrap();
        assert!(model.test(crate::condition::CONTINUOUS_FIRE_SLOW), "coolDown leaves SLOW");
        assert_eq!(g.firing_consecutive_shots(id, victim), 0, "coolDown restarts the count");
        // The cooldown was renewed for a second at frame 37 (to 67): the update at 68 clears SLOW. It then returns a
        // second's sleep even though `coolDown` has cleared the timer (FiringTracker.cpp:199-205), so the next call is
        // at 98, where no timer is left and the tracker sleeps forever.
        let h = g.helper_handle(id, HelperKind::FiringTracker).unwrap();
        g.run_frames(31);
        assert!(!g.model_conditions(id).unwrap().test(crate::condition::CONTINUOUS_FIRE_SLOW));
        assert_eq!(g.schedule.next_call_frame(h), 98);
        g.run_frames(30);
        assert_eq!(g.schedule.next_call_frame(h), UPDATE_SLEEP_FOREVER);
    }

    /// `TempWeaponBonusHelper::doTempWeaponBonus` (TempWeaponBonusHelper.cpp:88-108) sets the bonus and wakes the helper
    /// for the duration; its update clears the bonus at that frame.
    #[test]
    fn a_temporary_bonus_is_set_for_its_duration_then_cleared() {
        let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 1);
        let id = g.create_object(&Arc::new(armed_infantry()), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let cond = bonus_cond::FRENZY_ONE;
        g.do_temp_weapon_bonus(id, cond, 10);
        assert_ne!(bonus_bits(&g, id) & (1 << cond), 0);
        let h = g.helper_handle(id, HelperKind::TempWeaponBonus).unwrap();
        assert_eq!(g.schedule.next_call_frame(h), g.frame + 10);
        g.run_frames(9);
        assert_ne!(bonus_bits(&g, id) & (1 << cond), 0, "still in effect");
        // The helper's wake frame is 10: its update runs in the call that starts at frame 10 (`run_frames` runs
        // `update` once per frame, and each call advances the frame after its modules run).
        g.run_frames(2);
        assert_eq!(bonus_bits(&g, id) & (1 << cond), 0, "cleared by the update at its end frame");
    }

    /// A second temporary bonus replaces the first (`clearTempWeaponBonus`, TempWeaponBonusHelper.cpp:89-94).
    #[test]
    fn a_second_temporary_bonus_replaces_the_first() {
        let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 1);
        let id = g.create_object(&Arc::new(armed_infantry()), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        g.do_temp_weapon_bonus(id, bonus_cond::FRENZY_ONE, 10);
        g.run_frames(5);
        g.do_temp_weapon_bonus(id, bonus_cond::FRENZY_TWO, 10);
        assert_eq!(bonus_bits(&g, id) & (1 << bonus_cond::FRENZY_ONE), 0);
        assert_ne!(bonus_bits(&g, id) & (1 << bonus_cond::FRENZY_TWO), 0);
        let h = g.helper_handle(id, HelperKind::TempWeaponBonus).unwrap();
        assert_eq!(g.schedule.next_call_frame(h), g.frame + 10, "the timer restarts");
    }

    /// `StatusDamageHelper::doStatusDamage` (StatusDamageHelper.cpp:80-92): the status holds for the duration, then its
    /// update clears it.
    #[test]
    fn a_status_damage_holds_its_status_for_the_duration() {
        let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 1);
        let id = g.create_object(&Arc::new(armed_infantry()), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        g.do_status_damage(id, status::MASKED, 5);
        assert_ne!(g.objects[&id].status_mask(g.frame) & status::MASKED, 0);
        g.run_frames(4);
        assert_ne!(g.objects[&id].status_mask(g.frame) & status::MASKED, 0);
        // Wake frame 5: cleared by the update of frame 5 (see the temporary-bonus test).
        g.run_frames(2);
        assert_eq!(g.objects[&id].status_mask(g.frame) & status::MASKED, 0);
    }

    /// `SubdualDamageHelper` (SubdualDamageHelper.cpp:56-86): a subdual hit wakes the helper for the next frame; each
    /// `SubdualDamageHealRate` frames it heals `SubdualDamageHealAmount` through the body. Here 100 subdual damage on
    /// 100 max health subdues the unit (`DISABLED_SUBDUED`), and a heal of 50 every 10 frames takes it out again.
    #[test]
    fn subdual_damage_heals_one_step_per_rate_and_wakes_the_helper() {
        let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 1);
        let id = g.create_object(&Arc::new(armed_infantry()), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let body = ActiveBody::new(100.0, 100.0, 0.7, 0.35).with_subdual(1000.0, 10, 50.0);
        g.objects.get_mut(&id).unwrap().body = Some(Box::new(body) as Box<dyn BodyModule>);
        let hit = DamageInfo { source: ObjectId::INVALID, amount: 100.0, kind: DamageType::SubdualVehicle };
        let was = g.objects[&id].body.as_ref().unwrap().is_subdued();
        g.objects.get_mut(&id).unwrap().body.as_mut().unwrap().attempt_damage(&hit);
        g.after_subdual_hit(id, was);
        assert!(g.objects[&id].is_disabled_by_type(DisabledType::Subdued));
        let h = g.helper_handle(id, HelperKind::SubdualDamage).unwrap();
        assert_eq!(g.schedule.next_call_frame(h), g.frame + 1);
        let body_now = |g: &GameLogic| g.objects[&id].body.as_ref().unwrap().is_subdued();
        // The countdown is 10 at frame 0, so the first step runs in the update of frame 10 (the countdown
        // reads 10 - frame until it reaches 0 there), and the second in the update of frame 20.
        g.run_frames(9);
        assert!(body_now(&g), "no heal step before the rate has passed");
        g.run_frames(2);
        assert!(!body_now(&g), "the first step heals 50 of 100");
        g.run_frames(10);
        assert!(!g.objects[&id].body.as_ref().unwrap().has_any_subdual_damage(), "the second step clears the rest");
        assert_eq!(g.schedule.next_call_frame(h), UPDATE_SLEEP_FOREVER);
    }

    /// A body without a `SubdualDamageCap` cannot be subdued (`canBeSubdued`, ActiveBody.cpp:1277), so a subdual hit
    /// neither subdues nor wakes the helper.
    #[test]
    fn a_subdual_hit_on_a_body_without_a_cap_does_not_wake_the_helper() {
        let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 1);
        let id = g.create_object(&Arc::new(armed_infantry()), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let hit = DamageInfo { source: ObjectId::INVALID, amount: 100.0, kind: DamageType::SubdualVehicle };
        let was = g.objects[&id].body.as_ref().unwrap().is_subdued();
        g.objects.get_mut(&id).unwrap().body.as_mut().unwrap().attempt_damage(&hit);
        g.after_subdual_hit(id, was);
        assert!(!g.objects[&id].is_disabled_by_type(DisabledType::Subdued));
        let h = g.helper_handle(id, HelperKind::SubdualDamage).unwrap();
        assert_eq!(g.schedule.next_call_frame(h), UPDATE_SLEEP_FOREVER);
    }
}
