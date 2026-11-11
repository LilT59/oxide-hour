//! The `ActionManager` dock / get-repaired / get-healed queries and the player dock commands
//! (`Command::{Dock,GetRepaired,GetHealed}`). Synthetic data only, no game install.
//!
//! Ports under test: `ActionManager::{canDockAt, canGetRepairedAt, canGetHealedAt}`
//! (`Common/RTS/ActionManager.cpp`) and the `AIUpdateInterface::private*` command paths
//! (`AIUpdate.cpp`).

use std::sync::Arc;

use zr_sim::contain::{ContainKind, ContainTemplate, GarrisonFirePoints};
use zr_sim::modules2::RepairDockTemplate;
use zr_sim::supply::{DockKind, DockTemplate, SupplyTruckTemplate};
use zr_sim::template::{collide_kind, kind_of, GeometryShape, LocomotorKind, LocomotorParams};
use zr_sim::{
    Command, Coord3D, DamageInfo, DamageType, GameLogic, ObjectId, ObjectTemplate, TemplateSource, TerrainLogic,
};

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
    t
}

fn infantry(name: &str) -> ObjectTemplate {
    let mut t = mobile(name);
    t.kind_of = kind_of::INFANTRY | kind_of::SELECTABLE;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Legs, 30.0, 60.0, 120.0, 180.0));
    t
}

fn structure(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;
    t.max_health = 500.0;
    t.initial_health = 500.0;
    t.geometry.shape = GeometryShape::Box;
    t.geometry.major_radius = 15.0;
    t.geometry.minor_radius = 15.0;
    t
}

fn contain(heal_frames: f32) -> ContainTemplate {
    ContainTemplate {
        // A heal-pad building is a `HealContain` (`GameLogic/Object/Contain/HealContain.cpp`); the port
        // used to fold it into `Open` and infer it from `HealthRegen%PerSec`, which `OpenContain` does
        // not parse.
        kind: ContainKind::Heal,
        capacity: -1,
        allow_inside_kind_of: None,
        forbid_inside_kind_of: 0,
        allow_allies_inside: true,
        allow_enemies_inside: false,
        allow_neutral_inside: false,
        passengers_allowed_to_fire: false,
        damage_percentage_to_units: 0.0,
        number_of_exit_paths: 1,
        exit_delay: 0,
        door_open_time: 1,
        scatter_nearby_on_exit: true,
        orient_like_container_on_exit: false,
        reset_mood_check_on_exit: false,
        health_regen: 0.0,
        do_i_heal_objects: false,
        heal_frames,
        initial_payload: Vec::new(),
        tunnel_frames_for_full_heal: 1.0,
        is_enclosing_container: false,
        fire_points: GarrisonFirePoints::default(),
        passengers_in_turret: false,
        weapon_bonus_passed_to_passengers: false,
        immune_to_clear_building_attacks: false,
    }
}

fn world(templates: Vec<ObjectTemplate>) -> GameLogic {
    let store = FixedStore(templates.into_iter().map(Arc::new).collect());
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 0);
    g.set_template_source(Arc::new(store));
    g
}

fn damage(g: &mut GameLogic, id: ObjectId, amount: f32) {
    g.damage_object(id, DamageInfo { source: ObjectId::INVALID, amount, kind: DamageType::Unresistable });
}

fn supply_dock(kind: DockKind, boxes: i32) -> DockTemplate {
    DockTemplate { kind, starting_boxes: boxes, delete_when_empty: false, number_approach_positions: 5, allows_passthrough: true }
}

/// Dock bones for a synthetic warehouse, read the way `DockUpdate::loadDockPositions` reads them from the model
/// (DockUpdate.cpp:492-540: `DockStart`, `DockAction`, `DockEnd`, `DockWaiting01..`). Without `DockStart` a dock's
/// `getDockPosition` is the docker's own position (DockUpdate.cpp:279-288), so a boneless warehouse docks wherever
/// the approach stopped, and the four-unit box pickup radius (SupplyWarehouseDockUpdate.cpp:93-106) is met only by
/// luck. The real warehouses carry these bones (zr-game `dock_bones.rs`). The offsets are synthetic, sized to a
/// 10-unit footprint, object-local.
fn warehouse_bones() -> zr_sim::dock::DockBones {
    zr_sim::dock::DockBones {
        enter: zr_sim::Coord3D::new(16.0, 0.0, 0.0),
        dock: zr_sim::Coord3D::new(11.0, 0.0, 0.0),
        exit: zr_sim::Coord3D::new(16.0, 6.0, 0.0),
        approach: (0..5).map(|i| zr_sim::Coord3D::new(24.0, -12.0 + 6.0 * i as f32, 0.0)).collect(),
    }
}

