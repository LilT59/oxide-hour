use std::sync::Arc;
use zr_sim::body::{DamageInfo, DamageType};
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::{Command, Coord3D, GameLogic, ObjectId, ObjectTemplate, TerrainLogic};

fn tank() -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new("TestTank");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 8.0;
    t.max_health = 300.0;
    t.initial_health = 300.0;
    // A ground locomotor sticks to the terrain (the INI's `StickToGround`; `from_ini` leaves it off).
    let mut l = LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0);
    l.stick_to_ground = true;
    t.locomotor = Some(l);
    Arc::new(t)
}

fn hilly() -> TerrainLogic {
    // Deterministic synthetic bumps; no game data.
    let mut hm = zr_map::HeightMap { version: 4, width: 70, height: 70, border_size: 0, boundaries: vec![], data: vec![0; 70 * 70] };
    for y in 0..70usize {
        for x in 0..70usize {
            hm.data[y * 70 + x] = ((x * 7 + y * 13) % 40) as u8;
        }
    }
    TerrainLogic::from_height_map(&hm)
}

fn scenario(seed: u32, frames: u32, mut on_frame: impl FnMut(&GameLogic)) {
    let mut g = GameLogic::new(hilly(), seed);
    let t = tank();
    let mut ids = Vec::new();
    for i in 0..20 {
        let x = 50.0 + 20.0 * i as f32;
        let y = g.rng_mut().value_real(50.0, 300.0);
        ids.push(g.create_object(&t, Coord3D::new(x, y, 0.0), 0.0, (i % 2) as u8));
    }
    for f in 0..frames {
        if f % 40 == 0 {
            let x = g.rng_mut().value_real(10.0, 650.0);
            let y = g.rng_mut().value_real(10.0, 650.0);
            let who: Vec<ObjectId> = ids.iter().copied().filter(|i| i.0 % 2 == (f / 40) % 2).collect();
            g.submit(((f / 40) % 2) as u8, Command::MoveTo { ids: who, pos: Coord3D::new(x, y, 0.0) });
        }
        if f == 100 {
            g.submit(0, Command::Stop { ids: ids.iter().copied().take(4).collect() });
            g.damage_object(ids[5], DamageInfo { source: ObjectId::INVALID, amount: 999.0, kind: DamageType::Explosion });
        }
        g.update();
        on_frame(&g);
    }
}

#[test]
fn two_instances_have_identical_crc_every_frame() {
    let mut a = Vec::new();
    let mut b = Vec::new();
    scenario(42, 600, |g| a.push(g.crc()));
    scenario(42, 600, |g| b.push(g.crc()));
    assert_eq!(a, b);
    assert!(a.windows(2).any(|w| w[0] != w[1]), "state must actually evolve");
}

#[test]
fn different_seed_diverges() {
    let (mut a, mut b) = (0, 0);
    scenario(1, 100, |g| a = g.crc());
    scenario(2, 100, |g| b = g.crc());
    assert_ne!(a, b);
}

#[test]
fn ids_are_monotonic_and_never_reused() {
    let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 0);
    let t = tank();
    let a = g.create_object(&t, Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);
    let b = g.create_object(&t, Coord3D::new(20.0, 10.0, 0.0), 0.0, 0);
    assert!(b > a && a.0 == 1);
    g.destroy_object(a);
    // Deferred: hidden to lookups but still counted by the scheduler until end of frame.
    assert!(g.find_object(a).is_none());
    // Each tank is in the heap with its four constructor helpers and its AI module (lane 99: `object_helpers.rs`).
    assert_eq!(g.update_count(), 2 * 5);
    g.update();
    assert_eq!(g.update_count(), 5);
    let c = g.create_object(&t, Coord3D::new(30.0, 10.0, 0.0), 0.0, 0);
    assert_eq!(c.0, 3);
}

#[test]
fn move_to_arrives_stays_on_terrain_and_sleeps() {
    let mut g = GameLogic::new(hilly(), 0);
    let t = tank();
    let id = g.create_object(&t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 3);
    let goal = Coord3D::new(400.0, 300.0, 0.0);
    g.submit(3, Command::MoveTo { ids: vec![id], pos: goal });
    // Another player's order is ignored.
    let other = g.create_object(&t, Coord3D::new(500.0, 500.0, 0.0), 0.0, 4);
    g.submit(3, Command::MoveTo { ids: vec![other], pos: goal });
    let mut max_speed = 0.0f32;
    let mut last = g.find_object(id).unwrap().position;
    for _ in 0..30 * 40 {
        g.update();
        let o = g.find_object(id).unwrap();
        let step = o.position.dist_2d(&last);
        max_speed = max_speed.max(step);
        last = o.position;
        let h = g.terrain().ground_height(o.position.x, o.position.y);
        assert!((o.position.z - h).abs() < 1e-3);
    }
    let o = g.find_object(id).unwrap();
    // Ground paths end on a pathfind cell (the original only snaps to the exact goal for "ultra accurate" locomotors).
    assert!(o.position.dist_2d(&goal) < 8.0, "{:?}", o.position);
    assert!(max_speed <= 1.0 + 1e-4, "30 u/s = 1 u/frame, saw {max_speed}");
    assert_eq!(g.find_object(other).unwrap().position.x, 500.0);
    // After arrival every module sleeps forever: nothing is due.
    let before = g.stats().module_updates;
    g.run_frames(100);
    assert_eq!(g.stats().module_updates, before);
}

