//! Real-install test. Skips (passes) gracefully when no Zero Hour install is
//! found, so the workspace suite still runs on CI without the game.

use zr_assets::{Installs, Vfs};
use zr_audio::{make_report, AudioDatabase};

#[test]
fn real_install_audio_tables_and_wav_headers() {
    let Some(install) = Installs::detect().zero_hour else {
        eprintln!("skipping: no Zero Hour install found");
        return;
    };
    let Ok(vfs) = Vfs::zero_hour(&install) else {
        eprintln!("skipping: could not mount Zero Hour VFS");
        return;
    };
    let loaded = zr_ini::load_zero_hour(&vfs);
    let db = AudioDatabase::from_ini(&loaded.db, &loaded.language);

    let (sfx, music, dialog) = db.counts();
    assert!(sfx > 1000, "sound effects parsed: {sfx}");
    assert!(music > 20, "music tracks parsed: {music}");
    assert!(dialog > 1000, "dialog events parsed: {dialog}");

    let report = make_report(&db, &vfs);
    assert!(report.wav.parsed > 1000, "wav headers decoded: {}", report.wav.parsed);
    assert!(report.total_references() > 0);
    // The retail data is self-contained; if this drops, a folder/extension guess is wrong.
    let pct = report.total_resolved() * 100 / report.total_references().max(1);
    assert!(pct > 80, "resolved {pct}% of {} references", report.total_references());
    eprintln!("{}", report.summary());
}
