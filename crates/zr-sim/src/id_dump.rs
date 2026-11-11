//! Object-id allocation dump (`ZR_ID_DUMP=<path>`), the port side of the patched C++ build's read-only
//! `ZR_L64_ID` printf in `Object::Object` (`zh-cpp-build/src-patched/.../Object.cpp`, after
//! `setID(TheGameLogic->allocateObjectID())`).
//!
//! One line per allocated id, in allocation order:
//! `ZR_L64_ID id=<id> f=<frame> tmpl=<template> at=<file>:<line>`.
//! `GameLogic::allocate_object_id` numbers every id the same way C++ `GameLogic::allocateObjectID`
//! does (`GameLogic.cpp:4069-4075`), including the ids of things the sim keeps as records rather
//! than objects (projectiles, laser beams, projectile streams: `ThingFactory::newObject` in
//! `Weapon.cpp:1105`, `:2515`, `:2543`), so the sequences are directly comparable.
//!
//! `ZR_ID_DUMP_MAX` caps the dump (default 400 ids): the divergence tooling cares about start-up and
//! the early game. The dump reads and writes nothing the simulation depends on, so the CRC is the
//! same with it on or off. Off (env unset): one branch per allocation.

use std::io::{BufWriter, Write};
use std::panic::Location;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

/// Environment variable that names the dump file; read once per process.
const DUMP_ENV: &str = "ZR_ID_DUMP";
/// Environment variable that caps the dumped id (inclusive); default [`DEFAULT_MAX`].
const MAX_ENV: &str = "ZR_ID_DUMP_MAX";
const DEFAULT_MAX: u32 = 400;

struct Dump {
    sink: Box<dyn Write + Send>,
    max: u32,
}

/// Process-wide dump sink. The game runs one logic world at a time (a second `GameLogic` truncates and
/// restarts the dump, as the RNG trace does).
static DUMP: Mutex<Option<Dump>> = Mutex::new(None);
static DUMP_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();

fn lock_dump() -> MutexGuard<'static, Option<Dump>> {
    DUMP.lock().unwrap_or_else(PoisonError::into_inner)
}

fn max_from_env() -> u32 {
    std::env::var(MAX_ENV).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(DEFAULT_MAX)
}

/// Starts the dump for a new logic world when `ZR_ID_DUMP` is set. Unset: no-op.
pub(crate) fn enable_from_env() {
    let path = DUMP_PATH.get_or_init(|| std::env::var_os(DUMP_ENV).map(PathBuf::from));
    let Some(path) = path else { return };
    match std::fs::File::create(path) {
        Ok(f) => *lock_dump() = Some(Dump { sink: Box::new(BufWriter::new(f)), max: max_from_env() }),
        Err(e) => eprintln!("{DUMP_ENV}: cannot create {}: {e}", path.display()),
    }
}

/// Records one allocation. `loc` is the caller's location (passed in because the interesting caller is
/// the object-creation entry point, not `allocate_object_id` itself).
pub(crate) fn record(id: u32, frame: u32, template: &str, loc: &Location<'_>) {
    if let Some(d) = lock_dump().as_mut() {
        if id > d.max {
            return;
        }
        let line = format!("ZR_L64_ID id={id} f={frame} tmpl={template} at={}:{}\n", loc.file(), loc.line());
        // A dump that cannot be written must not change the game, so a write error only drops the line.
        let _ = d.sink.write_all(line.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shared sink so the test can read the dump back.
    #[derive(Clone)]
    struct SharedBuf(std::sync::Arc<Mutex<Vec<u8>>>);

    impl Write for SharedBuf {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn the_dump_caps_ids_and_records_the_caller() {
        let buf = std::sync::Arc::new(Mutex::new(Vec::new()));
        *lock_dump() = Some(Dump { sink: Box::new(SharedBuf(buf.clone())), max: 5 });
        let loc = Location::caller();
        record(5, 3, "SomeTemplate", loc);
        record(6, 3, "TooHigh", loc);
        let text = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        *lock_dump() = None;
        // The dump is process-wide, so tests running in parallel may add their own lines while it is installed;
        // only this test's templates are checked.
        let ours: String = text
            .lines()
            .filter(|l| l.contains("tmpl=SomeTemplate ") || l.contains("tmpl=TooHigh "))
            .map(|l| format!("{l}\n"))
            .collect();
        assert_eq!(ours, format!("ZR_L64_ID id=5 f=3 tmpl=SomeTemplate at={}:{}\n", loc.file(), loc.line()));
    }
}