#[test]
fn damage_destroys_at_end_of_frame() {
    let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 0);
    let id = g.create_object(&tank(), Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);
    g.damage_object(id, DamageInfo { source: ObjectId::INVALID, amount: 100.0, kind: DamageType::SmallArms });
    assert_eq!(g.find_object(id).unwrap().health(), 200.0);
    g.damage_object(id, DamageInfo { source: ObjectId::INVALID, amount: 500.0, kind: DamageType::SmallArms });
    assert!(g.find_object(id).is_none());
    g.update();
    assert_eq!(g.object_count(), 0);
    assert_eq!(g.update_count(), 0);
    assert!(g.partition().is_empty());
}

#[cfg(feature = "possession")]
#[test]
fn possession_messages_are_accepted_and_inert() {
    let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 0);
    let id = g.create_object(&tank(), Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);
    let before = g.find_object(id).unwrap().position;
    g.submit(0, Command::Possess { id });
    g.submit(0, Command::PossessInput(zr_sim::PossessInput { move_x: 32767, move_y: 0, aim: Coord3D::ZERO, buttons: 1 }));
    g.submit(0, Command::Release);
    g.run_frames(5);
    assert_eq!(g.find_object(id).unwrap().position, before);
}

#[test]
fn structures_block_paths_and_release_cells_when_destroyed() {
    use zr_sim::pathfind::CellType;
    use zr_sim::template::{Geometry, GeometryShape};
    let mut hall = ObjectTemplate::new("TestHall");
    hall.kind_of = kind_of::STRUCTURE;
    hall.geometry = Geometry { shape: GeometryShape::Box, major_radius: 40.0, minor_radius: 40.0, height: 20.0, is_small: false };
    let hall = Arc::new(hall);
    let run = || {
        let mut g = GameLogic::new(TerrainLogic::flat(60, 60, 0), 3);
        let b = g.create_object(&hall, Coord3D::new(300.0, 200.0, 0.0), 0.0, 0);
        assert_eq!(g.pathfinder().cell_type_at(&Coord3D::new(300.0, 200.0, 0.0)), Some(CellType::Obstacle));
        let id = g.create_object(&tank(), Coord3D::new(105.0, 205.0, 0.0), 0.0, 1);
        g.submit(1, Command::MoveTo { ids: vec![id], pos: Coord3D::new(505.0, 205.0, 0.0) });
        let mut crcs = Vec::new();
        for _ in 0..30 * 40 {
            g.update();
            let p = g.find_object(id).unwrap().position;
            assert!(!(p.x > 262.0 && p.x < 338.0 && p.y > 162.0 && p.y < 238.0), "drove through the hall: {p:?}");
            crcs.push(g.crc());
        }
        // The unit stops at the group-adjusted destination, not the click (C++ `computeIndividualDestination`
        // -> `adjustDestination(groupDest)`, AIGroup.cpp:546): the 1-radius tank's candidate cell (51,21) is tightened
        // along the line to the click's cell (50,20) by `tightenPath` (AIPathfind.cpp:8387, callback :8359) over
        // `iterateCellsAlongLine` (the cells (51,20), (50,20), (50,19); last accepted (50,19)), giving (500.5,190.5);
        // `checkPathCost` (AIPathfind.cpp:8409) is 10 there and `1.4*(dx+dy)` is 25.2, so it is accepted. The
        // `onEnter` re-adjust (AIStates.cpp:1663) keeps it (`checkDestination` ignores the unit's own goal,
        // AIPathfind.cpp:5058). The port reaches the same goal; the old `< 10` from the click was port-only.
        assert!(g.find_object(id).unwrap().position.dist_2d(&Coord3D::new(500.5, 190.5, 0.0)) < 2.0);
        g.destroy_object(b);
        g.update();
        assert_eq!(g.pathfinder().cell_type_at(&Coord3D::new(300.0, 200.0, 0.0)), Some(CellType::Clear));
        crcs
    };
    assert_eq!(run(), run());
}
