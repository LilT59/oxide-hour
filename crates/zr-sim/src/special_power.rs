//! Special powers (`Common/RTS/SpecialPower.cpp`, `GameLogic/Object/SpecialPower/*`) and unit
//! special abilities (`SpecialAbilityUpdate.cpp`), reduced to a cooldown + an effect.
//!
//! Ported: `SpecialPowerTemplate` (reload, required science, shared/public timer), the
//! `OCLSpecialPower` / `FireWeaponPower` / `CashHackSpecialPower` effects and `SpecialPowerStore::
//! canUseSpecialPower`'s science gate. `SpecialAbilityUpdate`'s approach/prep/unpack state machine
//! and the capture/snipe/disable effects are **stubbed** (documented): abilities currently fire
//! their effect immediately, like a power.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::ai_states::{AiStateType, CommandSource};
use crate::combat::AttackTarget;
use crate::damage::Relationship;
use crate::logic::GameLogic;
use crate::math::Coord3D;
use crate::object::{ObjectId, PlayerIndex};
use crate::ocl::OclTemplate;
use crate::pathfind::PATHFIND_CELL_SIZE_F;

/// `SPECIAL_ABILITY_HUGE_DISTANCE` (`SpecialAbilityUpdate.h:44`): the default start and abort range.
const SPECIAL_ABILITY_HUGE_DISTANCE: f32 = 10_000_000.0;

/// Where a power is aimed (`aiSpecialPower`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SpecialPowerTarget {
    None,
    Position(Coord3D),
    Object(ObjectId),
}

/// What a special power module does (`OCLSpecialPower`, `FireWeaponPower`, `CashHackSpecialPower`).
#[derive(Clone, Debug)]
pub enum SpecialPowerEffect {
    /// `OCLSpecialPower`: create the OCL at the source or the target (`CreateLocation`).
    /// `create_location` is the `OCLSpecialPower` `CreateLocation` (the primary point of a `DeliverPayload`).
    Ocl { ocl: Option<Arc<OclTemplate>>, create_at_target: bool, create_location: crate::ocl::CreateLocation },
    /// `FireWeaponPower`: fire the current weapon at the target.
    FireWeapon,
    /// `CashHackSpecialPower`: steal `amount` from the victim's owner.
    CashHack { amount: i32 },
    /// `CleanupAreaPower`: tell the source's `CleanupHazardUpdate` to clear hazards around a point.
    CleanupArea { range: f32 },
    /// `BaikonurLaunchPower`: create `detonation_object` at the target location.
    Baikonur { detonation_object: Option<Arc<crate::template::ObjectTemplate>> },
    /// `SpyVisionSpecialPower`: reveal enemy units for a duration scaled by captured passengers.
    SpyVision { base: u32, bonus_per_captured: u32, max: u32 },
    /// `ParticleUplinkCannonUpdate`: begin the superweapon charge at the target.
    ParticleUplink,
    /// `SpectreGunshipDeploymentUpdate`: deploy the gunship over the target.
    SpectreDeployment,
    /// `CashBountyPower`: passive (applied on creation); listed for completeness.
    CashBounty,
}

/// A special power attached to an object template.
#[derive(Clone, Debug)]
pub struct SimSpecialPower {
    pub power: String,
    pub effect: SpecialPowerEffect,
    /// `StartsPaused` (`SpecialPowerModule.cpp:131`).
    pub starts_paused: bool,
    /// `true` for a `SpecialPowerModule` (`OCLSpecialPower`, `FireWeaponPower`, ...). The superweapon
    /// update modules (`ParticleUplinkCannonUpdate`, `SpectreGunshipDeploymentUpdate`) are
    /// `SpecialPowerUpdateModule`s and own no pause count.
    pub power_module: bool,
}

/// `SpecialPowerModule::m_pausedCount` and `m_pausedOnFrame` for one power of one object.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PowerPause {
    pub count: i32,
    pub paused_on_frame: u32,
}

/// What the sim needs from a `SpecialPowerTemplate`.
#[derive(Clone, Debug, PartialEq)]
pub struct SimSpecialPowerTemplate {
    pub name: String,
    /// `Enum` (`SpecialPowerType`, e.g. `SPECIAL_BLACKLOTUS_CAPTURE_BUILDING`).
    pub kind: String,
    pub reload_time: u32,
    pub required_science: Option<String>,
    pub shared_synced_timer: bool,
    pub public_timer: bool,
    /// `RadiusCursorRadius`: the area the AI scans when it fires the power at the most costly spot
    /// (`AIPlayer::computeSuperweaponTarget` / `doSkirmishFireSpecialPowerAtMostCost`).
    pub radius_cursor_radius: f32,
}

/// `SpecialAbilityUpdateModuleData` (sim copy): a unit special ability. A unit with a `SpecialAbility`
/// (power module) and a `SpecialAbilityUpdate` for the same power carries both: the update holds the
/// timings (`StartAbilityRange`, `PreparationTime`, ...) and the power module holds the pause and the
/// cooldown (`SpecialAbility.cpp`).
#[derive(Clone, Debug)]
pub struct SpecialAbilityTemplate {
    pub power: String,
    /// `m_startAbilityRange` (`SPECIAL_ABILITY_HUGE_DISTANCE` by default).
    pub start_ability_range: f32,
    /// `m_abilityAbortRange` (`SPECIAL_ABILITY_HUGE_DISTANCE` by default).
    pub ability_abort_range: f32,
    /// `m_approachRequiresLOS` (`TRUE` by default). The line-of-sight half of the filter is not ported.
    pub approach_requires_los: bool,
    /// Frames.
    pub preparation_frames: u32,
    /// `m_packTime` / `m_unpackTime` in frames.
    pub pack_time: u32,
    pub unpack_time: u32,
    /// `m_packUnpackVariationFactor`.
    pub pack_unpack_variation: f32,
    /// `m_skipPackingWithNoTarget`.
    pub skip_packing_with_no_target: bool,
    /// `m_needToFaceTarget`.
    pub need_to_face: bool,
    /// `m_awardXPForTriggering` and `m_skillPointsForTriggering` (-1: use the XP value).
    pub award_xp_for_triggering: i32,
    pub skill_points_for_triggering: i32,
    pub effect_duration: u32,
    pub effect_value: i32,
    /// `SpecialObject` resolved (charge, napalm bomb, ...).
    pub special_object: Option<Arc<crate::template::ObjectTemplate>>,
    pub max_special_objects: u32,
    pub flee_range_after_completion: f32,
    /// `StartsPaused` on the `SpecialAbility` module (`SpecialPowerModule.cpp:131`).
    pub starts_paused: bool,
    /// `true` for the `SpecialAbility` class, a `SpecialPowerModule` (its power's cooldown and pause
    /// count live on it). The `SpecialAbilityUpdate` class is an update module and has neither.
    pub power_module: bool,
}

/// `SpecialAbilityUpdate::m_packingState` (`SpecialAbilityUpdate.h`, same order).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PackingState {
    #[default]
    None,
    Packing,
    Unpacking,
    Packed,
    Unpacked,
}

/// `SpecialAbilityUpdate`'s running state (`m_active` is `Object::ability` being `Some`).
#[derive(Clone, Debug)]
pub struct AbilityState {
    pub power: String,
    pub target_obj: Option<ObjectId>,
    /// `m_targetPos`; zero when the use has no position (a target object or a no-target command).
    pub target_pos: Coord3D,
    /// `m_prepFrames`: preparation is complete at 0 (`isPreparationComplete`).
    pub prep_frames: u32,
    /// `m_packingState`.
    pub packing: PackingState,
    /// `m_animFrames`: the pack / unpack countdown.
    pub anim_frames: u32,
    /// `m_withinStartAbilityRange`: sticky once the unit has been within range.
    pub within_start_range: bool,
    /// `m_facingInitiated` / `m_facingComplete`. The turn itself is not modelled, so facing completes
    /// as soon as it starts.
    pub facing_initiated: bool,
    pub facing_complete: bool,
    /// `m_noTargetCommand`: the use had neither a target object nor a position.
    pub no_target_command: bool,
    pub special_objects: Vec<ObjectId>,
}

/// The ability behaviour selected by the `SpecialPowerTemplate`'s `Enum`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AbilityKind {
    CaptureBuilding,
    Disable,
    StealCash,
    Charge,
    Napalm,
    MissileDefender,
    Other,
}

/// Whether a `SpecialPowerTemplate` kind is a building capture (`SPECIAL_INFANTRY_CAPTURE_BUILDING`
/// or the Black Lotus form): the command-button hunt then scans with `ActionManager::canCaptureBuilding`.
pub(crate) fn is_capture_power_kind(kind: &str) -> bool {
    ability_kind(kind) == AbilityKind::CaptureBuilding
}

