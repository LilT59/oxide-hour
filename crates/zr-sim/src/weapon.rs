//! Weapons: `WeaponTemplate`, the per-object `Weapon` state machine and `WeaponSet`
//! (GameLogic/Object/Weapon.cpp, WeaponSet.cpp).
//!
//! `zr-sim` does not depend on the INI crate: the adapter converts `zr-templates` values (already
//! in frames / per-frame speeds / radians) into [`WeaponTemplate`] and [`CombatTemplate`].
//!
//! Ported: the weapon status machine (`getStatus`, `reloadWithBonus`, `privateFireWeapon`'s state
//! changes, `preFireWeapon`/`getPreAttackDelay`), range helpers with `RATIONALIZE_ATTACK_RANGE`, the
//! weapon bonus fields (`Weapon::computeBonus` is `zr-sim::bonus`, the condition flags come from
//! `upgrade.rs` `weapon_bonus_flags`), ammo clips and the two weapon locks.
//! Stubbed: lasers, historic bonus, scatter target tables, pitch limits, shared reload between slots,
//! and the `WeaponLaunchBone` *drawable* (the launcher position comes from the template's resolved bones,
//! see `combat.rs` `launch_position`). (The continuous-fire and `AutoReloadWhenIdle` numbers are carried
//! here; their state is the `FiringTracker`, `object_helpers.rs`.)

use crate::armor::ArmorTemplate;
use crate::crc::Crc;
use crate::damage::{DamageType, DeathType};
use crate::projectile::ProjectileTemplate;
use crate::random::GameRandom;
use std::sync::Arc;

pub const WEAPON_SLOT_COUNT: usize = 3;
/// `PATHFIND_CELL_SIZE_F`: `RATIONALIZE_ATTACK_RANGE` undersizes ranges by a quarter cell.
pub const PATHFIND_CELL_SIZE_F: f32 = 10.0;
// `WeaponSetType` condition bits (`WeaponSetType.h`; names in `WeaponSet.cpp` `s_bitNameList`).
// Each bit is `1 << index`, where `index` is the `zr_templates::WeaponSetType` index, so these
// agree with the template's `Mask<WeaponSetType>`. `zr-game/tests/set_flag_bits.rs` checks every
// constant against that enum, so the two cannot drift. Retail never rebuilds the flag set from
// scratch: each setter touches only its own bit (`Object::setWeaponSetFlag`, `Object.cpp:3101`).
pub const WEAPONSET_VETERAN: u32 = 1 << 0;
pub const WEAPONSET_ELITE: u32 = 1 << 1;
pub const WEAPONSET_HERO: u32 = 1 << 2;
/// Set by `WeaponSetUpgrade::upgradeImplementation` (`WeaponSetUpgrade.cpp:56`).
pub const WEAPONSET_PLAYER_UPGRADE: u32 = 1 << 3;
/// Salvage crate: `SalvageCrateCollide::doWeaponSet` (`SalvageCrateCollide.cpp:198`).
pub const WEAPONSET_CRATEUPGRADE_ONE: u32 = 1 << 4;
pub const WEAPONSET_CRATEUPGRADE_TWO: u32 = 1 << 5;
pub const WEAPONSET_VEHICLE_HIJACK: u32 = 1 << 6;
/// The dead-man's-trigger weapon set (`Object.cpp:2219`).
pub const WEAPONSET_CARBOMB: u32 = 1 << 7;
/// Set by `MSG_SET_MINE_CLEARING_DETAIL` (`AIGroup::setMineClearingDetail`, `AIGroup.cpp:3059`) and by
/// the dozer / worker while it clears mines.
pub const WEAPONSET_MINE_CLEARING_DETAIL: u32 = 1 << 8;
/// Combat-bike riders (`RiderChangeContain`, `RiderChangeContain.cpp:221`), indices 9..=16.
pub const WEAPONSET_RIDER1: u32 = 1 << 9;
pub const WEAPONSET_RIDER2: u32 = 1 << 10;
pub const WEAPONSET_RIDER3: u32 = 1 << 11;
pub const WEAPONSET_RIDER4: u32 = 1 << 12;
pub const WEAPONSET_RIDER5: u32 = 1 << 13;
pub const WEAPONSET_RIDER6: u32 = 1 << 14;
pub const WEAPONSET_RIDER7: u32 = 1 << 15;
pub const WEAPONSET_RIDER8: u32 = 1 << 16;

/// `WeaponAffectsMaskType` bits.
pub mod affects {
    pub const SELF: u8 = 1 << 0;
    pub const ALLIES: u8 = 1 << 1;
    pub const ENEMIES: u8 = 1 << 2;
    pub const NEUTRALS: u8 = 1 << 3;
    /// `WEAPON_KILLS_SELF` (INI `SUICIDE`).
    pub const KILLS_SELF: u8 = 1 << 4;
    pub const DOESNT_AFFECT_SIMILAR: u8 = 1 << 5;
    pub const DOESNT_AFFECT_AIRBORNE: u8 = 1 << 6;
    pub const DEFAULT: u8 = ALLIES | ENEMIES | NEUTRALS;
}

/// `WeaponAntiMaskType` bits (only the ones the sim distinguishes).
/// `WeaponAntiFlags` (`GameLogic/Weapon.h:100-107`): the victim classes a weapon may target
/// (`getVictimAntiMask`, `WeaponSet.cpp:389-433`). The eight values are distinct bits of the `u8`.
pub mod anti {
    pub const AIRBORNE_VEHICLE: u8 = 0x01;
    pub const GROUND: u8 = 0x02;
    pub const PROJECTILE: u8 = 0x04;
    pub const SMALL_MISSILE: u8 = 0x08;
    pub const MINE: u8 = 0x10;
    pub const AIRBORNE_INFANTRY: u8 = 0x20;
    pub const BALLISTIC_MISSILE: u8 = 0x40;
    pub const PARACHUTE: u8 = 0x80;
}

/// `WeaponPrefireType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prefire {
    PerShot,
    PerAttack,
    PerClip,
}

