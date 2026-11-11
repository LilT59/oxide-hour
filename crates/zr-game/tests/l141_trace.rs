//! Per-object trace writer tests (`docs/HARNESS.md`, "Per-object trace (`ZR_TRACE`)"): synthetic INI
//! and map, no game install needed.

use std::sync::Arc;
use zr_game::trace::{self, FrameRange, IdFilter, Trace};
use zr_game::{Game, GameData, PlayerSetup};
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
  Behavior = PhysicsBehavior ModuleTag_Phys
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

/// The `TRACE` lines of one frame block written by a fresh writer with `ids`/`frames` filters.
fn traced(g: &mut Game, ids: IdFilter, frames: FrameRange, n: u32) -> String {
    let mut tr = Trace::new(Vec::new(), ids, frames);
    trace::run_frames::<Vec<u8>, Vec<u8>>(g, n, None, Some(&mut tr)).expect("write to memory");
    String::from_utf8(tr.into_inner().expect("flush")).expect("ASCII records")
}

/// The field names of a line, in order.
fn keys(line: &str) -> Vec<&str> {
    line.split(' ').skip(2).map(|t| t.split('=').next().unwrap_or("")).collect()
}

fn field<'a>(line: &'a str, name: &str) -> &'a str {
    let prefix = format!(" {name}=");
    let start = line.find(&prefix).unwrap_or_else(|| panic!("{name} in {line}")) + prefix.len();
    let rest = &line[start..];
    rest.split(' ').next().unwrap_or("")
}

#[test]
fn one_line_per_object_with_the_documented_fields() {
    let mut g = game(1);
    let text = traced(&mut g, IdFilter::All, FrameRange::ALL, 0);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), g.logic().object_count(), "one line per object");
    let expected = [
        "id", "tmpl", "x", "y", "z", "xb", "yb", "zb", "ang", "angb", "vx", "vy", "vz", "vxb", "vyb", "vzb", "vmag",
        "pfall", "pffree", "pfstun", "pfcoll", "pfturn", "lbrake", "lbf", "ldonut", "ai", "aisleep", "aiwake", "aiphase",
        "sub", "goaltype", "gx", "pcount", "pfirst", "plast", "pnext", "victim", "wslot", "wstat", "wnext", "dtask",
        "dbuild", "dact", "ssub", "cont",
    ];
    for line in &lines {
        assert!(line.starts_with("TRACE f=0 id="), "{line}");
        assert_eq!(keys(line), expected, "{line}");
        // Ids ascend, like the C++ writer's sort.
        assert_eq!(field(line, "f"), "0", "{line}");
    }
    let ids: Vec<u32> = lines.iter().map(|l| field(l, "id").parse().unwrap()).collect();
    assert!(ids.windows(2).all(|w| w[0] < w[1]), "id order: {ids:?}");

    let tank = lines.iter().find(|l| l.contains("tmpl=TestTank")).expect("a starting tank");
    assert_eq!(field(tank, "ai"), "Idle");
    assert_eq!(field(tank, "goaltype"), "None");
    assert_eq!(field(tank, "pcount"), "0");
    assert_eq!(field(tank, "pfirst"), "-");
    assert_eq!(field(tank, "cont"), "0");
    assert_eq!(field(tank, "aiphase"), "-", "the port has no heap phase for the AI pass");
    // The tank has a physics module (zeros at frame 0) and no weapon.
    assert_eq!(field(tank, "vx"), "0.0000", "{tank}");
    assert_eq!(field(tank, "pfall"), "0");
    assert_eq!(field(tank, "pfturn"), "0");
    assert_eq!(field(tank, "wslot"), "-");
    assert_eq!(field(tank, "dtask"), "-", "the port does not model m_currentTask");

    // No body/AI/physics: every such field is `-` on the port.
    let rock = lines.iter().find(|l| l.contains("tmpl=TestRock")).expect("the rock");
    for name in ["vx", "vy", "vz", "vmag", "pfall", "pffree", "pfstun", "pfcoll", "pfturn", "lbrake", "lbf", "ldonut", "ai", "aisleep", "aiwake", "sub", "goaltype", "gx", "wstat", "wnext", "dact", "ssub"] {
        assert_eq!(field(rock, name), "-", "{name}: {rock}");
    }
    assert_eq!(field(rock, "ang"), "0.5000", "the map object's angle: {rock}");
    assert_eq!(field(rock, "angb"), format!("{:08x}", 0.5f32.to_bits()).as_str(), "{rock}");

    // The position/bit fields are the object's own values.
    let hq = lines.iter().find(|l| l.contains("tmpl=TestHQ")).expect("the HQ");
    let o = g.logic().objects().find(|o| o.template.name == "TestHQ").expect("HQ object");
    assert_eq!(field(hq, "x"), format!("{:.4}", o.position.x).as_str(), "{hq}");
    assert_eq!(field(hq, "xb"), format!("{:08x}", o.position.x.to_bits()).as_str(), "{hq}");
}

