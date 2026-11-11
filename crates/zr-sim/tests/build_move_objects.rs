//! `BuildAssistant::moveObjectsForConstruction` (`BuildAssistant.cpp:1421-1494`) and its call in `buildObjectNow`
//! (`:346-353`), through the dozer's `construct` (synthetic data, no game install): neutral and allied units in the
//! footprint are ordered out, an enemy or an unmovable unit fails a human's order but not an AI's, and immobile
//! objects and mines are handled as the engine does.

use std::sync::Arc;

use zr_sim::ai_states::AiStateType;
use zr_sim::damage::Relationship;
use zr_sim::skirmish_ai::SkirmishAiConfig;
use zr_sim::template::{kind_of, GeometryShape, LocomotorKind, LocomotorParams};
use zr_sim::{Coord3D, GameLogic, ObjectId, ObjectTemplate, TemplateSource, TerrainLogic};

struct FixedStore(Vec<Arc<ObjectTemplate>>);

impl TemplateSource for FixedStore {
    fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
        self.0.iter().find(|t| t.name == name).cloned()
    }
}

/// A box of half extents `half` with the given kind-of bits.
fn boxed(name: &str, kind: u64, half: f32) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind;
    t.max_health = 500.0;
    t.initial_health = 500.0;
    t.geometry.shape = GeometryShape::Box;
    t.geometry.major_radius = half;
    t.geometry.minor_radius = half;
    t
}

/// A small mobile unit with a locomotor, so it has an AI interface (`ai_handle`).
fn mobile(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.geometry.major_radius = 5.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 60.0, 100.0, 100.0, 360.0));
    t
}

/// A unit with no locomotor and no AI interface: `getAIUpdateInterface()` is null in the engine.
fn inert_unit(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.geometry.major_radius = 5.0;
    t
}

/// The construction site: a barracks-sized structure (half extent 15) that takes 10 frames to build.
fn site_template() -> ObjectTemplate {
    let mut t = boxed("Barracks", kind_of::STRUCTURE | kind_of::IMMOBILE, 15.0);
    t.build_cost = 300;
    t.build_time_frames = 10;
    t
}

/// Players 0 (the builder) and 1 (the unit owner). Player 1 is neutral to player 0 unless a test sets a relation.
fn world(templates: Vec<ObjectTemplate>, money: u32) -> GameLogic {
    let store = FixedStore(templates.into_iter().map(Arc::new).collect());
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, money);
    g.add_player(1, 0);
    g.set_template_source(Arc::new(store));
    g
}

/// The site centre used by every test.
fn site() -> Coord3D {
    Coord3D::new(200.0, 200.0, 0.0)
}

/// Spawn the dozer for player 0 at a point well away from the site.
fn spawn_dozer(g: &mut GameLogic) -> ObjectId {
    let t = g.find_template("Dozer").unwrap();
    g.create_object(&t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0)
}

/// `BuildAssistant.cpp:1469-1474`: a unit ordered out lands `GameLogicRandomValueReal(0.5, 1.5) * radius` from the
/// site, where `radius = 1.4 * sqrt(major^2 + minor^2)` of the footprint box (`:1438-1439`). The Barracks box is
/// 15 by 15, so `radius` is `1.4 * sqrt(450)`.
fn assert_goal_in_move_band(goal: Coord3D) {
    let radius = (15.0f32 * 15.0 + 15.0 * 15.0).sqrt() * 1.4;
    let d = ((goal.x - site().x).powi(2) + (goal.y - site().y).powi(2)).sqrt();
    assert!(d >= 0.5 * radius - 0.01 && d <= 1.5 * radius + 0.01, "goal {d} from the site outside [{}, {}]", 0.5 * radius, 1.5 * radius);
}

/// An allied unit in the footprint is ordered out with `aiMoveToPositionEvenIfSleeping`, and the order lands in the
/// engine's distance band. The order is `CMD_FROM_AI` and reaches `AI_MOVE_TO` (`AIUpdate.cpp:2657`, `:2889`).
#[test]
fn an_allied_unit_in_the_footprint_is_ordered_out() {
    let mut g = world(vec![mobile("Dozer"), mobile("Soldier"), site_template()], 1000);
    g.set_relationship_symmetric(0, 1, Relationship::Allies);
    let soldier_t = g.find_template("Soldier").unwrap();
    let soldier = g.create_object(&soldier_t, site(), 0.0, 1);
    let dozer = spawn_dozer(&mut g);

    assert!(g.construct(dozer, "Barracks", site(), 0.0).is_some(), "an ally does not block the order");
    let s = g.find_object(soldier).unwrap();
    assert_eq!(s.ai.machine.current, AiStateType::MoveTo, "the ally was ordered to move");
    assert_goal_in_move_band(s.ai.machine.goal_position);
}

/// The ordered ally walks out of the footprint: after the order it stands outside the 15-half-extent box.
/// The engine does not guarantee this (a short distance can land inside the box), so the seed is fixed and the test
/// checks the position the engine's own draws give.
#[test]
fn the_ordered_ally_walks_out_of_the_footprint() {
    let mut g = world(vec![mobile("Dozer"), mobile("Soldier"), site_template()], 1000);
    g.set_relationship_symmetric(0, 1, Relationship::Allies);
    let soldier_t = g.find_template("Soldier").unwrap();
    let soldier = g.create_object(&soldier_t, site(), 0.0, 1);
    let dozer = spawn_dozer(&mut g);
    assert!(g.construct(dozer, "Barracks", site(), 0.0).is_some());
    for _ in 0..300 {
        g.update();
    }
    let p = g.find_object(soldier).unwrap().position;
    let (dx, dy) = ((p.x - site().x).abs(), (p.y - site().y).abs());
    assert!(dx > 15.0 || dy > 15.0, "the ally stands at ({}, {}), still inside the footprint", p.x, p.y);
}

