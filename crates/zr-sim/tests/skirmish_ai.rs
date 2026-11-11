//! Skirmish AI: build-list processing, gatherer upkeep and attack groups (synthetic data).

use std::sync::Arc;

use zr_sim::skirmish_ai::{AiBuildOrder, SkirmishAiConfig};
use zr_sim::supply::{DockKind, DockTemplate, SupplyTruckTemplate};
use zr_sim::template::{kind_of, GeometryShape, LocomotorKind, LocomotorParams};
use zr_sim::{Coord3D, GameLogic, ObjectTemplate, TemplateSource, TerrainLogic};

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

fn structure(name: &str, cost: i32, frames: u32) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;
    t.max_health = 500.0;
    t.initial_health = 500.0;
    t.geometry.shape = GeometryShape::Box;
    t.geometry.major_radius = 12.0;
    t.geometry.minor_radius = 12.0;
    t.build_cost = cost;
    t.build_time_frames = frames;
    t
}

fn command_center(name: &str) -> ObjectTemplate {
    let mut t = structure(name, 0, 0);
    t.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE | kind_of::COMMANDCENTER;
    t
}

fn order(template: &str, x: f32, y: f32) -> AiBuildOrder {
    AiBuildOrder {
        template: template.into(),
        location: (x, y),
        angle: 0.0,
        initially_built: false,
        rebuilds: 1,
        automatically_build: true,
    }
}

