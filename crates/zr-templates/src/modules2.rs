//! Round-2 module families: crush (`SquishCollide`, `CrushDie`), crates (`CreateCrateDie`,
//! `CrateCollide`), `ToppleUpdate`, spawners (`SpawnBehavior`, `SlavedUpdate`), `HordeUpdate`,
//! income/base modules (`AutoDepositUpdate`, `BaseRegenerateUpdate`, `PropagandaTowerBehavior`,
//! `FireWeaponUpdate`, `RepairDockUpdate`, `VeterancyGainCreate`) and `EjectPilotDie`.
//!
//! Field tables ported from each class's `buildFieldParse` (GeneralsMD).

use zr_ini::Block;

use crate::behavior::fields;
use crate::diag::Diag;
use crate::flags::{KindOfMask, Mask};
use crate::names::{LocomotorSetType, ObjectStatus};
use crate::registry::{OclId, Ref, WeaponId};
use crate::value::Args;

// ---------------------------------------------------------------------------------------------
// SquishCollide (marker; `MAKE_STANDARD_MODULE_MACRO`, no fields)

/// `SquishCollide`: a marker module; the actual crush uses `CrusherLevel`/`CrushableLevel`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SquishCollideData;

impl SquishCollideData {
    pub(crate) fn parse(_block: &Block, _d: &mut Diag) -> Self {
        SquishCollideData
    }
}

// ---------------------------------------------------------------------------------------------
// CrushDie (a die module; fields consumed into DieKind::Crush)

/// `CrushDieModuleData` (sound percentages; client-only, kept for completeness).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CrushDieData {
    pub total_crush_sound_percent: i32,
    pub back_end_crush_sound_percent: i32,
    pub front_end_crush_sound_percent: i32,
}

// ---------------------------------------------------------------------------------------------
// CreateCrateDie (a die module)

/// `CreateCrateDieModuleData`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CreateCrateDieData {
    pub crate_names: Vec<String>,
}

// ---------------------------------------------------------------------------------------------
// EjectPilotDie (a die module)

/// `EjectPilotDieModuleData`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct EjectPilotDieData {
    pub air_creation_list: Option<Ref<OclId>>,
    pub ground_creation_list: Option<Ref<OclId>>,
    pub invulnerable_time: u32,
}

// ---------------------------------------------------------------------------------------------
// CrateCollide

/// The subclass-specific effect of a `CrateCollide`.
#[derive(Debug, Clone, PartialEq)]
pub enum CrateKind {
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
    /// Other subclasses: typed, not simulated.
    Other,
}

/// `CrateCollideModuleData` plus the subclass effect.
#[derive(Debug, Clone, PartialEq)]
pub struct CrateCollideData {
    pub required_kind_of: KindOfMask,
    pub forbidden_kind_of: KindOfMask,
    pub forbid_owner_player: bool,
    pub building_pickup: bool,
    pub human_only: bool,
    pub allow_multi_pickup: bool,
    pub pickup_science: Option<String>,
    pub kind: CrateKind,
}

