//! Lane 134: the map's `Lighting` time of day and `WorldInfo` weather become the sim's game-wide
//! `NIGHT` / `SNOW` model conditions.
//!
//! `WorldHeightMap::ParseLightingDataChunk` stores the chunk's `TimeOfDay` in `TheGlobalData->m_timeOfDay`
//! (WorldHeightMap.cpp:742-744) and `ParseWorldDictDataChunk` stores `weather` in `m_weather` (:723-733);
//! `Object::friend_bindToDrawable` (Object.cpp:3038-3048) folds `TIME_OF_DAY_NIGHT` / `WEATHER_SNOWY` into every
//! drawable's conditions. The engine default is afternoon / normal (GlobalData.cpp:725-730), and `TimeOfDay` is
//! 1 morning, 2 afternoon, 3 evening, 4 night while `Weather` is 0 normal, 1 snowy
//! (Core/GameEngine/Include/Common/GameType.h:64-85).

use std::sync::Arc;

use zr_game::{Game, GameData, PlayerSetup};
use zr_ini::IniDb;
use zr_map::{Dict, DictValue, GlobalLight, GlobalLighting, HeightMap, MapFile, MapObject};

const INI: &str = "
Object TestTower
  KindOf = IMMOBILE
End
PlayerTemplate FactionTest
  Side = Test
  StartMoney = 0
End
";

fn data() -> Arc<GameData> {
    let mut db = IniDb::new();
    db.extend(zr_ini::parse("t.ini", INI).expect("test INI parses"));
    Arc::new(GameData::from_ini(&db))
}

fn map() -> MapFile {
    let mut m = MapFile::default();
    let (w, h) = (60usize, 60usize);
    m.height_map = Some(HeightMap { version: 4, width: w as i32, height: h as i32, border_size: 0, boundaries: vec![], data: vec![16; w * h] });
    m.objects = vec![MapObject {
        version: 3,
        pos: [300.0, 300.0, 0.0],
        angle: 0.0,
        flags: 0,
        template: "*Waypoints/Waypoint".to_owned(),
        props: Dict(vec![
            ("waypointName".to_owned(), DictValue::Ascii("Player_1_Start".to_owned())),
            ("waypointID".to_owned(), DictValue::Int(1)),
        ]),
    }];
    m
}

fn lighting(time_of_day: i32) -> GlobalLighting {
    GlobalLighting {
        version: 3,
        time_of_day,
        terrain: [[GlobalLight::default(); 3]; 4],
        objects: [[GlobalLight::default(); 3]; 4],
        shadow_color: None,
    }
}

fn start(map: &MapFile) -> Game {
    Game::from_map(data(), map, "synthetic", &[PlayerSetup::new("FactionTest", 0)], 7).expect("the synthetic map starts")
}

/// `Lighting` time of day 4 is `TIME_OF_DAY_NIGHT` and `WorldInfo weather` 1 is `WEATHER_SNOWY`; both become
/// the sim's game-wide `NIGHT` / `SNOW` bits. Fails if `from_map_inner` does not forward the map's chunks.
#[test]
fn the_map_sets_night_and_snow() {
    let mut m = map();
    m.lighting = Some(lighting(4));
    m.world_info = Some(Dict(vec![("weather".to_owned(), DictValue::Int(1))]));
    let g = start(&m);
    assert!(g.logic().launch_environment().night, "time of day 4 is night");
    assert!(g.logic().launch_environment().snow, "weather 1 is snowy");
}

/// The two chunks are independent: night alone is not snow, snow alone is not night, and afternoon/normal is
/// the neutral default (GlobalData.cpp:725-730). Fails if the night test compares the raw value against the
/// wrong enum member (e.g. evening = 3) or reads the weather as a boolean.
#[test]
fn night_and_snow_are_independent() {
    let mut m = map();
    m.lighting = Some(lighting(4));
    let g = start(&m);
    assert!(g.logic().launch_environment().night && !g.logic().launch_environment().snow, "night with no weather is not snow");

    let mut m = map();
    m.lighting = Some(lighting(3)); // TIME_OF_DAY_EVENING
    m.world_info = Some(Dict(vec![("weather".to_owned(), DictValue::Int(1))]));
    let g = start(&m);
    assert!(!g.logic().launch_environment().night && g.logic().launch_environment().snow, "evening is not night");
}

/// A map without a `Lighting` or `WorldInfo` chunk keeps the engine's afternoon / normal default
/// (GlobalData.cpp:725-730), which is what the sim already had before the hook existed.
#[test]
fn a_map_without_the_chunks_keeps_the_engine_default() {
    let g = start(&map());
    assert!(!g.logic().launch_environment().night && !g.logic().launch_environment().snow, "afternoon / normal");
    let mut m = map();
    m.lighting = Some(lighting(2)); // TIME_OF_DAY_AFTERNOON
    m.world_info = Some(Dict(vec![("weather".to_owned(), DictValue::Int(0))]));
    let g = start(&m);
    assert!(!g.logic().launch_environment().night && !g.logic().launch_environment().snow, "afternoon / normal");
}
