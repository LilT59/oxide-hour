//! Loading the shipped INI data from a mounted Zero Hour [`Vfs`], in the order
//! the retail engine loads it.
//!
//! # Engine load order
//!
//! The engine has no master list of INI files. Every subsystem loads its own
//! data from its `init()`, and `GameEngine::init`
//! (`GeneralsMD/Code/GameEngine/Source/Common/GameEngine.cpp`) creates the
//! subsystems in a fixed order. Most go through
//! `SubsystemInterfaceList::initSubsystem(sys, path1, path2, ...)`
//! (`Core/GameEngine/Source/Common/System/SubsystemInterface.cpp`), which calls
//! `sys->init()` and then `INI::loadFileDirectory(path1)` and
//! `INI::loadFileDirectory(path2)`, conventionally `Data\INI\Default\X` then
//! `Data\INI\X`. [`LOAD_ORDER`] reproduces that sequence, including the loads
//! done inside other subsystems' `init()` (audio, `GameLOD`, `Mouse`, the
//! control bar ...), each cited per entry.
//!
//! `INI::loadFileDirectory(name)` (`Core/.../Common/INI/INI.cpp`) is:
//!
//! 1. strip a trailing `.ini` to get the directory name;
//! 2. load `name.ini` if it exists;
//! 3. `INI::loadDirectory(name)`: list `name\*.ini` **recursively** (default
//!    `LoadFlags_SearchSubDirs`), as one `std::set` ordered case-insensitively
//!    by full path (`FileSystem::getFileListInDirectory`, `FilenameList` in
//!    `FileSystem.h`), then load first the files that sit directly in the
//!    directory and only afterwards the files of subdirectories, each group in
//!    set order.
//!
//! So `Data\INI\Object.ini` (absent in retail) would precede
//! `Data\INI\Object\*.ini`, and `Data\INI\Object\Foo\a.ini` follows
//! `Data\INI\Object\z.ini`.
//!
//! Gaps against the engine: only archived files are listed (the engine also
//! lists loose files in the install, which retail does not have); the debug-only
//! `GameDataDebug` / `CommandMapDebug` / `CommandMapDemo` loads are skipped; the
//! relative order of the client-side UI loads (`GameClient::init`) is
//! reconstructed from the init order and does not matter for the simulation
//! data, which lives in disjoint block types.
//!
//! # Per-map overrides
//!
//! See [`load_map_ini`].

use std::collections::BTreeSet;

use zr_assets::{normalize_path, Vfs};

use crate::{IniDb, ParseError};

/// One `INI::loadFileDirectory` call, with the subsystem that makes it.
/// `{lang}` in `path` is the registry language (`GetRegistryLanguage`, default `english`).
pub struct Step {
    pub subsystem: &'static str,
    pub path: &'static str,
}

const fn s(subsystem: &'static str, path: &'static str) -> Step {
    Step { subsystem, path }
}

