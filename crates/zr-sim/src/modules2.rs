//! Round-2 gameplay modules: crush (`SquishCollide` + `CrusherLevel`/`CrushableLevel`), `ToppleUpdate`,
//! income (`AutoDepositUpdate`), `BaseRegenerateUpdate`, `FireWeaponUpdate`, `VeterancyGainCreate`,
//! `HordeUpdate`, `EjectPilotDie`, `SpawnBehavior` + `SlavedUpdate`, and crates (`CreateCrateDie` +
//! `CrateCollide`). Each is driven from [`GameLogic::update_behaviors`] or a damage/death hook.

use std::sync::Arc;

use crate::body::DamageInfo;
use crate::damage::{DamageType, DeathType, HUGE_DAMAGE_AMOUNT};
use crate::logic::GameLogic;
use crate::math::{cos, sin};
use crate::module::{UPDATE_SLEEP_FOREVER, UPDATE_SLEEP_NONE};
use crate::object::ObjectId;
use crate::sleepy_families::FamilyKind;
use crate::weapon::WeaponTemplate;

/// `SLAVED_UPDATE_RATE` (SlavedUpdate.h:35): `LOGICFRAMES_PER_SECOND/4` frames between the slave body's runs.
const SLAVED_UPDATE_RATE: i32 = (crate::math::LOGICFRAMES_PER_SECOND / 4) as i32;

/// `ToppleUpdateModuleData` (sim units).
#[derive(Clone, Debug, PartialEq)]
pub struct ToppleTemplate {
    pub kill_when_start_toppling: bool,
    pub kill_when_finished_toppling: bool,
    pub topple_left_or_right_only: bool,
}

/// `AutoDepositUpdateModuleData` (sim units).
#[derive(Clone, Debug, PartialEq)]
pub struct AutoDepositTemplate {
    pub deposit_timing: u32,
    pub deposit_amount: i32,
    /// `InitialCaptureBonus` (`AutoDepositUpdate.cpp:129-151`): paid to the player that captures the building, once the
    /// module has had its first due update (`m_awardInitialCaptureBonus`).
    pub initial_capture_bonus: i32,
    pub upgraded_boost: Vec<(String, i32)>,
}

/// `FireWeaponUpdateModuleData` (sim units).
#[derive(Clone, Debug)]
pub struct FireWeaponTemplate {
    pub weapon: Arc<WeaponTemplate>,
    pub initial_delay: u32,
    pub exclusive_weapon_delay: u32,
}

/// `HordeUpdateModuleData` (sim units).
#[derive(Clone, Debug, PartialEq)]
pub struct HordeTemplate {
    pub update_rate: u32,
    pub kind_of: u64,
    pub count: i32,
    pub radius: f32,
    pub rub_off_radius: f32,
    pub allies_only: bool,
    pub exact_match: bool,
    /// `AllowedNationalism`: whether the member's player's `Upgrade_Nationalism` may apply
    /// (`HordeUpdate.cpp:170`, read by `evaluateMoraleBonus`).
    pub allowed_nationalism: bool,
}

/// `EjectPilotDieModuleData` (sim units): the OCL run on death.
#[derive(Clone, Debug, Default)]
pub struct EjectPilotTemplate {
    pub air_ocl: Option<Arc<crate::ocl::OclTemplate>>,
    pub ground_ocl: Option<Arc<crate::ocl::OclTemplate>>,
}

/// `VeterancyGainCreateModuleData` (sim units).
#[derive(Clone, Debug, PartialEq)]
pub struct VeterancyGainTemplate {
    pub starting_level: u8,
    pub science_required: Option<String>,
}

/// `SpawnBehaviorModuleData` (sim units).
#[derive(Clone, Debug, PartialEq)]
pub struct SpawnTemplate {
    pub spawn_number: i32,
    pub spawn_replace_delay: u32,
    pub one_shot: bool,
    pub spawn_template_names: Vec<String>,
    pub spawned_require_spawner: bool,
    /// `InitialBurst` (`m_initialBurst`): with a value above zero the first replacement entries are the
    /// `runtimeProduced` flag, not the list index (`SpawnBehavior.cpp:212-220`).
    pub initial_burst: i32,
    /// `AggregateHealth` (`m_aggregateHealth`): the spawner dies with its last spawn (`onSpawnDeath`, `:772-780`).
    pub aggregate_health: bool,
}

/// `SpawnBehavior`'s own members (`SpawnBehavior.h:186-214`). Defaulted on creation; the first update fills in the
/// rest (`times_inited`, `one_shot_countdown`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpawnRuntime {
    /// `m_framesToWait`: updates left before the next spawn check. Starts at 0, so the first update does not wait.
    pub frames_to_wait: i32,
    /// `m_replacementTimes`: one entry per spawn still owed. The first update makes `SpawnNumber` entries
    /// (`SpawnBehavior.cpp:203-225`); a spawn's death adds `SpawnReplaceDelay + now` (`onSpawnDeath`, `:765-766`).
    /// An entry is removed when its spawn is created, which waits until the frame is past the entry.
    pub replacement_times: Vec<u32>,
    /// `m_initialBurstTimesInited`.
    pub times_inited: bool,
    /// `!m_active`: set by `stopSpawning` (a one-shot batch that is done, or a hole being rebuilt).
    pub stopped: bool,
    /// `m_oneShotCountdown`: spawns left in a one-shot batch, `-1` when the spawner is not one-shot.
    pub one_shot_countdown: i32,
    /// `m_templateNameIterator`: the index of the next `SpawnTemplateName` (wraps to the start).
    pub template_index: usize,
}

/// `SlavedUpdateModuleData` (sim units).
#[derive(Clone, Debug, PartialEq)]
pub struct SlavedTemplate {
    pub guard_max_range: i32,
    pub guard_wander_range: i32,
    pub attack_range: i32,
    pub repair_rate_per_second: f32,
    pub stay_on_same_layer_as_master: bool,
}

/// `CrateCollideModuleData` (sim units): the pickup effect.
#[derive(Clone, Debug, PartialEq)]
pub enum CrateEffect {
    Money { money_provided: u32, upgrade_boost: Vec<(String, i32)> },
    Heal,
    Veterancy { effect_range: u32, adds_owner_veterancy: bool, is_pilot: bool },
    Unit { unit_count: u32, unit_name: String },
    Salvage { weapon_chance: f32, level_chance: f32, money_chance: f32, min_money: i32, max_money: i32 },
    Shroud,
    ConvertToCarBomb,
    ConvertToHijackedVehicle,
    SabotageCommandCenter,
    SabotageFakeBuilding,
    SabotageInternetCenter { sabotage_frames: u32 },
    SabotageMilitaryFactory { sabotage_frames: u32 },
    SabotagePowerPlant { power_sabotage_frames: u32 },
    SabotageSuperweapon,
    SabotageSupplyCenter { steal_cash_amount: u32 },
    SabotageSupplyDropzone { steal_cash_amount: u32 },
    Other,
}

