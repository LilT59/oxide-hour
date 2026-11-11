//! Prints the raw `map.ini` / `solo.ini` text of every shipped map whose file contains a needle:
//! `map_ini_dump [needle]` (case-insensitive; default lists all map.ini paths).

use zr_assets::{Installs, Vfs};

fn main() {
    let needle = std::env::args().nth(1).map(|s| s.to_lowercase());
    let installs = Installs::detect();
    let Some(zh) = &installs.zero_hour else {
        eprintln!("no Zero Hour install found; set ZR_ZEROHOUR_DIR");
        return;
    };
    let vfs = Vfs::zero_hour(zh).expect("mounting Zero Hour");
    let paths: Vec<String> = vfs
        .files()
        .map(|(p, _)| p.to_owned())
        .filter(|p| p.starts_with("maps\\") && (p.ends_with(r"\map.ini") || p.ends_with(r"\solo.ini")))
        .collect();
    for p in paths {
        let Some(raw) = vfs.read(&p) else { continue };
        let text = String::from_utf8_lossy(&raw).into_owned();
        match &needle {
            None => println!("{p} ({} bytes)", text.len()),
            Some(n) if text.to_lowercase().contains(n) => println!("===== {p}\n{text}"),
            _ => {}
        }
    }
}
