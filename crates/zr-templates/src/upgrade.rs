//! `UpgradeTemplate`. Ported from `Common/System/Upgrade.cpp`
//! (`UpgradeTemplate::m_upgradeFieldParseTable`, `UpgradeTemplate::UpgradeTemplate`,
//! `UpgradeCenter::newUpgrade` / `parseUpgradeDefinition`).
//!
//! `Upgrade NAME ... End` blocks. A repeated definition overlays the fields it names
//! (`parseUpgradeDefinition` finds the existing template and calls `initFromINI` on it); the first
//! definition of a name starts as a copy of `DefaultUpgrade` when that exists
//! (`UpgradeCenter::newUpgrade`).

use zr_ini::{Block, Item};

use crate::diag::{Diag, WarningKind};
use crate::names::{AcademyClassification, UpgradeType};
use crate::value::Args;

/// An upgrade definition. Defaults are the `UpgradeTemplate` constructor's.
#[derive(Debug, Clone, PartialEq)]
pub struct UpgradeTemplate {
    pub name: String,
    pub upgrade_type: UpgradeType,
    /// `DisplayName` localisation label.
    pub display_name: Option<String>,
    /// `BuildTime` in seconds (`parseReal`); `calcTimeToBuild` multiplies by the frame rate.
    pub build_time: f32,
    pub build_cost: i32,
    pub button_image: Option<String>,
    /// `ResearchSound`; `NoSound` leaves the previous value alone (`parseAudioEventRTS`).
    pub research_sound: Option<String>,
    /// `UnitSpecificSound`; same `NoSound` rule.
    pub unit_specific_sound: Option<String>,
    pub academy_classification: AcademyClassification,
}

impl UpgradeTemplate {
    /// `UpgradeTemplate::UpgradeTemplate()`.
    pub fn new(name: &str) -> Self {
        UpgradeTemplate {
            name: name.to_string(),
            upgrade_type: UpgradeType::Player,
            display_name: None,
            build_time: 0.0,
            build_cost: 0,
            button_image: None,
            research_sound: None,
            unit_specific_sound: None,
            academy_classification: AcademyClassification::None,
        }
    }

    /// `UpgradeCenter::parseUpgradeDefinition`'s `initFromINI` for one `Upgrade` block.
    pub(crate) fn apply_block(&mut self, b: &Block, d: &mut Diag) {
        d.push_scope(format!("Upgrade {}", self.name));
        for item in &b.items {
            let Item::Field(f) = item else {
                d.warn(WarningKind::Structure, "", 0, "unexpected nested block in Upgrade");
                continue;
            };
            let mut a = Args::new(&f.value);
            let r: Result<bool, String> = (|| {
                match f.key.as_str() {
                    "DisplayName" => self.display_name = Some(a.next()?.to_string()),
                    "Type" => self.upgrade_type = a.index()?,
                    "BuildTime" => self.build_time = a.real()?,
                    "BuildCost" => self.build_cost = a.int()?,
                    "ButtonImage" => self.button_image = Some(a.next()?.to_string()),
                    "ResearchSound" => {
                        // `parseAudioEventRTS`: `NoSound` leaves the event untouched.
                        let t = a.next()?;
                        if !t.eq_ignore_ascii_case("NoSound") {
                            self.research_sound = Some(t.to_string());
                        }
                    }
                    "UnitSpecificSound" => {
                        let t = a.next()?;
                        if !t.eq_ignore_ascii_case("NoSound") {
                            self.unit_specific_sound = Some(t.to_string());
                        }
                    }
                    "AcademyClassify" => self.academy_classification = a.index()?,
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
        d.pop_scope();
    }
}