/// The template the state machine runs for `power`: the `SpecialAbilityUpdate` module when the object
/// has one (it holds the timings), else the `SpecialAbility` module itself.
fn ability_template_for(list: &[Arc<SpecialAbilityTemplate>], power: &str) -> Option<Arc<SpecialAbilityTemplate>> {
    list.iter()
        .find(|a| a.power == power && !a.power_module)
        .or_else(|| list.iter().find(|a| a.power == power))
        .cloned()
}

/// `SpecialAbilityUpdate::needToPack` (:697): packed only after an unpacked use, unless it is a
/// no-target use that skips packing.
fn need_to_pack(a: &AbilityState, tpl: &SpecialAbilityTemplate) -> bool {
    a.packing == PackingState::Unpacked && !(tpl.skip_packing_with_no_target && a.no_target_command) && tpl.pack_time != 0
}

/// `SpecialAbilityUpdate::needToUnpack` (:715).
fn need_to_unpack(a: &AbilityState, tpl: &SpecialAbilityTemplate) -> bool {
    a.packing == PackingState::Packed && !(tpl.skip_packing_with_no_target && a.no_target_command) && tpl.unpack_time != 0
}

fn ability_kind(kind: &str) -> AbilityKind {
    match kind {
        "SPECIAL_INFANTRY_CAPTURE_BUILDING" | "SPECIAL_BLACKLOTUS_CAPTURE_BUILDING" => AbilityKind::CaptureBuilding,
        "SPECIAL_HACKER_DISABLE_BUILDING" | "SPECIAL_BLACKLOTUS_DISABLE_VEHICLE_HACK" => AbilityKind::Disable,
        "SPECIAL_BLACKLOTUS_STEAL_CASH_HACK" => AbilityKind::StealCash,
        "SPECIAL_REMOTE_CHARGES" | "SPECIAL_TIMED_CHARGES" | "SPECIAL_TANKHUNTER_TNT_ATTACK" | "SPECIAL_BOOBY_TRAP" => AbilityKind::Charge,
        "SPECIAL_HELIX_NAPALM_BOMB" => AbilityKind::Napalm,
        "SPECIAL_MISSILE_DEFENDER_LASER_GUIDED_MISSILES" => AbilityKind::MissileDefender,
        _ => AbilityKind::Other,
    }
}

impl GameLogic {
    /// Install the special-power store (set by the adapter).
    pub fn set_special_powers(&mut self, powers: BTreeMap<String, Arc<SimSpecialPowerTemplate>>) {
        self.special_powers = powers;
    }

    /// `SpecialPowerTemplate::ReloadTime` for `power` (0 if unknown), for the button cooldown wipe.
    pub fn special_power_reload(&self, power: &str) -> u32 {
        self.special_powers.get(power).map_or(0, |t| t.reload_time)
    }

    /// `SpecialPowerTemplate::getRadiusCursorRadius` for `power` (0 if unknown).
    pub fn special_power_radius_cursor_radius(&self, power: &str) -> f32 {
        self.special_powers.get(power).map_or(0.0, |t| t.radius_cursor_radius)
    }

    /// Diagnostics: every special power fired so far, by power name.
    pub fn power_fire_counts(&self) -> &BTreeMap<String, u64> {
        &self.power_fire_counts
    }

    /// `SpecialPowerTemplate::getSpecialPowerType` name for `power` ("" if unknown).
    pub fn special_power_kind(&self, power: &str) -> &str {
        self.special_powers.get(power).map_or("", |t| t.kind.as_str())
    }

    /// The frame `power` is next usable for `source` (`SpecialPowerStore::canUseSpecialPower`).
    pub fn special_power_ready_at(&self, source: ObjectId, power: &str) -> u32 {
        let template = self.special_powers.get(power);
        if let Some(t) = template {
            if t.shared_synced_timer {
                if let Some(player) = self.find_object(source).map(|o| o.player) {
                    return self.shared_power_ready.get(&(player, power.to_string())).copied().unwrap_or(0);
                }
            }
        }
        self.find_object(source).and_then(|o| o.special_power_ready.get(power).copied()).unwrap_or(0)
    }

    /// Whether `power` is a shared-synced timer (`SpecialPowerTemplate::isSharedNSync`).
    fn is_shared_power(&self, power: &str) -> bool {
        self.special_powers.get(power).is_some_and(|t| t.shared_synced_timer)
    }

    /// The `SpecialPowerModule`s of `id`, one `(power, starts_paused)` per module in template order
    /// (special-power modules first, then `SpecialAbility` modules). Update modules are excluded.
    pub(crate) fn special_power_modules(&self, id: ObjectId) -> Vec<(String, bool)> {
        let Some(o) = self.find_object(id) else { return Vec::new() };
        let powers = o.template.special_powers.iter().filter(|m| m.power_module).map(|m| (m.power.clone(), m.starts_paused));
        let abilities = o.template.special_abilities.iter().filter(|a| a.power_module).map(|a| (a.power.clone(), a.starts_paused));
        powers.chain(abilities).collect()
    }

    /// `SpecialPowerModule::m_pausedCount > 0` for `power` on `source`.
    pub fn special_power_paused(&self, source: ObjectId, power: &str) -> bool {
        self.find_object(source).and_then(|o| o.special_power_pause.get(power)).is_some_and(|p| p.count > 0)
    }

    /// `SpecialPowerModule::isReady` (SpecialPowerModule.cpp:296): a shared-timer power is ready
    /// when the player's timer has run out (the pause count does not apply, `:312`); any other power
    /// must be unpaused and past its ready frame.
    pub fn special_power_is_ready(&self, source: ObjectId, power: &str) -> bool {
        let now = self.frame;
        // `getOrStartSpecialPowerReadyFrame` (Player.cpp:3368): an absent timer reads as ready now.
        if self.is_shared_power(power) && self.find_object(source).is_some() {
            return now >= self.special_power_ready_at(source, power);
        }
        !self.special_power_paused(source, power) && now >= self.special_power_ready_at(source, power)
    }

    /// The side effect of `SpecialPowerModule::isReady` on a shared-timer power:
    /// `Player::getOrStartSpecialPowerReadyFrame` (Player.cpp:3368) adds the player's timer at `now` when
    /// there is none. The read in [`Self::special_power_is_ready`] cannot do that (it takes `&self`), and the
    /// fire that follows a ready check relies on it: `resetOrStartSpecialPowerReadyFrame` then restarts the
    /// existing timer at `now + reload` (Player.cpp:3324). Without it the fire inserts `now`, so the power
    /// reads ready again the next frame (AN3 seed 1, the second spy satellite at frame 2). Callers that test
    /// readiness right before firing call this first, as `ScriptActions.cpp:4271` does.
    pub(crate) fn special_power_start_shared_timer(&mut self, source: ObjectId, power: &str) {
        if !self.is_shared_power(power) {
            return;
        }
        let Some(player) = self.find_object(source).map(|o| o.player) else { return };
        let now = self.frame;
        self.shared_power_ready.entry((player, power.to_string())).or_insert(now);
    }

    /// `SpecialPowerStore::canUseSpecialPower` (SpecialPower.cpp:321): the object has a module for
    /// `power`, and the controlling player owns the template's `RequiredScience` (if any). The script
    /// condition `SKIRMISH_SPECIAL_POWER_READY` tests this before `isReady` (ScriptConditions.cpp:1927);
    /// the script action does not (ScriptActions.cpp:4232), so the condition is where it is gated.
    pub fn special_power_can_use(&self, source: ObjectId, power: &str) -> bool {
        let Some(o) = self.find_object(source) else { return false };
        if !self.special_power_modules(source).iter().any(|(p, _)| p == power) {
            return false;
        }
        match self.special_powers.get(power).and_then(|t| t.required_science.as_deref()) {
            Some(sci) => self.player_has_science(o.player, sci),
            None => true,
        }
    }

    /// `Player::resetOrStartSpecialPowerReadyFrame` (Player.cpp:3324): restart the player's shared
    /// timer at `now + reload`, or create it at `now` when it does not exist yet.
    fn reset_or_start_shared_ready(&mut self, player: PlayerIndex, power: &str, now: u32, reload: u32) {
        let key = (player, power.to_string());
        match self.shared_power_ready.get_mut(&key) {
            Some(r) => *r = now + reload,
            None => {
                self.shared_power_ready.insert(key, now);
            }
        }
    }

    /// `Player::expressSpecialPowerReadyFrame` (Player.cpp:3346): set the player's shared timer.
    fn express_shared_ready(&mut self, player: PlayerIndex, power: &str, frame: u32) {
        self.shared_power_ready.insert((player, power.to_string()), frame);
    }

