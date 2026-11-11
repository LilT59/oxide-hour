//! Tests on synthetic INI + map data (no game install needed).

use std::sync::Arc;
use zr_game::{convert_locomotor, kind_of_bits, Game, GameData, PlayerKind, PlayerSetup};
use zr_ini::IniDb;
use zr_map::{Dict, DictValue, HeightMap, MapFile, MapObject, PolygonTrigger, PolygonTriggers, Side, Sides};
use zr_sim::template::{kind_of, LocomotorKind};
use zr_sim::math::LOGICFRAMES_PER_SECOND;
use zr_sim::{Command, Coord3D, TemplateSource};

const INI: &str = "
Locomotor TankLoco
  Surfaces = GROUND
  Speed = 30
  TurnRate = 180
  Acceleration = 1000
  Braking = 1000
  Appearance = TREADS
End
Locomotor PlaneLoco
  Surfaces = AIR
  Speed = 150
  TurnRate = 90
  Acceleration = 300
  Braking = 100
  Appearance = WINGS
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
Object TestPlane
  KindOf = AIRCRAFT SELECTABLE
  Behavior = AIUpdateInterface ModuleTag_AI
  End
  Locomotor = SET_NORMAL PlaneLoco
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
Object TestFluff
  KindOf = IMMOBILE CLEARED_BY_BUILD
  Body = ActiveBody ModuleTag_Body
    MaxHealth = 1.0
    InitialHealth = 1.0
  End
End
Object TestFence
  KindOf = IMMOBILE CLEARED_BY_BUILD
  FenceWidth = 2
  Body = ActiveBody ModuleTag_Body
    MaxHealth = 100.0
    InitialHealth = 100.0
  End
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

fn obj(template: &str, x: f32, y: f32, owner: &str, extra: Vec<(String, DictValue)>) -> MapObject {
    let mut props = vec![("originalOwner".to_owned(), ascii(owner))];
    props.extend(extra);
    MapObject { version: 3, pos: [x, y, 0.0], angle: 0.5, flags: 0, template: template.to_owned(), props: Dict(props) }
}

fn waypoint(name: &str, x: f32, y: f32) -> MapObject {
    obj("*Waypoints/Waypoint", x, y, "team", vec![("waypointName".into(), ascii(name)), ("waypointID".into(), DictValue::Int(1))])
}

fn map() -> MapFile {
    let mut m = MapFile::default();
    let (w, h) = (100usize, 100usize);
    m.height_map = Some(HeightMap { version: 4, width: w as i32, height: h as i32, border_size: 0, boundaries: vec![], data: vec![16; w * h] });
    m.sides = Some(Sides {
        version: 3,
        sides: vec![
            side("", ""),
            side("PlyrCivilian", "FactionCivilian"),
            side("SkirmishAmerica", "FactionAmerica"), // dropped by prepareForMP_or_Skirmish
        ],
        teams: vec![],
        nested: vec![],
        scripts: None,
        script_error: None,
    });
    let mut road = obj("TestRock", 50.0, 50.0, "team", vec![]);
    road.flags = 0x2; // FLAG_ROAD_POINT1: terrain-side, never a game object
    m.objects = vec![
        waypoint("Player_1_Start", 300.0, 300.0),
        waypoint("Player_2_Start", 700.0, 700.0),
        obj("TestRock", 100.0, 100.0, "team", vec![]),
        obj("TestRock", 110.0, 100.0, "teamPlyrCivilian", vec![]),
        obj("TestRock", 120.0, 100.0, "teamSkirmishAmerica", vec![]), // map team no longer exists -> neutral
        obj("TestRock", 130.0, 100.0, "teamplayer0", vec![]),         // resolves to the first skirmish slot
        obj("NoSuchTemplate", 140.0, 100.0, "team", vec![]),
        road,
    ];
    m
}

fn two_players() -> [PlayerSetup; 2] {
    [PlayerSetup::new("FactionTest", 0), PlayerSetup::new("FactionRich", 1)]
}

