//! Headless run on Alpine Assault: spawn a placeholder unit at each player start waypoint,
//! order player 1's unit to the other start, simulate 30 s and print positions each second.
//!
//! `cargo run --release -p zr-sim --example alpine_headless` (needs a Zero Hour install, or
//! `ZR_ZEROHOUR_DIR`).

use std::sync::Arc;
use std::time::Instant;
use zr_assets::{Installs, Vfs};
use zr_map::{DictValue, MapFile};
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::{Command, Coord3D, GameLogic, ObjectTemplate, TerrainLogic, LOGICFRAMES_PER_SECOND};

fn main() {
    let installs = Installs::detect();
    let Some(zh) = &installs.zero_hour else {
        eprintln!("no Zero Hour install found; set ZR_ZEROHOUR_DIR");
        return;
    };
    let vfs = Vfs::zero_hour(zh).expect("mounting Zero Hour");
    let path = vfs
        .files()
        .map(|(p, _)| p.to_owned())
        .filter(|p| p.ends_with(".map") && p.contains("alpine"))
        .min()
        .expect("Alpine Assault map not found");
    let raw = vfs.read_archived(&path).expect("map bytes");
    let data = zr_assets::compression::decompress_if_needed(raw).expect("decompress");
    let map = MapFile::parse(&data).expect("parse map");
    let hm = map.height_map.as_ref().expect("height map");
    println!("map {path}: {}x{} cells, border {}, {} objects", hm.width, hm.height, hm.border_size, map.objects.len());

    let waypoint = |name: &str| -> Coord3D {
        let o = map
            .objects
            .iter()
            .find(|o| matches!(o.props.get("waypointName"), Some(DictValue::Ascii(s)) if s.eq_ignore_ascii_case(name)))
            .unwrap_or_else(|| panic!("waypoint {name} missing"));
        Coord3D::new(o.pos[0], o.pos[1], o.pos[2])
    };
    let (p1, p2) = (waypoint("Player_1_Start"), waypoint("Player_2_Start"));
    println!("Player_1_Start {:?}\nPlayer_2_Start {:?}", p1, p2);

    let mut t = ObjectTemplate::new("PlaceholderTank");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 8.0;
    // Roughly a Crusader: 30 u/s, 180 deg/s turn.
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 100.0, 200.0, 180.0));
    let t = Arc::new(t);

    let mut g = GameLogic::new(TerrainLogic::from_height_map(hm), 1);
    let a = g.create_object(&t, p1, 0.0, 1);
    let b = g.create_object(&t, p2, 0.0, 2);
    g.submit(1, Command::MoveTo { ids: vec![a], pos: p2 });
    let tl = g.terrain();
    println!("ground height: p1 {:.2}, mid {:.2}, p2 {:.2}, corner {:.2}", tl.ground_height(p1.x, p1.y), tl.ground_height((p1.x + p2.x) / 2.0, (p1.y + p2.y) / 2.0), tl.ground_height(p2.x, p2.y), tl.ground_height(0.0, 0.0));
    println!("start CRC {:08x}", g.crc());

    let start = Instant::now();
    let mut worst = std::time::Duration::ZERO;
    for sec in 0..=30 {
        let pa = g.find_object(a).unwrap();
        println!("t={sec:>2}s frame {:>4}  unit1 ({:8.2}, {:8.2}, {:6.2}) speed {:.3}/f   unit2 ({:8.2}, {:8.2})",
            g.frame(), pa.position.x, pa.position.y, pa.position.z, pa.ai.speed, g.find_object(b).unwrap().position.x, g.find_object(b).unwrap().position.y);
        if sec == 30 {
            break;
        }
        for _ in 0..LOGICFRAMES_PER_SECOND {
            let t0 = Instant::now();
            g.update();
            worst = worst.max(t0.elapsed());
        }
    }
    let total = start.elapsed();
    println!("{} frames in {:?} ({:?}/frame avg, {:?} worst), {} module updates", g.frame(), total, total / g.frame(), worst, g.stats().module_updates);
    println!("distance remaining {:.2}", g.find_object(a).unwrap().position.dist_2d(&p2));
    println!("final CRC {:08x}", g.crc());
}
