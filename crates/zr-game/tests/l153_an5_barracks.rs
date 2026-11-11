//! Lane 153 (wave 18): AN5 seed 1 (GLA p2 vs China p3, Normal, Alpine Assault). The China barracks (object 235) queues two
//! Red Guards; the C++ leaves the first at f1991 and the second at f1999 (`zr-refs/AN5_s1/cpp_fine.snap`, `Q obj=235`
//! `done` 1 -> 2, and `O id=248` first present at f1999). Before the fix the port's second Red Guard left at f2001.
//!
//! The cause was not the queue: `GenerateMinefieldBehavior` is an `UpdateModule` (its `update` returns NONE while upgradable,
//! GenerateMinefieldBehavior.cpp:492), so every China building with a minefield module has a sleepy heap entry, and the
//! barracks' `QueueProductionExitUpdate` and `ProductionUpdate` pop in a different order in some frames without them.
//!
//! The test uses the player's Zero Hour install and skips (passes) when none is found.

use std::sync::Arc;

use zr_assets::{Installs, Vfs};
use zr_game::{AiDifficulty, Game, GameData, PlayerSetup};

fn vfs() -> Option<Vfs> {
    let zh = Installs::detect().zero_hour?;
    Vfs::zero_hour(&zh).ok()
}

/// The frames at which a Red Guard produced by the China barracks of player 3 first appears, in order.
fn red_guard_exits(game: &mut Game, frames: u32) -> Vec<u32> {
    let mut seen: Vec<u32> = Vec::new();
    let mut exits = Vec::new();
    let mut barracks = None;
    for _ in 0..frames {
        game.step();
        let logic = game.logic();
        if barracks.is_none() {
            barracks = logic.objects().find(|o| o.player == 3 && o.template.name == "ChinaBarracks").map(|o| o.id);
        }
        let Some(b) = barracks else { continue };
        for o in logic.objects().filter(|o| o.template.name == "ChinaInfantryRedguard" && o.producer_id == b) {
            if !seen.contains(&o.id.0) {
                seen.push(o.id.0);
                exits.push(game.frame());
            }
        }
    }
    exits
}

/// AN5 seed 1 (Normal): the barracks' two Red Guards leave at f1991 and f1999, as the C++ does. Revert check: without the
/// minefield heap entries the second one leaves at f2001.
#[test]
fn an5_the_barracks_second_red_guard_leaves_at_the_c_plus_plus_frame() {
    let Some(vfs) = vfs() else { return };
    let data = Arc::new(GameData::load(&vfs));
    let setups = [
        PlayerSetup::ai("FactionGLA", 0).with_difficulty(AiDifficulty::Normal),
        PlayerSetup::ai("FactionChina", 1).with_difficulty(AiDifficulty::Normal),
    ];
    let mut g = Game::new_skirmish_with(data, &vfs, "Alpine Assault", &setups, 1).unwrap();
    let exits = red_guard_exits(&mut g, 2005);
    assert!(exits.len() >= 2, "the barracks made {exits:?} by f2005");
    assert_eq!(exits[0], 1991, "the first Red Guard leaves at f1991 (C++)");
    assert_eq!(exits[1], 1999, "the second Red Guard leaves at f1999 (C++); the port left it at f{}", exits[1]);
}