#[test]
fn no_double_unit_conversion() {
    let d = data();
    let t = d.store.find_template("TestTank").expect("template");
    let l = t.locomotor.expect("SET_NORMAL locomotor");
    // Speed = 30 /s is 1.0 per frame exactly once; TurnRate = 180 deg/s is pi/30 rad per frame.
    assert_eq!(l.max_speed, 1.0);
    assert!((l.max_turn_rate - 0.10472).abs() < 1e-5, "{}", l.max_turn_rate);
    assert!((l.acceleration - 1000.0 / 900.0).abs() < 1e-5);
    assert_eq!(l.kind, LocomotorKind::Treads);
    // `Locomotor.cpp:946` pushes the INI value only; this fixture sets no `StickToGround`, so the default (false,
    // `Locomotor.cpp:334`) stands even for a ground appearance.
    assert!(!l.stick_to_ground);
    // The wrong path (feeding converted values to `from_ini`) would give a 30x smaller speed.
    let wrong = zr_sim::template::LocomotorParams::from_ini(LocomotorKind::Treads, 1.0, 0.0, 0.0, 0.10472);
    assert!(wrong.max_speed < l.max_speed / 20.0);
    // Same value straight from the template layer.
    let raw = d.templates().locomotor("TankLoco").unwrap();
    assert_eq!(convert_locomotor(raw), l);
}

#[test]
fn body_geometry_vision_and_kind_of() {
    let d = data();
    let t = d.store.find_template("TestTank").unwrap();
    assert_eq!((t.max_health, t.initial_health), (480.0, 400.0));
    assert_eq!(t.vision_range, 150.0);
    assert_eq!((t.geometry.major_radius, t.geometry.minor_radius, t.geometry.height), (15.0, 10.0, 10.0));
    // Bits are mapped by name, not copied positionally.
    let thing = d.templates().thing("TestTank").unwrap();
    assert_eq!(kind_of_bits(thing), kind_of::SELECTABLE | kind_of::VEHICLE | kind_of::CAN_ATTACK);
    assert_eq!(t.kind_of, kind_of_bits(thing));
    let hq = d.store.find_template("TestHQ").unwrap();
    assert_eq!(hq.kind_of, kind_of::STRUCTURE | kind_of::SELECTABLE | kind_of::IMMOBILE);
    assert!(hq.locomotor.is_none());
    // Aircraft are not glued to the ground.
    let plane = d.store.find_template("TestPlane").unwrap();
    assert!(!plane.locomotor.unwrap().stick_to_ground);
    assert_eq!(plane.max_health, 0.0, "no body module -> no sim body");
    // Cached Arc, case-sensitive lookup.
    assert!(Arc::ptr_eq(&t, &d.store.find_template("TestTank").unwrap()));
    assert!(d.store.find_template("testtank").is_none());
}

#[test]
fn player_list_money_and_owner_mapping() {
    let g = Game::from_map(data(), &map(), "synthetic", &two_players(), 7).unwrap();
    let kinds: Vec<_> = g.players().iter().map(|p| (p.index, p.kind)).collect();
    assert_eq!(
        kinds,
        vec![(0, PlayerKind::Neutral), (1, PlayerKind::Civilian), (2, PlayerKind::Skirmish), (3, PlayerKind::Skirmish), (4, PlayerKind::Observer)]
    );
    // StartMoney 0 -> the Default=Yes skirmish choice; a non-zero StartMoney wins.
    assert_eq!(g.player(2).unwrap().money, 10000);
    assert_eq!(g.player(3).unwrap().money, 777);
    // `Player::init` (Player.cpp:427-443) gives *every* player with a template the starting cash:
    // the civilian and the replay-observer player have 10000 too (the AN3 snapshot's P p=1/p=4).
    assert_eq!(g.player(1).unwrap().money, 10000, "the civilian player is a normal player to Player::init");
    assert_eq!(g.player(4).unwrap().money, 10000, "the replay observer is a normal player to Player::init");
    // Rocks: "team" -> neutral, teamPlyrCivilian -> civilian, unknown map team -> neutral, teamplayer0 -> slot 0.
    let rocks: Vec<_> = g.objects().filter(|o| o.template == "TestRock").map(|o| o.owner).collect();
    assert_eq!(rocks, vec![0, 1, 0, 2]);
}

