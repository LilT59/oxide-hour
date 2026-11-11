//! Replay round-trip against the player's Zero Hour install (skips when none is found): record a
//! scripted AI-vs-AI + human-commands run, encode/decode it, replay it and compare the CRCs.

use std::sync::Arc;

use zr_assets::{Installs, Vfs};
use zr_game::replay::{Replay, ReplayHeader};
use zr_game::{Game, GameData, PlayerSetup};
use zr_sim::{Command, Coord3D};

const FRAMES: u32 = 3600; // two minutes at 30 Hz
const CRC_INTERVAL: u32 = 30;
const COMMAND_EVERY: u32 = 150;

fn vfs() -> Option<Vfs> {
    let zh = Installs::detect().zero_hour?;
    Vfs::zero_hour(&zh).ok()
}

fn setups() -> Vec<PlayerSetup> {
    vec![PlayerSetup::ai("FactionAmerica", 0), PlayerSetup::ai("FactionChina", 1)]
}

/// A deterministic scripted command for `frame` (moves the America dozer around).
fn scripted(game: &Game, frame: u32) -> Option<Command> {
    let dozer = game.spawned().iter().find(|s| s.player == 2 && s.template.contains("Dozer")).map(|s| s.id)?;
    let (w, h) = game.logic().terrain().world_size();
    let t = frame as f32 / 30.0;
    let x = w / 2.0 + (t * 0.7).sin() * 300.0;
    let y = h / 2.0 + (t * 0.5).cos() * 300.0;
    Some(Command::MoveTo { ids: vec![dozer], pos: Coord3D::new(x, y, 0.0) })
}

fn run(data: &Arc<GameData>, vfs: &Vfs) -> (Replay, Vec<(u32, u32)>) {
    let mut game = Game::new_skirmish_with(data.clone(), vfs, "Alpine Assault", &setups(), 7).unwrap();
    let header = ReplayHeader {
        map: "Alpine Assault".into(),
        seed: 7,
        starting_cash: 10000,
        superweapon_restriction: 0,
        slots: setups(),
    };
    let mut replay = Replay::new(header, CRC_INTERVAL);
    let mut crcs = Vec::new();
    for frame in 0..FRAMES {
        if frame % COMMAND_EVERY == 0 {
            if let Some(cmd) = scripted(&game, frame) {
                game.submit(2, cmd.clone());
                replay.record(frame, 2, cmd);
            }
        }
        game.step();
        if frame % CRC_INTERVAL == 0 {
            let crc = game.crc();
            replay.record_crc(frame, crc);
            crcs.push((frame, crc));
        }
    }
    (replay, crcs)
}

#[test]
fn replay_round_trips_and_reproduces_crcs() {
    let Some(vfs) = vfs() else { return };
    let data = Arc::new(GameData::load(&vfs));

    // Record a run.
    let (replay, recorded) = run(&data, &vfs);
    assert!(!replay.events.is_empty(), "recorded some commands");
    assert!(recorded.len() > 10, "recorded several CRCs");

    // The binary format round-trips.
    let bytes = replay.encode();
    let decoded = Replay::decode(&bytes).expect("decode");
    assert_eq!(decoded, replay, "replay encode/decode round-trip");

    // Replay the decoded stream; the CRCs must match the recording.
    let mut game = Game::new_skirmish_with(data.clone(), &vfs, "Alpine Assault", &decoded.header.slots, decoded.header.seed).unwrap();
    let by_frame = decoded.by_frame();
    let mut replayed_crcs = Vec::new();
    for frame in 0..FRAMES {
        if let Some(events) = by_frame.get(&frame) {
            for e in events {
                game.submit(e.player, e.command.clone());
            }
        }
        game.step();
        if frame % decoded.crc_interval == 0 {
            replayed_crcs.push((frame, game.crc()));
        }
    }
    assert_eq!(replayed_crcs, decoded.crcs, "replayed CRCs match the recording");
    assert_eq!(replayed_crcs, recorded, "replayed CRCs match the live run");
}
