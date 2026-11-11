//! Harness tests on synthetic INI + map data (the fixture `crates/zr-game/tests/synthetic.rs` uses):
//! no game install needed. The determinism test drives one game through the harness lib API and an
//! identical one directly through `zr-game`, and compares CRCs and snapshot bytes.

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use zr_game::snapshot::{self, Snapshot};
use zr_game::{Game, GameData, PlayerSetup};
use zr_harness::protocol;
use zr_harness::{Session, SnapshotSpec};
use zr_ini::IniDb;
use zr_map::{Dict, DictValue, HeightMap, MapFile, MapObject, Side, Sides};
use zr_sim::{Command, Coord3D};

const INI: &str = "
Locomotor TankLoco
  Surfaces = GROUND
  Speed = 30
  TurnRate = 180
  Acceleration = 1000
  Braking = 1000
  Appearance = TREADS
End
Object TestTank
  KindOf = SELECTABLE VEHICLE CAN_ATTACK
  VisionRange = 150
  Body = ActiveBody ModuleTag_Body
    MaxHealth = 480
    InitialHealth = 400
  End
  Behavior = AIUpdateInterface ModuleTag_AI
  End
  Locomotor = SET_NORMAL TankLoco
  Geometry = BOX
  GeometryMajorRadius = 15
  GeometryMinorRadius = 10
  GeometryHeight = 10
End
Object TestHQ
  KindOf = STRUCTURE SELECTABLE IMMOBILE
  PlacementViewAngle = 90
  Body = ActiveBody ModuleTag_Body
    MaxHealth = 1000
    InitialHealth = 1000
  End
  Geometry = BOX
  GeometryMajorRadius = 40
  GeometryMinorRadius = 40
  GeometryHeight = 30
End
Object TestRock
  KindOf = OBSTACLE
  Geometry = SPHERE
  GeometryMajorRadius = 5
End
PlayerTemplate FactionTest
  Side = Test
  StartMoney = 0
  StartingBuilding = TestHQ
  StartingUnit0 = TestTank
  StartingUnit1 = TestTank
End
PlayerTemplate FactionRich
  Side = Rich
  StartMoney = 777
  StartingBuilding = TestHQ
End
MultiplayerStartingMoneyChoice
  Value = 5000
End
MultiplayerStartingMoneyChoice
  Value = 10000
  Default = Yes
End
";

fn data() -> Arc<GameData> {
    let blocks = zr_ini::parse("test.ini", INI).expect("synthetic INI parses");
    let mut db = IniDb::new();
    db.extend(blocks);
    Arc::new(GameData::from_ini(&db))
}

fn ascii(s: &str) -> DictValue {
    DictValue::Ascii(s.to_owned())
}

fn side(name: &str, faction: &str) -> Side {
    Side {
        dict: Dict(vec![("playerName".into(), ascii(name)), ("playerFaction".into(), ascii(faction))]),
        build_list: vec![],
    }
}

fn obj(template: &str, x: f32, y: f32, owner: &str) -> MapObject {
    MapObject {
        version: 3,
        pos: [x, y, 0.0],
        angle: 0.5,
        flags: 0,
        template: template.to_owned(),
        props: Dict(vec![("originalOwner".to_owned(), ascii(owner))]),
    }
}

fn waypoint(name: &str, x: f32, y: f32) -> MapObject {
    let mut w = obj("*Waypoints/Waypoint", x, y, "team");
    w.props.0.push(("waypointName".into(), ascii(name)));
    w.props.0.push(("waypointID".into(), DictValue::Int(1)));
    w
}