    /// `SpecialPowerModule::startPowerRecharge` (SpecialPowerModule.cpp:406): a shared-timer power
    /// resets the player's timer, any other sets the module's ready frame to `now + ReloadTime`.
    /// Does nothing for an object with no controlling player (`:427`).
    fn start_power_recharge(&mut self, id: ObjectId, power: &str) {
        let now = self.frame;
        let reload = self.special_power_reload(power);
        let Some(player) = self.find_object(id).map(|o| o.player) else { return };
        if self.is_shared_power(power) {
            self.reset_or_start_shared_ready(player, power, now, reload);
        } else if let Some(o) = self.objects.get_mut(&id) {
            o.special_power_ready.insert(power.to_string(), now + reload);
        }
    }

    /// `SpecialPowerModule::pauseCountdown` (SpecialPowerModule.cpp:785). Pausing counts up, and the
    /// first pause records the frame. Unpausing counts down; when the last pause ends the ready frame
    /// moves forward by the paused span (`m_availableOnFrame += now - m_pausedOnFrame`, `:806`), so a
    /// paused power's cooldown does not run. A shared timer is not moved (C++ never reads it).
    pub(crate) fn pause_countdown(&mut self, id: ObjectId, power: &str, pausing: bool) {
        let now = self.frame;
        let shared = self.is_shared_power(power);
        let Some(o) = self.objects.get_mut(&id) else { return };
        let p = o.special_power_pause.entry(power.to_string()).or_default();
        if pausing {
            if p.count == 0 {
                p.paused_on_frame = now;
            }
            p.count += 1;
        } else if p.count > 0 {
            p.count -= 1;
            if p.count == 0 && !shared {
                let span = now.saturating_sub(p.paused_on_frame);
                let ready = o.special_power_ready.entry(power.to_string()).or_insert(0);
                *ready = ready.saturating_add(span);
            }
        }
    }

    /// `Object::pauseAllSpecialPowers` (Object.cpp:2404): pause or unpause every `SpecialPowerModule`
    /// of the object. Called on the non-`HELD` disabled edges (Object.cpp:2167, 2325).
    pub(crate) fn pause_all_special_powers(&mut self, id: ObjectId, pausing: bool) {
        for (power, _) in self.special_power_modules(id) {
            self.pause_countdown(id, &power, pausing);
        }
    }

    /// `SpecialPowerModule::setReadyFrame` (SpecialPowerModule.cpp:176) as the scripts and the
    /// science wake-up use it: the ready frame is `frame`, and the pause start is reset to now.
    fn set_power_ready_frame(&mut self, id: ObjectId, power: &str, frame: u32) {
        let now = self.frame;
        let shared = self.is_shared_power(power);
        let Some(o) = self.objects.get_mut(&id) else { return };
        if !shared {
            o.special_power_ready.insert(power.to_string(), frame);
        }
        o.special_power_pause.entry(power.to_string()).or_default().paused_on_frame = now;
    }

    /// `SpecialPowerModule::onSpecialPowerCreation` (SpecialPowerModule.cpp:209) for one module: a
    /// shared power expresses its player timer as ready now, any other starts a recharge; then a
    /// `StartsPaused` module pauses (again, as in C++, if the constructor already paused it).
    fn special_power_creation(&mut self, id: ObjectId, power: &str, starts_paused: bool) {
        let now = self.frame;
        let Some(player) = self.find_object(id).map(|o| o.player) else { return };
        if self.is_shared_power(power) {
            self.express_shared_ready(player, power, now);
        } else {
            self.start_power_recharge(id, power);
        }
        if starts_paused {
            self.pause_countdown(id, power, true);
        }
    }

    /// The `SpecialPowerModule` constructor (SpecialPowerModule.cpp:98-134), run for a new object.
    /// A finished object restarts the recharge of every non-shared power (`:116-124`). Every
    /// `StartsPaused` module pauses (`:131`), also when the object is under construction. An
    /// under-construction object's recharge waits for [`Self::on_special_power_creation`].
    pub(crate) fn init_special_powers(&mut self, id: ObjectId) {
        let modules = self.special_power_modules(id);
        let under_construction = self.find_object(id).is_some_and(|o| o.under_construction);
        if !under_construction {
            for (power, _) in &modules {
                if !self.is_shared_power(power) {
                    self.start_power_recharge(id, power);
                }
            }
        }
        for (power, starts_paused) in &modules {
            if *starts_paused {
                self.pause_countdown(id, power, true);
            }
        }
    }

    /// `SpecialPowerCreate::onBuildComplete` (SpecialPowerCreate.cpp:57): every `SpecialPowerModule`
    /// of the finished building runs `onSpecialPowerCreation`.
    pub(crate) fn on_special_power_creation(&mut self, id: ObjectId) {
        for (power, starts_paused) in self.special_power_modules(id) {
            self.special_power_creation(id, &power, starts_paused);
        }
    }

    /// `Player::addScience` (Player.cpp:2565): the first time a science is added, every module whose
    /// power requires it runs `onSpecialPowerCreation` and is set ready now (`:2585-2593`). Objects
    /// in the player's teams are approximated by the player's live objects.
    pub(crate) fn wake_special_powers_for_science(&mut self, player: PlayerIndex, science: &str) {
        let now = self.frame;
        let owned: Vec<ObjectId> = self.objects.values().filter(|o| o.player == player && !o.destroyed).map(|o| o.id).collect();
        for id in owned {
            for (power, starts_paused) in self.special_power_modules(id) {
                let requires = self.special_powers.get(&power).and_then(|t| t.required_science.as_deref());
                if requires != Some(science) {
                    continue;
                }
                self.special_power_creation(id, &power, starts_paused);
                // `CashBountyPower::onSpecialPowerCreation` for the granted science (Player.cpp:2590-2600).
                if self.is_cash_bounty_power(id, &power) {
                    self.cash_bounty_on_special_power_creation(id);
                }
                self.set_power_ready_frame(id, &power, now);
            }
        }
    }

    /// `MSG_DO_SPECIAL_POWER`: validate, then either start a `SpecialAbilityUpdate` (approach /
    /// preparation / effect) or apply an instant `OCLSpecialPower` / `FireWeaponPower` /
    /// `CashHackSpecialPower`. Returns whether the power was actually fired (a rejected attempt --
    /// on cooldown, missing science, disabled source -- returns false).
    pub(crate) fn command_do_special_power(&mut self, player: PlayerIndex, source: ObjectId, power: &str, target: SpecialPowerTarget) -> bool {
        self.do_special_power(player, source, power, target, None, false, None)
    }

    /// `MSG_DO_SPECIAL_POWER_AT_LOCATION` (`GameLogicDispatch.cpp:1175`): as
    /// [`command_do_special_power`](Self::command_do_special_power) at `pos`, with the build `angle`
    /// that `OCLSpecialPower::doSpecialPowerAtLocation` passes to its object creation list.
    pub(crate) fn command_do_special_power_at_location(&mut self, player: PlayerIndex, source: ObjectId, power: &str, pos: Coord3D, angle: Option<f32>) -> bool {
        self.do_special_power(player, source, power, SpecialPowerTarget::Position(pos), angle, false, None)
    }

    /// A script's fire: `Object::doCommandButton` / `doCommandButtonAtObject` / `doCommandButtonAtPosition`
    /// with `CMD_FROM_SCRIPT` (`Object.cpp:5518`, `:5665`, `:5774`), which call `doSpecialPower(..., forced =
    /// TRUE)`. The team's group fires go through here (`AIGroup::groupDoCommandButton*`, `CMD_FROM_SCRIPT`).
    ///
    /// `scripts` is the engine the action runs on. `ScriptEngine::notifyOfTriggeredSpecialPower` /
    /// `notifyOfCompletedSpecialPower` (`ScriptEngine.cpp:7220`, `:7236`) push into the *singleton*
    /// engine's lists, so a fire reached from a script action must record on the engine that action
    /// runs on: `script_engine_update` takes `GameLogic::scripts` out, so writing through `self.scripts`
    /// here would drop the event (and a `PLAYER_TRIGGERED_SPECIAL_POWER` condition later in the same
    /// pass would not see it).
    pub(crate) fn script_do_special_power(&mut self, player: PlayerIndex, source: ObjectId, power: &str, target: SpecialPowerTarget, scripts: &mut crate::script::ScriptEngine) -> bool {
        self.do_special_power(player, source, power, target, None, true, Some(scripts))
    }

    /// A script's location fire: `Object::doCommandButtonAtPosition` (`Object.cpp:5774`) with
    /// `INVALID_ANGLE`, forced as [`script_do_special_power`](Self::script_do_special_power).
    pub(crate) fn script_do_special_power_at_location(
        &mut self,
        player: PlayerIndex,
        source: ObjectId,
        power: &str,
        pos: Coord3D,
        scripts: &mut crate::script::ScriptEngine,
    ) -> bool {
        self.do_special_power(player, source, power, SpecialPowerTarget::Position(pos), None, true, Some(scripts))
    }

