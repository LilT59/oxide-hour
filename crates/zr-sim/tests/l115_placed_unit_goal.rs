//! `GameLogic::placeObjectAtPosition`'s `updateGoal` (`GameLogic.cpp:523-525`).
//!
//! A starting unit that the pathfinder snaps onto a cell (`adjustDestination`) also *reserves* that
//! cell as its goal (`Pathfinder::updateGoal(obj, &pos, LAYER_GROUND)`) before `setPosition`. The
//! reservation is what makes another unit's `checkForMovement` see the cell as an ally's goal
//! (`allyGoal`) and `checkDestination` refuse to usurp it (`AIPathfind.cpp:5078`). Synthetic data,
//! no game install.

use std::sync::Arc;

use zr_sim::pathfind::PathObject;
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::{Coord3D, GameLogic, ObjectId, ObjectTemplate, TemplateSource, TerrainLogic};

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
    t
}

fn world() -> GameLogic {
    let store = FixedStore(vec![Arc::new(dozer_template())]);
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 1000);
    g.set_template_source(Arc::new(store));
    g
}

fn spawn(g: &mut GameLogic, at: Coord3D) -> ObjectId {
    let t = g.find_template("Dozer").expect("template");
    g.create_object(&t, at, 0.0, 0)
}

/// A placed unit's snapped cell is reserved as its goal, so an ally's movement probe at that cell
/// sees a `UNIT_PRESENT_FIXED` unit (`posUnit == goalUnit`, `Pathfinder::updateGoal`,
/// `AIPathfind.cpp:9805-9880`, called from `GameLogic.cpp:523`). Without the goal the placed unit
/// would be `UNIT_PRESENT_MOVING`, the transient state the A* ignores.
#[test]
fn a_placed_unit_reserves_its_adjusted_cell_as_its_goal() {
    let mut g = world();
    // Not on a cell centre: `adjustDestination` snaps it (`checkForAdjust` sets `*dest` for retail
    // pathfinding, `AIPathfind.cpp:5355`), so the unit moves and the moved-to cell is its goal.
    let placed = spawn(&mut g, Coord3D::new(60.0, 300.0, 0.0));
    let before = g.find_object(placed).unwrap().position;
    g.adjust_placed_unit(placed);
    let after = g.find_object(placed).unwrap().position;
    assert!(after.dist_2d(&before) > 0.0, "the spot is snapped, not kept: {before:?} -> {after:?}");

    // An ally probing that cell sees the placed unit on its own goal (fixed); a cell further away
    // has no goal mark.
    let ally = spawn(&mut g, Coord3D::new(120.0, 300.0, 0.0));
    let po = PathObject::for_template(&dozer_template(), ally, 0).expect("a ground unit");
    let info = g.pathfinder().check_for_movement_full(&po, &after, false);
    assert_eq!(info.ally_fixed_count, 1, "the placed unit's adjusted cell is its goal and it stands there (`updateGoal`, GameLogic.cpp:523)");
    assert!(!info.ally_goal, "a unit on its own goal cell is UNIT_PRESENT_FIXED, not UNIT_GOAL");
    let other = Coord3D::new(after.x + 30.0, after.y + 30.0, 0.0);
    assert_eq!(g.pathfinder().check_for_movement_full(&po, &other, false).ally_fixed_count, 0, "a cell 3 cells away is not the goal");
}
