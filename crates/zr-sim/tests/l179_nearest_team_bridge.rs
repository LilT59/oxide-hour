//! Lane 179 (R21-13): `AIPlayer::buildSpecificBuildingNearestTeam` copies `newPos` from the team's location and
//! changes only x and y (AIPlayer.cpp:2119-2202), so every probe keeps the team member's z. The wiggle probes
//! run `TERRAIN_RESTRICTIONS`, and that refuses a site whose position is on a bridge layer
//! (`getLayerForDestination(worldPos) != LAYER_GROUND`, BuildAssistant.cpp:1036).
//!
//! A first member standing on a bridge deck (z = 10) therefore gets no order: its blocked spot's wiggle probes
//! are all on the bridge layer. The same team on the ground gets the order at the first legal cell, as the
//! lane 174 test shows. Synthetic data only.
//!
//! Revert checks: a ground z in the probe (the old `legal` closure) adds an order at (180, 180) and the first
//! test fails; dropping the layer check in `build.rs::terrain_allows_build` does the same.

use std::sync::Arc;

use zr_sim::bridge::Bridge;
use zr_sim::skirmish_ai::SkirmishAiConfig;
use zr_sim::template::{kind_of, GeometryShape};
use zr_sim::{Coord3D, GameLogic, ObjectTemplate, TemplateSource, TerrainLogic};

struct Store(Vec<Arc<ObjectTemplate>>);

impl TemplateSource for Store {
    fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
        self.0.iter().find(|t| t.name == name).cloned()
    }
}

fn trap() -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new("Trap");
    t.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.geometry.shape = GeometryShape::Cylinder;
    t.geometry.major_radius = 4.0;
    t.geometry.minor_radius = 4.0;
    Arc::new(t)
}

/// The same blocker as the lane 174 test: it makes the team's own spot illegal, so the wiggle runs.
fn block() -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new("Block");
    t.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;
    t.max_health = 1000.0;
    t.initial_health = 1000.0;
    t.geometry.shape = GeometryShape::Cylinder;
    t.geometry.major_radius = 12.0;
    t.geometry.minor_radius = 12.0;
    Arc::new(t)
}

/// A flat map with a bridge deck at z = 10 over the team's area. The wiggle reaches `2 * SUPPLY_CENTER_CLOSE_DIST`
/// (400 units, `posOffset < 400`), so the deck is made large enough to cover every probe, and the first legal
/// cell in the control is (180, 180) as in the lane 174 test.
fn world() -> GameLogic {
    let mut terrain = TerrainLogic::flat(400, 400, 0);
    terrain.add_bridge(Bridge::new(Coord3D::new(-500.0, 200.0, 10.0), Coord3D::new(1000.0, 200.0, 10.0), 1000.0, 2));
    let mut g = GameLogic::new(terrain, 1);
    g.add_player(0, 1000);
    g.set_template_source(Arc::new(Store(vec![trap(), block()])));
    g.add_ai_player(0, SkirmishAiConfig::default());
    g.create_object(&block(), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    g
}

/// A team member on the bridge deck: the spot is illegal, and every wiggle probe at its z is on the bridge layer,
/// so no order is prepended (the engine's `valid` stays false).
#[test]
fn a_team_on_a_bridge_deck_gets_no_nearest_team_order() {
    let mut g = world();
    g.ai_build_specific_nearest_team(0, "Trap", Coord3D::new(200.0, 200.0, 10.0));
    let ai = g.ai_player(0).expect("the AI player");
    assert!(ai.config.build_list.is_empty(), "every probe keeps z = 10, which is the bridge layer: {:?}", ai.config.build_list);
}

/// Control: the same blocked spot with the team on the ground (z = 0) is restricted by nothing, so the wiggle
/// finds the first legal cell (180, 180), as in the lane 174 test.
#[test]
fn a_team_on_the_ground_gets_the_order_at_the_first_legal_cell() {
    let mut g = world();
    g.ai_build_specific_nearest_team(0, "Trap", Coord3D::new(200.0, 200.0, 0.0));
    let ai = g.ai_player(0).expect("the AI player");
    assert_eq!(ai.config.build_list.len(), 1);
    assert_eq!((ai.config.build_list[0].location.0, ai.config.build_list[0].location.1), (180.0, 180.0));
}
