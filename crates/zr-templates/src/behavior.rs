//! Common gameplay update/behavior modules: `AutoHealBehavior`, `FlammableUpdate`,
//! `PoisonedBehavior`, `FireWeaponWhenDeadBehavior`, `LifetimeUpdate`, `StealthUpdate` and
//! `StealthDetectorUpdate`. Ported from each module's `buildFieldParse` (GeneralsMD) and the
//! engine's INI parsers (durations are ms -> frames, velocities per second -> per frame, percents
//! -> fractions).

use zr_ini::{Block, Item};

use crate::diag::{Diag, WarningKind};
use crate::flags::{KindOfMask, Mask};
use crate::names::ObjectStatus;
use crate::registry::{Ref, WeaponId};
use crate::upgrade_module::UpgradeMuxData;
use crate::value::Args;

/// Shared field-loop helper: parse each top-level field with `f`, warning on unknown keys.
pub(crate) fn fields(block: &Block, d: &mut Diag, scope: &str, mut f: impl FnMut(&str, &mut Args) -> Result<bool, String>) {
    for item in &block.items {
        let Item::Field(field) = item else {
            d.warn(WarningKind::Structure, "", 0, format!("unexpected nested block in {scope}"));
            continue;
        };
        let mut a = Args::new(&field.value);
        match f(&field.key, &mut a) {
            Ok(true) => {}
            Ok(false) => d.unknown_field(field),
            Err(e) => d.bad_value(field, e),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// AutoHealBehavior

/// `AutoHealBehaviorModuleData`.
#[derive(Debug, Clone, PartialEq)]
pub struct AutoHealData {
    pub starts_active: bool,
    pub single_burst: bool,
    pub healing_amount: f32,
    /// Frames (`HealingDelay`); `u32::MAX` = never.
    pub healing_delay: u32,
    pub radius: f32,
    /// `KindOf` (defaults to all).
    pub kind_of: KindOfMask,
    pub forbidden_kind_of: KindOfMask,
    /// Frames.
    pub start_healing_delay: u32,
    pub affects_whole_player: bool,
    pub skip_self_for_healing: bool,
    pub upgrade: UpgradeMuxData,
}

impl Default for AutoHealData {
    fn default() -> Self {
        AutoHealData {
            starts_active: false,
            single_burst: false,
            healing_amount: 0.0,
            healing_delay: u32::MAX,
            radius: 0.0,
            kind_of: KindOfMask::from_bits(u32::MAX as u128),
            forbidden_kind_of: KindOfMask::EMPTY,
            start_healing_delay: 0,
            affects_whole_player: false,
            skip_self_for_healing: false,
            upgrade: UpgradeMuxData::default(),
        }
    }
}

impl AutoHealData {
    pub(crate) fn parse(block: &Block, d: &mut Diag) -> Self {
        let mut p = AutoHealData::default();
        fields(block, d, "AutoHealBehavior", |k, a| {
            match k {
                "StartsActive" => p.starts_active = a.boolean()?,
                "SingleBurst" => p.single_burst = a.boolean()?,
                "HealingAmount" => p.healing_amount = a.int()? as f32,
                "HealingDelay" => p.healing_delay = a.duration_frames()?,
                "Radius" => p.radius = a.real()?,
                "KindOf" => p.kind_of = p.kind_of.parse(a.rest())?,
                "ForbiddenKindOf" => p.forbidden_kind_of = p.forbidden_kind_of.parse(a.rest())?,
                "StartHealingDelay" => p.start_healing_delay = a.duration_frames()?,
                "AffectsWholePlayer" => p.affects_whole_player = a.boolean()?,
                "SkipSelfForHealing" => p.skip_self_for_healing = a.boolean()?,
                // Client particle templates, not simulated.
                "RadiusParticleSystemName" | "UnitHealPulseParticleSystemName" => {
                    let _ = a.next()?;
                }
                _ => return p.upgrade.field(k, a),
            }
            Ok(true)
        });
        p
    }
}

// ---------------------------------------------------------------------------------------------
// FlammableUpdate

/// `FlammableUpdateModuleData`.
#[derive(Debug, Clone, PartialEq)]
pub struct FlammableData {
    /// Frames.
    pub burned_delay: u32,
    pub aflame_duration: u32,
    pub aflame_damage_delay: u32,
    pub aflame_damage_amount: f32,
    pub burning_sound_name: Option<String>,
    pub flame_damage_limit: f32,
    pub flame_damage_expiration: u32,
}

impl Default for FlammableData {
    fn default() -> Self {
        FlammableData {
            burned_delay: 0,
            aflame_duration: 0,
            aflame_damage_delay: 0,
            aflame_damage_amount: 0.0,
            burning_sound_name: None,
            flame_damage_limit: 20.0,
            flame_damage_expiration: crate::value::LOGICFRAMES_PER_SECOND * 2,
        }
    }
}

impl FlammableData {
    pub(crate) fn parse(block: &Block, d: &mut Diag) -> Self {
        let mut p = FlammableData::default();
        fields(block, d, "FlammableUpdate", |k, a| {
            match k {
                "BurnedDelay" => p.burned_delay = a.duration_frames()?,
                "AflameDuration" => p.aflame_duration = a.duration_frames()?,
                "AflameDamageDelay" => p.aflame_damage_delay = a.duration_frames()?,
                "AflameDamageAmount" => p.aflame_damage_amount = a.int()? as f32,
                "BurningSoundName" => p.burning_sound_name = Some(a.next()?.to_string()),
                "FlameDamageLimit" => p.flame_damage_limit = a.real()?,
                "FlameDamageExpiration" => p.flame_damage_expiration = a.duration_frames()?,
                _ => return Ok(false),
            }
            Ok(true)
        });
        p
    }
}

// ---------------------------------------------------------------------------------------------
// PoisonedBehavior

/// `PoisonedBehaviorModuleData`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PoisonedData {
    /// Frames between poison ticks.
    pub poison_damage_interval: u32,
    /// Frames the poison lasts.
    pub poison_duration: u32,
}

impl PoisonedData {
    pub(crate) fn parse(block: &Block, d: &mut Diag) -> Self {
        let mut p = PoisonedData::default();
        fields(block, d, "PoisonedBehavior", |k, a| {
            match k {
                "PoisonDamageInterval" => p.poison_damage_interval = a.duration_frames()?,
                "PoisonDuration" => p.poison_duration = a.duration_frames()?,
                _ => return Ok(false),
            }
            Ok(true)
        });
        p
    }
}

// ---------------------------------------------------------------------------------------------
// FireWeaponWhenDeadBehavior

/// `FireWeaponWhenDeadBehaviorModuleData`.
#[derive(Debug, Clone, PartialEq)]
pub struct FireWeaponWhenDeadData {
    pub starts_active: bool,
    pub death_weapon: Option<Ref<WeaponId>>,
    pub upgrade: UpgradeMuxData,
    pub die: crate::die::DieMuxData,
}

impl FireWeaponWhenDeadData {
    pub(crate) fn parse(block: &Block, d: &mut Diag) -> Self {
        let mut p = FireWeaponWhenDeadData {
            starts_active: false,
            death_weapon: None,
            upgrade: UpgradeMuxData::default(),
            die: crate::die::DieMuxData::default(),
        };
        fields(block, d, "FireWeaponWhenDeadBehavior", |k, a| {
            match k {
                "StartsActive" => p.starts_active = a.boolean()?,
                "DeathWeapon" => p.death_weapon = Some(Ref::named(a.next()?.to_string())),
                "DeathTypes" => p.die.death_types = crate::value::parse_flag_list_all(a.rest())?,
                "VeterancyLevels" => p.die.veterancy_levels = crate::value::parse_veterancy_level_flags(a.rest())?,
                "ExemptStatus" => p.die.exempt_status = p.die.exempt_status.parse(a.rest())?,
                "RequiredStatus" => p.die.required_status = p.die.required_status.parse(a.rest())?,
                _ => return p.upgrade.field(k, a),
            }
            Ok(true)
        });
        p
    }
}

// ---------------------------------------------------------------------------------------------
// LifetimeUpdate

/// `LifetimeUpdateModuleData`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LifetimeData {
    /// Frames.
    pub min_lifetime: u32,
    pub max_lifetime: u32,
}

impl LifetimeData {
    pub(crate) fn parse(block: &Block, d: &mut Diag) -> Self {
        let mut p = LifetimeData::default();
        fields(block, d, "LifetimeUpdate", |k, a| {
            match k {
                "MinLifetime" => p.min_lifetime = a.duration_frames()?,
                "MaxLifetime" => p.max_lifetime = a.duration_frames()?,
                _ => return Ok(false),
            }
            Ok(true)
        });
        p
    }
}

// ---------------------------------------------------------------------------------------------
// StealthUpdate

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

/// `StealthUpdateModuleData`.
#[derive(Debug, Clone, PartialEq)]
pub struct StealthData {
    /// Frames before stealth engages (`u32::MAX` = never).
    pub stealth_delay: u32,
    pub move_threshold_speed: f32,
    pub forbidden_conditions: u32,
    pub required_status: Mask<ObjectStatus>,
    pub forbidden_status: Mask<ObjectStatus>,
    pub innate_stealth: bool,
    pub granted_by_special_power: bool,
    pub disguises_as_team: bool,
    pub use_rider_stealth: bool,
    pub reveal_distance_from_target: f32,
    pub order_idle_enemies_to_attack_on_reveal: bool,
    pub disguise_transition_time: u32,
    pub disguise_reveal_transition_time: u32,
}

impl Default for StealthData {
    fn default() -> Self {
        StealthData {
            stealth_delay: u32::MAX,
            move_threshold_speed: 0.0,
            forbidden_conditions: 0,
            required_status: Mask::EMPTY,
            forbidden_status: Mask::EMPTY,
            innate_stealth: true,
            granted_by_special_power: false,
            disguises_as_team: false,
            use_rider_stealth: false,
            reveal_distance_from_target: 0.0,
            order_idle_enemies_to_attack_on_reveal: false,
            disguise_transition_time: 0,
            disguise_reveal_transition_time: 0,
        }
    }
}

/// `TheStealthLevelNames` bitstring32 (`parseBitString32`).
fn parse_stealth_forbidden(toks: &[&str]) -> Result<u32, String> {
    use stealth_forbidden as f;
    let mut bits = 0;
    for t in toks {
        bits |= match t.to_ascii_uppercase().as_str() {
            "NONE" => 0,
            "ATTACKING" => f::ATTACKING,
            "MOVING" => f::MOVING,
            "USING_ABILITY" => f::USING_ABILITY,
            "FIRING_PRIMARY" => f::FIRING_PRIMARY,
            "FIRING_SECONDARY" => f::FIRING_SECONDARY,
            "FIRING_TERTIARY" => f::FIRING_TERTIARY,
            "NO_BLACK_MARKET" => f::NO_BLACK_MARKET,
            "TAKING_DAMAGE" => f::TAKING_DAMAGE,
            "RIDERS_ATTACKING" => f::RIDERS_ATTACKING,
            "FIRING_WEAPON" => f::FIRING_WEAPON,
            other => return Err(format!("unknown stealth condition `{other}`")),
        };
    }
    Ok(bits)
}

impl StealthData {
    pub(crate) fn parse(block: &Block, d: &mut Diag) -> Self {
        let mut p = StealthData::default();
        fields(block, d, "StealthUpdate", |k, a| {
            match k {
                "StealthDelay" => p.stealth_delay = a.duration_frames()?,
                "MoveThresholdSpeed" => p.move_threshold_speed = a.velocity()?,
                "StealthForbiddenConditions" => p.forbidden_conditions = parse_stealth_forbidden(a.rest())?,
                "RequiredStatus" => p.required_status = p.required_status.parse(a.rest())?,
                "ForbiddenStatus" => p.forbidden_status = p.forbidden_status.parse(a.rest())?,
                "InnateStealth" => p.innate_stealth = a.boolean()?,
                "GrantedBySpecialPower" => p.granted_by_special_power = a.boolean()?,
                "DisguisesAsTeam" => p.disguises_as_team = a.boolean()?,
                "UseRiderStealth" => p.use_rider_stealth = a.boolean()?,
                "RevealDistanceFromTarget" => p.reveal_distance_from_target = a.real()?,
                "OrderIdleEnemiesToAttackMeUponReveal" => p.order_idle_enemies_to_attack_on_reveal = a.boolean()?,
                "DisguiseTransitionTime" => p.disguise_transition_time = a.duration_frames()?,
                "DisguiseRevealTransitionTime" => p.disguise_reveal_transition_time = a.duration_frames()?,
                // Client-only / not simulated yet.
                "HintDetectableConditions"
                | "FriendlyOpacityMin"
                | "FriendlyOpacityMax"
                | "PulseFrequency"
                | "DisguiseFX"
                | "DisguiseRevealFX"
                | "EnemyDetectionEvaEvent"
                | "OwnDetectionEvaEvent"
                | "BlackMarketCheckDelay" => {
                    let _ = a.rest();
                }
                _ => return Ok(false),
            }
            Ok(true)
        });
        p
    }
}

// ---------------------------------------------------------------------------------------------
// StealthDetectorUpdate

/// `StealthDetectorUpdateModuleData`.
#[derive(Debug, Clone, PartialEq)]
pub struct StealthDetectorData {
    /// Frames between detection sweeps.
    pub detection_rate: u32,
    pub detection_range: f32,
    pub initially_disabled: bool,
    pub extra_required_kind_of: KindOfMask,
    pub extra_forbidden_kind_of: KindOfMask,
    pub can_detect_while_garrisoned: bool,
    pub can_detect_while_contained: bool,
}

impl Default for StealthDetectorData {
    fn default() -> Self {
        StealthDetectorData {
            detection_rate: 1,
            detection_range: 0.0,
            initially_disabled: false,
            extra_required_kind_of: KindOfMask::EMPTY,
            extra_forbidden_kind_of: KindOfMask::EMPTY,
            can_detect_while_garrisoned: false,
            can_detect_while_contained: false,
        }
    }
}

impl StealthDetectorData {
    pub(crate) fn parse(block: &Block, d: &mut Diag) -> Self {
        let mut p = StealthDetectorData::default();
        fields(block, d, "StealthDetectorUpdate", |k, a| {
            match k {
                "DetectionRate" => p.detection_rate = a.duration_frames()?.max(1),
                "DetectionRange" => p.detection_range = a.real()?,
                "InitiallyDisabled" => p.initially_disabled = a.boolean()?,
                "ExtraRequiredKindOf" => p.extra_required_kind_of = p.extra_required_kind_of.parse(a.rest())?,
                "ExtraForbiddenKindOf" => p.extra_forbidden_kind_of = p.extra_forbidden_kind_of.parse(a.rest())?,
                "CanDetectWhileGarrisoned" => p.can_detect_while_garrisoned = a.boolean()?,
                "CanDetectWhileContained" => p.can_detect_while_contained = a.boolean()?,
                // Client audio / particle templates, not simulated.
                "PingSound"
                | "LoudPingSound"
                | "IRBeaconParticleSysName"
                | "IRParticleSysName"
                | "IRBrightParticleSysName"
                | "IRGridParticleSysName"
                | "IRParticleSysBone" => {
                    let _ = a.rest();
                }
                _ => return Ok(false),
            }
            Ok(true)
        });
        p
    }
}
