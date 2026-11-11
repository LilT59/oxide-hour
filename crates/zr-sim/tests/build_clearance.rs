//! Build placement rules of `BuildAssistant::isLocationClearOfObjects` and `clearRemovableForConstruction`
//! (synthetic data, no game install): factory exit rectangles, `FactoryExtraBibWidth`, the immobile rule, the
//! skirmish margin, and `KINDOF_CLEARED_BY_BUILD` / shrubbery removal.

use std::f32::consts::PI;
use std::sync::Arc;

use zr_sim::skirmish_ai::SkirmishAiConfig;
use zr_sim::template::{kind_of, GeometryShape, LocomotorKind, LocomotorParams};
use zr_sim::{Coord3D, GameLogic, ObjectTemplate, TemplateSource, TerrainLogic};

struct FixedStore(Vec<Arc<ObjectTemplate>>);

impl TemplateSource for FixedStore {
    fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
        self.0.iter().find(|t| t.name == name).cloned()
    }
}

/// A box of half extents `half` (both radii) with the given kind-of bits.
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

/// A barracks-sized immobile structure (half extent 15) with a factory exit `exit` wide.
fn factory(name: &str, exit: f32) -> ObjectTemplate {
    let mut t = boxed(name, kind_of::STRUCTURE | kind_of::IMMOBILE, 15.0);
    t.factory_exit_width = exit;
    t
}

fn mobile(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.geometry.major_radius = 5.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 60.0, 100.0, 100.0, 360.0));
    t
}

fn world(templates: Vec<ObjectTemplate>, money: u32) -> GameLogic {
    let store = FixedStore(templates.into_iter().map(Arc::new).collect());
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, money);
    g.set_template_source(Arc::new(store));
    g
}