/// What the sim needs from a `WeaponTemplate`. Times are frames, speeds per frame, angles radians.
#[derive(Clone, Debug)]
pub struct WeaponTemplate {
    pub name: String,
    pub primary_damage: f32,
    pub primary_damage_radius: f32,
    pub secondary_damage: f32,
    pub secondary_damage_radius: f32,
    /// `AttackRange` (before the `RATIONALIZE_ATTACK_RANGE` undersizing; see [`attack_range`](Self::attack_range)).
    pub attack_range: f32,
    pub minimum_attack_range: f32,
    pub damage_type: DamageType,
    pub death_type: DeathType,
    pub min_delay_between_shots: u32,
    pub max_delay_between_shots: u32,
    /// `ClipSize`; 0 means unlimited.
    pub clip_size: i32,
    pub clip_reload_time: u32,
    /// `WeaponTemplate::ShotsPerBarrel`: how many shots use one barrel before cycling. 1 by
    /// default. Not used by the simulation's firing state; the client uses it for barrel cycling.
    pub shots_per_barrel: i32,
    pub pre_attack_delay: u32,
    pub prefire: Prefire,
    /// Distance per frame. The default (999999) makes damage instantaneous.
    pub weapon_speed: f32,
    /// A `ProjectileObject` is set: damage is dealt when the projectile detonates.
    pub has_projectile: bool,
    /// The behavior of the weapon's `ProjectileObject` template, when the adapter resolved one.
    pub projectile: Option<Arc<ProjectileTemplate>>,
    pub scatter_radius: f32,
    /// `ScatterRadiusVsInfantry`.
    pub scatter_radius_vs_infantry: f32,
    pub damage_dealt_at_self_position: bool,
    pub auto_reloads_clip: bool,
    /// `WeaponTemplate::m_reloadType` (`AutoReloadsClip`): `auto_reloads_clip` is `AUTO_RELOAD`;
    /// `RETURN_TO_BASE_TO_RELOAD` weapons send a jet home to rearm (`JetAIUpdate::isOutOfSpecialReloadAmmo`).
    pub returns_to_base_to_reload: bool,
    /// `AcceptableAimDelta`.
    pub acceptable_aim_delta: f32,
    /// [`affects`] bits.
    pub affects: u8,
    /// [`anti`] bits.
    pub anti_mask: u8,
    /// `FireFX` (REGULAR veterancy): the FXList played at the muzzle on firing.
    pub fire_fx: Option<String>,
    /// `ProjectileDetonationFX` (REGULAR): the FXList played where the projectile detonates.
    pub projectile_detonation_fx: Option<String>,
    /// `ProjectileExhaust` / `VeterancyProjectileExhaust` per veterancy level (`Weapon::m_projectileExhausts`):
    /// the particle system a `MissileAIUpdate` projectile attaches at ignition, picked by the firer's level at fire
    /// time (`Weapon::fireWeaponTemplate`). Client data only: the sim never reads it back.
    pub projectile_exhaust: [Option<String>; 4],
    /// `ProjectileObject`: the projectile object template name (for rendering its model).
    pub projectile_name: String,
    /// `ProjectileStreamName`: the stream object a projectile of this weapon joins (`projectile_stream.rs`).
    pub projectile_stream_name: String,
    /// `RequestAssistRange` (Weapon.cpp:175): a shot at an object asks equivalent units within this range to
    /// assist (`Weapon::processRequestAssistance`, Weapon.cpp:3018).
    pub request_assist_range: f32,
    /// `LaserName` / `LaserBoneName` and the laser template's lifetime modules (`laser_beam.rs`).
    pub laser: Option<crate::laser_beam::LaserSpec>,
    /// `MissileCallsOnDie` (`m_dieOnDetonate`): when the projectile detonates, it damages itself
    /// for `DAMAGE_UNRESISTABLE`/`DEATH_DETONATED`, running its own die modules.
    pub die_on_detonate: bool,
    /// The projectile object's `SpecialPowerCompletionDie` power, if any: on detonation the
    /// firing source's special power is reported complete (`Weapon.cpp`).
    pub special_power_completion: Option<String>,
    /// `WeaponBonus = CONDITION FIELD percent` entries (`Weapon::computeBonus`).
    pub weapon_bonus: crate::bonus::WeaponBonusSet,
    /// `ContinuousFireOne` / `ContinuousFireTwo`: shots at one victim before the MEAN and FAST continuous-fire
    /// levels (`FiringTracker::shotFired`, FiringTracker.cpp:136-158).
    pub continuous_fire_one_shots_needed: i32,
    pub continuous_fire_two_shots_needed: i32,
    /// `ContinuousFireCoast` (frames): how long the shot count survives without a shot.
    pub continuous_fire_coast_frames: u32,
    /// `AutoReloadWhenIdle` (frames): the clip reloads this long after the last shot (`FiringTracker`).
    pub auto_reload_when_idle_frames: u32,
    /// `FireSoundLoopTime` (frames): the fire sound loops for this long (`FiringTracker::shotFired`).
    pub fire_sound_loop_time: u32,
    /// `DamageStatusType` as an object-status mask (`1 << ObjectStatusTypes`, `0` for `OBJECT_STATUS_NONE`): the status
    /// a `DAMAGE_STATUS` shot sets for `primary damage` milliseconds (`ActiveBody.cpp:483-490`).
    pub damage_status_mask: u64,
    /// `WeaponTemplate::m_allowAttackGarrisonedBldgs` (`AllowAttackGarrisonedBldgs`, `Weapon.cpp:235`): the weapon's
    /// damage estimate against a garrisoned building is 1.0 even when its normal damage would be zero
    /// (`Weapon.cpp:632-641`), so the AI will pick it for the clearing attack.
    pub allow_attack_garrisoned_bldgs: bool,
}

impl WeaponTemplate {
    /// `WeaponTemplate::WeaponTemplate()` defaults.
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            primary_damage: 0.0,
            primary_damage_radius: 0.0,
            secondary_damage: 0.0,
            secondary_damage_radius: 0.0,
            attack_range: 0.0,
            minimum_attack_range: 0.0,
            damage_type: DamageType::Explosion,
            death_type: DeathType::Normal,
            min_delay_between_shots: 0,
            max_delay_between_shots: 0,
            clip_size: 0,
            clip_reload_time: 0,
            shots_per_barrel: 1,
            pre_attack_delay: 0,
            prefire: Prefire::PerShot,
            weapon_speed: 999_999.0,
            has_projectile: false,
            projectile: None,
            scatter_radius: 0.0,
            scatter_radius_vs_infantry: 0.0,
            damage_dealt_at_self_position: false,
            auto_reloads_clip: true,
            returns_to_base_to_reload: false,
            acceptable_aim_delta: 0.0,
            affects: affects::DEFAULT,
            anti_mask: anti::GROUND,
            fire_fx: None,
            projectile_detonation_fx: None,
            projectile_exhaust: Default::default(),
            projectile_name: String::new(),
            projectile_stream_name: String::new(),
            request_assist_range: 0.0,
            laser: None,
            die_on_detonate: false,
            special_power_completion: None,
            weapon_bonus: crate::bonus::WeaponBonusSet::default(),
            continuous_fire_one_shots_needed: i32::MAX,
            continuous_fire_two_shots_needed: i32::MAX,
            continuous_fire_coast_frames: 0,
            auto_reload_when_idle_frames: 0,
            fire_sound_loop_time: 0,
            damage_status_mask: 0,
            allow_attack_garrisoned_bldgs: false,
        }
    }

    /// `WeaponTemplate::getAttackRange` with `RATIONALIZE_ATTACK_RANGE` (bonus = 1).
    pub fn attack_range(&self) -> f32 {
        let r = self.attack_range - PATHFIND_CELL_SIZE_F * 0.25;
        if r < 0.0 {
            0.0
        } else {
            r
        }
    }

    /// `WeaponTemplate::getMinimumAttackRange`.
    pub fn min_attack_range(&self) -> f32 {
        let r = self.minimum_attack_range - PATHFIND_CELL_SIZE_F * 0.25;
        if r < 0.0 {
            0.0
        } else {
            r
        }
    }

    /// `WeaponTemplate::isContactWeapon` (`Weapon.cpp:557-569`, `RATIONALIZE_ATTACK_RANGE` is defined at
    /// `Weapon.cpp:80`): a weapon whose range is under one pathfind cell. Its `FireFX` plays at the shot's target
    /// rather than at the firer when the drawable has no `WeaponFireFXBone` (`Weapon.cpp:961-965`).
    pub fn is_contact_weapon(&self) -> bool {
        self.attack_range - PATHFIND_CELL_SIZE_F * 0.25 < PATHFIND_CELL_SIZE_F
    }

    /// `WeaponTemplate::getDelayBetweenShots` (bonus = 1): the only RNG use in the weapon state.
    pub fn delay_between_shots(&self, rng: &mut GameRandom) -> u32 {
        self.delay_between_shots_with(rng, 1.0)
    }

    /// `WeaponTemplate::getDelayBetweenShots`: divide by the RATE_OF_FIRE bonus.
    pub fn delay_between_shots_with(&self, rng: &mut GameRandom, rate_of_fire: f32) -> u32 {
        let base = if self.min_delay_between_shots == self.max_delay_between_shots {
            self.min_delay_between_shots
        } else {
            rng.value(self.min_delay_between_shots as i32, self.max_delay_between_shots as i32) as u32
        };
        let rof = if rate_of_fire > 0.0 { rate_of_fire } else { 1.0 };
        crate::math::floor(base as f32 / rof) as u32
    }

    /// `WeaponTemplate::estimateWeaponTemplateDamage`'s garrison branches (`Weapon.cpp:632-641`), then
    /// `Object::estimateDamage` -> `ActiveBody::estimateDamage` (`ActiveBody.cpp:283-302`): a `DAMAGE_SURRENDER` or
    /// `AllowAttackGarrisonedBldgs` weapon can target a garrisoned building for the nonzero estimate 1.0; a
    /// `DAMAGE_KILL_GARRISONED` weapon estimates 1.0 only for a garrison it can actually clear
    /// (`isGarrisonable() && !isImmuneToClearBuildingAttacks()`, checked by the caller and passed as
    /// `clearable_garrison`). Everything else is armour-adjusted primary damage.
    pub fn estimate_damage(&self, armor: Option<&ArmorTemplate>, clearable_garrison: bool) -> f32 {
        if (self.damage_type == DamageType::Surrender || self.allow_attack_garrisoned_bldgs) && clearable_garrison {
            return 1.0;
        }
        if self.damage_type == DamageType::KillGarrisoned {
            return if clearable_garrison { 1.0 } else { 0.0 };
        }
        match armor {
            Some(a) => a.adjust_damage(self.damage_type, self.primary_damage),
            None => self.primary_damage,
        }
    }
}

