//! Death modules and their `DieMuxData`. Ported from `GameLogic/Object/Die/DieModule.cpp`
//! (`DieMuxData::getFieldParse`), `GameLogic/Object/Behavior/SlowDeathBehavior.cpp`,
//! `InstantDeathBehavior.cpp`, `GameLogic/Object/Die/CreateObjectDie.cpp` and `FXListDie.cpp`.
//!
//! These modules share the `DieMuxData` selector (`DeathTypes`, `VeterancyLevels`, `ExemptStatus`,
//! `RequiredStatus`); the fields are read as `parseDeathTypeFlags`, `parseVeterancyLevelFlags` and
//! `ObjectStatusMaskType::parseFromINI` (see [`crate::value`]).

use std::collections::BTreeMap;

use zr_ini::{Block, Item};

use crate::diag::{Diag, WarningKind};
use crate::flags::Mask;
use crate::names::{DeathType, ObjectStatus, SlowDeathPhase};
use crate::registry::{OclId, Ref, WeaponId};
use crate::upgrade_module::UpgradeMuxData;
use crate::value::{parse_flag_list_all, parse_veterancy_level_flags, Args};

/// `DieMuxData`: which deaths this die module handles.
#[derive(Debug, Clone, PartialEq)]
pub struct DieMuxData {
    /// `DeathTypes` (`parseDeathTypeFlags`, starts at ALL).
    pub death_types: Mask<DeathType>,
    /// `VeterancyLevels` (`parseVeterancyLevelFlags`); the engine's bit index is `level - 1`.
    pub veterancy_levels: u32,
    /// `ExemptStatus`: ignored if any of these object status bits are set.
    pub exempt_status: Mask<ObjectStatus>,
    /// `RequiredStatus`: ignored unless all of these object status bits are set.
    pub required_status: Mask<ObjectStatus>,
}

impl Default for DieMuxData {
    fn default() -> Self {
        DieMuxData {
            death_types: Mask::from_bits(u32::MAX as u128),
            veterancy_levels: u32::MAX,
            exempt_status: Mask::EMPTY,
            required_status: Mask::EMPTY,
        }
    }
}

impl DieMuxData {
    /// `DieMuxData::isDieApplicable`'s veterancy test (`getVeterancyLevelFlag`).
    pub fn applies_to_veterancy(&self, level_index: u8) -> bool {
        match level_index.checked_sub(1) {
            Some(bit) if bit < 32 => self.veterancy_levels & (1u32 << bit) != 0,
            // `REGULAR` has no bit in the engine (`1UL << (0 - 1)`), so it never matches.
            _ => false,
        }
    }

    /// One row of `DieMuxData::getFieldParse`; `Ok(false)` for an unknown key.
    pub(crate) fn field(&mut self, key: &str, a: &mut Args) -> Result<bool, String> {
        match key {
            "DeathTypes" => self.death_types = parse_flag_list_all(a.rest())?,
            "VeterancyLevels" => self.veterancy_levels = parse_veterancy_level_flags(a.rest())?,
            "ExemptStatus" => self.exempt_status = self.exempt_status.parse(a.rest())?,
            "RequiredStatus" => self.required_status = self.required_status.parse(a.rest())?,
            _ => return Ok(false),
        }
        Ok(true)
    }
}

/// The class-specific data of a die module.
#[derive(Debug, Clone, PartialEq)]
pub enum DieKind {
    /// `DestroyDie`: no data of its own.
    Destroy,
    /// `InstantDeathBehavior`: flat effect lists.
    InstantDeath {
        fx: Vec<Option<String>>,
        ocl: Vec<Ref<OclId>>,
        weapons: Vec<Ref<WeaponId>>,
    },
    /// `CreateObjectDie`.
    CreateObject {
        ocl: Option<Ref<OclId>>,
        transfer_previous_health: bool,
        transfer_selection: bool,
    },
    /// `FXListDie` (also an upgrade module).
    FxList {
        default_death_fx: Option<String>,
        orient_to_object: bool,
        initially_active: bool,
        upgrade: UpgradeMuxData,
    },
    /// `SlowDeathBehavior`.
    SlowDeath(SlowDeathData),
    /// `CrushDie`.
    Crush(crate::modules2::CrushDieData),
    /// `CreateCrateDie`.
    CreateCrate(crate::modules2::CreateCrateDieData),
    /// `EjectPilotDie`.
    EjectPilot(crate::modules2::EjectPilotDieData),
    /// `KeepObjectDie`: the object stays (as rubble) instead of being removed.
    Keep,
}