/// A crate's `CrateCollide` pickup (`CrateCollideModuleData` plus the subclass effect).
#[derive(Clone, Debug, PartialEq)]
pub struct CrateCollideTemplate {
    /// `RequiredKindOf` / `ForbiddenKindOf` (`isValidToExecute`).
    pub required_kind_of: u64,
    pub forbidden_kind_of: u64,
    /// `ForbidOwnerPlayer`: the dead thing's own player cannot pick it up.
    pub forbid_owner_player: bool,
    /// `BuildingPickup`: a structure may pick it up without an AI update.
    pub building_pickup: bool,
    /// `HumanOnly`: mission crate, humans only.
    pub human_only: bool,
    /// `AllowMultiPickup`: multiple objects may collect it on the same frame.
    pub allow_multi_pickup: bool,
    /// `PickupScience`: the picker's player must have this science.
    pub pickup_science: Option<String>,
    pub effect: CrateEffect,
}

impl Default for CrateCollideTemplate {
    fn default() -> Self {
        CrateCollideTemplate {
            required_kind_of: 0,
            forbidden_kind_of: 0,
            forbid_owner_player: false,
            building_pickup: false,
            human_only: false,
            allow_multi_pickup: true,
            pickup_science: None,
            effect: CrateEffect::Other,
        }
    }
}

/// `FireWeaponCollideModuleData` (sim units): shoot whatever collides with me.
#[derive(Clone, Debug)]
pub struct FireWeaponCollideTemplate {
    pub weapon: Arc<WeaponTemplate>,
    pub fire_once: bool,
    /// `RequiredStatus` / `ForbiddenStatus` as [`crate::object::status`] bits (`ObjectStatusMaskType`).
    pub required_status: u64,
    pub forbidden_status: u64,
}

/// `CrushDie` / `SquishCollide` info carried on the template.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CrushInfo {
    pub crusher_level: u8,
    pub crushable_level: u8,
    pub squish_collide: bool,
}

/// `Object::canCrushOrSquish(other, TEST_CRUSH_OR_SQUISH)` (`Object.cpp:1121-1200`) without its ALLIES test, which the
/// caller makes (`Object.cpp:1139-1145`): a `DISABLED_UNMANNED` crusher, or one with no crusher level, cannot crush; a
/// `SquishCollide` victim is squished (`:1162-1177`); otherwise the crusher level must exceed the victim's
/// `CrushableLevel` (`:1187-1196`). The pathfinder reuses it for `checkDestination` and `checkForMovement`
/// (`AIPathfind.cpp:5082-5086`, `:5201-5205`).
pub fn crush_or_squish(crusher: CrushInfo, crusher_unmanned: bool, victim: CrushInfo) -> bool {
    if crusher_unmanned || crusher.crusher_level == 0 {
        return false;
    }
    victim.squish_collide || crusher.crusher_level > victim.crushable_level
}

/// `PropagandaTowerBehaviorModuleData` (sim units).
#[derive(Clone, Debug, PartialEq)]
pub struct PropagandaTemplate {
    pub radius: f32,
    pub delay_between_updates: u32,
    pub heal_percent_each_second: f32,
    pub upgraded_heal_percent_each_second: f32,
    pub upgrade_required: Option<String>,
    pub affects_self: bool,
}

/// `RepairDockUpdateModuleData` (sim units).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RepairDockTemplate {
    pub frames_for_full_heal: f32,
    /// `NumberApproachPositions` (`DockUpdateModuleData`).
    pub number_approach_positions: i32,
    /// `AllowsPassthrough` (`DockUpdateModuleData`).
    pub allows_passthrough: bool,
}

/// `OverchargeBehaviorModuleData` (sim units) plus the object's rod timer, which `enable()` drives.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OverchargeTemplate {
    pub health_percent_to_drain_per_second: f32,
    pub not_allowed_when_health_below_percent: f32,
    /// The object's `PowerPlantUpdateModuleData::m_rodsExtendTime` (`PowerPlantUpdate.h`), 0 when it has no
    /// `PowerPlantUpdate` module: `OverchargeBehavior::enable` extends/retracts the rods through
    /// `PowerPlantUpdateInterface`, and both modules are separate heap entries (see [`ObjectTemplate::power_plant`]).
    pub rods_extend_time: u32,
}

/// `PowerPlantUpdateModuleData` (`PowerPlantUpdate.h`): the rod-extension timer `extendRods(TRUE)` wakes on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PowerPlantTemplate {
    pub rods_extend_time: u32,
}

/// `ProneUpdateModuleData` (sim units).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProneTemplate {
    pub damage_to_frames_ratio: f32,
}

/// `CountermeasuresBehaviorModuleData` (sim units).
#[derive(Clone, Debug, PartialEq)]
pub struct CountermeasuresTemplate {
    pub flare_template_name: Option<String>,
    pub evasion_rate: f32,
    pub volley_size: u32,
    /// `VolleyArcAngle`, radians.
    pub volley_arc_angle: f32,
    pub volley_velocity_factor: f32,
    pub number_of_volleys: u32,
    pub delay_between_volleys: u32,
    pub reload_time: u32,
    pub must_reload_at_airfield: bool,
    pub missile_decoy_delay: u32,
    /// `ReactionLaunchLatency` (frames).
    pub countermeasure_reaction_frames: u32,
    /// `UpgradeMuxData`: the behavior only runs once active.
    pub starts_active: bool,
    pub triggered_by: Vec<String>,
    pub conflicts_with: Vec<String>,
    /// The flare `LifetimeUpdate` (frames), resolved from the flare template.
    pub flare_lifetime_frames: u32,
}

/// `JetAIUpdateModuleData` (sim units): the airfield state machine.
#[derive(Clone, Debug, PartialEq)]
pub struct JetAiTemplate {
    pub needs_runway: bool,
    pub keeps_parking_space_when_airborne: bool,
    pub takeoff_pause: u32,
    pub return_to_base_idle_time: u32,
    /// `OutOfAmmoDamagePerSecond` as a fraction of max health.
    pub out_of_ammo_damage_per_second: f32,
    /// `TakeoffDistForMaxLift` (parsed but unused by the engine, as in `JetAIUpdate.cpp`).
    pub takeoff_dist_for_max_lift: f32,
    /// `MinHeight`: the drawable is lifted this far off the ground while taxiing (client only).
    pub min_height: f32,
    /// `ParkingOffset`.
    pub parking_offset: f32,
    pub sneaky_offset_when_attacking: f32,
    /// `AttackLocomotorType` / `ReturnForAmmoLocomotorType`.
    pub attacking_loco: crate::template::LocomotorSet,
    pub returning_loco: crate::template::LocomotorSet,
    pub attack_loco_persist_time: u32,
    pub attackers_miss_persist_time: u32,
}

/// `ParkingPlaceBehaviorModuleData` (sim units).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParkingPlaceTemplate {
    pub num_rows: i32,
    pub num_cols: i32,
    pub has_runways: bool,
    pub heal_amount_per_second: f32,
    pub approach_height: f32,
    pub landing_deck_height_offset: f32,
    pub park_in_hangars: bool,
}

