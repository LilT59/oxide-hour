//! Upgrade modules and their `UpgradeMuxData`. Ported from `GameLogic/Module/UpgradeModule.h`
//! (`UpgradeMuxData::getFieldParse`) and the `buildFieldParse` of `ArmorUpgrade`,
//! `WeaponSetUpgrade`, `LocomotorSetUpgrade` (`GameLogic/Object/Upgrade/*.cpp`) and
//! `MaxHealthUpgrade`.

use zr_ini::{Block, Item};

use crate::diag::{Diag, WarningKind};
use crate::names::MaxHealthChangeType;
use crate::registry::{OclId, Ref};
use crate::value::Args;

/// `UpgradeMuxData`: how an upgrade module is triggered and what it conflicts with.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct UpgradeMuxData {
    /// `TriggeredBy` (`m_activationUpgradeNames`); activation supports one name currently.
    pub triggered_by: Vec<String>,
    /// `ConflictsWith` (`m_conflictingUpgradeNames`), an OR.
    pub conflicts_with: Vec<String>,
    /// `RemovesUpgrades` (`m_removalUpgradeNames`).
    pub removes_upgrades: Vec<String>,
    /// `FXListUpgrade`.
    pub fx_list_upgrade: Option<String>,
    pub requires_all_triggers: bool,
}

impl UpgradeMuxData {
    /// One row of `UpgradeMuxData::getFieldParse`; `Ok(false)` for an unknown key.
    pub(crate) fn field(&mut self, key: &str, a: &mut Args) -> Result<bool, String> {
        match key {
            "TriggeredBy" => self.triggered_by = a.rest().iter().map(|s| s.to_string()).collect(),
            "ConflictsWith" => self.conflicts_with = a.rest().iter().map(|s| s.to_string()).collect(),
            "RemovesUpgrades" => self.removes_upgrades = a.rest().iter().map(|s| s.to_string()).collect(),
            "FXListUpgrade" => {
                let n = a.next()?;
                self.fx_list_upgrade = if n.eq_ignore_ascii_case("None") { None } else { Some(n.to_string()) };
            }
            "RequiresAllTriggers" => self.requires_all_triggers = a.boolean()?,
            _ => return Ok(false),
        }
        Ok(true)
    }
}

/// The class-specific data of a typed upgrade module.
#[derive(Debug, Clone, PartialEq)]
pub enum UpgradeKind {
    /// `ArmorUpgrade`: sets `ARMORSET_PLAYER_UPGRADE`.
    Armor,
    /// `WeaponSetUpgrade`: sets `WEAPONSET_PLAYER_UPGRADE`.
    WeaponSet,
    /// `LocomotorSetUpgrade`: sets the AI's locomotor upgrade flag.
    LocomotorSet,
    /// `MaxHealthUpgrade`.
    MaxHealth {
        add_max_health: f32,
        change_type: MaxHealthChangeType,
    },
    /// `ObjectCreationUpgrade`.
    ObjectCreation { ocl: Option<Ref<OclId>> },
    /// `CommandSetUpgrade`.
    CommandSet {
        command_set: String,
        command_set_alt: String,
        trigger_alt: String,
    },
    /// `ExperienceScalarUpgrade`.
    ExperienceScalar { add_xp_scalar: f32 },
    /// `WeaponBonusUpgrade` (no fields of its own).
    WeaponBonus,
    /// `ModelConditionUpgrade`: set a model-condition bit.
    ModelCondition { flag: Option<String> },
}

/// A typed upgrade module: the mux plus the class-specific kind.
#[derive(Debug, Clone, PartialEq)]
pub struct UpgradeData {
    pub mux: UpgradeMuxData,
    pub kind: UpgradeKind,
}

impl UpgradeData {
    pub(crate) fn parse(class: &str, block: &Block, d: &mut Diag) -> Self {
        let kind = match class {
            "ArmorUpgrade" => UpgradeKind::Armor,
            "WeaponSetUpgrade" => UpgradeKind::WeaponSet,
            "LocomotorSetUpgrade" => UpgradeKind::LocomotorSet,
            "MaxHealthUpgrade" => UpgradeKind::MaxHealth {
                add_max_health: 0.0,
                change_type: MaxHealthChangeType::SameCurrentHealth,
            },
            "ObjectCreationUpgrade" => UpgradeKind::ObjectCreation { ocl: None },
            "CommandSetUpgrade" => UpgradeKind::CommandSet {
                command_set: String::new(),
                command_set_alt: String::new(),
                trigger_alt: "none".to_string(),
            },
            "ExperienceScalarUpgrade" => UpgradeKind::ExperienceScalar { add_xp_scalar: 0.0 },
            "WeaponBonusUpgrade" => UpgradeKind::WeaponBonus,
            "ModelConditionUpgrade" => UpgradeKind::ModelCondition { flag: None },
            _ => UpgradeKind::Armor,
        };
        let mut data = UpgradeData { mux: UpgradeMuxData::default(), kind };
        for item in &block.items {
            let Item::Field(f) = item else {
                d.warn(WarningKind::Structure, "", 0, "unexpected nested block in an upgrade module");
                continue;
            };
            let mut a = Args::new(&f.value);
            let r: Result<bool, String> = (|| {
                if let (UpgradeKind::MaxHealth { add_max_health, .. }, "AddMaxHealth") = (&mut data.kind, f.key.as_str())
                {
                    *add_max_health = a.real()?;
                    return Ok(true);
                }
                if let (UpgradeKind::MaxHealth { change_type, .. }, "ChangeType") = (&mut data.kind, f.key.as_str()) {
                    *change_type = a.index()?;
                    return Ok(true);
                }
                if let (UpgradeKind::ObjectCreation { ocl }, "UpgradeObject") = (&mut data.kind, f.key.as_str()) {
                    *ocl = Some(Ref::named(a.next()?.to_string()));
                    return Ok(true);
                }
                if let UpgradeKind::CommandSet { command_set, command_set_alt, trigger_alt } = &mut data.kind {
                    match f.key.as_str() {
                        "CommandSet" => {
                            *command_set = a.next()?.to_string();
                            return Ok(true);
                        }
                        "CommandSetAlt" => {
                            *command_set_alt = a.next()?.to_string();
                            return Ok(true);
                        }
                        "TriggerAlt" => {
                            *trigger_alt = a.next()?.to_string();
                            return Ok(true);
                        }
                        _ => {}
                    }
                }
                if let (UpgradeKind::ExperienceScalar { add_xp_scalar }, "AddXPScalar") = (&mut data.kind, f.key.as_str()) {
                    *add_xp_scalar = a.real()?;
                    return Ok(true);
                }
                if let (UpgradeKind::ModelCondition { flag }, "ConditionFlag") = (&mut data.kind, f.key.as_str()) {
                    *flag = Some(a.next()?.to_string());
                    return Ok(true);
                }
                data.mux.field(&f.key, &mut a)
            })();
            match r {
                Ok(true) => {}
                Ok(false) => d.unknown_field(f),
                Err(e) => d.bad_value(f, e),
            }
        }
        data
    }
}

/// Whether `class` is one of the upgrade classes this module types.
pub(crate) fn is_typed_upgrade_class(class: &str) -> bool {
    matches!(
        class,
        "ArmorUpgrade"
            | "WeaponSetUpgrade"
            | "LocomotorSetUpgrade"
            | "MaxHealthUpgrade"
            | "ObjectCreationUpgrade"
            | "CommandSetUpgrade"
            | "ExperienceScalarUpgrade"
            | "WeaponBonusUpgrade"
            | "ModelConditionUpgrade"
    )
}
