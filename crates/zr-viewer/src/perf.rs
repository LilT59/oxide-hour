//! `ZR_PERF=1` viewer diagnostics: one `info!` line every ~5 s with the frame time and FPS, the
//! sim's cost per logic frame (`GameLogic::update` via `Game::step`, and the per-step object
//! snapshot the viewer takes), entity and asset counts, the live object count and the process
//! working set on Windows.
//!
//! Off unless `ZR_PERF` is set. The sim timers cost one `OnceLock` read per logic step when off.
//! Nothing here touches sim state; the timers only read the clock.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use bevy::prelude::*;

use crate::sim_view::SimRes;

/// Seconds between log lines.
const REPORT_SECS: f64 = 5.0;

/// `ZR_PERF` is set (read once).
pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("ZR_PERF").is_some_and(|v| !v.is_empty() && v != "0"))
}

/// The window's present mode. `ZR_PERF=uncapped` turns vsync off, so the frame time shows the real
/// cost; with vsync the frame is pinned at the display interval (16.7 ms at 60 Hz).
pub fn present_mode() -> bevy::window::PresentMode {
    if std::env::var("ZR_PERF").is_ok_and(|v| v == "uncapped") {
        bevy::window::PresentMode::AutoNoVsync
    } else {
        bevy::window::PresentMode::AutoVsync
    }
}

/// Start a timer for a sim call; `None` (and no clock read) when diagnostics are off.
pub(crate) fn start() -> Option<Instant> {
    enabled().then(Instant::now)
}

/// Accumulated cost of one timed sim call kind, drained by the logger every report.
struct Span {
    total_ns: AtomicU64,
    max_ns: AtomicU64,
    count: AtomicU64,
}

impl Span {
    fn record(&self, d: Duration) {
        let ns = d.as_nanos().min(u64::MAX as u128) as u64;
        self.total_ns.fetch_add(ns, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
        self.max_ns.fetch_max(ns, Ordering::Relaxed);
    }

    /// `(count, avg ms, max ms)` since the last drain, then resets.
    fn drain(&self) -> (u64, f64, f64) {
        let total = self.total_ns.swap(0, Ordering::Relaxed);
        let count = self.count.swap(0, Ordering::Relaxed);
        let max = self.max_ns.swap(0, Ordering::Relaxed);
        let avg = if count > 0 { total as f64 / count as f64 / 1e6 } else { 0.0 };
        (count, avg, max as f64 / 1e6)
    }
}

/// `Game::step` (one logic frame of the sim).
static STEP: Span = Span { total_ns: AtomicU64::new(0), max_ns: AtomicU64::new(0), count: AtomicU64::new(0) };
/// The viewer's per-step `snapshot` (every live object's position, rebuilt each logic frame).
static SNAPSHOT: Span = Span { total_ns: AtomicU64::new(0), max_ns: AtomicU64::new(0), count: AtomicU64::new(0) };

/// Records one `Game::step` that started at `t` (from [`start`]).
pub(crate) fn record_step(t: Option<Instant>) {
    if let Some(t) = t {
        STEP.record(t.elapsed());
    }
}

/// Records one object snapshot that started at `t` (from [`start`]).
pub(crate) fn record_snapshot(t: Option<Instant>) {
    if let Some(t) = t {
        SNAPSHOT.record(t.elapsed());
    }
}

/// Frame-time accumulator for the current report window.
#[derive(Resource, Default)]
struct FrameWindow {
    secs: f64,
    frames: u32,
    max_secs: f64,
}

pub fn add_systems(app: &mut App) {
    if !enabled() {
        return;
    }
    app.init_resource::<FrameWindow>();
    app.add_systems(Update, report);
}

/// `ZR_PERF` line: frame time and FPS over the window, sim cost per logic frame, counts.
#[allow(clippy::too_many_arguments)]
fn report(
    time: Res<Time<Real>>,
    mut win: ResMut<FrameWindow>,
    mut last_report: Local<Option<f64>>,
    sim: Option<NonSend<SimRes>>,
    entities: Query<Entity>,
    meshes: Res<Assets<Mesh>>,
    images: Res<Assets<Image>>,
    materials: Res<Assets<StandardMaterial>>,
) {
    // The first Update has a zero delta; start the window there.
    let dt = time.delta_secs_f64();
    win.secs += dt;
    win.frames += 1;
    win.max_secs = win.max_secs.max(dt);
    let now = time.elapsed_secs_f64();
    let since = *last_report.get_or_insert(now);
    if now - since < REPORT_SECS {
        return;
    }
    *last_report = Some(now);
    let frames = win.frames.max(1);
    let avg_ms = win.secs / frames as f64 * 1000.0;
    let fps = if win.secs > 0.0 { frames as f64 / win.secs } else { 0.0 };
    let (steps, step_avg, step_max) = STEP.drain();
    let (_, snap_avg, snap_max) = SNAPSHOT.drain();
    let (objects, logic_frame) = sim.as_ref().map_or((0, 0), |s| (s.game.objects().count(), s.game.frame()));
    info!(
        "perf: frame avg {avg_ms:.2} ms, max {:.2} ms, {fps:.1} fps ({frames} frames) | sim step avg {step_avg:.3} ms, max {step_max:.3} ms over {steps} logic frames (frame {logic_frame}) | snapshot avg {snap_avg:.3} ms, max {snap_max:.3} ms | objects {objects}, entities {}, meshes {}, images {}, materials {}{}",
        win.max_secs * 1000.0,
        entities.iter().count(),
        meshes.len(),
        images.len(),
        materials.len(),
        process_memory_mb().map_or(String::new(), |mb| format!(", working set {mb} MB")),
    );
    *win = FrameWindow::default();
}

/// The process working set in MiB (`K32GetProcessMemoryInfo`, kernel32; Windows only).
#[cfg(windows)]
fn process_memory_mb() -> Option<u64> {
    #[repr(C)]
    #[allow(non_snake_case)]
    struct ProcessMemoryCounters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
    }
    extern "system" {
        fn GetCurrentProcess() -> isize;
        fn K32GetProcessMemoryInfo(process: isize, counters: *mut ProcessMemoryCounters, cb: u32) -> i32;
    }
    let mut c = std::mem::MaybeUninit::<ProcessMemoryCounters>::zeroed();
    let size = std::mem::size_of::<ProcessMemoryCounters>() as u32;
    // SAFETY: `c` is a zeroed, correctly laid out `PROCESS_MEMORY_COUNTERS` whose `cb` is set to its
    // size, as the call requires; the current-process pseudo handle needs no close.
    unsafe {
        (*c.as_mut_ptr()).cb = size;
        if K32GetProcessMemoryInfo(GetCurrentProcess(), c.as_mut_ptr(), size) == 0 {
            return None;
        }
        Some((*c.as_ptr()).working_set_size as u64 / (1024 * 1024))
    }
}

#[cfg(not(windows))]
fn process_memory_mb() -> Option<u64> {
    None
}
