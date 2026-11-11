//! Possession duel with real Zero Hour data: the player possesses an AmericaTankCrusader, drives
//! it toward a ChinaTankBattleMaster with scripted input (world-space move vector, aim point,
//! FIRE bit) and kills it. Runs twice and checks the frame and CRC agree.
//!
//! `cargo run --release -p zr-game --example possess_duel` (needs a Zero Hour install, or `ZR_ZEROHOUR_DIR`).

use std::sync::Arc;
use zr_assets::{Installs, Vfs};
use zr_game::GameData;
use zr_sim::command::buttons;
use zr_sim::{Command, Coord3D, GameLogic, PossessInput, TemplateSource, TerrainLogic};

/// Returns (frames, crc, final distance, whether the Battlemaster died, whether the Crusader lived).
fn duel(data: &GameData, verbose: bool) -> (u32, u32, f32, bool, bool) {
    let ta = data.store.find_template("AmericaTankCrusader").expect("crusader");
    let tb = data.store.find_template("ChinaTankBattleMaster").expect("battlemaster");
    let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
    let ia = g.create_object(&ta, Coord3D::new(600.0, 1000.0, 0.0), 0.0, 0);
    let ib = g.create_object(&tb, Coord3D::new(900.0, 1000.0, 0.0), std::f32::consts::PI, 1);
    let range = ta.combat.weapon_set.as_ref().and_then(|s| s.weapons[0].as_ref()).map_or(0.0, |w| w.attack_range());
    let engage = range * 0.8;
    if verbose {
        println!("Crusader weapon range {range:.1}, engaging at {engage:.1}");
    }
    g.submit(0, Command::Possess { id: ia });
    let q = |v: f32| (v.clamp(-1.0, 1.0) * 32767.0) as i16;
    for _ in 0..30 * 120 {
        // The "client": read sim state, send this frame's input.
        let (Some(me), Some(foe)) = (g.find_object(ia), g.find_object(ib)) else { break };
        let (dx, dy) = (foe.position.x - me.position.x, foe.position.y - me.position.y);
        let dist = (dx * dx + dy * dy).sqrt();
        let (mx, my) = if dist > engage { (dx / dist, dy / dist) } else { (0.0, 0.0) };
        g.submit(
            0,
            Command::PossessInput(PossessInput { move_x: q(mx), move_y: q(my), aim: foe.position, buttons: buttons::FIRE }),
        );
        g.update();
        if verbose && g.frame() % 60 == 0 {
            let h = |id| g.find_object(id).map_or(0.0, |o| o.health());
            println!("  frame {:>4}: dist {:>6.1}  crusader {:>6.1}  battlemaster {:>6.1}", g.frame(), dist, h(ia), h(ib));
        }
    }
    let dist = match (g.find_object(ia), g.find_object(ib)) {
        (Some(a), Some(b)) => a.position.dist_2d(&b.position),
        _ => 0.0,
    };
    (g.frame(), g.crc(), dist, g.find_object(ib).is_none(), g.find_object(ia).is_some())
}

fn main() {
    let installs = Installs::detect();
    let Some(zh) = &installs.zero_hour else {
        eprintln!("no Zero Hour install found; set ZR_ZEROHOUR_DIR");
        return;
    };
    let vfs = Vfs::zero_hour(zh).expect("mounting Zero Hour");
    let data = Arc::new(GameData::load(&vfs));
    let (frames, crc, dist, killed, alive) = duel(&data, true);
    println!("battlemaster destroyed: {killed}, crusader alive: {alive}, after {frames} frames ({:.1} s), last distance {dist:.1}, crc {crc:08x}", frames as f32 / 30.0);
    let again = duel(&data, false);
    assert_eq!((frames, crc), (again.0, again.1), "possession duel must be deterministic");
    println!("deterministic: OK");
}
