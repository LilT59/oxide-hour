//! `GameLogic::adjust_placed_unit` goes through `Pathfinder::updateGoal`'s ground-movement dispatch
//! (`GameLogic.cpp:522-527` + `AIPathfind.cpp:9816-9827`): an air unit that is not doing ground
//! movement takes `updateAircraftGoal`, which removes the old goal and only marks cells for a
//! hover/wing aircraft (`isAircraftThatAdjustsDestination`, `AIUpdate.cpp:2408`). A thrust aircraft
//! therefore ends the placement with no ground goal cell. Synthetic data only.

use std::sync::Arc;

use zr_sim::template::{kind_of, GeometryShape, LocomotorKind, LocomotorParams};
use zr_sim::{Coord3D, GameLogic, ObjectTemplate, TemplateSource, TerrainLogic};

struct FixedStore(Vec<Arc<ObjectTemplate>>);

impl TemplateSource for FixedStore {
    fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
        self.0.iter().find(|t| t.name == name).cloned()
    }
}

fn jet() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Jet");
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.geometry.shape = GeometryShape::Box;
    t.geometry.major_radius = 5.0;
    t.geometry.minor_radius = 5.0;
    t.kind_of |= kind_of::AIRCRAFT;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Thrust, 60.0, 100.0, 100.0, 360.0));
    t
}

fn world() -> GameLogic {
    let store = FixedStore(vec![Arc::new(jet())]);
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 1000);
    g.set_template_source(Arc::new(store));
    g
}

/// A thrust aircraft placed off a cell centre is snapped by `adjustDestination` but takes no ground
/// goal: `updateAircraftGoal` removes it and the locomotor does not adjust destinations.
#[test]
fn a_placed_air_unit_takes_no_ground_goal() {
    let mut g = world();
    let t = g.find_template("Jet").expect("jet template");
    let id = g.create_object(&t, Coord3D::new(60.0, 300.0, 0.0), 0.0, 0);
    let before = g.find_object(id).unwrap().position;
    g.adjust_placed_unit(id);
    let after = g.find_object(id).unwrap().position;
    assert!(after.dist_2d(&before) > 0.0, "the adjust snapped the spot: {before:?} -> {after:?}");
    assert!(g.ai_pathfind_goal(id).is_none(), "a thrust aircraft keeps no ground goal cell");
}

/// The same placement on the ground keeps the ground goal (the dispatch's other arm).
#[test]
fn a_placed_ground_unit_keeps_its_goal() {
    let mut g = world();
    let mut tank = ObjectTemplate::new("Tank");
    tank.max_health = 100.0;
    tank.initial_health = 100.0;
    tank.geometry.shape = GeometryShape::Box;
    tank.geometry.major_radius = 5.0;
    tank.geometry.minor_radius = 5.0;
    tank.kind_of |= kind_of::VEHICLE;
    tank.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 60.0, 100.0, 100.0, 360.0));
    let id = g.create_object(&Arc::new(tank), Coord3D::new(60.0, 300.0, 0.0), 0.0, 0);
    g.adjust_placed_unit(id);
    assert!(g.ai_pathfind_goal(id).is_some(), "a ground placement keeps its goal cell");
}
