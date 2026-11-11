//! Lane 168 (wave 19, the R18 review, R18-8): a sinking object's height goes through `set_position`, so its transform
//! follows it (`Thing::setPosition`, `Thing.cpp:177-205`). A turn after the sink (`setOrientation` reads the translation
//! from the transform, `Thing.cpp:211-258`) then keeps the sunk height.

use super::*;
use crate::body::DamageInfo as Hit;
use crate::damage::DamageType;
use crate::math::Coord3D;
use crate::template::{kind_of, ObjectTemplate};
use crate::terrain::TerrainLogic;
use std::sync::Arc;

/// A unit whose slow death sinks it one unit a frame from its first frame of death.
fn sinking_unit() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Sinker");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 8.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.die = vec![DieModuleTemplate {
        mux: DieMux { death_types: u32::MAX, veterancy_levels: u32::MAX, ..DieMux::default() },
        kind: DieTemplate::SlowDeath(Arc::new(SlowDeathTemplate {
            probability_modifier: 1,
            modifier_bonus_per_overkill_percent: 0.0,
            sink_rate: 1.0,
            sink_delay: 0,
            sink_delay_variance: 0,
            destruction_delay: 1000,
            destruction_delay_variance: 0,
            destruction_altitude: -100.0,
            fx: [Vec::new(), Vec::new(), Vec::new()],
            ocl: [Vec::new(), Vec::new(), Vec::new()],
            weapons: [Vec::new(), Vec::new(), Vec::new()],
            fling_force: 0.0,
            fling_force_variance: 0.0,
            fling_pitch: 0.0,
            fling_pitch_variance: 0.0,
            special: None,
        })),
    }];
    t
}

/// R18-8 (`Thing.cpp:177-205`): the sink's `set_position` keeps the transform's translation in step with `position`, so a
/// turn on the sinking object (`setOrientation`, which takes its position from the transform) keeps the sunk height.
/// Revert check: write `o.position.z -= sd.sink_rate` alone in `die.rs` (the transform then keeps the old height).
#[test]
fn a_turn_after_a_sink_keeps_the_sunk_height() {
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 0);
    let id = g.create_object(&Arc::new(sinking_unit()), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    g.damage_object(id, Hit { source: ObjectId::INVALID, amount: 200.0, kind: DamageType::Explosion });
    g.run_frames(3);
    let before = g.find_object(id).unwrap().position.z;
    assert!(before < 0.0, "the object is sinking: {before}");
    assert_eq!(g.find_object(id).unwrap().transform.translation().z, before, "the transform follows the sink");
    {
        let o = g.objects.get_mut(&id).unwrap();
        o.set_orientation(1.0);
    }
    assert_eq!(g.find_object(id).unwrap().position.z, before, "a turn keeps the sunk height");
}
