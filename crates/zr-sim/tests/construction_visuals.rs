//! A structure under construction shows the construction art, not its damage art
//! (`ActiveBody::internalChangeHealth` skips `evaluateVisualCondition` while
//! OBJECT_STATUS_UNDER_CONSTRUCTION; `DozerAIUpdate::construct` starts the building at one hit point).
//! Synthetic data only.

use std::sync::Arc;

use zr_sim::condition::{ACTIVELY_BEING_CONSTRUCTED, AWAITING_CONSTRUCTION, DAMAGED, PARTIALLY_CONSTRUCTED, REALLYDAMAGED, RUBBLE};
use zr_sim::template::{kind_of, GeometryShape, LocomotorKind, LocomotorParams};
use zr_sim::{Coord3D, GameLogic, ObjectTemplate, TemplateSource, TerrainLogic};

struct FixedStore(Vec<Arc<ObjectTemplate>>);

impl TemplateSource for FixedStore {
    fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
        self.0.iter().find(|t| t.name == name).cloned()
    }
}

fn dozer_template() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Dozer");
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.geometry.major_radius = 5.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 60.0, 100.0, 100.0, 360.0));
    t.kind_of |= kind_of::DOZER;
    t.dozer = Some(zr_sim::dozer::DozerTemplate { repair_health_percent_per_second: 0.0, bored_time_frames: 1.0, bored_range: 0.0 });
    t
}

fn barracks() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Barracks");
    t.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;
    t.max_health = 500.0;
    t.initial_health = 500.0;
    t.geometry.shape = GeometryShape::Box;
    t.geometry.major_radius = 15.0;
    t.geometry.minor_radius = 15.0;
    t.build_cost = 300;
    t.build_time_frames = 60;
    t
}

fn world() -> GameLogic {
    let store = FixedStore(vec![Arc::new(dozer_template()), Arc::new(barracks())]);
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 1000);
    g.set_template_source(Arc::new(store));
    g
}

/// The construction stages show `PARTIALLY_CONSTRUCTED` / `ACTIVELY_BEING_CONSTRUCTED` with no damage
/// art, even though the building's health is still at one hit point (RUBBLE by health alone).
#[test]
fn no_damage_art_while_under_construction() {
    let mut g = world();
    let t = g.find_template("Dozer").expect("dozer template");
    let dozer = g.create_object(&t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let b = g.construct(dozer, "Barracks", Coord3D::new(160.0, 100.0, 0.0), 0.0).expect("construct");

    let mut under_construction_frames = 0;
    for _ in 0..200 {
        g.run_frames(1);
        let Some(o) = g.find_object(b) else { break };
        if !o.under_construction {
            break;
        }
        let flags = g.model_conditions(b).expect("flags");
        // `DozerAIUpdate::construct` starts AWAITING_CONSTRUCTION; the builder's arrival at the start point
        // swaps it for PARTIALLY_CONSTRUCTED | ACTIVELY_BEING_CONSTRUCTED (`DozerAIUpdate.cpp:356-360`).
        if g.find_object(b).is_some_and(|o| o.construction_arrived) {
            assert!(flags.test(PARTIALLY_CONSTRUCTED) && flags.test(ACTIVELY_BEING_CONSTRUCTED), "construction stage bits");
        } else {
            assert!(flags.test(AWAITING_CONSTRUCTION) && !flags.test(PARTIALLY_CONSTRUCTED), "awaiting bits before arrival");
        }
        assert!(!flags.test(DAMAGED) && !flags.test(REALLYDAMAGED) && !flags.test(RUBBLE), "damage art shown during construction");
        under_construction_frames += 1;
    }
    assert!(under_construction_frames > 10, "the building stayed under construction for {under_construction_frames} frames");

    // Completed: the construction bits clear and the (full-health) building shows no damage art.
    g.run_frames(2);
    let o = g.find_object(b).expect("building");
    assert!(!o.under_construction);
    let flags = g.model_conditions(b).expect("flags");
    assert!(!flags.test(PARTIALLY_CONSTRUCTED) && !flags.test(RUBBLE));
}
