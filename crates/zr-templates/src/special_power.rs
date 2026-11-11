//! Special powers, sciences and ranks.
//!
//! `SpecialPower` (`Common/RTS/SpecialPower.cpp` `m_specialPowerFieldParse`), `Science`
//! (`Common/RTS/Science.cpp` `friend_parseScienceDefinition`) and `Rank`
//! (`Core/.../RankInfo.cpp` field parse). Durations are frames.

use zr_ini::{Block, Item};

use crate::diag::{Diag, WarningKind};
use crate::value::Args;

/// A `SpecialPower NAME ... End` template.
#[derive(Debug, Clone, PartialEq)]
pub struct SpecialPowerTemplate {
    pub name: String,
    /// `Enum` (`SpecialPowerType`, e.g. `SPECIAL_A10_THUNDERBOLT_STRIKE`).
    pub kind: String,
    /// `ReloadTime`, frames.
    pub reload_time: u32,
    /// `RequiredScience` (`None` = `SCIENCE_INVALID`).
    pub required_science: Option<String>,
    /// `PublicTimer`.
    pub public_timer: bool,
    /// `SharedSyncedTimer`.
    pub shared_synced_timer: bool,
    /// `DetectionTime`, frames.
    pub detection_time: u32,
    /// `ViewObjectDuration`, frames.
    pub view_object_duration: u32,
    pub view_object_range: f32,
    pub radius_cursor_radius: f32,
    pub shortcut_power: bool,
    pub academy_classification: String,
}

impl Default for SpecialPowerTemplate {
    fn default() -> Self {
        SpecialPowerTemplate {
            name: String::new(),
            kind: String::new(),
            reload_time: 0,
            required_science: None,
            public_timer: false,
            shared_synced_timer: false,
            detection_time: 0,
            view_object_duration: 0,
            view_object_range: 0.0,
            radius_cursor_radius: 0.0,
            shortcut_power: false,
            academy_classification: String::new(),
        }
    }
}

impl SpecialPowerTemplate {
    pub(crate) fn parse(name: &str, b: &Block, d: &mut Diag) -> Self {
        let mut p = SpecialPowerTemplate { name: name.to_string(), ..SpecialPowerTemplate::default() };
        for item in &b.items {
            let Item::Field(f) = item else {
                d.warn(WarningKind::Structure, "", 0, "unexpected nested block in SpecialPower");
                continue;
            };
            let mut a = Args::new(&f.value);
            let r: Result<bool, String> = (|| {
                match f.key.as_str() {
                    "Enum" => p.kind = a.next()?.to_string(),
                    "ReloadTime" => p.reload_time = a.duration_frames()?,
                    "RequiredScience" => {
                        let t = a.next()?;
                        p.required_science = if t.eq_ignore_ascii_case("None") { None } else { Some(t.to_string()) };
                    }
                    "PublicTimer" => p.public_timer = a.boolean()?,
                    "SharedSyncedTimer" => p.shared_synced_timer = a.boolean()?,
                    "DetectionTime" => p.detection_time = a.duration_frames()?,
                    "ViewObjectDuration" => p.view_object_duration = a.duration_frames()?,
                    "ViewObjectRange" => p.view_object_range = a.real()?,
                    "RadiusCursorRadius" => p.radius_cursor_radius = a.real()?,
                    "ShortcutPower" => p.shortcut_power = a.boolean()?,
                    "AcademyClassify" => p.academy_classification = a.next()?.to_string(),
                    // Audio events: accepted, not simulated.
                    "InitiateSound" | "InitiateAtLocationSound" => {
                        let _ = a.string();
                    }
                    _ => return Ok(false),
                }
                Ok(true)
            })();
            match r {
                Ok(true) => {}
                Ok(false) => d.unknown_field(f),
                Err(e) => d.bad_value(f, e),
            }
        }
        p
    }
}

/// A `Science NAME ... End` template.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ScienceInfo {
    pub name: String,
    /// `PrerequisiteSciences`.
    pub prerequisite_sciences: Vec<String>,
    /// `SciencePurchasePointCost`.
    pub science_purchase_point_cost: i32,
    pub grantable: bool,
    pub display_name: Option<String>,
    pub description: Option<String>,
}

