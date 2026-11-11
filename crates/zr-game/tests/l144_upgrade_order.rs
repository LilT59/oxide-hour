//! Lane 144 (wave 17): the AI script's `AI_PLAYER_BUILD_UPGRADE` request and an OBJECT upgrade.
//!
//! `AIPlayer::buildUpgrade` (`AIPlayer.cpp:1786-1794`) refuses a request for an `UPGRADE_TYPE_OBJECT`
//! upgrade ("is an object, not a player upgrade"); only the team command-button action
//! (`TEAM_USE_COMMANDBUTTON_ABILITY` -> `Object::doCommandButton`, `Object.cpp:5556`) can order one at a
//! building. In AE1 seed 1 (Easy) America's `USA Upgrade Power Plant - E` script runs
//! `AI_PLAYER_BUILD_UPGRADE[<This Player> Upgrade_AmericaAdvancedControlRods]` at f1024 while the power
//! plant finishes; the C++ reference (`zr-refs/AE1_s1/cpp_fine.snap`) never queues that upgrade (its
//! `ZR_UPG f=1024` diagnostic is the refused request), while the port queued it on the power plant at
//! f1025 (`Q obj=226 i=0`, money 8900 instead of 9400). In AN3 seed 1 (Normal) the same upgrade comes
//! from `USA Upgrade Power Plant - N`'s team-button action and *is* queued by both, which the second test
//! pins: the fix must not break the command-button path.
//!
//! These tests use the player's Zero Hour install and skip (pass) when none is found.

use std::sync::Arc;

use zr_assets::{Installs, Vfs};
use zr_game::{AiDifficulty, Game, GameData, PlayerSetup};

const CONTROL_RODS: &str = "Upgrade_AmericaAdvancedControlRods";

fn vfs() -> Option<Vfs> {
    let zh = Installs::detect().zero_hour?;
    Vfs::zero_hour(&zh).ok()
}

/// The frames at which `player`'s `AmericaPowerPlant` has `CONTROL_RODS` in its production queue, with the
/// entry's object-upgrade flag (`ProductionUpdate::queueUpgrade`'s `UPGRADE_TYPE_OBJECT` branch).
fn rods_queued_at(game: &mut Game, player: u8, frames: u32) -> Vec<(u32, bool)> {
    let mut queued = Vec::new();
    for _ in 0..frames {
        game.step();
        for o in game.logic().objects().filter(|o| o.player == player && o.template.name == "AmericaPowerPlant") {
            let Some(p) = &o.production else { continue };
            for e in &p.queue {
                if matches!(&e.kind, zr_sim::production::ProductionKind::Upgrade(n) if n == CONTROL_RODS) {
                    queued.push((game.frame(), e.object_upgrade));
                }
            }
        }
    }
    queued
}

/// AE1 (Easy, America p2 vs China p3, seed 1): the `- E` script's `AI_PLAYER_BUILD_UPGRADE` request must
/// queue nothing, as the C++ does. Before the fix the port queued the upgrade at f1025 and paid 500
/// (`Q obj=226 i=0` only in the port; the C++'s own `ZR_UPG f=1024` diagnostic is the refused request).
#[test]
fn ae1_easy_script_does_not_queue_the_object_upgrade() {
    let Some(vfs) = vfs() else { return };
    let data = Arc::new(GameData::load(&vfs));
    let setups = [
        PlayerSetup::ai("FactionAmerica", 0).with_difficulty(AiDifficulty::Easy),
        PlayerSetup::ai("FactionChina", 1).with_difficulty(AiDifficulty::Easy),
    ];
    let mut g = Game::new_skirmish_with(data, &vfs, "Alpine Assault", &setups, 1).unwrap();
    let queued = rods_queued_at(&mut g, 2, 1100);
    assert!(queued.is_empty(), "{CONTROL_RODS} was queued at {queued:?}; the C++ AE1 reference never queues it");
}

/// AN3 (Normal, GLA p2 vs America p3, seed 1): the same upgrade through the team command button
/// (`USA Upgrade Power Plant - N` -> `TEAM_USE_COMMANDBUTTON_ABILITY`, `ScriptActions.cpp:4463`) must still be
/// queued at the power plant, as an *object* upgrade (the class value reaches `queueUpgrade`, so it is given to
/// the plant when it finishes). The C++ reference has `Q obj=228 i=0 frames=1` at f1024; the exact frame follows
/// the power plant's completion, which other sim work moves by a few frames (1020-1025 across concurrent edits),
/// so the window is wide.
#[test]
fn an3_normal_team_button_queues_the_object_upgrade() {
    let Some(vfs) = vfs() else { return };
    let data = Arc::new(GameData::load(&vfs));
    let setups = [
        PlayerSetup::ai("FactionGLA", 0).with_difficulty(AiDifficulty::Normal),
        PlayerSetup::ai("FactionAmerica", 1).with_difficulty(AiDifficulty::Normal),
    ];
    let mut g = Game::new_skirmish_with(data, &vfs, "Alpine Assault", &setups, 1).unwrap();
    let queued = rods_queued_at(&mut g, 3, 1100);
    let (first, object) = *queued.first().unwrap_or_else(|| panic!("{CONTROL_RODS} was never queued (C++: f1024)"));
    assert!(object, "the team command-button path queues an object upgrade, not a player upgrade");
    assert!((1000..=1100).contains(&first), "the C++ queues it at f1024; the port at f{first}");
}
