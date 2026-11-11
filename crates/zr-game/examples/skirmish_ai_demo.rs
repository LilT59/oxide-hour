//! Script-driven AI vs AI on Alpine Assault, headless. Runs until a winner (or 40 game minutes),
//! twice, and asserts both runs agree on the CRC. Prints a per-minute timeline and a wave summary.
//!
//! `cargo run --release -p zr-game --example skirmish_ai_demo [--map NAME] [--seed N] [faction ...] [easy|hard]`
//! (needs a Zero Hour install, or `ZR_ZEROHOUR_DIR`). Default `FactionChina FactionAmerica` Normal, seed 1.
//! `ZR_SNAPSHOT=<file>` writes the per-frame snapshot (`docs/HARNESS.md`) from the first (verbose) run only; the
//! second run has no snapshot, so the two CRCs also check that the writer does not change the simulation.

use std::sync::Arc;
use zr_assets::{Installs, Vfs};
use zr_game::snapshot::{self, FileSnapshot};
use zr_game::{AiDifficulty, Game, GameData, PlayerSetup};

/// Mobile objects owned by `p` (structures excluded).
fn mobile(g: &Game, p: u8) -> usize {
    g.logic().objects().filter(|o| o.player == p && o.template.locomotor.is_some()).count()
}

/// Total health and count of `p`'s live structures.
fn struct_hp(g: &Game, p: u8) -> (i32, usize) {
    let mut hp = 0;
    let mut n = 0;
    for o in g.logic().objects().filter(|o| o.player == p && o.is_kind_of(zr_sim::template::kind_of::STRUCTURE)) {
        hp += o.health() as i32;
        n += 1;
    }
    (hp, n)
}

/// Supply-center-type structures owned by `p` (any structure with a `Center` dock).
fn supply_count(g: &Game, p: u8) -> usize {
    g.logic()
        .objects()
        .filter(|o| o.player == p && o.template.dock.as_ref().is_some_and(|d| d.kind == zr_sim::DockKind::Center))
        .count()
}

/// Remaining warehouse supply: `(total boxes, live warehouses, per-player boxes within 900 of any
/// of the player's structures)`. A warehouse that empties and has `DeleteWhenEmpty` disappears, so
/// the count drops as maps run dry.
fn warehouse_supply(g: &Game, players: &[u8]) -> (i32, usize, Vec<i32>) {
    let l = g.logic();
    let whs: Vec<_> = l
        .objects()
        .filter(|o| o.template.dock.as_ref().is_some_and(|d| d.kind == zr_sim::DockKind::Warehouse))
        .collect();
    let total: i32 = whs.iter().map(|o| o.boxes).sum();
    let near = players
        .iter()
        .map(|&p| {
            let own: Vec<_> = l.objects().filter(|o| o.player == p && o.is_kind_of(zr_sim::template::kind_of::STRUCTURE)).collect();
            whs.iter()
                .filter(|w| {
                    own.iter().any(|s| {
                        let (dx, dy) = (s.position.x - w.position.x, s.position.y - w.position.y);
                        dx * dx + dy * dy <= 900.0 * 900.0
                    })
                })
                .map(|w| w.boxes)
                .sum()
        })
        .collect();
    (total, whs.len(), near)
}

/// Objects owned by `p` with an active attack order / attack-move / hunt state.
fn fighting(g: &Game, p: u8) -> (usize, usize, usize) {
    let (mut attacking, mut moving, mut hunting) = (0, 0, 0);
    for o in g.logic().objects().filter(|o| o.player == p) {
        if o.combat.attack.is_some() {
            attacking += 1;
        }
        if o.combat.attack_move {
            moving += 1;
        }
        if o.combat.hunt {
            hunting += 1;
        }
    }
    (attacking, moving, hunting)
}