impl ScienceInfo {
    pub(crate) fn parse(name: &str, b: &Block, d: &mut Diag) -> Self {
        let mut s = ScienceInfo { name: name.to_string(), ..ScienceInfo::default() };
        for item in &b.items {
            let Item::Field(f) = item else {
                d.warn(WarningKind::Structure, "", 0, "unexpected nested block in Science");
                continue;
            };
            let mut a = Args::new(&f.value);
            let r: Result<bool, String> = (|| {
                match f.key.as_str() {
                    "PrerequisiteSciences" => s.prerequisite_sciences = a.rest().iter().map(|t| t.to_string()).collect(),
                    "SciencePurchasePointCost" => s.science_purchase_point_cost = a.int()?,
                    "IsGrantable" => s.grantable = a.boolean()?,
                    "DisplayName" => s.display_name = Some(a.next()?.to_string()),
                    "Description" => s.description = Some(a.next()?.to_string()),
                    _ => return Ok(false),
                }
                Ok(true)
            })();
            match r {
                Ok(true) => {}
                Ok(false) => d.unknown_field(f),
                Err(e) => d.bad_value(f, e),
            }
        }
        s
    }
}

/// A `Rank` entry (`RankInfo`): one promotion level.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RankInfo {
    pub skill_points_needed: i32,
    pub sciences_granted: Vec<String>,
    pub science_purchase_points_granted: u32,
}

impl RankInfo {
    pub(crate) fn parse(b: &Block, d: &mut Diag) -> Self {
        let mut r = RankInfo::default();
        for item in &b.items {
            let Item::Field(f) = item else {
                d.warn(WarningKind::Structure, "", 0, "unexpected nested block in Rank");
                continue;
            };
            let mut a = Args::new(&f.value);
            let res: Result<bool, String> = (|| {
                match f.key.as_str() {
                    "SkillPointsNeeded" => r.skill_points_needed = a.int()?,
                    "SciencesGranted" => r.sciences_granted = a.rest().iter().map(|t| t.to_string()).collect(),
                    "SciencePurchasePointsGranted" => r.science_purchase_points_granted = a.uint()?,
                    // Localisation label for the rank; not simulated.
                    "RankName" => {
                        let _ = a.string();
                    }
                    _ => return Ok(false),
                }
                Ok(true)
            })();
            match res {
                Ok(true) => {}
                Ok(false) => d.unknown_field(f),
                Err(e) => d.bad_value(f, e),
            }
        }
        r
    }
}

/// The pause and ready flags every `SpecialPowerModuleData` carries
/// (`SpecialPowerModule.cpp:74-86`, `SpecialPowerModuleData::buildFieldParse`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SpecialPowerFlags {
    /// `UpdateModuleStartsAttack`: the update module starts the attack, so `doSpecialPower` does not
    /// trigger it immediately (`SpecialPowerModule.cpp:705`).
    pub update_module_starts_attack: bool,
    /// `StartsPaused`: paused on creation until an upgrade or script unpauses it
    /// (`SpecialPowerModule.cpp:131`).
    pub starts_paused: bool,
    /// `StartsReady`: `SpecialPowerModule::startsReady` is false under `RETAIL_COMPATIBLE_CRC`
    /// (`SpecialPowerModule.cpp:382`), so the flag has no effect in retail builds.
    pub starts_ready: bool,
    /// `ScriptedSpecialPowerOnly`: the player shortcut search skips it (`Player.cpp:1241`).
    pub scripted_special_power_only: bool,
}

impl SpecialPowerFlags {
    /// Parses one of the four flags; `None` for any other key.
    fn parse_field(&mut self, key: &str, a: &mut Args) -> Option<Result<(), String>> {
        let slot = match key {
            "UpdateModuleStartsAttack" => &mut self.update_module_starts_attack,
            "StartsPaused" => &mut self.starts_paused,
            "StartsReady" => &mut self.starts_ready,
            "ScriptedSpecialPowerOnly" => &mut self.scripted_special_power_only,
            _ => return None,
        };
        Some(a.boolean().map(|v| *slot = v))
    }
}