#[test]
fn trace_writes_only_due_frames_and_ids_once() {
    let mut g = game(1);
    let tank = g.spawned().iter().find(|s| s.template == "TestTank").expect("tank").id.0;
    let mut tr = Trace::new(Vec::new(), IdFilter::Ids(vec![tank]), FrameRange { first: 5, last: 10 });
    // Chunks that cross the range and repeat a due frame at a chunk boundary.
    for chunk in [3u32, 3, 4, 4] {
        trace::run_frames::<Vec<u8>, Vec<u8>>(&mut g, chunk, None, Some(&mut tr)).expect("write to memory");
    }
    let text = String::from_utf8(tr.into_inner().expect("flush")).expect("ASCII");
    let frames: Vec<&str> = text.lines().map(|l| field(l, "f")).collect();
    assert_eq!(frames, ["5", "6", "7", "8", "9", "10"], "only the due frames, once each: {text}");
    for line in text.lines() {
        assert_eq!(field(line, "id"), tank.to_string(), "only the filtered object: {line}");
    }
}

#[test]
fn trace_leaves_the_crc_unchanged() {
    let mut a = game(1);
    let mut b = game(1);
    for g in [&mut a, &mut b] {
        let tank = g.spawned().iter().find(|s| s.template == "TestTank").expect("tank").clone();
        g.submit(tank.player, Command::MoveTo { ids: vec![tank.id], pos: Coord3D::new(600.0, 600.0, 0.0) });
    }
    // Every frame, every object: the heaviest trace.
    let mut tr = Trace::new(Vec::new(), IdFilter::All, FrameRange::ALL);
    trace::run_frames::<Vec<u8>, Vec<u8>>(&mut a, 300, None, Some(&mut tr)).expect("write to memory");
    b.run_frames(300);
    assert_eq!(a.frame(), 300);
    assert_eq!(a.crc(), b.crc(), "the trace writer changed the simulation");
    let text = String::from_utf8(tr.into_inner().expect("flush")).expect("ASCII");
    assert_eq!(text.lines().filter(|l| l.starts_with("TRACE f=0 ")).count(), a.logic().object_count());
    assert_eq!(text.lines().filter(|l| l.starts_with("TRACE f=300 ")).count(), a.logic().object_count());
}

#[test]
fn same_seed_gives_a_byte_identical_trace() {
    let run = || {
        let mut g = game(7);
        let tank = g.spawned().iter().find(|s| s.template == "TestTank").expect("tank").clone();
        g.submit(tank.player, Command::MoveTo { ids: vec![tank.id], pos: Coord3D::new(600.0, 600.0, 0.0) });
        traced(&mut g, IdFilter::All, FrameRange::ALL, 100)
    };
    let (a, b) = (run(), run());
    assert!(!a.is_empty());
    assert_eq!(a, b);
}

#[test]
fn a_moving_tank_traces_its_path_and_heading() {
    let mut g = game(1);
    let tank = g.spawned().iter().find(|s| s.template == "TestTank").expect("tank").clone();
    g.submit(tank.player, Command::MoveTo { ids: vec![tank.id], pos: Coord3D::new(600.0, 600.0, 0.0) });
    let text = traced(&mut g, IdFilter::Ids(vec![tank.id.0]), FrameRange { first: 5, last: 5 }, 5);
    let line = text.lines().next().expect("one line for the tank");
    assert_eq!(field(line, "id"), tank.id.0.to_string());
    let o = g.logic().objects().find(|o| o.id == tank.id).expect("tank");
    assert_eq!(field(line, "ang"), format!("{:.4}", o.angle).as_str(), "{line}");
    // The move asked for a path: its first node is the tank's own cell, and the last is the goal.
    assert!(field(line, "pcount").parse::<u32>().unwrap() >= 1, "{line}");
    let goal = field(line, "plast");
    assert!(goal.contains(','), "{line}");
}

// ---- lane 149: same-quantity field parity (R14) ------------------------------------------------

