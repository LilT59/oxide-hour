//! Snapshot writer tests (`docs/HARNESS.md`) on a synthetic INI + map: no game install needed.

use std::borrow::Cow;
use std::ffi::OsStr;
use std::sync::Arc;
use zr_game::snapshot::{self, esc, printed_production, settings, write_frame, write_production, write_script_team, write_team_queue, Snapshot};
use zr_game::{Game, GameData, PlayerSetup};
use zr_ini::IniDb;
use zr_map::{Dict, DictValue, HeightMap, MapFile, MapObject, Side, Sides};
use zr_sim::production::{ProductionEntry, ProductionKind, ProductionState};
use zr_sim::script::ScriptEngine;
use zr_sim::skirmish_ai::{TeamInQueue, WorkOrder};
use zr_sim::{Command, Coord3D, ObjectId};

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
    let side = |name: &str, faction: &str| Side {
        dict: Dict(vec![("playerName".into(), ascii(name)), ("playerFaction".into(), ascii(faction))]),
        build_list: vec![],
    };
    m.sides = Some(Sides {
        version: 3,
        sides: vec![side("", ""), side("PlyrCivilian", "FactionCivilian")],
        teams: vec![],
        nested: vec![],
        scripts: None,
        script_error: None,
    });
    m.objects = vec![
        waypoint("Player_1_Start", 300.0, 300.0),
        waypoint("Player_2_Start", 700.0, 700.0),
        obj("TestRock", 100.0, 100.0, "team"),
    ];
    m
}

fn game(seed: u32) -> Game {
    let setups = [PlayerSetup::new("FactionTest", 0), PlayerSetup::new("FactionRich", 1)];
    Game::from_map(data(), &map(), "synthetic", &setups, seed).expect("synthetic game starts")
}

/// The text of one snapshot block of `g`.
fn block(g: &Game, bits: bool) -> String {
    let mut out = Vec::new();
    write_frame(&mut out, g.logic(), g.crc(), bits).expect("write to memory");
    String::from_utf8(out).expect("ASCII records")
}

/// The record tag: the token after `f=<frame>`.
fn tag(line: &str) -> &str {
    line.split(' ').nth(2).unwrap_or("")
}

/// The key names of a record, in order.
fn keys(line: &str) -> Vec<&str> {
    line.split(' ').skip(3).map(|t| t.split('=').next().unwrap_or("")).collect()
}

#[test]
fn block_has_header_player_and_object_records_in_schema_order() {
    let g = game(1);
    let text = block(&g, false);
    let lines: Vec<&str> = text.lines().collect();
    let header = lines[0];
    assert!(header.starts_with("SNAP f=0 FRAME crc="), "{header}");
    assert!(header.ends_with(&format!("objs={} players={}", g.logic().objects().count(), g.logic().players().count())), "{header}");

    let players: Vec<&&str> = lines.iter().filter(|l| tag(l) == "P").collect();
    assert_eq!(players.len(), g.logic().players().count());
    for p in &players {
        assert_eq!(keys(p), ["p", "money", "power", "prod", "cons", "ai"], "{p}");
    }
    assert!(lines.iter().any(|l| l.starts_with("SNAP f=0 P p=0 money=")), "{text}");

    let objects: Vec<&&str> = lines.iter().filter(|l| tag(l) == "O").collect();
    assert_eq!(objects.len(), g.logic().objects().count());
    let mut last_id = 0;
    for o in &objects {
        assert_eq!(keys(o), ["id", "tmpl", "owner", "x", "y", "z", "hp", "ai", "cons", "q", "atk", "bld"], "{o}");
        let id: u32 = o.split(' ').nth(3).unwrap()["id=".len()..].parse().unwrap();
        assert!(id > last_id, "objects are in id order: {o}");
        last_id = id;
    }
    let tank = objects.iter().find(|o| o.contains("tmpl=TestTank")).expect("a starting tank");
    assert!(tank.contains(" hp=400.000 "), "{tank}");
    assert!(tank.contains(" ai=Idle "), "{tank}");
    // Completed objects print CONSTRUCTION_COMPLETE (-1), not 0.
    assert!(tank.contains(" cons=-1.000 "), "{tank}");
    assert!(tank.ends_with(" q=0 atk=0 bld=0"), "{tank}");
    // No body module: hp `-`; no AIUpdate module: ai `-`.
    let rock = objects.iter().find(|o| o.contains("tmpl=TestRock")).expect("the rock");
    assert!(rock.contains(" hp=- ai=- cons=-1.000 "), "{rock}");
    let hq = objects.iter().find(|o| o.contains("tmpl=TestHQ")).expect("the HQ");
    assert!(hq.contains(" hp=1000.000 ai=- "), "{hq}");

    let with_bits = block(&g, true);
    let bit_line = with_bits.lines().find(|l| tag(l) == "O").expect("an object record");
    assert_eq!(keys(bit_line)[12..], ["xb", "yb", "zb", "hpb", "consb"], "{bit_line}");
}