/// `canDockAt`: a supply truck may dock a warehouse with boxes; the repair dock, an empty
/// warehouse, a non-supply truck, an enemy warehouse and an empty truck's own center are all
/// rejected.
#[test]
fn can_dock_at_rejects_non_docks() {
    let mut wh = structure("Warehouse");
    wh.dock = Some(supply_dock(DockKind::Warehouse, 2));
    let mut empty = structure("EmptyWarehouse");
    empty.dock = Some(supply_dock(DockKind::Warehouse, 0));
    let mut center = structure("Center");
    center.dock = Some(supply_dock(DockKind::Center, 0));
    let mut pad = structure("RepairPad");
    pad.repair_dock = Some(RepairDockTemplate { frames_for_full_heal: 60.0, number_approach_positions: 2, allows_passthrough: true });
    let mut truck = mobile("SupplyTruck");
    truck.supply_truck = Some(SupplyTruckTemplate { max_boxes: 2, center_action_delay: 1, warehouse_action_delay: 1, scan_distance: 1000.0 });
    let tank = mobile("Tank");

    let mut g = world(vec![wh, empty, center, pad, truck, tank]);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    let warehouse = g.create_object(&g.find_template("Warehouse").unwrap(), Coord3D::new(50.0, 100.0, 0.0), 0.0, 0);
    let empty_wh = g.create_object(&g.find_template("EmptyWarehouse").unwrap(), Coord3D::new(80.0, 100.0, 0.0), 0.0, 0);
    let supply_center = g.create_object(&g.find_template("Center").unwrap(), Coord3D::new(250.0, 100.0, 0.0), 0.0, 0);
    let repair_pad = g.create_object(&g.find_template("RepairPad").unwrap(), Coord3D::new(150.0, 200.0, 0.0), 0.0, 0);
    let t = g.create_object(&g.find_template("SupplyTruck").unwrap(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let tank = g.create_object(&g.find_template("Tank").unwrap(), Coord3D::new(100.0, 140.0, 0.0), 0.0, 0);
    let enemy_wh = g.create_object(&g.find_template("Warehouse").unwrap(), Coord3D::new(300.0, 300.0, 0.0), 0.0, 1);

    assert!(g.can_dock_at(t, warehouse), "a warehouse with boxes is dockable");
    assert!(!g.can_dock_at(t, empty_wh), "an empty warehouse is not");
    assert!(!g.can_dock_at(t, supply_center), "an empty truck cannot deliver to a center");
    assert!(!g.can_dock_at(t, repair_pad), "a repair dock is not a supply dock");
    assert!(!g.can_dock_at(tank, warehouse), "only a supply truck can transfer supplies");
    assert!(!g.can_dock_at(t, enemy_wh), "an enemy warehouse is not dockable");
}

/// `canDockAt`: a truck carrying boxes may dock its own supply center. There is no center in the
/// world while it picks up, so it cannot deliver the box away first.
#[test]
fn can_dock_at_accepts_a_loaded_truck_at_its_center() {
    let mut wh = structure("Warehouse");
    wh.geometry.major_radius = 10.0;
    wh.geometry.minor_radius = 10.0;
    wh.dock = Some(supply_dock(DockKind::Warehouse, 2));
    wh.dock_bones = warehouse_bones();
    let mut center = structure("Center");
    center.geometry.major_radius = 10.0;
    center.geometry.minor_radius = 10.0;
    center.dock = Some(supply_dock(DockKind::Center, 0));
    let mut truck = mobile("SupplyTruck");
    truck.supply_truck = Some(SupplyTruckTemplate { max_boxes: 2, center_action_delay: 1, warehouse_action_delay: 1, scan_distance: 1000.0 });

    let mut g = world(vec![wh, center, truck]);
    let warehouse = g.create_object(&g.find_template("Warehouse").unwrap(), Coord3D::new(50.0, 100.0, 0.0), 0.0, 0);
    let t = g.create_object(&g.find_template("SupplyTruck").unwrap(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    g.submit(0, Command::Dock { ids: vec![t], target: warehouse });
    g.run_frames(240);
    assert!(g.find_object(t).unwrap().boxes > 0, "the truck picked up a box");
    let supply_center = g.create_object(&g.find_template("Center").unwrap(), Coord3D::new(250.0, 100.0, 0.0), 0.0, 0);
    assert!(g.can_dock_at(t, supply_center), "a loaded truck docks its own supply center");
}

/// `canGetRepairedAt`: a damaged allied vehicle at a repair pad; full health, infantry, non-repair
/// structures and enemies are rejected. Aircraft need an airfield and must be airborne.
#[test]
fn can_get_repaired_at_checks_are_faithful() {
    let mut pad = structure("RepairPad");
    pad.kind_of |= kind_of::REPAIR_PAD;
    pad.repair_dock = Some(RepairDockTemplate { frames_for_full_heal: 60.0, number_approach_positions: 2, allows_passthrough: true });
    let plain = structure("Plain");
    let mut airfield = structure("Airfield");
    airfield.collide_kinds |= collide_kind::FS_AIRFIELD;
    let mut tank = mobile("Tank");
    tank.kind_of |= kind_of::VEHICLE;
    let mut plane = mobile("Plane");
    plane.kind_of |= kind_of::VEHICLE | kind_of::AIRCRAFT;
    plane.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Wings, 100.0, 100.0, 100.0, 360.0));
    // `from_ini`'s `base` leaves `stick_to_ground` on; an air locomotor keeps its altitude.
    plane.locomotor.as_mut().unwrap().stick_to_ground = false;

    let mut g = world(vec![pad, plain, airfield, tank, plane, infantry("Ranger")]);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    let pad = g.create_object(&g.find_template("RepairPad").unwrap(), Coord3D::new(150.0, 100.0, 0.0), 0.0, 0);
    let plain = g.create_object(&g.find_template("Plain").unwrap(), Coord3D::new(250.0, 100.0, 0.0), 0.0, 0);
    let airfield = g.create_object(&g.find_template("Airfield").unwrap(), Coord3D::new(150.0, 250.0, 0.0), 0.0, 0);
    let tank = g.create_object(&g.find_template("Tank").unwrap(), Coord3D::new(60.0, 100.0, 0.0), 0.0, 0);
    let plane_air = g.create_object(&g.find_template("Plane").unwrap(), Coord3D::new(60.0, 250.0, 100.0), 0.0, 0);
    let plane_ground = g.create_object(&g.find_template("Plane").unwrap(), Coord3D::new(60.0, 260.0, 0.0), 0.0, 0);
    let ranger = g.create_object(&g.find_template("Ranger").unwrap(), Coord3D::new(60.0, 140.0, 0.0), 0.0, 0);
    let enemy_pad = g.create_object(&g.find_template("RepairPad").unwrap(), Coord3D::new(300.0, 300.0, 0.0), 0.0, 1);

    assert!(!g.can_get_repaired_at(tank, pad), "a full-health vehicle needs no repair");
    damage(&mut g, tank, 40.0);
    damage(&mut g, plane_air, 40.0);
    damage(&mut g, plane_ground, 40.0);
    assert!(g.can_get_repaired_at(tank, pad), "a damaged allied vehicle may repair at a repair pad");
    assert!(!g.can_get_repaired_at(tank, plain), "a non-repair structure is rejected");
    assert!(!g.can_get_repaired_at(tank, enemy_pad), "an enemy repair pad is rejected");
    assert!(!g.can_get_repaired_at(ranger, pad), "only vehicles get repaired");

    // Aircraft: airborne at an airfield yes; grounded no; at a ground repair pad no.
    assert!(g.can_get_repaired_at(plane_air, airfield), "an airborne aircraft repairs at an airfield");
    assert!(!g.can_get_repaired_at(plane_air, pad), "an aircraft does not repair at a ground repair pad");
    assert!(!g.can_get_repaired_at(plane_ground, airfield), "a grounded aircraft is rejected");
}

/// `canGetHealedAt`: a damaged allied infantry at a heal pad; full health, vehicles, non-heal
/// structures and enemies are rejected.
#[test]
fn can_get_healed_at_checks_are_faithful() {
    let mut hospital = structure("Hospital");
    hospital.kind_of |= kind_of::HEAL_PAD;
    hospital.contain = Some(Arc::new(contain(1.0)));
    let plain = structure("Plain");
    let mut tank = mobile("Tank");
    tank.kind_of |= kind_of::VEHICLE;

    let mut g = world(vec![hospital, plain, tank, infantry("Ranger")]);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    let hospital = g.create_object(&g.find_template("Hospital").unwrap(), Coord3D::new(150.0, 100.0, 0.0), 0.0, 0);
    let plain = g.create_object(&g.find_template("Plain").unwrap(), Coord3D::new(250.0, 100.0, 0.0), 0.0, 0);
    let ranger = g.create_object(&g.find_template("Ranger").unwrap(), Coord3D::new(60.0, 100.0, 0.0), 0.0, 0);
    let tank = g.create_object(&g.find_template("Tank").unwrap(), Coord3D::new(60.0, 140.0, 0.0), 0.0, 0);
    let enemy_hospital = g.create_object(&g.find_template("Hospital").unwrap(), Coord3D::new(300.0, 300.0, 0.0), 0.0, 1);

    assert!(!g.can_get_healed_at(ranger, hospital), "full health needs no healing");
    damage(&mut g, ranger, 40.0);
    assert!(g.can_get_healed_at(ranger, hospital), "a damaged allied infantry heals at a heal pad");
    assert!(!g.can_get_healed_at(ranger, plain), "a non-heal structure is rejected");
    assert!(!g.can_get_healed_at(ranger, enemy_hospital), "an enemy heal pad is rejected");
    damage(&mut g, tank, 40.0);
    assert!(!g.can_get_healed_at(tank, hospital), "vehicles get repaired, not healed");
}

/// `Command::Dock`: a supply truck ordered to a warehouse picks up its boxes and delivers them to
/// the player's own supply center (money += `boxes * ValuePerSupplyBox`).
#[test]
fn command_dock_makes_a_truck_cycle() {
    let mut wh = structure("Warehouse");
    wh.geometry.major_radius = 10.0;
    wh.geometry.minor_radius = 10.0;
    wh.dock = Some(DockTemplate { kind: DockKind::Warehouse, starting_boxes: 2, delete_when_empty: false, number_approach_positions: 5, allows_passthrough: true });
    wh.dock_bones = warehouse_bones();
    let mut center = structure("Center");
    center.geometry.major_radius = 10.0;
    center.geometry.minor_radius = 10.0;
    center.dock = Some(DockTemplate { kind: DockKind::Center, starting_boxes: 0, delete_when_empty: false, number_approach_positions: 5, allows_passthrough: true });
    let mut truck = mobile("SupplyTruck");
    truck.supply_truck = Some(SupplyTruckTemplate { max_boxes: 2, center_action_delay: 1, warehouse_action_delay: 1, scan_distance: 1000.0 });

    let mut g = world(vec![wh, center, truck]);
    let warehouse = g.create_object(&g.find_template("Warehouse").unwrap(), Coord3D::new(50.0, 100.0, 0.0), 0.0, 0);
    g.create_object(&g.find_template("Center").unwrap(), Coord3D::new(250.0, 100.0, 0.0), 0.0, 0);
    let t = g.create_object(&g.find_template("SupplyTruck").unwrap(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);

    assert!(g.can_dock_at(t, warehouse));
    g.submit(0, Command::Dock { ids: vec![t], target: warehouse });
    g.run_frames(900);
    assert_eq!(g.find_object(warehouse).unwrap().boxes, 0, "both boxes were taken");
    assert_eq!(g.find_object(t).unwrap().boxes, 0, "the truck delivered its load");
    assert_eq!(g.player(0).unwrap().money, 200, "2 boxes * ValuePerSupplyBox 100");
}

/// `Command::GetRepaired`: a damaged vehicle ordered to a repair pad docks and is healed to full.
#[test]
fn command_get_repaired_heals_a_tank() {
    let mut pad = structure("RepairPad");
    pad.kind_of |= kind_of::REPAIR_PAD;
    pad.geometry.major_radius = 10.0;
    pad.geometry.minor_radius = 10.0;
    pad.repair_dock = Some(RepairDockTemplate { frames_for_full_heal: 60.0, number_approach_positions: 2, allows_passthrough: true });
    let mut tank = mobile("Tank");
    tank.kind_of |= kind_of::VEHICLE;

    let mut g = world(vec![pad, tank]);
    let pad = g.create_object(&g.find_template("RepairPad").unwrap(), Coord3D::new(150.0, 100.0, 0.0), 0.0, 0);
    let tank = g.create_object(&g.find_template("Tank").unwrap(), Coord3D::new(60.0, 100.0, 0.0), 0.0, 0);
    damage(&mut g, tank, 60.0);
    assert!(g.find_object(tank).unwrap().health() < 50.0);
    assert!(g.can_get_repaired_at(tank, pad));
    g.submit(0, Command::GetRepaired { ids: vec![tank], target: pad });
    g.run_frames(900);
    assert_eq!(g.find_object(tank).unwrap().health(), 100.0, "healed to full at the repair pad");
}

/// `Command::GetHealed`: a damaged infantry ordered to a heal building enters it and is healed.
#[test]
fn command_get_healed_heals_infantry() {
    let mut hospital = structure("Hospital");
    hospital.kind_of |= kind_of::HEAL_PAD;
    hospital.geometry.major_radius = 10.0;
    hospital.geometry.minor_radius = 10.0;
    hospital.contain = Some(Arc::new(contain(1.0)));

    let mut g = world(vec![hospital, infantry("Ranger")]);
    let hospital = g.create_object(&g.find_template("Hospital").unwrap(), Coord3D::new(150.0, 100.0, 0.0), 0.0, 0);
    let ranger = g.create_object(&g.find_template("Ranger").unwrap(), Coord3D::new(60.0, 100.0, 0.0), 0.0, 0);
    damage(&mut g, ranger, 40.0);
    assert!(g.can_get_healed_at(ranger, hospital));
    g.submit(0, Command::GetHealed { ids: vec![ranger], target: hospital });
    g.run_frames(600);
    // `HealContain::update` heals to full after `TimeForFullHeal` and then exits the passenger through a door
    // (`HealContain.cpp:113-118`, `:155-167`); this template's `TimeForFullHeal` is 1 frame.
    assert_eq!(g.find_object(ranger).unwrap().contained_by, None, "healed and ejected");
    assert_eq!(g.find_object(ranger).unwrap().health(), 100.0, "healed to full");
}

/// `Command::GetRepaired` rejects a full-health vehicle: the order is ignored (privateGetRepaired
/// bails), so the vehicle never docks.
#[test]
fn command_get_repaired_ignores_a_full_health_vehicle() {
    let mut pad = structure("RepairPad");
    pad.kind_of |= kind_of::REPAIR_PAD;
    pad.repair_dock = Some(RepairDockTemplate { frames_for_full_heal: 60.0, number_approach_positions: 2, allows_passthrough: true });
    let mut tank = mobile("Tank");
    tank.kind_of |= kind_of::VEHICLE;
    let mut g = world(vec![pad, tank]);
    let pad = g.create_object(&g.find_template("RepairPad").unwrap(), Coord3D::new(150.0, 100.0, 0.0), 0.0, 0);
    let tank = g.create_object(&g.find_template("Tank").unwrap(), Coord3D::new(60.0, 100.0, 0.0), 0.0, 0);
    g.submit(0, Command::GetRepaired { ids: vec![tank], target: pad });
    g.run_frames(30);
    assert_ne!(g.find_object(tank).unwrap().ai.machine.current, zr_sim::ai_states::AiStateType::Dock);
    assert_eq!(g.find_object(tank).unwrap().ai.machine.goal_object, ObjectId::INVALID);
}
