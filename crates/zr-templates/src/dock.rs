//! Supply dock updates. Ported from `DockUpdate/DockUpdate.cpp` (`DockUpdateModuleData::
//! buildFieldParse`), `SupplyWarehouseDockUpdate.cpp` and `SupplyCenterDockUpdate.cpp`.
//!
//! `DockUpdate` positions (`NumberApproachPositions`) are generated from the building geometry in
//! the engine; the sim uses the building position instead (see `zr-sim::supply`).

use zr_ini::{Block, Item};

use crate::diag::{Diag, WarningKind};
use crate::value::Args;

/// Which supply dock this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockKind {
    /// `SupplyWarehouseDockUpdate`: hands out boxes.
    SupplyWarehouse,
    /// `SupplyCenterDockUpdate`: takes boxes and pays money.
    SupplyCenter,
    /// `RailedTransportDockUpdate`: a `DockUpdate` for railed transport.
    RailedTransport,
}

/// The parsed fields of a supply dock.
#[derive(Debug, Clone, PartialEq)]
pub struct DockData {
    pub kind: DockKind,
    /// `StartingBoxes` (warehouse).
    pub starting_boxes: i32,
    /// `DeleteWhenEmpty` (warehouse).
    pub delete_when_empty: bool,
    /// `GrantTemporaryStealth` (center), frames.
    pub grant_temporary_stealth: u32,
    /// `NumberApproachPositions` (`DockUpdate`).
    pub number_approach_positions: i32,
    /// `AllowsPassthrough` (`DockUpdate`).
    pub allows_passthrough: bool,
    /// `PullInsideDuration` (railed transport), frames.
    pub pull_inside_duration: u32,
    /// `PushOutsideDuration` (railed transport), frames.
    pub push_outside_duration: u32,
    /// `ToleranceDistance` (railed transport).
    pub tolerance_distance: f32,
}

impl DockData {
    pub(crate) fn parse(class: &str, block: &Block, d: &mut Diag) -> Self {
        let kind = match class {
            "SupplyCenterDockUpdate" => DockKind::SupplyCenter,
            "RailedTransportDockUpdate" => DockKind::RailedTransport,
            _ => DockKind::SupplyWarehouse,
        };
        let mut p = DockData {
            kind,
            starting_boxes: 1,
            delete_when_empty: false,
            grant_temporary_stealth: 0,
            number_approach_positions: 0,
            allows_passthrough: true, // `DockUpdateModuleData` ctor: m_isAllowPassthrough = TRUE
            pull_inside_duration: 0,
            push_outside_duration: 0,
            tolerance_distance: 50.0,
        };
        for item in &block.items {
            let Item::Field(f) = item else {
                d.warn(WarningKind::Structure, "", 0, "unexpected nested block in a dock update");
                continue;
            };
            let mut a = Args::new(&f.value);
            let r: Result<bool, String> = (|| {
                match f.key.as_str() {
                    "StartingBoxes" => p.starting_boxes = a.int()?,
                    "DeleteWhenEmpty" => p.delete_when_empty = a.boolean()?,
                    "GrantTemporaryStealth" => p.grant_temporary_stealth = a.duration_frames()?,
                    "NumberApproachPositions" => p.number_approach_positions = a.int()?,
                    "AllowsPassthrough" => p.allows_passthrough = a.boolean()?,
                    "PullInsideDuration" => p.pull_inside_duration = a.duration_frames()?,
                    "PushOutsideDuration" => p.push_outside_duration = a.duration_frames()?,
                    "ToleranceDistance" => p.tolerance_distance = a.real()?,
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

/// Whether `class` is one of the supply dock classes this module types.
pub(crate) fn is_typed_dock_class(class: &str) -> bool {
    matches!(class, "SupplyWarehouseDockUpdate" | "SupplyCenterDockUpdate" | "RailedTransportDockUpdate")
}
