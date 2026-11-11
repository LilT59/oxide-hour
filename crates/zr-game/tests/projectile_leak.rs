//! Real-data check of projectile end states: in an AI-vs-AI skirmish no projectile may sit still
//! for long. Skips (passes) when no Zero Hour install is found.

use std::collections::BTreeMap;
use std::sync::Arc;
use zr_assets::{Installs, Vfs};
use zr_game::{Game, GameData, PlayerSetup};

/// Frames a projectile may stay motionless before it counts as stuck (10 s of game time).
const STUCK_FRAMES: u32 = 300;

fn vfs() -> Option<Vfs> {
    let zh = Installs::detect().zero_hour?;
    Vfs::zero_hour(&zh).ok()
}

#[test]
fn ai_skirmish_projectiles_do_not_hang_in_the_air() {
    let Some(vfs) = vfs() else { return };
    let data = Arc::new(GameData::load(&vfs));
    let ai_vs_ai = [PlayerSetup::ai("FactionAmerica", 0), PlayerSetup::ai("FactionChina", 1)];
    let human_vs_ai = [PlayerSetup::new("FactionAmerica", 0), PlayerSetup::ai("FactionGLA", 1)];
    for setups in [&ai_vs_ai[..], &human_vs_ai[..]] {
        let g = Game::new_skirmish_with(data.clone(), &vfs, "Alpine Assault", setups, 1).unwrap();
        run_and_check(g, 9000);
    }
}

/// A tracked projectile: position last frame, frame it last moved, template name.
type Seen = ((f32, f32, f32), u32, String);

fn run_and_check(mut g: Game, frames: u32) {
    let mut seen: BTreeMap<u32, Seen> = BTreeMap::new();
    let mut peak = 0;
    for _ in 0..frames {
        g.run_frames(1);
        let frame = g.frame();
        let live = g.projectiles();
        peak = peak.max(live.len());
        let mut next = BTreeMap::new();
        for p in live {
            let pos = (p.pos.x, p.pos.y, p.pos.z);
            let (last, moved, name) = match seen.get(&p.id) {
                Some((prev, moved, name)) if *prev == pos => (pos, *moved, name.clone()),
                _ => (pos, frame, p.template_name.clone()),
            };
            next.insert(p.id, (last, moved, name));
        }
        seen = next;
        if frame % 1500 == 0 {
            eprintln!("frame {frame}: {} live projectiles (peak {peak})", seen.len());
        }
    }
    // A projectile on the ground is not a leak here (a landing normally detonates, see the LOG).
    // Only one that hangs in the air and sits still is.
    let frame = g.frame();
    let terrain = g.logic().terrain();
    let mut stuck: BTreeMap<&str, usize> = BTreeMap::new();
    let mut resting: BTreeMap<&str, usize> = BTreeMap::new();
    for ((x, y, z), moved, name) in seen.values() {
        if frame.saturating_sub(*moved) < STUCK_FRAMES {
            continue;
        }
        if *z > terrain.ground_height(*x, *y) + 0.5 {
            *stuck.entry(name.as_str()).or_default() += 1;
        } else {
            *resting.entry(name.as_str()).or_default() += 1;
        }
    }
    eprintln!("end: {} live, peak {peak}, resting on the ground {resting:?}, hanging in the air {stuck:?}", seen.len());
    assert!(stuck.is_empty(), "projectiles hung in the air for {STUCK_FRAMES}+ frames: {stuck:?}");
}