/// `WeaponTemplateSet`: the weapons for the object's current weapon-set conditions.
#[derive(Clone, Debug, Default)]
pub struct WeaponSetTemplate {
    pub weapons: [Option<Arc<WeaponTemplate>>; WEAPON_SLOT_COUNT],
}

/// One `WeaponLaunchBone` barrel at rest: the bone's world translation and rotation in the model's bind pose
/// (`W3DModelDraw::validateWeaponBarrelInfo`'s `m_projectileOffsetMtx`, a pristine bone matrix).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LaunchBarrel {
    /// Model space translation (x east, y north, z up).
    pub pos: [f32; 3],
    /// Unit quaternion `(x, y, z, w)` of the bone's rest rotation. A missile launched from the barrel takes the
    /// rotated X axis as its heading (`MissileAIUpdate::projectileFireAtObjectOrPosition`).
    pub rot: [f32; 4],
}

/// `WeaponLaunchBone` barrel rest bones (model space) for one weapon slot, in barrel order.
pub type LaunchBarrels = Vec<LaunchBarrel>;

/// The model turret that drives a weapon slot's launch bones (`W3DModelDraw` `Turret` /
/// `TurretPitch` + `TurretAI` `ControlledWeaponSlots`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TurretLaunch {
    /// Weapon slots whose barrels sit on this turret (`TurretAIData::m_turretWeaponSlots`).
    pub slots: u8,
    /// The turret yaw bone's rest position (model space); a launch offset rotates about it
    /// (`Weapon::calcProjectileLaunchPosition`'s `turretRotPos`).
    pub yaw_pivot: [f32; 3],
    /// The turret pitch bone's rest position (model space), if the model has one
    /// (`turretPitchPos`).
    pub pitch_pivot: Option<[f32; 3]>,
}

/// One `W3DModelDraw` condition state's launch data (`ModelConditionInfo::m_weaponBarrelInfoVec` and its
/// `m_turrets`); the engine picks the state with `findBestInfo` from the drawable's current conditions.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LaunchState {
    /// The state's condition rules (`ModelConditionInfo::m_conditionsYesVec`, one per `ConditionState`/alias) as the
    /// sim's `ModelConditionFlags` bits. An all-zero entry is a default state.
    pub conditions: Vec<u128>,
    /// Per weapon slot, the state's `WeaponLaunchBone` rest matrices.
    pub slots: [LaunchBarrels; WEAPON_SLOT_COUNT],
    /// The state's `Turret`/`TurretPitch` bones, turned into the pivots `calcProjectileLaunchPosition` rotates about.
    /// `W3DModelDraw::getProjectileLaunchOffset` adds the module's `AttachToBoneInAnotherModule` offset to both pivots
    /// (`CACHE_ATTACH_BONE`, W3DModelDraw.cpp:3363-3388); the resolver bakes it in here.
    pub turret: Option<TurretLaunch>,
}

/// One `W3DModelDraw` draw module's condition-state table (`W3DModelDrawModuleData::m_conditionStates` and its
/// `IgnoreConditionStates`). `Drawable::getProjectileLaunchOffset` walks an object's draw modules in `Draw` order and
/// lets the first module whose selected state has a barrel for the slot answer (Drawable.cpp:662-671), so the per-slot
/// launch data is a merge over these tables (`LaunchBones::select_slot`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LaunchModule {
    /// `IgnoreConditionStates` (`W3DModelDrawModuleData::m_ignoreConditionStates`): cleared from the query before this
    /// module's own `findBestInfo`.
    pub ignore: u128,
    /// Every `ConditionState`/`DefaultConditionState` of the module, in state order.
    pub states: Vec<LaunchState>,
}

/// The launch data one `Drawable::getProjectileLaunchOffset` call resolves for a weapon slot.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LaunchSlot<'a> {
    /// The answering module state's `WeaponLaunchBone` barrels for the slot. Empty when no module answers (the C++
    /// caller then uses an identity matrix, i.e. the object's origin).
    pub barrels: &'a [LaunchBarrel],
    /// The answering state's turret; its pivots already carry the module's attach-bone offset.
    pub turret: Option<&'a TurretLaunch>,
}

/// `TheGlobalData->m_timeOfDay == TIME_OF_DAY_NIGHT` / `m_weather == WEATHER_SNOWY` as the `NIGHT`/`SNOW`
/// model-condition bits a drawable gets at bind time (`Object::friend_bindToDrawable`, Object.cpp:3038-3048) and on
/// `GameClient::setTimeOfDay` (`Drawable::setTimeOfDay`, Drawable.cpp:4319-4330). Both are game-wide (`GlobalData`)
/// and the engine defaults are afternoon / normal (GlobalData.cpp:725-730). They belong to the launch query because
/// `getProjectileLaunchOffset` matches the drawable's condition state (`findBestInfo`), and a state can be selected by
/// them (`ModelConditionInfo::matchesMode`, W3DModelDraw.cpp:417-430).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LaunchEnvironment {
    pub night: bool,
    pub snow: bool,
}

impl LaunchEnvironment {
    /// The `NIGHT` / `SNOW` bits for a `ModelConditionFlags` query.
    pub fn bits(&self) -> u128 {
        let mut bits = 0u128;
        if self.night {
            if let Some(b) = crate::condition::bit_for_name("NIGHT") {
                bits |= 1u128 << b;
            }
        }
        if self.snow {
            if let Some(b) = crate::condition::bit_for_name("SNOW") {
                bits |= 1u128 << b;
            }
        }
        bits
    }
}

