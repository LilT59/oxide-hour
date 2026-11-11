//! Locating the player's game installs.

use std::path::{Path, PathBuf};

/// Environment variables that override auto-detection.
pub const ZH_ENV: &str = "ZR_ZEROHOUR_DIR";
pub const RENEGADE_ENV: &str = "ZR_RENEGADE_DIR";

const ZH_STEAM_DIR: &str = "Command & Conquer Generals - Zero Hour";
const RENEGADE_STEAM_DIR: &str = "Command & Conquer Renegade";

#[derive(Debug, Clone)]
pub struct GameInstall {
    pub root: PathBuf,
}

#[derive(Debug, Clone, Default)]
pub struct Installs {
    pub zero_hour: Option<GameInstall>,
    pub renegade: Option<GameInstall>,
}

impl Installs {
    /// Finds installs via env overrides, then the default Steam library.
    pub fn detect() -> Self {
        Self {
            zero_hour: find(ZH_ENV, ZH_STEAM_DIR, |p| p.join("INIZH.big").is_file()),
            renegade: find(RENEGADE_ENV, RENEGADE_STEAM_DIR, |p| p.join("Data").join("always.dat").is_file()),
        }
    }
}

impl GameInstall {
    /// Base Generals archives that Zero Hour falls back to. Steam ships them
    /// in `ZH_Generals\`; the retail game read the path from the registry.
    pub fn generals_base_dir(&self) -> Option<PathBuf> {
        let p = self.root.join("ZH_Generals");
        p.is_dir().then_some(p)
    }
}

fn find(env: &str, steam_dir: &str, valid: impl Fn(&Path) -> bool) -> Option<GameInstall> {
    let candidates = std::env::var_os(env)
        .map(PathBuf::from)
        .into_iter()
        .chain(steam_libraries().into_iter().map(|lib| lib.join("steamapps").join("common").join(steam_dir)));
    candidates.filter(|p| valid(p)).map(|root| GameInstall { root }).next()
}

/// Default Steam root plus any extra libraries listed in `libraryfolders.vdf`.
fn steam_libraries() -> Vec<PathBuf> {
    let mut libs = Vec::new();
    for root in [r"C:\Program Files (x86)\Steam", r"C:\Program Files\Steam"] {
        let root = PathBuf::from(root);
        let vdf = root.join("steamapps").join("libraryfolders.vdf");
        libs.push(root);
        if let Ok(text) = std::fs::read_to_string(vdf) {
            for line in text.lines() {
                let mut parts = line.split('"').filter(|s| !s.trim().is_empty());
                if parts.next() == Some("path") {
                    if let Some(p) = parts.next() {
                        libs.push(PathBuf::from(p.replace(r"\\", r"\")));
                    }
                }
            }
        }
    }
    libs.dedup();
    libs
}