/// `TransitionDamageFXModuleData` (sim units): a client FX hook on body state change.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct TransitionDamageTemplate {
    /// `Damaged` / `ReallyDamaged` / `Rubble` FX list names.
    pub damaged_fx: Vec<String>,
    pub really_damaged_fx: Vec<String>,
    pub rubble_fx: Vec<String>,
}

/// `MinefieldBehaviorModuleData` (sim units).
#[derive(Clone, Debug, Default)]
pub struct MinefieldTemplate {
    pub detonation_weapon: Option<Arc<WeaponTemplate>>,
    pub regenerates: bool,
    pub workers_detonate: bool,
    pub num_virtual_mines: u32,
}

impl GameLogic {
    /// Whether `crusher` can crush `victim` (`Object::canCrushOrSquish`, `Object.cpp`). The ALLIES test is
    /// `getRelationship(otherObj) == ALLIES` ("Friends don't let friends crush friends"): an allied player's unit
    /// is not crushed, and the same player is allied with itself (`Relationship::Allies` in `relationship`).
    pub fn can_crush_or_squish(&self, crusher: ObjectId, victim: ObjectId) -> bool {
        let (Some(a), Some(b)) = (self.objects.get(&crusher), self.objects.get(&victim)) else { return false };
        if self.relationship(a.player, b.player) == crate::damage::Relationship::Allies {
            return false;
        }
        crush_or_squish(a.template.crush, a.unmanned(), b.template.crush)
    }

    /// `ToppleUpdate::applyTopplingForce` reduced to the port's fixed fall timer: `setWakeFrame(NONE)`, then
    /// `KillWhenStartToppling`'s `setWakeFrame(FOREVER); kill(); return` (`ToppleUpdate.cpp:139-155`), or the
    /// toppled flags and the delayed death. The angular fall, the bounce data and the stump are not ported.
    pub fn topple(&mut self, id: ObjectId, kill_immediately: bool) {
        let Some(t) = self.objects.get(&id).and_then(|o| o.template.topple.clone()) else { return };
        if self.objects.get(&id).is_some_and(|o| o.behavior.toppled) || self.is_effectively_dead(id) {
            return;
        }
        let now = self.frame;
        // `setWakeFrame(getObject(), UPDATE_SLEEP_NONE);` (`:146`).
        self.wake_topple(id, now.saturating_add(UPDATE_SLEEP_NONE));
        if t.kill_when_start_toppling {
            // `setWakeFrame(getObject(), UPDATE_SLEEP_FOREVER); getObject()->kill(); return;` (`:150-155`):
            // the module never runs again and the object dies a normal death of its current health
            // (`Object::kill`, Object.cpp:2010-2024: `Object::kill(DAMAGE_UNRESISTABLE, DEATH_NORMAL)`).
            self.wake_topple(id, now.saturating_add(UPDATE_SLEEP_FOREVER));
            // `Object::kill` (`Object.cpp:2010-2024`) hands `attemptDamage` the object's max health with
            // `m_kill` set; the body then caps the applied hit at its current health (`ActiveBody.cpp:527-535`),
            // so that is the amount the port passes directly (`DamageInfo` has no kill flag).
            let amount = self.objects.get(&id).and_then(|o| o.body.as_ref()).map_or(0.0, |b| b.health());
            let hit = DamageInfo { source: ObjectId::INVALID, amount, kind: DamageType::Unresistable };
            let has_body = self.objects.get(&id).is_some_and(|o| o.body.is_some());
            if has_body {
                self.damage_object_with_death(id, hit, DeathType::Normal);
            }
            if self.objects.get(&id).is_some_and(|o| !o.destroyed && !o.effectively_dead && !o.is_dying()) {
                // `Object::kill` needs a body (`getBodyModule()->getMaxHealth()`); the die pipeline runs
                // directly for a bodyless object, as `kill_object` does. `Object::kill` has no source (`:2017`).
                self.on_die(id, DeathType::Normal, 0.0, ObjectId::INVALID);
            }
            return;
        }
        if let Some(o) = self.objects.get_mut(&id) {
            o.behavior.toppled = true;
            o.behavior.topple_kill_frame = now + if kill_immediately { 0 } else { 30 };
        }
    }

    /// `AutoDepositUpdate::update` (AutoDepositUpdate.cpp:143-202), run from the sleepy heap: the deposit
    /// on its timer. The module is awake every frame (`UPDATE_SLEEP_NONE` on every path) and the constructor
    /// set `m_depositOnFrame = now + DepositTiming` (AutoDepositUpdate.cpp:104).
    ///
    /// The timer advances before the neutral / zero-amount / under-construction returns (:147-166), so a
    /// neutral or unfinished derrick still re-arms its deposit frame on every due frame.
    pub(crate) fn auto_deposit_update(&mut self, id: ObjectId, now: u32) -> u32 {
        let Some(t) = self.objects.get(&id).and_then(|o| o.template.auto_deposit.clone()) else {
            return crate::module::UPDATE_SLEEP_FOREVER;
        };
        let next = match self.objects.get(&id) {
            Some(o) if !o.is_dying() => o.behavior.deposit_next,
            _ => return crate::module::UPDATE_SLEEP_NONE,
        };
        if now < next {
            return crate::module::UPDATE_SLEEP_NONE;
        }
        // `if (!m_initialized) { m_awardInitialCaptureBonus = TRUE; m_initialized = TRUE; }` (`:143-150`): the capture
        // bonus is armed by the first due update, not by the constructor ("during load the team is set").
        if let Some(o) = self.objects.get_mut(&id) {
            if !o.behavior.deposit_initialized {
                o.behavior.deposit_initialized = true;
                o.behavior.deposit_initial_bonus_armed = true;
            }
        }
        // `m_depositOnFrame = TheGameLogic->getFrame() + modData->m_depositFrame` (`:155`), before the
        // three guards below.
        if let Some(o) = self.objects.get_mut(&id) {
            o.behavior.deposit_next = now.saturating_add(t.deposit_timing);
        }
        // `if (getObject()->isNeutralControlled() || modData->m_depositAmount <= 0) return`
        // (`AutoDepositUpdate.cpp:157-158`): a neutral-owned object pays nobody.
        if self.objects.get(&id).is_some_and(|o| self.neutral_player() == Some(o.player)) {
            return crate::module::UPDATE_SLEEP_NONE;
        }
        if t.deposit_amount <= 0 {
            return crate::module::UPDATE_SLEEP_NONE;
        }
        // `if (getObject()->getConstructionPercent() != CONSTRUCTION_COMPLETE) return` (`:161-162`).
        if self.objects.get(&id).is_some_and(|o| o.under_construction) {
            return crate::module::UPDATE_SLEEP_NONE;
        }
        let player = self.objects.get(&id).map(|o| o.player);
        let boost: i32 = t
            .upgraded_boost
            .iter()
            .find(|(n, _)| {
                self.objects.get(&id).is_some_and(|o| o.upgrades.contains(n))
                    || player.is_some_and(|p| self.player_upgrade_set(p).is_some_and(|s| s.contains(n)))
            })
            .map_or(0, |(_, b)| *b);
        let amount = (t.deposit_amount + boost).max(0) as u32;
        if amount > 0 {
            if let Some(p) = player.and_then(|p| self.players.get_mut(&p)) {
                p.deposit(amount);
            }
            // `AutoDepositUpdate::update` (AutoDepositUpdate.cpp:169): `addMoneyEarned(m_depositAmount)`.
            if let Some(p) = player {
                self.score_money_earned(p, amount);
            }
        }
        crate::module::UPDATE_SLEEP_NONE
    }