/// `SlowDeathBehaviorModuleData` (without the embedded `DieMuxData`). Defaults are its constructor's.
#[derive(Debug, Clone, PartialEq)]
pub struct SlowDeathData {
    /// `SinkRate`, units per frame.
    pub sink_rate: f32,
    pub probability_modifier: i32,
    pub modifier_bonus_per_overkill_percent: f32,
    /// Frames.
    pub sink_delay: u32,
    pub sink_delay_variance: u32,
    pub destruction_delay: u32,
    pub destruction_delay_variance: u32,
    pub destruction_altitude: f32,
    /// Effects by phase (`INITIAL`, `MIDPOINT`, `FINAL`).
    pub fx: BTreeMap<SlowDeathPhase, Vec<Option<String>>>,
    pub ocl: BTreeMap<SlowDeathPhase, Vec<Ref<OclId>>>,
    pub weapons: BTreeMap<SlowDeathPhase, Vec<Ref<WeaponId>>>,
    pub fling_force: f32,
    pub fling_force_variance: f32,
    /// Radians.
    pub fling_pitch: f32,
    pub fling_pitch_variance: f32,
}

impl SlowDeathData {
    pub(crate) fn new() -> Self {
        SlowDeathData {
            sink_rate: 0.0,
            probability_modifier: 10,
            modifier_bonus_per_overkill_percent: 0.0,
            sink_delay: 0,
            sink_delay_variance: 0,
            destruction_delay: 0,
            destruction_delay_variance: 0,
            destruction_altitude: -10.0,
            fx: BTreeMap::new(),
            ocl: BTreeMap::new(),
            weapons: BTreeMap::new(),
            fling_force: 0.0,
            fling_force_variance: 0.0,
            fling_pitch: 0.0,
            fling_pitch_variance: 0.0,
        }
    }