/// Engine order of `loadFileDirectory` calls.
///
/// Sources (all `GeneralsMD/Code/GameEngine/Source` unless noted; `Core/` =
/// `Core/GameEngine/Source`): the `initSubsystem(...)` list in
/// `Common/GameEngine.cpp` (`GameEngine::init`), plus the loads inside
/// `GameLODManager::init` (`Common/GameLOD.cpp`), `AudioManager::init`
/// (`Core/Common/Audio/GameAudio.cpp`), `GlobalLanguage::init`
/// (`Core/GameClient/GlobalLanguage.cpp`), `ParticleSystemManager::init`
/// (`Core/GameClient/System/ParticleSys.cpp`) and `GameClient::init` with its
/// children (`GameClient/GameClient.cpp`).
pub const LOAD_ORDER: &[Step] = &[
    // GameEngine.cpp: initSubsystem(TheWritableGlobalData, ..., "Default\\GameData", "GameData")
    s("GlobalData", r"data\ini\default\gamedata"),
    s("GlobalData", r"data\ini\gamedata"),
    // GameLODManager::init, right after TheArchiveFileSystem->loadMods()
    s("GameLOD", r"data\ini\gamelod"),
    s("GameLOD", r"data\ini\gamelodpresets"),
    // GameEngine.cpp: "read the water settings from INI (must do prior to initing GameClient)"
    s("Water", r"data\ini\default\water"),
    s("Water", r"data\ini\water"),
    s("Weather", r"data\ini\default\weather"),
    s("Weather", r"data\ini\weather"),
    // GameEngine.cpp (TheGameText is created in between and loads no INI)
    s("ScienceStore", r"data\ini\default\science"),
    s("ScienceStore", r"data\ini\science"),
    s("MultiplayerSettings", r"data\ini\default\multiplayer"),
    s("MultiplayerSettings", r"data\ini\multiplayer"),
    s("TerrainTypes", r"data\ini\default\terrain"),
    s("TerrainTypes", r"data\ini\terrain"),
    s("TerrainRoads", r"data\ini\default\roads"),
    s("TerrainRoads", r"data\ini\roads"),
    // GlobalLanguage::init: "Data\\%s\\Language"
    s("GlobalLanguage", r"data\{lang}\language"),
    // AudioManager::init (GameAudio.cpp:218..233), called by initSubsystem(TheAudio)
    s("Audio", r"data\ini\audiosettings"),
    s("Audio", r"data\ini\default\music"),
    s("Audio", r"data\ini\music"),
    s("Audio", r"data\ini\default\soundeffects"),
    s("Audio", r"data\ini\soundeffects"),
    s("Audio", r"data\ini\default\speech"),
    s("Audio", r"data\ini\speech"),
    s("Audio", r"data\ini\default\voice"),
    s("Audio", r"data\ini\voice"),
    s("Audio", r"data\ini\miscaudio"),
    // GameEngine.cpp: RankInfoStore passes (nullptr, "Data\\INI\\Rank")
    s("RankInfoStore", r"data\ini\rank"),
    s("PlayerTemplateStore", r"data\ini\default\playertemplate"),
    s("PlayerTemplateStore", r"data\ini\playertemplate"),
    // ParticleSystemManager::init
    s("ParticleSystem", r"data\ini\particlesystem"),
    s("FXListStore", r"data\ini\default\fxlist"),
    s("FXListStore", r"data\ini\fxlist"),
    s("WeaponStore", r"data\ini\weapon"),
    s("ObjectCreationListStore", r"data\ini\default\objectcreationlist"),
    s("ObjectCreationListStore", r"data\ini\objectcreationlist"),
    s("LocomotorStore", r"data\ini\locomotor"),
    s("SpecialPowerStore", r"data\ini\default\specialpower"),
    s("SpecialPowerStore", r"data\ini\specialpower"),
    s("DamageFXStore", r"data\ini\damagefx"),
    s("ArmorStore", r"data\ini\armor"),
    s("ThingFactory", r"data\ini\default\object"),
    s("ThingFactory", r"data\ini\object"),
    s("UpgradeCenter", r"data\ini\default\upgrade"),
    s("UpgradeCenter", r"data\ini\upgrade"),
    // initSubsystem(TheGameClient) -> GameClient::init, in its own order:
    s("GameClient", r"data\ini\drawgroupinfo"),
    // ImageCollection::load(512) (Core/GameClient/System/Image.cpp) calls `loadDirectory`
    // directly; there is no `TextureSize_512.ini`, so the `loadFileDirectory` form is equivalent.
    s("MappedImages", r"data\ini\mappedimages\texturesize_512"),
    s("MappedImages", r"data\ini\mappedimages\handcreated"),
    s("Anim2D", r"data\ini\animation2d"),
    s("Mouse", r"data\ini\mouse"), // Mouse::parseIni via initCursorResources
    s("HeaderTemplate", r"data\{lang}\headertemplate"),
    s("WindowTransitions", r"data\ini\windowtransitions"),
    s("Shell", r"data\ini\default\shellmenuscheme"), // Shell::init (ShellMenuScheme.cpp repeats it)
    s("Shell", r"data\ini\shellmenuscheme"),
    s("InGameUI", r"data\ini\ingameui"),
    s("ControlBar", r"data\ini\default\commandbutton"), // ControlBar::init, from InGameUI::init
    s("ControlBar", r"data\ini\commandbutton"),
    s("ControlBar", r"data\ini\commandset"),
    s("ControlBarScheme", r"data\ini\default\controlbarscheme"),
    s("ControlBarScheme", r"data\ini\controlbarscheme"),
    s("ControlBarResizer", r"data\ini\controlbarresizer"),
    s("ChallengeGenerals", r"data\ini\challengemode"),
    s("VideoPlayer", r"data\ini\default\video"),
    s("VideoPlayer", r"data\ini\video"),
    s("CampaignManager", r"data\ini\campaign"),
    s("Eva", r"data\ini\eva"),
    // back in GameEngine.cpp
    s("AI", r"data\ini\default\aidata"),
    s("AI", r"data\ini\aidata"),
    s("CrateSystem", r"data\ini\default\crate"),
    s("CrateSystem", r"data\ini\crate"),
    // MetaMap: path1 = "Data\\%s\\CommandMap", path2 = "Data\\INI\\CommandMap"
    s("MetaMap", r"data\{lang}\commandmap"),
    s("MetaMap", r"data\ini\commandmap"),
    // Loaded on demand (menus), not at startup: Credits::init, WebBrowser::init
    s("Credits", r"data\ini\credits"),
    s("WebBrowser", r"data\ini\webpages"),
];

