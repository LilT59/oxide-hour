//! Seeded logic RNG, ported exactly from `Core/GameEngine/Source/Common/RandomValue.cpp`
//! (`seedRandom`, `randomValue`, `GetGameLogicRandomValue[Real]`).
//!
//! The generator is a 6-word add-with-carry lagged generator. The original's quirks are kept on
//! purpose: the `ADC` macro is expanded with `SUM == A` for the later steps, so the carry there is
//! computed from `(sum < b)` only, and the increment-overflow path bumps the returned value.
//! Degenerate ranges follow the `RETAIL_COMPATIBLE_CRC` branch the build uses (`GameDefines.h:103`):
//! the integer draw returns without advancing only when `hi - lo + 1 == 0` in unsigned arithmetic,
//! and the real draw only when `hi - lo <= 0`.
//!
//! **RNG trace (divergence tooling).** With `ZR_RNG_TRACE=<path>` set, the logic stream appends one
//! line per draw that advances the generator, matching the patched C++ build's `ZR_RNG_TRACE`
//! (`zh-cpp-build/src-patched`, `RandomValue.cpp`) so `tools/rng_diff.py` can align the two:
//! `RNG f=<frame> n=<draw index> k=I lo=<int> hi=<int> r=<int> at=<file>:<line>` for integer draws and
//! `... k=R lo=<hex> hi=<hex> r=<hex>` for real ones, where the hex fields are the f32 bit patterns.
//! Only the logic world's generator is traced (`GameRandom::with_env_trace`, called from
//! `GameLogic::new`); the client and audio streams never are. The trace reads and writes nothing
//! the generator depends on, so the state and CRC are the same with it on or off.

use std::io::{BufWriter, Write};
use std::panic::Location;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

/// `theMultFactor = 1.0f / UINT_MAX`.
const MULT_FACTOR: f32 = 1.0 / (u32::MAX as f32);

const INIT: [u32; 6] = [0xf22d0e56, 0x883126e9, 0xc624dd2f, 0x0702c49c, 0x9e353f7d, 0x6fdf3b64];

/// Environment variable that names the RNG trace file; read once per process.
const TRACE_ENV: &str = "ZR_RNG_TRACE";

/// The open logic trace: its sink, the frame the draws are recorded under and the draw count.
struct TraceState {
    sink: Box<dyn Write + Send>,
    frame: u32,
    draws: u64,
}

/// Process-wide trace sink. The game runs one logic world at a time, so one sink is enough.
static TRACE: Mutex<Option<TraceState>> = Mutex::new(None);
static TRACE_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();

