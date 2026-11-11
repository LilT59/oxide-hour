//! `ProductionUpdate`. Ported from `GameLogic/Object/Update/ProductionUpdate.cpp`
//! (`ProductionUpdateModuleData::buildFieldParse`, constructor, `parseAppendQuantityModifier`).

use zr_ini::{Block, Item};

use crate::diag::{Diag, WarningKind};
use crate::flags::Mask;
use crate::names::DisabledType;
use crate::value::Args;

/// `ProductionUpdateModuleData::QuantityModifier`: a template name and a count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuantityModifier {
    pub template_name: String,
    pub quantity: i32,
}

/// `ProductionUpdateModuleData`. Defaults are its constructor's.
#[derive(Debug, Clone, PartialEq)]
pub struct ProductionData {
    pub max_queue_entries: i32,
    pub num_door_animations: i32,
    /// Frames.
    pub door_opening_time: u32,
    pub door_wait_open_time: u32,
    pub door_close_time: u32,
    pub construction_complete_duration: u32,
    pub quantity_modifiers: Vec<QuantityModifier>,
    /// `DisabledTypesToProcess` (default `DISABLED_HELD`).
    pub disabled_types_to_process: Mask<DisabledType>,
}

impl ProductionData {
    pub(crate) fn parse(block: &Block, d: &mut Diag) -> Self {
        let mut p = ProductionData {
            max_queue_entries: 9,
            num_door_animations: 0,
            door_opening_time: 0,
            door_wait_open_time: 0,
            door_close_time: 0,
            construction_complete_duration: 0,
            quantity_modifiers: Vec::new(),
            disabled_types_to_process: Mask::EMPTY.with(DisabledType::Held),
        };
        for item in &block.items {
            let Item::Field(f) = item else {
                d.warn(WarningKind::Structure, "", 0, "unexpected nested block in ProductionUpdate");
                continue;
            };
            let mut a = Args::new(&f.value);
            let r: Result<bool, String> = (|| {
                match f.key.as_str() {
                    "MaxQueueEntries" => p.max_queue_entries = a.int()?,
                    "NumDoorAnimations" => p.num_door_animations = a.int()?,
                    "DoorOpeningTime" => p.door_opening_time = a.duration_frames()?,
                    "DoorWaitOpenTime" => p.door_wait_open_time = a.duration_frames()?,
                    "DoorCloseTime" => p.door_close_time = a.duration_frames()?,
                    "ConstructionCompleteDuration" => p.construction_complete_duration = a.duration_frames()?,
                    "QuantityModifier" => {
                        let template_name = a.next()?.to_string();
                        let quantity = match a.next_opt() {
                            Some(t) => crate::value::scan_int(t)?,
                            None => 1,
                        };
                        p.quantity_modifiers.push(QuantityModifier { template_name, quantity });
                    }
                    "DisabledTypesToProcess" => p.disabled_types_to_process = p.disabled_types_to_process.parse(a.rest())?,
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
