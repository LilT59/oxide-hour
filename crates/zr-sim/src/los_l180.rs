//! Lane 180 (R21-3): the firing-line terrain march runs only for an attacker with a current weapon
//! (`isAttackViewBlockedByObstacle`, `AIPathfind.cpp:9484-9489`). The obstacle march is unchanged.

use super::*;
use crate::template::{kind_of, ObjectTemplate};
use std::sync::Arc;

fn infantry(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::INFANTRY | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
    t.geometry.major_radius = 4.0;
    t.geometry.minor_radius = 4.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.attack_needs_line_of_sight = true;
    t
}

/// A 400x400 map with a ridge of height samples 200 (125 world units) across the cells x 14..=16, that is
/// world x 140..170. A level line at eye height from x 100 to x 200 crosses it.
fn ridge_terrain() -> TerrainLogic {
    let (w, h) = (400, 400);
    let mut data = vec![0u8; (w * h) as usize];
    for y in 0..h {
        for x in 14..17 {
            data[(x + y * w) as usize] = 200;
        }
    }
    TerrainLogic::from_samples(w, h, 0, data)
}

/// A unit without a weapon does not run the terrain march: the ridge across its line does not block it.
/// The control: the same ridge blocks an attacker that has a weapon (the view built with `terrain_test`).
/// Revert check: dropping the `current_weapon` test from `GameLogic::attack_view` makes the infantry blocked.
#[test]
fn a_unit_without_a_weapon_skips_the_terrain_march() {
    let mut g = GameLogic::new(ridge_terrain(), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    let attacker = g.create_object(&Arc::new(infantry("Ranger")), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let victim = g.create_object(&Arc::new(infantry("Rebel")), Coord3D::new(200.0, 100.0, 0.0), 0.0, 1);

    assert!(!g.is_view_blocked_by_obstacle(attacker, victim), "no weapon: no terrain march, and no obstacle on the line");

    let view = g.attack_view(attacker, Some(victim)).expect("a LOS unit has a view");
    assert!(!view.terrain_test, "no current weapon: the terrain test is off");
    let armed = AttackView { terrain_test: true, ..view };
    let from = Coord3D::new(100.0, 100.0, 0.0);
    let to = Coord3D::new(200.0, 100.0, 0.0);
    assert!(g.pathfinder.attack_view_blocked(&g.terrain, &armed, &from, &to), "control: the ridge blocks the firing line of an armed unit");
}
