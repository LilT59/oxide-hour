//! Production, money and production exits (synthetic data, no game install).

use std::sync::Arc;

use zr_sim::production::{Prerequisite, ProductionExit, ProductionTemplate, QuantityModifier};
use zr_sim::template::{LocomotorKind, LocomotorParams};
use zr_sim::{Command, Coord3D, GameLogic, ObjectId, ObjectTemplate, TemplateSource, TerrainLogic};

/// A tiny name -> `Arc<ObjectTemplate>` source for the tests.
struct FixedStore(Vec<Arc<ObjectTemplate>>);

impl TemplateSource for FixedStore {
    fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
        self.0.iter().find(|t| t.name == name).cloned()
    }
}

fn factory() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Factory");
    t.max_health = 1000.0;
    t.initial_health = 1000.0;
    t.production = Some(ProductionTemplate {
        max_queue_entries: 9,
        quantity_modifiers: Vec::new(),
        exit: ProductionExit::Default { unit_create_point: (10.0, 0.0, 0.0), natural_rally_point: (100.0, 0.0, 0.0) },
        ..Default::default()
    });
    t
}

fn unit(name: &str, cost: i32, frames: u32) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.build_cost = cost;
    t.build_time_frames = frames;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Legs, 30.0, 100.0, 100.0, 360.0));
    t
}

/// A world with a factory (owned by player 0, 1000 cash) and a `Soldier` to build.
fn setup(unit_cost: i32, unit_frames: u32) -> (GameLogic, ObjectId) {
    let f = Arc::new(factory());
    let u = Arc::new(unit("Soldier", unit_cost, unit_frames));
    let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
    g.add_player(0, 1000);
    g.set_template_source(Arc::new(FixedStore(vec![f.clone(), u])));
    let factory = g.create_object(&f, Coord3D::new(0.0, 0.0, 0.0), 0.0, 0);
    // `ProductionUpdate` has no constructor wake: `registerObject` schedules it for frame 1 (GameLogic.cpp:4018-4045),
    // so the factory's first update is frame 1. The frame is spent before the tests queue anything.
    g.run_frames(1);
    (g, factory)
}

#[test]
fn queue_withdraws_and_cancel_refunds() {
    let (mut g, factory) = setup(300, 10);
    assert_eq!(g.player(0).unwrap().money, 1000);
    assert!(g.queue_unit(0, factory, "Soldier"));
    assert_eq!(g.player(0).unwrap().money, 700, "cost withdrawn at queue time");
    assert!(g.cancel_unit(0, factory, 0));
    assert_eq!(g.player(0).unwrap().money, 1000, "cancel refunds the cost");
    assert!(g.find_object(factory).unwrap().production.as_ref().unwrap().queue.is_empty());
    assert!(!g.cancel_unit(0, factory, 0), "a bad index is a no-op");
    // the wrong player cannot queue or cancel
    assert!(!g.queue_unit(1, factory, "Soldier"));
    assert!(g.queue_unit(0, factory, "Soldier"));
    assert!(!g.cancel_unit(1, factory, 0));
    assert_eq!(g.player(0).unwrap().money, 700);
}