impl CrateCollideData {
    pub(crate) fn parse(class: &str, block: &Block, d: &mut Diag) -> Self {
        let mut p = CrateCollideData {
            required_kind_of: KindOfMask::EMPTY,
            forbidden_kind_of: KindOfMask::EMPTY,
            forbid_owner_player: false,
            building_pickup: false,
            human_only: false,
            allow_multi_pickup: true,
            pickup_science: None,
            kind: match class {
                "MoneyCrateCollide" => CrateKind::Money { money_provided: 0, upgrade_boost: Vec::new() },
                "HealCrateCollide" => CrateKind::Heal,
                "VeterancyCrateCollide" => CrateKind::Veterancy { effect_range: 0, adds_owner_veterancy: false, is_pilot: false },
                "UnitCrateCollide" => CrateKind::Unit { unit_count: 0, unit_name: String::new() },
                "SalvageCrateCollide" => {
                    CrateKind::Salvage { weapon_chance: 1.0, level_chance: 0.25, money_chance: 0.75, min_money: 25, max_money: 75 }
                }
                "ShroudCrateCollide" => CrateKind::Shroud,
                "ConvertToCarBombCrateCollide" => CrateKind::ConvertToCarBomb,
                "ConvertToHijackedVehicleCrateCollide" => CrateKind::ConvertToHijackedVehicle,
                "SabotageCommandCenterCrateCollide" => CrateKind::SabotageCommandCenter,
                "SabotageFakeBuildingCrateCollide" => CrateKind::SabotageFakeBuilding,
                "SabotageInternetCenterCrateCollide" => CrateKind::SabotageInternetCenter { sabotage_frames: 0 },
                "SabotageMilitaryFactoryCrateCollide" => CrateKind::SabotageMilitaryFactory { sabotage_frames: 0 },
                "SabotagePowerPlantCrateCollide" => CrateKind::SabotagePowerPlant { power_sabotage_frames: 0 },
                "SabotageSuperweaponCrateCollide" => CrateKind::SabotageSuperweapon,
                "SabotageSupplyCenterCrateCollide" => CrateKind::SabotageSupplyCenter { steal_cash_amount: 0 },
                "SabotageSupplyDropzoneCrateCollide" => CrateKind::SabotageSupplyDropzone { steal_cash_amount: 0 },
                _ => CrateKind::Other,
            },
        };
        fields(block, d, "CrateCollide", |k, a| {
            match k {
                "RequiredKindOf" => p.required_kind_of = p.required_kind_of.parse(a.rest())?,
                "ForbiddenKindOf" => p.forbidden_kind_of = p.forbidden_kind_of.parse(a.rest())?,
                "ForbidOwnerPlayer" => p.forbid_owner_player = a.boolean()?,
                "BuildingPickup" => p.building_pickup = a.boolean()?,
                "HumanOnly" => p.human_only = a.boolean()?,
                "AllowMultiPickup" => p.allow_multi_pickup = a.boolean()?,
                "PickupScience" => {
                    let n = a.next()?;
                    p.pickup_science = if n.eq_ignore_ascii_case("None") { None } else { Some(n.to_string()) };
                }
                "MoneyProvided" => {
                    if let CrateKind::Money { money_provided, .. } = &mut p.kind {
                        *money_provided = a.uint()?;
                    } else {
                        let _ = a.uint()?;
                    }
                }
                "UpgradedBoost" => {
                    let toks = a.rest();
                    let mut name = None;
                    let mut boost = 0;
                    let mut i = 0;
                    while i < toks.len() {
                        match toks[i].to_ascii_lowercase().as_str() {
                            "upgradetype" => {
                                name = toks.get(i + 1).map(|s| s.to_string());
                                i += 2;
                            }
                            "boost" => {
                                boost = toks.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(0);
                                i += 2;
                            }
                            _ => i += 1,
                        }
                    }
                    if let CrateKind::Money { upgrade_boost, .. } = &mut p.kind {
                        if let Some(n) = name {
                            upgrade_boost.push((n, boost));
                        }
                    }
                }
                "SabotageDuration" => {
                    let v = a.duration_frames()?;
                    match &mut p.kind {
                        CrateKind::SabotageInternetCenter { sabotage_frames } => *sabotage_frames = v,
                        CrateKind::SabotageMilitaryFactory { sabotage_frames } => *sabotage_frames = v,
                        _ => {}
                    }
                }
                "SabotagePowerDuration" => {
                    let v = a.duration_frames()?;
                    if let CrateKind::SabotagePowerPlant { power_sabotage_frames } = &mut p.kind {
                        *power_sabotage_frames = v;
                    }
                }
                "StealCashAmount" => {
                    let v = a.uint()?;
                    match &mut p.kind {
                        CrateKind::SabotageSupplyCenter { steal_cash_amount } => *steal_cash_amount = v,
                        CrateKind::SabotageSupplyDropzone { steal_cash_amount } => *steal_cash_amount = v,
                        _ => {}
                    }
                }
                "EffectRange" => {
                    if let CrateKind::Veterancy { effect_range, .. } = &mut p.kind {
                        *effect_range = a.uint()?;
                    } else {
                        let _ = a.uint()?;
                    }
                }
                "AddsOwnerVeterancy" => {
                    if let CrateKind::Veterancy { adds_owner_veterancy, .. } = &mut p.kind {
                        *adds_owner_veterancy = a.boolean()?;
                    } else {
                        let _ = a.boolean()?;
                    }
                }
                "IsPilot" => {
                    if let CrateKind::Veterancy { is_pilot, .. } = &mut p.kind {
                        *is_pilot = a.boolean()?;
                    } else {
                        let _ = a.boolean()?;
                    }
                }
                "UnitCount" => {
                    if let CrateKind::Unit { unit_count, .. } = &mut p.kind {
                        *unit_count = a.uint()?;
                    } else {
                        let _ = a.uint()?;
                    }
                }
                "UnitName" => {
                    if let CrateKind::Unit { unit_name, .. } = &mut p.kind {
                        *unit_name = a.next()?.to_string();
                    } else {
                        let _ = a.next()?;
                    }
                }
                "WeaponChance" => {
                    if let CrateKind::Salvage { weapon_chance, .. } = &mut p.kind {
                        *weapon_chance = a.percent()?;
                    } else {
                        let _ = a.percent()?;
                    }
                }
                "LevelChance" => {
                    if let CrateKind::Salvage { level_chance, .. } = &mut p.kind {
                        *level_chance = a.percent()?;
                    } else {
                        let _ = a.percent()?;
                    }
                }
                "MoneyChance" => {
                    if let CrateKind::Salvage { money_chance, .. } = &mut p.kind {
                        *money_chance = a.percent()?;
                    } else {
                        let _ = a.percent()?;
                    }
                }
                "MinMoney" => {
                    if let CrateKind::Salvage { min_money, .. } = &mut p.kind {
                        *min_money = a.int()?;
                    } else {
                        let _ = a.int()?;
                    }
                }
                "MaxMoney" => {
                    if let CrateKind::Salvage { max_money, .. } = &mut p.kind {
                        *max_money = a.int()?;
                    } else {
                        let _ = a.int()?;
                    }
                }
                // Base/subclass fields not simulated (science, FX, sabotage durations, ...).
                _ => {
                    let _ = a.rest();
                }
            }
            Ok(true)
        });
        p
    }
}

// ---------------------------------------------------------------------------------------------
// FireWeaponCollide

/// `FireWeaponCollideModuleData` (shoot the object that collides with me).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FireWeaponCollideData {
    pub weapon: Option<Ref<WeaponId>>,
    pub fire_once: bool,
    /// `RequiredStatus` / `ForbiddenStatus` (`ObjectStatusMaskType::parseFromINI`).
    pub required_status: Mask<ObjectStatus>,
    pub forbidden_status: Mask<ObjectStatus>,
}

impl FireWeaponCollideData {
    pub(crate) fn parse(block: &Block, d: &mut Diag) -> Self {
        let mut p = FireWeaponCollideData::default();
        fields(block, d, "FireWeaponCollide", |k, a| {
            match k {
                "CollideWeapon" => p.weapon = Some(Ref::named(a.next()?.to_string())),
                "FireOnce" => p.fire_once = a.boolean()?,
                "RequiredStatus" => p.required_status = p.required_status.parse(a.rest())?,
                "ForbiddenStatus" => p.forbidden_status = p.forbidden_status.parse(a.rest())?,
                _ => return Ok(false),
            }
            Ok(true)
        });
        p
    }
}

