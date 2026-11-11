//! The construction model conditions over a whole build: the structure is `AWAITING_CONSTRUCTION` until
//! its builder reaches the start point, then `PARTIALLY_CONSTRUCTED | ACTIVELY_BEING_CONSTRUCTED`, and
//! the builder shows `ACTIVELY_CONSTRUCTING` while it builds at its dock (`DozerAIUpdate.cpp:356-360`,
//! `:524-528`, `:565-572`, `:1737-1744`). Synthetic data only.

use std::sync::Arc;

use zr_sim::condition::{ACTIVELY_BEING_CONSTRUCTED, ACTIVELY_CONSTRUCTING, AWAITING_CONSTRUCTION, PARTIALLY_CONSTRUCTED};
use zr_sim::dozer::DozerActionSub;
use zr_sim::template::{kind_of, GeometryShape, LocomotorKind, LocomotorParams};
use zr_sim::{Coord3D, GameLogic, ObjectId, ObjectTemplate, TemplateSource, TerrainLogic};

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
    t.geometry.minor_radius = 5.0;
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

#[test]
fn awaiting_then_partial_then_complete_with_the_builder_states() {
    let mut g = world();
    let t = g.find_template("Dozer").expect("dozer template");
    let d: ObjectId = g.create_object(&t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let b = g.construct(d, "Barracks", Coord3D::new(200.0, 100.0, 0.0), 0.0).expect("construct");

    // At the order: the structure awaits its builder, and the dozer is not yet building. The bits are
    // derived once per frame, so the first frame sets them.
    g.run_frames(1);
    let flags = g.model_conditions(b).expect("flags");
    assert!(flags.test(AWAITING_CONSTRUCTION) && !flags.test(PARTIALLY_CONSTRUCTED) && !flags.test(ACTIVELY_BEING_CONSTRUCTED));
    assert!(!g.model_conditions(d).expect("dozer flags").test(ACTIVELY_CONSTRUCTING));

    // Arrival at the start point swaps AWAITING for PARTIALLY | ACTIVELY (the percent is still 0).
    let mut arrived_at = None;
    for frame in 1..=600u32 {
        g.run_frames(1);
        if g.find_object(b).is_some_and(|o| o.construction_arrived) {
            arrived_at = Some(frame);
            break;
        }
    }
    let arrived_at = arrived_at.expect("the dozer reached the start point");
    let flags = g.model_conditions(b).expect("flags");
    assert!(!flags.test(AWAITING_CONSTRUCTION), "arrival clears AWAITING (frame {arrived_at})");
    assert!(flags.test(PARTIALLY_CONSTRUCTED) && flags.test(ACTIVELY_BEING_CONSTRUCTED));

    // While the dozer builds at its dock it shows ACTIVELY_CONSTRUCTING; the structure keeps its bits.
    let mut building_frames = 0;
    for _ in 0..600 {
        g.run_frames(1);
        let Some(o) = g.find_object(b) else { break };
        if !o.under_construction {
            break;
        }
        let dozer = g.find_object(d).expect("dozer");
        if dozer.ai.dozer_action.sub == DozerActionSub::DoBuildAtDock {
            building_frames += 1;
            assert!(g.model_conditions(d).expect("dozer flags").test(ACTIVELY_CONSTRUCTING), "dozer builds at its dock");
        }
    }
    assert!(building_frames > 10, "the dozer built for {building_frames} frames");

    // Completion clears the structure's construction bits and the dozer's ACTIVELY_CONSTRUCTING.
    g.run_frames(2);
    assert!(!g.find_object(b).expect("building").under_construction);
    let flags = g.model_conditions(b).expect("flags");
    assert!(!flags.test(AWAITING_CONSTRUCTION) && !flags.test(PARTIALLY_CONSTRUCTED) && !flags.test(ACTIVELY_BEING_CONSTRUCTED));
    assert!(!g.model_conditions(d).expect("dozer flags").test(ACTIVELY_CONSTRUCTING));
}