    /// One row of `SlowDeathBehavior::buildFieldParse`; `Ok(false)` for an unknown key.
    pub(crate) fn field(&mut self, key: &str, a: &mut Args) -> Result<bool, String> {
        match key {
            "SinkRate" => self.sink_rate = a.velocity()?,
            "ProbabilityModifier" => self.probability_modifier = a.int()?,
            "ModifierBonusPerOverkillPercent" => self.modifier_bonus_per_overkill_percent = a.percent()?,
            "SinkDelay" => self.sink_delay = a.duration_frames()?,
            "SinkDelayVariance" => self.sink_delay_variance = a.duration_frames()?,
            "DestructionDelay" => self.destruction_delay = a.duration_frames()?,
            "DestructionDelayVariance" => self.destruction_delay_variance = a.duration_frames()?,
            "DestructionAltitude" => self.destruction_altitude = a.real()?,
            "FlingForce" => self.fling_force = a.real()?,
            "FlingForceVariance" => self.fling_force_variance = a.real()?,
            "FlingPitch" => self.fling_pitch = a.angle()?,
            "FlingPitchVariance" => self.fling_pitch_variance = a.angle()?,
            "FX" => {
                let phase: SlowDeathPhase = a.index()?;
                self.fx.entry(phase).or_default().extend(a.rest().iter().map(|s| none_if_none(s.to_string())));
            }
            "OCL" => {
                let phase: SlowDeathPhase = a.index()?;
                self.ocl.entry(phase).or_default().extend(a.rest().iter().map(|s| Ref::named(s.to_string())));
            }
            "Weapon" => {
                let phase: SlowDeathPhase = a.index()?;
                self.weapons.entry(phase).or_default().extend(a.rest().iter().map(|s| Ref::named(s.to_string())));
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
}

/// A typed die module: the shared mux plus the class-specific kind.
#[derive(Debug, Clone, PartialEq)]
pub struct DieData {
    pub mux: DieMuxData,
    pub kind: DieKind,
}

impl DieData {
    /// `ModuleFactory::newModuleDataFromINI` for one of the typed die classes.
    pub(crate) fn parse(class: &str, block: &Block, d: &mut Diag) -> Self {
        let mut mux = DieMuxData::default();
        let mut kind = match class {
            "SlowDeathBehavior" => DieKind::SlowDeath(SlowDeathData::new()),
            "InstantDeathBehavior" => DieKind::InstantDeath { fx: Vec::new(), ocl: Vec::new(), weapons: Vec::new() },
            "CreateObjectDie" => DieKind::CreateObject { ocl: None, transfer_previous_health: false, transfer_selection: false },
            "FXListDie" => DieKind::FxList {
                default_death_fx: None,
                orient_to_object: true,
                initially_active: true,
                upgrade: UpgradeMuxData::default(),
            },
            "CrushDie" => DieKind::Crush(crate::modules2::CrushDieData { total_crush_sound_percent: 100, back_end_crush_sound_percent: 100, front_end_crush_sound_percent: 100 }),
            "CreateCrateDie" => DieKind::CreateCrate(crate::modules2::CreateCrateDieData::default()),
            "EjectPilotDie" => DieKind::EjectPilot(crate::modules2::EjectPilotDieData::default()),
            "KeepObjectDie" => DieKind::Keep,
            _ => DieKind::Destroy,
        };
        for item in &block.items {
            let Item::Field(f) = item else {
                d.warn(WarningKind::Structure, "", 0, "unexpected nested block in a die module");
                continue;
            };
            let mut a = Args::new(&f.value);
            let r: Result<bool, String> = (|| {
                // Shared DieMuxData first.
                match f.key.as_str() {
                    "DeathTypes" => {
                        mux.death_types = parse_flag_list_all(a.rest())?;
                        return Ok(true);
                    }
                    "VeterancyLevels" => {
                        mux.veterancy_levels = parse_veterancy_level_flags(a.rest())?;
                        return Ok(true);
                    }
                    "ExemptStatus" => {
                        mux.exempt_status = mux.exempt_status.parse(a.rest())?;
                        return Ok(true);
                    }
                    "RequiredStatus" => {
                        mux.required_status = mux.required_status.parse(a.rest())?;
                        return Ok(true);
                    }
                    _ => {}
                }
                match (&mut kind, f.key.as_str()) {
                    (DieKind::SlowDeath(sd), "SinkRate") => sd.sink_rate = a.velocity()?,
                    (DieKind::SlowDeath(sd), "ProbabilityModifier") => sd.probability_modifier = a.int()?,
                    (DieKind::SlowDeath(sd), "ModifierBonusPerOverkillPercent") => {
                        sd.modifier_bonus_per_overkill_percent = a.percent()?
                    }
                    (DieKind::SlowDeath(sd), "SinkDelay") => sd.sink_delay = a.duration_frames()?,
                    (DieKind::SlowDeath(sd), "SinkDelayVariance") => sd.sink_delay_variance = a.duration_frames()?,
                    (DieKind::SlowDeath(sd), "DestructionDelay") => sd.destruction_delay = a.duration_frames()?,
                    (DieKind::SlowDeath(sd), "DestructionDelayVariance") => {
                        sd.destruction_delay_variance = a.duration_frames()?
                    }
                    (DieKind::SlowDeath(sd), "DestructionAltitude") => sd.destruction_altitude = a.real()?,
                    (DieKind::SlowDeath(sd), "FlingForce") => sd.fling_force = a.real()?,
                    (DieKind::SlowDeath(sd), "FlingForceVariance") => sd.fling_force_variance = a.real()?,
                    (DieKind::SlowDeath(sd), "FlingPitch") => sd.fling_pitch = a.angle()?,
                    (DieKind::SlowDeath(sd), "FlingPitchVariance") => sd.fling_pitch_variance = a.angle()?,
                    (DieKind::SlowDeath(sd), "FX") => {
                        let phase: SlowDeathPhase = a.index()?;
                        sd.fx.entry(phase).or_default().extend(a.rest().iter().map(|s| none_if_none(s.to_string())));
                    }
                    (DieKind::SlowDeath(sd), "OCL") => {
                        let phase: SlowDeathPhase = a.index()?;
                        sd.ocl.entry(phase).or_default().extend(a.rest().iter().map(|s| Ref::named(s.to_string())));
                    }
                    (DieKind::SlowDeath(sd), "Weapon") => {
                        let phase: SlowDeathPhase = a.index()?;
                        sd.weapons.entry(phase).or_default().extend(a.rest().iter().map(|s| Ref::named(s.to_string())));
                    }
                    (DieKind::InstantDeath { fx, .. }, "FX") => {
                        fx.extend(a.rest().iter().map(|s| none_if_none(s.to_string())))
                    }
                    (DieKind::InstantDeath { ocl, .. }, "OCL") => {
                        ocl.extend(a.rest().iter().map(|s| Ref::named(s.to_string())))
                    }
                    (DieKind::InstantDeath { weapons, .. }, "Weapon") => {
                        weapons.extend(a.rest().iter().map(|s| Ref::named(s.to_string())))
                    }
                    (DieKind::CreateObject { ocl, .. }, "CreationList") => {
                        let n = a.next()?;
                        *ocl = if n.eq_ignore_ascii_case("None") { None } else { Some(Ref::named(n.to_string())) };
                    }
                    (DieKind::CreateObject { transfer_previous_health, .. }, "TransferPreviousHealth") => {
                        *transfer_previous_health = a.boolean()?
                    }
                    (DieKind::CreateObject { transfer_selection, .. }, "TransferSelection") => {
                        *transfer_selection = a.boolean()?
                    }
                    (DieKind::FxList { default_death_fx, .. }, "DeathFX") => {
                        let n = a.next()?;
                        *default_death_fx = none_if_none(n.to_string());
                    }
                    (DieKind::FxList { orient_to_object, .. }, "OrientToObject") => *orient_to_object = a.boolean()?,
                    (DieKind::FxList { initially_active, .. }, "StartsActive") => *initially_active = a.boolean()?,
                    (DieKind::FxList { upgrade, .. }, key) => {
                        if !upgrade.field(key, &mut a)? {
                            return Ok(false);
                        }
                    }
                    (DieKind::Crush(c), "TotalCrushSoundPercent") => c.total_crush_sound_percent = a.int()?,
                    (DieKind::Crush(c), "BackEndCrushSoundPercent") => c.back_end_crush_sound_percent = a.int()?,
                    (DieKind::Crush(c), "FrontEndCrushSoundPercent") => c.front_end_crush_sound_percent = a.int()?,
                    (DieKind::Crush(_), "TotalCrushSound" | "BackEndCrushSound" | "FrontEndCrushSound") => {
                        let _ = a.rest();
                    }
                    (DieKind::CreateCrate(c), "CrateData") => c.crate_names.push(a.next()?.to_string()),
                    (DieKind::EjectPilot(e), "AirCreationList") => e.air_creation_list = Some(Ref::named(a.next()?.to_string())),
                    (DieKind::EjectPilot(e), "GroundCreationList") => e.ground_creation_list = Some(Ref::named(a.next()?.to_string())),
                    (DieKind::EjectPilot(e), "InvulnerableTime") => e.invulnerable_time = a.duration_frames()?,
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
        DieData { mux, kind }
    }
}

/// Whether `class` is one of the die classes this module types.
pub(crate) fn is_typed_die_class(class: &str) -> bool {
    matches!(
        class,
        "DestroyDie"
            | "InstantDeathBehavior"
            | "CreateObjectDie"
            | "FXListDie"
            | "SlowDeathBehavior"
            | "CrushDie"
            | "CreateCrateDie"
            | "EjectPilotDie"
            | "KeepObjectDie"
    )
}

fn none_if_none(s: String) -> Option<String> {
    if s.is_empty() || s.eq_ignore_ascii_case("None") {
        None
    } else {
        Some(s)
    }
}
