//! `AIPlayer::buildStructureWithDozer`'s placement spiral (`AIPlayer.cpp:563-622`, AN3 seed 1 f1567).
//!
//! The skirmish AI adds `2*PATHFIND_CELL_SIZE_F` to `posOffset` in the loop *body* and the `for` loop
//! increments it by another `2*PATHFIND_CELL_SIZE_F`, so the rings the candidates sit on are 10, 30,
//! 50, ... units from the requested spot, not 10, 20, 30, ... . The port had only the body increment,
//! so it accepted an earlier ring than the engine: the AN3 AmericaWarFactory landed one cell diagonal
//! away from the C++'s site (817.4610,383.9408 against 827.4611,393.9408).

use std::sync::Arc;

use zr_sim::skirmish_ai::{AiBuildOrder, SkirmishAiConfig};
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

fn structure(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;
    t.max_health = 500.0;
    t.initial_health = 500.0;
    t.geometry.shape = GeometryShape::Box;
    t.geometry.major_radius = 12.0;
    t.geometry.minor_radius = 12.0;
    t.build_cost = 100;
    t.build_time_frames = 5;
    t
}

/// A half-unit-radius sphere structure on the requested site. It is the player's own, so the
/// enemy-only gate (`NO_ENEMY_OBJECT_OVERLAP`) passes and only the full object test starts the
/// spiral. A non-box blocker's grown bounds are its major radius only (`BuildAssistant.cpp:796-800`),
/// so unlike a box its body is not widened by the skirmish margin.
fn blocker() -> ObjectTemplate {
    let mut t = structure("Blocker");
    t.geometry.shape = GeometryShape::Sphere;
    t.geometry.major_radius = 0.5;
    t.geometry.minor_radius = 0.5;
    t
}

fn world() -> GameLogic {
    let mut dozer = mobile("Dozer");
    dozer.kind_of = kind_of::DOZER;
    dozer.dozer = Some(zr_sim::dozer::DozerTemplate::default());
    let factory = structure("WarFactory");
    let enemy = structure("EnemyBase");
    let store = FixedStore(vec![dozer, factory, enemy, blocker()].into_iter().map(Arc::new).collect());
    let mut g = GameLogic::new(TerrainLogic::flat(600, 600, 0), 1);
    g.add_player(0, 10000);
    g.add_player(1, 10000);
    g.set_enemies(0, 1);
    g.set_template_source(Arc::new(store));
    g.scripts_mut().set_player_name("p0", 0);
    g.scripts_mut().set_player_name("p1", 1);
    g.scripts_mut().load_default_team_protos(&[("teamP0", "p0"), ("teamP1", "p1")]);
    g.init_from_sides(&[("teamP0", 0), ("teamP1", 1)]);
    let dozer = g.find_template("Dozer").unwrap();
    g.create_object(&dozer, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let enemy = g.find_template("EnemyBase").unwrap();
    g.create_object(&enemy, Coord3D::new(560.0, 560.0, 0.0), 0.0, 1);
    g
}

fn ai_config() -> SkirmishAiConfig {
    SkirmishAiConfig {
        build_list: vec![AiBuildOrder {
            template: "WarFactory".into(),
            location: (300.0, 300.0),
            angle: 0.0,
            initially_built: false,
            rebuilds: 1,
            automatically_build: true,
        }],
        resource_gatherers: 1,
        base_defense: None,
        gatherer_template: "SupplyTruck".into(),
        gatherer_factory: "WarFactory".into(),
        attack_units: Vec::new(),
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

/// The blocker sits 25 units diagonally from the requested spot: it rejects the exact site and all of
/// the 10-unit ring (each candidate is within its 48-unit structure query), but the first sample of
/// the old 20-unit ring is 45 units out in y, past the skirmish margin. The engine (rings 10, 30,
/// 50, ...) never tests that 20-unit sample and takes `(280,270)` from its 30-unit ring; the port
/// before this fix accepted `(290,280)` from the 20-unit ring first. Both samples are legal under the
/// skirmish margin, so the difference is the candidate order, not the clearance test.
#[test]
fn the_build_site_spiral_uses_the_engines_odd_rings() {
    let mut g = world();
    let blocker = g.find_template("Blocker").unwrap();
    g.create_object(&blocker, Coord3D::new(325.0, 325.0, 0.0), 0.0, 0);
    let factory = g.find_template("WarFactory").unwrap();
    // The old 20-unit ring's first sample and the engine's 30-unit ring's first sample both clear the
    // blocker with the skirmish AI's 3-cell margin (the test's own builder margin).
    assert!(g.is_location_legal_to_build_for(&factory, &Coord3D::new(290.0, 280.0, 0.0), 0.0, Some(0), Some(0)), "the old ring's sample is clear");
    assert!(g.is_location_legal_to_build_for(&factory, &Coord3D::new(280.0, 270.0, 0.0), 0.0, Some(0), Some(0)), "the engine's ring sample is clear");

    g.add_ai_player(0, ai_config());
    g.ai_build_specific(0, "WarFactory");
    for _ in 0..10 {
        g.run_frames(1);
        if let Some(o) = g.objects().find(|o| o.player == 0 && o.template.name == "WarFactory") {
            assert_eq!((o.position.x, o.position.y), (280.0, 270.0), "the engine's 30-unit ring sample, not the 20-unit one");
            return;
        }
    }
    eprintln!("fail counts: {:?}", g.ai_structure_fail_counts());
    eprintln!("ai: {:?}", g.ai_player(0).map(|a| (&a.built, &a.priority, a.ready_to_build_structure, a.build_delay, a.structures_built)));
    eprintln!("objects: {:?}", g.objects().map(|o| (o.id, o.template.name.clone(), o.player)).collect::<Vec<_>>());
    panic!("the AI did not start the war factory within 10 frames");
}
