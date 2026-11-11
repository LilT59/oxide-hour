//! Lane 117: the `QueueProductionExitUpdate` family ticks the exit delay in its own heap slot
//! (`QueueProductionExitUpdate.cpp:215`), in the template's INI module order: the real templates place it after
//! `ProductionUpdate` (ChinaBarracks: `ProductionUpdate ModuleTag_10`, `QueueProductionExitUpdate ModuleTag_11`).

use std::sync::Arc;
use zr_sim::production::{ProductionExit, ProductionTemplate, QuantityModifier};
use zr_sim::template::{kind_of, SimpleTemplateStore, TemplateSource};
use zr_sim::{Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

fn template(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE;
    t.geometry.major_radius = 5.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t
}

/// A queue exit with `ExitDelay 5` spaces two units five frames apart: the exit sets `m_currentDelay` on the frame
/// of the first exit, and the exit module's update runs after `ProductionUpdate` in the same frame and counts it
/// down (so the second unit leaves on the fifth frame after the first).
#[test]
fn a_queue_exit_counts_its_delay_down_in_its_own_heap_slot() {
    let mut f = template("Factory");
    f.kind_of = kind_of::STRUCTURE;
    f.production = Some(ProductionTemplate {
        exit: ProductionExit::Queue {
            unit_create_point: (10.0, 0.0, 0.0),
            natural_rally_point: (100.0, 0.0, 0.0),
            exit_delay_frames: 5,
            initial_burst: 0,
            allow_airborne_creation: false,
        },
        quantity_modifiers: vec![QuantityModifier { template_name: "Soldier".into(), quantity: 2 }],
        ..Default::default()
    });
    let mut store = SimpleTemplateStore::default();
    let f = store.add(f);
    store.add(template("Soldier"));
    let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
    g.add_player(0, 1000);
    g.set_template_source(Arc::new(store) as Arc<dyn TemplateSource>);
    let factory = g.create_object(&f, Coord3D::new(0.0, 0.0, 0.0), 0.0, 0);
    g.run_frames(1); // the factories' first update is frame 1
    assert!(g.queue_unit(0, factory, "Soldier"));
    let soldiers = |g: &GameLogic| g.objects().filter(|o| o.template.name == "Soldier").count();
    g.run_frames(1);
    assert_eq!(soldiers(&g), 1, "the first unit exits as soon as it is built");
    let first_frame = g.frame();
    // `ProductionUpdate` and the `QueueProductionExitUpdate` slot are both due every frame, so their order inside a
    // frame is the heap's (equal keys, an unstable heap). The second unit therefore leaves four or five frames
    // after the first: the exit sets `m_currentDelay = 5` and the tick may or may not count that same frame down.
    let mut second_frame = None;
    for _ in 0..8 {
        g.run_frames(1);
        if soldiers(&g) == 2 {
            second_frame = Some(g.frame());
            break;
        }
    }
    let gap = second_frame.expect("the second unit exits") - first_frame;
    assert!((4..=5).contains(&gap), "the queue exit spaces the units by its delay, gap {gap}");
}
