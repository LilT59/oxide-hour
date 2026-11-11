//! Lane 179 (R21-2): `attackBlockedByObstacleCallback` (`AIPathfind.cpp:9385-9430`) exempts the attacker's
//! container (`:9409`), not the victim's. The victim's container is an obstacle the engine tests: its only
//! exemption is the victim-cell rule (`:9423`), which applies when the victim's own cell holds that obstacle.
//! The `@todo` at `:9412` says the victim's container is not exempted yet.

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

fn structure(name: &str, radius: f32) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE | kind_of::SELECTABLE;
    t.geometry.major_radius = radius;
    t.geometry.minor_radius = radius;
    t.max_health = 1000.0;
    t.initial_health = 1000.0;
    t
}

/// A garrison building `C` (a large footprint) holds the victim `V` on its centre, and a small structure `X`
/// placed first owns the victim's own cell, so `C`'s cells on the line are not the victim's cell's obstacle.
/// The engine's line from the attacker crosses `C`'s cells and blocks; the victim's container is not exempt.
/// Revert check: restoring `Some(obstacle) == victim_container` in `los.rs` exempts `C` and the view is clear.
#[test]
fn the_victim_container_is_not_exempt_from_the_obstacle_march() {
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    let attacker = g.create_object(&Arc::new(infantry("Ranger")), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let victim_pos = Coord3D::new(200.0, 100.0, 0.0);
    let x = g.create_object(&Arc::new(structure("Pylon", 10.0)), victim_pos, 0.0, 1);
    let c = g.create_object(&Arc::new(structure("Bunker", 40.0)), victim_pos, 0.0, 1);
    let victim = g.create_object(&Arc::new(infantry("Rebel")), victim_pos, 0.0, 1);
    g.objects.get_mut(&victim).unwrap().contained_by = Some(c);

    // The fixture: the victim's cell belongs to `X`, and the cell between the two units to `C`.
    assert_eq!(g.pathfinder.los_cell_obstacle_at(&victim_pos), Some(x.0), "X owns the victim's cell");
    assert_eq!(g.pathfinder.los_cell_obstacle_at(&Coord3D::new(160.0, 100.0, 0.0)), Some(c.0), "C owns a cell on the line");

    assert!(g.is_view_blocked_by_obstacle(attacker, victim), "the engine blocks on C's cells: the victim's container is not exempt");
}
