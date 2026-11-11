//! `WeaponTemplate`. Ported from `GameLogic/Object/Weapon.cpp` (`WeaponTemplate::TheWeaponTemplateFieldParseTable`,
//! constructor defaults, `parseShotDelay`, `WeaponBonusSet::parseWeaponBonusSet`).
//!
//! Durations are in logic frames, speeds in distance per frame and angles in radians; see
//! [`crate::value`] for the conversions.

use std::collections::BTreeMap;

use zr_ini::{Block, Item};

use crate::diag::{Diag, WarningKind};
use crate::flags::{name_enum, Mask, NameEnum};
use crate::names::*;
use crate::registry::{Ref, ThingId, WeaponId};
use crate::value::{self, parse_coord2d, Args, PI, SEPS_COLON};

name_enum! {
    /// `WeaponAntiMaskType` bits (`Weapon.h`); the individual `Anti*` yes/no fields set one bit each.
    pub enum WeaponAnti {
        AirborneVehicle = "AntiAirborneVehicle",
        Ground = "AntiGround",
        Projectile = "AntiProjectile",
        SmallMissile = "AntiSmallMissile",
        Mine = "AntiMine",
        AirborneInfantry = "AntiAirborneInfantry",
        BallisticMissile = "AntiBallisticMissile",
        Parachute = "AntiParachute",
    }
}

/// Number of veterancy levels (`LEVEL_COUNT`: regular, veteran, elite, heroic).
pub const LEVEL_COUNT: usize = 4;

/// Veterancy level names (`TheVeterancyNames`, `Common/GameCommon.h`... `Player.cpp`).
pub const VETERANCY_NAMES: [&str; LEVEL_COUNT] = ["REGULAR", "VETERAN", "ELITE", "HEROIC"];

/// A weapon definition. Defaults are the `WeaponTemplate` constructor's.
#[derive(Debug, Clone, PartialEq)]
pub struct WeaponTemplate {
    pub name: String,
    pub primary_damage: f32,
    pub primary_damage_radius: f32,
    pub secondary_damage: f32,
    pub secondary_damage_radius: f32,
    pub shock_wave_amount: f32,
    pub shock_wave_radius: f32,
    pub shock_wave_taper_off: f32,
    pub attack_range: f32,
    pub minimum_attack_range: f32,
    pub request_assist_range: f32,
    /// `AcceptableAimDelta`, radians.
    pub acceptable_aim_delta: f32,
    pub scatter_radius: f32,
    pub scatter_target_scalar: f32,
    /// `ScatterRadiusVsInfantry` (`m_infantryInaccuracyDist`).
    pub scatter_radius_vs_infantry: f32,
    pub damage_type: DamageType,
    pub damage_status_type: ObjectStatus,
    pub death_type: DeathType,
    /// Distance per logic frame (`parseVelocityReal`); default 999999 means effectively instant.
    pub weapon_speed: f32,
    pub min_weapon_speed: f32,
    pub scale_weapon_speed: bool,
    /// `WeaponRecoil`, radians.
    pub weapon_recoil: f32,
    pub min_target_pitch: f32,
    pub max_target_pitch: f32,
    pub radius_damage_angle: f32,
    /// `ProjectileObject`; `None` for the `None` keyword or when unset.
    pub projectile_object: Option<Ref<ThingId>>,
    pub fire_sound: Option<String>,
    /// `FireSoundLoopTime`, frames.
    pub fire_sound_loop_time: u32,
    /// Per veterancy level (index = level). `FireFX` sets all four, `VeterancyFireFX` one.
    pub fire_fx: [Option<String>; LEVEL_COUNT],
    pub projectile_detonation_fx: [Option<String>; LEVEL_COUNT],
    pub fire_ocl: [Option<String>; LEVEL_COUNT],
    pub projectile_detonation_ocl: [Option<String>; LEVEL_COUNT],
    pub projectile_exhaust: [Option<String>; LEVEL_COUNT],
    pub clip_size: i32,
    pub continuous_fire_one_shots_needed: i32,
    pub continuous_fire_two_shots_needed: i32,
    /// Frames.
    pub continuous_fire_coast_frames: u32,
    /// Frames.
    pub auto_reload_when_idle_frames: u32,
    /// `ClipReloadTime`, frames (rounded up).
    pub clip_reload_time: u32,
    /// `DelayBetweenShots` range, frames (rounded up). A plain number sets both.
    pub min_delay_between_shots: u32,
    pub max_delay_between_shots: u32,
    pub shots_per_barrel: i32,
    pub damage_dealt_at_self_position: bool,
    pub radius_damage_affects: Mask<WeaponAffects>,
    pub projectile_collides_with: Mask<WeaponCollide>,
    pub anti_mask: Mask<WeaponAnti>,
    pub auto_reloads_clip: WeaponReload,
    pub projectile_stream_name: String,
    pub laser_name: String,
    pub laser_bone_name: String,
    /// `WeaponBonus = CONDITION FIELD percent`: multiplier fraction (1.25 for 125%).
    pub weapon_bonus: BTreeMap<(WeaponBonusCondition, WeaponBonusField), f32>,
    /// Frames.
    pub historic_bonus_time: u32,
    pub historic_bonus_radius: f32,
    pub historic_bonus_count: i32,
    pub historic_bonus_weapon: Option<Ref<WeaponId>>,
    pub leech_range_weapon: bool,
    pub scatter_targets: Vec<(f32, f32)>,
    pub capable_of_following_waypoints: bool,
    pub shows_ammo_pips: bool,
    pub allow_attack_garrisoned_bldgs: bool,
    pub play_fx_when_stealthed: bool,
    /// `PreAttackDelay`, frames.
    pub pre_attack_delay: u32,
    pub pre_attack_type: WeaponPrefire,
    pub continue_attack_range: f32,
    /// `SuspendFXDelay`, frames.
    pub suspend_fx_delay: u32,
    /// `MissileCallsOnDie` (`m_dieOnDetonate`).
    pub missile_calls_on_die: bool,
}