fn run(data: &Arc<GameData>, vfs: &Vfs, map: &str, factions: &[String], difficulty: AiDifficulty, seed: u32, verbose: bool) -> (u32, Option<u8>, u32) {
    let setups: Vec<PlayerSetup> = factions
        .iter()
        .enumerate()
        .map(|(i, f)| PlayerSetup::ai(f, i as u8).with_difficulty(difficulty))
        .collect();
    let matchup = format!("{map}: {} (seed {seed})", factions.join(" vs "));
    let mut g = Game::new_skirmish_with(data.clone(), vfs, map, &setups, seed).expect("start skirmish");
    let mut snap = if verbose { FileSnapshot::from_env().expect("ZR_SNAPSHOT / ZR_SNAPSHOT_EVERY") } else { None };
    let ai_players: Vec<_> = g.players().iter().filter(|p| p.is_ai).map(|p| p.index).collect();
    if verbose {
        for p in g.players() {
            println!("  player #{} {} side={} money={} start={:?}", p.index, p.faction, p.side, p.money, p.start_position);
        }
        let s = g.logic().scripts();
        let recruitable = s.team_protos.values().filter(|p| p.is_ai_recruitable).count();
        println!(
            "{matchup}: {} lists, script_driven={}, {} team prototypes ({recruitable} recruitable)",
            s.lists.len(),
            s.script_driven,
            s.team_protos.len()
        );
        for p in g.players() {
            let Some(ai) = g.logic().ai_player(p.index) else { continue };
            println!(
                "  AI #{} base_defense={:?} base_center=({:.0},{:.0}) base_radius={:.0} extra_dist={} struct_s={} team_s={} team_res={} skill_sets={:?}",
                p.index,
                ai.config.base_defense,
                ai.base_center.0,
                ai.base_center.1,
                ai.base_radius,
                ai.config.base_defense_extra_distance,
                ai.config.structure_seconds,
                ai.config.team_seconds,
                ai.config.team_resources_to_build,
                ai.config.skill_sets
            );
            for (i, o) in ai.config.build_list.iter().enumerate() {
                println!(
                    "    build[{i}] {} at ({:.0},{:.0}) init={} auto={} rb={}",
                    o.template, o.location.0, o.location.1, o.initially_built, o.automatically_build, o.rebuilds
                );
            }
        }
    }

    if verbose {
        let (t, n, near) = warehouse_supply(&g, &ai_players);
        println!("  start: warehouses {n}, {t} boxes, near each player {near:?}");
    }
    let mut first_attack: Option<(u32, u8)> = None;
    let mut winner = None;
    let mut end_frame = 0;
    // (player, first frame at which it owns a supply center, 30-frame resolution).
    let mut first_supply: Vec<(u8, u32)> = Vec::new();    for minute in 1..=40u32 {
        for _ in 0..60 {
            // `ZR_FINE=<object id>:<from frame>`: single-step 40 frames from there, one line per frame.
            let fine = std::env::var("ZR_FINE").ok().and_then(|v| {
                let (a, b) = v.split_once(':')?;
                Some((a.parse::<u32>().ok()?, b.parse::<u32>().ok()?))
            });
            if let (true, Some((oid, from))) = (verbose, fine) {
                if g.frame() >= from && g.frame() < from + 40 {
                    for _ in 0..30 {
                        snapshot::run_frames(&mut g, 1, snap.as_mut()).expect("snapshot write");
                        if let Some(o) = g.logic().objects().find(|o| o.id.0 == oid) {
                            println!(
                                "  fine f{} #{} ({:.2},{:.2}) sub {:?} idx {} path {} wait {} blocked {} stuck {} bframes {} qfp {} pts {} goal {:?} rpath {} speed {:.3}",
                                g.frame(),
                                oid,
                                o.position.x,
                                o.position.y,
                                o.ai.machine.dock_sub,
                                o.ai.machine.dock_approach,
                                o.ai.path.len(),
                                o.ai.waiting_for_path,
                                o.ai.is_blocked,
                                o.ai.is_blocked_and_stuck,
                                o.ai.blocked_frames,
                                o.ai.queue_for_path_frame,
                                o.ai.path_timestamp,
                                o.ai.move_goal.map(|c| (c.x as i32, c.y as i32)),
                                o.ai.raw_path.is_some(),
                                o.ai.speed
                            );
                        }
                    }
                    continue;
                }
            }
            snapshot::run_frames(&mut g, 30, snap.as_mut()).expect("snapshot write");
            // `ZR_TRACE=<player>:<from frame>`: every 90 frames print that player's docking trucks.
            if verbose {
                if let Some((tp, from)) = std::env::var("ZR_TRACE").ok().and_then(|v| {
                    let (a, b) = v.split_once(':')?;
                    Some((a.parse::<u8>().ok()?, b.parse::<u32>().ok()?))
                }) {
                    if g.frame() >= from && g.frame() % 90 == 0 && g.frame() < from + 900 {
                        for o in g.logic().objects().filter(|o| o.player == tp && o.template.supply_truck.is_some()) {
                            if o.supply_ai.state == zr_sim::supply::SupplyState::Docking {
                                println!(
                                    "  f{} #{} ({:.0},{:.0}) boxes {} -> {:?} sub {:?} idx {} path {} blocked {} wait {}",
                                    g.frame(),
                                    o.id.0,
                                    o.position.x,
                                    o.position.y,
                                    o.boxes,
                                    o.supply_target.map(|t| t.0),
                                    o.ai.machine.dock_sub,
                                    o.ai.machine.dock_approach,
                                    o.ai.path.len(),
                                    o.ai.is_blocked,
                                    o.ai.waiting_for_path
                                );
                            }
                        }
                    }
                }
            }
            for &p in &ai_players {
                if first_supply.iter().all(|(q, _)| *q != p) && supply_count(&g, p) > 0 {
                    first_supply.push((p, g.frame()));
                }
            }
        }
        let end = g.logic().check_for_game_end();
        if let Some(w) = end.winner {
            winner = Some(w);
            end_frame = g.frame();
        }
        let mut line = format!("t={minute:>2}min frame {:>6}", g.frame());
        for &p in &ai_players {
            let units = mobile(&g, p);
            let (hp, sn) = struct_hp(&g, p);
            let (attacking, moving, hunting) = fighting(&g, p);
            let teams = g.logic().scripts().teams.values().filter(|t| t.created && !t.members.is_empty()).count();
            let sup = supply_count(&g, p);
            let doz = g.logic().objects().filter(|o| o.player == p && o.is_kind_of(zr_sim::template::kind_of::DOZER)).count();
            let (def, pow, sci) = g.logic().ai_player(p).map_or((0, 0, 0), |a| (a.base_defenses_built, a.powers_fired, a.sciences_bought));
            // Auto-deposit income sources (GLA black markets: `AutoDepositUpdate` $20 / 2 s each).
            let bm = g.logic().objects().filter(|o| o.player == p && o.template.name.to_ascii_lowercase().contains("blackmarket")).count();
            if attacking > 0 && first_attack.is_none() {
                first_attack = Some((g.frame(), p));
            }
            line.push_str(&format!(
                "  p{p}: money {:>5} units {units:>3} hunt {hunting:>3} atk {attacking:>3} mv {moving:>3} teams {teams:>2} structs {sn}/{hp} sup {sup} doz {doz} def {def} pow {pow} sci {sci} bm {bm}",
                g.player_money(p).unwrap_or(0)
            ));
        }
        let (wh_total, wh_n, wh_near) = warehouse_supply(&g, &ai_players);
        line.push_str(&format!("  | wh {wh_total} boxes/{wh_n} near {wh_near:?}"));
        if verbose {
            println!("{line}");
        }
        // Snapshot at a few minutes: owned objects by template (count, queued production) and the AI's
        // team queue / timers, to compare with the C++ `ZR_OBJ` / `ZR_AI` lines.
        if verbose && matches!(minute, 6 | 10 | 14 | 18 | 22) {
            for &p in &ai_players {
                let mut counts: std::collections::BTreeMap<String, (usize, usize)> = std::collections::BTreeMap::new();
                for o in g.logic().objects().filter(|o| o.player == p && !o.is_destroyed()) {
                    let e = counts.entry(o.template.name.clone()).or_default();
                    e.0 += 1;
                    if let Some(pr) = &o.production {
                        e.1 += pr.queue.len();
                    }
                }
                let list: Vec<String> = counts.iter().map(|(n, (c, q))| format!("{n}x{c}{}", if *q > 0 { format!("(q{q})") } else { String::new() })).collect();
                println!("  snap min{minute} p{p}: {}", list.join(" "));
                if let Some(ai) = g.logic().ai_player(p) {
                    println!(
                        "  snap min{minute} p{p} teams: build_queue={} ready_to_build_team={} team_timer={} team_delay={}",
                        ai.team_build_queue.len(),
                        ai.ready_to_build_team,
                        ai.script_team_timer,
                        ai.team_delay
                    );
                    for q in &ai.team_build_queue {
                        let orders: Vec<String> = q
                            .work_orders
                            .iter()
                            .map(|w| format!("{}:{}/{}{}", w.thing, w.completed, w.required_count, if w.factory.is_some() { "*" } else { "" }))
                            .collect();
                        let cond = g.logic().scripts().team_proto(&q.key).map(|t| (t.production_condition.clone(), t.max_instances, t.production_priority)).unwrap_or_default();
                        println!("  snap min{minute} p{p} queued {} prio={} cond='{}' max={} pri={} [{}]", q.key, q.priority_build, cond.0, cond.1, cond.2, orders.join(" "));
                    }
                }
            }
        }
        if winner.is_some() {
            break;
        }
    }
    if verbose {
        let s = g.logic().scripts();
        let mut teams: Vec<String> = s
            .teams
            .iter()
            .filter(|(_, t)| t.created)
            .map(|(n, t)| format!("{n}({:?},{})", t.state, t.members.len()))
            .collect();
        teams.sort();
        println!("winner {winner:?} at frame {end_frame}; first attack {first_attack:?}; first supply center {first_supply:?}; {} live teams", teams.len());
        for &p in &ai_players {
            let (hp, sn) = struct_hp(&g, p);
            let units = mobile(&g, p);
            let vic = g
                .logic()
                .objects()
                .filter(|o| {
                    o.player == p
                        && o.is_kind_of(zr_sim::template::kind_of::STRUCTURE)
                        && o.is_kind_of(zr_sim::template::kind_of::MP_COUNT_FOR_VICTORY)
                })
                .count();
            let (cand, pcf, aff) = g.logic().ai_player(p).map_or((0, 0, 0), |a| (a.team_candidates, a.team_pc_fail, a.team_aff_fail));
            let bq = g.logic().ai_player(p).map_or(0, |a| a.team_build_queue.len());
            let rq = g.logic().ai_player(p).map_or(0, |a| a.team_ready_queue.len());
            let (def, pow, sci) = g.logic().ai_player(p).map_or((0, 0, 0), |a| (a.base_defenses_built, a.powers_fired, a.sciences_bought));
            println!("  p{p} end: units {units} structures {sn} (hp {hp}) victory-buildings {vic} money {} cand {cand} pc_fail {pcf} aff_fail {aff} buildq {bq} readyq {rq} | base_defenses {def} powers_fired {pow} sciences_bought {sci} spp {} skill {} rank {}", g.player_money(p).unwrap_or(0), g.logic().player_science_purchase_points(p), g.logic().player_skill_points(p), g.logic().player_rank_level(p));
            let mut structs: Vec<String> = g
                .logic()
                .objects()
                .filter(|o| o.player == p && o.is_kind_of(zr_sim::template::kind_of::STRUCTURE))
                .map(|o| format!("{}({:.0},{:.0})", o.template.name, o.position.x, o.position.y))
                .collect();
            structs.sort();
            let scis: Vec<String> = g.logic().player_sciences(p).map(|s| s.iter().cloned().collect()).unwrap_or_default();
            println!("    p{p} structures {structs:?}");
            println!("    p{p} sciences {scis:?}");
        }
        // Supply diagnosis: each live warehouse (position, boxes), each AI's supply centers and trucks.
        let l = g.logic();
        for o in l.objects().filter(|o| o.template.dock.as_ref().is_some_and(|d| d.kind == zr_sim::DockKind::Warehouse)) {
            println!(
                "  warehouse #{} {} at ({:.0},{:.0}) boxes {} dock: active {} inside {} open {} crippled {}",
                o.id.0,
                o.template.name,
                o.position.x,
                o.position.y,
                o.boxes,
                o.dock_state.active_docker.0,
                o.dock_state.docker_inside,
                o.dock_state.open,
                o.dock_state.crippled
            );
        }
        for o in l.objects().filter(|o| o.template.dock.as_ref().is_some_and(|d| d.kind == zr_sim::DockKind::Center)) {
            println!(
                "  center #{} p{} dock: active {} inside {} open {} crippled {}",
                o.id.0, o.player, o.dock_state.active_docker.0, o.dock_state.docker_inside, o.dock_state.open, o.dock_state.crippled
            );
        }
        for &p in &ai_players {
            for o in l.objects().filter(|o| o.player == p && o.template.dock.as_ref().is_some_and(|d| d.kind == zr_sim::DockKind::Center)) {
                println!("  p{p} supply center #{} {} at ({:.0},{:.0})", o.id.0, o.template.name, o.position.x, o.position.y);
            }
            for o in l.objects().filter(|o| o.player == p && o.template.supply_truck.is_some()) {
                println!(
                    "  p{p} truck #{} {} at ({:.0},{:.0}) state {:?} boxes {} target {:?} preferred {} force_wanting {}",
                    o.id.0,
                    o.template.name,
                    o.position.x,
                    o.position.y,
                    o.supply_ai.state,
                    o.boxes,
                    o.supply_target.map(|t| t.0),
                    o.supply_ai.preferred_dock.0,
                    o.supply_ai.force_wanting
                );
                if std::env::var_os("ZR_TRUCK_DEBUG").is_some() {
                    let m = format!("{:?} sub {:?} approach {}", o.ai.machine.current, o.ai.machine.dock_sub, o.ai.machine.dock_approach);
                    println!(
                        "      blocked {} stuck {} bframes {} waiting_path {} path {} goal {:?} | {m}",
                        o.ai.is_blocked,
                        o.ai.is_blocked_and_stuck,
                        o.ai.blocked_frames,
                        o.ai.waiting_for_path,
                        o.ai.path.len(),
                        o.ai.move_goal.map(|c| (c.x as i32, c.y as i32))
                    );
                }
            }
        }
        println!(
            "scripts ran: actions={} conditions={} unimplemented invocations={} distinct={}",
            s.actions_executed,
            s.conditions_evaluated,
            s.warnings.values().sum::<u64>(),
            s.warnings.len()
        );
        let mut pows: Vec<_> = g.logic().power_fire_counts().iter().collect();
        pows.sort_by(|a, b| b.1.cmp(a.1));
        println!("    special powers fired ({} distinct):", pows.len());
        for (n, c) in pows.iter() {
            println!("      fired {c:>6}  {n}");
        }
        let mut apows: Vec<_> = s.special_power_action_counts.iter().collect();
        apows.sort_by(|a, b| b.1.cmp(a.1));
        println!("    SKIRMISH_FIRE_SPECIAL_POWER_AT_MOST_COST invocations ({} distinct):", apows.len());
        for (n, c) in apows.iter().take(15) {
            println!("      action {c:>6}  {n}");
        }
        let mut bs: Vec<_> = g.logic().ai_build_specific_counts().iter().collect();
        bs.sort_by(|a, b| b.1.cmp(a.1));
        println!("    build-specific requests ({} distinct):", bs.len());
        for (n, c) in bs.iter().take(20) {
            println!("      buildspec {c:>6}  {n}");
        }
        let mut sf: Vec<_> = g.logic().ai_structure_fail_counts().iter().collect();
        sf.sort_by(|a, b| b.1.cmp(a.1));
        println!("    structure build-start failures ({} distinct):", sf.len());
        for (n, c) in sf.iter().take(20) {
            println!("      fail {c:>6}  {n}");
        }
        let mut top: Vec<_> = s.warnings.iter().collect();
        top.sort_by(|a, b| b.1.cmp(a.1));
        for (n, c) in top.iter().take(15) {
            println!("    {c:>8}  {n}");
        }
    }
    if let Some(s) = snap.as_mut() {
        s.finish().expect("snapshot flush");
    }
    (g.crc(), winner, end_frame)
}