/// Launch-bone data resolved from an object's model condition states at load (`zr_game::launch_bones`). Model space is
/// Generals (x east, y north, z up).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LaunchBones {
    /// Per weapon slot, the `WeaponLaunchBone` rest positions of the state `findBestInfo` picks for the empty condition
    /// set (the object's rest/default state), one per barrel. `None` when no module has a barrel for the slot.
    pub slots: [LaunchBarrels; WEAPON_SLOT_COUNT],
    /// The default-condition turret: that of the module that answers slot 0 for the empty query, if any. Used only as the
    /// fallback for a template without state tables; fire-time selection returns the answering state's turret.
    pub turret: Option<TurretLaunch>,
    /// The object's first launch-capable draw module's condition states, in state order
    /// (`W3DModelDrawModuleData::m_conditionStates`). Kept as the single-table form (synthetic templates set only
    /// this); `select_slot` walks this table first, then [`extra_modules`](Self::extra_modules).
    pub states: Vec<LaunchState>,
    /// `IgnoreConditionStates` of the module in [`states`](Self::states).
    pub ignore: u128,
    /// The object's further launch-capable `W3D*Draw` modules, in `Draw` order after the one in
    /// [`states`](Self::states). `Drawable::getProjectileLaunchOffset` continues with them when the earlier modules'
    /// selected states have no barrel for the slot (e.g. the GLA Technical's turret module, `ModuleTag_02`, which holds
    /// the crate-upgrade `Muzzle02`/`Muzzle03` bones while the first module is the truck body).
    pub extra_modules: Vec<LaunchModule>,
}

impl LaunchBones {
    /// `W3DModelDrawModuleData::findBestInfo` (`SparseMatchFinder::findBestInfoSlow`, SparseMatchFinder.h:123-186) over
    /// one module table: the state with the most matching "yes" bits wins, ties go to the fewest extraneous yes bits, and
    /// a remaining tie keeps the earlier state. The query's ignored bits are cleared first. Falls back to the
    /// precomputed default when the table is empty.
    pub fn select(&self, conditions: u128) -> (&[LaunchBarrels; WEAPON_SLOT_COUNT], Option<&TurretLaunch>) {
        match best_state(self.ignore, &self.states, conditions) {
            Some(idx) => (&self.states[idx].slots, self.states[idx].turret.as_ref()),
            None => (&self.slots, self.turret.as_ref()),
        }
    }

    /// `Drawable::getProjectileLaunchOffset(wslot, ...)` (Drawable.cpp:662-671): walk the object's draw modules in
    /// `Draw` order; the first module whose `findBestInfo` state for `conditions` has a barrel for `slot` answers with
    /// that state's barrel list and turret. The first module is [`states`](Self::states) (with
    /// [`ignore`](Self::ignore)), then [`extra_modules`](Self::extra_modules) in order (each with its own ignore list).
    /// No answer leaves an empty slot (the engine returns false and the caller uses an identity matrix). A template with
    /// no state table at all (synthetic tests) falls back to the precomputed [`slots`](Self::slots).
    pub fn select_slot(&self, slot: usize, conditions: u128) -> LaunchSlot<'_> {
        if self.states.is_empty() && self.extra_modules.is_empty() {
            return LaunchSlot { barrels: self.slots.get(slot).map_or(&[], Vec::as_slice), turret: self.turret.as_ref() };
        }
        if let Some(idx) = best_state(self.ignore, &self.states, conditions) {
            let state = &self.states[idx];
            if state.slots.get(slot).is_some_and(|b| !b.is_empty()) {
                return LaunchSlot { barrels: state.slots[slot].as_slice(), turret: state.turret.as_ref() };
            }
        }
        for module in &self.extra_modules {
            if let Some(idx) = best_state(module.ignore, &module.states, conditions) {
                let state = &module.states[idx];
                if state.slots.get(slot).is_some_and(|b| !b.is_empty()) {
                    return LaunchSlot { barrels: state.slots[slot].as_slice(), turret: state.turret.as_ref() };
                }
            }
        }
        // Every module skipped this slot: an identity matrix, as `calcProjectileLaunchPosition` falls back to
        // (Weapon.cpp:3065-3072). `launch_position` maps an empty barrel list to the object's position.
        LaunchSlot::default()
    }
}

/// Index of the [`LaunchState`] `findBestInfo` selects in one module table, or `None` for an empty table.
fn best_state(ignore: u128, states: &[LaunchState], conditions: u128) -> Option<usize> {
    let bits = conditions & !ignore;
    let mut best: Option<(usize, i32, i32)> = None;
    for (idx, st) in states.iter().enumerate() {
        for c in st.conditions.iter().rev() {
            let yes = (c & bits).count_ones() as i32;
            let extra = (c & !bits).count_ones() as i32;
            let replace = match best {
                None => true,
                Some((_, best_yes, best_extra)) => yes > best_yes || (yes == best_yes && extra < best_extra),
            };
            if replace {
                best = Some((idx, yes, extra));
            }
        }
    }
    best.map(|(idx, _, _)| idx)
}

/// Combat-related template data attached to an [`ObjectTemplate`](crate::template::ObjectTemplate).
#[derive(Clone, Debug)]
pub struct CombatTemplate {
    /// The armor in force for the normal armor set.
    pub armor: Option<Arc<ArmorTemplate>>,
    /// The weapon set for the normal (no-upgrade) condition.
    pub weapon_set: Option<WeaponSetTemplate>,
    /// `WeaponLaunchBone` launch positions per weapon slot, resolved over every `W3D*Draw` module of the model
    /// (`zr_game::launch_bones`, `LaunchBones::select_slot`).
    pub launch_bones: LaunchBones,
    /// `TurretTurnRate` (radians per frame) of the first turret, if the unit has one.
    pub turret_turn_rate: Option<f32>,
    /// `TurretPitchRate` (radians per frame); `None` disables pitch slewing.
    pub turret_pitch_rate: Option<f32>,
    /// `NaturalTurretAngle` (radians), the resting angle relative to the body (`TurretAIData`).
    pub turret_natural_angle: f32,
    /// `ControlledWeaponSlots` (`TurretAIData::m_turretWeaponSlots`): which slots this turret
    /// drives. The engine throws `INI_INVALID_DATA` when it is 0; the sim's synthetic templates
    /// omit it, so an empty mask is treated as every slot (`turret_slot_on_turret`).
    pub turret_controlled_slots: u8,
    /// `TurretFireAngleSweep` per weapon slot (radians); 0 disables the sweep.
    pub turret_fire_angle_sweep: [f32; WEAPON_SLOT_COUNT],
    /// `TurretSweepSpeedModifier` per weapon slot (default 1).
    pub turret_sweep_speed_modifier: [f32; WEAPON_SLOT_COUNT],
    /// `MinIdleScanAngle` (radians, relative to the natural angle).
    pub turret_min_idle_scan_angle: f32,
    /// `MaxIdleScanAngle` (radians).
    pub turret_max_idle_scan_angle: f32,
    /// `MinIdleScanInterval` (frames).
    pub turret_min_idle_scan_interval: u32,
    /// `MaxIdleScanInterval` (frames; the default effectively never scans).
    pub turret_max_idle_scan_interval: u32,
    /// `RecenterTime` (frames, default 2 s).
    pub turret_recenter_time: u32,
    /// `InitiallyDisabled`.
    pub turret_initially_disabled: bool,
    /// `FiresWhileTurning` (gates the rotation-sound bookkeeping only).
    pub turret_fires_while_turning: bool,
    /// `TurretsLinked` (`AIUpdateModuleData::m_turretsLinked`): every turret fires together and a
    /// turret accepts a weapon slot it does not own (`TurretAI::isWeaponSlotOkToFire`).
    pub turrets_linked: bool,
    /// `AllowsPitch`.
    pub turret_allows_pitch: bool,
    /// `FirePitch` (radians); > 0 fires at a fixed pitch instead of tracking the target.
    pub turret_fire_pitch: f32,
    /// `MinPhysicalPitch` (radians).
    pub turret_min_pitch: f32,
    /// `GroundUnitPitch` (radians).
    pub turret_ground_unit_pitch: f32,
    /// `NaturalTurretPitch` (radians), the resting pitch.
    pub turret_natural_pitch: f32,
    /// Half the object's height above its position (`TurretAI` pitch reference point).
    pub turret_max_height: f32,
    /// `AutoAcquireEnemiesWhenIdle` contains `YES` (or `ATTACK_BUILDINGS`).
    pub auto_acquire_enemies: bool,
    /// `AutoAcquireEnemiesWhenIdle` contains `ATTACK_BUILDINGS`.
    pub auto_acquire_buildings: bool,
    /// `AutoAcquireEnemiesWhenIdle` contains `NOTWHILEATTACKING` (`AAS_Idle_Not_While_Attacking`, AIUpdate.h:119):
    /// an attacking unit does not look for a mood target (`getNextMoodTarget`, AIUpdate.cpp:4555).
    pub auto_acquire_not_while_attacking: bool,
    /// `AutoAcquireEnemiesWhenIdle` contains `STEALTHED` (`AAS_Idle_Stealthed`, AIUpdate.h:117): a stealthed
    /// idle unit may acquire (`canAutoAcquireWhileStealthed`, AIUpdate.cpp:4524).
    pub auto_acquire_while_stealthed: bool,
    /// `MoodAttackCheckRate` (frames between idle target scans).
    pub mood_attack_check_rate: u32,
    /// Set-condition bits for armor sets (veterancy / `PLAYER_UPGRADE`).
    pub armor_flag_bits: crate::upgrade::SetFlagBits,
    /// Set-condition bits for weapon sets (veterancy / `PLAYER_UPGRADE`).
    pub weapon_flag_bits: crate::upgrade::SetFlagBits,
}

