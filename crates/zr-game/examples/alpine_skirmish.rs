//! Alpine Assault skirmish: player 1 FactionAmerica at start 1, player 2 FactionChina at start 2,
//! seed 1. Prints the start-up, orders player 1's dozer to the map centre, runs 20 s twice
//! in-process and asserts both runs end on the same CRC.
//!
//! `cargo run --release -p zr-game --example alpine_skirmish` (needs a Zero Hour install, or `ZR_ZEROHOUR_DIR`).

use std::sync::Arc;
use zr_assets::{Installs, Vfs};
use zr_game::{Game, GameData, PlayerSetup};
use zr_sim::{Command, Coord3D, LOGICFRAMES_PER_SECOND};

fn start(data: &Arc<GameData>, vfs: &Vfs) -> Game {
    let setups = [PlayerSetup::new("FactionAmerica", 0), PlayerSetup::new("FactionChina", 1)];
    Game::new_skirmish_with(data.clone(), vfs, "Alpine Assault", &setups, 1).expect("start skirmish")
}

fn run(data: &Arc<GameData>, vfs: &Vfs, verbose: bool) -> u32 {
    let mut g = start(data, vfs);
    if verbose {
        let s = g.map_load_stats();
        println!("map {}: {} map objects -> {} created ({} waypoints, {} road/bridge points, {} without template, {} props/trees skipped)",
            g.map_name(), s.map_objects, s.created, s.waypoints, s.road_or_bridge_points, s.no_template, s.props_or_trees_skipped);
        println!("players:");
        for p in g.players() {
            println!("  #{} {:<16} {:?} faction={:<16} side={:<10} money={} start={:?}", p.index, if p.name.is_empty() { "(neutral)" } else { &p.name }, p.kind, p.faction, p.side, p.money, p.start_position);
        }
        println!("starting sets:");
        for s in g.spawned() {
            println!("  player #{} {:<14} {:<22} id {:>5} at ({:8.2}, {:8.2}, {:6.2})", s.player, s.role, s.template, s.id.0, s.pos.x, s.pos.y, s.pos.z);
        }
        println!("objects by owner: {:?}", g.object_counts_by_owner());
        println!("start CRC {:08x}", g.crc());
    }

    let p1 = g.skirmish_player(0).expect("player0").index;
    let dozer = g.spawned().iter().find(|s| s.player == p1 && s.template.contains("Dozer")).expect("player 1 dozer").id;
    let (w, h) = g.logic().terrain().world_size();
    let centre = Coord3D::new(w / 2.0, h / 2.0, 0.0);
    if verbose {
        println!("ordering dozer {} to the map centre ({:.0}, {:.0})", dozer.0, centre.x, centre.y);
    }
    g.submit(p1, Command::MoveTo { ids: vec![dozer], pos: centre });
    for sec in 0..=20 {
        if verbose {
            let o = g.object(dozer).expect("dozer alive");
            println!("  t={sec:>2}s frame {:>3}  dozer ({:8.2}, {:8.2}, {:6.2}) angle {:6.3} hp {:.0}", g.frame(), o.position.x, o.position.y, o.position.z, o.angle, o.health);
        }
        if sec < 20 {
            g.run_frames(LOGICFRAMES_PER_SECOND);
        }
    }
    if verbose {
        let o = g.object(dozer).unwrap();
        println!("distance to centre {:.2}", o.position.dist_2d(&centre));
    }
    g.crc()
}

fn main() {
    let installs = Installs::detect();
    let Some(zh) = &installs.zero_hour else {
        eprintln!("no Zero Hour install found; set ZR_ZEROHOUR_DIR");
        return;
    };
    let vfs = Vfs::zero_hour(zh).expect("mounting Zero Hour");
    let data = Arc::new(GameData::load(&vfs));
    println!("{} template warnings, default skirmish cash {}", data.warnings.len(), data.default_starting_cash);
    let a = run(&data, &vfs, true);
    let b = run(&data, &vfs, false);
    println!("final CRC run 1 {a:08x}, run 2 {b:08x}");
    assert_eq!(a, b, "two runs from the same seed diverged");
    println!("deterministic: OK");
}