    /// `BaseRegenerateUpdate::update` (BaseRenerateUpdate.cpp:113-146), run from the sleepy heap: heal the
    /// object (base buildings) on the engine's three-frame cadence. Returns the module's sleep: `NONE`
    /// while under construction, `FOREVER` when sold or at full health, else `HEAL_RATE` (3).
    pub(crate) fn base_regenerate_update(&mut self, id: ObjectId, _now: u32) -> u32 {
        let pct = self.rules.base_regen_health_percent_per_second;
        let Some(o) = self.objects.get(&id) else { return crate::module::UPDATE_SLEEP_FOREVER };
        if o.under_construction {
            return crate::module::UPDATE_SLEEP_NONE;
        }
        if o.status_mask(self.frame) & crate::object::status::SOLD != 0 {
            return crate::module::UPDATE_SLEEP_FOREVER;
        }
        let Some(b) = o.body.as_ref() else { return crate::module::UPDATE_SLEEP_FOREVER };
        if b.health() >= b.max_health() {
            return crate::module::UPDATE_SLEEP_FOREVER;
        }
        // `HEAL_RATE` frames' worth of `BaseRegenHealthPercentPerSecond` (`:137-142`).
        let amount = 3.0 * b.max_health() * pct / 30.0;
        self.heal_object(id, amount);
        3
    }

    /// `BaseRegenerateUpdate::onDamage` (BaseRenerateUpdate.cpp:95-106): a non-healing hit wakes the module
    /// after `BaseRegenDelay`; when the rule is off (or the hit is a heal) it sleeps forever.
    pub(crate) fn base_regen_on_damage(&mut self, id: ObjectId, info: &DamageInfo) {
        if self.objects.get(&id).is_none_or(|o| !o.template.base_regenerate) {
            return;
        }
        let now = self.frame;
        let when = if self.rules.base_regen_health_percent_per_second > 0.0
            && info.kind != crate::damage::DamageType::Healing
        {
            now.saturating_add(self.rules.base_regen_delay)
        } else {
            UPDATE_SLEEP_FOREVER
        };
        self.wake_family(id, FamilyKind::BaseRegenerate, now, when);
    }

    /// `Player::becomingTeamMember` -> `AutoDepositUpdate::awardInitialCaptureBonus` (`Player.cpp:1036-1049`,
    /// `AutoDepositUpdate.cpp:113-151`): re-arm the deposit timer on every team change and pay the
    /// `InitialCaptureBonus` once, to a non-neutral player, if the module's first due update has armed it.
    pub(crate) fn auto_deposit_on_team_member(&mut self, id: ObjectId) {
        let now = self.frame;
        let (timing, bonus, armed, player) = match self.objects.get(&id) {
            Some(o) => match (&o.template.auto_deposit, o.behavior.deposit_initial_bonus_armed) {
                (Some(t), armed) => (t.deposit_timing, t.initial_capture_bonus, armed, o.player),
                (None, _) => return,
            },
            None => return,
        };
        if let Some(o) = self.objects.get_mut(&id) {
            o.behavior.deposit_next = now.saturating_add(timing);
        }
        if !armed || bonus <= 0 || self.neutral_player() == Some(player) {
            return;
        }
        let amount = bonus as u32;
        if let Some(p) = self.players.get_mut(&player) {
            p.deposit(amount);
        }
        self.score_money_earned(player, amount);
        if let Some(o) = self.objects.get_mut(&id) {
            o.behavior.deposit_initial_bonus_armed = false;
        }
    }

    /// `SpawnBehavior::onDamage` (`SpawnBehavior.cpp:797-821`): tell every spawn that its slaver was hurt, so a
    /// spawn with a `SlavedUpdate` can react (`SlavedUpdate::onSlaverDamage` -> `AIUpdateInterface::aiGoProne` ->
    /// its own `ProneUpdate::goProne`, `SlavedUpdate.cpp:100-106`, `AIUpdate.cpp:3973-3981`,
    /// `ProneUpdate.cpp:92-101`). `dealt` is the damage the *slaver* took (`damageInfo->out.m_actualDamageDealt`).
    pub(crate) fn spawn_behavior_on_damage(&mut self, id: ObjectId, dealt: f32) {
        let spawns = match self.objects.get(&id) {
            Some(o) if o.template.spawn.is_some() => o.spawned.clone(),
            _ => return,
        };
        for sid in spawns {
            // `SlavedUpdate::onSlaverDamage` needs the spawn's `AIUpdateInterface` (`SlavedUpdate.cpp:102-105`) before it
            // can reach the spawn's `ProneUpdate`.
            if self.objects.get(&sid).is_some_and(|o| o.template.ai_update.is_some()) {
                self.on_prone_damage(sid, dealt);
            }
        }
    }

    /// `SpawnBehavior::onDie` (`SpawnBehavior.cpp:134-189`): each spawn's `SlavedUpdate` hears that its slaver died
    /// (`onSlaverDie` -> `stopSlavedEffects`, `SlavedUpdate.cpp:94-97`, `:736-744`), the spawn's producer is
    /// invalidated, and with `SpawnedRequireSpawner` every live spawn is killed (`Object::kill`).
    pub(crate) fn spawn_behavior_on_die(&mut self, id: ObjectId, death: DeathType) {
        let Some(t) = self.objects.get(&id).and_then(|o| o.template.spawn.clone()) else { return };
        // `if (modData->m_dieMuxData.isDieApplicable(getObject(), damageInfo) == FALSE) return;` (`:139-140`).
        let (veterancy, status) = self.objects.get(&id).map_or((0u8, 0u64), |o| (0, o.status_mask(self.frame)));
        if let Some(mux) = self.objects.get(&id).and_then(|o| o.template.spawn_die_mux) {
            if !mux.is_applicable(death, veterancy, status) {
                return;
            }
        }
        let spawns = self.objects.get(&id).map(|o| o.spawned.clone()).unwrap_or_default();
        for sid in &spawns {
            if let Some(s) = self.objects.get_mut(sid) {
                if s.template.slaved.is_some() {
                    // `SlavedUpdate::stopSlavedEffects`: no slaver, no guard point, and the slave is selectable
                    // again (`:736-744`). The port keeps the slaver in `Object::master`.
                    s.master = None;
                    s.status_bits &= !crate::object::status::UNSELECTABLE;
                    s.disabled_mask &= !crate::disabled::DisabledType::Held.bit();
                }
                // `currentSpawn->setProducer(nullptr)` (`:162`).
                s.producer_id = ObjectId::INVALID;
            }
        }
        if t.spawned_require_spawner {
            for sid in spawns {
                if self.objects.get(&sid).is_some_and(|o| !o.destroyed && !o.effectively_dead) {
                    self.kill_object(sid);
                }
            }
        }
    }