impl Default for CombatTemplate {
    fn default() -> Self {
        Self {
            armor: None,
            weapon_set: None,
            launch_bones: LaunchBones::default(),
            turret_turn_rate: None,
            turret_pitch_rate: None,
            turret_natural_angle: 0.0,
            turret_controlled_slots: 0,
            turret_fire_angle_sweep: [0.0; WEAPON_SLOT_COUNT],
            turret_sweep_speed_modifier: [1.0; WEAPON_SLOT_COUNT],
            turret_min_idle_scan_angle: 0.0,
            turret_max_idle_scan_angle: 0.0,
            turret_min_idle_scan_interval: 9_999_999,
            turret_max_idle_scan_interval: 9_999_999,
            turret_recenter_time: 2 * crate::math::LOGICFRAMES_PER_SECOND,
            turret_initially_disabled: false,
            turret_fires_while_turning: false,
            turrets_linked: false,
            turret_allows_pitch: false,
            turret_fire_pitch: 0.0,
            turret_min_pitch: 0.0,
            turret_ground_unit_pitch: 0.0,
            turret_natural_pitch: 0.0,
            turret_max_height: 0.0,
            auto_acquire_enemies: false,
            auto_acquire_buildings: false,
            auto_acquire_not_while_attacking: false,
            auto_acquire_while_stealthed: false,
            mood_attack_check_rate: 2 * crate::math::LOGICFRAMES_PER_SECOND,
            armor_flag_bits: crate::upgrade::SetFlagBits::default(),
            weapon_flag_bits: crate::upgrade::SetFlagBits::default(),
        }
    }
}

/// `WeaponStatus`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WeaponStatus {
    ReadyToFire,
    BetweenFiringShots,
    ReloadingClip,
    PreAttack,
    OutOfAmmo,
}

const NEVER: u32 = 0x7fff_ffff;

/// A weapon instance (`Weapon`). Weapons are created empty; [`WeaponSet`] loads them instantly.
#[derive(Clone, Debug)]
pub struct Weapon {
    pub template: Arc<WeaponTemplate>,
    pub slot: usize,
    status: WeaponStatus,
    ammo_in_clip: i32,
    when_we_can_fire_again: u32,
    when_pre_attack_finished: u32,
    when_last_reload_started: u32,
    last_fire_frame: u32,
    /// `Weapon::m_curBarrel`: barrel used next (`privateFireWeapon`).
    cur_barrel: usize,
    /// `Weapon::m_numShotsForCurBarrel`: shots left on the current barrel before cycling.
    num_shots_for_cur_barrel: i32,
}

impl Weapon {
    pub fn new(template: Arc<WeaponTemplate>, slot: usize) -> Self {
        let pieces = template.shots_per_barrel.max(1);
        Self {
            template,
            slot,
            status: WeaponStatus::OutOfAmmo,
            ammo_in_clip: 0,
            when_we_can_fire_again: 0,
            when_pre_attack_finished: 0,
            when_last_reload_started: 0,
            last_fire_frame: 0,
            cur_barrel: 0,
            num_shots_for_cur_barrel: pieces,
        }
    }

    /// `Weapon::privateFireWeapon`'s barrel reset: the barrel to use for this shot. `barrel_count`
    /// is the model's barrel count for the slot (the sim's resolved `WeaponLaunchBone` list).
    pub fn current_barrel(&mut self, barrel_count: usize) -> usize {
        if barrel_count == 0 {
            return 0;
        }
        if self.cur_barrel >= barrel_count {
            self.cur_barrel = 0;
            self.num_shots_for_cur_barrel = self.template.shots_per_barrel.max(1);
        }
        self.cur_barrel
    }

    /// `Weapon::privateFireWeapon`'s barrel cycle after a shot.
    pub fn advance_barrel(&mut self, barrel_count: usize) {
        if barrel_count == 0 {
            return;
        }
        self.num_shots_for_cur_barrel -= 1;
        if self.num_shots_for_cur_barrel <= 0 {
            self.cur_barrel += 1;
            self.num_shots_for_cur_barrel = self.template.shots_per_barrel.max(1);
        }
    }

    pub fn ammo_in_clip(&self) -> i32 {
        self.ammo_in_clip
    }
    /// `Weapon::setClipPercentFull` (used by the airfield rearm).
    pub fn set_ammo_in_clip(&mut self, ammo: i32) {
        self.ammo_in_clip = ammo.clamp(0, self.template.clip_size.max(0));
    }
    /// `Weapon::setClipPercentFull` (`Weapon.cpp:1961`): raise the clip to `percent` full (lower it
    /// too when `allow_reduction`). The engine writes `m_status = ammo ? OUT_OF_AMMO : READY_TO_FIRE`
    /// (sic) but sets `m_whenWeCanFireAgain = now`, so [`status`](Self::status) reads READY/OUT by
    /// ammo from the next query; the sim stores `ReloadingClip` as the transient value.
    pub fn set_clip_percent_full(&mut self, percent: f32, allow_reduction: bool, now: u32) {
        if self.template.clip_size == 0 {
            return;
        }
        let ammo = crate::math::floor(self.template.clip_size as f32 * percent) as i32;
        if ammo > self.ammo_in_clip || (allow_reduction && ammo < self.ammo_in_clip) {
            self.ammo_in_clip = ammo;
            self.status = WeaponStatus::ReloadingClip;
            self.when_last_reload_started = now;
            self.when_we_can_fire_again = now;
        }
    }
    pub fn last_fire_frame(&self) -> u32 {
        self.last_fire_frame
    }
    /// Frame at which the weapon can fire again (`m_whenWeCanFireAgain`).
    pub fn when_we_can_fire_again(&self) -> u32 {
        self.when_we_can_fire_again
    }

