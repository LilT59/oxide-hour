//! `GameRandom::set_trace_file` (the runtime RNG-trace toggle behind the harness's `rng_trace`):
//! the file gets one line per draw with the frame the caller passes, and starting/stopping the trace
//! leaves the generator state (and so the game) untouched.

use std::path::PathBuf;

use zr_sim::GameRandom;

fn temp_path(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
    p.push(format!("zr-harness-{}-{nanos}-{name}", std::process::id()));
    p
}

#[test]
fn runtime_trace_records_draws_and_keeps_the_generator() {
    let path = temp_path("rng.txt");
    let mut plain = GameRandom::new(7);
    let mut traced = GameRandom::new(7);
    traced.set_trace_file(Some(&path), 42).expect("start the trace");
    assert!(traced.is_traced());

    let expected: Vec<i32> = (0..200).map(|_| plain.value(-5, 11)).collect();
    let recorded: Vec<i32> = (0..200).map(|_| traced.value(-5, 11)).collect();
    assert_eq!(recorded, expected, "the traced draws are the same values");
    assert_eq!(plain.state(), traced.state(), "tracing must not change the generator");

    // `GameLogic::update` calls this each frame and it flushes the buffered lines.
    traced.set_trace_frame(42);
    let text = std::fs::read_to_string(&path).expect("trace file");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 200, "one line per advancing draw");
    assert!(lines[0].starts_with("RNG f=42 n=0 k=I lo=-5 hi=11 r="), "{}", lines[0]);
    assert!(lines[199].starts_with("RNG f=42 n=199 "), "{}", lines[199]);
    assert!(lines.iter().all(|l| l.contains(" at=")));

    // Stopping writes no further lines and leaves the generator where it was.
    let mut expect = GameRandom::new(7);
    for _ in 0..200 {
        expect.value(-5, 11);
    }
    traced.set_trace_file(None, 0).expect("stop the trace");
    assert!(!traced.is_traced());
    let before = std::fs::read(&path).expect("trace file").len();
    for _ in 0..100 {
        assert_eq!(traced.value(0, 9), expect.value(0, 9));
    }
    assert_eq!(traced.state(), expect.state());
    assert_eq!(std::fs::read(&path).expect("trace file").len(), before, "nothing is written after the stop");
    let _ = std::fs::remove_file(&path);
}
