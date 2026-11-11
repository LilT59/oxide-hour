//! Real-data UI report against the player's Zero Hour install, plus a dump of
//! the in-game command bar `Window\ControlBar.wnd` tree.
//!
//! Usage: `cargo run --release -p zr-ui --example ui_report`
//! (`ZR_ZEROHOUR_DIR` overrides Steam detection).

use zr_assets::{Installs, Vfs};
use zr_ui::{make_report, Window};

fn main() {
    let install = Installs::detect().zero_hour.expect("Zero Hour install not found (set ZR_ZEROHOUR_DIR)");
    let vfs = Vfs::zero_hour(&install).expect("mounting Zero Hour");
    let loaded = zr_ini::load_zero_hour(&vfs);

    print!("{}", make_report(&vfs, &loaded.db, &loaded.language));

    let bytes = vfs.read(r"Window\ControlBar.wnd").expect("ControlBar.wnd not in VFS");
    let file = zr_ui::parse(&bytes).expect("ControlBar.wnd parses");
    println!("\n--- Window\\ControlBar.wnd (version {}) ---", file.file_version);
    println!("layout init/update/shutdown: {:?}/{:?}/{:?}", file.layout.init, file.layout.update, file.layout.shutdown);
    for w in &file.windows {
        dump(w, 0);
    }
}

fn dump(w: &Window, depth: usize) {
    let pad = "  ".repeat(depth);
    let rect = w.rect;
    let name = w.name.as_deref().unwrap_or("(unnamed)");
    println!(
        "{pad}{:<14} {name:<40} [{},{} {}x{}] status={:#x} style={:#x}{}",
        w.window_type,
        rect.upper_left.0,
        rect.upper_left.1,
        rect.width(),
        rect.height(),
        w.status,
        w.style,
        w.gadget.as_ref().map_or(String::new(), |g| format!(" gadget={g:?}"))
    );
    for c in &w.children {
        dump(c, depth + 1);
    }
}
