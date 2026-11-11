//! Lane 151 (wave 17): `GrantUpgradeCreate` through real object creation paths.
//!
//! `onCreate` runs inside `ThingFactory::newObject` (`ThingFactory.cpp:333-341`); `onBuildComplete`
//! runs on the finished object -- a dozer-built structure (`DozerAIUpdate.cpp:599`), a factory's unit
//! (`ProductionUpdate.cpp:800`), a starting building (`GameLogic.cpp:511`) or a map object
//! (`GameLogic.cpp:1953`). Synthetic data only.

use std::sync::Arc;

use zr_sim::body::MaxHealthChangeType;
use zr_sim::template::{kind_of, GeometryShape, LocomotorKind, LocomotorParams, GrantUpgradeCreateTemplate};
use zr_sim::upgrade::{SimUpgradeModule, UpgradeEffect};
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

/// A barracks whose `GrantUpgradeCreate` grants `Upgrade_Radar` (`ExemptStatus = UNDER_CONSTRUCTION`,
/// so nothing at construction start) and whose upgrade module turns that into 100 extra max health.
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
    t.upgrade_modules = vec![Arc::new(SimUpgradeModule {
        triggered_by: vec!["Upgrade_Radar".into()],
        conflicts_with: vec![],
        removes_upgrades: vec![],
        requires_all_triggers: false,
        effect: UpgradeEffect::MaxHealth { add_max_health: 100.0, change_type: MaxHealthChangeType::SameCurrentHealth },
    })];
    t.grant_upgrade_create =
        vec![GrantUpgradeCreateTemplate { upgrade: "Upgrade_Radar".into(), player_upgrade: false, exempt_under_construction: true }];
    t
}

fn world() -> GameLogic {
    let store = FixedStore(vec![Arc::new(dozer_template()), Arc::new(barracks())]);
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 1000);
    g.set_template_source(Arc::new(store));
    g
}

fn max_health(g: &GameLogic, id: zr_sim::ObjectId) -> f32 {
    g.find_object(id).and_then(|o| o.body.as_ref()).map_or(0.0, |b| b.max_health())
}

/// `GrantUpgradeCreate::onCreate` skips the scaffold (`ExemptStatus = UNDER_CONSTRUCTION`), and
/// `DozerAIUpdate`'s completion loop (`DozerAIUpdate.cpp:596-606`) then runs `onBuildComplete`, which
/// grants it. Without the loop the finished building never gets its upgrade.
#[test]
fn a_dozer_built_structure_gets_its_grant_upgrade_at_build_complete() {
    let mut g = world();
    let t = g.find_template("Dozer").expect("dozer template");
    let dozer = g.create_object(&t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let b = g.construct(dozer, "Barracks", Coord3D::new(200.0, 100.0, 0.0), 0.0).expect("construct");
    for _ in 0..400 {
        g.run_frames(1);
        let Some(o) = g.find_object(b) else { break };
        if !o.under_construction {
            break;
        }
        assert!(o.upgrades.is_empty(), "nothing is granted while the building is a scaffold");
    }
    let o = g.find_object(b).expect("the building is finished");
    assert!(!o.under_construction, "the dozer finished the building");
    assert!(o.upgrades.contains("Upgrade_Radar"), "onBuildComplete granted the upgrade: {:?}", o.upgrades);
    assert!((max_health(&g, b) - 600.0).abs() < 0.01, "the upgrade module reacted");
}

/// A factory's produced unit runs its create modules' `onBuildComplete` right after
/// `Player::onUnitCreated` (`ProductionUpdate.cpp:791-805`), not `onCreate` alone: a
/// `GrantUpgradeCreate` without `ExemptStatus` (the shipped `GLAVehicleRadarVan` ->
/// `Upgrade_GLARadar`) only grants at the exit. `SpawnBehavior`'s `createSpawn` does not run the
/// loop, so the spawn path must not grant it either.
#[test]
fn a_produced_unit_gets_its_grant_upgrade_at_the_factory_exit() {
    let mut factory = ObjectTemplate::new("Factory");
    factory.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;
    factory.max_health = 500.0;
    factory.initial_health = 500.0;
    factory.geometry.major_radius = 20.0;
    factory.production = Some(zr_sim::production::ProductionTemplate { max_queue_entries: 5, ..Default::default() });
    let mut tank = ObjectTemplate::new("Tank");
    tank.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    tank.max_health = 100.0;
    tank.initial_health = 100.0;
    tank.geometry.major_radius = 5.0;
    tank.build_cost = 100;
    tank.build_time_frames = 10;
    tank.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 60.0, 100.0, 100.0, 360.0));
    tank.upgrade_modules = vec![Arc::new(SimUpgradeModule {
        triggered_by: vec!["Upgrade_Radar".into()],
        conflicts_with: vec![],
        removes_upgrades: vec![],
        requires_all_triggers: false,
        effect: UpgradeEffect::MaxHealth { add_max_health: 100.0, change_type: MaxHealthChangeType::SameCurrentHealth },
    })];
    tank.grant_upgrade_create =
        vec![GrantUpgradeCreateTemplate { upgrade: "Upgrade_Radar".into(), player_upgrade: false, exempt_under_construction: false }];
    let store = FixedStore(vec![Arc::new(factory), Arc::new(tank)]);
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 1000);
    g.set_template_source(Arc::new(store));
    let f = g.find_template("Factory").unwrap();
    let factory = g.create_object(&f, Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    assert!(g.queue_unit(0, factory, "Tank"), "the unit is queued");
    let mut produced = None;
    for _ in 0..120 {
        g.run_frames(1);
        if let Some(o) = g.objects().find(|o| o.template.name == "Tank") {
            produced = Some(o.id);
            break;
        }
    }
    let id = produced.expect("the factory produced the unit");
    let o = g.find_object(id).unwrap();
    assert!(o.upgrades.contains("Upgrade_Radar"), "the exit's create-module loop granted the upgrade: {:?}", o.upgrades);
    assert!((max_health(&g, id) - 200.0).abs() < 0.01, "the upgrade module reacted");
}