/// Result of [`load_zero_hour`].
pub struct Loaded {
    pub db: IniDb,
    /// `.ini` files parsed: those loaded in engine order, plus (for
    /// validation only) per-map `map.ini` files and files nothing loads.
    pub files: usize,
    pub bytes: usize,
    /// Top-level blocks in per-map `map.ini` files, which are not merged into `db`.
    pub map_override_blocks: usize,
    pub failures: Vec<ParseError>,
    /// Files that fail but are never loaded by the engine (`Default\ButtonSets.ini`).
    pub legacy: Vec<ParseError>,
    /// The language directory used for `Data\{lang}\...` (e.g. `english`).
    pub language: String,
    /// Files merged into `db`, in load order (normalized paths).
    pub order: Vec<String>,
    /// Archived `.ini` files (outside `maps\`) that no engine load reaches.
    /// They were parsed for validation but not merged.
    pub unreferenced: Vec<String>,
}

/// The language the engine would use: `GetRegistryLanguage` defaults to
/// `english` (`System/registry.cpp`); the registry is not consulted here, so
/// pick `english` when the install has it, else the first language directory
/// that has a `Language.ini`.
pub fn detect_language(vfs: &Vfs) -> String {
    let mut langs = BTreeSet::new();
    for (p, _) in vfs.files() {
        if let Some(rest) = p.strip_prefix(r"data\") {
            if let Some(l) = rest.strip_suffix(r"\language.ini") {
                if !l.contains('\\') {
                    langs.insert(l.to_owned());
                }
            }
        }
    }
    if langs.is_empty() || langs.contains("english") {
        "english".into()
    } else {
        langs.into_iter().next().unwrap_or_default()
    }
}

/// Archived `.ini` paths, sorted the way the engine's case-insensitive
/// `std::set` orders them (paths are already lowercase, so a byte compare).
fn ini_paths(vfs: &Vfs) -> Vec<&str> {
    let mut v: Vec<&str> = vfs.files().map(|(p, _)| p).filter(|p| p.ends_with(".ini")).collect();
    v.sort_unstable();
    v
}

/// `INI::loadFileDirectory(name)` resolved against `paths` (sorted, see
/// [`ini_paths`]): the file `name.ini`, then the files directly in `name\`,
/// then the files in its subdirectories.
fn expand_file_directory<'a>(paths: &[&'a str], name: &str) -> Vec<&'a str> {
    let dir = name.strip_suffix(".ini").unwrap_or(name);
    let file = format!("{dir}.ini");
    let prefix = format!("{dir}\\");
    let mut out = Vec::new();
    if let Ok(i) = paths.binary_search(&file.as_str()) {
        out.push(paths[i]);
    }
    let start = paths.partition_point(|p| *p < prefix.as_str());
    let in_dir: Vec<&str> =
        paths[start..].iter().copied().take_while(|p| p.starts_with(&prefix)).collect();
    out.extend(in_dir.iter().copied().filter(|p| !p[prefix.len()..].contains('\\')));
    out.extend(in_dir.iter().copied().filter(|p| p[prefix.len()..].contains('\\')));
    out
}