/// The union of the special-power module data (`OCLSpecialPower`, `FireWeaponPower`,
/// `CashHackSpecialPower`); `SpecialPowerTemplate` is common to all.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SpecialPowerModuleData {
    pub module_class: String,
    pub special_power: String,
    pub flags: SpecialPowerFlags,
    /// `OCLSpecialPower` `OCL`.
    pub ocl: Option<String>,
    /// `OCLSpecialPower` `CreateLocation`: `CREATE_AT_TARGET` vs `CREATE_AT_EDGE_NEAR_SOURCE`.
    pub create_at_target: bool,
    /// `OCLSpecialPower` `CreateLocation` as written (`CREATE_AT_EDGE_NEAR_SOURCE` when absent, OCLSpecialPower.cpp:72).
    pub create_location: String,
    pub reference_object: Option<String>,
    /// `FireWeaponPower` `MaxShotsToFire`.
    pub max_shots_to_fire: u32,
    /// `CashHackSpecialPower` `MoneyAmount`.
    pub money_amount: i32,
    /// `BaikonurLaunchPower` `DetonationObject`.
    pub detonation_object: Option<String>,
    /// `CashBountyPower` `Bounty` (percent -> fraction).
    pub bounty: f32,
    /// `CleanupAreaPower` `MaxMoveDistanceFromLocation`.
    pub cleanup_move_range: f32,
    /// `SpyVisionSpecialPower` `BaseDuration`, frames.
    pub spy_vision_base_duration: u32,
    /// `SpyVisionSpecialPower` `BonusDurationPerCaptured`, frames.
    pub spy_vision_bonus_per_captured: u32,
    /// `SpyVisionSpecialPower` `MaxDuration`, frames.
    pub spy_vision_max_duration: u32,
}

impl SpecialPowerModuleData {
    pub(crate) fn parse(class: &str, b: &Block, d: &mut Diag) -> Self {
        let mut m = SpecialPowerModuleData { module_class: class.to_string(), ..SpecialPowerModuleData::default() };
        for item in &b.items {
            let Item::Field(f) = item else {
                d.warn(WarningKind::Structure, "", 0, "unexpected nested block in special power module");
                continue;
            };
            let mut a = Args::new(&f.value);
            let r: Result<bool, String> = (|| {
                match f.key.as_str() {
                    "SpecialPowerTemplate" => m.special_power = a.next()?.to_string(),
                    "OCL" => m.ocl = Some(a.next()?.to_string()),
                    "CreateLocation" => {
                        let t = a.next()?;
                        m.create_at_target = t.eq_ignore_ascii_case("CREATE_AT_TARGET") || t.eq_ignore_ascii_case("CREATE_AT_LOCATION");
                        m.create_location = t.to_string();
                    }
                    "ReferenceObject" => m.reference_object = Some(a.next()?.to_string()),
                    "MaxShotsToFire" => m.max_shots_to_fire = a.uint()?,
                    "MoneyAmount" => m.money_amount = a.int()?,
                    "DetonationObject" => m.detonation_object = Some(a.next()?.to_string()),
                    "Bounty" => m.bounty = a.percent()?,
                    "MaxMoveDistanceFromLocation" => m.cleanup_move_range = a.real()?,
                    "BaseDuration" => m.spy_vision_base_duration = a.duration_frames()?,
                    "BonusDurationPerCaptured" => m.spy_vision_bonus_per_captured = a.duration_frames()?,
                    "MaxDuration" => m.spy_vision_max_duration = a.duration_frames()?,
                    "UpdateModuleStartsAttack" | "StartsPaused" | "StartsReady" | "ScriptedSpecialPowerOnly" => {
                        if let Some(r) = m.flags.parse_field(&f.key, &mut a) {
                            r?;
                        }
                    }
                    "InitiateSound" | "UpgradeOCL" | "UpgradeMoneyAmount" | "OCLAdjustPositionToPassable" => {
                        let _ = a.next();
                    }
                    _ => return Ok(false),
                }
                Ok(true)
            })();
            match r {
                Ok(true) => {}
                Ok(false) => d.unknown_field(f),
                Err(e) => d.bad_value(f, e),
            }
        }
        m
    }
}

/// `true` for the special-power module classes this crate types.
pub(crate) fn is_typed_special_power_class(class: &str) -> bool {
    matches!(
        class,
        "OCLSpecialPower"
            | "FireWeaponPower"
            | "CashHackSpecialPower"
            | "BaikonurLaunchPower"
            | "CashBountyPower"
            | "CleanupAreaPower"
            | "SpyVisionSpecialPower"
    )
}

