//! Tests against the player's Zero Hour install. They skip (pass) when none is found.

use std::sync::Arc;
use zr_assets::{Installs, Vfs};
use zr_game::{Game, GameData, PlayerSetup};
use zr_sim::template::LocomotorKind;
use zr_sim::TemplateSource;

fn vfs() -> Option<Vfs> {
    let zh = Installs::detect().zero_hour?;
    Vfs::zero_hour(&zh).ok()
}

#[test]
fn crusader_is_one_unit_per_frame_and_six_degrees_per_frame() {
    let Some(vfs) = vfs() else { return };
    let data = GameData::load(&vfs);
    assert!(data.warnings.is_empty());
    let t = data.store.find_template("AmericaTankCrusader").expect("Crusader");
    let l = t.locomotor.expect("locomotor");
    assert_eq!(l.kind, LocomotorKind::Treads);
    assert_eq!(l.max_speed, 1.0);
    assert!((l.max_turn_rate - 0.10472).abs() < 1e-5, "{}", l.max_turn_rate);
    assert_eq!(t.max_health, 480.0);
}

fn start(data: &Arc<GameData>, vfs: &Vfs, seed: u32) -> Game {
    let setups = [PlayerSetup::new("FactionAmerica", 0), PlayerSetup::new("FactionChina", 1)];
    Game::new_skirmish_with(data.clone(), vfs, "Alpine Assault", &setups, seed).unwrap()
}

#[test]
fn alpine_start_matches_the_player_templates() {
    let Some(vfs) = vfs() else { return };
    let data = Arc::new(GameData::load(&vfs));
    let g = start(&data, &vfs, 1);
    let sets: Vec<_> = g.spawned().iter().map(|s| (s.player, s.template.as_str())).collect();
    assert_eq!(
        sets,
        vec![(2, "AmericaCommandCenter"), (2, "AmericaVehicleDozer"), (3, "ChinaCommandCenter"), (3, "ChinaVehicleDozer")]
    );
    assert_eq!(g.skirmish_player(0).unwrap().money, 10000);
    // `Player::init` also gives the civilian (p1) and replay-observer (p4) players the starting cash
    // (Player.cpp:427-443; the AN3 snapshot's `P p=1` / `P p=4` money).
    assert_eq!(g.player(1).unwrap().money, 10000);
    assert_eq!(g.player(4).unwrap().money, 10000);
    let counts = g.object_counts_by_owner();
    assert_eq!((counts[&2], counts[&3]), (2, 2));
    // Start waypoints are where the buildings stand.
    let cc = g.object(g.spawned()[0].id).unwrap();
    let w = g.waypoint("Player_1_Start").unwrap();
    assert_eq!((cc.position.x, cc.position.y), (w.pos.x, w.pos.y));
}

