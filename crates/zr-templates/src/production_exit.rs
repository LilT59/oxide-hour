//! Production exit updates. Ported from `DefaultProductionExitUpdate.cpp`,
//! `QueueProductionExitUpdate.cpp` and `SupplyCenterProductionExitUpdate.cpp`
//! (their `buildFieldParse` tables).
//!
//! `UnitCreatePoint` and `NaturalRallyPoint` are model-space coords; the sim transforms them by the
//! producing building's transform at exit time.

use zr_ini::{Block, Item};

use crate::diag::{Diag, WarningKind};
use crate::value::{parse_coord3d, Args};

/// Which exit update class this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductionExitKind {
    Default,
    Queue,
    SupplyCenter,
    /// `SpawnPointProductionExitUpdate`.
    SpawnPoint,
}

/// The parsed fields of a production exit update.
#[derive(Debug, Clone, PartialEq)]
pub struct ProductionExitData {
    pub kind: ProductionExitKind,
    pub unit_create_point: (f32, f32, f32),
    pub natural_rally_point: (f32, f32, f32),
    /// `ExitDelay` (`Queue` only), frames.
    pub exit_delay: u32,
    /// `AllowAirborneCreation` (`Queue` only).
    pub allow_airborne_creation: bool,
    /// `InitialBurst` (`Queue` only).
    pub initial_burst: u32,
    /// `GrantTemporaryStealth` (`SupplyCenter` only), frames.
    pub grant_temporary_stealth: u32,
    /// `UseSpawnRallyPoint` (`Default` only).
    pub use_spawn_rally_point: bool,
    /// `SpawnPointBoneName` (`SpawnPoint` only).
    pub spawn_point_bone_name: String,
}

impl ProductionExitData {
    pub(crate) fn parse(class: &str, block: &Block, d: &mut Diag) -> Self {
        let kind = match class {
            "QueueProductionExitUpdate" => ProductionExitKind::Queue,
            "SupplyCenterProductionExitUpdate" => ProductionExitKind::SupplyCenter,
            "SpawnPointProductionExitUpdate" => ProductionExitKind::SpawnPoint,
            _ => ProductionExitKind::Default,
        };
        let mut p = ProductionExitData {
            kind,
            unit_create_point: (0.0, 0.0, 0.0),
            natural_rally_point: (0.0, 0.0, 0.0),
            exit_delay: 0,
            allow_airborne_creation: false,
            initial_burst: 0,
            grant_temporary_stealth: 0,
            use_spawn_rally_point: false,
            spawn_point_bone_name: String::new(),
        };
        for item in &block.items {
            let Item::Field(f) = item else {
                d.warn(WarningKind::Structure, "", 0, "unexpected nested block in a production exit update");
                continue;
            };
            let mut a = Args::new(&f.value);
            let r: Result<bool, String> = (|| {
                match f.key.as_str() {
                    "UnitCreatePoint" => p.unit_create_point = parse_coord3d(&f.value)?,
                    "NaturalRallyPoint" => p.natural_rally_point = parse_coord3d(&f.value)?,
                    "ExitDelay" => p.exit_delay = a.duration_frames()?,
                    "AllowAirborneCreation" => p.allow_airborne_creation = a.boolean()?,
                    "InitialBurst" => p.initial_burst = a.uint()?,
                    "GrantTemporaryStealth" => p.grant_temporary_stealth = a.duration_frames()?,
                    "UseSpawnRallyPoint" => p.use_spawn_rally_point = a.boolean()?,
                    "SpawnPointBoneName" => p.spawn_point_bone_name = a.next()?.to_string(),
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

/// Whether `class` is one of the production exit classes this module types.
pub(crate) fn is_typed_production_exit_class(class: &str) -> bool {
    matches!(
        class,
        "DefaultProductionExitUpdate"
            | "QueueProductionExitUpdate"
            | "SupplyCenterProductionExitUpdate"
            | "SpawnPointProductionExitUpdate"
    )
}
