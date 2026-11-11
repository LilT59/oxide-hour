//! Per-map INI overrides (`GameData::for_map`), string tables and map water, on synthetic data.

use std::sync::Arc;
use zr_assets::Vfs;
use zr_game::{Game, GameData, PlayerSetup};
use zr_sim::TemplateSource;
use zr_map::{Dict, DictValue, HeightMap, MapFile, MapObject, PolygonTrigger, PolygonTriggers};

/// A synthetic `BIGF` archive mounted into a fresh [`Vfs`].
fn vfs_with(files: &[(&str, &str)]) -> Vfs {
    let header = 16 + files.iter().map(|(n, _)| 9 + n.len()).sum::<usize>();
    let mut out = b"BIGF".to_vec();
    let total = header + files.iter().map(|(_, d)| d.len()).sum::<usize>();
    out.extend((total as u32).to_le_bytes());
    out.extend((files.len() as u32).to_be_bytes());
    out.extend((header as u32).to_be_bytes());
    let mut offset = header;
    for (name, data) in files {
        out.extend((offset as u32).to_be_bytes());
        out.extend((data.len() as u32).to_be_bytes());
        out.extend(name.as_bytes());
        out.push(0);
        offset += data.len();
    }
    out.resize(header, 0);
    for (_, data) in files {
        out.extend(data.as_bytes());
    }
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("zr-game-mapini-{}-{n}.big", std::process::id()));
    std::fs::write(&path, out).unwrap();
    let mut vfs = Vfs::empty();
    vfs.mount(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    vfs
}

const BASE: &str = "
Object Tank
  DisplayName = OBJECT:Tank
  KindOf = SELECTABLE VEHICLE
  VisionRange = 100
  Body = ActiveBody ModuleTag_Body
    MaxHealth = 100
    InitialHealth = 100
  End
End
PlayerTemplate FactionTest
  Side = Test
End
";

fn vfs() -> Vfs {
    vfs_with(&[
        ("Data/INI/Object/Tank.ini", BASE),
        ("Data/Generals.str", "OBJECT:Tank\n\"Base Tank\"\nEND\n"),
        ("Maps/Foo/Foo.map", "x"),
        (
            "Maps/Foo/map.ini",
            "Object Tank\n  VisionRange = 250\n  Body = ActiveBody ModuleTag_Body\n    MaxHealth = 900\n    InitialHealth = 900\n  End\nEnd\nObject MapOnly\n  KindOf = SELECTABLE\nEnd\n",
        ),
        ("Maps/Foo/map.str", "OBJECT:Tank\n\"Foo Tank\"\nEND\nMAP:Hello\n\"hi\"\nEND\n"),
        ("Maps/Bar/Bar.map", "x"),
    ])
}

#[test]
fn map_ini_overrides_templates_without_touching_shared_data() {
    let vfs = vfs();
    let shared = Arc::new(GameData::load(&vfs));
    assert_eq!(shared.store.find_template("Tank").unwrap().vision_range, 100.0);
    assert!(shared.templates().thing("MapOnly").is_none());
    assert_eq!(shared.display_name("Tank"), Some("Base Tank"));

    let foo = shared.for_map(&vfs, r"maps\foo\foo.map");
    assert!(!Arc::ptr_eq(&shared, &foo));
    assert_eq!(foo.map_ini_files, [r"maps\foo\map.ini"]);
    let t = foo.store.find_template("Tank").unwrap();
    assert_eq!(t.vision_range, 250.0);
    assert!(foo.templates().thing("MapOnly").is_some());
    // map.str takes second place after the main table (first lookup wins), and adds labels.
    assert_eq!(foo.text.get("MAP:Hello"), Some("hi"));
    assert_eq!(foo.display_name("Tank"), Some("Base Tank"));

    // The shared data and a later call are unaffected (the overlay was rolled back).
    assert_eq!(shared.store.find_template("Tank").unwrap().vision_range, 100.0);
    assert!(shared.text.get("MAP:Hello").is_none());
    let bar = shared.for_map(&vfs, r"maps\bar\bar.map");
    assert!(Arc::ptr_eq(&shared, &bar), "a map without map.ini reuses the shared data");
    let again = shared.for_map(&vfs, r"Maps\Foo\Foo.map");
    assert_eq!(again.store.find_template("Tank").unwrap().vision_range, 250.0);

    // Already map-specific data is not overlaid twice.
    assert!(Arc::ptr_eq(&foo, &foo.for_map(&vfs, r"maps\foo\foo.map")));
}