    /// `SpawnBehavior::computeAggregateStates` (`SpawnBehavior.cpp:862-1020`): while `AggregateHealth`, the spawner's
    /// body health is the aggregate of its spawns — `setInitialHealth(100 * sum(health) / (avg(maxHealth) *
    /// SpawnNumber))`, where the `Real` product truncates into the `Int` parameter (`ActiveBody.cpp:883-895`) — and
    /// the spawns' veterancy is mirrored onto the spawner (and the spawner's onto them, `:911-920`). It runs every
    /// frame from `update` (`:196-200`), whose first call is the frame *after* creation (`UPDATE_SLEEP_NONE` in the
    /// module constructor, `SpawnBehavior.cpp:57-91`): the spawner is targetable on its creation frame and
    /// `OBJECT_STATUS_MASKED` (`:1017`) hides it from `WeaponSet::getAttackResult` (`WeaponSet.cpp:483-496`,
    /// [`GameLogic::target_info`]) from the first heap frame on. The health-box offset, the selection broadcast
    /// (`:947-989`) and `m_selfTaskingSpawnCount` are client/AI bits the port does not model.
    fn spawn_compute_aggregate_states(&mut self, id: ObjectId) {
        let Some(t) = self.objects.get(&id).and_then(|o| o.template.spawn.clone()) else { return };
        if !t.aggregate_health {
            return;
        }
        let (spawned, parent_level) = match self.objects.get(&id) {
            Some(o) => (o.spawned.clone(), o.experience.level),
            None => return,
        };
        let mut count: i32 = 0;
        let mut acr_health: f32 = 0.0;
        let mut avg_health_max: f32 = 0.0;
        let mut levels: Vec<(ObjectId, crate::experience::VeterancyLevel)> = Vec::new();
        for s in &spawned {
            let Some(o) = self.objects.get(s) else { continue };
            let Some(b) = o.body.as_ref() else { continue };
            levels.push((*s, o.experience.level));
            acr_health += b.health();
            avg_health_max += b.max_health();
            count += 1;
        }
        // "whoever has more, set the other guy" (`:911-920`). `vetLevel` is a snapshot taken before the loop, so a
        // level the spawner gains here is not pushed back onto a later spawn in the same pass.
        for (sid, level) in levels {
            if level > parent_level {
                self.set_veterancy_level(id, level);
            } else if level < parent_level {
                self.set_veterancy_level(sid, parent_level);
            }
        }
        // "HEALTH STATE" (`:1001-1013`): `setInitialHealth(0)` with no spawns.
        let percent = if count > 0 {
            avg_health_max /= count as f32;
            let perfect_total = avg_health_max * t.spawn_number as f32;
            let actual = acr_health / perfect_total;
            (100.0 * actual) as i32
        } else {
            0
        };
        if let Some(o) = self.objects.get_mut(&id) {
            o.status_bits |= crate::object::status::MASKED;
            if let Some(b) = o.body.as_mut() {
                b.set_initial_health(percent);
            }
        }
    }

    /// `SpawnBehavior::update` (SpawnBehavior.cpp:192-265), run from the sleepy heap every frame: on the first update
    /// the replacement list is made (`:203-225`); then the 15-frame countdown (`SPAWN_UPDATE_RATE`, SpawnBehavior.h:33,
    /// `:230-235`); then each owed spawn whose frame has passed is created (`:244-259`). Returns `UPDATE_SLEEP_NONE`
    /// on every path (the constructor and `update` never sleep, SpawnBehavior.cpp:57-91, :264).
    pub(crate) fn spawn_update(&mut self, id: ObjectId, now: u32) -> u32 {
        // `if (m_aggregateHealth) computeAggregateStates();` (`SpawnBehavior.cpp:196-200`), every frame before the
        // countdown.
        self.spawn_compute_aggregate_states(id);
        let Some(t) = self.objects.get(&id).and_then(|o| o.template.spawn.clone()) else { return crate::module::UPDATE_SLEEP_NONE };
        let Some(o) = self.objects.get_mut(&id) else { return crate::module::UPDATE_SLEEP_NONE };
        if !o.spawn.times_inited {
            o.spawn.times_inited = true;
            // `runtimeProduced`: a factory made it, rather than a script or the map (`:207`).
            let runtime_produced = o.producer_id != ObjectId::INVALID;
            for list_index in 0..t.spawn_number.max(0) {
                // The burst entry is the flag itself (`:220`); the `birthFrame` of `:214-218` is never read.
                let entry = if t.initial_burst > 0 { runtime_produced as u32 } else { list_index as u32 };
                o.spawn.replacement_times.push(entry);
            }
            o.spawn.one_shot_countdown = if t.one_shot { t.spawn_number } else { -1 };
        }
        // `if (--m_framesToWait > 0) return UPDATE_SLEEP_NONE` (`:230-233`); `m_framesToWait` is an Int, so the first
        // update (0) drops to -1 and does not wait.
        o.spawn.frames_to_wait -= 1;
        if o.spawn.frames_to_wait > 0 {
            return crate::module::UPDATE_SLEEP_NONE;
        }
        o.spawn.frames_to_wait = 15;
        if self.spawn_should_try_to_spawn(id) {
            // Walk the list in order; an entry whose frame has passed (`currentTime > replacementTime`, `:249`) is
            // created and removed (`erase`), an entry that cannot be created (no free exit, `createSpawn` FALSE) stays.
            let mut i = 0;
            while let Some(due) = self.objects.get(&id).and_then(|o| o.spawn.replacement_times.get(i).copied()) {
                if now > due && self.spawn_create_spawn(id) {
                    if let Some(o) = self.objects.get_mut(&id) {
                        o.spawn.replacement_times.remove(i);
                    }
                } else {
                    i += 1;
                }
            }
            // `if (m_isOneShotData && m_oneShotCountdown <= 0) stopSpawning()` (`:261-262`).
            if let Some(o) = self.objects.get_mut(&id) {
                if t.one_shot && o.spawn.one_shot_countdown <= 0 {
                    o.spawn.stopped = true;
                }
            }
        }
        crate::module::UPDATE_SLEEP_NONE
    }

    /// `SpawnBehavior::shouldTryToSpawn` (SpawnBehavior.cpp:825-846): not while stopped, not for a one-shot hole that
    /// is being rebuilt (which then stops for good), not under construction or being sold, not civilian.
    fn spawn_should_try_to_spawn(&mut self, id: ObjectId) -> bool {
        let neutral = self.neutral_player();
        let Some(o) = self.objects.get_mut(&id) else { return false };
        if o.spawn.stopped {
            return false;
        }
        let one_shot = o.template.spawn.as_ref().is_some_and(|t| t.one_shot);
        if o.status_bits & crate::object::status::RECONSTRUCTING != 0 && one_shot {
            o.spawn.stopped = true;
            return false;
        }
        if o.under_construction || o.status_bits & crate::object::status::SOLD != 0 {
            return false;
        }
        neutral != Some(o.player)
    }