/// The C++ writer flushes after every block (`GameLogicTrace.cpp:720`); the port must too, so a live
/// or killed run leaves the same complete frames. Revert check: dropping the `flush` in
/// `Trace::write_if_due` leaves only `into_inner`'s flush and fails the count.
#[test]
fn every_traced_frame_is_flushed() {
    struct Counting {
        flushes: usize,
        data: Vec<u8>,
    }
    impl std::io::Write for Counting {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.data.extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.flushes += 1;
            Ok(())
        }
    }
    let mut g = game(1);
    let mut tr = Trace::new(Counting { flushes: 0, data: Vec::new() }, IdFilter::All, FrameRange { first: 0, last: 1 });
    trace::run_frames::<Vec<u8>, Counting>(&mut g, 1, None, Some(&mut tr)).expect("write to memory");
    let out = tr.into_inner().expect("flush");
    assert_eq!(out.flushes, 3, "one flush after frame 0, one after frame 1, one in into_inner");
    assert!(!out.data.is_empty());
}

/// A sim object with no game data: optional weapons, AI, physics and locomotor, plus a body.
fn parity_unit(weapons: Option<[Option<std::sync::Arc<zr_sim::weapon::WeaponTemplate>>; 3]>, ai: bool, physics: bool, locomotor: bool) -> (zr_sim::GameLogic, zr_sim::ObjectId) {
    use std::sync::Arc;
    let mut g = zr_sim::GameLogic::new(zr_sim::TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 1000);
    g.add_player(1, 1000);
    let id = g.create_object(&Arc::new(parity_template("ParityUnit", weapons, ai, physics, locomotor)), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    (g, id)
}

fn parity_template(
    name: &str,
    weapons: Option<[Option<std::sync::Arc<zr_sim::weapon::WeaponTemplate>>; 3]>,
    ai: bool,
    physics: bool,
    locomotor: bool,
) -> zr_sim::ObjectTemplate {
    let mut t = zr_sim::ObjectTemplate::new(name);
    t.max_health = 200.0;
    t.initial_health = 200.0;
    t.geometry.major_radius = 10.0;
    if ai {
        t.ai_update = Some(0);
    }
    if physics {
        t.physics = Some(zr_sim::physics::PhysicsTemplate::default());
    }
    if locomotor {
        t.locomotor = Some(zr_sim::template::LocomotorParams::from_engine_units(
            zr_sim::template::LocomotorKind::Treads,
            1.0,
            0.0,
            1.1,
            1.1,
            0.1,
            true,
        ));
    }
    if let Some(weapons) = weapons {
        t.combat = zr_sim::weapon::CombatTemplate { weapon_set: Some(zr_sim::weapon::WeaponSetTemplate { weapons }), ..Default::default() };
    }
    if ai {
        t.combat.auto_acquire_enemies = true;
    }
    t
}

fn parity_gun(name: &str) -> std::sync::Arc<zr_sim::weapon::WeaponTemplate> {
    let mut w = zr_sim::weapon::WeaponTemplate::new(name);
    w.primary_damage = 10.0;
    w.attack_range = 100.0;
    std::sync::Arc::new(w)
}

fn parity_line(g: &zr_sim::GameLogic, id: zr_sim::ObjectId) -> String {
    zr_game::trace::object_line(g.frame(), g.find_object(id).expect("unit"))
}

/// `AI_ATTACK_POSITION` with a closing phase prints `Pursue`: the C++ attack machine's `CHASE_TARGET`
/// state is `AIAttackApproachTargetState` for a position (`AIStates.cpp:367-376`), and `attackSubName`
/// prints that state id as `Pursue` (`GameLogicTrace.cpp:356`).
#[test]
fn an_attack_on_a_position_prints_pursue_while_closing() {
    let (mut g, id) = parity_unit(Some([Some(parity_gun("Primary")), None, None]), true, false, true);
    // A far-away ground target: out of the weapon's range, so the attack has to close.
    g.submit(0, Command::AttackGround { ids: vec![id], pos: Coord3D::new(900.0, 900.0, 0.0) });
    let mut seen = false;
    for _ in 0..10 {
        g.update();
        let o = g.find_object(id).expect("unit");
        if o.ai.machine.current == zr_sim::ai_states::AiStateType::AttackPosition {
            seen = true;
            let phase = o.combat.attack.as_ref().expect("an attack order").phase;
            if phase == zr_sim::combat::AttackPhase::Approach {
                assert_eq!(field(&parity_line(&g, id), "sub"), "Pursue", "closing on a position");
            }
        }
    }
    assert!(seen, "AttackGround must enter AI_ATTACK_POSITION");
}

/// `AI_ATTACK_MOVE_TO`/`AI_HUNT`/`AI_ATTACK_AREA` print no sub-state: their immediate sub-machine is
/// the attack-move/hunt/area machine, not the attack machine, and the C++ writer names only the four
/// `AI_ATTACK_*` states (`GameLogicTrace.cpp:443-466`). The unit really has an attack order in these
/// states (the enemy is in range), so a port that printed the inner attack phase would be caught.
#[test]
fn attack_move_hunt_and_attack_area_print_no_substate() {
    use std::sync::Arc;
    use zr_sim::ai_states::AiStateType;
    let enemy = Arc::new(parity_template("Enemy", None, true, false, true));
    for (setup, state) in [
        (
            (|g: &mut zr_sim::GameLogic, id| g.submit(0, Command::AttackMove { ids: vec![id], pos: Coord3D::new(350.0, 100.0, 0.0) })) as fn(&mut zr_sim::GameLogic, zr_sim::ObjectId),
            AiStateType::AttackMoveTo,
        ),
        ((|g: &mut zr_sim::GameLogic, id| g.command_hunt(0, &[id])) as fn(&mut zr_sim::GameLogic, zr_sim::ObjectId), AiStateType::Hunt),
        (
            (|g: &mut zr_sim::GameLogic, id| g.command_attack_area(0, &[id], Coord3D::new(140.0, 110.0, 0.0), 100.0)) as fn(&mut zr_sim::GameLogic, zr_sim::ObjectId),
            AiStateType::AttackArea,
        ),
    ] {
        let (mut g, id) = parity_unit(Some([Some(parity_gun("Primary")), None, None]), true, false, true);
        g.create_object(&enemy, Coord3D::new(130.0, 100.0, 0.0), 0.0, 1);
        g.set_enemies(0, 1);
        g.reveal_map_permanently_for_player(0);
        setup(&mut g, id);
        let mut with_order = 0;
        for _ in 0..200 {
            g.update();
            let o = g.find_object(id).expect("unit");
            if o.combat.attack.is_some() {
                with_order += 1;
                assert!(
                    matches!(
                        o.ai.machine.current,
                        AiStateType::AttackObject | AiStateType::AttackPosition | AiStateType::ForceAttackObject | AiStateType::AttackAndFollowObject
                    ),
                    "{state:?}: the attack runs as the port's main state: {:?}",
                    o.ai.machine.current
                );
                let line = parity_line(&g, id);
                assert_eq!(field(&line, "ai"), format!("{state:?}").as_str(), "{state:?}: the enclosing AI state is the name");
                assert_eq!(field(&line, "sub"), "-", "{state:?} with an attack order");
            }
        }
        assert!(with_order > 0, "{state:?} must acquire an attack order");
    }
}

/// `Object::getCurrentWeapon(&slot)` returns null when the current slot is empty (`Object.cpp:1428`),
/// and the C++ trace prints `wslot=- wstat=- wnext=-`; the port used to print the empty slot number.
#[test]
fn an_empty_current_weapon_slot_prints_a_dash() {
    let (g, id) = parity_unit(Some([None, Some(parity_gun("Secondary")), None]), true, false, false);
    let line = parity_line(&g, id);
    assert_eq!(g.find_object(id).expect("unit").combat.current_weapon_slot, 0, "PRIMARY is the current slot");
    assert_eq!(field(&line, "wslot"), "-", "{line}");
    assert_eq!(field(&line, "wstat"), "-", "{line}");
    assert_eq!(field(&line, "wnext"), "-", "{line}");
}

/// `lbrake`/`lbf`/`ldonut` need `getAIUpdateInterface() && getCurLocomotor()` (`GameLogicTrace.cpp:559-573`);
/// a physics module and a locomotor template alone are not enough.
#[test]
fn the_locomotor_flags_need_an_ai_update() {
    // Physics and a locomotor, but no AIUpdateInterface: `-` on every locomotor field.
    let (g, id) = parity_unit(None, false, true, true);
    let line = parity_line(&g, id);
    assert_eq!(field(&line, "lbrake"), "-", "{line}");
    assert_eq!(field(&line, "lbf"), "-", "{line}");
    assert_eq!(field(&line, "ldonut"), "-", "{line}");
    // With an AIUpdateInterface the values print.
    let (g, id) = parity_unit(None, true, true, true);
    let line = parity_line(&g, id);
    assert_eq!(field(&line, "lbrake"), "0", "{line}");
    assert_eq!(field(&line, "lbf"), "1.0000", "{line}");
    assert_ne!(field(&line, "ldonut"), "-", "the donut timer prints with a current locomotor: {line}");
}
