//! Dozer construction, supply gathering and power (synthetic data, no game install).

use std::sync::Arc;

use zr_sim::supply::{DockKind, DockTemplate, SupplyTruckTemplate};
use zr_sim::template::{kind_of, GeometryShape, LocomotorKind, LocomotorParams};
use zr_sim::{Coord3D, GameLogic, GameRules, ObjectTemplate, TemplateSource, TerrainLogic};

/// Dock bones for a synthetic warehouse (`DockUpdate::loadDockPositions`, DockUpdate.cpp:492-540). A boneless dock
/// docks at the docker's own position (`getDockPosition`, DockUpdate.cpp:279-288) and the pickup radius
/// (SupplyWarehouseDockUpdate.cpp:93-106) is then met only by luck. The real warehouses carry these bones
/// (zr-game `dock_bones.rs`). The offsets are synthetic, object-local, sized to a 10-unit footprint.
fn warehouse_bones() -> zr_sim::dock::DockBones {
    zr_sim::dock::DockBones {
        enter: Coord3D::new(16.0, 0.0, 0.0),
        dock: Coord3D::new(11.0, 0.0, 0.0),
        exit: Coord3D::new(16.0, 6.0, 0.0),
        approach: (0..5).map(|i| Coord3D::new(24.0, -12.0 + 6.0 * i as f32, 0.0)).collect(),
    }
}

struct FixedStore(Vec<Arc<ObjectTemplate>>);

impl TemplateSource for FixedStore {
    fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
        self.0.iter().find(|t| t.name == name).cloned()
    }
}

fn mobile(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.geometry.major_radius = 5.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 60.0, 100.0, 100.0, 360.0));
    // Every shipped mobile template carries `PhysicsBehavior` (`PhysicsUpdate.h`): without it the sim's
    // reduced kinematic mover stands in for the locomotor + physics, which turns through a goal instead
    // of arriving (a tank cannot reach a point inside its turning circle once it is moving).
    t.physics = Some(zr_sim::physics::PhysicsTemplate::default());
    t
}

fn structure(name: &str, cost: i32, frames: u32) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;
    t.max_health = 500.0;
    t.initial_health = 500.0;
    t.geometry.shape = GeometryShape::Box;
    t.geometry.major_radius = 15.0;
    t.geometry.minor_radius = 15.0;
    t.build_cost = cost;
    t.build_time_frames = frames;
    t
}

fn world(templates: Vec<ObjectTemplate>, money: u32) -> GameLogic {
    let store = FixedStore(templates.into_iter().map(Arc::new).collect());
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, money);
    g.set_template_source(Arc::new(store));
    g
}