// ---------------------------------------------------------------------------------------------
// VeterancyGainCreate

/// `VeterancyGainCreateModuleData`.
#[derive(Debug, Clone, PartialEq)]
pub struct VeterancyGainCreateData {
    /// `StartingLevel` (`TheVeterancyNames`; default `REGULAR`).
    pub starting_level: crate::names::VeterancyLevel,
    /// `ScienceRequired` name (`None` = none).
    pub science_required: Option<String>,
}

impl Default for VeterancyGainCreateData {
    fn default() -> Self {
        VeterancyGainCreateData { starting_level: crate::names::VeterancyLevel::Regular, science_required: None }
    }
}

impl VeterancyGainCreateData {
    pub(crate) fn parse(block: &Block, d: &mut Diag) -> Self {
        let mut p = VeterancyGainCreateData::default();
        fields(block, d, "VeterancyGainCreate", |k, a| {
            match k {
                "StartingLevel" => p.starting_level = a.index()?,
                "ScienceRequired" => {
                    let n = a.next()?;
                    p.science_required = if n.eq_ignore_ascii_case("None") { None } else { Some(n.to_string()) };
                }
                _ => return Ok(false),
            }
            Ok(true)
        });
        p
    }
}

// ---------------------------------------------------------------------------------------------
// ToppleUpdate

/// `ToppleUpdateModuleData`.
#[derive(Debug, Clone, PartialEq)]
pub struct ToppleData {
    pub stump_name: Option<String>,
    pub kill_when_start_toppling: bool,
    pub kill_when_finished_toppling: bool,
    pub kill_stump_when_toppled: bool,
    pub topple_left_or_right_only: bool,
    pub reorient_toppled_rubble: bool,
    pub initial_velocity_percent: f32,
    pub initial_accel_percent: f32,
    pub bounce_velocity_percent: f32,
}

impl Default for ToppleData {
    fn default() -> Self {
        ToppleData {
            stump_name: None,
            kill_when_start_toppling: false,
            kill_when_finished_toppling: true,
            kill_stump_when_toppled: false,
            topple_left_or_right_only: false,
            reorient_toppled_rubble: false,
            initial_velocity_percent: 0.2,
            initial_accel_percent: 0.01,
            bounce_velocity_percent: 0.3,
        }
    }
}