#[test]
fn map_object_filtering_and_ground_snap() {
    let g = Game::from_map(data(), &map(), "synthetic", &two_players(), 7).unwrap();
    let s = g.map_load_stats();
    assert_eq!((s.waypoints, s.road_or_bridge_points, s.no_template, s.created), (2, 1, 1, 4));
    assert_eq!(g.waypoint("Player_2_Start").unwrap().pos.x, 700.0);
    // Height sample 16 -> 16 * MAP_HEIGHT_SCALE (0.625) = 10.
    let rock = g.objects().find(|o| o.template == "TestRock").unwrap();
    assert!((rock.position.z - 10.0).abs() < 1e-4, "{}", rock.position.z);
    assert!((g.waypoint("Player_1_Start").unwrap().pos.z - 10.0).abs() < 1e-4);
    assert!((rock.angle - 0.5).abs() < 1e-6);
}

#[test]
fn multiplayer_map_load_turns_cleared_by_build_fluff_into_client_props() {
    // `GameLogic.cpp:1909-1922` with `forceFluffToProp` (multiplayer, `:1840-1843`):
    // `CLEARED_BY_BUILD` objects without a fence width are client props, not game objects.
    let mut m = map();
    m.objects.push(obj("TestFluff", 200.0, 200.0, "team", vec![]));
    m.objects.push(obj("TestFence", 220.0, 200.0, "team", vec![]));
    let g = Game::from_map(data(), &m, "synthetic", &two_players(), 7).unwrap();
    assert!(g.objects().all(|o| o.template != "TestFluff"), "fluff must not be a logic object");
    assert!(g.objects().any(|o| o.template == "TestFence"), "a fence (FenceWidth > 0) is a game object");
    assert_eq!(g.map_load_stats().props_or_trees_skipped, 1);
}

#[test]
fn starting_sets_follow_the_player_template() {
    let g = Game::from_map(data(), &map(), "synthetic", &two_players(), 7).unwrap();
    let sp = g.spawned();
    // player0: building + two units; player1: building only.
    let roles: Vec<_> = sp.iter().map(|s| (s.player, s.role.as_str(), s.template.as_str())).collect();
    assert_eq!(
        roles,
        vec![
            (2, "StartingBuilding", "TestHQ"),
            (2, "StartingUnit0", "TestTank"),
            (2, "StartingUnit1", "TestTank"),
            (3, "StartingBuilding", "TestHQ"),
        ]
    );
    let hq = g.object(sp[0].id).unwrap();
    assert_eq!((hq.position.x, hq.position.y), (300.0, 300.0));
    assert!((hq.angle - std::f32::consts::FRAC_PI_2).abs() < 1e-5, "PlacementViewAngle in radians, got {}", hq.angle);
    assert_eq!(hq.health, 1000.0);
    // Units ring the building at 0.7R..1.3R of its bounding sphere, offset by the y shift of R/2.
    let r = g.data().templates().thing("TestHQ").unwrap().geometry.bounding_sphere_radius;
    let c = Coord3D::new(300.0, 300.0 - r / 2.0, 0.0);
    let (u0, u1) = (g.object(sp[1].id).unwrap(), g.object(sp[2].id).unwrap());
    for u in [u0, u1] {
        let d = u.position.dist_2d(&c);
        assert!(d >= r * 0.7 - 1e-3 && d <= r * 1.3 + 5.0, "unit at distance {d} (R = {r})");
    }
    // They do not overlap each other (radius-5 sphere test against existing objects).
    assert!(u0.position.dist_2d(&u1.position) > 5.0);
}