    /// `Object::doSpecialPower` (`Object.cpp:5424`). `forced` is the engine's flag: a script's fire skips
    /// `SpecialPowerStore::canUseSpecialPower` (`:5431`, the science test). The readiness test is not part
    /// of the object layer in the engine either (the GUI and the caller make it), so a forced fire skips it
    /// as well and restarts the cooldown (`SpecialPowerModule::triggerSpecialPower`). The pause count
    /// (`SpecialPowerModule::doSpecialPower`, `SpecialPowerModule.cpp:693`) and `isDisabled` apply to every
    /// fire. `scripts` is the engine the fire runs on when it comes from a script action; `None` means the
    /// frame path, where the live engine is `self.scripts`.
    #[allow(clippy::too_many_arguments)] // the C++ call's inputs plus the script-engine hand-off
    fn do_special_power(
        &mut self,
        player: PlayerIndex,
        source: ObjectId,
        power: &str,
        target: SpecialPowerTarget,
        angle: Option<f32>,
        forced: bool,
        mut scripts: Option<&mut crate::script::ScriptEngine>,
    ) -> bool {
        let now = self.frame;
        let (ability, module, pos) = {
            let Some(o) = self.find_object(source) else { return false };
            // `Object::doSpecialPower` returns while `isDisabled()` (any disabled type, `HELD` too,
            // Object.cpp:5427).
            if o.player != player || o.is_dying() || o.ai.suspended || o.is_disabled() {
                return false;
            }
            let ability = ability_template_for(&o.template.special_abilities, power);
            let module = o.template.special_powers.iter().find(|m| m.power == power).cloned();
            if ability.is_none() && module.is_none() {
                return false;
            }
            (ability, module, o.position)
        };
        // A player's (non-forced) fire follows the GUI's `isReady`, which starts a missing shared timer
        // (`getOrStartSpecialPowerReadyFrame`, Player.cpp:3368), so the restart below is `now + reload`. A forced
        // (scripted) fire does not query readiness (`Object::doSpecialPower` only tests `canUseSpecialPower` when
        // not forced, Object.cpp:5431; `ScriptActions::doNamedFireSpecialPowerAtWaypoint` calls the module directly,
        // ScriptActions.cpp:4232), so `resetOrStartSpecialPowerReadyFrame` finds no timer and inserts `now`.
        if !forced {
            self.special_power_start_shared_timer(source, power);
        }
        // `SpecialPowerModule::doSpecialPower` returns while `m_pausedCount > 0` (SpecialPowerModule.cpp:693).
        if self.special_power_paused(source, power) {
            return false;
        }
        if !forced {
            if now < self.special_power_ready_at(source, power) {
                return false;
            }
            // `SpecialPowerStore::canUseSpecialPower` (SpecialPower.cpp:321): the required science must be owned.
            if let Some(t) = self.special_powers.get(power) {
                if let Some(sci) = &t.required_science {
                    if !self.player_has_science(player, sci) {
                        return false;
                    }
                }
            }
        }
        // A special-power *module* (`OCLSpecialPower`, the superweapon update modules, ...) owns the
        // effect; only when there is none does the `SpecialAbilityUpdate` state machine run. Some
        // superweapon buildings carry an inert `SpecialAbility` with the same power name.
        if let Some(module) = module {
            // `SpecialPowerStore::onSpecialPowerTriggered`: the `PLAYER_TRIGGERED_SPECIAL_POWER`
            // condition consumes this.
            self.record_special_power(scripts.as_deref_mut(), false, player, power, source);
            self.apply_special_power(source, &module, target, angle);
            // `MissileLauncherBuildingUpdate::initiateIntentToDoSpecialPower`: close the door.
            self.notify_missile_launcher_fired(source, power, now);
            self.start_special_power_cooldown(source, player, power, now);
            // `SpecialPowerStore::onSpecialPowerCompleted`.
            self.record_special_power(scripts, true, player, power, source);
            *self.power_fire_counts.entry(power.to_string()).or_default() += 1;
            return true;
        }
        if let Some(tpl) = ability {
            // `SpecialAbilityUpdate::initiateIntentToDoSpecialPower` (SpecialAbilityUpdate.cpp:471): the
            // state is reset, the unit packed or (when there is nothing to unpack) unpacked, and the AI
            // goes idle as `CMD_FROM_AI`. The recharge starts when preparation starts (`startPreparation`).
            let target_obj = match target {
                SpecialPowerTarget::Object(id) => Some(id),
                _ => None,
            };
            let no_target = target_obj.is_none() && matches!(target, SpecialPowerTarget::None);
            let target_pos = if no_target { Coord3D::ZERO } else { self.target_position(target).unwrap_or(pos) };
            let packing = if tpl.unpack_time == 0 || (no_target && tpl.skip_packing_with_no_target) {
                PackingState::Unpacked
            } else {
                PackingState::Packed
            };
            if let Some(o) = self.objects.get_mut(&source) {
                o.combat.attack = None;
            }
            self.ai_idle(source, CommandSource::Ai);
            if let Some(o) = self.objects.get_mut(&source) {
                o.ability = Some(AbilityState {
                    power: power.to_string(),
                    target_obj,
                    target_pos,
                    prep_frames: 0,
                    packing,
                    anim_frames: 0,
                    within_start_range: false,
                    facing_initiated: false,
                    facing_complete: false,
                    no_target_command: no_target,
                    special_objects: Vec::new(),
                });
            }
            // `SpecialAbilityUpdate::initiateIntentToDoSpecialPower`'s `setWakeFrame(getObject(), UPDATE_SLEEP_NONE)`
            // (SpecialAbilityUpdate.cpp:556): the update module for this power runs on the next frame.
            if let Some(index) = self.special_ability_module_index(source, power) {
                self.wake_special_ability(source, index);
            }
        }
        *self.power_fire_counts.entry(power.to_string()).or_default() += 1;
        true
    }

    /// `ScriptEngine::notifyOfTriggeredSpecialPower` / `notifyOfCompletedSpecialPower`
    /// (`ScriptEngine.cpp:7220-7240`): append to the engine's `m_triggeredSpecialPowers` /
    /// `m_finishedSpecialPowers`, which `isSpecialPowerTriggered` / `isSpecialPowerComplete` scan.
    ///
    /// `scripts` is the engine a script action is running on (`Some`) or `None` for the frame path,
    /// where `self.scripts` is the live engine. A script action must pass its engine: the singleton
    /// engine is out of `GameLogic` while scripts run, so the event would land in an empty engine and
    /// be dropped when `script_engine_update` restores the real one -- and a condition later in the
    /// same script pass would not see a fire of its own pass.
    fn record_special_power(
        &mut self,
        scripts: Option<&mut crate::script::ScriptEngine>,
        completed: bool,
        player: PlayerIndex,
        power: &str,
        source: ObjectId,
    ) {
        let engine = match scripts {
            Some(e) => e,
            None => &mut self.scripts,
        };
        let list = if completed {
            &mut engine.completed_special_powers
        } else {
            &mut engine.triggered_special_powers
        };
        list.entry(player).or_default().push((power.to_string(), source));
    }

    /// `SpecialPowerCompletionDie::notifyScriptEngine`: the payload object records that the power
    /// belonging to its controlling player (issued by `creator`) is complete. Runs from the die
    /// module's update (the frame), so the engine is in place.
    pub(crate) fn record_completed_special_power(&mut self, player: PlayerIndex, power: &str, creator: ObjectId) {
        self.record_special_power(None, true, player, power, creator);
    }

    /// Whether `player` has any completed special-power event recorded (for the script condition
    /// `PLAYER_COMPLETED_SPECIAL_POWER`).
    pub fn completed_special_power_seen(&self, player: PlayerIndex) -> bool {
        self.scripts.completed_special_powers.get(&player).is_some_and(|v| !v.is_empty())
    }

    fn start_special_power_cooldown(&mut self, source: ObjectId, player: PlayerIndex, power: &str, now: u32) {
        let reload = self.special_powers.get(power).map_or(0, |t| t.reload_time);
        let shared = self.is_shared_power(power);
        if shared {
            self.reset_or_start_shared_ready(player, power, now, reload);
        } else if let Some(o) = self.objects.get_mut(&source) {
            o.special_power_ready.insert(power.to_string(), now + reload);
        }
    }

