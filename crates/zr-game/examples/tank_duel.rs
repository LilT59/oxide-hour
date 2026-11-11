//! Crusader vs Battlemaster on flat ground with real Zero Hour data: both attack each other from
//! 120 units apart; prints the weapons and the frames until the first one dies.
//!
//! `cargo run --release -p zr-game --example tank_duel` (needs a Zero Hour install, or `ZR_ZEROHOUR_DIR`).

use std::sync::Arc;
use zr_assets::{Installs, Vfs};
use zr_game::GameData;
use zr_sim::{Command, Coord3D, GameLogic, ObjectId, TemplateSource, TerrainLogic};

fn duel(data: &GameData, a: &str, b: &str, dist: f32, verbose: bool) -> (u32, Option<&'static str>, u32) {
    let ta = data.store.find_template(a).expect("template a");
    let tb = data.store.find_template(b).expect("template b");
    let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
    let ia = g.create_object(&ta, Coord3D::new(600.0, 1000.0, 0.0), 0.0, 0);
    let ib = g.create_object(&tb, Coord3D::new(600.0 + dist, 1000.0, 0.0), std::f32::consts::PI, 1);
    if verbose {
        for (name, t, id) in [(a, &ta, ia), (b, &tb, ib)] {
            println!("{name}: hp {} turret {:?} armor {:?}", t.max_health, t.combat.turret_turn_rate, t.combat.armor.as_ref().map(|a| &a.name));
            if let Some(ws) = &t.combat.weapon_set {
                for (slot, w) in ws.weapons.iter().enumerate() {
                    if let Some(w) = w {
                        println!(
                            "  slot {slot} {}: dmg {} {:?} range {} delay {}..{} clip {} reload {} projectile {} speed {:.2}/frame radius {}",
                            w.name, w.primary_damage, w.damage_type, w.attack_range, w.min_delay_between_shots, w.max_delay_between_shots,
                            w.clip_size, w.clip_reload_time, w.has_projectile, w.weapon_speed, w.primary_damage_radius
                        );
                    }
                }
            }
            let _ = id;
        }
    }
    g.submit(0, Command::Attack { ids: vec![ia], target: ib });
    g.submit(1, Command::Attack { ids: vec![ib], target: ia });
    let alive = |g: &GameLogic, id: ObjectId| g.find_object(id).is_some();
    for _ in 0..30 * 120 {
        g.update();
        if verbose && g.frame() % 30 == 0 {
            let h = |id| g.find_object(id).map_or(0.0, |o| o.health());
            println!("  frame {:>4}: {a} {:>6.1}  {b} {:>6.1}", g.frame(), h(ia), h(ib));
        }
        if !alive(&g, ia) || !alive(&g, ib) {
            let winner = match (alive(&g, ia), alive(&g, ib)) {
                (true, false) => Some("first"),
                (false, true) => Some("second"),
                _ => None,
            };
            return (g.frame(), winner, g.crc());
        }
    }
    (g.frame(), None, g.crc())
}

fn main() {
    let installs = Installs::detect();
    let Some(zh) = &installs.zero_hour else {
        eprintln!("no Zero Hour install found; set ZR_ZEROHOUR_DIR");
        return;
    };
    let vfs = Vfs::zero_hour(zh).expect("mounting Zero Hour");
    let data = Arc::new(GameData::load(&vfs));
    let (a, b) = ("AmericaTankCrusader", "ChinaTankBattleMaster");
    let (frames, winner, crc) = duel(&data, a, b, 120.0, true);
    println!("duel over after {frames} frames ({:.1} s), winner: {winner:?}, crc {crc:08x}", frames as f32 / 30.0);
    let (f2, w2, c2) = duel(&data, a, b, 120.0, false);
    assert_eq!((frames, winner, crc), (f2, w2, c2), "duel must be deterministic");
    println!("deterministic: OK");
}