/// Every file the engine loads at startup, in order: [`LOAD_ORDER`] expanded
/// through `loadFileDirectory` semantics.
pub fn engine_load_order(vfs: &Vfs, language: &str) -> Vec<String> {
    let paths = ini_paths(vfs);
    let lang = normalize_path(language);
    LOAD_ORDER
        .iter()
        .flat_map(|step| expand_file_directory(&paths, &step.path.replace("{lang}", &lang)))
        .map(str::to_owned)
        .collect()
}

fn parse_file(vfs: &Vfs, path: &str) -> Option<(usize, Result<Vec<crate::Block>, ParseError>)> {
    let data = vfs.read_archived(path)?;
    let text = String::from_utf8_lossy(data);
    let mut loader = |inc: &str| vfs.read_archived(inc).map(|b| String::from_utf8_lossy(b).into());
    Some((data.len(), crate::parse_with(path, &text, &mut loader)))
}

/// Parses the base-data `.ini` files into one [`IniDb`] in engine order
/// (see the module docs and [`LOAD_ORDER`]), with the detected language.
///
/// Files the engine never reaches are still parsed so syntax problems show up,
/// but are kept out of `db`: map-local `map.ini` files are per-map overrides
/// (see [`load_map_ini`]) and the rest are listed in [`Loaded::unreferenced`].
pub fn load_zero_hour(vfs: &Vfs) -> Loaded {
    load_zero_hour_lang(vfs, &detect_language(vfs))
}

/// As [`load_zero_hour`] with an explicit `Data\<language>` directory.
pub fn load_zero_hour_lang(vfs: &Vfs, language: &str) -> Loaded {
    let order = engine_load_order(vfs, language);
    let mut out = Loaded {
        db: IniDb::new(),
        files: 0,
        bytes: 0,
        map_override_blocks: 0,
        failures: Vec::new(),
        legacy: Vec::new(),
        language: language.to_owned(),
        order: Vec::new(),
        unreferenced: Vec::new(),
    };
    let mut seen = BTreeSet::new();
    for path in &order {
        seen.insert(path.as_str());
        let Some((len, res)) = parse_file(vfs, path) else { continue };
        out.files += 1;
        out.bytes += len;
        match res {
            Ok(blocks) => {
                out.db.extend(blocks);
                out.order.push(path.clone());
            }
            Err(e) => out.failures.push(e),
        }
    }
    for path in ini_paths(vfs) {
        if seen.contains(path) {
            continue;
        }
        let Some((len, res)) = parse_file(vfs, path) else { continue };
        out.files += 1;
        out.bytes += len;
        let is_map = path.starts_with("maps\\");
        match res {
            Ok(blocks) if is_map => out.map_override_blocks += blocks.len(),
            Ok(_) => out.unreferenced.push(path.to_owned()),
            // `ButtonSet` is absent from the engine's `theTypeTable`; nothing loads this file.
            Err(e) if e.file.ends_with("buttonsets.ini") => out.legacy.push(e),
            Err(e) if is_map => out.failures.push(e),
            Err(e) => {
                out.unreferenced.push(path.to_owned());
                out.legacy.push(e);
            }
        }
    }
    out
}

/// Result of [`load_map_ini`].
#[derive(Debug, Default)]
pub struct MapIni {
    /// Files overlaid onto the database, in order (`map.ini`, then `solo.ini`).
    pub loaded: Vec<String>,
    pub failures: Vec<ParseError>,
    /// `<mapdir>\map.str` if the map has one; `GameLogic::loadMapINI` hands it to
    /// `TheGameText->initMapStringFile` (see `zr_text`).
    pub map_str: Option<String>,
    /// `IniDb::checkpoint` taken before the overlay; pass it to
    /// [`IniDb::rollback`] when the map ends (the engine deletes the override
    /// instances on reset, e.g. `ThingFactory::reset`).
    pub checkpoint: usize,
}