/// `GameData` `MovementPenaltyDamageState` (`GlobalData.cpp:411`): the engine default is REALLYDAMAGED
/// (`GlobalData.cpp:1032`), and a map's `map.ini` `GameData` block overrides it for that map only.
#[test]
fn movement_penalty_damage_state_is_game_data_and_a_map_ini_can_override_it() {
    use zr_sim::BodyDamageType;
    let vfs = vfs_with(&[
        ("Data/INI/Object/Tank.ini", BASE),
        ("Maps/Foo/Foo.map", "x"),
        ("Maps/Foo/map.ini", "GameData\n  MovementPenaltyDamageState = DAMAGED\nEnd\n"),
    ]);
    let shared = Arc::new(GameData::load(&vfs));
    assert_eq!(shared.movement_penalty_damage_state, BodyDamageType::ReallyDamaged, "no GameData block: the engine default");
    let foo = shared.for_map(&vfs, r"maps\foo\foo.map");
    assert_eq!(foo.movement_penalty_damage_state, BodyDamageType::Damaged, "the map.ini override reaches the map's data");
    assert_eq!(shared.movement_penalty_damage_state, BodyDamageType::ReallyDamaged, "the shared data is unaffected");
}

fn flat_map(water: Option<PolygonTrigger>) -> MapFile {
    let mut m = MapFile::default();
    let (w, h) = (100usize, 100usize);
    m.height_map = Some(HeightMap { version: 4, width: w as i32, height: h as i32, border_size: 0, boundaries: vec![], data: vec![16; w * h] });
    let wp = |name: &str, x: f32, y: f32| MapObject {
        version: 3,
        pos: [x, y, 0.0],
        angle: 0.0,
        flags: 0,
        template: "*Waypoints/Waypoint".into(),
        props: Dict(vec![("waypointName".into(), DictValue::Ascii(name.into())), ("waypointID".into(), DictValue::Int(1))]),
    };
    m.objects = vec![wp("Player_1_Start", 100.0, 100.0)];
    m.triggers = water.map(|t| PolygonTriggers { version: 4, triggers: vec![t] });
    m
}

#[test]
fn map_water_reaches_terrain_logic() {
    let data = Arc::new(GameData::from_ini(&{
        let mut db = zr_ini::IniDb::new();
        db.extend(zr_ini::parse("t.ini", BASE).unwrap());
        db
    }));
    let lake = PolygonTrigger {
        name: "lake".into(),
        layer: String::new(),
        id: 1,
        is_water: true,
        is_river: false,
        river_start: 0,
        points: vec![[200, 200, 100], [400, 200, 100], [400, 400, 100], [200, 400, 100]],
    };
    let setups = [PlayerSetup::new("FactionTest", 0)];
    let wet = Game::from_map(data.clone(), &flat_map(Some(lake)), "wet", &setups, 1).unwrap();
    let dry = Game::from_map(data, &flat_map(None), "dry", &setups, 1).unwrap();
    let t = wet.logic().terrain();
    assert!(t.is_underwater(300.0, 300.0));
    assert_eq!(t.water_height_at(300.0, 300.0), Some(100.0));
    assert!(!t.is_underwater(500.0, 300.0));
    assert!(!dry.logic().terrain().is_underwater(300.0, 300.0));
    assert_ne!(wet.logic().crc_with_terrain(), dry.logic().crc_with_terrain(), "water is part of the terrain CRC");
}