impl WeaponTemplate {
    /// `WeaponTemplate::WeaponTemplate()`.
    pub fn new(name: &str) -> Self {
        WeaponTemplate {
            name: name.to_string(),
            primary_damage: 0.0,
            primary_damage_radius: 0.0,
            secondary_damage: 0.0,
            secondary_damage_radius: 0.0,
            shock_wave_amount: 0.0,
            shock_wave_radius: 0.0,
            shock_wave_taper_off: 0.0,
            attack_range: 0.0,
            minimum_attack_range: 0.0,
            request_assist_range: 0.0,
            acceptable_aim_delta: 0.0,
            scatter_radius: 0.0,
            scatter_target_scalar: 0.0,
            scatter_radius_vs_infantry: 0.0,
            damage_type: DamageType::Explosion,
            damage_status_type: ObjectStatus::None,
            death_type: DeathType::Normal,
            weapon_speed: 999999.0,
            min_weapon_speed: 999999.0,
            scale_weapon_speed: false,
            weapon_recoil: 0.0,
            min_target_pitch: -PI,
            max_target_pitch: PI,
            radius_damage_angle: PI,
            projectile_object: None,
            fire_sound: None,
            fire_sound_loop_time: 0,
            fire_fx: Default::default(),
            projectile_detonation_fx: Default::default(),
            fire_ocl: Default::default(),
            projectile_detonation_ocl: Default::default(),
            projectile_exhaust: Default::default(),
            clip_size: 0,
            continuous_fire_one_shots_needed: i32::MAX,
            continuous_fire_two_shots_needed: i32::MAX,
            continuous_fire_coast_frames: 0,
            auto_reload_when_idle_frames: 0,
            clip_reload_time: 0,
            min_delay_between_shots: 0,
            max_delay_between_shots: 0,
            shots_per_barrel: 1,
            damage_dealt_at_self_position: false,
            radius_damage_affects: [WeaponAffects::Allies, WeaponAffects::Enemies, WeaponAffects::Neutrals]
                .into_iter()
                .collect(),
            projectile_collides_with: Mask::EMPTY.with(WeaponCollide::Structures),
            anti_mask: Mask::EMPTY.with(WeaponAnti::Ground),
            auto_reloads_clip: WeaponReload::Yes,
            projectile_stream_name: String::new(),
            laser_name: String::new(),
            laser_bone_name: String::new(),
            weapon_bonus: BTreeMap::new(),
            historic_bonus_time: 0,
            historic_bonus_radius: 0.0,
            historic_bonus_count: 0,
            historic_bonus_weapon: None,
            leech_range_weapon: false,
            scatter_targets: Vec::new(),
            capable_of_following_waypoints: false,
            shows_ammo_pips: false,
            allow_attack_garrisoned_bldgs: false,
            play_fx_when_stealthed: false,
            pre_attack_delay: 0,
            pre_attack_type: WeaponPrefire::PerShot,
            continue_attack_range: 0.0,
            suspend_fx_delay: 0,
            missile_calls_on_die: false,
        }
    }

    pub(crate) fn from_block(b: &Block, d: &mut Diag) -> Self {
        let mut w = WeaponTemplate::new(b.name().unwrap_or(""));
        w.apply_block(b, d);
        w
    }

