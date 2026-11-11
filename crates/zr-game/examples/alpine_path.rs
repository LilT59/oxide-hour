//! Paths the America dozer across Alpine Assault to China's base with the ported pathfinder and
//! drives it there. Skips when no Zero Hour install is found.
//!
//! `cargo run --release -p zr-game --example alpine_path` (needs a Zero Hour install, or `ZR_ZEROHOUR_DIR`).

use std::sync::Arc;
use std::time::Instant;
use zr_assets::{Installs, Vfs};
use zr_game::{Game, GameData, PlayerSetup};
use zr_sim::{Command, Coord3D, LOGICFRAMES_PER_SECOND};

fn main() {
    let installs = Installs::detect();
    let Some(zh) = &installs.zero_hour else {
        eprintln!("no Zero Hour install found; set ZR_ZEROHOUR_DIR");
        return;
    };
    let vfs = Vfs::zero_hour(zh).expect("mounting Zero Hour");
    let data = Arc::new(GameData::load(&vfs));
    let setups = [PlayerSetup::new("FactionAmerica", 0), PlayerSetup::new("FactionChina", 1)];
    let mut g = Game::new_skirmish_with(data, &vfs, "Alpine Assault", &setups, 1).expect("start skirmish");
    let p1 = g.skirmish_player(0).expect("player0").index;
    let dozer = g.spawned().iter().find(|s| s.player == p1 && s.template.contains("Dozer")).expect("dozer").id;
    let start = g.object(dozer).unwrap().position;
    let goal = Coord3D::new(1126.0, 361.0, 0.0); // next to China's dozer

    g.submit(p1, Command::MoveTo { ids: vec![dozer], pos: goal });
    let t = Instant::now();
    g.run_frames(1);
    println!("path computed + first frame in {:?}", t.elapsed());
    let path = g.logic().find_object(dozer).unwrap().ai.path.clone();
    println!("start ({:.0}, {:.0}) -> goal ({:.0}, {:.0}), straight line {:.0}", start.x, start.y, goal.x, goal.y, start.dist_2d(&goal));
    let mut len = 0.0;
    let mut prev = start;
    for (i, p) in path.iter().enumerate() {
        len += prev.dist_2d(p);
        println!("  wp {i:>2}: ({:8.1}, {:8.1})", p.x, p.y);
        prev = *p;
    }
    println!("{} waypoints, path length {len:.0}", path.len());
    assert!(!path.is_empty(), "no path found");

    let mut frames = 0;
    while g.logic().find_object(dozer).is_some_and(|o| o.ai.is_moving()) && frames < 300 * LOGICFRAMES_PER_SECOND {
        g.run_frames(LOGICFRAMES_PER_SECOND);
        frames += LOGICFRAMES_PER_SECOND;
    }
    let end = g.object(dozer).unwrap().position;
    println!("after {} s: dozer at ({:.1}, {:.1}), {:.1} from the goal, CRC {:08x}", frames / LOGICFRAMES_PER_SECOND, end.x, end.y, end.dist_2d(&goal), g.crc());
}