    /// `Weapon::getStatus` (the engine caches the READY/OUT_OF_AMMO result; here it is derived).
    pub fn status(&self, now: u32) -> WeaponStatus {
        if now < self.when_pre_attack_finished {
            return WeaponStatus::PreAttack;
        }
        if now >= self.when_we_can_fire_again {
            return if self.ammo_in_clip > 0 { WeaponStatus::ReadyToFire } else { WeaponStatus::OutOfAmmo };
        }
        self.status
    }

    /// `Weapon::loadAmmoNow`.
    pub fn load_ammo_now(&mut self, now: u32) {
        self.reload(now, true, 1.0);
    }

    /// `Weapon::reloadAmmo` (reload time applies).
    pub fn reload_ammo(&mut self, now: u32) {
        self.reload(now, false, 1.0);
    }

    /// `Weapon::reloadWithBonus`: divide the clip reload time by the RATE_OF_FIRE bonus.
    fn reload(&mut self, now: u32, instantly: bool, rate_of_fire: f32) {
        if self.template.clip_size > 0 && self.ammo_in_clip == self.template.clip_size {
            return; // do not restart our reload delay
        }
        self.ammo_in_clip = if self.template.clip_size <= 0 { i32::MAX } else { self.template.clip_size };
        self.status = WeaponStatus::ReloadingClip;
        let rof = if rate_of_fire > 0.0 { rate_of_fire } else { 1.0 };
        let reload_time = if instantly { 0 } else { crate::math::floor(self.template.clip_reload_time as f32 / rof) as u32 };
        self.when_last_reload_started = now;
        self.when_we_can_fire_again = now + reload_time;
    }

    /// `Weapon::getPreAttackDelay`: `consecutive_shots` is `getNumConsecutiveShotsFiredAtTarget`.
    /// `1.0` is the default `WeaponBonus` (`Weapon.h`, `WeaponBonus::clear`), as `Weapon::computeBonus`
    /// leaves it when no bonus condition applies.
    pub fn pre_attack_delay(&self, consecutive_shots: u32) -> u32 {
        self.pre_attack_delay_with(consecutive_shots, 1.0)
    }

    /// `WeaponTemplate::getPreAttackDelay` (Weapon.cpp:528-531): `m_preAttackDelay * bonus.getField(PRE_ATTACK)`,
    /// with no fallback: a PRE_ATTACK bonus of 0 gives a delay of 0 (`Weapon::preFireWeapon` then starts no
    /// pre-attack, Weapon.cpp:2795-2807).
    pub fn pre_attack_delay_with(&self, consecutive_shots: u32, pre_attack: f32) -> u32 {
        let base = match self.template.prefire {
            Prefire::PerClip if self.template.clip_size > 0 && self.ammo_in_clip < self.template.clip_size => 0,
            Prefire::PerAttack if consecutive_shots > 0 => 0,
            _ => self.template.pre_attack_delay,
        };
        (base as f32 * pre_attack) as u32
    }

    /// `Weapon::preFireWeapon` with the default `WeaponBonus`.
    pub fn pre_fire(&mut self, now: u32, consecutive_shots: u32) {
        self.pre_fire_with(now, consecutive_shots, 1.0);
    }

    /// `Weapon::preFireWeapon` with the PRE_ATTACK bonus.
    pub fn pre_fire_with(&mut self, now: u32, consecutive_shots: u32, pre_attack: f32) {
        let delay = self.pre_attack_delay_with(consecutive_shots, pre_attack);
        if delay > 0 {
            self.status = WeaponStatus::PreAttack;
            self.when_pre_attack_finished = now + delay;
        }
    }

    /// `AIAttackFireWeaponState::onExit`: abandon a pending pre-attack.
    pub fn cancel_pre_attack(&mut self, now: u32) {
        if self.status(now) == WeaponStatus::PreAttack {
            self.when_pre_attack_finished = 0;
        }
    }

    /// The state change of `Weapon::privateFireWeapon` after a shot. The caller has checked
    /// [`status`](Self::status) is `ReadyToFire`. Returns `true` if the clip auto-reloaded.
    pub fn consume_shot(&mut self, now: u32, rng: &mut GameRandom) -> bool {
        self.consume_shot_with(now, rng, 1.0)
    }

    /// `Weapon::privateFireWeapon` with the RATE_OF_FIRE bonus (delay and reload).
    pub fn consume_shot_with(&mut self, now: u32, rng: &mut GameRandom, rate_of_fire: f32) -> bool {
        self.last_fire_frame = now;
        self.ammo_in_clip -= 1;
        if self.ammo_in_clip <= 0 {
            if self.template.auto_reloads_clip {
                self.reload(now, false, rate_of_fire);
                return true;
            }
            self.status = WeaponStatus::OutOfAmmo;
            self.when_we_can_fire_again = NEVER;
        } else {
            self.status = WeaponStatus::BetweenFiringShots;
            let delay = self.template.delay_between_shots_with(rng, rate_of_fire);
            self.when_last_reload_started = now;
            self.when_we_can_fire_again = now + delay;
        }
        false
    }

    fn crc(&self, c: &mut Crc) {
        c.add_u32(self.status as u32);
        c.add_u32(self.ammo_in_clip as u32);
        c.add_u32(self.when_we_can_fire_again);
        c.add_u32(self.when_pre_attack_finished);
        c.add_u32(self.when_last_reload_started);
        c.add_u32(self.last_fire_frame);
        c.add_u32(self.cur_barrel as u32);
        c.add_u32(self.num_shots_for_cur_barrel as u32);
    }
}

/// `WeaponLockType` (`WeaponSet.h`): the two lock kinds `setWeaponLock` / `releaseWeaponLock` take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WeaponLockType {
    /// `LOCKED_TEMPORARILY`: a special attack holds its slot until the clip is spent or the unit's
    /// group is released (`WeaponSet::releaseWeaponLock`, `WeaponSet.cpp:1113`).
    Temporarily,
    /// `LOCKED_PERMANENTLY`: `MSG_SWITCH_WEAPONS`, released only by a weapon-set change.
    Permanently,
}

/// `WeaponSet`: the object's weapon instances.
#[derive(Clone, Debug, Default)]
pub struct WeaponSet {
    pub weapons: [Option<Weapon>; WEAPON_SLOT_COUNT],
    /// The `LOCKED_PERMANENTLY` slot (`WeaponSet::m_curWeaponLockedStatus` with `m_curWeapon`):
    /// `MSG_SWITCH_WEAPONS` (`WeaponSet::setWeaponLock`, `WeaponSet.cpp:1078`) forces this weapon until
    /// another switch or a weapon-set change (`updateWeaponSet`, `WeaponSet.cpp:329`).
    pub permanent_lock: Option<usize>,
    /// The `LOCKED_TEMPORARILY` slot. C++ keeps one status for both kinds, so this is set only while
    /// there is no permanent lock ([`WeaponSet::set_weapon_lock`]). It lives on the object, not the
    /// attack order: an attack's end does not release it (`AIAttackState::onExit` does not, `AIStates.cpp:5714`).
    pub temporary_lock: Option<usize>,
}

