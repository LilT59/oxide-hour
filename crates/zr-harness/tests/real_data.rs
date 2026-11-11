//! Real-data smoke test: load AN3 (`Alpine Assault`, GLA vs America, seed 1), step 300 frames,
//! query, and compare the harness CRC with a direct `zr-game` run. Skips when no Zero Hour install is
//! found, so `cargo test` works without one.

use std::path::PathBuf;

use zr_assets::{Installs, Vfs};
use zr_game::{AiDifficulty, Game};
use zr_harness::protocol;
use zr_harness::{LoadSpec, PlayerSpec, Session, SnapshotSpec};

fn temp_path(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
    p.push(format!("zr-harness-{}-{nanos}-{name}", std::process::id()));
    p
}

fn an3() -> LoadSpec {
    let ai = |faction: &str, start: u8| PlayerSpec {
        faction: faction.to_owned(),
        ai: true,
        difficulty: AiDifficulty::Normal,
        start,
        team: 0,
        color: -1,
        cash: None,
    };
    LoadSpec { map: "Alpine Assault".to_owned(), seed: 1, players: vec![ai("FactionGLA", 0), ai("FactionAmerica", 1)] }
}

#[test]
fn alpine_300_frames_match_a_direct_run() {
    let installs = Installs::detect();
    let Some(zh) = &installs.zero_hour else {
        eprintln!("skipped: no Zero Hour install found; set ZR_ZEROHOUR_DIR");
        return;
    };
    let spec = an3();
    let snap_path = temp_path("an3.snap");
    let trace_path = temp_path("an3.rng");
    let mut session = Session::new();
    session
        .load(&spec, Some(SnapshotSpec { path: snap_path.clone(), every: 30, bits: false }))
        .expect("load Alpine Assault AN3");

    // The per-draw RNG trace, as the divergence tooling expects it (`tools/rng_diff.py` format).
    let start_trace = format!(r#"{{"cmd":"rng_trace","path":{}}}"#, serde_json::json!(trace_path));
    let reply = protocol::handle_line(&mut session, &start_trace).0;
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(reply["traced"], true);

    // The dozers exist before any frame and the query reads them (the smoke run's "query the dozers").
    let start = protocol::handle_line(&mut session, r#"{"cmd":"query","what":"objects","kind":"DOZER"}"#).0;
    assert_eq!(start["ok"], true, "{start}");
    assert!(!start["objects"].as_array().expect("objects").is_empty(), "AN3 starts with dozers/workers: {start}");

    let step = protocol::handle_line(&mut session, r#"{"cmd":"step","frames":300}"#).0;
    assert_eq!(step["ok"], true, "{step}");
    assert_eq!(step["frame"], 300);
    let crc = session.crc_u32().expect("crc");

    let dozers = protocol::handle_line(&mut session, r#"{"cmd":"query","what":"objects","kind":"DOZER"}"#).0;
    let dozers = dozers["objects"].as_array().expect("dozers").clone();
    assert!(!dozers.is_empty(), "query after 300 frames");

    // A direct run of the same setup (same data, a fresh VFS mount) must land on the same state.
    let data = session.game().expect("loaded").data().clone();
    let vfs = Vfs::zero_hour(zh).expect("mounting Zero Hour");
    let mut direct = Game::new_skirmish_with(data, &vfs, &spec.map, &spec.setups(), spec.seed).expect("direct AN3 run");
    direct.run_frames(300);
    assert_eq!(direct.crc(), crc, "the harness session and a direct zr-game run diverged at frame 300");
    assert_eq!(direct.frame(), 300);

    let mut direct_dozers: Vec<_> = direct
        .logic()
        .objects()
        .filter(|o| o.is_kind_of(zr_sim::template::kind_of::DOZER))
        .map(|o| (o.id.0, o.template.name.clone(), o.player, o.position.x, o.position.y, o.position.z, o.health()))
        .collect();
    direct_dozers.sort_by_key(|d| d.0);
    assert_eq!(dozers.len(), direct_dozers.len(), "dozer count");
    for (q, d) in dozers.iter().zip(&direct_dozers) {
        assert_eq!(q["id"].as_u64().expect("id") as u32, d.0);
        assert_eq!(q["template"], d.1);
        assert_eq!(q["owner"].as_u64().expect("owner") as u8, d.2);
        let pos = q["pos"].as_array().expect("pos");
        assert!((pos[0].as_f64().expect("x") - f64::from(d.3)).abs() < 1e-3);
        assert!((pos[1].as_f64().expect("y") - f64::from(d.4)).abs() < 1e-3);
        assert!((pos[2].as_f64().expect("z") - f64::from(d.5)).abs() < 1e-3);
    }

    // Players, AI and scripts are readable on real data too.
    for what in ["players", "ai", "scripts"] {
        let reply = protocol::handle_line(&mut session, &format!(r#"{{"cmd":"query","what":"{what}"}}"#)).0;
        assert_eq!(reply["ok"], true, "{what}: {reply}");
    }

    // The streaming writer reached frame 300 (every 30 frames) and wrote it once.
    let text = std::fs::read_to_string(&snap_path).expect("snapshot file");
    assert!(text.starts_with("SNAP f=0 FRAME "), "frame 0 is written before the first update");
    assert_eq!(text.lines().filter(|l| l.starts_with("SNAP f=300 FRAME ")).count(), 1);
    assert!(text.lines().any(|l| l.starts_with("SNAP f=270 FRAME ")));
    assert!(text.lines().any(|l| l.contains(" O id=") && l.contains(" tmpl=")));

    // The RNG trace recorded the logic draws of the run with the documented line format.
    let stop = protocol::handle_line(&mut session, r#"{"cmd":"rng_trace","off":true}"#).0;
    assert_eq!(stop["ok"], true, "{stop}");
    let trace = std::fs::read_to_string(&trace_path).expect("rng trace");
    assert!(trace.lines().count() > 10, "AN3 draws during the run: {} lines", trace.lines().count());
    assert!(trace.lines().next().is_some_and(|l| l.starts_with("RNG f=0 n=0 ")), "{}", trace.lines().next().unwrap_or(""));
    assert!(trace.lines().all(|l| l.starts_with("RNG f=") && l.contains(" k=") && l.contains(" at=")));

    let _ = std::fs::remove_file(&snap_path);
    let _ = std::fs::remove_file(&trace_path);
}
