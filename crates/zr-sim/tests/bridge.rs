//! Bridge layers: drive over water at deck height, pass under a land bridge, and block a
//! destroyed bridge (`TerrainLogic` `Bridge` / `Pathfinder` `PathfindLayer`). Synthetic data only.

use zr_map::PolygonTrigger;
use zr_sim::bridge::Bridge;
use zr_sim::body::DamageInfo;
use zr_sim::damage::DamageType;
use zr_sim::die::{DieMux, DieModuleTemplate, DieTemplate};
use zr_sim::pathfind::{surface, PathObject, Pathfinder};
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::{Coord3D, GameLogic, ObjectId, ObjectTemplate, TerrainLogic};

/// 1000x1000 flat map (ground z = 0) with a north-south river at x in [200, 400].
fn river_terrain() -> TerrainLogic {
    let mut t = TerrainLogic::flat(100, 100, 0);
    t.add_water_area(PolygonTrigger {
        name: "River".into(),
        layer: String::new(),
        id: 0,
        is_water: true,
        is_river: false,
        river_start: 0,
        points: vec![[200, -50, 5], [400, -50, 5], [400, 1050, 5], [200, 1050, 5]],
    });
    t
}

fn ground_object() -> PathObject {
    PathObject { id: ObjectId::INVALID, surfaces: surface::GROUND, bounding_radius: 8.0, layer: zr_sim::pathfind::PathLayer::Ground, player: 0, infantry: false, crusher: false, ..Default::default() }
}

/// A ground unit can cross the river via the bridge (the water is otherwise impassable).
#[test]
fn bridge_over_water_is_drivable() {
    let mut terrain = river_terrain();
    terrain.add_bridge(Bridge::new(Coord3D::new(150.0, 300.0, 10.0), Coord3D::new(450.0, 300.0, 10.0), 40.0, 2));

    let mut pf = Pathfinder::new(&terrain);
    let obj = ground_object();
    let path = pf.find_path(&terrain, &obj, &Coord3D::new(100.0, 300.0, 0.0), &Coord3D::new(500.0, 300.0, 0.0));
    assert!(path.is_some(), "river is crossable over the bridge");
}

/// A goal cell on the bridge over impassable ground resolves to the deck height.
#[test]
fn bridge_waypoint_uses_deck_height() {
    let mut terrain = river_terrain();
    terrain.add_bridge(Bridge::new(Coord3D::new(150.0, 300.0, 10.0), Coord3D::new(450.0, 300.0, 10.0), 40.0, 2));

    let mut pf = Pathfinder::new(&terrain);
    let obj = ground_object();
    let path = pf.find_path(&terrain, &obj, &Coord3D::new(100.0, 300.0, 0.0), &Coord3D::new(300.0, 300.0, 0.0)).unwrap();
    let end = *path.waypoints().last().unwrap();
    assert!((end.z - 10.0).abs() < 0.01, "goal on the deck must sit at z = 10, got {end:?}");
}

/// A bridge over passable ground keeps the ground route (units pass underneath).
#[test]
fn bridge_over_land_does_not_change_waypoint_height() {
    let mut terrain = TerrainLogic::flat(100, 100, 0);
    terrain.add_bridge(Bridge::new(Coord3D::new(150.0, 300.0, 10.0), Coord3D::new(450.0, 300.0, 10.0), 40.0, 2));

    let mut pf = Pathfinder::new(&terrain);
    let obj = ground_object();
    let path = pf.find_path(&terrain, &obj, &Coord3D::new(100.0, 300.0, 0.0), &Coord3D::new(500.0, 300.0, 0.0)).unwrap();

    assert!(path.waypoints().iter().all(|p| p.z.abs() < 0.01), "land bridge must not lift waypoints: {:?}", path.waypoints());
}

/// Destroying the bridge makes the river impassable again (re-classified via `refresh_bridges`).
#[test]
fn destroyed_bridge_blocks_the_crossing() {
    let mut terrain = river_terrain();
    terrain.add_bridge(Bridge::new(Coord3D::new(150.0, 300.0, 10.0), Coord3D::new(450.0, 300.0, 10.0), 40.0, 2));

    let mut pf = Pathfinder::new(&terrain);
    let obj = ground_object();
    let from = Coord3D::new(100.0, 300.0, 0.0);
    let to = Coord3D::new(500.0, 300.0, 0.0);
    assert!(pf.find_path(&terrain, &obj, &from, &to).is_some());

    terrain.bridges_mut()[0].destroyed = true;
    pf.refresh_bridges(&terrain);
    assert!(pf.find_path(&terrain, &obj, &from, &to).is_none(), "no crossing without the bridge");
}

/// `TerrainLogic::surface_height` snaps to whichever of ground/deck is closest in z.
#[test]
fn surface_height_picks_closest_layer() {
    let mut terrain = river_terrain();
    terrain.add_bridge(Bridge::new(Coord3D::new(150.0, 300.0, 10.0), Coord3D::new(450.0, 300.0, 10.0), 40.0, 2));

    assert!((terrain.surface_height(300.0, 300.0, 9.0) - 10.0).abs() < 0.01, "near the deck");
    assert!((terrain.surface_height(300.0, 300.0, 1.0) - 0.0).abs() < 0.01, "near the ground");
    assert_eq!(terrain.layer_for_destination(&Coord3D::new(300.0, 300.0, 9.0)), 2);
    assert_eq!(terrain.layer_for_destination(&Coord3D::new(300.0, 300.0, 1.0)), 1);
}