impl ToppleData {
    pub(crate) fn parse(block: &Block, d: &mut Diag) -> Self {
        let mut p = ToppleData::default();
        fields(block, d, "ToppleUpdate", |k, a| {
            match k {
                "StumpName" => p.stump_name = Some(a.next()?.to_string()),
                "KillWhenStartToppling" => p.kill_when_start_toppling = a.boolean()?,
                "KillWhenFinishedToppling" => p.kill_when_finished_toppling = a.boolean()?,
                "KillStumpWhenToppled" => p.kill_stump_when_toppled = a.boolean()?,
                "ToppleLeftOrRightOnly" => p.topple_left_or_right_only = a.boolean()?,
                "ReorientToppledRubble" => p.reorient_toppled_rubble = a.boolean()?,
                "InitialVelocityPercent" => p.initial_velocity_percent = a.percent()?,
                "InitialAccelPercent" => p.initial_accel_percent = a.percent()?,
                "BounceVelocityPercent" => p.bounce_velocity_percent = a.percent()?,
                // FX lists are client-only.
                "ToppleFX" | "BounceFX" => {
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
// SpawnBehavior

/// `SpawnBehaviorModuleData`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SpawnData {
    pub spawn_number: i32,
    pub spawn_replace_delay: u32,
    pub one_shot: bool,
    pub can_reclaim_orphans: bool,
    pub aggregate_health: bool,
    pub exit_by_budding: bool,
    pub spawn_template_names: Vec<String>,
    pub spawned_require_spawner: bool,
    pub initial_burst: i32,
    pub slaves_have_free_will: bool,
    pub die: crate::die::DieMuxData,
}

impl SpawnData {
    pub(crate) fn parse(block: &Block, d: &mut Diag) -> Self {
        let mut p = SpawnData::default();
        fields(block, d, "SpawnBehavior", |k, a| {
            match k {
                "SpawnNumber" => p.spawn_number = a.int()?,
                "SpawnReplaceDelay" => p.spawn_replace_delay = a.duration_frames()?,
                "OneShot" => p.one_shot = a.boolean()?,
                "CanReclaimOrphans" => p.can_reclaim_orphans = a.boolean()?,
                "AggregateHealth" => p.aggregate_health = a.boolean()?,
                "ExitByBudding" => p.exit_by_budding = a.boolean()?,
                "SpawnTemplateName" => p.spawn_template_names = a.rest().iter().map(|s| s.to_string()).collect(),
                "SpawnedRequireSpawner" => p.spawned_require_spawner = a.boolean()?,
                "InitialBurst" => p.initial_burst = a.int()?,
                "SlavesHaveFreeWill" => p.slaves_have_free_will = a.boolean()?,
                "DeathTypes" => p.die.death_types = crate::value::parse_flag_list_all(a.rest())?,
                "VeterancyLevels" => p.die.veterancy_levels = crate::value::parse_veterancy_level_flags(a.rest())?,
                "ExemptStatus" => p.die.exempt_status = p.die.exempt_status.parse(a.rest())?,
                "RequiredStatus" => p.die.required_status = p.die.required_status.parse(a.rest())?,
                _ => {
                    let _ = a.rest();
                }
            }
            Ok(true)
        });
        p
    }
}

// ---------------------------------------------------------------------------------------------
// SlavedUpdate

/// `SlavedUpdateModuleData`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SlavedData {
    pub guard_max_range: i32,
    pub guard_wander_range: i32,
    pub attack_range: i32,
    pub attack_wander_range: i32,
    pub scout_range: i32,
    pub scout_wander_range: i32,
    pub repair_range: i32,
    pub dist_to_target_to_grant_range_bonus: i32,
    pub repair_rate_per_second: f32,
    pub repair_when_below_health_percent: i32,
    pub stay_on_same_layer_as_master: bool,
}

impl SlavedData {
    pub(crate) fn parse(block: &Block, d: &mut Diag) -> Self {
        let mut p = SlavedData::default();
        fields(block, d, "SlavedUpdate", |k, a| {
            match k {
                "GuardMaxRange" => p.guard_max_range = a.int()?,
                "GuardWanderRange" => p.guard_wander_range = a.int()?,
                "AttackRange" => p.attack_range = a.int()?,
                "AttackWanderRange" => p.attack_wander_range = a.int()?,
                "ScoutRange" => p.scout_range = a.int()?,
                "ScoutWanderRange" => p.scout_wander_range = a.int()?,
                "RepairRange" => p.repair_range = a.int()?,
                "DistToTargetToGrantRangeBonus" => p.dist_to_target_to_grant_range_bonus = a.int()?,
                "RepairRatePerSecond" => p.repair_rate_per_second = a.real()?,
                "RepairWhenBelowHealth%" => p.repair_when_below_health_percent = a.int()?,
                "StayOnSameLayerAsMaster" => p.stay_on_same_layer_as_master = a.boolean()?,
                _ => {
                    let _ = a.rest();
                }
            }
            Ok(true)
        });
        p
    }
}

// ---------------------------------------------------------------------------------------------
// HordeUpdate

/// `HordeUpdateModuleData`.
#[derive(Debug, Clone, PartialEq)]
pub struct HordeData {
    pub update_rate: u32,
    pub kind_of: KindOfMask,
    pub count: i32,
    pub radius: f32,
    pub rub_off_radius: f32,
    pub allies_only: bool,
    pub exact_match: bool,
    pub allowed_nationalism: bool,
}

impl Default for HordeData {
    fn default() -> Self {
        HordeData {
            update_rate: crate::value::LOGICFRAMES_PER_SECOND,
            kind_of: KindOfMask::EMPTY,
            count: 0,
            radius: 0.0,
            rub_off_radius: 20.0,
            allies_only: true,
            exact_match: false,
            allowed_nationalism: true,
        }
    }
}

impl HordeData {
    pub(crate) fn parse(block: &Block, d: &mut Diag) -> Self {
        let mut p = HordeData::default();
        fields(block, d, "HordeUpdate", |k, a| {
            match k {
                "UpdateRate" => p.update_rate = a.duration_frames()?,
                "KindOf" => p.kind_of = p.kind_of.parse(a.rest())?,
                "Count" => p.count = a.int()?,
                "Radius" => p.radius = a.real()?,
                "RubOffRadius" => p.rub_off_radius = a.real()?,
                "AlliesOnly" => p.allies_only = a.boolean()?,
                "ExactMatch" => p.exact_match = a.boolean()?,
                "AllowedNationalism" => p.allowed_nationalism = a.boolean()?,
                _ => {
                    let _ = a.rest();
                }
            }
            Ok(true)
        });
        p
    }
}

// ---------------------------------------------------------------------------------------------
// AutoDepositUpdate

/// `AutoDepositUpdateModuleData`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AutoDepositData {
    pub deposit_timing: u32,
    pub deposit_amount: i32,
    pub initial_capture_bonus: i32,
    pub actual_money: bool,
    /// `UpgradedBoost` pairs (upgrade name, boost).
    pub upgraded_boost: Vec<(String, i32)>,
}

impl AutoDepositData {
    pub(crate) fn parse(block: &Block, d: &mut Diag) -> Self {
        let mut p = AutoDepositData { actual_money: true, ..Default::default() };
        let mut pending: Option<String> = None;
        fields(block, d, "AutoDepositUpdate", |k, a| {
            match k {
                "DepositTiming" => p.deposit_timing = a.duration_frames()?,
                "DepositAmount" => p.deposit_amount = a.int()?,
                "InitialCaptureBonus" => p.initial_capture_bonus = a.int()?,
                "ActualMoney" => p.actual_money = a.boolean()?,
                "UpgradedBoost" => {
                    // `UpgradeType: <name> Boost: <int>` on one line.
                    let toks = a.rest();
                    let mut name = None;
                    let mut boost = 0;
                    let mut i = 0;
                    while i < toks.len() {
                        match toks[i].to_ascii_lowercase().as_str() {
                            "upgradetype" => {
                                name = toks.get(i + 1).map(|s| s.to_string());
                                i += 2;
                            }
                            "boost" => {
                                boost = toks.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(0);
                                i += 2;
                            }
                            _ => i += 1,
                        }
                    }
                    if let Some(n) = name {
                        p.upgraded_boost.push((n, boost));
                    }
                    pending = None;
                }
                _ => return Ok(false),
            }
            Ok(true)
        });
        let _ = pending;
        p
    }
}

// ---------------------------------------------------------------------------------------------
// BaseRegenerateUpdate (no fields)

/// `BaseRegenerateUpdate` (tuning comes from `GameData`: `BaseRegenHealthPercentPerSecond`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BaseRegenerateData;

impl BaseRegenerateData {
    pub(crate) fn parse(_block: &Block, _d: &mut Diag) -> Self {
        BaseRegenerateData
    }
}

// ---------------------------------------------------------------------------------------------
// PropagandaTowerBehavior

/// `PropagandaTowerBehaviorModuleData`.
#[derive(Debug, Clone, PartialEq)]
pub struct PropagandaData {
    pub radius: f32,
    pub delay_between_updates: u32,
    pub heal_percent_each_second: f32,
    pub upgraded_heal_percent_each_second: f32,
    pub upgrade_required: Option<String>,
    pub affects_self: bool,
}

impl Default for PropagandaData {
    fn default() -> Self {
        PropagandaData {
            radius: 1.0,
            delay_between_updates: 100,
            heal_percent_each_second: 0.01,
            upgraded_heal_percent_each_second: 0.02,
            upgrade_required: None,
            affects_self: false,
        }
    }
}

impl PropagandaData {
    pub(crate) fn parse(block: &Block, d: &mut Diag) -> Self {
        let mut p = PropagandaData::default();
        fields(block, d, "PropagandaTowerBehavior", |k, a| {
            match k {
                "Radius" => p.radius = a.real()?,
                "DelayBetweenUpdates" => p.delay_between_updates = a.duration_frames()?,
                "HealPercentEachSecond" => p.heal_percent_each_second = a.percent()?,
                "UpgradedHealPercentEachSecond" => p.upgraded_heal_percent_each_second = a.percent()?,
                "UpgradeRequired" => {
                    let n = a.next()?;
                    p.upgrade_required = if n.eq_ignore_ascii_case("None") { None } else { Some(n.to_string()) };
                }
                "AffectsSelf" => p.affects_self = a.boolean()?,
                "PulseFX" | "UpgradedPulseFX" => {
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
// FireWeaponUpdate

/// `FireWeaponUpdateModuleData`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FireWeaponData {
    pub weapon: Option<Ref<crate::registry::WeaponId>>,
    pub initial_delay: u32,
    pub exclusive_weapon_delay: u32,
}

impl FireWeaponData {
    pub(crate) fn parse(block: &Block, d: &mut Diag) -> Self {
        let mut p = FireWeaponData::default();
        fields(block, d, "FireWeaponUpdate", |k, a| {
            match k {
                "Weapon" => p.weapon = Some(Ref::named(a.next()?.to_string())),
                "InitialDelay" => p.initial_delay = a.duration_frames()?,
                "ExclusiveWeaponDelay" => p.exclusive_weapon_delay = a.duration_frames()?,
                _ => return Ok(false),
            }
            Ok(true)
        });
        p
    }
}

// ---------------------------------------------------------------------------------------------
// RepairDockUpdate

/// `RepairDockUpdateModuleData` (extends `DockUpdate`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RepairDockData {
    pub frames_for_full_heal: f32,
    pub number_approach_positions: i32,
    /// `AllowsPassthrough` (`DockUpdateModuleData`, default TRUE).
    pub allows_passthrough: bool,
}

impl RepairDockData {
    pub(crate) fn parse(block: &Block, d: &mut Diag) -> Self {
        let mut p = RepairDockData { frames_for_full_heal: 1.0, allows_passthrough: true, ..Default::default() };
        fields(block, d, "RepairDockUpdate", |k, a| {
            match k {
                "TimeForFullHeal" => p.frames_for_full_heal = a.duration_real()?,
                "NumberApproachPositions" => p.number_approach_positions = a.int()?,
                "AllowsPassthrough" => p.allows_passthrough = a.boolean()?,
                _ => {
                    let _ = a.rest();
                }
            }
            Ok(true)
        });
        p
    }
}

/// `OverchargeBehavior` + `PowerPlantUpdate`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OverchargeData {
    pub health_percent_to_drain_per_second: f32,
    pub not_allowed_when_health_below_percent: f32,
    pub rods_extend_time: u32,
}

impl OverchargeData {
    pub(crate) fn parse(block: &Block, d: &mut Diag) -> Self {
        let mut p = OverchargeData::default();
        fields(block, d, "OverchargeBehavior", |k, a| {
            match k {
                "HealthPercentToDrainPerSecond" => p.health_percent_to_drain_per_second = a.percent()?,
                "NotAllowedWhenHealthBelowPercent" => p.not_allowed_when_health_below_percent = a.percent()?,
                "RodsExtendTime" => p.rods_extend_time = a.duration_frames()?,
                _ => return Ok(false),
            }
            Ok(true)
        });
        p
    }
}

/// `MinefieldBehavior`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MinefieldData {
    pub detonation_weapon: Option<Ref<crate::registry::WeaponId>>,
    pub regenerates: bool,
    pub workers_detonate: bool,
    pub num_virtual_mines: u32,
    pub creation_list: Option<Ref<OclId>>,
}

impl MinefieldData {
    pub(crate) fn parse(block: &Block, d: &mut Diag) -> Self {
        let mut p = MinefieldData { num_virtual_mines: 1, ..Default::default() };
        fields(block, d, "MinefieldBehavior", |k, a| {
            match k {
                "DetonationWeapon" => p.detonation_weapon = Some(Ref::named(a.next()?.to_string())),
                "Regenerates" => p.regenerates = a.boolean()?,
                "WorkersDetonate" => p.workers_detonate = a.boolean()?,
                "NumVirtualMines" => p.num_virtual_mines = a.uint()?,
                "CreationList" => p.creation_list = Some(Ref::named(a.next()?.to_string())),
                _ => {
                    let _ = a.rest();
                }
            }
            Ok(true)
        });
        p
    }
}

/// `ProneUpdate` / `DemoTrapUpdate` / `CleanupHazardUpdate` / `CommandButtonHuntUpdate` (typed).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SimpleUpdateData {
    pub class: String,
    pub damage_to_frames_ratio: f32,
    pub scan_rate: u32,
    pub scan_range: f32,
    pub trigger_detonation_range: f32,
    pub detonation_weapon: Option<Ref<WeaponId>>,
    pub ignore_kind_of: KindOfMask,
    pub friendly_detonation: bool,
    pub detonate_when_killed: bool,
    pub default_proximity_mode: bool,
}

impl SimpleUpdateData {
    pub(crate) fn parse(class: &str, block: &Block, d: &mut Diag) -> Self {
        let mut p = SimpleUpdateData { class: class.to_string(), damage_to_frames_ratio: 1.0, ..Default::default() };
        if class == "CommandButtonHuntUpdate" {
            // `CommandButtonHuntUpdateModuleData` constructor (CommandButtonHuntUpdate.cpp:58-61): one
            // second and a 9999 range unless the INI says otherwise. Without this a rebel never scans.
            p.scan_rate = 30;
            p.scan_range = 9999.0;
        }
        fields(block, d, class, |k, a| {
            match k {
                "DamageToFramesRatio" => p.damage_to_frames_ratio = a.real()?,
                "ScanRate" => p.scan_rate = a.duration_frames()?,
                "ScanRange" => p.scan_range = a.real()?,
                "TriggerDetonationRange" => p.trigger_detonation_range = a.real()?,
                "DetonationWeapon" => p.detonation_weapon = Some(Ref::named(a.next()?.to_string())),
                "IgnoreTargetTypes" => p.ignore_kind_of = p.ignore_kind_of.parse(a.rest())?,
                "AutoDetonationWithFriendsInvolved" => p.friendly_detonation = a.boolean()?,
                "DetonateWhenKilled" => p.detonate_when_killed = a.boolean()?,
                "DefaultProximityMode" => p.default_proximity_mode = a.boolean()?,
                _ => {
                    let _ = a.rest();
                }
            }
            Ok(true)
        });
        p
    }
}

/// `TransitionDamageFXModuleData`: FX list names per body damage state (`Damaged` / `ReallyDamaged`
/// / `Rubble`, slots 1..12).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TransitionDamageData {
    pub damaged_fx: Vec<String>,
    pub really_damaged_fx: Vec<String>,
    pub rubble_fx: Vec<String>,
}

impl TransitionDamageData {
    pub(crate) fn parse(block: &Block, d: &mut Diag) -> Self {
        let mut p = TransitionDamageData::default();
        fields(block, d, "TransitionDamageFX", |k, a| {
            let (state, fx) = if let Some(rest) = k.strip_prefix("Damaged ") {
                (0, rest)
            } else if let Some(rest) = k.strip_prefix("ReallyDamaged ") {
                (1, rest)
            } else if let Some(rest) = k.strip_prefix("Rubble ") {
                (2, rest)
            } else {
                let _ = a.rest();
                return Ok(true);
            };
            if !fx.to_ascii_lowercase().starts_with("fxlist") {
                let _ = a.rest();
                return Ok(true);
            }
            // `... FXList:<name>` (client FX; the OCL/particle forms are consumed too).
            let name = a.rest().iter().find_map(|t| t.strip_prefix("FXList:")).map(str::to_string);
            if let Some(n) = name {
                match state {
                    0 => p.damaged_fx.push(n),
                    1 => p.really_damaged_fx.push(n),
                    _ => p.rubble_fx.push(n),
                }
            }
            Ok(true)
        });
        p
    }
}

/// `JetAIUpdateModuleData` (the airfield state machine fields).
#[derive(Debug, Clone, PartialEq)]
pub struct JetAiData {
    pub needs_runway: bool,
    pub keeps_parking_space_when_airborne: bool,
    pub takeoff_pause: u32,
    pub return_to_base_idle_time: u32,
    pub out_of_ammo_damage_per_second: f32,
    /// `TakeoffDistForMaxLift` (percent -> fraction).
    pub takeoff_dist_for_max_lift: f32,
    /// `MinHeight`: how far off the ground the drawable is lifted when taxiing.
    pub min_height: f32,
    /// `ParkingOffset`: tweak of the parking location along the parking orientation.
    pub parking_offset: f32,
    /// `SneakyOffsetWhenAttacking`.
    pub sneaky_offset_when_attacking: f32,
    /// `AttackLocomotorType` / `ReturnForAmmoLocomotorType` (`SET_NORMAL` by default).
    pub attacking_loco: LocomotorSetType,
    pub returning_loco: LocomotorSetType,
    /// `AttackLocomotorPersistTime` / `AttackersMissPersistTime`, frames.
    pub attack_loco_persist_time: u32,
    pub attackers_miss_persist_time: u32,
}

impl Default for JetAiData {
    fn default() -> Self {
        JetAiData {
            needs_runway: true,
            keeps_parking_space_when_airborne: true,
            takeoff_pause: 0,
            return_to_base_idle_time: 0,
            out_of_ammo_damage_per_second: 0.0,
            takeoff_dist_for_max_lift: 0.0,
            min_height: 0.0,
            parking_offset: 0.0,
            sneaky_offset_when_attacking: 0.0,
            attacking_loco: LocomotorSetType::SetNormal,
            returning_loco: LocomotorSetType::SetNormal,
            attack_loco_persist_time: 0,
            attackers_miss_persist_time: 0,
        }
    }
}

/// Parse the derived `JetAIUpdate` fields out of the raw `extra` items.
pub(crate) fn parse_jet_ai(extra: &[crate::diag::RawItem], d: &mut Diag) -> JetAiData {
    let mut p = JetAiData::default();
    for item in extra {
        let crate::diag::RawItem::Field(f) = item else { continue };
        let mut a = Args::new(&f.value);
        let r: Result<(), String> = (|| {
            match f.key.as_str() {
                "NeedsRunway" => p.needs_runway = a.boolean()?,
                "KeepsParkingSpaceWhenAirborne" => p.keeps_parking_space_when_airborne = a.boolean()?,
                "TakeoffPause" => p.takeoff_pause = a.duration_frames()?,
                "ReturnToBaseIdleTime" => p.return_to_base_idle_time = a.duration_frames()?,
                "OutOfAmmoDamagePerSecond" => p.out_of_ammo_damage_per_second = a.percent()?,
                "TakeoffDistForMaxLift" => p.takeoff_dist_for_max_lift = a.percent()?,
                "MinHeight" => p.min_height = a.real()?,
                "ParkingOffset" => p.parking_offset = a.real()?,
                "SneakyOffsetWhenAttacking" => p.sneaky_offset_when_attacking = a.real()?,
                "AttackLocomotorType" => p.attacking_loco = a.index()?,
                "ReturnForAmmoLocomotorType" => p.returning_loco = a.index()?,
                "AttackLocomotorPersistTime" => p.attack_loco_persist_time = a.duration_frames()?,
                "AttackersMissPersistTime" => p.attackers_miss_persist_time = a.duration_frames()?,
                _ => return Ok(()),
            }
            Ok(())
        })();
        if let Err(e) = r {
            d.bad_value(&zr_ini::Field { key: f.key.clone(), value: f.value.clone(), line: f.line }, e);
        }
    }
    p
}

/// `ParkingPlaceBehaviorModuleData`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ParkingPlaceData {
    pub num_rows: i32,
    pub num_cols: i32,
    pub has_runways: bool,
    pub heal_amount_per_second: f32,
    /// `ApproachHeight`.
    pub approach_height: f32,
    /// `LandingDeckHeightOffset` (aircraft carriers).
    pub landing_deck_height_offset: f32,
    /// `ParkInHangars`: park at the hangar production spot instead of the parking place.
    pub park_in_hangars: bool,
}

impl ParkingPlaceData {
    pub(crate) fn parse(block: &Block, d: &mut Diag) -> Self {
        let mut p = ParkingPlaceData::default();
        fields(block, d, "ParkingPlaceBehavior", |k, a| {
            match k {
                "NumRows" => p.num_rows = a.int()?,
                "NumCols" => p.num_cols = a.int()?,
                "HasRunways" => p.has_runways = a.boolean()?,
                "HealAmountPerSecond" => p.heal_amount_per_second = a.real()?,
                "ApproachHeight" => p.approach_height = a.real()?,
                "LandingDeckHeightOffset" => p.landing_deck_height_offset = a.real()?,
                "ParkInHangars" => p.park_in_hangars = a.boolean()?,
                _ => {
                    let _ = a.rest();
                }
            }
            Ok(true)
        });
        p
    }
}

/// `CountermeasuresBehaviorModuleData`.
#[derive(Debug, Clone, PartialEq)]
pub struct CountermeasuresData {
    pub flare_template_name: Option<String>,
    pub evasion_rate: f32,
    pub volley_size: u32,
    pub volley_arc_angle: f32,
    pub volley_velocity_factor: f32,
    pub number_of_volleys: u32,
    pub delay_between_volleys: u32,
    pub reload_time: u32,
    pub must_reload_at_airfield: bool,
    pub missile_decoy_delay: u32,
    pub countermeasure_reaction_frames: u32,
    pub starts_active: bool,
    pub triggered_by: Vec<String>,
    pub conflicts_with: Vec<String>,
}

impl Default for CountermeasuresData {
    fn default() -> Self {
        CountermeasuresData {
            flare_template_name: None,
            evasion_rate: 0.0,
            volley_size: 0,
            volley_arc_angle: 0.0,
            volley_velocity_factor: 1.0,
            number_of_volleys: 0,
            delay_between_volleys: 0,
            reload_time: 0,
            must_reload_at_airfield: false,
            missile_decoy_delay: 0,
            countermeasure_reaction_frames: 0,
            starts_active: false,
            triggered_by: Vec::new(),
            conflicts_with: Vec::new(),
        }
    }
}

impl CountermeasuresData {
    pub(crate) fn parse(block: &Block, d: &mut Diag) -> Self {
        let mut p = CountermeasuresData::default();
        fields(block, d, "CountermeasuresBehavior", |k, a| {
            match k {
                "FlareTemplateName" => p.flare_template_name = Some(a.next()?.to_string()),
                "EvasionRate" => p.evasion_rate = a.percent()?,
                "VolleySize" => p.volley_size = a.uint()?,
                "VolleyArcAngle" => p.volley_arc_angle = a.angle()?,
                "VolleyVelocityFactor" => p.volley_velocity_factor = a.real()?,
                "NumberOfVolleys" => p.number_of_volleys = a.uint()?,
                "DelayBetweenVolleys" => p.delay_between_volleys = a.duration_frames()?,
                "ReloadTime" => p.reload_time = a.duration_frames()?,
                "MustReloadAtAirfield" => p.must_reload_at_airfield = a.boolean()?,
                "MissileDecoyDelay" => p.missile_decoy_delay = a.duration_frames()?,
                "ReactionLaunchLatency" => p.countermeasure_reaction_frames = a.duration_frames()?,
                "StartsActive" => p.starts_active = a.boolean()?,
                "TriggeredBy" => p.triggered_by = a.rest().iter().map(|s| s.to_string()).collect(),
                "ConflictsWith" => p.conflicts_with = a.rest().iter().map(|s| s.to_string()).collect(),
                _ => {
                    let _ = a.rest();
                }
            }
            Ok(true)
        });
        p
    }
}

/// `RebuildHoleBehaviorModuleData`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RebuildHoleData {
    pub worker_object_name: Option<String>,
    pub worker_respawn_delay: u32,
    pub hole_health_regen_percent_per_second: f32,
}

impl RebuildHoleData {
    pub(crate) fn parse(block: &Block, d: &mut Diag) -> Self {
        let mut p = RebuildHoleData { hole_health_regen_percent_per_second: 0.1, ..Default::default() };
        fields(block, d, "RebuildHoleBehavior", |k, a| {
            match k {
                "WorkerObjectName" => p.worker_object_name = Some(a.next()?.to_string()),
                "WorkerRespawnDelay" => p.worker_respawn_delay = a.duration_real()? as u32,
                "HoleHealthRegen%PerSecond" => p.hole_health_regen_percent_per_second = a.percent()?,
                _ => {
                    let _ = a.rest();
                }
            }
            Ok(true)
        });
        p
    }
}

/// `RebuildHoleExposeDieModuleData`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RebuildHoleExposeData {
    pub hole_name: Option<String>,
    pub hole_max_health: f32,
    pub transfer_attackers: bool,
}

