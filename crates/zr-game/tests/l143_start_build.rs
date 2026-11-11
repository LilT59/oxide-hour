//! Lane 143 (wave 17): the starting dozer takes the first scripted build on frame 1, so the frame 2
//! snapshot shows the structure under construction.
//!
//! The one-shot skirmish start scripts (`China 1st Power Plant - N` for AN1, `GLA Barracks - H` for
//! AH7, both `CONDITION_TRUE`/dozer-count conditions in `Data\Scripts\SkirmishScripts.scb`) run
//! `SKIRMISH_BUILD_BUILDING` -> `ScriptActions::doBuildBuilding` (`ScriptActions.cpp:1091`) ->
//! `Player::buildSpecificBuilding` -> `AISkirmishPlayer::buildSpecificAIBuilding`
//! (`AISkirmishPlayer.cpp:410-460`), which only marks the build-list entry priority and clears
//! `m_buildDelay`. `AISkirmishPlayer::doBaseBuilding` (`:869-905`) then runs `processBaseBuilding`
//! (`:100-300`) on frame 1: `m_structureTimer` starts at 2 (`AIPlayer.cpp:77`), it reaches 0 on
//! frame 1, and `m_buildDelay` starts at 0, so the first check is frame 1. The priority entry is
//! taken after `isLocationSafe` (`:197`, `AIPlayer::isLocationSafe` at `AIPlayer.cpp:1004`) and
//! built with `buildStructureWithDozer` (`AIPlayer.cpp:516`). Money is deducted and the scaffold
//! object created in that same frame, so the frame-2 snapshot has the structure at `cons=0.000`,
//! the dozer pointed at it and the cost gone.
//!
//! AN1 seed 1's C++ reference (`zr-refs\AN1_s1\cpp_fine.snap`) at frame 2: `P p=3 money=9600`,
//! `O id=211 ChinaVehicleDozer bld=228`, `O id=228 ChinaPowerPlant cons=0.000` (and
//! `O id=214 SpySatellitePing`, the USA spy-satellite ping the same frame's scripts fire at the
//! China base, which `isLocationSafe` must not see yet: the ping's partition cells are only placed
//! at the end of the frame, `ThingFactory.cpp:346` + `PartitionManager.cpp:2311-2345` +
//! `GameLogic.cpp:3897`). AH7 seed 1 (`zr-refs\AH7_s1\cpp_fine.snap`) at frame 2: `P p=2 money=9500`,
//! `O id=209 GLAInfantryWorker bld=227`, `O id=227 GLABarracks cons=0.000`.
//!
//! Tests against the player's install; they skip (pass) when none is found.

use std::sync::Arc;

use zr_assets::{Installs, Vfs};
use zr_game::{AiDifficulty, Game, GameData, PlayerSetup};

fn vfs() -> Option<Vfs> {
    let zh = Installs::detect().zero_hour?;
    Vfs::zero_hour(&zh).ok()
}

/// Load a matrix matchup (`tools/matchups.json`): slot 0 = player 2, slot 1 = player 3, seed 1, and
/// step two frames (the C++ snapshot's frame 2 is after two updates; `m_frame++` ends
/// `GameLogic::update`, `GameLogic.cpp:3921`).
fn start(data: &Arc<GameData>, vfs: &Vfs, left: &str, right: &str, difficulty: AiDifficulty) -> Game {
    let setups = [PlayerSetup::ai(left, 0).with_difficulty(difficulty), PlayerSetup::ai(right, 1).with_difficulty(difficulty)];
    let mut g = Game::new_skirmish_with(data.clone(), vfs, "Alpine Assault", &setups, 1).unwrap();
    g.run_frames(2);
    assert_eq!(g.frame(), 2);
    g
}

fn find<'a>(g: &'a Game, template: &str, owner: u8) -> Option<&'a zr_sim::Object> {
    g.logic().objects().find(|o| o.template.name == template && o.player == owner)
}

/// AN1 (America p2 vs China p3, Normal): the China scripted power plant starts on frame 1.
#[test]
fn an1_the_starting_dozer_starts_the_scripted_power_plant_on_frame_1() {
    let Some(vfs) = vfs() else { return };
    let data = Arc::new(GameData::load(&vfs));
    let g = start(&data, &vfs, "FactionAmerica", "FactionChina", AiDifficulty::Normal);
    let plant = find(&g, "ChinaPowerPlant", 3).expect("the scripted China 1st Power Plant is started at frame 1");
    assert!(plant.under_construction, "it is a scaffold, `cons=0.000` in the C++ frame-2 snapshot");
    assert_eq!(plant.construction_percent, 0.0);
    let dozer = find(&g, "ChinaVehicleDozer", 3).expect("the starting China dozer");
    assert_eq!(dozer.build_target, Some(plant.id), "O id=211 bld=228");
    assert_eq!(g.player_money(3), Some(9600), "the power plant cost is deducted in the same frame");
    // The USA spy-satellite ping exists at frame 2 (id 214 in the C++) but was created in the same
    // frame's script pass; it must not make `isLocationSafe` skip the site.
    assert!(find(&g, "SpySatellitePing", 2).is_some(), "O id=214 SpySatellitePing");
}

/// AH7 (GLA p2 vs America p3, Hard): the GLA scripted barracks starts on frame 1.
#[test]
fn ah7_the_starting_worker_starts_the_scripted_barracks_on_frame_1() {
    let Some(vfs) = vfs() else { return };
    let data = Arc::new(GameData::load(&vfs));
    let g = start(&data, &vfs, "FactionGLA", "FactionAmerica", AiDifficulty::Hard);
    let barracks = find(&g, "GLABarracks", 2).expect("the scripted GLA Barracks - H is started at frame 1");
    assert!(barracks.under_construction, "`cons=0.000` in the C++ frame-2 snapshot");
    assert_eq!(barracks.construction_percent, 0.0);
    let worker = find(&g, "GLAInfantryWorker", 2).expect("the starting GLA worker is a dozer");
    assert_eq!(worker.build_target, Some(barracks.id), "O id=209 bld=227");
    assert_eq!(g.player_money(2), Some(9500), "the barracks cost is deducted in the same frame");
}