/// A bridge object that persists in `BODY_RUBBLE` (`FXListDie`, like `MonumentTrainBridge`).
fn bridge_template() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Bridge");
    t.is_bridge = true;
    t.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;
    t.geometry.major_radius = 200.0;
    t.geometry.minor_radius = 20.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.die.push(DieModuleTemplate {
        mux: DieMux { death_types: u32::MAX, veterancy_levels: u32::MAX, exempt_status: 0, required_status: 0 },
        kind: DieTemplate::FxList { default_death_fx: None, orient_to_object: false, initially_active: false },
    });
    t
}

fn tank_template() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Tank");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 8.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    t
}

/// `Bridge::updateDamageState` on `BODY_RUBBLE`: the crossing is re-blocked and every object on the
/// deck dies (`DAMAGE_FALLING` / `DEATH_SPLATTED`); repairing the body restores the crossing.
#[test]
fn destroying_a_bridge_kills_occupants_and_repair_restores_it() {
    let mut terrain = river_terrain();
    terrain.add_bridge(Bridge::new(Coord3D::new(150.0, 300.0, 10.0), Coord3D::new(450.0, 300.0, 10.0), 40.0, 2));
    let mut g = GameLogic::new(terrain, 1);

    let bt = std::sync::Arc::new(bridge_template());
    let mid = Coord3D::new(300.0, 300.0, 10.0);
    let bridge = g.create_object(&bt, mid, 0.0, 0);
    assert!(g.link_bridge_object(bridge, &mid));
    let tt = std::sync::Arc::new(tank_template());
    let occupant = g.create_object(&tt, Coord3D::new(300.0, 320.0, 10.0), 0.0, 0);

    let dealt = g.damage_object(bridge, DamageInfo { source: ObjectId::INVALID, amount: 100.0, kind: DamageType::Explosion });
    assert_eq!(dealt, 100.0);
    assert!(g.terrain().bridges()[0].destroyed, "the bridge is destroyed");
    assert!(
        g.find_object(occupant).is_none() || g.find_object(occupant).is_some_and(|o| o.is_dead()),
        "an object on the deck dies when the bridge falls"
    );

    // `TerrainLogic::isBridgeRepaired`: healing the bridge object restores the crossing.
    let healed = g.heal_object(bridge, 100.0);
    assert!(healed > 0.0, "the bridge object survived in rubble");
    assert!(!g.terrain().bridges()[0].destroyed, "the bridge is repaired");
}

/// Damage to a tower propagates to its bridge by health percentage (`BridgeTowerBehavior::onDamage`).
#[test]
fn tower_damage_propagates_to_the_bridge() {
    let mut terrain = river_terrain();
    terrain.add_bridge(Bridge::new(Coord3D::new(150.0, 300.0, 10.0), Coord3D::new(450.0, 300.0, 10.0), 40.0, 2));
    let mut g = GameLogic::new(terrain, 1);

    let bt = std::sync::Arc::new(bridge_template());
    let mid = Coord3D::new(300.0, 300.0, 10.0);
    let bridge = g.create_object(&bt, mid, 0.0, 0);
    g.link_bridge_object(bridge, &mid);
    let mut tower_t = bridge_template();
    tower_t.is_bridge = false;
    tower_t.name = "Tower".into();
    let tower = g.create_object(&std::sync::Arc::new(tower_t), Coord3D::new(160.0, 300.0, 10.0), 0.0, 0);
    g.link_bridge_tower(tower, bridge);

    // 50% of the tower's 100 hp: the bridge takes 50% of its 100 hp, leaving it damaged but alive.
    g.damage_object(tower, DamageInfo { source: ObjectId::INVALID, amount: 50.0, kind: DamageType::Explosion });
    let hp = g.find_object(bridge).unwrap().health();
    assert!((hp - 50.0).abs() < 0.01, "bridge took half its health, got {hp}");
    assert!(!g.terrain().bridges()[0].destroyed);
}

/// `Command::Repair`: a dozer walks to a destroyed bridge and heals it out of rubble.
#[test]
fn dozer_repairs_a_destroyed_bridge() {
    let mut terrain = river_terrain();
    terrain.add_bridge(Bridge::new(Coord3D::new(150.0, 300.0, 10.0), Coord3D::new(450.0, 300.0, 10.0), 40.0, 2));
    let mut g = GameLogic::new(terrain, 1);

    let bt = std::sync::Arc::new(bridge_template());
    let mid = Coord3D::new(300.0, 300.0, 10.0);
    let bridge = g.create_object(&bt, mid, 0.0, 0);
    assert!(g.link_bridge_object(bridge, &mid));
    g.damage_object(bridge, DamageInfo { source: ObjectId::INVALID, amount: 100.0, kind: DamageType::Explosion });
    assert!(g.terrain().bridges()[0].destroyed);

    let mut dozer_t = tank_template();
    dozer_t.name = "Dozer".into();
    // Bridge repair is an extension over retail (`canRepairObject` rejects bridges); give the
    // synthetic dozer the real repair rate field so the low-level heal runs.
    dozer_t.dozer = Some(zr_sim::dozer::DozerTemplate { repair_health_percent_per_second: 0.05, ..Default::default() });
    let dozer = g.create_object(&std::sync::Arc::new(dozer_t), Coord3D::new(100.0, 300.0, 0.0), 0.0, 0);
    assert!(g.command_repair(0, dozer, bridge), "repair accepted");
    g.run_frames(600);
    assert!(!g.terrain().bridges()[0].destroyed, "the dozer repaired the bridge");
}