impl WeaponSet {
    /// `WeaponSet::updateWeaponSet` for a fresh object: every weapon starts with a full clip.
    pub fn from_template(t: &WeaponSetTemplate, now: u32) -> Self {
        let mut set = WeaponSet::default();
        for (i, w) in t.weapons.iter().enumerate() {
            if let Some(w) = w {
                let mut weapon = Weapon::new(w.clone(), i);
                weapon.load_ammo_now(now);
                set.weapons[i] = Some(weapon);
            }
        }
        set
    }

    pub fn is_empty(&self) -> bool {
        self.weapons.iter().all(Option::is_none)
    }

    pub fn weapon(&self, slot: usize) -> Option<&Weapon> {
        self.weapons.get(slot).and_then(Option::as_ref)
    }

    /// `WeaponSet::isCurWeaponLocked` (`WeaponSet.cpp`): either kind of lock is held.
    pub fn is_cur_weapon_locked(&self) -> bool {
        self.permanent_lock.is_some() || self.temporary_lock.is_some()
    }

    /// The locked slot, if any: the permanent one first (the status holds one slot, `m_curWeapon`).
    pub fn locked_slot(&self) -> Option<usize> {
        self.permanent_lock.or(self.temporary_lock)
    }

    /// `WeaponSet::setWeaponLock` (`WeaponSet.cpp:1078-1108`). A permanent lock takes the slot. A temporary
    /// lock takes it only when no permanent lock is held (`m_curWeaponLockedStatus != LOCKED_PERMANENTLY`).
    /// Returns `false` when the slot has no weapon (`:1106`).
    pub fn set_weapon_lock(&mut self, slot: usize, kind: WeaponLockType) -> bool {
        if self.weapon(slot).is_none() {
            return false;
        }
        match kind {
            WeaponLockType::Permanently => {
                self.permanent_lock = Some(slot);
                self.temporary_lock = None;
            }
            WeaponLockType::Temporarily => {
                if self.permanent_lock.is_none() {
                    self.temporary_lock = Some(slot);
                }
            }
        }
        true
    }

    /// `WeaponSet::releaseWeaponLock` (`WeaponSet.cpp:1113-1133`). A permanent release sets the status to
    /// `NOT_LOCKED`, which drops a temporary lock too. A temporary release only clears a temporary status.
    pub fn release_weapon_lock(&mut self, kind: WeaponLockType) {
        match kind {
            WeaponLockType::Permanently => {
                self.permanent_lock = None;
                self.temporary_lock = None;
            }
            WeaponLockType::Temporarily => {
                if self.permanent_lock.is_none() {
                    self.temporary_lock = None;
                }
            }
        }
    }

