//! Per-user data directory for replays and the options config. Never inside the repository.

use std::path::{Path, PathBuf};

/// The user data directory (`%APPDATA%\OxideHour` on Windows, `~/.oxide-hour` elsewhere, falling
/// back to the temp dir).
///
/// The directory had another name under the project's earlier working title. When only that
/// older directory exists it is renamed to the new name, so options and replays carry over; if
/// the rename fails, the older directory keeps being used.
pub fn dir() -> PathBuf {
    let (current, legacy) = candidates();
    resolve(current, &legacy)
}

/// The directory under the current name and under the earlier working title's name.
fn candidates() -> (PathBuf, PathBuf) {
    if let Ok(appdata) = std::env::var("APPDATA") {
        let base = PathBuf::from(appdata);
        return (base.join("OxideHour"), base.join("ZeroHourRenegade"));
    }
    if let Ok(home) = std::env::var("HOME") {
        let base = PathBuf::from(home);
        return (base.join(".oxide-hour"), base.join(".zerohour-renegade"));
    }
    let base = std::env::temp_dir();
    (base.join("oxide-hour"), base.join("zerohour-renegade"))
}

/// Picks `current`, after moving `legacy` there when only `legacy` exists. Returns `legacy` when
/// that move fails.
fn resolve(current: PathBuf, legacy: &Path) -> PathBuf {
    if current.exists() || !legacy.is_dir() {
        return current;
    }
    match std::fs::rename(legacy, &current) {
        Ok(()) => current,
        Err(_) => legacy.to_path_buf(),
    }
}

/// Creates and returns the user data directory.
pub fn ensure() -> std::io::Result<PathBuf> {
    let d = dir();
    std::fs::create_dir_all(&d)?;
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::resolve;
    use std::path::PathBuf;

    /// A fresh, empty scratch directory for one test.
    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("zr-userdata-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn legacy_directory_is_moved_to_the_new_name() {
        let base = scratch("move");
        let (current, legacy) = (base.join("new"), base.join("old"));
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("options.ini"), "x").unwrap();

        assert_eq!(resolve(current.clone(), &legacy), current);
        assert_eq!(std::fs::read_to_string(current.join("options.ini")).unwrap(), "x");
        assert!(!legacy.exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn existing_new_directory_wins_and_legacy_is_left_alone() {
        let base = scratch("both");
        let (current, legacy) = (base.join("new"), base.join("old"));
        std::fs::create_dir_all(&current).unwrap();
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("a.zrp"), "x").unwrap();

        assert_eq!(resolve(current.clone(), &legacy), current);
        assert!(legacy.join("a.zrp").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn no_directory_yet_gives_the_new_name() {
        let base = scratch("none");
        let (current, legacy) = (base.join("new"), base.join("old"));

        assert_eq!(resolve(current.clone(), &legacy), current);
        assert!(!current.exists());
        let _ = std::fs::remove_dir_all(&base);
    }
}