    /// `FireWeaponUpdate::update`: fire the module weapon on its own reload.
    fn update_fire_weapon(&mut self, id: ObjectId, now: u32) {
        let Some(t) = self.objects.get(&id).and_then(|o| o.template.fire_weapon.clone()) else { return };
        if self.objects.get(&id).is_some_and(|o| o.under_construction) {
            return;
        }
        let next = match self.objects.get(&id) {
            Some(o) if !o.is_dying() => o.behavior.fire_next,
            _ => return,
        };
        let due = if next == 0 { now + t.initial_delay } else { next };
        if now < due {
            if let Some(o) = self.objects.get_mut(&id) {
                o.behavior.fire_next = due;
            }
            return;
        }
        self.fire_temp_weapon(id, t.weapon.clone());
        let delay = t.exclusive_weapon_delay.max(1);
        if let Some(o) = self.objects.get_mut(&id) {
            o.behavior.fire_next = now + delay;
        }
    }

    /// `SpawnBehavior::createSpawn` (SpawnBehavior.cpp:596-750): make the next template's unit and exit it. Returns
    /// FALSE when the exit is busy, and the entry stays owed (`:610`). A spawner whose exit is a
    /// `SupplyCenterProductionExitUpdate` or a `SpawnPointProductionExitUpdate` reserves that exit; a tunnel leaves
    /// through its `OpenContain` exit. Any other spawner is placed on a 12-unit ring (a stand-in for its exit door,
    /// which the port does not model).
    pub(crate) fn spawn_create_spawn(&mut self, id: ObjectId) -> bool {
        let Some(t) = self.objects.get(&id).and_then(|o| o.template.spawn.clone()) else { return false };
        let n = t.spawn_template_names.len();
        if n == 0 {
            return false;
        }
        let index = self.objects.get(&id).map_or(0, |o| o.spawn.template_index) % n;
        let Some(tt) = self.template_source.as_ref().and_then(|s| s.find_template(&t.spawn_template_names[index])) else {
            return false;
        };
        let Some(player) = self.objects.get(&id).map(|o| o.player) else { return false };
        let source = self.objects.get(&id).map(|o| o.position).unwrap_or(crate::math::Coord3D::new(0.0, 0.0, 0.0));
        // `SpawnBehavior.cpp:629`: `TheThingFactory->newObject(m_spawnTemplate, parent->getTeam())`.
        let spawn_team = self.objects.get(&id).and_then(|o| o.team.clone());
        // `SpawnBehavior::createSpawn` leaves through the spawner's exit (`reserveDoorForExit` then
        // `exitObjectViaDoor`) when it has a supply center's exit or a spawn point exit. The port reads the exit from
        // `template.production`: a spawner whose template has no `ProductionUpdate` (the campaign/demo stinger-site
        // variants, and the Angry Mob nexus's queue exit) is ring-placed instead. See `l173_spawn_exits` (R19
        // suspicion 1): the list is campaign-only, and routing those through the real exit needs the exit module's own
        // occupancy state, which the port keeps in `ProductionState`.
        let via_supply_exit = self.objects.get(&id).is_some_and(|o| {
            matches!(
                o.template.production.as_ref().map(|p| &p.exit),
                Some(crate::production::ProductionExit::SupplyCenter { .. } | crate::production::ProductionExit::SpawnPoint { .. })
            )
        });
        if via_supply_exit {
            let sid = self.spawn_produced_unit(id, &tt);
            if sid == ObjectId::INVALID {
                // `reserveDoorForExit` returned DOOR_NONE_AVAILABLE: `createSpawn` returns FALSE (`:609-610`).
                return false;
            }
            if let Some(o) = self.objects.get_mut(&sid) {
                // `newSpawn->setProducer(parent)` (`SpawnBehavior.cpp:665`). The master is set by the enslave hook
                // below, before the exit (`:657-666`).
                o.producer_id = id;
            }
            if let Some(t) = &spawn_team {
                self.set_object_team_key(sid, t);
            }
            // `m_spawnIDs.push_back(newSpawn->getID())` (`SpawnBehavior.cpp:668`).
            if let Some(o) = self.objects.get_mut(&id) {
                o.spawned.push(sid);
            }
        } else if self.has_open_contain_exit(id) {
            // A tunnel network leaves through its `OpenContain` exit: the unit is created at the `ExitStart` bone
            // and walks the exit path (`SpawnBehavior.cpp:727-728`, `OpenContain::exitObjectViaDoor`).
            let sid = self.spawn_through_open_contain_exit(id, &tt);
            if let Some(t) = &spawn_team {
                self.set_object_team_key(sid, t);
            }
            // `m_spawnIDs.push_back(newSpawn->getID())` (`SpawnBehavior.cpp:668`).
            if let Some(o) = self.objects.get_mut(&id) {
                o.spawned.push(sid);
            }
        } else {
            // Ring stand-in: the batch index is the number of live spawns, which is the old first-batch index.
            let i = self.objects.get(&id).map_or(0, |o| o.spawned.len());
            let angle = i as f32 * 0.7;
            // The sim's `Cos`/`Sin` (`crate::math`), never std's platform libm (AGENTS.md "Numbers").
            let pos = crate::math::Coord3D::new(
                source.x + crate::math::cos(angle) * 12.0,
                source.y + crate::math::sin(angle) * 12.0,
                source.z,
            );
            let sid = self.create_object(&tt, pos, 0.0, player);
            if let Some(o) = self.objects.get_mut(&sid) {
                // `newSpawn->setProducer(parent)` (`SpawnBehavior.cpp:665`).
                o.producer_id = id;
            }
            // `SpawnBehavior::createSpawn`'s `onEnslave` (SpawnBehavior.cpp:657-666), before the exit.
            self.slaved_on_enslave(sid, id);
            // `m_spawnIDs.push_back(newSpawn->getID())` (`SpawnBehavior.cpp:668`).
            if let Some(o) = self.objects.get_mut(&id) {
                o.spawned.push(sid);
            }
            if let Some(t) = &spawn_team {
                self.set_object_team_key(sid, t);
            }
            // `SpawnBehavior::createSpawn` (SpawnBehavior.cpp:635): `onUnitCreated(parent, newSpawn)`.
            self.score_unit_created(sid);
        }
        // `++m_templateNameIterator` with the wrap (`:647-651`), and `m_oneShotCountdown--` (`:735-736`).
        if let Some(o) = self.objects.get_mut(&id) {
            o.spawn.template_index = (index + 1) % n;
            if t.one_shot {
                o.spawn.one_shot_countdown -= 1;
            }
        }
        true
    }