    pub(crate) fn crc(&self, c: &mut Crc) {
        for w in &self.weapons {
            match w {
                Some(w) => {
                    c.add_bool(true);
                    w.crc(c);
                }
                None => c.add_bool(false),
            }
        }
        c.add_u32(self.permanent_lock.map_or(u32::MAX, |s| s as u32));
        // Only hashed when held, so objects without a temporary lock keep their historical CRC.
        if let Some(s) = self.temporary_lock {
            c.add_u32(s as u32);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gun() -> Arc<WeaponTemplate> {
        let mut w = WeaponTemplate::new("Gun");
        w.primary_damage = 10.0;
        w.attack_range = 100.0;
        w.min_delay_between_shots = 5;
        w.max_delay_between_shots = 5;
        w.clip_size = 3;
        w.clip_reload_time = 20;
        Arc::new(w)
    }

    #[test]
    fn clip_and_reload_timing() {
        let mut rng = GameRandom::new(1);
        let mut w = Weapon::new(gun(), 0);
        assert_eq!(w.status(0), WeaponStatus::OutOfAmmo);
        w.load_ammo_now(10);
        assert_eq!(w.status(10), WeaponStatus::ReadyToFire);
        assert!(!w.consume_shot(10, &mut rng));
        assert_eq!(w.status(14), WeaponStatus::BetweenFiringShots);
        assert_eq!(w.status(15), WeaponStatus::ReadyToFire);
        assert!(!w.consume_shot(15, &mut rng));
        assert!(w.consume_shot(20, &mut rng)); // third shot empties the clip and reloads
        assert_eq!(w.status(39), WeaponStatus::ReloadingClip);
        assert_eq!(w.status(40), WeaponStatus::ReadyToFire);
        assert_eq!(w.ammo_in_clip(), 3);
    }

    #[test]
    fn range_is_undersized_by_a_quarter_cell() {
        let t = gun();
        assert_eq!(t.attack_range(), 97.5);
        assert_eq!(t.min_attack_range(), 0.0);
    }

    /// `WeaponTemplate::estimateWeaponTemplateDamage`'s garrison branches (`Weapon.cpp:632-641`) and
    /// `ActiveBody::estimateDamage`'s `DAMAGE_KILL_GARRISONED` case (`ActiveBody.cpp:294-302`). A clear-building weapon
    /// estimates 1.0 only for a garrison it can clear (garrisonable, occupied, not immune); a normal weapon is
    /// unaffected by the garrison state. Reverting the two branches (armour-adjusted damage only) fails this.
    #[test]
    fn the_damage_estimate_knows_a_clearable_garrison() {
        let mut clear = WeaponTemplate::new("Clear");
        clear.damage_type = DamageType::KillGarrisoned;
        assert_eq!(clear.estimate_damage(None, true), 1.0);
        assert_eq!(clear.estimate_damage(None, false), 0.0, "an immune or empty garrison is not worth clearing");
        let mut allow = WeaponTemplate::new("Allow");
        allow.primary_damage = 50.0;
        allow.allow_attack_garrisoned_bldgs = true;
        assert_eq!(allow.estimate_damage(None, true), 1.0);
        assert_eq!(allow.estimate_damage(None, false), 50.0, "the normal damage when there is nothing to clear");
        let mut surrender = WeaponTemplate::new("Surrender");
        surrender.damage_type = DamageType::Surrender;
        assert_eq!(surrender.estimate_damage(None, true), 1.0);
        assert_eq!(surrender.estimate_damage(None, false), 0.0);
        assert_eq!(gun().estimate_damage(None, true), 10.0, "a normal weapon ignores the garrison");
        assert_eq!(gun().estimate_damage(None, false), 10.0);
    }

    /// `WeaponTemplate::isContactWeapon` (`Weapon.cpp:557-569`, `RATIONALIZE_ATTACK_RANGE` defined): a range under one
    /// pathfind cell after the quarter-cell undersize. Reverting the formula (e.g. to a fixed `false`) fails this.
    #[test]
    fn a_contact_weapon_has_a_range_under_one_pathfind_cell() {
        let mut w = WeaponTemplate::new("Contact");
        w.attack_range = 12.0; // 12 - 2.5 = 9.5 < 10
        assert!(w.is_contact_weapon());
        w.attack_range = 12.4; // 9.9 < 10
        assert!(w.is_contact_weapon());
        w.attack_range = 12.5; // 10.0 is not < 10
        assert!(!w.is_contact_weapon());
        w.attack_range = 100.0;
        assert!(!w.is_contact_weapon());
        assert!(!gun().is_contact_weapon(), "a 100-unit gun is not a contact weapon");
    }

    /// `W3DModelDrawModuleData::findBestInfo` over the launch states: the most matching "yes" bits wins, ties go to the
    /// fewest extraneous yes bits, and a remaining tie keeps the earlier state (SparseMatchFinder.h:123-186). The
    /// query's ignored bits are cleared first. Reverting `select` to always return the default state fails this.
    #[test]
    fn launch_states_are_picked_like_find_best_info() {
        let barrel = |x: f32| LaunchBarrel { pos: [x, 0.0, 0.0], rot: [0.0, 0.0, 0.0, 1.0] };
        let bit = |name: &str| 1u128 << crate::condition::bit_for_name(name).unwrap();
        let state = |conditions: Vec<u128>, x: f32| LaunchState {
            conditions,
            slots: [vec![barrel(x)], Vec::new(), Vec::new()],
            turret: None,
        };
        let mut lb = LaunchBones { slots: [vec![barrel(0.0)], Vec::new(), Vec::new()], ..LaunchBones::default() };
        assert_eq!(lb.select(0).0[0][0].pos[0], 0.0, "no state table: the precomputed default");
        lb.states.push(state(vec![0], 1.0));
        lb.states.push(state(vec![bit("REALLYDAMAGED")], 2.0));
        lb.states.push(state(vec![bit("REALLYDAMAGED") | bit("MOVING")], 3.0));
        assert_eq!(lb.select(0).0[0][0].pos[0], 1.0, "the empty query takes the default state");
        assert_eq!(lb.select(bit("MOVING")).0[0][0].pos[0], 3.0, "one matched bit beats the default state's zero");
        assert_eq!(
            lb.select(bit("REALLYDAMAGED")).0[0][0].pos[0],
            2.0,
            "both damaged states match one bit; the one with the fewest extras wins"
        );
        assert_eq!(lb.select(bit("REALLYDAMAGED") | bit("MOVING")).0[0][0].pos[0], 3.0, "two matched bits win");
        lb.ignore = bit("MOVING");
        assert_eq!(lb.select(bit("REALLYDAMAGED") | bit("MOVING")).0[0][0].pos[0], 2.0, "IgnoreConditionStates is cleared from the query");
    }

    /// `Drawable::getProjectileLaunchOffset` walks every draw module in `Draw` order (`Drawable.cpp:662-671`): the first
    /// module whose `findBestInfo` state has a barrel for the slot answers, with that state's turret. Reverting
    /// `select_slot` to the single table (`select`/`slots`) leaves the GLA Technical's crate-upgrade shot at the
    /// object origin and fails this.
    #[test]
    fn launch_modules_are_merged_per_slot() {
        let barrel = |x: f32| LaunchBarrel { pos: [x, 0.0, 0.0], rot: [0.0, 0.0, 0.0, 1.0] };
        let bit = |name: &str| 1u128 << crate::condition::bit_for_name(name).unwrap();
        let state = |conditions: Vec<u128>, slot: usize, x: f32, turret: Option<TurretLaunch>| {
            let mut slots: [LaunchBarrels; WEAPON_SLOT_COUNT] = Default::default();
            slots[slot] = vec![barrel(x)];
            LaunchState { conditions, slots, turret }
        };
        let turret = TurretLaunch { slots: 0b111, yaw_pivot: [0.0, 0.0, 3.0], pitch_pivot: None };
        let mut lb = LaunchBones {
            // The first module has only a secondary-slot bone (like the truck body of the Technical).
            states: vec![state(vec![0], 1, 1.0, None)],
            // A second module has the primary bone in its default state, a third only in a NIGHT state.
            extra_modules: vec![
                LaunchModule { ignore: 0, states: vec![state(vec![0], 0, 5.0, Some(turret.clone()))] },
                LaunchModule {
                    ignore: 0,
                    states: vec![state(vec![0], 0, 7.0, None), state(vec![bit("NIGHT")], 0, 11.0, None)],
                },
            ],
            // A precomputed default must not stand in for a table that answered nothing.
            slots: [vec![barrel(99.0)], Vec::new(), vec![barrel(99.0)]],
            ..LaunchBones::default()
        };

        assert_eq!(lb.select_slot(0, 0).barrels[0].pos[0], 5.0, "the second module answers the primary slot");
        assert_eq!(lb.select_slot(0, 0).turret.expect("its turret").yaw_pivot[2], 3.0, "the answering module's turret");
        assert_eq!(lb.select_slot(1, 0).barrels[0].pos[0], 1.0, "the first module keeps the secondary slot");
        assert!(lb.select_slot(2, 0).barrels.is_empty(), "no module has a tertiary barrel: an identity launch");
        // Within a module the query picks the state (`findBestInfo`), so NIGHT can select a different bone.
        lb.extra_modules[0].states[0].slots[0] = Vec::new();
        assert_eq!(lb.select_slot(0, 0).barrels[0].pos[0], 7.0, "the default state of the third module");
        assert_eq!(lb.select_slot(0, bit("NIGHT")).barrels[0].pos[0], 11.0, "the NIGHT state of the third module");
        // Each module clears its own IgnoreConditionStates from the query.
        lb.extra_modules[1].ignore = bit("NIGHT");
        assert_eq!(lb.select_slot(0, bit("NIGHT")).barrels[0].pos[0], 7.0, "the module ignores NIGHT, so the default state keeps the tie");
    }

    #[test]
    fn no_auto_reload_runs_dry() {
        let mut rng = GameRandom::new(1);
        let mut t = WeaponTemplate::new("OneShot");
        t.clip_size = 1;
        t.auto_reloads_clip = false;
        let mut w = Weapon::new(Arc::new(t), 0);
        w.load_ammo_now(0);
        assert!(!w.consume_shot(0, &mut rng));
        assert_eq!(w.status(1000), WeaponStatus::OutOfAmmo);
    }

    #[test]
    fn pre_attack_per_attack_only_first_shot() {
        let mut t = WeaponTemplate::new("Pre");
        t.pre_attack_delay = 6;
        t.prefire = Prefire::PerAttack;
        let mut w = Weapon::new(Arc::new(t), 0);
        w.load_ammo_now(0);
        w.pre_fire(10, 0);
        assert_eq!(w.status(15), WeaponStatus::PreAttack);
        assert_eq!(w.status(16), WeaponStatus::ReadyToFire);
        assert_eq!(w.pre_attack_delay(1), 0);
    }

    /// `WeaponSet::setWeaponLock` / `releaseWeaponLock` (`WeaponSet.cpp:1078-1133`): one status for both kinds. A
    /// temporary lock does not replace a permanent one; a permanent lock replaces a temporary one; a temporary
    /// release leaves a permanent lock; a permanent release clears everything. Reverting the permanent check in
    /// `set_weapon_lock` fails the `temporary_lock` assert.
    #[test]
    fn the_lock_kinds_share_one_status() {
        use WeaponLockType::{Permanently, Temporarily};
        let mut s = WeaponSet::default();
        s.weapons[0] = Some(Weapon::new(gun(), 0));
        s.weapons[1] = Some(Weapon::new(gun(), 1));
        assert!(!s.set_weapon_lock(2, Temporarily), "no weapon in slot 2, no lock");
        assert!(s.set_weapon_lock(0, Temporarily));
        assert!(s.set_weapon_lock(1, Temporarily));
        assert_eq!(s.locked_slot(), Some(1), "a temporary lock replaces a temporary one");
        assert!(s.set_weapon_lock(0, Permanently));
        assert!(s.set_weapon_lock(1, Temporarily));
        assert_eq!((s.locked_slot(), s.temporary_lock), (Some(0), None), "a temporary lock does not replace a permanent one");
        s.release_weapon_lock(Temporarily);
        assert_eq!(s.locked_slot(), Some(0), "a temporary release leaves the permanent lock");
        s.release_weapon_lock(Permanently);
        assert!(!s.is_cur_weapon_locked());
        assert!(s.set_weapon_lock(1, Temporarily));
        s.release_weapon_lock(Temporarily);
        assert!(!s.is_cur_weapon_locked(), "a temporary release clears a temporary status");
    }
}