fn map() -> MapFile {
    let mut m = MapFile::default();
    let (w, h) = (100usize, 100usize);
    m.height_map = Some(HeightMap { version: 4, width: w as i32, height: h as i32, border_size: 0, boundaries: vec![], data: vec![16; w * h] });
    m.sides = Some(Sides {
        version: 3,
        sides: vec![side("", ""), side("PlyrCivilian", "FactionCivilian")],
        teams: vec![],
        nested: vec![],
        scripts: None,
        script_error: None,
    });
    m.objects = vec![waypoint("Player_1_Start", 300.0, 300.0), waypoint("Player_2_Start", 700.0, 700.0), obj("TestRock", 100.0, 100.0, "team")];
    m
}

fn two_players() -> [PlayerSetup; 2] {
    [PlayerSetup::new("FactionTest", 0), PlayerSetup::new("FactionRich", 1)]
}

fn game(data: &Arc<GameData>, seed: u32) -> Game {
    Game::from_map(data.clone(), &map(), "synthetic", &two_players(), seed).expect("synthetic game")
}

/// A unique path under the system temp dir (cleaned up by the test).
fn temp_path(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("zr-harness-{}-{}-{name}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos())));
    p
}

fn tank(g: &Game) -> zr_game::Spawned {
    g.spawned().iter().find(|s| s.template == "TestTank").expect("a starting tank").clone()
}

/// The four-chunk command sequence both runs use: move, wait, add a waypoint, stop, then move again.
fn commands(g: &Game) -> Vec<(u32, Command)> {
    let t = tank(g);
    vec![
        (0, Command::MoveTo { ids: vec![t.id], pos: Coord3D::new(600.0, 600.0, 0.0) }),
        (40, Command::AddWaypoint { ids: vec![t.id], pos: Coord3D::new(200.0, 600.0, 0.0) }),
        (120, Command::Stop { ids: vec![t.id] }),
        (200, Command::AttackMove { ids: vec![t.id], pos: Coord3D::new(500.0, 200.0, 0.0) }),
    ]
}

/// Run the command sequence directly through `zr-game` (`Game::submit` / `Game::run_frames`), with a
/// streaming snapshot writer (`ZR_SNAPSHOT_EVERY=30`), returning the CRC at each 50-frame checkpoint.
fn run_direct(g: &mut Game, snap_path: &Path) -> Vec<(u32, u32)> {
    let file = File::create(snap_path).expect("snapshot file");
    let mut snap = Snapshot::new(BufWriter::new(file), 30, false);
    // Frame 0 before any order, like `Session::start_snapshot` (the queued-message count is part of
    // `GameLogic::crc`, so the block must be written at the same point in both runs).
    snap.write_if_due(g).expect("frame 0 snapshot");
    let mut out = Vec::new();
    let mut at = 0u32;
    for (frame, cmd) in commands(g) {
        while at < frame {
            let n = (frame - at).min(50);
            snapshot::run_frames(g, n, Some(&mut snap)).expect("snapshot write");
            at += n;
            if at % 50 == 0 {
                out.push((g.frame(), g.crc()));
            }
        }
        g.submit(tank(g).player, cmd);
    }
    while at < 300 {
        let n = (300 - at).min(50);
        snapshot::run_frames(g, n, Some(&mut snap)).expect("snapshot write");
        at += n;
        out.push((g.frame(), g.crc()));
    }
    snap.finish().expect("snapshot flush");
    out
}

/// The same sequence through the harness lib (`Session::submit`, `Session::step`), snapshot enabled
/// from frame 0 like `load`'s `snapshot` option.
fn run_harness(s: &mut Session, snap_path: &Path) -> Vec<(u32, u32)> {
    s.start_snapshot(&SnapshotSpec { path: snap_path.to_path_buf(), every: 30, bits: false }).expect("start snapshot");
    let mut out = Vec::new();
    let mut at = 0u32;
    for (frame, cmd) in commands(s.game().expect("loaded")) {
        while at < frame {
            let n = (frame - at).min(50);
            s.step(n).expect("step");
            at += n;
            if at % 50 == 0 {
                out.push((s.frame().expect("frame"), s.crc_u32().expect("crc")));
            }
        }
        let player = tank(s.game().expect("loaded")).player;
        s.submit(player, cmd).expect("command");
    }
    while at < 300 {
        let n = (300 - at).min(50);
        s.step(n).expect("step");
        at += n;
        out.push((s.frame().expect("frame"), s.crc_u32().expect("crc")));
    }
    s.stop_snapshot().expect("stop snapshot");
    out
}

#[test]
fn harness_matches_a_direct_game_in_crc_and_snapshot_bytes() {
    let data = data();
    let mut direct = game(&data, 7);
    let mut harness = Session::from_game(game(&data, 7));
    let direct_snap = temp_path("direct.snap");
    let harness_snap = temp_path("harness.snap");

    let direct_crcs = run_direct(&mut direct, &direct_snap);
    let harness_crcs = run_harness(&mut harness, &harness_snap);

    assert_eq!(harness_crcs, direct_crcs, "the harness run and the direct run diverged");
    assert_eq!(harness.frame(), Some(300));
    assert_eq!(harness.crc_u32(), Some(direct.crc()));
    // The snapshot writer is the same code, so an identical command stream must give identical bytes
    // (including the `FRAME crc=` field, so this also pins the CRC).
    let a = String::from_utf8(std::fs::read(&direct_snap).expect("direct snapshot")).expect("ascii");
    let b = String::from_utf8(std::fs::read(&harness_snap).expect("harness snapshot")).expect("ascii");
    let (al, bl): (Vec<&str>, Vec<&str>) = (a.lines().collect(), b.lines().collect());
    let first_diff = al.iter().zip(&bl).position(|(x, y)| x != y);
    match first_diff {
        None if al.len() == bl.len() => {}
        None => panic!("snapshot line count differs: direct {}, harness {}", al.len(), bl.len()),
        Some(i) => panic!(
            "snapshot line {i} differs ({} vs {} lines):\n  direct: {:?}\n  harness: {:?}",
            al.len(),
            bl.len(),
            al[i],
            bl[i]
        ),
    }
    assert!(a.starts_with("SNAP f=0 FRAME "), "frame 0 is written first");
    assert!(a.lines().any(|l| l.starts_with("SNAP f=300 FRAME ")), "frame 300 block written");
    assert_eq!(a.lines().filter(|l| l.starts_with("SNAP f=300 FRAME ")).count(), 1, "once per due frame");
    let _ = std::fs::remove_file(&direct_snap);
    let _ = std::fs::remove_file(&harness_snap);
}

#[test]
fn queries_read_the_same_objects_the_simulation_has() {
    let data = data();
    let mut s = Session::from_game(game(&data, 7));
    s.step(5).expect("step");

    let reply = protocol::handle_line(&mut s, r#"{"cmd":"query","what":"objects","kind":"VEHICLE"}"#).0;
    assert_eq!(reply["ok"], true, "{reply}");
    let objects = reply["objects"].as_array().expect("objects array");
    assert_eq!(objects.len(), 2, "two starting tanks: {reply}");
    {
        let g = s.game().expect("loaded");
        for o in objects {
            let id = o["id"].as_u64().expect("id") as u32;
            let sim_object = g.logic().find_object(zr_sim::ObjectId(id)).expect("object exists");
            assert_eq!(o["template"], sim_object.template.name);
            assert_eq!(o["owner"], sim_object.player);
            assert_eq!(o["health"].as_f64().expect("health"), f64::from((sim_object.health() * 1000.0).round() / 1000.0));
            assert!(o["pos"].as_array().is_some_and(|p| p.len() == 3));
            assert!(o["ai"].is_string(), "AI state is a string");
        }
    }

    // Filters: one id, one owner, an exact template, and a limit.
    let one = protocol::handle_line(&mut s, &format!(r#"{{"cmd":"query","what":"objects","id":{}}}"#, objects[0]["id"])).0;
    assert_eq!(one["count"], 1, "{one}");
    let owner = protocol::handle_line(&mut s, r#"{"cmd":"query","what":"objects","owner":2}"#).0;
    assert!(owner["objects"].as_array().expect("array").iter().all(|o| o["owner"] == 2));
    let tmpl = protocol::handle_line(&mut s, r#"{"cmd":"query","what":"objects","template":"TestHQ"}"#).0;
    assert_eq!(tmpl["count"], 2, "one HQ per starting set: {tmpl}");
    let limited = protocol::handle_line(&mut s, r#"{"cmd":"query","what":"objects","limit":1}"#).0;
    assert_eq!(limited["count"], 1, "{limited}");

    // Player state: money, power, rank/skill points and score.
    let players = protocol::handle_line(&mut s, r#"{"cmd":"query","what":"players"}"#).0;
    let p2 = players["players"].as_array().expect("players").iter().find(|p| p["index"] == 2).expect("player 2").clone();
    assert_eq!(p2["money"], 10000);
    assert_eq!(p2["kind"], "skirmish");
    assert!(p2["power"]["sufficient"].is_boolean());
    assert!(p2["rank"].as_i64().is_some());
    assert!(p2["score"].as_u64().is_some());

    // AI state (this fixture is not an AI game, so the list is empty but present).
    let ai = protocol::handle_line(&mut s, r#"{"cmd":"query","what":"ai"}"#).0;
    assert_eq!(ai["ok"], true, "{ai}");
    assert_eq!(ai["ai"].as_array().map(Vec::len), Some(0));

    // Script state: counters and flags are objects.
    let scripts = protocol::handle_line(&mut s, r#"{"cmd":"query","what":"scripts"}"#).0;
    assert_eq!(scripts["ok"], true, "{scripts}");
    assert!(scripts["scripts"]["counters"].is_object());
    assert!(scripts["scripts"]["flags"].is_object());
    assert!(scripts["scripts"]["actions_executed"].as_u64().is_some());

    // `all` includes every part.
    let all = protocol::handle_line(&mut s, r#"{"cmd":"query","what":"all"}"#).0;
    for key in ["objects", "players", "ai", "scripts"] {
        assert!(all.get(key).is_some(), "all is missing {key}: {all}");
    }
}

#[test]
fn bad_requests_are_errors_and_never_stop_the_session() {
    let data = data();
    let mut s = Session::from_game(game(&data, 7));
    let lines = [
        "not json",
        "[]",
        "{}",
        r#"{"cmd":"nope"}"#,
        r#"{"cmd":"step"}"#,
        r#"{"cmd":"step","frames":1,"until":2}"#,
        r#"{"cmd":"step","until":-1}"#,
        r#"{"cmd":"command","player":0,"command":{"type":"move_to","ids":[1]}}"#,
        r#"{"cmd":"command","player":99,"command":{"type":"stop","ids":[1]}}"#,
        r#"{"cmd":"command","player":2,"command":{"type":"stop","ids":["x"]}}"#,
        r#"{"cmd":"query","what":"nope"}"#,
        r#"{"cmd":"query","what":"objects","kind":"NOT_A_KIND"}"#,
        r#"{"cmd":"query","what":"objects","limit":-1}"#,
        r#"{"cmd":"snapshot","path":""}"#,
        r#"{"cmd":"rng_trace","path":"x","off":true}"#,
        r#"{"cmd":"crc"}"#,
        r#"{"cmd":"help"}"#,
    ];
    for line in lines {
        let (reply, quit) = protocol::handle_line(&mut s, line);
        if line == r#"{"cmd":"crc"}"# || line == r#"{"cmd":"help"}"# {
            assert_eq!(reply["ok"], true, "{line} -> {reply}");
        } else {
            assert_eq!(reply["ok"], false, "{line} -> {reply}");
            assert!(reply["error"].as_str().is_some_and(|e| !e.is_empty()), "{line} -> {reply}");
        }
        assert!(!quit, "{line} must not end the session");
    }
    assert_eq!(s.frame(), Some(0), "no failed request may advance or mutate the game");

    let (reply, quit) = protocol::handle_line(&mut s, r#"{"cmd":"quit"}"#);
    assert_eq!(reply["ok"], true);
    assert!(quit);
}

#[test]
fn snapshot_command_writes_the_current_frame_and_then_a_stream() {
    let data = data();
    let mut s = Session::from_game(game(&data, 7));
    let path = temp_path("once.snap");
    let line = format!(r#"{{"cmd":"snapshot","path":{},"bits":true}}"#, serde_json::json!(path));
    let reply = protocol::handle_line(&mut s, &line).0;
    assert_eq!(reply["ok"], true, "{reply}");
    let text = std::fs::read_to_string(&path).expect("snapshot");
    assert!(text.lines().all(|l| !l.starts_with("SNAP f=30")), "one frame only: {text}");
    assert_eq!(text.lines().filter(|l| l.contains(" FRAME ")).count(), 1);
    assert!(text.lines().any(|l| l.contains(" O id=") && l.contains(" xb=")), "bits fields: {text}");

    s.step(30).expect("step");
    let line = format!(r#"{{"cmd":"snapshot","path":{}}}"#, serde_json::json!(path));
    let reply = protocol::handle_line(&mut s, &line).0;
    assert_eq!(reply["ok"], true, "{reply}");
    let text = std::fs::read_to_string(&path).expect("snapshot");
    assert!(text.starts_with("SNAP f=30 FRAME "), "the current frame: {text}");

    // Streaming: frame 30 now, then 60 after a step, and stop.
    let stream = temp_path("stream.snap");
    let line = format!(r#"{{"cmd":"snapshot","path":{},"every":30}}"#, serde_json::json!(stream));
    assert_eq!(protocol::handle_line(&mut s, &line).0["ok"], true);
    s.step(30).expect("step");
    let stop = protocol::handle_line(&mut s, r#"{"cmd":"snapshot"}"#).0;
    assert_eq!(stop["ok"], true, "{stop}");
    let text = std::fs::read_to_string(&stream).expect("stream");
    assert!(text.lines().any(|l| l.starts_with("SNAP f=30 FRAME ")));
    assert!(text.lines().any(|l| l.starts_with("SNAP f=60 FRAME ")));
    assert_eq!(text.lines().filter(|l| l.starts_with("SNAP f=30 FRAME ")).count(), 1);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&stream);
}

#[test]
fn rng_trace_toggles_the_session_sink() {
    let data = data();
    let mut s = Session::from_game(game(&data, 7));
    let path = temp_path("rng.txt");
    let line = format!(r#"{{"cmd":"rng_trace","path":{}}}"#, serde_json::json!(path));
    let reply = protocol::handle_line(&mut s, &line).0;
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(reply["traced"], true);
    assert!(path.exists(), "the trace file is created when the trace starts");
    // Starting it twice replaces the sink (truncating the file); stopping it is not an error.
    assert_eq!(protocol::handle_line(&mut s, &line).0["traced"], true);
    let stop = protocol::handle_line(&mut s, r#"{"cmd":"rng_trace","off":true}"#).0;
    assert_eq!(stop["ok"], true, "{stop}");
    assert_eq!(stop["traced"], false);
    let again = protocol::handle_line(&mut s, r#"{"cmd":"rng_trace"}"#).0;
    assert_eq!(again["ok"], true, "stopping a stopped trace is a no-op: {again}");
    // The synthetic fixture draws nothing while idle, so the file is empty; the draw format itself
    // is tested in `tests/rng_trace.rs` and on real data in `tests/real_data.rs`.
    assert_eq!(std::fs::read_to_string(&path).expect("trace").len(), 0);
    let _ = std::fs::remove_file(&path);
}