#[test]
fn the_object_list_counts_objects_destroyed_earlier_in_the_frame() {
    // `GameLogic::getObjectCount()` walks `m_objList` (`GameLogic.cpp:4795-4804`); `destroyObject` only marks
    // the object and queues it (`:4232-4247`), so it leaves the list at the frame's `processDestroyList`. The
    // C++ frame-0 block is printed before the first update, which is why it still lists the two command centres
    // `startNewGame` replaced (HARNESS.md).
    let mut g = game(1);
    let hq = g.logic().objects().find(|o| o.template.name == "TestHQ").map(|o| o.id).expect("the HQ");
    let live = g.logic().objects().count();
    g.logic_mut().destroy_object(hq);
    assert_eq!(g.logic().objects().count(), live - 1, "hidden from queries");
    assert_eq!(g.logic().object_count(), live, "still on the object list");
    let text = block(&g, false);
    let header = text.lines().next().unwrap();
    assert!(header.ends_with(&format!("objs={live} players={}", g.logic().players().count())), "{header}");
    assert!(text.lines().any(|l| tag(l) == "O" && l.contains(&format!("id={} ", hq.0))), "the destroyed object has a record");
    // The end of the frame removes it, so the next block has neither the record nor the count.
    g.step();
    let after = block(&g, false);
    assert!(!after.lines().any(|l| tag(l) == "O" && l.contains(&format!("id={} ", hq.0))), "gone after the frame");
    assert_eq!(g.logic().object_count(), g.logic().objects().count());
}

#[test]
fn cadence_writes_each_due_frame_once() {
    let mut g = game(1);
    let mut snap = Snapshot::new(Vec::new(), 30, false);
    snapshot::run_frames(&mut g, 90, Some(&mut snap)).expect("write to memory");
    let text = String::from_utf8(snap.into_inner().expect("flush")).expect("ASCII");
    let frames: Vec<&str> = text.lines().filter(|l| tag(l) == "FRAME").map(|l| l.split(' ').nth(1).unwrap()).collect();
    assert_eq!(frames, ["f=0", "f=30", "f=60", "f=90"]);
}

#[test]
fn snapshot_leaves_the_crc_unchanged() {
    let mut a = game(1);
    let mut b = game(1);
    for g in [&mut a, &mut b] {
        let tank = g.spawned().iter().find(|s| s.template == "TestTank").expect("tank").clone();
        g.submit(tank.player, Command::MoveTo { ids: vec![tank.id], pos: Coord3D::new(600.0, 600.0, 0.0) });
    }
    // Every frame, with the bit fields: the heaviest writer.
    let mut snap = Snapshot::new(Vec::new(), 1, true);
    snapshot::run_frames(&mut a, 300, Some(&mut snap)).expect("write to memory");
    b.run_frames(300);
    assert_eq!(a.frame(), 300);
    assert_eq!(a.crc(), b.crc(), "the snapshot writer changed the simulation");
    let text = String::from_utf8(snap.into_inner().expect("flush")).expect("ASCII");
    assert_eq!(text.lines().filter(|l| tag(l) == "FRAME").count(), 301);
}