fn lock_trace() -> MutexGuard<'static, Option<TraceState>> {
    TRACE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Appends one trace line for a draw made at `loc` (the caller's location, see `#[track_caller]`).
fn trace_draw(loc: &Location<'_>, kind: char, fields: String) {
    if let Some(t) = lock_trace().as_mut() {
        let line = format!("RNG f={} n={} k={kind} {fields} at={}:{}\n", t.frame, t.draws, loc.file(), loc.line());
        // A trace that cannot be written must not change the game, so a write error only drops the line.
        let _ = t.sink.write_all(line.as_bytes());
        t.draws += 1;
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameRandom {
    seed: [u32; 6],
    base_seed: u32,
    /// Records this generator's draws in the process trace. Set only by [`GameRandom::with_env_trace`];
    /// a clone of a traced generator writes to the same trace.
    traced: bool,
}

impl Default for GameRandom {
    fn default() -> Self {
        Self::new(0)
    }
}

impl GameRandom {
    /// `InitRandom(seed)` for the logic stream.
    pub fn new(seed: u32) -> Self {
        let mut s = [0u32; 6];
        seed_random(seed, &mut s);
        Self { seed: s, base_seed: seed, traced: false }
    }

    /// Starts the RNG trace for this (logic) generator when `ZR_RNG_TRACE` is set. Every call creates
    /// the file afresh and restarts the draw count at 0, so a process that builds several logic worlds
    /// (the skirmish demos run the same game twice) leaves the trace of the last one. Unset: no-op.
    pub fn with_env_trace(mut self) -> Self {
        let path = TRACE_PATH.get_or_init(|| std::env::var_os(TRACE_ENV).map(PathBuf::from));
        if let Some(path) = path {
            match std::fs::File::create(path) {
                Ok(f) => {
                    let sink: Box<dyn Write + Send> = Box::new(BufWriter::new(f));
                    *lock_trace() = Some(TraceState { sink, frame: 0, draws: 0 });
                    self.traced = true;
                }
                Err(e) => eprintln!("{TRACE_ENV}: cannot create {}: {e}", path.display()),
            }
        }
        self
    }

    /// Starts, replaces or stops the process trace sink at runtime (the control harness's `rng_trace`
    /// command). `Some(path)` creates (truncates) the file and records the following draws under
    /// `frame` (the game's current logic frame, so a mid-game toggle labels its lines correctly);
    /// `None` drops the sink. Like [`GameRandom::with_env_trace`], tracing reads and writes nothing the
    /// generator depends on, so the state and CRC are the same with it on or off.
    pub fn set_trace_file(&mut self, path: Option<&std::path::Path>, frame: u32) -> std::io::Result<()> {
        match path {
            Some(path) => {
                let sink: Box<dyn Write + Send> = Box::new(BufWriter::new(std::fs::File::create(path)?));
                *lock_trace() = Some(TraceState { sink, frame, draws: 0 });
                self.traced = true;
            }
            None => {
                *lock_trace() = None;
                self.traced = false;
            }
        }
        Ok(())
    }

    /// Whether this generator's draws are recorded in the process trace.
    pub fn is_traced(&self) -> bool {
        self.traced
    }

    /// Sets the frame later draws are recorded under (the logic frame counter, `GameLogic::getFrame`) and
    /// flushes the lines of the frame that ended. No-op unless traced.
    pub fn set_trace_frame(&mut self, frame: u32) {
        if self.traced {
            if let Some(t) = lock_trace().as_mut() {
                t.frame = frame;
                let _ = t.sink.flush();
            }
        }
    }

    /// `GetGameLogicRandomSeed`.
    pub fn base_seed(&self) -> u32 {
        self.base_seed
    }

    pub fn state(&self) -> &[u32; 6] {
        &self.seed
    }

    /// Raw 32-bit output (`randomValue`).
    pub fn next_u32(&mut self) -> u32 {
        random_value(&mut self.seed)
    }

    /// `GetGameLogicRandomValue(lo, hi)` (`RandomValue.cpp:267`, `RETAIL_COMPATIBLE_CRC` = 1):
    /// inclusive range. Only `hi - lo + 1 == 0` (unsigned) returns `hi` without drawing. A one-value
    /// range (`lo == hi`) and a reversed one (`lo > hi`, the delta wrapping as the C++ `UnsignedInt`)
    /// both advance the generator, as every C++ call does.
    /// `#[track_caller]` so the trace records the caller's file:line, not this function's.
    #[track_caller]
    pub fn value(&mut self, lo: i32, hi: i32) -> i32 {
        let delta = (hi.wrapping_sub(lo) as u32).wrapping_add(1);
        if delta == 0 {
            return hi;
        }
        let r = ((self.next_u32() % delta) as i32).wrapping_add(lo);
        if self.traced {
            trace_draw(Location::caller(), 'I', format!("lo={lo} hi={hi} r={r}"));
        }
        r
    }

    /// `GetGameLogicRandomValueReal(lo, hi)` (`RandomValue.cpp:294-298`, retail branch): returns `hi`
    /// without drawing only when `hi - lo <= 0`.
    #[track_caller]
    pub fn value_real(&mut self, lo: f32, hi: f32) -> f32 {
        let delta = hi - lo;
        if delta <= 0.0 {
            return hi;
        }
        let r = (self.next_u32() as f32 * MULT_FACTOR) * delta + lo;
        if self.traced {
            let bits = |f: f32| format!("{:08x}", f.to_bits());
            trace_draw(Location::caller(), 'R', format!("lo={} hi={} r={}", bits(lo), bits(hi), bits(r)));
        }
        r
    }

    /// Hash of the generator state (`GetGameLogicRandomSeedCRC`).
    pub fn crc(&self, c: &mut crate::crc::Crc) {
        for w in self.seed {
            c.add_u32(w);
        }
    }
}

fn seed_random(seed: u32, out: &mut [u32; 6]) {
    let mut ax = seed.wrapping_add(INIT[0]);
    out[0] = ax;
    for i in 1..6 {
        ax = ax.wrapping_add(INIT[i].wrapping_sub(INIT[i - 1]));
        out[i] = ax;
    }
}

fn random_value(seed: &mut [u32; 6]) -> u32 {
    // ADC(ax, seed[5], seed[4], c): distinct operands, so the full carry rule applies.
    let mut ax = seed[5].wrapping_add(seed[4]);
    let mut c = u32::from(ax < seed[5] || ax < seed[4]);
    seed[4] = ax;
    // ADC(ax, ax, seed[k], c): SUM aliases A, so `SUM < A` is always false.
    ax = ax.wrapping_add(seed[3]).wrapping_add(c);
    c = u32::from(ax < seed[3]);
    seed[3] = ax;
    ax = ax.wrapping_add(seed[2]).wrapping_add(c);
    c = u32::from(ax < seed[2]);
    seed[2] = ax;
    ax = ax.wrapping_add(seed[1]).wrapping_add(c);
    c = u32::from(ax < seed[1]);
    seed[1] = ax;
    ax = ax.wrapping_add(seed[0]).wrapping_add(c);
    seed[0] = ax;

    // Increment the seed array, bubbling up the carries.
    seed[5] = seed[5].wrapping_add(1);
    if seed[5] == 0 {
        seed[4] = seed[4].wrapping_add(1);
        if seed[4] == 0 {
            seed[3] = seed[3].wrapping_add(1);
            if seed[3] == 0 {
                seed[2] = seed[2].wrapping_add(1);
                if seed[2] == 0 {
                    seed[1] = seed[1].wrapping_add(1);
                    if seed[1] == 0 {
                        seed[0] = seed[0].wrapping_add(1);
                        ax = ax.wrapping_add(1);
                    }
                }
            }
        }
    }
    ax
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeding_zero_matches_original_table() {
        assert_eq!(GameRandom::new(0).seed, INIT);
    }

    #[test]
    fn first_output_hand_checked() {
        // Step 1 of the C++: 6fdf3b64 + 9e353f7d = 0e147ae1 (with carry).
        assert_eq!(0x6fdf3b64u32.wrapping_add(0x9e353f7d), 0x0e147ae1);
        let mut s = INIT;
        let out = random_value(&mut s);
        assert_eq!(s[4], 0x0e147ae1);
        assert_eq!(s[5], INIT[5].wrapping_add(1));
        assert_eq!(s[0], out);
    }

    #[test]
    fn same_seed_same_stream() {
        let mut a = GameRandom::new(1234);
        let mut b = GameRandom::new(1234);
        for _ in 0..1000 {
            assert_eq!(a.value(-5, 17), b.value(-5, 17));
        }
        assert_eq!(a, b);
        assert_ne!(GameRandom::new(1), GameRandom::new(2));
    }

    #[test]
    fn range_respected_and_real_degenerate_returns_hi() {
        let mut r = GameRandom::new(7);
        for _ in 0..1000 {
            assert!((3..=9).contains(&r.value(3, 9)));
            assert!((1.0..=2.0).contains(&r.value_real(1.0, 2.0)));
        }
        let before = r.clone();
        assert_eq!(r.value_real(5.0, 5.0), 5.0);
        assert_eq!(r.value_real(6.0, 5.0), 5.0);
        assert_eq!(r, before, "real degenerate range (hi - lo <= 0) must not advance the generator");
    }

    #[test]
    fn retail_one_value_range_still_draws() {
        // RETAIL_COMPATIBLE_CRC: only `hi - lo + 1 == 0` returns early, so `lo == hi` advances.
        let mut r = GameRandom::new(7);
        let before = r.clone();
        assert_eq!(r.value(0, 0), 0);
        assert_ne!(r, before, "a one-value integer range advances the generator");
    }

    #[test]
    fn reversed_integer_range_draws_with_unsigned_delta() {
        // `delta = hi - lo + 1` is `0xFFFF_FFFF` for (5, 3) in C++ `UnsignedInt` arithmetic, so the
        // draw is `r % 0xFFFF_FFFF + lo` and the generator advances.
        let mut a = GameRandom::new(7);
        let mut b = GameRandom::new(7);
        let raw = b.next_u32();
        assert_eq!(a.value(5, 3), ((raw % 0xFFFF_FFFF) as i32).wrapping_add(5));
        assert_eq!(a, b);
    }

    #[test]
    fn zero_delta_returns_hi_without_draw() {
        // `hi - lo + 1 == 0`: (0, -1), the empty `GetGameLogicRandomValue(0, count - 1)` of a list.
        let mut r = GameRandom::new(7);
        let before = r.clone();
        assert_eq!(r.value(0, -1), -1);
        assert_eq!(r, before);
    }

    /// Shared sink so the test can read the trace back.
    #[derive(Clone, Default)]
    struct SharedBuf(std::sync::Arc<Mutex<Vec<u8>>>);

    impl Write for SharedBuf {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap_or_else(PoisonError::into_inner).extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    // One test does both checks: the trace is process-wide, so parallel tests would share it.
    #[test]
    fn traced_draws_match_untraced_and_record_the_call_site() {
        let buf = SharedBuf::default();
        *lock_trace() = Some(TraceState { sink: Box::new(buf.clone()), frame: 0, draws: 0 });
        let mut plain = GameRandom::new(42);
        let mut traced = GameRandom::new(42);
        traced.traced = true;
        let mut expect_lines = 0;
        for i in 0..200 {
            if i == 100 {
                traced.set_trace_frame(7);
                plain.set_trace_frame(7);
            }
            assert_eq!(plain.value(-3, 11), traced.value(-3, 11));
            assert_eq!(plain.value(4, 4), traced.value(4, 4));
            assert_eq!(plain.value_real(0.25, 9.5).to_bits(), traced.value_real(0.25, 9.5).to_bits());
            // Degenerate ranges draw nothing and write no line.
            assert_eq!(plain.value_real(2.0, 2.0), traced.value_real(2.0, 2.0));
            assert_eq!(plain.value(5, 3), traced.value(5, 3));
            expect_lines += 4;
        }
        assert_eq!(plain.state(), traced.state(), "tracing must not change the generator");
        assert_eq!(plain.value(0, 1000), traced.value(0, 1000));
        expect_lines += 1;
        // The line for the draw on the line below records this test's call site and the frame set above.
        let expect_line = line!() + 1;
        let r = traced.value(0, 9);
        assert_eq!(r, plain.value(0, 9));
        expect_lines += 1;
        let text = String::from_utf8(buf.0.lock().unwrap_or_else(PoisonError::into_inner).clone()).expect("ascii");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), expect_lines);
        assert!(lines[0].starts_with("RNG f=0 n=0 k=I lo=-3 hi=11 r="), "{}", lines[0]);
        let last = lines[lines.len() - 1];
        assert!(last.starts_with("RNG f=7 n="), "{last}");
        // The path form depends on how cargo invokes rustc, so only the file name and line are fixed.
        assert!(last.ends_with(&format!("random.rs:{expect_line}")), "{last}");
        assert!(lines.iter().any(|l| l.contains(" k=R lo=")), "real draws are traced in hex bit form");
        *lock_trace() = None;
    }
}
