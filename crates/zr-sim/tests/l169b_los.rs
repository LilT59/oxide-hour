//! Lane 169b (wave 20): `PartitionFilterLineOfSight` in target acquisition (`PartitionManager.cpp:5426-5437`),
//! the terrain march (`BaseHeightMap.cpp:1020-1150`) and the obstacle-cell march (`AIPathfind.cpp:9377-9480`).
//! AH7 s1 f2816: Ranger 241 acquired rebel 240 *through* the Tech Oil Derrick it was capturing, because the
//! port had no line-of-sight test at all. Synthetic data only.

use std::sync::Arc;
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::weapon::{CombatTemplate, WeaponSetTemplate, WeaponTemplate};
use zr_sim::{Command, Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

/// A rifleman: `KINDOF_ATTACK_NEEDS_LINE_OF_SIGHT` and a rifle that reaches past the test's gap.
fn rifleman(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::INFANTRY | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
    t.geometry.major_radius = 4.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    // `KINDOF_ATTACK_NEEDS_LINE_OF_SIGHT`: `getNextMoodTarget` adds `AI::CAN_SEE` for it
    // (`AIUpdate.cpp:4648-4655`).
    t.attack_needs_line_of_sight = true;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Legs, 30.0, 60.0, 120.0, 120.0));
    let mut w = WeaponTemplate::new("Rifle");
    w.primary_damage = 10.0;
    w.attack_range = 300.0;
    w.min_delay_between_shots = 1000;
    w.max_delay_between_shots = 1000;
    let mut s = WeaponSetTemplate::default();
    s.weapons[0] = Some(Arc::new(w));
    t.combat = CombatTemplate { weapon_set: Some(s), ..CombatTemplate::default() };
    t
}

/// An immobile structure big enough to own the pathfind cells between two units.
fn wall(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE | kind_of::SELECTABLE;
    t.geometry.major_radius = 20.0;
    t.max_health = 1000.0;
    t.initial_health = 1000.0;
    t
}

fn world(terrain: TerrainLogic) -> GameLogic {
    let mut g = GameLogic::new(terrain, 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    // The scan's computer-controlled path: no `WITHIN_ATTACK_RANGE` / `UNFOGGED` flags (`AIUpdate.cpp:4659-4672`).
    g.add_ai_player(0, zr_sim::SkirmishAiConfig::default());
    g
}

/// An attack-move past the enemy: does the scan acquire it within `frames` frames? The first scan is due
/// one frame after the state's enter (`resetNextMoodCheckTime`, `AIUpdate.cpp:4501`), and ten frames of
/// movement do not carry the unit around the wall.
fn attack_move_acquires(terrain: TerrainLogic, with_wall: bool) -> bool {
    let mut g = world(terrain);
    let a = g.create_object(&Arc::new(rifleman("Ranger")), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    g.create_object(&Arc::new(rifleman("Enemy")), Coord3D::new(180.0, 100.0, 0.0), 0.0, 1);
    if with_wall {
        g.create_object(&Arc::new(wall("Wall")), Coord3D::new(140.0, 100.0, 0.0), 0.0, 0);
    }
    g.submit(0, Command::AttackMove { ids: vec![a], pos: Coord3D::new(300.0, 100.0, 0.0) });
    let mut acquired = false;
    for _ in 0..10 {
        g.run_frames(1);
        acquired |= g.find_object(a).unwrap().combat.attack.is_some();
    }
    acquired
}

/// `Pathfinder::isAttackViewBlockedByObstacle`'s obstacle march (`AIPathfind.cpp:9410-9440`): a structure
/// between the two units owns the pathfind cells on the line, so the scan finds nothing. Open ground on the
/// same layout acquires the enemy — the fix's control.
#[test]
fn a_structure_between_two_units_blocks_the_attack_move_scan() {
    assert!(!attack_move_acquires(TerrainLogic::flat(400, 400, 0), true), "the wall blocks the line of sight");
    assert!(attack_move_acquires(TerrainLogic::flat(400, 400, 0), false), "open ground: the enemy is acquired");
}

/// `BaseHeightMapRenderObjClass::isClearLineOfSight` (`BaseHeightMap.cpp:1020-1150`): a ridge between the
/// two units rises above the line, so the terrain half of the filter rejects the target too.
#[test]
fn a_terrain_ridge_between_two_units_blocks_the_attack_move_scan() {
    // 32 x 32 samples = a 320 x 320 world. A 60-sample (37.5-unit) ridge at world x 140..160, far above the
    // 4-unit eye height.
    let mut data = vec![0u8; 32 * 32];
    for y in 0..32 {
        for x in 14..16 {
            data[y * 32 + x] = 60;
        }
    }
    let ridge = TerrainLogic::from_samples(32, 32, 0, data);
    assert!(!attack_move_acquires(ridge, false), "the ridge blocks the line of sight");
    assert!(attack_move_acquires(TerrainLogic::flat(32, 32, 0), false), "flat ground: the enemy is acquired");
}

/// `AIData`'s `AttackUsesLineOfSight` (`AI.cpp:941`, default true) switches the whole filter off: with it
/// false the scan acquires through the wall.
#[test]
fn turning_off_attack_uses_line_of_sight_acquires_through_the_wall() {
    let mut g = world(TerrainLogic::flat(400, 400, 0));
    let t = zr_sim::ai::AiTunables { attack_uses_line_of_sight: false, ..Default::default() };
    g.set_ai_tunables(t);
    let a = g.create_object(&Arc::new(rifleman("Ranger")), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    g.create_object(&Arc::new(rifleman("Enemy")), Coord3D::new(180.0, 100.0, 0.0), 0.0, 1);
    g.create_object(&Arc::new(wall("Wall")), Coord3D::new(140.0, 100.0, 0.0), 0.0, 0);
    g.submit(0, Command::AttackMove { ids: vec![a], pos: Coord3D::new(300.0, 100.0, 0.0) });
    let mut acquired = false;
    for _ in 0..10 {
        g.run_frames(1);
        acquired |= g.find_object(a).unwrap().combat.attack.is_some();
    }
    assert!(acquired, "without the flag the wall is not considered");
}