#[test]
fn production_and_team_queue_records() {
    let pr = ProductionState {
        queue: vec![ProductionEntry {
            kind: ProductionKind::Upgrade("Upgrade X".into()),
            quantity_total: 2,
            quantity_produced: 1,
            frames: 150,
            door: None,
            object_upgrade: false,
        }],
        ..Default::default()
    };
    let mut out = Vec::new();
    write_production(&mut out, 60, ObjectId(7), &pr).expect("write to memory");
    assert_eq!(
        String::from_utf8(out).expect("ASCII"),
        "SNAP f=60 Q obj=7 i=0 kind=upgrade tmpl=Upgrade%20X total=2 done=1 frames=150 door=-\n"
    );

    // `T key` is the prototype of the team the entry fills (`TeamInQueue::m_team`), not the port's
    // `proto#id` instance key (the C++ snapshot prints `m_team->getPrototype()->getName()`).
    let mut scripts = ScriptEngine::default();
    scripts.load_teams(&[zr_map::Dict(vec![
        ("teamName".to_owned(), zr_map::DictValue::Ascii("GLA Tunnel".to_owned())),
        ("teamOwner".to_owned(), zr_map::DictValue::Ascii("SkirmishGLA".to_owned())),
        ("teamUnitType1".to_owned(), zr_map::DictValue::Ascii("GLAInfantryRebel".to_owned())),
        ("teamUnitMaxCount1".to_owned(), zr_map::DictValue::Int(1)),
    ])]);
    let key = scripts.create_team("GLA Tunnel").expect("team instance");
    assert_eq!(key, "gla tunnel#1");
    let t = TeamInQueue {
        key,
        frame_started: 42,
        priority_build: true,
        reinforcement: false,
        reinforcement_id: None,
        sent_to_start_location: false,
        work_orders: vec![
            WorkOrder { thing: "GLAInfantryRebel".into(), factory: Some(ObjectId(9)), completed: 1, required_count: 3, required: true, resource_gatherer: false },
            WorkOrder { thing: "GLAVehicleTechnical".into(), factory: None, completed: 0, required_count: 2, required: false, resource_gatherer: false },
        ],
    };
    let mut out = Vec::new();
    write_team_queue(&mut out, 60, 1, "build", 0, &t, &scripts).expect("write to memory");
    assert_eq!(
        String::from_utf8(out).expect("ASCII"),
        "SNAP f=60 T p=1 q=build i=0 key=gla%20tunnel prio=1 started=42 reinf=0 recruit=0 sent=0 \
         work=GLAInfantryRebel:1/3:1:9,GLAVehicleTechnical:0/2:0:0\n"
    );
}

#[test]
fn escapes_percent_space_and_equals_with_uppercase_hex() {
    assert_eq!(esc("a b%c=d"), "a%20b%25c%3Dd");
    assert_eq!(esc("%20"), "%2520");
    assert!(matches!(esc("plain_Name#2"), Cow::Borrowed(_)));
}

#[test]
fn sabotage_gate_only_zeroes_the_printed_production() {
    assert_eq!(printed_production(7, false), 7);
    assert_eq!(printed_production(7, true), 0);
    assert_eq!(printed_production(0, true), 0);
}

#[test]
fn environment_settings() {
    assert_eq!(settings(None, None, None).unwrap(), None);
    assert_eq!(settings(Some(OsStr::new("")), Some("5"), None).unwrap(), None);
    let s = settings(Some(OsStr::new("x.snap")), None, None).unwrap().unwrap();
    assert_eq!((s.every, s.bits), (30, false));
    let s = settings(Some(OsStr::new("x.snap")), Some(" 10 "), Some("1")).unwrap().unwrap();
    assert_eq!((s.every, s.bits), (10, true));
    assert!(!settings(Some(OsStr::new("x.snap")), None, Some("yes")).unwrap().unwrap().bits);
    let zero = settings(Some(OsStr::new("x.snap")), Some("0"), None).unwrap_err();
    assert_eq!(zero.kind(), std::io::ErrorKind::InvalidInput);
    assert!(settings(Some(OsStr::new("x.snap")), Some("often"), None).is_err());
}

#[test]
fn chunked_run_writes_each_due_frame_once() {
    // Chunks of 20: the due frame 30 is mid-chunk. Chunks of 30: every due frame is a chunk boundary, so the next
    // call starts on a due frame that the previous call already wrote.
    for chunk in [20u32, 30] {
        let mut g = game(1);
        let mut snap = Snapshot::new(Vec::new(), 30, false);
        for _ in 0..(60 / chunk) {
            snapshot::run_frames(&mut g, chunk, Some(&mut snap)).expect("write to memory");
        }
        let text = String::from_utf8(snap.into_inner().expect("flush")).expect("ASCII");
        let frames: Vec<&str> = text.lines().filter(|l| tag(l) == "FRAME").map(|l| l.split(' ').nth(1).unwrap()).collect();
        assert_eq!(frames, ["f=0", "f=30", "f=60"], "chunks of {chunk}");
    }
}

#[test]
fn no_writer_is_the_plain_run() {
    let mut a = game(1);
    let mut b = game(1);
    snapshot::run_frames::<Vec<u8>>(&mut a, 90, None).expect("no output");
    b.run_frames(90);
    assert_eq!(a.frame(), 90);
    assert_eq!(a.crc(), b.crc());
}