    /// `Object::onDie`'s notification to the spawner (Object.cpp:4657-4665): when the dying object was produced by a
    /// spawner (its producer, `Object::getProducerID`), that spawner's `onSpawnDeath` runs. `source` is the damage's
    /// attacker (`damageInfo->in.m_sourceID`), the killer that is scored when the spawner dies with its last spawn.
    pub(crate) fn spawn_notify_spawner_of_death(&mut self, dead: ObjectId, source: ObjectId) {
        let Some(spawner) = self.objects.get(&dead).map(|o| o.producer_id).filter(|&p| p != ObjectId::INVALID) else { return };
        if self.objects.get(&spawner).is_some_and(|o| o.template.spawn.is_some()) {
            self.spawn_on_spawn_death(spawner, dead, source);
        }
    }

    /// `SpawnBehavior::onSpawnDeath` (SpawnBehavior.cpp:753-782): a spawn that is not in the list is ignored; the
    /// spawn leaves `m_spawnIDs`, and a replacement is owed `SpawnReplaceDelay` frames from now (`:765-766`). With
    /// `AggregateHealth`, a spawner with no spawns left is scored to its killer and destroyed (`:772-781`).
    fn spawn_on_spawn_death(&mut self, spawner: ObjectId, dead: ObjectId, killer: ObjectId) {
        let Some(t) = self.objects.get(&spawner).and_then(|o| o.template.spawn.clone()) else { return };
        let now = self.frame;
        let Some(o) = self.objects.get_mut(&spawner) else { return };
        let Some(index) = o.spawned.iter().position(|&s| s == dead) else { return };
        o.spawned.remove(index);
        o.spawn.replacement_times.push(now.saturating_add(t.spawn_replace_delay));
        let none_left = o.spawned.is_empty();
        if none_left && t.aggregate_health {
            // `killer->scoreTheKill(getObject())` (`:774-776`); `score_the_kill` ignores a missing killer.
            self.score_the_kill(spawner, killer);
            self.destroy_object(spawner);
        }
    }

    /// `SpawnBehavior::createSpawn`'s and `ObjectCreationList`'s `sdu->onEnslave(source)` (SpawnBehavior.cpp:657-666,
    /// ObjectCreationList.cpp:1000-1005) -> `SlavedUpdate::onEnslave` -> `startSlavedEffects` (SlavedUpdate.cpp:88-91,
    /// :703-716): the slave's master is set (`m_slaver = slaver->getID()`, `:706`), the pinned stray point is drawn
    /// — `GameLogicRandomValue(0, 2*PI)` (`:712`, an *integer* draw, so 0..6) — and `m_guardPointOffset` is
    /// `GuardMaxRange * (Cos, Sin)` of that direction (`:712-715`). The module data is the *slave's* own.
    /// Only an object with a `SlavedUpdate` has a master (the engine's loop breaks on the first module that
    /// answers).
    pub(crate) fn slaved_on_enslave(&mut self, slave: ObjectId, master: ObjectId) {
        let Some(st) = self.objects.get(&slave).and_then(|o| o.template.slaved.clone()) else { return };
        if let Some(o) = self.objects.get_mut(&slave) {
            o.master = Some(master);
        }
        let dir = self.rng.value(0, 6) as f32;
        let range = st.guard_max_range as f32;
        if let Some(o) = self.objects.get_mut(&slave) {
            o.behavior.slaved_guard_offset = crate::math::Coord3D::new(range * cos(dir), range * sin(dir), 0.0);
        }
    }

    /// `SlavedUpdate::update` (SlavedUpdate.cpp:110-280), run from the sleepy heap every frame; returns
    /// `UPDATE_SLEEP_NONE` on every path. The module's countdown (`m_framesToWait`, :114-127) runs the body once every
    /// `SLAVED_UPDATE_RATE` frames (SlavedUpdate.h:35). The body is the port's leash only (the attack, scout and
    /// repair logic is not ported; the repair state those priorities drive is never set, so `endRepair` in the leash
    /// is a no-op).
    pub(crate) fn slaved_update(&mut self, id: ObjectId, _now: u32) -> u32 {
        let Some(o) = self.objects.get_mut(&id) else { return crate::module::UPDATE_SLEEP_NONE };
        if o.behavior.slaved_frames_to_wait > 0 {
            o.behavior.slaved_frames_to_wait -= 1;
        }
        if o.behavior.slaved_frames_to_wait > 0 {
            return crate::module::UPDATE_SLEEP_NONE;
        }
        o.behavior.slaved_frames_to_wait = SLAVED_UPDATE_RATE;
        // `if (m_slaver == INVALID_ID) return UPDATE_SLEEP_NONE;` (:129-130).
        let master = o.master;
        if let Some(master) = master {
            self.slaved_leash(id, master);
        }
        crate::module::UPDATE_SLEEP_NONE
    }

    /// The guard block of `SlavedUpdate::update` (`SlavedUpdate.cpp:255-278`): the slave stays near the
    /// pinned point `masterPos + m_guardPointOffset`. If it is idle and more than `CLOSE_ENOUGH` (15 units)
    /// from that point — or more than `STRAY_MULTIPLIER * GuardMaxRange` from its master, whatever it is
    /// doing — it is sent there with `aiMoveToPosition(CMD_FROM_AI)`.
    pub(crate) fn slaved_leash(&mut self, slave: ObjectId, master: ObjectId) {
        // `if (!master || master->isEffectivelyDead() || master->isDisabledByType(DISABLED_UNMANNED))`
        // (`:143-158`): `stopSlavedEffects` clears the slaver and the guard point. The `UNMANNED` half is not
        // modelled on the master here.
        if self.objects.get(&master).is_none_or(|o| o.is_dying() || o.destroyed) {
            if let Some(o) = self.objects.get_mut(&slave) {
                o.master = None;
                o.behavior.slaved_guard_offset = crate::math::Coord3D::ZERO;
            }
            return;
        }
        let (Some(s), Some(m)) = (self.objects.get(&slave), self.objects.get(&master)) else { return };
        let Some(st) = s.template.slaved.clone() else { return };
        let (sp, mp, offset, idle) = (
            s.position,
            m.position,
            s.behavior.slaved_guard_offset,
            s.ai.machine.current == crate::ai_states::AiStateType::Idle,
        );
        // `pinnedPosition = *master->getPosition(); x += offset.x; y += offset.y;` keeps the master's z;
        // only `m_guardPointOffset.z` takes the ground height (`:255-259`).
        let pinned = crate::math::Coord3D::new(mp.x + offset.x, mp.y + offset.y, mp.z);
        let ground = self.terrain.ground_height(pinned.x, pinned.y);
        if let Some(o) = self.objects.get_mut(&slave) {
            o.behavior.slaved_guard_offset.z = ground;
        }
        let sqr_3d = |a: &crate::math::Coord3D, b: &crate::math::Coord3D| {
            let (dx, dy, dz) = (a.x - b.x, a.y - b.y, a.z - b.z);
            dx * dx + dy * dy + dz * dz
        };
        // `const Real CLOSE_ENOUGH = 15; const Real CLOSE_ENOUGH_SQR = (CLOSE_ENOUGH * CLOSE_ENOUGH);`
        // (`:54-55`), `STRAY_MULTIPLIER` 2.0 (`:54`).
        const CLOSE_ENOUGH_SQR: f32 = 15.0 * 15.0;
        let guard = if st.guard_max_range != 0 {
            if idle && sqr_3d(&sp, &pinned) > CLOSE_ENOUGH_SQR {
                true
            } else {
                let stray = 2.0 * st.guard_max_range as f32;
                sqr_3d(&sp, &mp) > stray * stray
            }
        } else {
            false
        };
        if guard {
            // `endRepair()` (`:519-545`) resets the locomotor set; no repair state exists here, so only the
            // guard order follows (`doGuardLogic`, `:406-427`).
            self.slaved_do_guard_logic(slave, pinned, &st);
        }
    }

