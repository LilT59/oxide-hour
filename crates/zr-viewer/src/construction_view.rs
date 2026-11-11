//! Construction presentation: the building rises out of the ground as it is built.
//!
//! Ported from `W3DModelDraw::adjustTransformMtx` (`Core/GameEngineDevice/Source/W3DDevice/GameClient/
//! Drawable/Draw/W3DModelDraw.cpp`): a condition state whose `Flags` include
//! `ADJUST_HEIGHT_BY_CONSTRUCTION_PERCENT` moves the model down by
//! `-height + height * percent / 100`, where `height` is `GeometryInfo::getMaxHeightAbovePosition`.
//! The Alpine command centre's `AWAITING_CONSTRUCTION PARTIALLY_CONSTRUCTED ACTIVELY_BEING_CONSTRUCTED`
//! states set that flag, so the building sits fully below its footprint at 0% and reaches its own
//! height at 100%. Another draw module gets the same offset only when its own selected state carries the flag
//! (`construct_modules`); a power plant's fence, scaffolding and crane states do not, so they stay put.

use zr_sim::template::{Geometry, GeometryShape};
use zr_sim::ObjectId;

use crate::anim;
use crate::sim_view::SimRes;

/// The `Flags` entry that asks for the construction height offset.
pub(crate) const ADJUST_HEIGHT: &str = "ADJUST_HEIGHT_BY_CONSTRUCTION_PERCENT";

/// `GeometryInfo::getMaxHeightAbovePosition` (`GeometryInfo` in `Geometry.cpp:210`): a sphere reaches
/// its major radius, a box or cylinder its height.
pub(crate) fn max_height_above_position(g: &Geometry) -> f32 {
    match g.shape {
        GeometryShape::Sphere => g.major_radius,
        GeometryShape::Box | GeometryShape::Cylinder => g.height,
    }
}

/// Whether a condition state's `Flags` ask for the construction height offset.
pub(crate) fn adjusts_height(flags: &[String]) -> bool {
    flags.iter().any(|f| f == ADJUST_HEIGHT)
}

/// `W3DModelDraw::adjustTransformMtx`'s `Translate_Z` for an object at its construction percent: zero
/// once complete (`CONSTRUCTION_COMPLETE`), `-height + height * pct / 100` while under construction.
pub(crate) fn lift_at_percent(geometry: &Geometry, percent: f32, under_construction: bool) -> f32 {
    if !under_construction {
        return 0.0;
    }
    let height = max_height_above_position(geometry);
    let pct = percent.clamp(0.0, 100.0);
    -height + height * pct / 100.0
}

/// Vertical offset (world units, Z up) the object's selected condition state applies to its model.
/// Zero for everything that does not use the construction height adjustment.
pub(crate) fn height_offset(sim: &SimRes, anim_cache: &mut anim::AnimCache, ini: &zr_ini::IniDb, id: ObjectId) -> f32 {
    let Some(o) = sim.game.logic().find_object(id) else { return 0.0 };
    // `Object::getConstructionPercent` is `CONSTRUCTION_COMPLETE` (-1) once built, and the flag is only
    // ever selected while the building is under construction.
    if !o.under_construction {
        return 0.0;
    }
    let flags = anim::view_conditions(sim, id, o.model_conditions);
    let adjusts = anim_cache
        .states(ini, &o.template.name)
        .and_then(|s| s.find_best_state(&flags).map(|st| adjusts_height(&st.flags)))
        .unwrap_or(false);
    if !adjusts {
        return 0.0;
    }
    lift_at_percent(&o.template.geometry, o.construction_percent, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn height_follows_the_shape() {
        let box_geom = Geometry { shape: GeometryShape::Box, major_radius: 15.0, minor_radius: 15.0, height: 20.0, is_small: false };
        let sphere = Geometry { shape: GeometryShape::Sphere, major_radius: 3.0, minor_radius: 3.0, height: 10.0, is_small: false };
        assert_eq!(max_height_above_position(&box_geom), 20.0);
        assert_eq!(max_height_above_position(&sphere), 3.0);
    }

    #[test]
    fn the_building_rises_from_below_its_footprint() {
        let g = Geometry { shape: GeometryShape::Box, major_radius: 15.0, minor_radius: 15.0, height: 20.0, is_small: false };
        assert_eq!(lift_at_percent(&g, 0.0, true), -20.0);
        assert_eq!(lift_at_percent(&g, 50.0, true), -10.0);
        assert_eq!(lift_at_percent(&g, 100.0, true), 0.0);
        assert_eq!(lift_at_percent(&g, 100.0, false), 0.0, "complete structures are not lifted");
        assert!(adjusts_height(&["ADJUST_HEIGHT_BY_CONSTRUCTION_PERCENT".to_string()]));
        assert!(!adjusts_height(&["START_FRAME_LAST".to_string()]));
    }
}