impl RebuildHoleExposeData {
    pub(crate) fn parse(block: &Block, d: &mut Diag) -> Self {
        let mut p = RebuildHoleExposeData { transfer_attackers: true, ..Default::default() };
        fields(block, d, "RebuildHoleExposeDie", |k, a| {
            match k {
                "HoleName" => p.hole_name = Some(a.next()?.to_string()),
                "HoleMaxHealth" => p.hole_max_health = a.real()?,
                "TransferAttackers" => p.transfer_attackers = a.boolean()?,
                _ => {
                    let _ = a.rest();
                }
            }
            Ok(true)
        });
        p
    }
}

/// `GenerateMinefieldBehaviorModuleData`.
#[derive(Debug, Clone, PartialEq)]
pub struct GenerateMinefieldData {
    pub mine_name: Option<String>,
    pub upgraded_mine_name: Option<String>,
    pub upgraded_triggered_by: Option<String>,
    pub distance_around_object: f32,
    pub mines_per_square_foot: f32,
    pub generate_only_on_death: bool,
    pub border_only: bool,
    pub smart_border: bool,
    pub smart_border_skip_interior: bool,
    pub always_circular: bool,
    pub upgradable: bool,
    pub random_jitter: f32,
    pub triggered_by: Vec<String>,
    pub conflicts_with: Vec<String>,
}