    /// `SpecialAbilityUpdate::update` (SpecialAbilityUpdate.cpp:184) for the module at `module_index` in the
    /// template's `special_abilities`. The module is a heap slot (`sleepy_families.rs`), so `m_active` is whether the
    /// object's active ability is this module's power; `calcSleepTime` (SpecialAbilityUpdate.h:252-255) returns
    /// `UPDATE_SLEEP_NONE` while it is active and `UPDATE_SLEEP_FOREVER` once it is not.
    pub(crate) fn special_ability_update(&mut self, id: ObjectId, module_index: usize) -> u32 {
        let Some(tpl) = self.objects.get(&id).and_then(|o| o.template.special_abilities.get(module_index)).cloned() else {
            return crate::module::UPDATE_SLEEP_FOREVER;
        };
        if tpl.power_module || !self.special_ability_active(id, &tpl.power) {
            return crate::module::UPDATE_SLEEP_FOREVER;
        }
        self.update_special_ability(id);
        if self.special_ability_active(id, &tpl.power) {
            crate::module::UPDATE_SLEEP_NONE
        } else {
            crate::module::UPDATE_SLEEP_FOREVER
        }
    }

    /// `SpecialAbilityUpdate::isActive` (`SpecialAbilityUpdate.h:184`): this module's `m_active`. The port keeps one
    /// ability state per object (`Object::ability`), so the module is active while that state names its power.
    fn special_ability_active(&self, id: ObjectId, power: &str) -> bool {
        self.objects.get(&id).is_some_and(|o| o.ability.as_ref().is_some_and(|a| a.power == power))
    }

    /// The index in the template's `special_abilities` of the `SpecialAbilityUpdate` module for `power` (the
    /// `SpecialAbility` power module is not an `UpdateModule`, `SpecialAbilityUpdate.h:169`).
    fn special_ability_module_index(&self, id: ObjectId, power: &str) -> Option<usize> {
        self.objects
            .get(&id)?
            .template
            .special_abilities
            .iter()
            .position(|a| !a.power_module && a.power == power)
    }

    /// Stores `a` as the object's ability, or clears it.
    fn set_ability(&mut self, id: ObjectId, a: Option<AbilityState>) {
        if let Some(o) = self.objects.get_mut(&id) {
            o.ability = a;
        }
    }

    /// `SpecialAbilityUpdate::finishAbility` (:1763) without the flee move: the AI goes idle and the
    /// ability ends (`onExit`).
    fn finish_ability(&mut self, id: ObjectId) {
        self.ai_idle(id, CommandSource::Ai);
        self.end_ability(id);
    }

    /// `SpecialAbilityUpdate::endPreparation` (`SpecialAbilityUpdate.cpp:1961-1997`): the ability stops
    /// using the object, so `OBJECT_STATUS_IS_USING_ABILITY` is cleared *at the end of the preparation*,
    /// not when the whole ability ends (`onExit` also calls this, `:601`). The preparation sound loop is
    /// client-only. The special-object kill list below it covers the missile defender, the hacker's
    /// disable and the Black Lotus' three powers; the port creates special objects only for the charge
    /// and napalm kinds, which the list explicitly keeps ("No, don't delete placed charges."), and the
    /// killing kinds create none, so there is nothing to destroy here.
    fn end_preparation(&mut self, id: ObjectId) {
        if let Some(o) = self.objects.get_mut(&id) {
            o.status_bits &= !crate::object::status::USING_ABILITY;
        }
    }

    /// The `onExit` half that every end of an ability runs (`SpecialAbilityUpdate.cpp:1795-1802`): the
    /// `IS_USING_ABILITY` status goes away with the ability.
    fn end_ability(&mut self, id: ObjectId) {
        self.end_preparation(id);
        self.set_ability(id, None);
    }

    /// `SpecialAbilityUpdate::isFacing` (`SpecialAbilityUpdate.cpp:1846-1876`): a facing that has started
    /// and not completed is still in progress while the AI is out of `AI_IDLE`; the AI being back in
    /// `AI_IDLE` completes it and the ability goes on. Without AI the unit "always faces its target".
    fn ability_is_facing(&mut self, id: ObjectId, a: &mut AbilityState) -> bool {
        let Some(o) = self.objects.get(&id) else { return true };
        if o.ai_handle.is_none() {
            return true;
        }
        if !a.facing_complete && a.facing_initiated {
            if o.ai.machine.current == AiStateType::Idle {
                // "We finished facing the target".
                a.facing_complete = true;
                return false;
            }
            // "We are still in the process of facing the target".
            return true;
        }
        false
    }

    /// `SpecialAbilityUpdate::needToFace` (`SpecialAbilityUpdate.cpp:1880-1897`): the template's
    /// `NeedToFaceTarget` (the engine default is `TRUE`) and a facing that is not already complete.
    fn ability_need_to_face(&self, a: &AbilityState, tpl: &SpecialAbilityTemplate) -> bool {
        tpl.need_to_face && (!a.facing_initiated || !a.facing_complete)
    }

    /// `SpecialAbilityUpdate::startFacing` (`SpecialAbilityUpdate.cpp:1899-1926`): stop the unit, drop its
    /// dynamic physics, then face the target object (or the target position, when the use has one) as
    /// `CMD_FROM_AI`. The turn itself runs in the AI state machine (`AI_FACE_OBJECT`/`AI_FACE_POSITION`).
    fn start_facing(&mut self, id: ObjectId, a: &AbilityState) {
        self.ai_idle(id, CommandSource::Ai);
        // "First kill it's physics."
        if let Some(o) = self.objects.get_mut(&id) {
            if let Some(p) = o.physics.as_mut() {
                p.reset_dynamic_physics();
            }
        }
        let target = a.target_obj.filter(|t| self.objects.contains_key(t));
        if let Some(t) = target {
            self.ai_face_object(id, t, CommandSource::Ai);
        } else if a.target_pos.x != 0.0 || a.target_pos.y != 0.0 || a.target_pos.z != 0.0 {
            self.ai_face_position(id, a.target_pos, CommandSource::Ai);
        }
    }