/// `SpecialAbilityUpdateModuleData`: a unit special ability (Black Lotus, Jarmen Kell, Burton, ...).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SpecialAbilityData {
    pub special_power: String,
    pub start_ability_range: f32,
    pub ability_abort_range: f32,
    /// Frames.
    pub preparation_frames: u32,
    pub persistent_prep_frames: u32,
    pub pack_time: u32,
    pub unpack_time: u32,
    /// `PackUnpackVariationFactor`: the pack / unpack time varies by +/- this fraction.
    pub pack_unpack_variation: f32,
    /// `SkipPackingWithNoTarget` (`SpecialAbilityUpdate.h`): no pack / unpack for a no-target use.
    pub skip_packing_with_no_target: bool,
    pub effect_duration: u32,
    pub effect_value: i32,
    pub special_object: Option<String>,
    pub max_special_objects: u32,
    pub special_objects_persistent: bool,
    pub flee_range_after_completion: f32,
    pub need_to_face: bool,
    pub approach_requires_los: bool,
    pub award_xp_for_triggering: i32,
    pub skill_points_for_triggering: i32,
    /// The pause and ready flags. `SpecialAbility` modules derive from `SpecialPowerModule`, so
    /// they carry them (`SpecialAbility.h`); `SpecialAbilityUpdate` modules do not, and the flags
    /// stay at their defaults for them.
    pub flags: SpecialPowerFlags,
    /// `true` for the `SpecialAbility` class (a `SpecialPowerModule`, which owns the power's
    /// cooldown and pause count). `SpecialAbilityUpdate` is a `SpecialPowerUpdateModule`
    /// (`SpecialAbilityUpdate.h:169`) and has neither.
    pub power_module: bool,
}

impl SpecialAbilityData {
    pub(crate) fn parse(class: &str, b: &Block, d: &mut Diag) -> Self {
        // `SpecialAbilityUpdateModuleData` constructor (`SpecialAbilityUpdate.h:89-114`).
        let mut m = SpecialAbilityData {
            max_special_objects: 1,
            effect_value: 1,
            need_to_face: true,
            // `SPECIAL_ABILITY_HUGE_DISTANCE` (`SpecialAbilityUpdate.h:44`): the engine default is
            // effectively unlimited range, for both the start and the abort distance.
            start_ability_range: 10_000_000.0,
            ability_abort_range: 10_000_000.0,
            approach_requires_los: true,
            skill_points_for_triggering: -1,
            power_module: class == "SpecialAbility",
            ..SpecialAbilityData::default()
        };
        for item in &b.items {
            let Item::Field(f) = item else {
                d.warn(WarningKind::Structure, "", 0, "unexpected nested block in SpecialAbilityUpdate");
                continue;
            };
            let mut a = Args::new(&f.value);
            let r: Result<bool, String> = (|| {
                match f.key.as_str() {
                    "SpecialPowerTemplate" => m.special_power = a.next()?.to_string(),
                    "StartAbilityRange" => m.start_ability_range = a.real()?,
                    "AbilityAbortRange" => m.ability_abort_range = a.real()?,
                    "PreparationTime" => m.preparation_frames = a.duration_frames()?,
                    "PersistentPrepTime" => m.persistent_prep_frames = a.duration_frames()?,
                    "PackTime" => m.pack_time = a.duration_frames()?,
                    "UnpackTime" => m.unpack_time = a.duration_frames()?,
                    "PackUnpackVariationFactor" => m.pack_unpack_variation = a.real()?,
                    "SkipPackingWithNoTarget" => m.skip_packing_with_no_target = a.boolean()?,
                    "EffectDuration" => m.effect_duration = a.duration_frames()?,
                    "EffectValue" => m.effect_value = a.int()?,
                    "SpecialObject" => m.special_object = Some(a.next()?.to_string()),
                    "MaxSpecialObjects" => m.max_special_objects = a.uint()?,
                    "SpecialObjectsPersistent" => m.special_objects_persistent = a.boolean()?,
                    "FleeRangeAfterCompletion" => m.flee_range_after_completion = a.real()?,
                    "NeedToFaceTarget" => m.need_to_face = a.boolean()?,
                    "ApproachRequiresLOS" => m.approach_requires_los = a.boolean()?,
                    "AwardXPForTriggering" => m.award_xp_for_triggering = a.int()?,
                    "SkillPointsForTriggering" => m.skill_points_for_triggering = a.int()?,
                    "UpdateModuleStartsAttack" | "StartsPaused" | "StartsReady" | "ScriptedSpecialPowerOnly" => {
                        if let Some(r) = m.flags.parse_field(&f.key, &mut a) {
                            r?;
                        }
                    }
                    // Accepted, not simulated.
                    _ => {
                        let _ = a.next();
                    }
                }
                Ok(true)
            })();
            match r {
                Ok(true) => {}
                Ok(false) => d.unknown_field(f),
                Err(e) => d.bad_value(f, e),
            }
        }
        m
    }
}