fn main() {
    let installs = Installs::detect();
    let Some(zh) = &installs.zero_hour else {
        eprintln!("no Zero Hour install found; set ZR_ZEROHOUR_DIR");
        return;
    };
    let vfs = Vfs::zero_hour(zh).expect("mounting Zero Hour");
    let data = Arc::new(GameData::load(&vfs));
    let args: Vec<String> = std::env::args().skip(1).collect();
    // `--map NAME` picks the map (default Alpine Assault); every other non-difficulty argument is a
    // faction, so 2, 3 or 4+ AIs can be run (4 = FFA on a 4-start map).
    let mut map = "Alpine Assault".to_owned();
    // `--seed N` feeds `GameLogic`'s RNG (default 1, the seed the matrix CRCs were recorded with).
    let mut seed = 1u32;
    let mut rest: Vec<String> = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--map" {
            if let Some(m) = it.next() {
                map = m.clone();
            }
        } else if a == "--seed" {
            seed = it.next().and_then(|s| s.parse().ok()).expect("--seed needs an unsigned integer");
        } else {
            rest.push(a.clone());
        }
    }
    let difficulty = match rest.last().map(String::as_str) {
        Some("easy") => AiDifficulty::Easy,
        Some("hard") => AiDifficulty::Hard,
        _ => AiDifficulty::Normal,
    };
    let mut factions: Vec<String> = rest
        .into_iter()
        .filter(|s| s != "easy" && s != "hard")
        .collect();
    if factions.is_empty() {
        factions = vec!["FactionChina".to_owned(), "FactionAmerica".to_owned()];
    }

    let (a, wa, ea) = run(&data, &vfs, &map, &factions, difficulty, seed, true);
    let (b, wb, eb) = run(&data, &vfs, &map, &factions, difficulty, seed, false);
    println!("{map}: {} [{difficulty:?}] seed {seed}: winner {wa:?} frame {ea}; final CRC run 1 {a:08x}, run 2 {b:08x}", factions.join(" vs "));
    assert_eq!(a, b, "two runs from the same seed diverged");
    assert_eq!((wa, ea), (wb, eb));
    println!("deterministic: OK");
}
