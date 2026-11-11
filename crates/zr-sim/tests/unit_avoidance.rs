//! Dynamic unit avoidance (`Pathfinder::updatePos` / `adjustDestination`), synthetic data only.

use std::sync::Arc;
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::{Command, Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

fn flat() -> TerrainLogic {
    TerrainLogic::flat(100, 100, 0)
}

fn tank(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
    t.geometry.major_radius = 8.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    t
}

/// A group ordered to one point must fan out into distinct goal cells instead of stacking.
#[test]
fn group_move_to_one_point_spreads_out() {
    let mut g = GameLogic::new(flat(), 1);
    let t = Arc::new(tank("T"));
    // A 2x2 block, 20 units apart so the 2x2-cell footprints do not start overlapping.
    let ids: Vec<_> = [
        (300.0, 300.0),
        (320.0, 300.0),
        (300.0, 320.0),
        (320.0, 320.0),
    ]
    .into_iter()
    .map(|(x, y)| g.create_object(&t, Coord3D::new(x, y, 0.0), 0.0, 0))
    .collect();

    let goal = Coord3D::new(400.0, 400.0, 0.0);
    g.submit(0, Command::MoveTo { ids: ids.clone(), pos: goal });
    g.run_frames(600);

    let positions: Vec<Coord3D> = ids.iter().map(|&id| g.find_object(id).unwrap().position).collect();
    for p in &positions {
        assert!(p.dist_2d(&goal) < 60.0, "unit stopped at {p:?}, goal {goal:?}");
    }
    let mut min_gap = f32::MAX;
    for i in 0..positions.len() {
        for j in (i + 1)..positions.len() {
            min_gap = min_gap.min(positions[i].dist_2d(&positions[j]));
        }
    }
    assert!(min_gap > 15.0, "units must not stack, closest pair {min_gap}");
}
