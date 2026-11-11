//! Lane 170 (wave 20): AN1 seed 1 (USA p2 vs China p3, Normal, Alpine Assault) — the fine A*'s
//! moving-ally cost penalty must be measured from the search's start cell, not from the parent cell
//! being expanded (`AIPathfind.cpp:6544-6552`, `startCellNdx` from `worldToCell`, `:6811-6812`).
//!
//! Red Guard 257 leaves the China barracks at f2808 and follows its exit path from f2838. The C++ A*
//! returns the 44-cell route through cells (103,33) and (104,33); with the parent-relative distance the
//! port charged `3*COST_DIAGONAL` for every cell near the moving dozer 211 and returned a 47-cell
//! detour through (105,31), 56 cost worse, and the unit's heading drifted 1.0 unit by f2963
//! (`zr-refs/AN1_s1/cpp_fine.snap`).
//!
//! The test uses the player's Zero Hour install and skips (passes) when none is found.

use std::sync::Arc;

use zr_assets::{Installs, Vfs};
use zr_game::{AiDifficulty, Game, GameData, PlayerSetup};

fn vfs() -> Option<Vfs> {
    let zh = Installs::detect().zero_hour?;
    Vfs::zero_hour(&zh).ok()
}

/// AN1 seed 1: Red Guard 257's barracks-exit path is the C++'s 44-cell route. Revert check: with the
/// distance measured from the parent cell the port returns 47 cells and this fails.
#[test]
fn an1_the_red_guards_exit_path_is_the_c_plus_plus_route() {
    let Some(vfs) = vfs() else { return };
    let data = Arc::new(GameData::load(&vfs));
    let setups = [
        PlayerSetup::ai("FactionAmerica", 0).with_difficulty(AiDifficulty::Normal),
        PlayerSetup::ai("FactionChina", 1).with_difficulty(AiDifficulty::Normal),
    ];
    let mut g = Game::new_skirmish_with(data, &vfs, "Alpine Assault", &setups, 1).unwrap();
    while g.frame() < 2838 {
        g.step();
    }
    let o = g.logic().find_object(zr_sim::ObjectId(257)).expect("Red Guard 257 exists at f2838");
    assert_eq!(o.template.name, "ChinaInfantryRedguard", "object 257 is the barracks' second Red Guard");
    let raw = o.ai.raw_path.as_ref().expect("the exit path is computed at f2837");
    assert_eq!(raw.nodes.len(), 44, "the C++ A* returns 44 cells; the port's detour had 47");
    // The C++ route's cells around the dozer 211: (103,33) and (104,33) are on it, (105,31) is not.
    let cells: Vec<(i32, i32)> = raw
        .nodes
        .iter()
        .map(|n| ((n.pos.x / 10.0).floor() as i32, (n.pos.y / 10.0).floor() as i32))
        .collect();
    assert!(cells.contains(&(103, 33)) && cells.contains(&(104, 33)), "the route passes the dozer: {cells:?}");
    assert!(!cells.contains(&(105, 31)), "no detour north of the dozer: {cells:?}");
}