/// An enemy in the footprint is not moved, and it blocks a human's order (`buildObjectNow` returns null, `:352-353`).
#[test]
fn an_enemy_in_the_footprint_blocks_a_human_order() {
    let mut g = world(vec![mobile("Dozer"), mobile("Tank"), site_template()], 1000);
    g.set_relationship_symmetric(0, 1, Relationship::Enemies);
    let tank_t = g.find_template("Tank").unwrap();
    let tank = g.create_object(&tank_t, site(), 0.0, 1);
    let dozer = spawn_dozer(&mut g);

    assert!(g.construct(dozer, "Barracks", site(), 0.0).is_none(), "an enemy in the footprint fails the order");
    let t = g.find_object(tank).unwrap();
    assert_ne!(t.ai.machine.current, AiStateType::MoveTo, "an enemy is never ordered out");
}

/// A neutral unit with no AI interface cannot be moved, so `moveObjectsForConstruction` returns false and a
/// human's order fails. The unit is left where it stands (`BuildAssistant.cpp:1486-1494`).
#[test]
fn a_neutral_unit_with_no_ai_blocks_a_human_order() {
    let mut g = world(vec![mobile("Dozer"), inert_unit("Rock"), site_template()], 1000);
    let rock_t = g.find_template("Rock").unwrap();
    let rock = g.create_object(&rock_t, site(), 0.0, 1);
    let dozer = spawn_dozer(&mut g);

    assert!(g.construct(dozer, "Barracks", site(), 0.0).is_none(), "an unmovable neutral unit fails the order");
    assert_eq!(g.find_object(rock).unwrap().position, site(), "the unit was not moved");
}

/// `buildObjectNow` ignores the failed move for an AI (`BuildAssistant.cpp:352-353`): "ai gets to cheat". The same
/// unmovable neutral unit then does not stop the AI's build.
#[test]
fn an_ai_builds_over_an_unmovable_neutral_unit() {
    let mut g = world(vec![mobile("Dozer"), inert_unit("Rock"), site_template()], 1000);
    g.add_ai_player(0, SkirmishAiConfig::default());
    let rock_t = g.find_template("Rock").unwrap();
    g.create_object(&rock_t, site(), 0.0, 1);
    let dozer = spawn_dozer(&mut g);
    assert!(g.construct(dozer, "Barracks", site(), 0.0).is_some(), "the AI builds regardless of the failed move");
}

/// An immobile ally has an AI interface, so `moveObjectsForConstruction` does not count it as unmovable, but
/// `privateMoveToPosition` returns for an immobile unit (`AIUpdate.cpp:2891`, `isMobile() == FALSE`). It is not moved.
/// The site is still illegal because an immobile object blocks (`BuildAssistant.cpp:729`), so the order fails.
#[test]
fn an_immobile_object_is_not_moved() {
    let mut post = mobile("Post");
    post.kind_of = kind_of::IMMOBILE;
    let mut g = world(vec![mobile("Dozer"), post, site_template()], 1000);
    g.set_relationship_symmetric(0, 1, Relationship::Allies);
    let post_t = g.find_template("Post").unwrap();
    let post_id = g.create_object(&post_t, site(), 0.0, 1);
    let dozer = spawn_dozer(&mut g);

    assert!(g.construct(dozer, "Barracks", site(), 0.0).is_none(), "the immobile ally still blocks the site");
    let p = g.find_object(post_id).unwrap();
    assert_eq!(p.position, site(), "the immobile object is not moved");
    assert_ne!(p.ai.machine.current, AiStateType::MoveTo, "no move order reaches an immobile object");
}

/// Mines and inert objects are skipped by the move (`BuildAssistant.cpp:1444-1449`): a neutral mine in the footprint
/// neither fails the order nor gets an order. Without the skip, the neutral mine (no AI) would count as unmovable.
#[test]
fn a_mine_is_built_over_and_not_moved() {
    let mine = boxed("Mine", kind_of::MINE, 3.0);
    let mut g = world(vec![mobile("Dozer"), mine, site_template()], 1000);
    let mine_t = g.find_template("Mine").unwrap();
    let mine_id = g.create_object(&mine_t, site(), 0.0, 1);
    let dozer = spawn_dozer(&mut g);

    assert!(g.construct(dozer, "Barracks", site(), 0.0).is_some(), "a neutral mine does not fail the order");
    assert_eq!(g.find_object(mine_id).unwrap().position, site(), "the mine is not moved");
}

/// The draws are seeded and in a fixed order, so two identical worlds give the same destinations to the bit.
#[test]
fn the_move_destinations_are_deterministic() {
    let run = || {
        let mut g = world(vec![mobile("Dozer"), mobile("Soldier"), site_template()], 1000);
        g.set_relationship_symmetric(0, 1, Relationship::Allies);
        let soldier_t = g.find_template("Soldier").unwrap();
        let soldier = g.create_object(&soldier_t, site(), 0.0, 1);
        let dozer = spawn_dozer(&mut g);
        assert!(g.construct(dozer, "Barracks", site(), 0.0).is_some());
        let goal = g.find_object(soldier).unwrap().ai.machine.goal_position;
        (goal.x.to_bits(), goal.y.to_bits(), goal.z.to_bits())
    };
    assert_eq!(run(), run());
}