fn world() -> GameLogic {
    let mut dozer = mobile("Dozer");
    dozer.build_cost = 0;
    dozer.kind_of = kind_of::DOZER;
    // The AI's dozer builds through `DozerAIUpdate`'s machine, so it needs that module data.
    dozer.dozer = Some(zr_sim::dozer::DozerTemplate::default());
    let mut power = structure("PowerPlant", 100, 5);
    power.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE | kind_of::FS_POWER;
    power.energy_production = 10;
    let mut center = structure("SupplyCenter", 100, 5);
    center.dock = Some(DockTemplate { kind: DockKind::Center, starting_boxes: 0, delete_when_empty: false, number_approach_positions: 5, allows_passthrough: true });
    center.production = Some(zr_sim::production::ProductionTemplate::default());
    let mut factory = structure("WarFactory", 100, 5);
    factory.production = Some(zr_sim::production::ProductionTemplate::default());
    let mut truck = mobile("SupplyTruck");
    truck.build_cost = 50;
    truck.supply_truck = Some(SupplyTruckTemplate { max_boxes: 1, center_action_delay: 1, warehouse_action_delay: 1, scan_distance: 100.0 });
    let mut tank = mobile("Tank");
    tank.build_cost = 50;
    tank.kind_of = kind_of::VEHICLE | kind_of::CAN_ATTACK;
    let mut enemy = structure("EnemyBase", 0, 0);
    enemy.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;

    let store = FixedStore(
        vec![dozer, power, center, factory, truck, tank, enemy, command_center("CommandCenter")]
            .into_iter()
            .map(Arc::new)
            .collect(),
    );
    let mut g = GameLogic::new(TerrainLogic::flat(600, 600, 0), 1);
    g.add_player(0, 10000);
    g.add_player(1, 10000);
    g.set_enemies(0, 1);
    g.set_template_source(Arc::new(store));
    // Each player's default team (`Player::setDefaultTeam`): a supply truck's work order is on it.
    g.scripts_mut().set_player_name("p0", 0);
    g.scripts_mut().set_player_name("p1", 1);
    g.scripts_mut().load_default_team_protos(&[("teamP0", "p0"), ("teamP1", "p1")]);
    g.init_from_sides(&[("teamP0", 0), ("teamP1", 1)]);
    // A dozer for player 0 and an enemy structure to attack.
    g.create_object(&g.find_template("Dozer").unwrap(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    g.create_object(&g.find_template("EnemyBase").unwrap(), Coord3D::new(500.0, 500.0, 0.0), 0.0, 1);
    g
}

fn ai_config() -> SkirmishAiConfig {
    SkirmishAiConfig {
        build_list: vec![order("PowerPlant", 250.0, 250.0), order("SupplyCenter", 150.0, 300.0), order("WarFactory", 300.0, 150.0)],
        resource_gatherers: 1,
        base_defense: None,
        gatherer_template: "SupplyTruck".into(),
        gatherer_factory: "SupplyCenter".into(),
        attack_units: vec!["Tank".into()],
        attack_factory: "WarFactory".into(),
        structure_seconds: 0,
        team_seconds: 5,
        rebuild_delay_seconds: 30,
        resources_wealthy: 2000,
        resources_poor: 500,
        structures_wealthy_mod: 1.0,
        structures_poor_mod: 1.0,
        team_wealthy_mod: 1.0,
        team_poor_mod: 1.0,
        team_resources_to_build: 1.0,
        supply_center_safe_radius: 250.0,
        max_recruit_distance: 500.0,
        skill_sets: Vec::new(),
        base_defense_extra_distance: 0.0,
        difficulty: zr_sim::script::Difficulty::Normal,
    }
}

#[test]
fn ai_builds_its_build_list() {
    let mut g = world();
    g.add_ai_player(0, ai_config());
    // `AISkirmishPlayer::processBaseBuilding`: an automatic non-power entry is never the chosen plan
    // (`canMakeUnit(dozer, bldgPlan)` with no plan is `CANMAKE_NO_PREREQ`), so the script requests
    // (`buildSpecificAIBuilding`) build the factory and the supply center. The power plant is forced.
    g.ai_build_specific(0, "SupplyCenter");
    g.ai_build_specific(0, "WarFactory");
    g.run_frames(900);
    assert!(g.objects().any(|o| o.player == 0 && o.template.name == "PowerPlant"), "power plant built");
    assert!(g.objects().any(|o| o.player == 0 && o.template.name == "SupplyCenter"), "supply center built");
    assert!(g.objects().any(|o| o.player == 0 && o.template.name == "WarFactory"), "war factory built");
    assert!(g.player(0).unwrap().money < 10000, "the AI spent money");
    assert!(g.ai_player(0).unwrap().structures_built >= 3);
}

#[test]
fn ai_keeps_gatherers_and_produces_an_attack() {
    let mut g = world();
    // `InitiallyBuilt` build-list entries are built fresh by `AISkirmishPlayer::newMap`
    // (`buildStructureNow`): no hand-placed objects, so the AI's own ids are the ones to query.
    let mut cfg = ai_config();
    cfg.build_list[1].initially_built = true;
    cfg.build_list[2].initially_built = true;
    g.add_ai_player(0, cfg);
    let center = g.objects().find(|o| o.player == 0 && o.template.name == "SupplyCenter").map(|o| o.id).expect("supply center built at map start");
    let factory = g.objects().find(|o| o.player == 0 && o.template.name == "WarFactory").map(|o| o.id).expect("war factory built at map start");
    g.run_frames(150);
    // `queueSupplyTruck`: the first gatherer is "automatic" (the center's own `SpawnBehavior` truck),
    // so the work order waits on the center. The test world has no spawner; produce it by hand.
    // `AIPlayer::queueSupplyTruck` (AIPlayer.cpp:223): the order is a team-queue entry on the default team.
    let queue = &g.ai_player(0).unwrap().team_build_queue;
    let orders: Vec<_> = queue.iter().flat_map(|q| q.work_orders.iter()).filter(|w| w.resource_gatherer).collect();
    assert_eq!(orders.len(), 1, "one gatherer work order: {queue:?}");
    assert_eq!(orders[0].factory, Some(center), "the first one is automatic (factory = the center)");
    assert_eq!(queue[0].key, "teamp0", "the entry is on the player's default team");
    assert!(queue[0].priority_build, "m_priorityBuild");
    assert!(g.queue_unit(0, center, "SupplyTruck"));
    g.run_frames(900);
    let truck = g.objects().find(|o| o.player == 0 && o.template.name == "SupplyTruck");
    assert!(truck.is_some_and(|t| t.supply_ai.preferred_dock == center), "gatherer assigned to the center");
    assert!(g.objects().any(|o| o.player == 0 && o.template.name == "Tank"), "attack unit produced");
    assert!(g.ai_player(0).unwrap().attacks_launched >= 1, "an attack was launched");
    let _ = factory;
}

/// `AISkirmishPlayer::adjustBuildList` + `newMap` (`AISkirmishPlayer.cpp:989-1134`): the placed
/// starting command center is destroyed (`onStructureUndone`, `removeObjectFromPathfindMap`,
/// `destroyObject`) and the build list's command center is built fresh by `buildStructureNow`, so the
/// old object's id is spent and the new building takes the next ids (it used to be reused, which made
/// every later object id one lower per AI player than the original's).
#[test]
fn ai_new_map_destroys_and_rebuilds_the_starting_command_center() {
    let mut g = world();
    let template = g.find_template("CommandCenter").unwrap();
    let placed = g.create_object(&template, Coord3D::new(250.0, 250.0, 0.0), 0.0, 0);
    let mut cfg = ai_config();
    let mut cc = order("CommandCenter", 150.0, 300.0);
    cc.initially_built = true;
    cfg.build_list.insert(0, cc);
    g.add_ai_player(0, cfg);
    assert!(g.find_object(placed).is_none(), "the placed command center is destroyed");
    let rebuilt = g
        .objects()
        .find(|o| o.player == 0 && o.template.name == "CommandCenter")
        .expect("the build list's command center is built at map start")
        .id;
    assert_ne!(rebuilt, placed);
    assert!(rebuilt.0 > placed.0, "the rebuilt command center's id follows the old one");
    let o = g.find_object(rebuilt).unwrap();
    assert_eq!((o.position.x, o.position.y), (150.0, 300.0));
    let ai = g.ai_player(0).unwrap();
    assert_eq!(ai.built[0], Some(rebuilt));
    assert_eq!(ai.structures_built, 1);
}

/// `AISkirmishPlayer::adjustBuildList` (`AISkirmishPlayer.cpp:997-1009`) walks
/// `TheGameLogic->getFirstObject()` and breaks at the first command center of the player, and
/// `GameLogic::registerObject` *prepends* to `m_objList` (`GameLogic.cpp:4137`): the newest command
/// center is the one destroyed, not the lowest id.
#[test]
fn ai_new_map_destroys_the_newest_command_center() {
    let mut g = world();
    let template = g.find_template("CommandCenter").unwrap();
    let older = g.create_object(&template, Coord3D::new(450.0, 450.0, 0.0), 0.0, 0);
    let newer = g.create_object(&template, Coord3D::new(250.0, 250.0, 0.0), 0.0, 0);
    let mut cfg = ai_config();
    let mut cc = order("CommandCenter", 150.0, 300.0);
    cc.initially_built = true;
    cfg.build_list.insert(0, cc);
    g.add_ai_player(0, cfg);
    assert!(g.find_object(older).is_some(), "the older command center survives");
    assert!(g.find_object(newer).is_none(), "the newest command center is destroyed");
}

#[test]
fn ai_prioritises_power_when_underpowered() {
    let mut g = world();
    // A consumer makes the player underpowered; the power plant should be chosen first.
    let mut consumer = structure("Consumer", 0, 0);
    consumer.energy_production = -5;
    g.create_object(&Arc::new(consumer), Coord3D::new(100.0, 200.0, 0.0), 0.0, 0);
    let mut config = ai_config();
    // Reverse the order so power is not simply the first entry.
    config.build_list = vec![order("WarFactory", 300.0, 150.0), order("PowerPlant", 250.0, 250.0)];
    g.add_ai_player(0, config);
    g.run_frames(600);
    assert!(g.objects().any(|o| o.player == 0 && o.template.name == "PowerPlant"), "power plant prioritised");
}

#[test]
fn ai_is_deterministic() {
    let run = || {
        let mut g = world();
        g.add_ai_player(0, ai_config());
        g.run_frames(600);
        g.crc()
    };
    assert_eq!(run(), run());
}