    /// `SlavedUpdate::doGuardLogic` (`SlavedUpdate.cpp:406-427`): with `GuardWanderRange`, re-roll the offset
    /// (`GameLogicRandomValue(0, 2*PI)`, another integer draw, and `GuardMaxRange * (Cos, Sin)` — the range,
    /// not the wander range) and move the pinned point; then `ai->aiMoveToPosition(pinned, CMD_FROM_AI)`.
    fn slaved_do_guard_logic(&mut self, slave: ObjectId, mut pinned: crate::math::Coord3D, st: &SlavedTemplate) {
        if st.guard_wander_range != 0 {
            let dir = self.rng.value(0, 6) as f32;
            let range = st.guard_max_range as f32;
            let offset = crate::math::Coord3D::new(range * cos(dir), range * sin(dir), 0.0);
            pinned.x += offset.x;
            pinned.y += offset.y;
            let z = self.terrain.ground_height(pinned.x, pinned.y);
            if let Some(o) = self.objects.get_mut(&slave) {
                o.behavior.slaved_guard_offset = crate::math::Coord3D::new(offset.x, offset.y, z);
            }
        }
        self.ai_move_to_position_cmd(slave, pinned, crate::ai_states::CommandSource::Ai);
    }

    /// `CreateCrateDie` + `EjectPilotDie`: the die-module spawns.
    pub(crate) fn die_spawns(&mut self, id: ObjectId, _death: DeathType) {
        // `EjectPilotDie`: run the air/ground OCL.
        if let Some(t) = self.objects.get(&id).and_then(|o| o.template.eject_pilot.clone()) {
            let airborne = self.objects.get(&id).is_some_and(|o| {
                o.position.z - self.terrain.ground_height(o.position.x, o.position.y) > 9.0
            });
            if let Some(ocl) = if airborne { t.air_ocl } else { t.ground_ocl } {
                self.run_ocl(&ocl, id);
            }
        }
        // `CreateCrateDie`: spawn the listed crate templates.
        if let Some(names) = self.objects.get(&id).and_then(|o| o.template.create_crate.clone()) {
            let (pos, player, killer) = match self.objects.get(&id) {
                Some(o) => (o.position, o.player, o.last_damage_info.source),
                None => return,
            };
            for name in names {
                if let Some(tt) = self.template_source.as_ref().and_then(|s| s.find_template(&name)) {
                    let crate_id = self.create_object(&tt, pos, 0.0, player);
                    // `CreateCrateDie::onDie` (CreateCrateDie.cpp:109): a computer-controlled killer
                    // is told about the crate (`AIUpdateInterface::notifyCrate`).
                    let ai_killer = self.objects.get(&killer).is_some_and(|k| self.is_ai_player(k.player));
                    if ai_killer {
                        if let Some(k) = self.objects.get_mut(&killer) {
                            k.ai.crate_created = crate_id;
                        }
                    }
                }
            }
        }
    }

    /// `VeterancyGainCreate`: set the minimum veterancy level on creation.
    pub(crate) fn apply_veterancy_gain(&mut self, id: ObjectId) {
        let Some(t) = self.objects.get(&id).and_then(|o| o.template.veterancy_gain.clone()) else { return };
        let has_science = match (&t.science_required, self.objects.get(&id).map(|o| o.player)) {
            (None, _) => true,
            (Some(s), Some(p)) => self.player_has_science(p, s),
            _ => false,
        };
        if has_science {
            self.set_veterancy_level(id, crate::experience::VeterancyLevel::from_index(t.starting_level as usize));
        }
    }

    /// `VeterancyCrateCollide`: grant one veterancy level (capped at HEROIC).
    pub(crate) fn add_veterancy(&mut self, id: ObjectId) {
        let Some(level) = self.objects.get(&id).map(|o| o.experience.level.index() + 1) else { return };
        if level < crate::experience::VeterancyLevel::COUNT {
            self.set_veterancy_level(id, crate::experience::VeterancyLevel::from_index(level));
        }
    }

    /// The periodic pass for the round-2 modules still driven by a sweep. `AutoDepositUpdate`,
    /// `BaseRegenerateUpdate` and `SpawnBehavior` are sleepy families now (`sleepy_families.rs`).
    pub(crate) fn update_modules2(&mut self) {
        let ids: Vec<ObjectId> = self.objects.values().filter(|o| !o.destroyed).map(|o| o.id).collect();
        for id in ids {
            self.update_fire_weapon(id, self.frame);
        }
    }

    /// `ToppleUpdate::update` (ToppleUpdate.cpp): the kill after the fall. Returns `UPDATE_SLEEP_NONE` while the
    /// object is still falling and `UPDATE_SLEEP_FOREVER` once it is down. The fall is the port's fixed timer, not the
    /// C++ angular accumulation (the transform and the bounce data are not ported).
    pub(crate) fn topple_update(&mut self, id: ObjectId, now: u32) -> u32 {
        let (toppled, frame) = match self.objects.get(&id) {
            Some(o) => (o.behavior.toppled, o.behavior.topple_kill_frame),
            None => return UPDATE_SLEEP_FOREVER,
        };
        if toppled && frame != 0 && now >= frame {
            if self.objects.get(&id).and_then(|o| o.template.topple.clone()).is_some_and(|t| t.kill_when_finished_toppling) {
                self.damage_object_with_death(
                    id,
                    DamageInfo { source: ObjectId::INVALID, amount: HUGE_DAMAGE_AMOUNT, kind: DamageType::Unresistable },
                    DeathType::Toppled,
                );
            }
            if let Some(o) = self.objects.get_mut(&id) {
                o.behavior.topple_kill_frame = 0;
            }
        }
        // Falling: the update returns NONE (ToppleUpdate.cpp:...); down: FOREVER (`TOPPLE_DOWN`).
        if self.objects.get(&id).is_some_and(|o| o.behavior.toppled && o.behavior.topple_kill_frame != 0) {
            UPDATE_SLEEP_NONE
        } else {
            UPDATE_SLEEP_FOREVER
        }
    }
}

/// Lane 179 (R21-20): the slaved leash's pinned point keeps the master's z (SlavedUpdate.cpp:255-262).
#[cfg(test)]
#[path = "modules2_l179.rs"]
mod l179_tests;