#[test]
fn missing_start_waypoint_is_an_error() {
    let mut m = map();
    m.objects.retain(|o| o.props.str("waypointName") != Some("Player_2_Start"));
    assert!(Game::from_map(data(), &m, "synthetic", &two_players(), 7).is_err());
    let bad = [PlayerSetup::new("FactionNope", 0)];
    assert!(Game::from_map(data(), &map(), "synthetic", &bad, 7).is_err());
}

fn run(seed: u32, frames: u32) -> (u32, Vec<u32>) {
    let mut g = Game::from_map(data(), &map(), "synthetic", &two_players(), seed).unwrap();
    let tank = g.spawned().iter().find(|s| s.template == "TestTank").unwrap().clone();
    g.submit(tank.player, Command::MoveTo { ids: vec![tank.id], pos: Coord3D::new(600.0, 600.0, 0.0) });
    let mut crcs = Vec::new();
    for _ in 0..frames {
        g.step();
        if g.frame() % 50 == 0 {
            crcs.push(g.crc());
        }
    }
    (g.crc(), crcs)
}

#[test]
fn deterministic_start_same_seed_same_crc() {
    let (a, ta) = run(1, 400);
    let (b, tb) = run(1, 400);
    assert_eq!(a, b);
    assert_eq!(ta, tb, "CRC trace diverged");
    // A different seed changes the start-angle of the unit ring, hence the state.
    let c = Game::from_map(data(), &map(), "synthetic", &two_players(), 2).unwrap().crc();
    let d = Game::from_map(data(), &map(), "synthetic", &two_players(), 1).unwrap().crc();
    assert_ne!(c, d);
}

#[test]
fn ordered_unit_moves_toward_the_goal() {
    let mut g = Game::from_map(data(), &map(), "synthetic", &two_players(), 1).unwrap();
    let s = g.spawned().iter().find(|s| s.template == "TestTank").unwrap().clone();
    let goal = Coord3D::new(600.0, 600.0, 0.0);
    g.submit(s.player, Command::MoveTo { ids: vec![s.id], pos: goal });
    // The start spot is `tryPosition`'s ring spot, then `placeObjectAtPosition`'s `adjustDestination` moves the tank to
    // its cell centre (GameLogic.cpp:520-527), (245, 285). The HQ footprint (an 80-unit box, half-size 40) takes
    // `REAL_TO_INT_CEIL(2*40/5)` = 17 steps in the retail build (`fast_float_ceil` of 16, BaseFunctions.h:81-88;
    // AIPathfind.cpp:4194), one more sample than `ceil`, which moves the route's first corner one cell north: the
    // first waypoint is (245, 355), 70 units north (the old footprint, with `ceil`, gave (245, 345) and frame 67).
    g.run_frames(1);
    let first = g.logic().find_object(s.id).unwrap().ai.path.first().copied().expect("the move has a path");
    let mut arrived = None;
    let mut prev_queue = 0;
    for _ in 0..90 {
        g.run_frames(1);
        let l = g.logic();
        let o = l.find_object(s.id).unwrap();
        // A path computed within the last 3 frames makes the next request wait one second (`requestPath`,
        // AIUpdate.cpp:499-503). The queue is set during the frame before this observation. This layout
        // (clear first leg, see above) need not queue a repath; when one is queued, the delay must hold.
        if prev_queue == 0 && o.ai.queue_for_path_frame != 0 {
            assert_eq!(o.ai.queue_for_path_frame, l.frame() - 1 + LOGICFRAMES_PER_SECOND, "queued repath delay");
        }
        prev_queue = o.ai.queue_for_path_frame;
        if arrived.is_none() && o.position.dist_2d(&first) < 2.0 {
            arrived = Some(l.frame());
        }
    }
    let arrived = arrived.expect("the tank reaches its first waypoint");
    // Measured in this tree: the first waypoint is (245, 355) and the tank reaches it at frame 77 (the arrival test is
    // `dist_2d < 2.0`, see the loop above). The bound is the measured frame +-2, not a derivation.
    assert!((first.x - 245.0).abs() < 1e-3 && (first.y - 355.0).abs() < 1e-3, "first waypoint {first:?}");
    assert!((75..=79).contains(&arrived), "measured frame 77 (+-2), reached at frame {arrived}");
}