impl Default for GenerateMinefieldData {
    fn default() -> Self {
        GenerateMinefieldData {
            mine_name: None,
            upgraded_mine_name: None,
            upgraded_triggered_by: None,
            distance_around_object: 40.0,
            mines_per_square_foot: 0.0,
            generate_only_on_death: false,
            border_only: true,
            smart_border: false,
            smart_border_skip_interior: true,
            always_circular: false,
            upgradable: false,
            random_jitter: 0.0,
            triggered_by: Vec::new(),
            conflicts_with: Vec::new(),
        }
    }
}

impl GenerateMinefieldData {
    pub(crate) fn parse(block: &Block, d: &mut Diag) -> Self {
        let mut p = GenerateMinefieldData::default();
        fields(block, d, "GenerateMinefieldBehavior", |k, a| {
            match k {
                "MineName" => p.mine_name = Some(a.next()?.to_string()),
                "UpgradedMineName" => p.upgraded_mine_name = Some(a.next()?.to_string()),
                "UpgradedTriggeredBy" => p.upgraded_triggered_by = Some(a.next()?.to_string()),
                "DistanceAroundObject" => p.distance_around_object = a.real()?,
                "MinesPerSquareFoot" => p.mines_per_square_foot = a.real()?,
                "GenerateOnlyOnDeath" => p.generate_only_on_death = a.boolean()?,
                "BorderOnly" => p.border_only = a.boolean()?,
                "SmartBorder" => p.smart_border = a.boolean()?,
                "SmartBorderSkipInterior" => p.smart_border_skip_interior = a.boolean()?,
                "AlwaysCircular" => p.always_circular = a.boolean()?,
                "Upgradable" => p.upgradable = a.boolean()?,
                "RandomJitter" => p.random_jitter = a.percent()?,
                "TriggeredBy" => p.triggered_by = a.rest().iter().map(|s| s.to_string()).collect(),
                "ConflictsWith" => p.conflicts_with = a.rest().iter().map(|s| s.to_string()).collect(),
                _ => {
                    let _ = a.rest();
                }
            }
            Ok(true)
        });
        p
    }
}