#[test]
fn same_seed_gives_byte_identical_snapshots() {
    let run = || {
        let mut g = game(7);
        let mut snap = Snapshot::new(Vec::new(), 10, true);
        snapshot::run_frames(&mut g, 100, Some(&mut snap)).expect("write to memory");
        snap.into_inner().expect("flush")
    };
    let (a, b) = (run(), run());
    assert!(!a.is_empty());
    assert_eq!(a, b);
}

#[test]
fn script_team_record_sorts_members_and_lowercases_proto() {
    let mut out = Vec::new();
    write_script_team(&mut out, 60, 4, "Usa Team", &[ObjectId(9), ObjectId(3)]).expect("write to memory");
    assert_eq!(String::from_utf8(out).expect("ASCII"), "SNAP f=60 S id=4 proto=usa%20team members=3,9\n");
    let mut empty = Vec::new();
    write_script_team(&mut empty, 60, 5, "gla", &[]).expect("write to memory");
    assert_eq!(String::from_utf8(empty).expect("ASCII"), "SNAP f=60 S id=5 proto=gla members=\n");
}

/// The `O` record's `ai=` is the C++ `getCurrentStateID()` (`aiStateText`, GameLogicSnapshot.cpp:263-285): an attack
/// inside an attack-move, hunt, attack-area or attack-follow prints the enclosing state (AIStates.cpp:1999-2006,
/// :7122, :7329); a direct attack and the plain states print their own name. Revert check: make `shown_state`
/// return `m.current` and the first assertion fails (the port printed `AttackObject` where C++ prints `AttackMoveTo`).
#[test]
fn ai_field_prints_the_enclosing_attack_state_like_the_cpp_snapshot() {
    use zr_sim::ai_states::{AiMachine, AiStateType};
    let state = |current, attack_return| {
        let m = AiMachine { current, attack_return, ..AiMachine::default() };
        format!("{:?}", snapshot::shown_state(&m))
    };
    assert_eq!(state(AiStateType::AttackObject, AiStateType::AttackMoveTo), "AttackMoveTo");
    assert_eq!(state(AiStateType::AttackPosition, AiStateType::AttackMoveTo), "AttackMoveTo");
    assert_eq!(state(AiStateType::ForceAttackObject, AiStateType::Hunt), "Hunt");
    assert_eq!(state(AiStateType::AttackAndFollowObject, AiStateType::AttackArea), "AttackArea");
    assert_eq!(state(AiStateType::AttackObject, AiStateType::AttackFollowWaypointPath), "AttackFollowWaypointPath");
    // A direct attack order (`attack_return == Idle`) keeps its own name, and so does an attack-move that is moving.
    assert_eq!(state(AiStateType::AttackObject, AiStateType::Idle), "AttackObject");
    assert_eq!(state(AiStateType::AttackMoveTo, AiStateType::Idle), "AttackMoveTo");
    assert_eq!(state(AiStateType::Idle, AiStateType::Idle), "Idle");
}

/// The `atk` field (`victimText`, GameLogicSnapshot.cpp:292-309) tests `getCurrentStateID() == AI_ATTACK_POSITION`, the
/// enclosing state. An attack on a position inside an attack-move prints `0` (the attack-move is the state), not
/// `ground`. Revert check: make `victim_text` test `m.current` instead of `shown_state`; the second `0` assertion then
/// prints `ground` and the test fails.
#[test]
fn atk_field_tests_the_enclosing_attack_position_state_like_the_cpp_snapshot() {
    use zr_sim::ai_states::{AiMachine, AiStateType};
    let machine = |current, attack_return, victim: Option<u32>| {
        let current_victim = victim.map_or(ObjectId::INVALID, ObjectId);
        AiMachine { current, attack_return, current_victim, ..AiMachine::default() }
    };
    // A direct attack on a position prints `ground`; without a running attack it prints `0`.
    assert_eq!(snapshot::victim_text(&machine(AiStateType::AttackPosition, AiStateType::Idle, None), true), "ground");
    assert_eq!(snapshot::victim_text(&machine(AiStateType::AttackPosition, AiStateType::Idle, None), false), "0");
    // Inside an attack-move the enclosing state is AttackMoveTo, so no `ground`.
    assert_eq!(snapshot::victim_text(&machine(AiStateType::AttackPosition, AiStateType::AttackMoveTo, None), true), "0");
    // A current victim id wins over both.
    assert_eq!(snapshot::victim_text(&machine(AiStateType::AttackObject, AiStateType::AttackMoveTo, Some(42)), true), "42");
}