    /// Parses `b`'s fields on top of the current values: a `CREATE_OVERRIDES` redefinition
    /// (`WeaponStore::newOverride` copies the template, then `initFromINI` overlays it).
    pub(crate) fn apply_block(&mut self, b: &Block, d: &mut Diag) {
        let w = self;
        d.push_scope(format!("Weapon {}", w.name));
        for item in &b.items {
            let Item::Field(f) = item else {
                d.warn(WarningKind::Structure, "", 0, "unexpected nested block");
                continue;
            };
            let mut a = Args::new(&f.value);
            match w.field(&f.key, &f.value, &mut a) {
                Ok(true) => {}
                Ok(false) => d.unknown_field(f),
                Err(e) => d.bad_value(f, e),
            }
        }
        // `WeaponStore::parseWeaponTemplateDefinition`: `ProjectileObject = None` clears the name.
        if let Some(r) = &w.projectile_object {
            if r.name.eq_ignore_ascii_case("None") {
                w.projectile_object = None;
            }
        }
        d.pop_scope();
    }

    /// One row of `TheWeaponTemplateFieldParseTable`. `Ok(false)` means the key is not in the table.
    fn field(&mut self, key: &str, value: &str, a: &mut Args) -> Result<bool, String> {
        macro_rules! per_vet_all {
            ($dst:expr) => {{
                let v = none_or(a.string());
                for slot in $dst.iter_mut() {
                    *slot = v.clone();
                }
            }};
        }
        macro_rules! per_vet_one {
            ($dst:expr) => {{
                let t = a.next()?;
                let lvl = VETERANCY_NAMES
                    .iter()
                    .position(|n| n.eq_ignore_ascii_case(t))
                    .ok_or_else(|| format!("`{t}` is not a veterancy level"))?;
                $dst[lvl] = none_or(a.string());
            }};
        }
        match key {
            "PrimaryDamage" => self.primary_damage = a.real()?,
            "PrimaryDamageRadius" => self.primary_damage_radius = a.real()?,
            "SecondaryDamage" => self.secondary_damage = a.real()?,
            "SecondaryDamageRadius" => self.secondary_damage_radius = a.real()?,
            "ShockWaveAmount" => self.shock_wave_amount = a.real()?,
            "ShockWaveRadius" => self.shock_wave_radius = a.real()?,
            "ShockWaveTaperOff" => self.shock_wave_taper_off = a.real()?,
            "AttackRange" => self.attack_range = a.real()?,
            "MinimumAttackRange" => self.minimum_attack_range = a.real()?,
            "RequestAssistRange" => self.request_assist_range = a.real()?,
            "AcceptableAimDelta" => self.acceptable_aim_delta = a.angle()?,
            "ScatterRadius" => self.scatter_radius = a.real()?,
            "ScatterTargetScalar" => self.scatter_target_scalar = a.real()?,
            "ScatterRadiusVsInfantry" => self.scatter_radius_vs_infantry = a.real()?,
            "DamageType" => self.damage_type = a.index()?,
            "DamageStatusType" => self.damage_status_type = a.index()?,
            "DeathType" => self.death_type = a.index()?,
            "WeaponSpeed" => self.weapon_speed = a.velocity()?,
            "MinWeaponSpeed" => self.min_weapon_speed = a.velocity()?,
            "ScaleWeaponSpeed" => self.scale_weapon_speed = a.boolean()?,
            "WeaponRecoil" => self.weapon_recoil = a.angle()?,
            "MinTargetPitch" => self.min_target_pitch = a.angle()?,
            "MaxTargetPitch" => self.max_target_pitch = a.angle()?,
            "RadiusDamageAngle" => self.radius_damage_angle = a.angle()?,
            "ProjectileObject" => self.projectile_object = Some(Ref::named(a.string())),
            // `parseAudioEventRTS`: `NoSound` leaves the previous value alone.
            "FireSound" => {
                let t = a.next()?;
                if !t.eq_ignore_ascii_case("NoSound") {
                    self.fire_sound = Some(t.to_string());
                }
            }
            "FireSoundLoopTime" => self.fire_sound_loop_time = a.duration_frames()?,
            "FireFX" => per_vet_all!(self.fire_fx),
            "ProjectileDetonationFX" => per_vet_all!(self.projectile_detonation_fx),
            "FireOCL" => per_vet_all!(self.fire_ocl),
            "ProjectileDetonationOCL" => per_vet_all!(self.projectile_detonation_ocl),
            "ProjectileExhaust" => per_vet_all!(self.projectile_exhaust),
            "VeterancyFireFX" => per_vet_one!(self.fire_fx),
            "VeterancyProjectileDetonationFX" => per_vet_one!(self.projectile_detonation_fx),
            "VeterancyFireOCL" => per_vet_one!(self.fire_ocl),
            "VeterancyProjectileDetonationOCL" => per_vet_one!(self.projectile_detonation_ocl),
            "VeterancyProjectileExhaust" => per_vet_one!(self.projectile_exhaust),
            "ClipSize" => self.clip_size = a.int()?,
            "ContinuousFireOne" => self.continuous_fire_one_shots_needed = a.int()?,
            "ContinuousFireTwo" => self.continuous_fire_two_shots_needed = a.int()?,
            "ContinuousFireCoast" => self.continuous_fire_coast_frames = a.duration_frames()?,
            "AutoReloadWhenIdle" => self.auto_reload_when_idle_frames = a.duration_frames()?,
            "ClipReloadTime" => self.clip_reload_time = a.duration_frames()?,
            "DelayBetweenShots" => {
                let (lo, hi) = parse_shot_delay(value)?;
                self.min_delay_between_shots = lo;
                self.max_delay_between_shots = hi;
            }
            "ShotsPerBarrel" => self.shots_per_barrel = a.int()?,
            "DamageDealtAtSelfPosition" => self.damage_dealt_at_self_position = a.boolean()?,
            "RadiusDamageAffects" => self.radius_damage_affects = self.radius_damage_affects.parse(a.rest())?,
            "ProjectileCollidesWith" => self.projectile_collides_with = self.projectile_collides_with.parse(a.rest())?,
            "AntiAirborneVehicle" | "AntiGround" | "AntiProjectile" | "AntiSmallMissile" | "AntiMine"
            | "AntiParachute" | "AntiAirborneInfantry" | "AntiBallisticMissile" => {
                // `parseBitInInt32`: set or clear one bit.
                let bit = WeaponAnti::from_name(key).expect("table entry");
                if a.boolean()? {
                    self.anti_mask.insert(bit);
                } else {
                    self.anti_mask.remove(bit);
                }
            }
            "AutoReloadsClip" => self.auto_reloads_clip = a.index()?,
            "ProjectileStreamName" => self.projectile_stream_name = a.string(),
            "LaserName" => self.laser_name = a.string(),
            "LaserBoneName" => self.laser_bone_name = a.string(),
            "WeaponBonus" => {
                let cond: WeaponBonusCondition = a.index()?;
                let field: WeaponBonusField = a.index()?;
                self.weapon_bonus.insert((cond, field), a.percent()?);
            }
            "HistoricBonusTime" => self.historic_bonus_time = a.duration_frames()?,
            "HistoricBonusRadius" => self.historic_bonus_radius = a.real()?,
            "HistoricBonusCount" => self.historic_bonus_count = a.int()?,
            "HistoricBonusWeapon" => {
                let n = a.string();
                self.historic_bonus_weapon = if n.eq_ignore_ascii_case("None") { None } else { Some(Ref::named(n)) };
            }
            "LeechRangeWeapon" => self.leech_range_weapon = a.boolean()?,
            "ScatterTarget" => self.scatter_targets.push(parse_coord2d(value)?),
            "CapableOfFollowingWaypoints" => self.capable_of_following_waypoints = a.boolean()?,
            "ShowsAmmoPips" => self.shows_ammo_pips = a.boolean()?,
            "AllowAttackGarrisonedBldgs" => self.allow_attack_garrisoned_bldgs = a.boolean()?,
            "PlayFXWhenStealthed" => self.play_fx_when_stealthed = a.boolean()?,
            "PreAttackDelay" => self.pre_attack_delay = a.duration_frames()?,
            "PreAttackType" => self.pre_attack_type = a.index()?,
            "ContinueAttackRange" => self.continue_attack_range = a.real()?,
            "SuspendFXDelay" => self.suspend_fx_delay = a.duration_frames()?,
            "MissileCallsOnDie" => self.missile_calls_on_die = a.boolean()?,
            _ => return Ok(false),
        }
        Ok(true)
    }
}

/// FX/OCL/particle names: the keyword `None` means no effect (`INI::parseFXList`).
fn none_or(s: String) -> Option<String> {
    if s.is_empty() || s.eq_ignore_ascii_case("None") {
        None
    } else {
        Some(s)
    }
}

/// `WeaponTemplate::parseShotDelay`: `N`, or `Min:N Max:M`; both converted ms to frames rounding up.
/// A `Min` without a following `Max` label uses the min for both.
pub fn parse_shot_delay(value: &str) -> Result<(u32, u32), String> {
    let mut a = Args::with_seps(value, SEPS_COLON);
    let first = a.next()?;
    let (lo, hi) = if first.eq_ignore_ascii_case("Min") {
        let lo = a.uint()?;
        let hi = match a.next_opt() {
            Some(t) if t.eq_ignore_ascii_case("Max") => a.uint()?,
            _ => lo,
        };
        (lo, hi)
    } else {
        let v = value::scan_uint(first)?;
        (v, v)
    };
    Ok((value::duration_ms_to_frames_ceil(lo), value::duration_ms_to_frames_ceil(hi)))
}