    /// One frame of `SpecialAbilityUpdate::update`: the packing step, the aborts, then preparation,
    /// the start-range branch (facing, unpack, `startPreparation`, trigger) or the approach.
    fn update_special_ability(&mut self, id: ObjectId) {
        let Some(mut a) = self.objects.get(&id).and_then(|o| o.ability.clone()) else { return };
        let Some(tpl) = self.objects.get(&id).and_then(|o| ability_template_for(&o.template.special_abilities, &a.power)) else {
            self.end_ability(id);
            return;
        };
        // `isEffectivelyDead()` -> `onExit(TRUE)`.
        if self.objects.get(&id).is_none_or(|o| o.destroyed || o.is_dying()) {
            self.end_ability(id);
            return;
        }
        // `!ai` -> `onExit(false)`; a move, attack or script order (`getLastCommandSource() != CMD_FROM_AI`)
        // ends the ability at once.
        if self.objects.get(&id).is_none_or(|o| o.ai_handle.is_none() || o.ai.machine.last_command_source != CommandSource::Ai) {
            self.end_ability(id);
            return;
        }
        let kind = self.special_power_kind(&a.power).to_string();
        // A capture is broken by movement (`isMoving() && isPowerCurrentlyInUse() && !m_facingInitiated`,
        // :210). `isPowerCurrentlyInUse` with no command is "packing and within start range" (:585).
        let moving = self.objects.get(&id).is_some_and(|o| o.is_ai_moving());
        if moving && is_capture_power_kind(&kind) && a.packing != PackingState::None && a.within_start_range && !a.facing_initiated {
            self.end_ability(id);
            return;
        }
        // `handlePackingProcessing` (:634).
        if a.anim_frames > 0 {
            a.anim_frames -= 1;
            if a.anim_frames > 0 {
                self.set_ability(id, Some(a));
                return;
            }
            match a.packing {
                PackingState::Packing => {
                    a.packing = PackingState::Packed;
                    self.finish_ability(id);
                    return;
                }
                PackingState::Unpacking => a.packing = PackingState::Unpacked,
                _ => {}
            }
        }
        // A dead target ends the use; a capture stops when a colleague has taken the target (:290).
        let mut should_abort = false;
        if let Some(t) = a.target_obj {
            if let Some(dead) = self.find_object(t).map(|o| o.is_dying() || o.destroyed) {
                let stealth_hidden = self.is_stealthed(t);
                let prep_done = a.prep_frames == 0;
                if dead {
                    should_abort = true;
                } else {
                    match kind.as_str() {
                        "SPECIAL_INFANTRY_CAPTURE_BUILDING" | "SPECIAL_BLACKLOTUS_CAPTURE_BUILDING" | "SPECIAL_HACKER_DISABLE_BUILDING" => {
                            // `target->getTeam() == getObject()->getTeam()` (:270): a colleague's team, not its player.
                            if self.same_object_team(id, t) {
                                should_abort = true;
                            }
                            should_abort |= stealth_hidden && !prep_done;
                        }
                        "SPECIAL_BLACKLOTUS_STEAL_CASH_HACK" | "SPECIAL_BOOBY_TRAP" => should_abort |= stealth_hidden && !prep_done,
                        "SPECIAL_REMOTE_CHARGES" | "SPECIAL_TIMED_CHARGES" => {
                            should_abort |= !need_to_unpack(&a, &tpl) && stealth_hidden && !prep_done;
                        }
                        "SPECIAL_MISSILE_DEFENDER_LASER_GUIDED_MISSILES" => {
                            should_abort |= self.find_object(t).is_some_and(|o| o.is_kind_of(crate::template::kind_of::STRUCTURE));
                            should_abort |= stealth_hidden;
                        }
                        "SPECIAL_BLACKLOTUS_DISABLE_VEHICLE_HACK" => should_abort |= stealth_hidden,
                        _ => {}
                    }
                }
            }
        }
        // `getMySPM() == nullptr` also stops the ability.
        let has_power_module = self.special_power_modules(id).iter().any(|(p, _)| p == &a.power);
        if should_abort || !has_power_module {
            self.ai_idle(id, CommandSource::Ai);
            self.end_ability(id);
            return;
        }

        // STEP 3 (preparation in progress): count down, then trigger or abort (`!isPreparationComplete`).
        if a.prep_frames != 0 {
            a.prep_frames -= 1;
            if a.prep_frames == 0 {
                // STEP 4: trigger, then pack or finish.
                self.trigger_ability_effect(id, &tpl, a.target_obj, a.target_pos);
                // `endPreparation()` after `triggerAbilityEffect` (`SpecialAbilityUpdate.cpp:365-383`).
                self.end_preparation(id);
                if need_to_pack(&a, &tpl) {
                    self.start_packing(id, &mut a, &tpl);
                    self.set_ability(id, Some(a));
                } else {
                    self.finish_ability(id);
                }
                return;
            }
            if self.continue_preparation(id, &a, &tpl) {
                self.set_ability(id, Some(a));
            } else {
                // `continuePreparation` failed (`SpecialAbilityUpdate.cpp:390-403`): `endPreparation`, then
                // pack (unsuccessful) or finish.
                self.end_preparation(id);
                if need_to_pack(&a, &tpl) {
                    self.start_packing(id, &mut a, &tpl);
                    self.set_ability(id, Some(a));
                } else {
                    self.finish_ability(id);
                }
            }
            return;
        }

        if self.within_start_ability_range(id, &a, &tpl) {
            // `m_withinStartAbilityRange = true` (sticky, :406).
            a.within_start_range = true;
            // STEP 1b: `!isFacing() && needToFace()` -> `startFacing()` (`:407-411`). The state machine
            // then turns the unit (`AI_FACE_OBJECT`/`AI_FACE_POSITION`) and `isFacing` only reports the
            // facing done once the AI is idle again (`:1846-1876`).
            if !self.ability_is_facing(id, &mut a) && self.ability_need_to_face(&a, &tpl) {
                self.start_facing(id, &a);
                a.facing_initiated = true;
                self.set_ability(id, Some(a));
                return;
            }
            // STEP 2: unpack.
            if need_to_unpack(&a, &tpl) {
                self.start_unpacking(id, &mut a, &tpl);
                self.set_ability(id, Some(a));
                return;
            }
            if a.packing == PackingState::Unpacked {
                // STEP 3: `startPreparation`.
                self.start_preparation(id, &mut a, &tpl);
                if a.prep_frames == 0 {
                    // STEP 4 (skipping preparation): trigger, then pack or finish.
                    self.trigger_ability_effect(id, &tpl, a.target_obj, a.target_pos);
                    // `endPreparation()` (`SpecialAbilityUpdate.cpp:365-383`).
                    self.end_preparation(id);
                    if need_to_pack(&a, &tpl) {
                        self.start_packing(id, &mut a, &tpl);
                        self.set_ability(id, Some(a));
                    } else {
                        self.finish_ability(id);
                    }
                    return;
                }
            }
            self.set_ability(id, Some(a));
            return;
        }

        // STEP 1: approach while the AI is idle (`ai->isIdle()`).
        let idle = self.objects.get(&id).is_some_and(|o| o.ai.machine.current == AiStateType::Idle);
        if idle {
            self.approach_target(id, &a);
        }
        self.set_ability(id, Some(a));
    }

    /// `SpecialAbilityUpdate::approachTarget` (:950): `ignoreObstacle` and `aiMoveToObject`, or
    /// `aiMoveToPosition` for a ground use.
    fn approach_target(&mut self, id: ObjectId, a: &AbilityState) {
        if let Some(t) = a.target_obj {
            if self.find_object(t).is_some() {
                self.ai_set_ignore_obstacle(id, t);
                self.ai_move_to_object_cmd(id, t, CommandSource::Ai);
            }
        } else if a.target_pos != Coord3D::ZERO {
            self.ai_move_to_position_cmd(id, a.target_pos, CommandSource::Ai);
        }
    }

    /// The boundary distance squared from the unit to the ability's target, as `isWithinStartAbilityRange`
    /// and `isWithinAbilityAbortRange` measure it (`FROM_BOUNDINGSPHERE_2D`). `None` when the use has no
    /// target object and no position (the checks then pass). A target object that is gone reads as 0.
    fn ability_distance_sqr(&self, id: ObjectId, a: &AbilityState) -> Option<f32> {
        if a.target_obj.is_some() {
            return Some(a.target_obj.filter(|t| self.find_object(*t).is_some()).map_or(0.0, |t| self.boundary_dist_sqr_2d(id, t)));
        }
        if a.target_pos != Coord3D::ZERO {
            let me = self.find_object(id)?;
            let d = me.position.dist_2d(&a.target_pos) - me.template.geometry.bounding_circle_radius();
            return Some(if d <= 0.0 { 0.0 } else { d * d });
        }
        None
    }

    /// `isWithinStartAbilityRange` (SpecialAbilityUpdate.cpp:813). Sticky once true (:824). The
    /// distance is compared with the full `StartAbilityRange`; the 2.5 `UNDERSIZE` only shrinks the
    /// contact and line-of-sight range. The line-of-sight filter is not ported and counts as clear.
    fn within_start_ability_range(&self, id: ObjectId, a: &AbilityState, tpl: &SpecialAbilityTemplate) -> bool {
        if a.within_start_range {
            return true;
        }
        let Some(dist_sqr) = self.ability_distance_sqr(id, a) else { return true };
        if dist_sqr > tpl.start_ability_range * tpl.start_ability_range {
            return false;
        }
        let range = (tpl.start_ability_range - PATHFIND_CELL_SIZE_F * 0.25).max(0.0);
        let target_found = a.target_obj.is_some_and(|t| self.find_object(t).is_some());
        if range == 0.0 && a.target_obj.is_some() {
            // Contact: the target must be overlapping the unit (`iteratePotentialCollisions`).
            return target_found && dist_sqr == 0.0;
        }
        if tpl.approach_requires_los {
            // `iterateObjectsInRange(self, range, ...)` must find the target itself (`target == theTarget`).
            return target_found && dist_sqr <= range * range;
        }
        true
    }

    /// `isWithinAbilityAbortRange` (SpecialAbilityUpdate.cpp:894): the same test against
    /// `AbilityAbortRange`, with no line-of-sight part.
    fn within_ability_abort_range(&self, id: ObjectId, a: &AbilityState, tpl: &SpecialAbilityTemplate) -> bool {
        let Some(dist_sqr) = self.ability_distance_sqr(id, a) else { return true };
        if dist_sqr > tpl.ability_abort_range * tpl.ability_abort_range {
            return false;
        }
        let range = (tpl.start_ability_range - PATHFIND_CELL_SIZE_F * 0.25).max(0.0);
        if range == 0.0 && a.target_obj.is_some() {
            let target_found = a.target_obj.is_some_and(|t| self.find_object(t).is_some());
            return target_found && dist_sqr == 0.0;
        }
        true
    }