/// The list of class names this module types.
pub(crate) fn is_typed2_class(class: &str) -> bool {
    matches!(
        class,
        "SquishCollide"
            | "CrateCollide"
            | "MoneyCrateCollide"
            | "HealCrateCollide"
            | "VeterancyCrateCollide"
            | "UnitCrateCollide"
            | "SalvageCrateCollide"
            | "ShroudCrateCollide"
            | "ConvertToCarBombCrateCollide"
            | "ConvertToHijackedVehicleCrateCollide"
            | "SabotageCommandCenterCrateCollide"
            | "SabotageFakeBuildingCrateCollide"
            | "SabotageInternetCenterCrateCollide"
            | "SabotageMilitaryFactoryCrateCollide"
            | "SabotagePowerPlantCrateCollide"
            | "SabotageSuperweaponCrateCollide"
            | "SabotageSupplyCenterCrateCollide"
            | "SabotageSupplyDropzoneCrateCollide"
            | "FireWeaponCollide"
            | "VeterancyGainCreate"
            | "ToppleUpdate"
            | "SpawnBehavior"
            | "SlavedUpdate"
            | "HordeUpdate"
            | "AutoDepositUpdate"
            | "BaseRegenerateUpdate"
            | "PropagandaTowerBehavior"
            | "FireWeaponUpdate"
            | "RepairDockUpdate"
            | "HealContain"
            | "TunnelContain"
            | "OverchargeBehavior"
            | "PowerPlantUpdate"
            | "MinefieldBehavior"
            | "ProneUpdate"
            | "DemoTrapUpdate"
            | "CleanupHazardUpdate"
            | "CommandButtonHuntUpdate"
            | "TransitionDamageFX"
            | "ParkingPlaceBehavior"
            | "CountermeasuresBehavior"
            | "RebuildHoleBehavior"
            | "RebuildHoleExposeDie"
            | "GenerateMinefieldBehavior"
    )
}