#[test]
fn dozer_constructs_a_building() {
    let mut dozer_t = mobile("Dozer");
    // `DozerAIUpdate` module data: the dozer machine (and the construction advance) lives in it.
    dozer_t.dozer = Some(zr_sim::dozer::DozerTemplate::default());
    let mut g = world(vec![dozer_t, structure("Barracks", 300, 10)], 1000);
    let dozer = g.create_object(&g.find_template("Dozer").unwrap(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    // The dozer starts just outside the footprint, so it builds immediately.
    let site = Coord3D::new(200.0, 200.0, 0.0);
    let id = g.construct(dozer, "Barracks", site, 0.0).expect("construct");
    assert_eq!(g.player(0).unwrap().money, 700, "cost withdrawn at placement");
    let b = g.find_object(id).unwrap();
    assert!(b.under_construction, "starts under construction");
    assert_eq!(b.health(), 1.0, "starts at 1 hp");
    assert!(b.template.locomotor.is_none());
    g.run_frames(120);
    let b = g.find_object(id).unwrap();
    assert!(!b.under_construction, "finished");
    assert_eq!(b.construction_percent, 100.0);
    assert_eq!(b.health(), 500.0, "healed to full");
}

#[test]
fn construct_rejects_overlap_and_off_map() {
    let mut g = world(vec![mobile("Dozer"), structure("Barracks", 300, 10)], 1000);
    let dozer = g.create_object(&g.find_template("Dozer").unwrap(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    // An existing building blocks the site.
    let existing = g.find_template("Barracks").unwrap();
    g.create_object(&existing, Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    assert!(g.construct(dozer, "Barracks", Coord3D::new(205.0, 200.0, 0.0), 0.0).is_none(), "overlap");
    assert_eq!(g.player(0).unwrap().money, 1000, "nothing withdrawn");
    // Off the map.
    assert!(g.construct(dozer, "Barracks", Coord3D::new(-50.0, 10.0, 0.0), 0.0).is_none());
    // A legal spot works.
    assert!(g.construct(dozer, "Barracks", Coord3D::new(300.0, 100.0, 0.0), 0.0).is_some());
}

#[test]
fn cancel_construct_refunds_and_removes() {
    let mut g = world(vec![mobile("Dozer"), structure("Barracks", 300, 10)], 1000);
    let dozer = g.create_object(&g.find_template("Dozer").unwrap(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let id = g.construct(dozer, "Barracks", Coord3D::new(200.0, 200.0, 0.0), 0.0).unwrap();
    assert_eq!(g.player(0).unwrap().money, 700);
    assert!(g.cancel_construct(dozer));
    assert_eq!(g.player(0).unwrap().money, 1000, "refunded");
    g.update();
    assert!(g.find_object(id).is_none(), "unfinished building removed");
}

#[test]
fn power_tally_and_low_power_factor() {
    let mut plant = structure("PowerPlant", 0, 0);
    plant.energy_production = 10;
    let mut consumer = structure("Consumer", 0, 0);
    consumer.energy_production = -5;
    let mut g = world(vec![plant, consumer, mobile("Dozer")], 0);
    let plant = g.find_template("PowerPlant").unwrap();
    let consumer = g.find_template("Consumer").unwrap();

    g.create_object(&plant, Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    assert_eq!(g.energy_tally(0), (10, 0));
    assert_eq!(g.energy_supply_ratio(0), 10.0);
    assert!(g.has_sufficient_power(0));

    g.create_object(&consumer, Coord3D::new(100.0, 50.0, 0.0), 0.0, 0);
    assert_eq!(g.energy_tally(0), (10, 5));
    assert_eq!(g.energy_supply_ratio(0), 2.0);
    assert!(g.has_sufficient_power(0));

    // Under construction structures do not contribute power yet.
    let dozer = g.create_object(&g.find_template("Dozer").unwrap(), Coord3D::new(280.0, 300.0, 0.0), 0.0, 0);
    let plant2 = g.construct(dozer, "PowerPlant", Coord3D::new(300.0, 300.0, 0.0), 0.0);
    assert!(plant2.is_some());
    assert_eq!(g.energy_tally(0), (10, 5), "the unfinished plant contributes nothing");

    // The low-power factor from `ThingTemplate::calcTimeToBuild`.
    let rules = GameRules {
        low_energy_penalty_modifier: 0.5,
        min_low_energy_production_speed: 0.25,
        max_low_energy_production_speed: 0.8,
        ..GameRules::default()
    };
    assert_eq!(rules.low_power_penalty(1.0), 1.0);
    assert_eq!(rules.low_power_penalty(0.0), 0.5);
    assert_eq!(rules.low_power_penalty(0.5), 0.75);
}

#[test]
fn supply_truck_harvests_and_delivers() {
    let mut wh = structure("Warehouse", 0, 0);
    wh.geometry.major_radius = 10.0;
    wh.geometry.minor_radius = 10.0;
    wh.dock = Some(DockTemplate { kind: DockKind::Warehouse, starting_boxes: 2, delete_when_empty: false, number_approach_positions: 5, allows_passthrough: true });
    wh.dock_bones = warehouse_bones();
    let mut center = structure("SupplyCenter", 0, 0);
    center.geometry.major_radius = 10.0;
    center.geometry.minor_radius = 10.0;
    center.dock = Some(DockTemplate { kind: DockKind::Center, starting_boxes: 0, delete_when_empty: false, number_approach_positions: 5, allows_passthrough: true });
    let mut truck = mobile("SupplyTruck");
    truck.supply_truck = Some(SupplyTruckTemplate { max_boxes: 2, center_action_delay: 1, warehouse_action_delay: 1, scan_distance: 1000.0 });

    let mut g = world(vec![wh, center, truck], 0);
    let warehouse = g.create_object(&g.find_template("Warehouse").unwrap(), Coord3D::new(50.0, 100.0, 0.0), 0.0, 0);
    g.create_object(&g.find_template("SupplyCenter").unwrap(), Coord3D::new(250.0, 100.0, 0.0), 0.0, 0);
    let t = g.create_object(&g.find_template("SupplyTruck").unwrap(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    // A truck only starts harvesting once forced into the Wanting state (a Supply Center's exit does).
    g.set_supply_force_wanting(t, true);
    g.run_frames(600);
    assert_eq!(g.player(0).unwrap().money, 200, "2 boxes * ValuePerSupplyBox 100");
    assert_eq!(g.find_object(t).unwrap().boxes, 0);
    assert_eq!(g.find_object(warehouse).unwrap().boxes, 0);
}

#[test]
fn warehouse_deletes_when_empty() {
    let mut wh = structure("Pile", 0, 0);
    wh.geometry.major_radius = 10.0;
    wh.geometry.minor_radius = 10.0;
    wh.dock = Some(DockTemplate { kind: DockKind::Warehouse, starting_boxes: 1, delete_when_empty: true, number_approach_positions: 5, allows_passthrough: true });
    let mut center = structure("Center", 0, 0);
    center.geometry.major_radius = 10.0;
    center.geometry.minor_radius = 10.0;
    center.dock = Some(DockTemplate { kind: DockKind::Center, starting_boxes: 0, delete_when_empty: false, number_approach_positions: 5, allows_passthrough: true });
    let mut truck = mobile("Truck");
    truck.supply_truck = Some(SupplyTruckTemplate { max_boxes: 1, center_action_delay: 1, warehouse_action_delay: 1, scan_distance: 1000.0 });

    let mut g = world(vec![wh, center, truck], 0);
    let pile = g.create_object(&g.find_template("Pile").unwrap(), Coord3D::new(50.0, 100.0, 0.0), 0.0, 0);
    g.create_object(&g.find_template("Center").unwrap(), Coord3D::new(250.0, 100.0, 0.0), 0.0, 0);
    let truck = g.create_object(&g.find_template("Truck").unwrap(), Coord3D::new(80.0, 100.0, 0.0), 0.0, 0);
    g.set_supply_force_wanting(truck, true);
    g.run_frames(600);
    assert!(g.find_object(pile).is_none(), "warehouse deleted when empty");
    assert_eq!(g.player(0).unwrap().money, 100, "one box delivered");
}



fn dock_world(approach_positions: i32) -> (GameLogic, zr_sim::ObjectId, zr_sim::ObjectId, zr_sim::ObjectId) {
    let mut wh = structure("Warehouse", 0, 0);
    wh.geometry.major_radius = 10.0;
    wh.geometry.minor_radius = 10.0;
    wh.dock = Some(DockTemplate {
        kind: DockKind::Warehouse,
        starting_boxes: 6,
        delete_when_empty: false,
        number_approach_positions: approach_positions,
        allows_passthrough: true,
    });
    wh.dock_bones = warehouse_bones();
    let mut center = structure("SupplyCenter", 0, 0);
    center.geometry.major_radius = 10.0;
    center.geometry.minor_radius = 10.0;
    center.dock = Some(DockTemplate {
        kind: DockKind::Center,
        starting_boxes: 0,
        delete_when_empty: false,
        number_approach_positions: 5,
        allows_passthrough: true,
    });
    let mut truck = mobile("SupplyTruck");
    truck.supply_truck = Some(SupplyTruckTemplate { max_boxes: 2, center_action_delay: 1, warehouse_action_delay: 1, scan_distance: 1000.0 });
    let mut g = world(vec![wh, center, truck], 0);
    let warehouse = g.create_object(&g.find_template("Warehouse").unwrap(), Coord3D::new(50.0, 100.0, 0.0), 0.0, 0);
    g.create_object(&g.find_template("SupplyCenter").unwrap(), Coord3D::new(250.0, 100.0, 0.0), 0.0, 0);
    let a = g.create_object(&g.find_template("SupplyTruck").unwrap(), Coord3D::new(100.0, 90.0, 0.0), 0.0, 0);
    let b = g.create_object(&g.find_template("SupplyTruck").unwrap(), Coord3D::new(100.0, 115.0, 0.0), 0.0, 0);
    (g, warehouse, a, b)
}

/// `DockUpdate`'s approach queue: with one approach position the second truck waits for the first
/// (`AIDockWaitForClearanceState`), and both end up delivering. `SupplyCenterDockUpdate::action`
/// pays `boxes * ValuePerSupplyBox`.
#[test]
fn two_trucks_share_a_one_position_warehouse() {
    let (mut g, warehouse, a, b) = dock_world(1);
    g.set_supply_force_wanting(a, true);
    g.set_supply_force_wanting(b, true);
    g.run_frames(2400);
    assert_eq!(g.find_object(warehouse).unwrap().boxes, 6 - 6, "all six boxes were taken");
    assert_eq!(g.player(0).unwrap().money, 600, "6 boxes * ValuePerSupplyBox 100 delivered");
}

/// A truck is only a gatherer once forced into the Wanting state (`setForceWantingState`): an
/// idle truck that nobody ordered does not walk to a warehouse.
#[test]
fn an_unforced_truck_stays_idle() {
    let (mut g, warehouse, a, _b) = dock_world(5);
    g.run_frames(300);
    assert_eq!(g.find_object(warehouse).unwrap().boxes, 6);
    assert_eq!(g.find_object(a).unwrap().boxes, 0);
    assert_eq!(g.find_object(a).unwrap().ai.machine.current, zr_sim::ai_states::AiStateType::Idle);
}

/// `AIDockMachine::halt` -> `DockUpdate::cancelDock`: stopping a docking truck frees its approach
/// position, so another truck can use the (single-position) dock.
///
/// The dock-entry check proves the reservation; the delivery check proves the dock action (the
/// truck is close enough to the dock, `SupplyWarehouseDockUpdate::action`, :88-106, and
/// `dock_action` runs from `AIDockProcessDockState`). The synthetic trucks carry a `PhysicsBehavior`
/// like every shipped mobile template: without it the sim's reduced kinematic mover turns through a
/// short goal instead of arriving, which is what made this delivery fail (lane 127's leftover; the
/// approach sample itself is inside the action radius).
#[test]
fn halting_a_docker_frees_its_approach_position() {
    let (mut g, warehouse, a, b) = dock_world(1);
    g.set_supply_force_wanting(a, true);
    // Run until `a` has reserved the only position, then stop it.
    for _ in 0..600 {
        g.update();
        if g.find_object(a).unwrap().ai.machine.current == zr_sim::ai_states::AiStateType::Dock {
            break;
        }
    }
    assert_eq!(g.find_object(a).unwrap().ai.machine.current, zr_sim::ai_states::AiStateType::Dock);
    g.run_frames(20);
    g.submit(0, zr_sim::Command::Stop { ids: vec![a] });
    g.update();
    g.set_supply_force_wanting(b, true);
    // `b` may only dock with the freed position: run until it is in `AI_DOCK` (its approach
    // reservation succeeded).
    let mut docked = false;
    for _ in 0..900 {
        g.update();
        if g.find_object(b).unwrap().ai.machine.current == zr_sim::ai_states::AiStateType::Dock {
            docked = true;
            break;
        }
    }
    assert!(docked, "b reserved and entered the approach position a had held");
    // The dock action runs at the end of that dock sequence: the box leaves the warehouse and the
    // centre pays `ValuePerSupplyBox` for it (`SupplyWarehouseDockUpdate::action`,
    // `SupplyCenterDockUpdate::action`).
    let mut delivered = false;
    for _ in 0..1800 {
        g.update();
        if g.player(0).unwrap().money > 0 {
            delivered = true;
            break;
        }
    }
    assert!(delivered, "b delivered a box after taking the freed approach position");
    assert_eq!(g.find_object(warehouse).unwrap().boxes, 4, "b took its two boxes");
    assert_eq!(g.player(0).unwrap().money, 200, "2 boxes * ValuePerSupplyBox 100");
}

/// `RepairDockUpdate::action`: a docked unit is healed over `TimeForFullHeal`.
#[test]
fn a_repair_dock_heals_a_docker() {
    let mut pad = structure("RepairPad", 0, 0);
    pad.geometry.major_radius = 10.0;
    pad.geometry.minor_radius = 10.0;
    pad.repair_dock = Some(zr_sim::modules2::RepairDockTemplate { frames_for_full_heal: 60.0, number_approach_positions: 2, allows_passthrough: true });
    let mut g = world(vec![pad, mobile("Tank")], 0);
    let pad = g.create_object(&g.find_template("RepairPad").unwrap(), Coord3D::new(150.0, 100.0, 0.0), 0.0, 0);
    let tank = g.create_object(&g.find_template("Tank").unwrap(), Coord3D::new(60.0, 100.0, 0.0), 0.0, 0);
    g.damage_object(tank, zr_sim::DamageInfo { source: zr_sim::ObjectId::INVALID, amount: 60.0, kind: zr_sim::DamageType::Unresistable });
    assert!(g.find_object(tank).unwrap().health() < 50.0);
    g.ai_dock(tank, pad);
    g.run_frames(900);
    assert_eq!(g.find_object(tank).unwrap().health(), 100.0, "healed to full");
}