/// `isLocationClearOfObjects` loop one: an immobile object blocks even when it is neither a structure nor
/// selectable. The port used to exempt such scenery (its "ambient" exception); `BuildAssistant.cpp:729` has
/// no such rule.
#[test]
fn an_immobile_object_blocks_even_when_it_is_not_a_structure() {
    let mut g = world(vec![boxed("Post", kind_of::IMMOBILE, 5.0), factory("Site", 0.0)], 0);
    let post = g.find_template("Post").unwrap();
    g.create_object(&post, Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let site = g.find_template("Site").unwrap();
    assert!(!g.is_location_legal_to_build(&site, &Coord3D::new(200.0, 200.0, 0.0), 0.0), "a post under the footprint");
    assert!(g.is_location_legal_to_build(&site, &Coord3D::new(300.0, 100.0, 0.0), 0.0), "clear ground");
}

/// `isLocationClearOfObjects` loop two, the existing factory's exit against this site's body (`:886`'s
/// `checkHisExit` branch): the exit is centred `major + exit / 2` = 35 ahead of the body, with half width 20,
/// so it covers x in [215, 255].
#[test]
fn a_factory_exit_keeps_its_lane_clear() {
    let mut g = world(vec![factory("Factory", 40.0), factory("Site", 0.0)], 0);
    let f = g.find_template("Factory").unwrap();
    g.create_object(&f, Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let site = g.find_template("Site").unwrap();
    assert!(!g.is_location_legal_to_build(&site, &Coord3D::new(250.0, 200.0, 0.0), 0.0), "in the exit lane");
    assert!(g.is_location_legal_to_build(&site, &Coord3D::new(300.0, 200.0, 0.0), 0.0), "beyond the exit");
    assert!(g.is_location_legal_to_build(&site, &Coord3D::new(120.0, 200.0, 0.0), 0.0), "behind the factory");
}

/// The new factory's own exit against an existing structure's body (`:877`, `checkMyExit`).
#[test]
fn a_new_factory_exit_must_clear_an_existing_structure() {
    let mut g = world(vec![factory("Factory", 40.0), factory("Barracks", 0.0)], 0);
    let f = g.find_template("Factory").unwrap();
    let at = Coord3D::new(200.0, 200.0, 0.0);
    assert!(g.is_location_legal_to_build(&f, &at, 0.0), "empty ground");
    let b = g.find_template("Barracks").unwrap();
    g.create_object(&b, Coord3D::new(250.0, 200.0, 0.0), 0.0, 0);
    assert!(!g.is_location_legal_to_build(&f, &at, 0.0), "its exit runs into the barracks");
}

/// Two factory exits facing each other (`:895`, both exits): the second factory faces back at the first, so
/// its exit covers [220, 260] and meets the first exit at [215, 255]. Turned away, the same site is clear.
#[test]
fn two_factory_exits_may_not_meet() {
    let mut g = world(vec![factory("Factory", 40.0)], 0);
    let f = g.find_template("Factory").unwrap();
    g.create_object(&f, Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    assert!(!g.is_location_legal_to_build(&f, &Coord3D::new(275.0, 200.0, 0.0), PI), "facing the first factory");
    assert!(g.is_location_legal_to_build(&f, &Coord3D::new(275.0, 200.0, 0.0), 0.0), "facing away");
}

/// `FactoryExtraBibWidth` grows the body box an immobile structure keeps clear (`:838-843`): a box's major
/// grows by the bib, so a 30 bib on a half-15 body reaches x = 155..245. A plain body reaches 185..215.
#[test]
fn factory_extra_bib_width_widens_the_body_it_keeps_clear() {
    let mut bibbed = boxed("Bibbed", kind_of::STRUCTURE | kind_of::IMMOBILE, 15.0);
    bibbed.factory_extra_bib_width = 30.0;
    let plain = boxed("Plain", kind_of::STRUCTURE | kind_of::IMMOBILE, 15.0);
    let site = boxed("Site", kind_of::STRUCTURE | kind_of::IMMOBILE, 15.0);
    let mut g = world(vec![bibbed, plain, site], 0);
    let b = g.find_template("Bibbed").unwrap();
    g.create_object(&b, Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let p = g.find_template("Plain").unwrap();
    g.create_object(&p, Coord3D::new(200.0, 330.0, 0.0), 0.0, 0);
    let s = g.find_template("Site").unwrap();
    assert!(!g.is_location_legal_to_build(&s, &Coord3D::new(255.0, 200.0, 0.0), 0.0), "inside the bib");
    assert!(g.is_location_legal_to_build(&s, &Coord3D::new(270.0, 200.0, 0.0), 0.0), "clear of the bib");
    assert!(g.is_location_legal_to_build(&s, &Coord3D::new(255.0, 330.0, 0.0), 0.0), "the plain body clears at 255");
}

/// Exit rectangles are checked only for an immobile structure (`:874`): a mobile structure's exit does not block.
#[test]
fn only_an_immobile_structure_keeps_an_exit_clear() {
    let mut mobile_structure = factory("Factory", 40.0);
    mobile_structure.kind_of = kind_of::STRUCTURE;
    let mut g = world(vec![mobile_structure, factory("Site", 0.0)], 0);
    let f = g.find_template("Factory").unwrap();
    g.create_object(&f, Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let site = g.find_template("Site").unwrap();
    assert!(g.is_location_legal_to_build(&site, &Coord3D::new(250.0, 200.0, 0.0), 0.0));
}

/// The skirmish margin (`:783-789`): `myExtraWidth` becomes 30 and `myFactoryExitWidth` loses that 30. The
/// neutral exit (40 wide) reaches the barracks at 250..280; the skirmish exit (10 wide) and body (grown to
/// 155..245) do not.
#[test]
fn a_skirmish_ai_takes_its_margin_out_of_the_exit() {
    let mut g = world(vec![factory("Factory", 40.0), factory("Barracks", 0.0)], 0);
    let b = g.find_template("Barracks").unwrap();
    g.create_object(&b, Coord3D::new(265.0, 200.0, 0.0), 0.0, 0);
    let f = g.find_template("Factory").unwrap();
    let at = Coord3D::new(200.0, 200.0, 0.0);
    assert!(!g.is_location_legal_to_build_for(&f, &at, 0.0, None, None), "neutral: the exit reaches the barracks");
    g.add_ai_player(0, SkirmishAiConfig::default());
    assert!(g.is_location_legal_to_build_for(&f, &at, 0.0, None, Some(0)), "skirmish: the margin shortens the exit");
}

/// `KINDOF_CLEARED_BY_BUILD` and shrubbery are not obstacles, and `buildObjectNow` destroys them (`:346`,
/// `:1378`). The fence is immobile, so only the removable rule lets the site through.
#[test]
fn cleared_objects_do_not_block_and_construction_removes_them() {
    let mut fence = boxed("Fence", kind_of::IMMOBILE, 5.0);
    fence.cleared_by_build = true;
    let shrub = boxed("Shrub", kind_of::SHRUBBERY | kind_of::IMMOBILE, 5.0);
    let mut site = boxed("Barracks", kind_of::STRUCTURE | kind_of::IMMOBILE, 15.0);
    site.build_cost = 300;
    site.build_time_frames = 10;
    let mut g = world(vec![mobile("Dozer"), fence, shrub, site], 1000);
    let dozer_t = g.find_template("Dozer").unwrap();
    let dozer = g.create_object(&dozer_t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let fence_t = g.find_template("Fence").unwrap();
    let f = g.create_object(&fence_t, Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let shrub_t = g.find_template("Shrub").unwrap();
    let s = g.create_object(&shrub_t, Coord3D::new(210.0, 200.0, 0.0), 0.0, 0);
    let site_t = g.find_template("Barracks").unwrap();
    let at = Coord3D::new(200.0, 200.0, 0.0);
    assert!(g.is_location_legal_to_build(&site_t, &at, 0.0), "removable objects do not block");
    let built = g.construct(dozer, "Barracks", at, 0.0).expect("construct over removable objects");
    g.update();
    assert!(g.find_object(f).is_none(), "the fence is cleared");
    assert!(g.find_object(s).is_none(), "the shrubbery is cleared");
    assert!(g.find_object(built).is_some(), "the building stands");
}

/// `clearRemovableForConstruction` skips `KINDOF_ALWAYS_SELECTABLE` objects (`BuildAssistant.cpp:1408`), but
/// they still do not block the site (`isRemovableForConstruction` is true for them).
#[test]
fn always_selectable_cleared_objects_survive_construction() {
    let mut marker = boxed("Marker", kind_of::IMMOBILE, 5.0);
    marker.cleared_by_build = true;
    marker.always_selectable = true;
    let mut site = boxed("Barracks", kind_of::STRUCTURE | kind_of::IMMOBILE, 15.0);
    site.build_cost = 300;
    site.build_time_frames = 10;
    let mut g = world(vec![mobile("Dozer"), marker, site], 1000);
    let dozer_t = g.find_template("Dozer").unwrap();
    let dozer = g.create_object(&dozer_t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let marker_t = g.find_template("Marker").unwrap();
    let m = g.create_object(&marker_t, Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let at = Coord3D::new(200.0, 200.0, 0.0);
    let site_t = g.find_template("Barracks").unwrap();
    assert!(g.is_location_legal_to_build(&site_t, &at, 0.0));
    assert!(g.construct(dozer, "Barracks", at, 0.0).is_some());
    g.update();
    assert!(g.find_object(m).is_some(), "always-selectable objects are not cleared");
}