/// `ProductionUpdate::queueCreateUnit` runs `BuildAssistant::canMakeUnit` first (`ProductionUpdate.cpp:358`,
/// `BuildAssistant.cpp:1313-1343`): a factory whose command set lacks the unit (`isPossibleToMakeUnit`), or
/// that is script-disabled (`OBJECT_STATUS_SCRIPT_DISABLED`), queues nothing and takes no money.
#[test]
fn queue_checks_command_set_and_script_disable() {
    struct CommandSetStore(Vec<Arc<ObjectTemplate>>);
    impl TemplateSource for CommandSetStore {
        fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
            self.0.iter().find(|t| t.name == name).cloned()
        }
        // The factory's command set holds only the `Soldier` build button.
        fn can_make_unit(&self, builder: &str, unit: &str) -> Option<bool> {
            Some(builder == "Factory" && unit == "Soldier")
        }
    }
    let f = Arc::new(factory());
    let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
    g.add_player(0, 1000);
    g.set_template_source(Arc::new(CommandSetStore(vec![f.clone(), Arc::new(unit("Soldier", 300, 10)), Arc::new(unit("Tank", 300, 10))])));
    let factory = g.create_object(&f, Coord3D::new(0.0, 0.0, 0.0), 0.0, 0);

    assert!(!g.queue_unit(0, factory, "Tank"), "not in the factory's command set");
    assert_eq!(g.player(0).unwrap().money, 1000);

    g.set_disabled(factory, zr_sim::DisabledType::ScriptDisabled);
    assert!(!g.queue_unit(0, factory, "Soldier"), "script-disabled factory refuses");
    assert_eq!(g.player(0).unwrap().money, 1000);
    g.clear_disabled(factory, zr_sim::DisabledType::ScriptDisabled);

    assert!(g.queue_unit(0, factory, "Soldier"));
    assert_eq!(g.player(0).unwrap().money, 700);
}

#[test]
fn queue_full_and_no_money_are_rejected() {
    let (mut g, factory) = setup(0, 10);
    for _ in 0..9 {
        assert!(g.queue_unit(0, factory, "Soldier"));
    }
    assert!(!g.queue_unit(0, factory, "Soldier"), "queue is full (default 9)");

    let (mut g2, factory2) = setup(2000, 10);
    assert!(!g2.queue_unit(0, factory2, "Soldier"), "cannot afford 2000 with 1000");
    assert_eq!(g2.player(0).unwrap().money, 1000);
}

#[test]
fn build_time_is_frames() {
    let (mut g, factory) = setup(0, 10);
    g.submit(0, Command::QueueUnit { factory, template: "Soldier".into() });
    g.run_frames(9);
    assert!(g.objects().all(|o| o.template.name == "Factory"), "not built before the build time");
    g.run_frames(1);
    assert_eq!(g.objects().filter(|o| o.template.name == "Soldier").count(), 1, "built after 10 frames");
}

#[test]
fn quantity_modifier_builds_two_for_one_price() {
    let mut f = factory();
    f.production.as_mut().unwrap().quantity_modifiers.push(QuantityModifier { template_name: "Soldier".into(), quantity: 2 });
    let f = Arc::new(f);
    let u = Arc::new(unit("Soldier", 300, 10));
    let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
    g.add_player(0, 1000);
    g.set_template_source(Arc::new(FixedStore(vec![f.clone(), u])));
    let factory = g.create_object(&f, Coord3D::new(0.0, 0.0, 0.0), 0.0, 0);
    g.run_frames(1); // the factory's first `ProductionUpdate` is frame 1 (GameLogic.cpp:4018-4045)
    assert!(g.queue_unit(0, factory, "Soldier"));
    assert_eq!(g.player(0).unwrap().money, 700, "one price");
    g.run_frames(10);
    assert_eq!(g.objects().filter(|o| o.template.name == "Soldier").count(), 2, "two units for one price");
}

#[test]
fn prerequisites_block_until_owned() {
    let f = Arc::new(factory());
    let mut u = unit("Soldier", 100, 5);
    u.prerequisites = vec![Prerequisite::AnyOfObjects(vec!["Barracks".into()])];
    let u = Arc::new(u);
    let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
    g.add_player(0, 1000);
    g.set_template_source(Arc::new(FixedStore(vec![f.clone(), u])));
    let factory = g.create_object(&f, Coord3D::new(0.0, 0.0, 0.0), 0.0, 0);
    assert!(!g.queue_unit(0, factory, "Soldier"), "no Barracks yet");
    assert_eq!(g.player(0).unwrap().money, 1000, "nothing withdrawn");
    let mut barracks = ObjectTemplate::new("Barracks");
    barracks.max_health = 100.0;
    g.create_object(&Arc::new(barracks), Coord3D::new(50.0, 0.0, 0.0), 0.0, 0);
    assert!(g.queue_unit(0, factory, "Soldier"), "prerequisite satisfied");
    assert_eq!(g.player(0).unwrap().money, 900);
}