/// `PlayerList::newGame` -> `TeamFactory::initFromSides` (Team.cpp:217-241) creates an inactive
/// instance of every singleton team prototype (ids from `m_uniqueTeamID`), then `Player::setDefaultTeam`
/// (Player.cpp:760-770) activates each player's `team` + player name. For AN3 seed 1 the C++ snapshot's
/// `S` records are ids 1,2,3,4,6: `teamReplayObserver`, `team`, `teamPlyrCivilian`, `teamSkirmishGLA0`,
/// `teamSkirmishAmerica1`; id 5 is the GLA `GLA Worker` singleton, created but never activated.
#[test]
fn alpine_start_creates_every_players_default_team() {
    let Some(vfs) = vfs() else { return };
    let data = Arc::new(GameData::load(&vfs));
    let setups = [PlayerSetup::ai("FactionGLA", 0), PlayerSetup::ai("FactionAmerica", 1)];
    let g = Game::new_skirmish_with(data.clone(), &vfs, "Alpine Assault", &setups, 1).unwrap();
    let s = g.logic().scripts();
    let mut active: Vec<(u32, String)> = s.teams.values().filter(|t| t.created).map(|t| (t.seq, t.proto.clone())).collect();
    active.sort_unstable();
    assert_eq!(
        active,
        vec![
            (1, "teamreplayobserver".to_owned()),
            (2, "team".to_owned()),
            (3, "teamplyrcivilian".to_owned()),
            (4, "teamskirmishgla0".to_owned()),
            (6, "teamskirmishamerica1".to_owned()),
        ],
        "the default teams are active, in the C++ snapshot's id order"
    );
    assert!(s.teams.contains_key("gla worker0"), "the singleton instance exists (id 5) but is inactive");
    assert!(!s.teams["gla worker0"].created, "a singleton instance is created inactive");
    // `GameLogic::startNewGame` puts every map object on the team its `originalOwner` names
    // (`PlayerList::validateTeam`, PlayerList.cpp:294-308; `newObject(thingTemplate, team)`,
    // GameLogic.cpp:1923-1927) and every starting unit on its owner's default team. These are the
    // C++ snapshot's frame-0 member lists (AN3 seed 1).
    let members = |key: &str| -> Vec<u32> {
        let mut v: Vec<u32> = s.teams[key].members.iter().map(|m| m.0).collect();
        v.sort_unstable();
        v
    };
    assert_eq!(
        members("team"),
        vec![1, 2, 3, 4, 5, 101, 105, 106, 107, 108, 110, 111, 112, 113],
        "the neutral player's map objects"
    );
    assert_eq!(members("teamskirmishgla0"), vec![208, 209, 212], "the GLA start: placed + rebuilt command center, worker");
    assert_eq!(members("teamskirmishamerica1"), vec![210, 211, 213], "the USA start");
    assert_eq!(s.teams["teamplyrcivilian"].members.len(), 193, "the map's civilian buildings are the civilian player's");
    assert!(members("teamreplayobserver").is_empty(), "the observer owns nothing");
    assert!(s.teams["gla worker0"].members.is_empty(), "an inactive team has no members");
}

/// GAME_SHELL runs the same `GameLogic::startNewGame` block as a skirmish game
/// (GameLogic.cpp:1543-1557): the replay observer's `teamReplayObserver` and every side's default
/// `"team" + playerName` team (Core/SidesList.cpp:750-784) exist in the shell map too.
#[test]
fn shell_map_sides_get_the_same_default_teams() {
    let Some(vfs) = vfs() else { return };
    let data = Arc::new(GameData::load(&vfs));
    let g = Game::new_shell(data, &vfs, "Maps\\ShellMap1\\ShellMap1.map", 1).expect("the shell map loads");
    let s = g.logic().scripts();
    assert!(s.team_protos.contains_key("teamreplayobserver"), "teamReplayObserver is unconditional");
    assert!(s.teams.get("teamreplayobserver").is_some_and(|t| t.created), "and the observer's team is active");
    // The neutral player (index 0) and every named shell side have their default team.
    assert!(s.teams.get("team").is_some_and(|t| t.created), "the neutral side's default team");
    for p in g.players() {
        assert!(
            s.player_default_team(p.index).is_some(),
            "player {} ({} / {:?}) has no default team",
            p.index,
            p.name,
            p.kind
        );
    }
}

/// `Bridge::Bridge(Object*)` (`Core/.../GameLogic/Map/TerrainLogic.cpp:318-418`): a landmark bridge
/// creates its four tower objects inside its own creation (`createTower`, :125), so their ids follow
/// the bridge's immediately. Alpine Assault's `MonumentTrainBridge` is the bridge the AN3 comparison
/// depends on (C++ ids 1-5); the expected positions are the C++ snapshot's frame-0 values.
#[test]
fn alpine_landmark_bridge_creates_its_towers_during_its_own_creation() {
    let Some(vfs) = vfs() else { return };
    let data = Arc::new(GameData::load(&vfs));
    let g = start(&data, &vfs, 1);
    let l = g.logic();
    let bridge = l.objects().find(|o| o.template.name == "MonumentTrainBridge").expect("Alpine has a landmark bridge");
    let bridge_id = bridge.id;
    assert_eq!(bridge_id.0, 1, "the landmark bridge is the first map object");
    // `BridgeTowerConcrete{Left,Right}{01,02}` in `BRIDGE_MAX_TOWERS` order, from the C++ run.
    let expected = [
        (2235.2188f32, 1064.1376f32),
        (2189.4004, 1060.0574),
        (2270.6992, 665.7144),
        (2224.8809, 661.6342),
    ];
    for (i, (x, y)) in expected.iter().enumerate() {
        let id = zr_sim::ObjectId(bridge_id.0 + 1 + i as u32);
        let tower = l.find_object(id).unwrap_or_else(|| panic!("tower id {} missing", id.0));
        assert!(tower.template.name.starts_with("BridgeTower"), "id {} is {}", id.0, tower.template.name);
        assert_eq!(tower.bridge, Some(bridge_id), "tower {} is not linked to the bridge", id.0);
        assert!(
            (tower.position.x - x).abs() < 0.1 && (tower.position.y - y).abs() < 0.1,
            "tower {} at ({:.4},{:.4}), C++ ({x},{y})",
            id.0,
            tower.position.x,
            tower.position.y
        );
    }
}