/// `AIUpdateInterface::requestPath` (AIUpdate.cpp:499-506): a request within three frames of the last computed path
/// waits one second (`setQueueForPathTime(LOGICFRAMES_PER_SECOND)`). The first order is queued in frame 2, and the
/// frame-2 pathfinder queue is taken by the zone calculation (`processPathfindQueue`, AIPathfind.cpp:6010-6013; the C++
/// trace shows `ZR_ZONECALC f=2` and the path at frame 3), so its path is computed in frame 3. A reorder in frame 4 is
/// then within three frames of that path, and its request waits.
#[test]
fn a_second_move_order_within_three_frames_queues_its_path_for_a_second() {
    let mut g = Game::from_map(data(), &map(), "synthetic", &two_players(), 1).unwrap();
    let s = g.spawned().iter().find(|s| s.template == "TestTank").unwrap().clone();
    // order at frame 2; the path is computed in frame 3; the reorder at frame 4 is within `frame-3` of it.
    g.run_frames(2);
    g.submit(s.player, Command::MoveTo { ids: vec![s.id], pos: Coord3D::new(600.0, 600.0, 0.0) });
    g.run_frames(2);
    g.submit(s.player, Command::MoveTo { ids: vec![s.id], pos: Coord3D::new(200.0, 600.0, 0.0) });
    g.run_frames(1);
    let l = g.logic();
    let o = l.find_object(s.id).unwrap();
    assert_ne!(o.ai.queue_for_path_frame, 0, "the second request waits for its path");
    assert_eq!(o.ai.queue_for_path_frame, l.frame() - 1 + LOGICFRAMES_PER_SECOND, "one second from the request");
}

/// `Pathfinder::processPathfindQueue` (AIPathfind.cpp:6010-6013): the frame whose zone calculation is due
/// (frame 2 on a fresh map, `markZonesDirty` at frame < 2) computes no path. An order given in that frame gets its
/// path one frame later, so the tank has no path after the frame it is ordered in.
#[test]
fn a_move_order_in_the_zone_calculation_frame_has_no_path_yet() {
    let mut g = Game::from_map(data(), &map(), "synthetic", &two_players(), 1).unwrap();
    let s = g.spawned().iter().find(|s| s.template == "TestTank").unwrap().clone();
    g.run_frames(2);
    g.submit(s.player, Command::MoveTo { ids: vec![s.id], pos: Coord3D::new(600.0, 600.0, 0.0) });
    g.run_frames(1);
    assert!(g.logic().find_object(s.id).unwrap().ai.path.is_empty(), "frame 2 is the zone calculation frame");
    g.run_frames(1);
    assert!(!g.logic().find_object(s.id).unwrap().ai.path.is_empty(), "the path is computed in frame 3");
}

/// The whole map is underwater (water surface above the ground): starting units cannot be placed
/// (`PartitionManager::tryPosition` water test) while the building still uses its start point.
fn map_all_water() -> MapFile {
    let mut m = map();
    m.triggers = Some(PolygonTriggers {
        version: 2,
        triggers: vec![PolygonTrigger {
            name: "flood".into(),
            layer: String::new(),
            id: 1,
            is_water: true,
            is_river: false,
            river_start: 0,
            points: vec![[0, 0, 20], [1000, 0, 20], [1000, 1000, 20], [0, 1000, 20]],
        }],
    });
    m
}

#[test]
fn starting_units_avoid_water() {
    let g = Game::from_map(data(), &map_all_water(), "synthetic", &two_players(), 7).unwrap();
    let sp = g.spawned();
    assert!(sp.iter().any(|s| s.role == "StartingBuilding"), "buildings are still placed");
    assert!(
        sp.iter().all(|s| !s.role.starts_with("StartingUnit")),
        "no ground unit may spawn in water: {:?}",
        sp.iter().map(|s| &s.role).collect::<Vec<_>>()
    );
}
