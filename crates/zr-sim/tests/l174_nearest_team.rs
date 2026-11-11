//! Lane 174: `AI_PLAYER_BUILD_TYPE_NEAREST_TEAM` / `AIPlayer::buildSpecificBuildingNearestTeam`
//! (`ScriptActions.cpp:1112-1122`, `Player.cpp:2416`, `AIPlayer.cpp:2109-2205`).
//!
//! The retail placement only adds a build order when the team's own spot is **illegal**: `valid` is
//! assigned inside the `isLocationLegalToBuild` failure branch (`AIPlayer.cpp:2204-2280`), so a team
//! standing on a legal spot adds nothing. The wiggle is the same growing-square scan as
//! `buildBySupplies` but with `2 * SUPPLY_CENTER_CLOSE_DIST` (20 cells) as the limit and no skirmish
//! double step.

use std::sync::Arc;

use zr_sim::skirmish_ai::SkirmishAiConfig;
use zr_sim::template::{kind_of, GeometryShape};
use zr_sim::{Coord3D, GameLogic, ObjectTemplate, TemplateSource, TerrainLogic};

struct Store(Vec<Arc<ObjectTemplate>>);

impl TemplateSource for Store {
    fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
        self.0.iter().find(|t| t.name == name).cloned()
    }
}

/// A cylinder like `GLADemoTrap` (`GeometryMajorRadius = 4.0`).
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

/// An immobile blocker big enough that the first wiggle ring (offset 10, corner distance 14.14)
/// still collides: 4 + 12 > 14.14, while the second ring (offset 20, distance 28.28) is clear.
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

fn world() -> GameLogic {
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 1000);
    g.set_template_source(Arc::new(Store(vec![trap(), block()])));
    g.add_ai_player(0, SkirmishAiConfig::default());
    g
}

/// `AIPlayer::buildSpecificBuildingNearestTeam` (`AIPlayer.cpp:2109-2205`): the team's spot is
/// blocked, so the wiggle scans `posOffset` 20, 40, ... and prepends a one-rebuild priority order at
/// the first legal cell. The first ring's six cells are all within 4 + 12 of the blocker; the second
/// ring's first cell (180, 180) is the one the C++ scan reaches first.
///
/// Revert check: pass `Some(builder)`/skip the wiggle (the old `build_specific` stand-in added
/// nothing here) and `config.build_list` stays empty.
#[test]
fn the_nearest_team_placement_wiggles_to_the_first_legal_cell() {
    let mut g = world();
    g.create_object(&block(), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    g.ai_build_specific_nearest_team(0, "Trap", Coord3D::new(200.0, 200.0, 0.0));
    let ai = g.ai_player(0).expect("the AI player");
    assert_eq!(ai.config.build_list.len(), 1, "one priority build order is prepended");
    let order = &ai.config.build_list[0];
    assert_eq!(order.template, "Trap");
    assert_eq!((order.location.0, order.location.1), (180.0, 180.0), "the first legal cell of the C++ wiggle");
    assert_eq!(order.rebuilds, 1, "`Player::addToPriorityBuildList` (Player.cpp:663) builds once");
    assert!(order.automatically_build, "`BuildListInfo` defaults `automaticallyBuild` to true");
    assert_eq!((order.angle, 0.0), (0.0, 0.0));
    assert!(ai.priority.contains(&0), "the new entry is a priority build");
}

/// The retail quirk at `AIPlayer.cpp:2204-2283`: `valid` is only set when the first probe fails, so a
/// team whose own position is legal adds **no** build order at all.
///
/// Revert check: initialise `valid = true` (or drop the branch) and the list gains an entry, failing
/// the `is_empty` assertion.
#[test]
fn a_team_on_a_legal_spot_adds_no_build_order() {
    let mut g = world();
    g.ai_build_specific_nearest_team(0, "Trap", Coord3D::new(200.0, 200.0, 0.0));
    let ai = g.ai_player(0).expect("the AI player");
    assert!(ai.config.build_list.is_empty(), "the retail bug keeps the list empty: {:?}", ai.config.build_list);
    assert!(ai.priority.is_empty());
}

/// An immobile, non-structure blocker (the object test's first loop blocks on `KINDOF_IMMOBILE`; a
/// non-structure keeps the second, structure-range loop out of the way).
fn peg() -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new("Peg");
    t.kind_of = kind_of::IMMOBILE;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.geometry.shape = GeometryShape::Cylinder;
    t.geometry.major_radius = 2.0;
    t.geometry.minor_radius = 2.0;
    Arc::new(t)
}

/// The C++ row scan (`AIPlayer.cpp:2224-2240`) does **not** break after its second test, so `valid`
/// and `newPos` end at the *last* x of the row, not the first legal cell. Here the first test of every
/// row cell is blocked by the pegs above it, the second test is legal, and only the last second test
/// survives: the order is (210, 210), whereas a break-on-first-hit port chooses (190, 210).
///
/// Revert check: put the `break 'wiggle` back inside the two-cell `for` and the location is (190, 210).
#[test]
fn the_row_scan_keeps_the_last_second_test() {
    let mut g = world();
    g.create_object(&peg(), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0); // makes the team spot illegal
    for x in [190.0, 200.0, 210.0] {
        g.create_object(&peg(), Coord3D::new(x, 193.0, 0.0), 0.0, 0);
    }
    g.ai_build_specific_nearest_team(0, "Trap", Coord3D::new(200.0, 200.0, 0.0));
    let ai = g.ai_player(0).expect("the AI player");
    assert_eq!(ai.config.build_list.len(), 1, "the row's last legal second test was kept");
    assert_eq!(
        (ai.config.build_list[0].location.0, ai.config.build_list[0].location.1),
        (210.0, 210.0),
        "the C++ leaves `newPos` at the row's last test"
    );
}

/// An unknown template is a silent return (`TheThingFactory->findTemplate` null, :2180-2186).
#[test]
fn an_unknown_template_adds_nothing() {
    let mut g = world();
    g.ai_build_specific_nearest_team(0, "NoSuchThing", Coord3D::new(200.0, 200.0, 0.0));
    assert!(g.ai_player(0).expect("the AI player").config.build_list.is_empty());
}