    /// `SpecialAbilityUpdate::startPreparation` (:980): the preparation countdown is set first; an
    /// ally target (or a colleague's capture) returns early and `continuePreparation` aborts next frame.
    /// Otherwise the power is marked triggered (script event) and its recharge starts, and the AI idles.
    fn start_preparation(&mut self, id: ObjectId, a: &mut AbilityState, tpl: &SpecialAbilityTemplate) {
        a.prep_frames = tpl.preparation_frames;
        let kind = self.special_power_kind(&a.power).to_string();
        let caster_player = self.find_object(id).map(|o| o.player);
        match kind.as_str() {
            "SPECIAL_INFANTRY_CAPTURE_BUILDING" => {
                // "it's been captured by a colleague! we should stop." (`target->getTeam() == getObject()->getTeam()`, :1010)
                if a.target_obj.is_some_and(|t| self.same_object_team(id, t)) {
                    return;
                }
            }
            "SPECIAL_HACKER_DISABLE_BUILDING"
            | "SPECIAL_BLACKLOTUS_CAPTURE_BUILDING"
            | "SPECIAL_BLACKLOTUS_DISABLE_VEHICLE_HACK"
            | "SPECIAL_BLACKLOTUS_STEAL_CASH_HACK"
                // `Relationship r = getObject()->getRelationship(target); if (r == ALLIES) return;` (:1051-1053).
                if a.target_obj.is_some_and(|t| self.object_relationship(id, t) == Relationship::Allies) =>
            {
                return;
            }
            _ => {}
        }
        // `markSpecialPowerTriggered` -> `triggerSpecialPower`: `aboutToDoSpecialPower` (the script event)
        // and `startPowerRecharge`.
        if let Some(p) = caster_player {
            self.record_special_power(None, false, p, &a.power, id);
        }
        self.start_power_recharge(id, &a.power);
        self.ai_idle(id, CommandSource::Ai);
        // `getObject()->setStatus(OBJECT_STATUS_IS_USING_ABILITY)` (`:992`).
        if let Some(o) = self.objects.get_mut(&id) {
            o.status_bits |= crate::object::status::USING_ABILITY;
        }
    }

    /// `SpecialAbilityUpdate::continuePreparation` (:1143). The per-frame checks: the abort range, a
    /// missing target, and an ally target (a colleague captured it). An infantry capture keeps its
    /// recharge running while it prepares (`startPowerRecharge`, :1192).
    fn continue_preparation(&mut self, id: ObjectId, a: &AbilityState, tpl: &SpecialAbilityTemplate) -> bool {
        if tpl.ability_abort_range < SPECIAL_ABILITY_HUGE_DISTANCE && !self.within_ability_abort_range(id, a, tpl) {
            return false;
        }
        let kind = self.special_power_kind(&a.power).to_string();
        match kind.as_str() {
            "SPECIAL_MISSILE_DEFENDER_LASER_GUIDED_MISSILES" | "SPECIAL_BLACKLOTUS_DISABLE_VEHICLE_HACK" => {
                if !self.target_present_and_not_allied(id, a) {
                    return false;
                }
            }
            "SPECIAL_INFANTRY_CAPTURE_BUILDING" | "SPECIAL_BLACKLOTUS_CAPTURE_BUILDING" => {
                if !self.target_present_and_not_allied(id, a) {
                    return false;
                }
                if kind == "SPECIAL_INFANTRY_CAPTURE_BUILDING" {
                    self.start_power_recharge(id, &a.power);
                }
            }
            _ => {}
        }
        true
    }

    /// The target object exists and is not an ally of the unit (the ALLIES checks of `continuePreparation`).
    /// `getObject()->getRelationship(target)` (SpecialAbilityUpdate.cpp:1204), so an undetected defector
    /// reads as an ally (`defect_l193.rs`).
    fn target_present_and_not_allied(&self, id: ObjectId, a: &AbilityState) -> bool {
        let Some(t) = a.target_obj.filter(|t| self.find_object(*t).is_some()) else { return false };
        if self.find_object(id).is_none() {
            return false;
        }
        self.object_relationship(id, t) != Relationship::Allies
    }

    /// `SpecialAbilityUpdate::startUnpacking` (:786): the unpack countdown, with the variation draw, and
    /// the unit goes `AI_BUSY` for the length of the animation (`ai->aiBusy`, `:809`).
    fn start_unpacking(&mut self, id: ObjectId, a: &mut AbilityState, tpl: &SpecialAbilityTemplate) {
        a.packing = PackingState::Unpacking;
        a.anim_frames = self.pack_anim_frames(tpl.unpack_time, tpl.pack_unpack_variation);
        self.ai_busy(id, CommandSource::Ai);
    }

    /// `SpecialAbilityUpdate::startPacking` (:733): the pack countdown, with the variation draw, and the
    /// unit goes `AI_BUSY` for the length of the animation (`ai->aiBusy`, `:755`).
    fn start_packing(&mut self, id: ObjectId, a: &mut AbilityState, tpl: &SpecialAbilityTemplate) {
        a.packing = PackingState::Packing;
        a.anim_frames = self.pack_anim_frames(tpl.pack_time, tpl.pack_unpack_variation);
        self.ai_busy(id, CommandSource::Ai);
    }

    /// `m_animFrames = time * GameLogicRandomValueReal(1 - f, 1 + f)` (the truncation to `UnsignedInt`).
    fn pack_anim_frames(&mut self, frames: u32, variation: f32) -> u32 {
        let factor = self.rng.value_real(1.0 - variation, 1.0 + variation);
        (frames as f32 * factor) as u32
    }

    /// `SpecialAbilityUpdate::triggerAbilityEffect` (:1257): the experience and skill points for
    /// triggering, then the effect. A capture of a garrisoned building ejects the garrison and does not
    /// defect (`TRIGGER_GARRISON_EJECT`, :1428). The recharge is not restarted here: it starts when
    /// preparation starts (and runs every frame of an infantry capture), as in C++.
    fn trigger_ability_effect(&mut self, source: ObjectId, template: &SpecialAbilityTemplate, target_obj: Option<ObjectId>, target_pos: Coord3D) {
        let now = self.frame;
        let kind = self.special_powers.get(&template.power).map(|t| t.kind.clone()).unwrap_or_default();
        let caster_player = self.find_object(source).map(|o| o.player);
        if template.award_xp_for_triggering != 0 {
            self.add_experience(source, template.award_xp_for_triggering);
        }
        // `skillPoints = m_skillPointsForTriggering != -1 ? m_skillPointsForTriggering : m_awardXPForTriggering`.
        let skill_points = if template.skill_points_for_triggering != -1 { template.skill_points_for_triggering } else { template.award_xp_for_triggering };
        if skill_points > 0 {
            if let Some(p) = caster_player {
                self.add_skill_points(p, skill_points as u32);
            }
        }
        match ability_kind(&kind) {
            AbilityKind::CaptureBuilding => {
                let (Some(target), Some(player)) = (target_obj, caster_player) else { return };
                if self.find_object(target).is_none() {
                    return;
                }
                // "it's been captured by a colleague! we should stop." (`target->getTeam() == object->getTeam()`, :1448)
                if self.same_object_team(source, target) {
                    return;
                }
                let garrisoned = self.find_object(target).and_then(|o| o.template.contain.as_ref()).is_some_and(|c| c.is_garrison());
                if garrisoned {
                    // `contain->removeAllContained(TRUE)`; the building stays with its owner (:1428).
                    for unit in self.contained_units(target).to_vec() {
                        self.remove_from_contain(target, unit);
                    }
                } else {
                    // `target->defect(defaultTeam, 1)` (:1468): the undetected-defector flag for one frame
                    // (ALLIES to the capturing units' next preparation check), the defection timer, then
                    // `setTeam`, the production refund and the rider kick-out (`Object.cpp:6315-6320`, every
                    // OpenContain-based module but cave and tunnel). See `defect_l193.rs`. A garrison never
                    // reaches here (the branch above empties it first, as `removeAllContained(TRUE)` does).
                    self.object_defect(target, player, 1);
                    // `SPECIAL_BLACKLOTUS_CAPTURE_BUILDING` restarts its recharge here (:1475); the
                    // infantry capture restarts its recharge every frame of preparation instead.
                    if kind == "SPECIAL_BLACKLOTUS_CAPTURE_BUILDING" {
                        self.start_power_recharge(source, &template.power);
                    }
                }
            }
            AbilityKind::Disable => {
                if let Some(target) = target_obj {
                    // `SpecialAbilityUpdate.cpp:1386`: the target is disabled `DISABLED_HACKED`
                    // for `effectDuration` frames.
                    self.set_disabled_until(target, crate::disabled::DisabledType::Hacked, now + template.effect_duration);
                }
            }
            AbilityKind::StealCash => {
                if let (Some(target), Some(player)) = (target_obj, caster_player) {
                    if let Some(vp) = self.find_object(target).map(|o| o.player) {
                        if vp != player {
                            let amount = if template.effect_value > 0 { template.effect_value as u32 } else { 1000 };
                            let taken = self.withdraw_cash(vp, amount);
                            self.deposit_cash(player, taken);
                            // `SpecialAbilityUpdate` steal-cash (SpecialAbilityUpdate.cpp:1510): `addMoneyEarned(cash)`.
                            self.score_money_earned(player, taken);
                        }
                    }
                }
            }
            AbilityKind::Charge | AbilityKind::Napalm => {
                if let Some(obj) = template.special_object.clone() {
                    let owner = caster_player.unwrap_or(0);
                    self.create_object(&obj, target_pos, 0.0, owner);
                }
            }
            AbilityKind::MissileDefender => {
                // `SpecialAbilityUpdate.cpp:1302-1306`: `aiAttackObject` on the secondary slot, whose shots end in
                // `fireCurrentWeapon` (`Object.cpp:1510`): a reloaded shot releases the temporary lock.
                if let Some(target) = target_obj {
                    if self.fire_weapon(source, 1, AttackTarget::Object(target)) {
                        self.release_weapon_lock(source, crate::weapon::WeaponLockType::Temporarily);
                    }
                }
            }
            AbilityKind::Other => {}
        }
        if let Some(player) = caster_player {
            // `SpecialPowerStore::onSpecialPowerCompleted` for the ability path.
            self.record_special_power(None, true, player, &template.power, source);
        }
    }