#[test]
fn exit_point_and_rally_move() {
    let (mut g, factory) = setup(0, 5);
    g.submit(0, Command::QueueUnit { factory, template: "Soldier".into() });
    g.run_frames(6);
    let soldier = g.objects().find(|o| o.template.name == "Soldier").expect("soldier built");
    assert!(soldier.position.x >= 10.0 - 0.5, "spawned at the exit point: {:?}", soldier.position);
    let (id, x0) = (soldier.id, soldier.position.x);
    let rally = Coord3D::new(100.0, 0.0, 0.0);
    let before = soldier.position.dist_2d(&rally);
    g.run_frames(30);
    let soldier = g.find_object(id).unwrap();
    assert!(soldier.position.x > x0 + 5.0, "moved toward the natural rally point: {:?}", soldier.position);
    assert!(soldier.position.dist_2d(&rally) < before, "closing on the rally point");
}

#[test]
fn factory_death_refunds_the_queue() {
    use zr_sim::body::{DamageInfo, DamageType};
    let (mut g, factory) = setup(300, 10);
    assert!(g.queue_unit(0, factory, "Soldier"));
    assert_eq!(g.player(0).unwrap().money, 700);
    g.damage_object(factory, DamageInfo { source: zr_sim::ObjectId::INVALID, amount: 99999.0, kind: DamageType::Explosion });
    assert_eq!(g.player(0).unwrap().money, 1000, "the queued unit is refunded on death");
}

#[test]
fn queue_exit_delay_spaces_units() {
    let mut f = factory();
    let prod = f.production.as_mut().unwrap();
    prod.quantity_modifiers.push(QuantityModifier { template_name: "Soldier".into(), quantity: 2 });
    prod.exit = ProductionExit::Queue {
        unit_create_point: (10.0, 0.0, 0.0),
        natural_rally_point: (100.0, 0.0, 0.0),
        exit_delay_frames: 5,
        initial_burst: 0,
        allow_airborne_creation: false,
    };
    let f = Arc::new(f);
    let u = Arc::new(unit("Soldier", 0, 1));
    let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
    g.add_player(0, 1000);
    g.set_template_source(Arc::new(FixedStore(vec![f.clone(), u])));
    let factory = g.create_object(&f, Coord3D::new(0.0, 0.0, 0.0), 0.0, 0);
    g.run_frames(1); // the factory's first `ProductionUpdate` is frame 1 (GameLogic.cpp:4018-4045)
    assert!(g.queue_unit(0, factory, "Soldier"));

    let soldiers =|g: &GameLogic| g.objects().filter(|o| o.template.name == "Soldier").count();
    g.run_frames(1);
    assert_eq!(soldiers(&g), 1, "the first unit exits as soon as it is built");
    let first_frame = g.frame();
    // `ProductionUpdate` and the `QueueProductionExitUpdate` slot are both due every frame, so their order inside
    // a frame is the sleepy heap's (equal keys, an unstable heap), as in C++. The second unit leaves four or five
    // frames after the first: `exitObjectViaDoor` sets `m_currentDelay`, whose tick may land in the same frame.
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

#[test]
fn rally_point_override_is_used() {
    let (mut g, factory) = setup(0, 5);
    assert!(g.set_rally_point(0, factory, Coord3D::new(0.0, 150.0, 0.0)));
    g.submit(0, Command::QueueUnit { factory, template: "Soldier".into() });
    g.run_frames(6);
    let soldier = g.objects().find(|o| o.template.name == "Soldier").unwrap();
    let id = soldier.id;
    // The exit path is the natural rally point (120,0) first, then the rally point (`aiFollowExitProductionPath`).
    g.run_frames(30);
    let soldier = g.find_object(id).unwrap();
    assert!(soldier.position.x > 20.0 && soldier.position.y.abs() < 5.0, "heads for the natural rally point first: {:?}", soldier.position);
    g.run_frames(400);
    let soldier = g.find_object(id).unwrap();
    assert!(soldier.position.y > 100.0, "then walks to the player rally point: {:?}", soldier.position);
}
