//! Real-data audio report against the player's Zero Hour install.
//!
//! Usage: `cargo run --release -p zr-audio --example audio_report`
//! (`ZR_ZEROHOUR_DIR` overrides Steam detection).

use zr_assets::{Installs, Vfs};
use zr_audio::{make_report, AudioDatabase};

fn main() {
    let install = Installs::detect().zero_hour.expect("Zero Hour install not found (set ZR_ZEROHOUR_DIR)");
    let vfs = Vfs::zero_hour(&install).expect("mounting Zero Hour");
    let loaded = zr_ini::load_zero_hour(&vfs);
    let db = AudioDatabase::from_ini(&loaded.db, &loaded.language);
    print!("{}", make_report(&db, &vfs));
}