/// `Iron Dragon` ships 116 `FortressWall*` pieces (`KINDOF_WALK_ON_TOP_OF_WALL`); loading it must
/// register the wall layer (`addWallPiece`), so a unit on the wall can be detected by z.
#[test]
fn iron_dragon_registers_its_wall_layer() {
    let Some(vfs) = vfs() else { return };
    let data = Arc::new(GameData::load(&vfs));
    let setups = [PlayerSetup::new("FactionAmerica", 0), PlayerSetup::new("FactionChina", 1)];
    let Ok(g) = Game::new_skirmish_with(data.clone(), &vfs, "Iron Dragon", &setups, 1) else { return };
    let pf = g.logic().pathfinder();
    assert!(pf.wall_piece_count() > 0, "Iron Dragon has no wall pieces");
    assert!(pf.wall_height() > 0.0, "AIData WallHeight not set");
    assert!(pf.wall_extent().is_some());
    let first = g.objects().find(|o| o.template.starts_with("FortressWall")).map(|o| o.position);
    assert!(first.is_some_and(|p| pf.is_point_on_wall(&p)), "a wall piece position is not on the wall");
}

#[test]
fn alpine_same_seed_same_crc_after_n_frames() {
    let Some(vfs) = vfs() else { return };
    let data = Arc::new(GameData::load(&vfs));
    let run = |seed| {
        let mut g = start(&data, &vfs, seed);
        let s = g.spawned()[1].clone();
        let centre = zr_sim::Coord3D::new(1200.0, 1300.0, 0.0);
        g.submit(s.player, zr_sim::Command::MoveTo { ids: vec![s.id], pos: centre });
        g.run_frames(300);
        g.crc()
    };
    assert_eq!(run(1), run(1));
}

/// `Player::initFromDict` (`Player.cpp:822`): a human slot gets the Civilian skirmish side's script
/// list, never the faction's `Skirmish<Faction>` AI list (e.g. the "Give AI money to cover bug"
/// one-shot); only the computer slot gets that.
#[test]
fn human_slots_do_not_get_the_faction_ai_scripts() {
    let Some(vfs) = vfs() else { return };
    let data = Arc::new(GameData::load(&vfs));
    let setups = [PlayerSetup::new("FactionAmerica", 0), PlayerSetup::ai("FactionChina", 1)];
    let g = Game::new_skirmish_with(data, &vfs, "Alpine Assault", &setups, 1).unwrap();
    let human = g.skirmish_player(0).unwrap().index;
    let ai = g.skirmish_player(1).unwrap().index;
    let lists = &g.logic().scripts().lists;
    let count = |p| {
        lists
            .iter()
            .filter(|l| l.player == p)
            .map(|l| l.scripts.len() + l.groups.iter().map(|g| g.scripts.len()).sum::<usize>())
            .sum::<usize>()
    };
    let has_ai_money = |p| {
        lists.iter().filter(|l| l.player == p).any(|l| {
            l.scripts.iter().chain(l.groups.iter().flat_map(|g| g.scripts.iter())).any(|s| s.def.name.to_ascii_lowercase().contains("give ai money"))
        })
    };
    assert!(!has_ai_money(human), "human got the AI money script");
    assert!(has_ai_money(ai), "AI lost its money script");
    assert!(count(ai) > count(human), "AI {} vs human {}", count(ai), count(human));
}