    fn apply_special_power(&mut self, source: ObjectId, module: &SimSpecialPower, target: SpecialPowerTarget, angle: Option<f32>) {
        match &module.effect {
            SpecialPowerEffect::Ocl { ocl, create_at_target, create_location } => {
                let Some(ocl) = ocl.clone() else { return };
                let pos = self.target_position(target);
                // `OCLSpecialPower::doSpecialPowerAtLocation` (OCLSpecialPower.cpp:190-244) is the only caller that
                // resolves the `CreateLocation`; the no-location `doSpecialPower` (:237-244) uses the source's position.
                let location = pos.map(|_| *create_location);
                self.run_ocl_at_angle(&ocl, source, pos, *create_at_target, angle, location);
            }
            SpecialPowerEffect::FireWeapon => {
                let attack = match target {
                    SpecialPowerTarget::Object(id) => AttackTarget::Object(id),
                    SpecialPowerTarget::Position(p) => AttackTarget::Ground(p),
                    SpecialPowerTarget::None => return,
                };
                // `FireWeaponPower::doSpecialPower*` (`FireWeaponPower.cpp:106, :132, :161`): `reloadAllAmmo(TRUE)`
                // before the attack order, so the power always fires with a full clip. The shot itself is
                // `aiAttackObject` -> `fireCurrentWeapon` (`AIStates.cpp:5265`), whose reloaded shot releases the
                // temporary lock (`Object.cpp:1510`).
                self.reload_all_ammo(source);
                if self.fire_weapon(source, 0, attack) {
                    self.release_weapon_lock(source, crate::weapon::WeaponLockType::Temporarily);
                }
            }
            SpecialPowerEffect::CashHack { amount } => {
                let Some(player) = self.find_object(source).map(|o| o.player) else { return };
                // Steal from the target's owner when there is one, otherwise create the cash.
                if let SpecialPowerTarget::Object(victim) = target {
                    if let Some(vp) = self.find_object(victim).map(|o| o.player) {
                        if vp != player {
                            let amount = (*amount).max(0) as u32;
                            let taken = self.withdraw_cash(vp, amount);
                            self.deposit_cash(player, taken);
                            // `CashHackSpecialPower::doSpecialPowerAtObject` (CashHackSpecialPower.cpp:158): `addMoneyEarned(cash)`.
                            self.score_money_earned(player, taken);
                            return;
                        }
                    }
                }
                self.deposit_cash(player, (*amount).max(0) as u32);
            }
            SpecialPowerEffect::CleanupArea { range } => {
                let pos = self.target_position(target).or_else(|| self.find_object(source).map(|o| o.position));
                if let Some(pos) = pos {
                    self.set_cleanup_area(source, pos, *range);
                }
            }
            SpecialPowerEffect::Baikonur { detonation_object } => {
                if let Some(t) = detonation_object.clone() {
                    let owner = self.find_object(source).map(|o| o.player).unwrap_or(0);
                    if let Some(pos) = self.target_position(target) {
                        self.create_object(&t, pos, 0.0, owner);
                    }
                }
            }
            SpecialPowerEffect::SpyVision { base, bonus_per_captured, max } => {
                let count = self.find_object(source).map_or(0, |o| o.contained.len() as u32);
                let mut duration = base + count * bonus_per_captured;
                if *max > 0 {
                    duration = duration.min(*max);
                }
                self.activate_spy_vision(source, duration);
            }
            SpecialPowerEffect::ParticleUplink => {
                if let Some(pos) = self.target_position(target) {
                    self.start_particle_uplink(source, pos);
                }
            }
            SpecialPowerEffect::SpectreDeployment => {
                if let Some(pos) = self.target_position(target) {
                    self.start_spectre_deployment(source, pos);
                }
            }
            SpecialPowerEffect::CashBounty => {}
        }
    }

    fn target_position(&self, target: SpecialPowerTarget) -> Option<Coord3D> {
        match target {
            SpecialPowerTarget::Position(p) => Some(p),
            SpecialPowerTarget::Object(id) => self.find_object(id).map(|o| o.position),
            SpecialPowerTarget::None => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logic::GameLogic;
    use crate::template::ObjectTemplate;
    use crate::terrain::TerrainLogic;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    /// A shared-timer power (`SharedSyncedTimer`, a player's timer) with a 100-frame reload, and one source
    /// object for player 0 that carries it.
    fn shared_power_world() -> (GameLogic, ObjectId) {
        let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
        let mut map = BTreeMap::new();
        map.insert(
            "P".to_string(),
            Arc::new(SimSpecialPowerTemplate {
                name: "P".to_string(),
                kind: String::new(),
                reload_time: 100,
                required_science: None,
                shared_synced_timer: true,
                public_timer: false,
                radius_cursor_radius: 0.0,
            }),
        );
        g.set_special_powers(map);
        let mut t = ObjectTemplate::new("Source");
        t.special_powers = vec![Arc::new(SimSpecialPower { power: "P".into(), effect: SpecialPowerEffect::FireWeapon, starts_paused: false, power_module: true })];
        let src = g.create_object(&Arc::new(t), crate::math::Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
        (g, src)
    }

    /// A player's fire with no shared timer yet: the GUI's readiness test starts the timer (`getOrStart...`,
    /// Player.cpp:3368) and the fire restarts it at `now + reload` (`resetOrStart...`, Player.cpp:3324). Without the
    /// start the fire inserts `now` and the power is ready again the next frame (the second spy satellite of AN3
    /// seed 1). Revert check: remove the `!forced` start in `do_special_power`: the ready-at assert fails (3, not 103).
    #[test]
    fn a_player_fire_with_no_timer_yet_keeps_the_shared_cooldown() {
        let (mut g, src) = shared_power_world();
        g.run_frames(3);
        let now = g.frame();
        assert!(g.special_power_is_ready(src, "P"), "a fresh shared timer reads as ready");
        assert!(g.do_special_power(0, src, "P", SpecialPowerTarget::Position(crate::math::Coord3D::new(60.0, 60.0, 0.0)), None, false, None), "the player's fire goes through");
        assert_eq!(g.special_power_ready_at(src, "P"), now + 100, "the fire restarts the shared timer at now + reload");
        assert!(!g.special_power_is_ready(src, "P"), "the power is not ready again the next frame");
    }

    /// A scripted fire does not query readiness (Object.cpp:5431, ScriptActions.cpp:4232), so with no timer the
    /// `resetOrStartSpecialPowerReadyFrame` inserts `now` (Player.cpp:3324-3343) and the power stays ready.
    #[test]
    fn a_scripted_fire_with_no_timer_leaves_the_power_ready() {
        let (mut g, src) = shared_power_world();
        g.run_frames(3);
        let now = g.frame();
        let mut engine = crate::script::ScriptEngine::default();
        assert!(g.script_do_special_power_at_location(0, src, "P", crate::math::Coord3D::new(60.0, 60.0, 0.0), &mut engine), "the script fire goes through");
        assert_eq!(g.special_power_ready_at(src, "P"), now, "a missing timer is inserted at now, not now + reload");
        assert!(g.special_power_is_ready(src, "P"));
    }

    #[test]
    fn starting_the_shared_timer_keeps_an_existing_one() {
        let (mut g, src) = shared_power_world();
        g.run_frames(3);
        let now = g.frame();
        g.special_power_start_shared_timer(src, "P");
        assert!(g.script_do_special_power_at_location(0, src, "P", crate::math::Coord3D::new(60.0, 60.0, 0.0), &mut crate::script::ScriptEngine::default()));
        g.run_frames(10);
        g.special_power_start_shared_timer(src, "P");
        assert_eq!(g.special_power_ready_at(src, "P"), now + 100, "an existing timer is not restarted by a later check");
    }
}
