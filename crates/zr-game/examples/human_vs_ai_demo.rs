//! Passive human vs one skirmish AI, headless: slot 0 is a human (`PlayerSetup::new`, no commands
//! at all, just the retail start), slot 1 is an AI. Runs up to 40 game minutes, twice, and asserts
//! both runs agree on the CRC. Reports the frame the AI first gets a unit near the human base, the
//! first frame a human structure loses health, and the frame the human is defeated
//! (`VictoryConditions`) or that none of it happens.
//!
//! `cargo run --release -p zr-game --example human_vs_ai_demo [--map NAME] [--human FactionX] [--verbose] AIFaction [easy|normal|hard]`
//! (needs a Zero Hour install, or `ZR_ZEROHOUR_DIR`). Default map Alpine Assault, human FactionAmerica.

use std::sync::Arc;
use zr_assets::{Installs, Vfs};
use zr_game::{AiDifficulty, Game, GameData, PlayerSetup};
use zr_sim::template::kind_of;

#[derive(Debug, PartialEq, Eq, Default)]
struct Outcome {
    /// First frame an AI-owned mobile object is within 600 of a human structure.
    near: Option<u32>,
    /// First frame the human's total structure health fell below its starting value.
    damaged: Option<u32>,
    /// Frame `check_for_game_end` listed the human as defeated / named the AI winner.
    defeated: Option<u32>,
    /// `AISkirmishPlayer` base defenses started by the AI.
    base_defenses: u32,
    /// `SKIRMISH_FIRE_SPECIAL_POWER_AT_MOST_COST` (and other) special-power fires by the AI.
    powers_fired: u32,
    /// General's sciences the AI bought (`doUpgradesAndSkills`).
    sciences_bought: u32,
    crc: u32,
}

fn human_hp(g: &Game, p: u8) -> i64 {
    g.logic().objects().filter(|o| o.player == p && o.is_kind_of(kind_of::STRUCTURE)).map(|o| o.health() as i64).sum()
}

fn run(data: &Arc<GameData>, vfs: &Vfs, map: &str, human: &str, ai: &str, diff: AiDifficulty, verbose: bool) -> Outcome {
    let setups = [PlayerSetup::new(human, 0), PlayerSetup::ai(ai, 1).with_difficulty(diff)];
    let mut g = Game::new_skirmish_with(data.clone(), vfs, map, &setups, 1).expect("start skirmish");
    let h = g.players().iter().find(|p| !p.is_ai && p.kind == zr_game::PlayerKind::Skirmish).map(|p| p.index).unwrap();
    let a = g.players().iter().find(|p| p.is_ai).map(|p| p.index).unwrap();
    let mut out = Outcome::default();
    let hp0 = human_hp(&g, h);
    for minute in 1..=40u32 {
        for _ in 0..60 {
            g.run_frames(30);
            let l = g.logic();
            if out.near.is_none() {
                let hs: Vec<_> = l.objects().filter(|o| o.player == h && o.is_kind_of(kind_of::STRUCTURE)).map(|o| o.position).collect();
                let hit = l.objects().any(|o| {
                    o.player == a
                        && o.template.locomotor.is_some()
                        && hs.iter().any(|s| {
                            let (dx, dy) = (s.x - o.position.x, s.y - o.position.y);
                            dx * dx + dy * dy <= 600.0 * 600.0
                        })
                });
                if hit {
                    out.near = Some(g.frame());
                }
            }
            if out.damaged.is_none() && human_hp(&g, h) < hp0 {
                out.damaged = Some(g.frame());
            }
            let end = g.logic().check_for_game_end();
            if end.defeated.contains(&h) || end.winner.is_some() {
                out.defeated = Some(g.frame());
                break;
            }
        }
        if verbose {
            let l = g.logic();
            let count = |p: u8, s: bool| l.objects().filter(|o| o.player == p && o.is_kind_of(kind_of::STRUCTURE) == s).count();
            let atk = l.objects().filter(|o| o.player == a && (o.combat.attack.is_some() || o.combat.hunt || o.combat.attack_move)).count();
            println!(
                "t={minute:>2}min f{:>6} human: money {:>5} structs {} hp {} units {} | ai: money {:>5} structs {} units {} fighting {atk} | near {:?} dmg {:?}",
                g.frame(),
                g.player_money(h).unwrap_or(0),
                count(h, true),
                human_hp(&g, h),
                count(h, false),
                g.player_money(a).unwrap_or(0),
                count(a, true),
                count(a, false),
                out.near,
                out.damaged
            );
        }
        if out.defeated.is_some() {
            break;
        }
    }
    out.crc = g.crc();
    if let Some(ai) = g.logic().ai_player(a) {
        out.base_defenses = ai.base_defenses_built;
        out.powers_fired = ai.powers_fired;
        out.sciences_bought = ai.sciences_bought;
    }
    if verbose {
        let mut pows: Vec<_> = g.logic().power_fire_counts().iter().collect();
        pows.sort_by(|x, y| y.1.cmp(x.1));
        println!(
            "  ai base_defenses {} powers_fired {} sciences_bought {} rank {} skill {} sci_pts {}",
            out.base_defenses,
            out.powers_fired,
            out.sciences_bought,
            g.logic().player_rank_level(a),
            g.logic().player_skill_points(a),
            g.logic().player_science_purchase_points(a),
        );
        for (n, c) in pows.iter().take(15) {
            println!("    fired {c:>6}  {n}");
        }
    }
    out
}

fn main() {
    let installs = Installs::detect();
    let Some(zh) = &installs.zero_hour else {
        eprintln!("no Zero Hour install found; set ZR_ZEROHOUR_DIR");
        return;
    };
    let vfs = Vfs::zero_hour(zh).expect("mounting Zero Hour");
    let data = Arc::new(GameData::load(&vfs));
    let mut map = "Alpine Assault".to_owned();
    let mut human = "FactionAmerica".to_owned();
    let mut ai = "FactionChina".to_owned();
    let mut diff = AiDifficulty::Normal;
    let mut verbose = false;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut it = args.iter();
    while let Some(x) = it.next() {
        match x.as_str() {
            "--map" => map = it.next().cloned().unwrap_or(map),
            "--human" => human = it.next().cloned().unwrap_or(human),
            "--verbose" => verbose = true,
            "easy" => diff = AiDifficulty::Easy,
            "normal" => diff = AiDifficulty::Normal,
            "hard" => diff = AiDifficulty::Hard,
            f => ai = f.to_owned(),
        }
    }
    let r1 = run(&data, &vfs, &map, &human, &ai, diff, verbose);
    let r2 = run(&data, &vfs, &map, &human, &ai, diff, false);
    println!(
        "{map}: passive {human} vs {ai} [{diff:?}]: ai near human base {:?}; first human damage {:?}; human defeated {:?}; base_defenses {}; powers_fired {}; sciences_bought {}; CRC {:08x}/{:08x} {}",
        r1.near,
        r1.damaged,
        r1.defeated,
        r1.base_defenses,
        r1.powers_fired,
        r1.sciences_bought,
        r1.crc,
        r2.crc,
        if r1 == r2 { "deterministic" } else { "DIVERGED" }
    );
    assert_eq!(r1, r2, "two runs from the same seed diverged");
}