/// Directory of a map for `map.ini` lookup, as `GameLogic::loadMapINI` computes it:
/// drop the last 4 characters (`.map`) and everything after the preceding
/// path separator. `None` when the name is shorter than 4 characters.
pub fn map_dir(map_path: &str) -> Option<String> {
    if map_path.len() < 4 || !map_path.is_char_boundary(map_path.len() - 4) {
        return None;
    }
    let mut cut = map_path.len() - 4;
    let b = map_path.as_bytes();
    while cut > 0 && b[cut] != b'\\' && b[cut] != b'/' {
        cut -= 1;
    }
    Some(map_path[..cut].to_owned())
}

/// Overlays a map's `map.ini` (and `solo.ini`) onto `db`.
///
/// Ported from `GameLogic::loadMapINI`
/// (`GeneralsMD/Code/GameEngine/Source/GameLogic/System/GameLogic.cpp`), called
/// from `startNewGame` before the map's objects are created: for the map file
/// `Maps\Foo\Foo.map` it loads `Maps\Foo\map.ini` and then `Maps\Foo\solo.ini`
/// with `INI_LOAD_CREATE_OVERRIDES`, if they exist. (The engine also loads a
/// `mapcache.ini` per map directory, `MapCache::loadMapsFromMapCacheINI`; that is
/// menu metadata, not gameplay data, and is not handled here.)
///
/// `CREATE_OVERRIDES` stacks a new override instance per (type, name) that
/// replaces fields it mentions; [`IniDb`] already models that: later definitions
/// win field by field in [`IniDb::resolved`] / [`crate::Block::field`]. Use
/// [`IniDb::rollback`] with the returned checkpoint to remove the overlay.
pub fn load_map_ini(db: &mut IniDb, vfs: &Vfs, map_path: &str) -> MapIni {
    let mut out = MapIni { checkpoint: db.checkpoint(), ..MapIni::default() };
    let Some(dir) = map_dir(&normalize_path(map_path)) else { return out };
    for name in ["map.ini", "solo.ini"] {
        let path = format!("{dir}\\{name}");
        let Some((_, res)) = parse_file(vfs, &path) else { continue };
        match res {
            Ok(blocks) => {
                db.extend(blocks);
                out.loaded.push(path);
            }
            Err(e) => out.failures.push(e),
        }
    }
    let str_path = format!("{dir}\\map.str");
    if vfs.read_archived(&str_path).is_some() {
        out.map_str = Some(str_path);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a synthetic `BIGF` archive and mounts it into a fresh [`Vfs`].
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
        let path = std::env::temp_dir().join(format!("zr-ini-test-{}-{n}.big", std::process::id()));
        std::fs::write(&path, out).unwrap();
        let mut vfs = Vfs::empty();
        vfs.mount(&path).unwrap();
        let _ = std::fs::remove_file(&path); // mapping stays valid on unix; on Windows removal may fail, ignored
        vfs
    }

    #[test]
    fn directory_order_files_then_subdirs_case_insensitive() {
        let paths = [
            r"data\ini\object.ini",
            r"data\ini\object\b.ini",
            r"data\ini\object\a.ini",
            r"data\ini\object\sub\a.ini",
            r"data\ini\object\z.ini",
            r"data\ini\objectx.ini",
            r"data\ini\object\readme.txt",
        ];
        let mut p: Vec<&str> = paths.iter().copied().filter(|p| p.ends_with(".ini")).collect();
        p.sort_unstable();
        assert_eq!(
            expand_file_directory(&p, r"data\ini\object"),
            [
                r"data\ini\object.ini",
                r"data\ini\object\a.ini",
                r"data\ini\object\b.ini",
                r"data\ini\object\z.ini",
                r"data\ini\object\sub\a.ini"
            ]
        );
        assert_eq!(expand_file_directory(&p, r"data\ini\object.ini").len(), 5, "`.ini` suffix is stripped");
        assert!(expand_file_directory(&p, r"data\ini\missing").is_empty());
    }

    #[test]
    fn load_follows_engine_order_not_alphabet() {
        let vfs = vfs_with(&[
            // alphabetical order would load Armor before Default\Armor-like and Object before Weapon
            ("Data/INI/Weapon.ini", "Weapon W\n Damage = 1\nEnd\n"),
            ("Data/INI/Object/Tank.ini", "Object T\n Weapon = W\nEnd\n"),
            ("Data/INI/Default/Object.ini", "Object T\n Side = Default\nEnd\n"),
            ("Data/INI/Object/Sub/Late.ini", "Object T\n Side = Late\nEnd\n"),
            ("Data/INI/Object/Aaa.ini", "Object T\n Side = Early\nEnd\n"),
            ("Data/INI/GameData.ini", "GameData\nEnd\n"),
            ("Data/English/Language.ini", "Language\nEnd\n"),
            ("Data/INI/Stray.ini", "Armor A\nEnd\n"),
            ("Maps/M/map.ini", "Weapon W\n Damage = 9\nEnd\n"),
        ]);
        assert_eq!(detect_language(&vfs), "english");
        let loaded = load_zero_hour(&vfs);
        assert!(loaded.failures.is_empty(), "{:?}", loaded.failures);
        assert_eq!(
            loaded.order,
            [
                r"data\ini\gamedata.ini",
                r"data\english\language.ini",
                r"data\ini\weapon.ini",
                r"data\ini\default\object.ini",
                r"data\ini\object\aaa.ini",
                r"data\ini\object\tank.ini",
                r"data\ini\object\sub\late.ini",
            ]
        );
        assert_eq!(loaded.unreferenced, [r"data\ini\stray.ini"]);
        assert_eq!(loaded.map_override_blocks, 1);
        let defs = loaded.db.definitions("Object", "T");
        let sides: Vec<_> = defs.iter().map(|b| b.value("Side")).collect();
        assert_eq!(sides, [Some("Default"), Some("Early"), None, Some("Late")]);
        assert_eq!(loaded.db.get("Weapon", "W").unwrap().value("Damage"), Some("1"), "map.ini is not in base data");
    }

    #[test]
    fn map_dir_matches_engine() {
        assert_eq!(map_dir(r"maps\alpine assault\alpine assault.map").as_deref(), Some(r"maps\alpine assault"));
        assert_eq!(map_dir("maps/foo/foo.map").as_deref(), Some("maps/foo"));
        assert_eq!(map_dir("foo.map").as_deref(), Some(""));
        assert_eq!(map_dir("map"), None);
    }

    #[test]
    fn map_ini_overlays_and_rolls_back() {
        let vfs = vfs_with(&[
            ("Maps/Foo/Foo.map", "x"),
            ("Maps/Foo/map.ini", "Weapon W\n Damage = 9\nEnd\nObject New\n Side = Map\nEnd\n"),
            ("Maps/Foo/solo.ini", "Weapon W\n Damage = 12\nEnd\n"),
            ("Maps/Foo/map.str", "x"),
            ("Maps/Bar/map.ini", "Weapon Bar\nEnd\n"),
            ("Data/INI/Weapon.ini", "Weapon W\n Damage = 1\n Range = 5\nEnd\n"),
        ]);
        let mut db = load_zero_hour(&vfs).db;
        let before = db.len();
        let m = load_map_ini(&mut db, &vfs, r"Maps\Foo\Foo.map");
        assert_eq!(m.loaded, [r"maps\foo\map.ini", r"maps\foo\solo.ini"]);
        assert_eq!(m.map_str.as_deref(), Some(r"maps\foo\map.str"));
        assert!(m.failures.is_empty());
        assert_eq!(db.len(), before + 3);
        let w = db.resolved("Weapon", "W").unwrap();
        assert_eq!((w.value("Damage"), w.value("Range")), (Some("12"), Some("5")), "latest wins per field");
        assert!(db.get("Object", "New").is_some());
        assert!(db.get("Weapon", "Bar").is_none(), "other maps' map.ini are untouched");

        db.rollback(m.checkpoint);
        assert_eq!(db.len(), before);
        assert_eq!(db.get("Weapon", "W").unwrap().value("Damage"), Some("1"));
        assert!(db.get("Object", "New").is_none());
        assert_eq!(db.definitions("Weapon", "W").len(), 1);

        let none = load_map_ini(&mut db, &vfs, "maps/missing/missing.map");
        assert!(none.loaded.is_empty() && none.map_str.is_none());
    }
}
