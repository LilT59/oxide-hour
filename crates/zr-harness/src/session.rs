//! The harness session: the loaded game plus the read/command/step API behind the JSON protocol.
//!
//! One [`Session`] owns the shared [`GameData`] (loaded once per process from the install), the
//! optional [`Vfs`] it came from (so `load` can start another map without reloading the INI) and the
//! current [`Game`]. Everything the protocol exposes is a method here, so tests and other tools drive
//! the same code as the `zr-harness` binary.
//!
//! Only [`submit`](Session::submit) (and [`step`](Session::step), which advances time) changes the
//! simulation; every query reads it, matching the client-only-reads rule (AGENTS.md).

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Map, Value};

use zr_assets::{Installs, Vfs};
use zr_game::snapshot::{self, FileSnapshot, Snapshot};
use zr_game::trace::{self, FileTrace, FrameRange, IdFilter, Trace};
use zr_game::{AiDifficulty, Game, GameData, PlayerKind, PlayerSetup};
use zr_sim::combat::AttackTarget;
use zr_sim::orders::{GuardMode, GuardTarget};
use zr_sim::skirmish_ai::TeamInQueue;
use zr_sim::template::kind_of;
use zr_sim::Command;

/// Default map name (the demo's Alpine Assault, `AN3`).
pub const DEFAULT_MAP: &str = "Alpine Assault";
/// Default logic seed (the seed the matrix CRCs were recorded with).
pub const DEFAULT_SEED: u32 = 1;
/// Longest accepted filter list (`id`, `owner`).
const MAX_FILTER: usize = 4096;

/// One requested skirmish slot (`load`'s `players[i]`).
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerSpec {
    /// `PlayerTemplate` name, e.g. `FactionGLA`.
    pub faction: String,
    pub ai: bool,
    pub difficulty: AiDifficulty,
    /// Zero-based start position; defaults to the slot's array index.
    pub start: u8,
    pub team: u8,
    pub color: i32,
    /// Per-slot starting cash (the game default when `None`).
    pub cash: Option<u32>,
}

impl PlayerSpec {
    fn from_json(v: &Value, slot: usize) -> Result<Self, String> {
        let o = v.as_object().ok_or_else(|| format!("load: players[{slot}] must be an object"))?;
        let faction = o
            .get("faction")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("load: players[{slot}] needs a non-empty string \"faction\""))?
            .to_owned();
        let ai = match o.get("ai") {
            None | Some(Value::Null) => true,
            Some(v) => v.as_bool().ok_or_else(|| format!("load: players[{slot}] \"ai\" must be a boolean"))?,
        };
        let difficulty = match o.get("difficulty") {
            None | Some(Value::Null) => AiDifficulty::Normal,
            Some(v) => {
                let s = v.as_str().ok_or_else(|| format!("load: players[{slot}] \"difficulty\" must be a string"))?;
                match s.to_ascii_lowercase().as_str() {
                    "easy" => AiDifficulty::Easy,
                    "normal" | "medium" => AiDifficulty::Normal,
                    "hard" | "brutal" => AiDifficulty::Hard,
                    other => return Err(format!("load: players[{slot}] difficulty {other:?} is not easy, normal or hard")),
                }
            }
        };
        let start = opt_u8(o, "start")?.unwrap_or(slot as u8);
        let team = opt_u8(o, "team")?.unwrap_or(0);
        let color = opt_i32(o, "color")?.unwrap_or(-1);
        let cash = opt_u32(o, "cash")?;
        Ok(PlayerSpec { faction, ai, difficulty, start, team, color, cash })
    }

    /// The `zr-game` setup for this slot.
    pub fn setup(&self) -> PlayerSetup {
        let mut s = if self.ai { PlayerSetup::ai(&self.faction, self.start) } else { PlayerSetup::new(&self.faction, self.start) };
        s.difficulty = self.difficulty;
        s.team = self.team;
        s.color = self.color;
        s.starting_cash = self.cash;
        s
    }
}

/// A `load` request: map, seed and slots (`Game::new_skirmish_with`, the same start path the viewer
/// and `skirmish_ai_demo` use).
#[derive(Clone, Debug, PartialEq)]
pub struct LoadSpec {
    pub map: String,
    pub seed: u32,
    pub players: Vec<PlayerSpec>,
}

impl LoadSpec {
    pub fn from_json(o: &Map<String, Value>) -> Result<Self, String> {
        let map = match o.get("map") {
            None | Some(Value::Null) => DEFAULT_MAP.to_owned(),
            Some(v) => v.as_str().filter(|s| !s.is_empty()).ok_or("load: \"map\" must be a non-empty string")?.to_owned(),
        };
        let seed = opt_u32(o, "seed")?.unwrap_or(DEFAULT_SEED);
        let arr = o
            .get("players")
            .ok_or("load: missing \"players\" (an array of skirmish slots with \"faction\")")?
            .as_array()
            .ok_or("load: \"players\" must be an array")?;
        if arr.is_empty() {
            return Err("load: \"players\" must not be empty".to_owned());
        }
        if arr.len() > 8 {
            return Err(format!("load: at most 8 skirmish slots, got {}", arr.len()));
        }
        let players = arr.iter().enumerate().map(|(i, v)| PlayerSpec::from_json(v, i)).collect::<Result<Vec<_>, _>>()?;
        // A top-level `cash` is the skirmish lobby's starting-cash option: it fills every slot that
        // does not override it (per-slot `cash` still wins).
        let mut players = players;
        if let Some(cash) = opt_u32(o, "cash")? {
            for p in &mut players {
                if p.cash.is_none() {
                    p.cash = Some(cash);
                }
            }
        }
        Ok(LoadSpec { map, seed, players })
    }

    /// The `zr-game` setups, in slot order.
    pub fn setups(&self) -> Vec<PlayerSetup> {
        self.players.iter().map(PlayerSpec::setup).collect()
    }
}

/// A streaming snapshot writer (`ZR_SNAPSHOT` semantics: a block every `every` frames).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotSpec {
    pub path: PathBuf,
    pub every: u32,
    pub bits: bool,
}

impl SnapshotSpec {
    pub fn from_json(v: Option<&Value>) -> Result<Option<Self>, String> {
        let Some(v) = v else { return Ok(None) };
        if v.is_null() {
            return Ok(None);
        }
        let o = v.as_object().ok_or("snapshot: want an object {\"path\":...,\"every\":N,\"bits\":bool}")?;
        let path = o
            .get("path")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or("snapshot: missing non-empty string \"path\"")?;
        let every = opt_u32(o, "every")?.unwrap_or(snapshot::DEFAULT_EVERY);
        if every == 0 {
            return Err("snapshot: \"every\" must be a positive frame count".to_owned());
        }
        let bits = opt_bool(o, "bits")?.unwrap_or(false);
        Ok(Some(SnapshotSpec { path: PathBuf::from(path), every, bits }))
    }
}

/// A streaming per-object trace writer (`ZR_TRACE` semantics; the `trace` request).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceSpec {
    pub path: PathBuf,
    pub ids: IdFilter,
    pub frames: FrameRange,
}

impl TraceSpec {
    /// The `trace` request's `ids` (a comma list or `all`) and `frames` (`a-b` or `a`) fields; both
    /// default to everything. `ids`/`frames` are strings, as the environment variables are.
    pub fn from_json(o: &Map<String, Value>, path: PathBuf) -> Result<Self, String> {
        let ids = IdFilter::parse(opt_str(o, "ids")?).map_err(|e| format!("trace: {e}"))?;
        let frames = FrameRange::parse(opt_str(o, "frames")?).map_err(|e| format!("trace: {e}"))?;
        Ok(TraceSpec { path, ids, frames })
    }
}

/// What a query asks for (`objects`, `players`, `ai`, `scripts` or `all`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum What {
    #[default]
    Objects,
    Players,
    Ai,
    Scripts,
    All,
}

impl What {
    fn name(self) -> &'static str {
        match self {
            What::Objects => "objects",
            What::Players => "players",
            What::Ai => "ai",
            What::Scripts => "scripts",
            What::All => "all",
        }
    }
}

/// A `query` request. The filters apply to `objects` (and to the `objects`/`players`/`ai` part of
/// `all`); `player` filters players and AI players.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Query {
    pub what: What,
    /// Object ids (or a single id) to include.
    pub ids: Option<Vec<u32>>,
    /// Owning player indices (or a single index) to include.
    pub owners: Option<Vec<u8>>,
    /// Exact `ThingTemplate` name.
    pub template: Option<String>,
    /// `KindOf` bit (`template::kind_of`, by name, case-insensitive).
    pub kind: Option<u64>,
    /// Player index for `players`/`ai`.
    pub player: Option<u8>,
    /// Maximum number of object records.
    pub limit: Option<usize>,
}

impl Query {
    pub fn from_json(o: &Map<String, Value>) -> Result<Self, String> {
        let what = match o.get("what") {
            None => return Err("query: missing string field \"what\" (objects, players, ai, scripts or all)".to_owned()),
            Some(v) => {
                let s = v.as_str().ok_or("query: \"what\" must be a string")?;
                match s.to_ascii_lowercase().as_str() {
                    "objects" | "object" => What::Objects,
                    "players" | "player" => What::Players,
                    "ai" => What::Ai,
                    "scripts" | "script" => What::Scripts,
                    "all" | "state" => What::All,
                    other => return Err(format!("query: unknown \"what\" {other:?} (want objects, players, ai, scripts or all)")),
                }
            }
        };
        let ids = match o.get("id") {
            None | Some(Value::Null) => None,
            Some(v) => Some(u32_list(v, "id")?),
        };
        let owners = match o.get("owner") {
            None | Some(Value::Null) => None,
            Some(v) => Some(u8_list(v, "owner")?),
        };
        let template = match o.get("template") {
            None | Some(Value::Null) => None,
            Some(v) => Some(v.as_str().ok_or("query: \"template\" must be a string")?.to_owned()),
        };
        let kind = match o.get("kind") {
            None | Some(Value::Null) => None,
            Some(v) => {
                let s = v.as_str().ok_or("query: \"kind\" must be a string (a KindOf name)")?;
                Some(kind_of::bit_for_name(s).ok_or_else(|| format!("query: unknown kind {s:?}"))?)
            }
        };
        let player = match o.get("player") {
            None | Some(Value::Null) => None,
            Some(v) => Some(u8_field(v, "player")?),
        };
        let limit = match o.get("limit") {
            None | Some(Value::Null) => None,
            Some(v) => Some(
                v.as_u64()
                    .and_then(|n| usize::try_from(n).ok())
                    .ok_or("query: \"limit\" must be a non-negative integer")?,
            ),
        };
        Ok(Query { what, ids, owners, template, kind, player, limit })
    }
}

/// One control-harness session.
pub struct Session {
    data: Option<Arc<GameData>>,
    /// The install the data was loaded from (`None` for a session built from a game in tests).
    vfs: Option<Vfs>,
    game: Option<Game>,
    snap: Option<FileSnapshot>,
    snap_path: Option<PathBuf>,
    trace: Option<FileTrace>,
    trace_path: Option<PathBuf>,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(s) = self.snap.as_mut() {
            let _ = s.finish();
        }
        if let Some(t) = self.trace.as_mut() {
            let _ = t.finish();
        }
    }
}

impl Session {
    pub fn new() -> Self {
        Session { data: None, vfs: None, game: None, snap: None, snap_path: None, trace: None, trace_path: None }
    }

    /// A session over an already built game (tests: no install needed). `load` then fails, because
    /// there is no VFS to start another map from.
    pub fn from_game(game: Game) -> Self {
        let data = game.data().clone();
        Session { data: Some(data), vfs: None, game: Some(game), snap: None, snap_path: None, trace: None, trace_path: None }
    }

    pub fn is_loaded(&self) -> bool {
        self.game.is_some()
    }

    /// Read access to the loaded game (tests / other tools). The protocol never mutates through it.
    pub fn game(&self) -> Option<&Game> {
        self.game.as_ref()
    }

    /// The sim's current frame (or `None` before `load`).
    pub fn frame(&self) -> Option<u32> {
        self.game.as_ref().map(Game::frame)
    }

    /// The sim's current CRC (or `None` before `load`).
    pub fn crc_u32(&self) -> Option<u32> {
        self.game.as_ref().map(Game::crc)
    }

    fn ensure_data(&mut self) -> Result<(), String> {
        if self.data.is_some() {
            return Ok(());
        }
        let installs = Installs::detect();
        let zh = installs.zero_hour.as_ref().ok_or("load: no Zero Hour install found; set ZR_ZEROHOUR_DIR")?;
        let vfs = Vfs::zero_hour(zh).map_err(|e| format!("load: mounting Zero Hour: {e}"))?;
        let data = Arc::new(GameData::load(&vfs));
        self.data = Some(data);
        self.vfs = Some(vfs);
        Ok(())
    }

    /// Starts a skirmish on `spec.map` (the `Game::new_skirmish_with` path) and optionally opens the
    /// streaming snapshot writer before frame 0, exactly like `ZR_SNAPSHOT` in `skirmish_ai_demo`.
    pub fn load(&mut self, spec: &LoadSpec, snap: Option<SnapshotSpec>) -> Result<Value, String> {
        // Flush a writer left over from a previous game before the game it belongs to is dropped.
        self.stop_snapshot().ok();
        self.stop_trace().ok();
        self.game = None;
        self.ensure_data()?;
        let data = self.data.clone().ok_or("load: game data is not loaded")?;
        let vfs = self.vfs.as_ref().ok_or("load: this session has no VFS (it was created from a game)")?;
        let game = Game::new_skirmish_with(data, vfs, &spec.map, &spec.setups(), spec.seed)
            .map_err(|e| format!("load: {} (seed {}): {e:#}", spec.map, spec.seed))?;
        self.game = Some(game);
        let snapshot = match &snap {
            Some(s) => {
                self.start_snapshot(s)?;
                Some(s.path.clone())
            }
            None => None,
        };
        // `ZR_TRACE` at load is the environment-driven half of the trace (the C++ side's only half);
        // the `trace` request starts one later, with the same fields. A malformed value turned the
        // writer off with a warning in `env_settings`; an unwritable file warns here and leaves the
        // load running, as the C++ writer does (`GameLogicTrace.cpp`, `readTraceSettings`).
        let trace = match env_trace_spec() {
            Some(s) => match self.start_trace(&s) {
                Ok(_) => Some(s.path.clone()),
                Err(e) => {
                    eprintln!("ZR_TRACE={}: {e} (trace off)", s.path.display());
                    None
                }
            },
            None => None,
        };
        let game = self.game.as_ref().ok_or("load: the game did not start")?;
        let mut payload = state_map(game);
        payload.insert("map".to_owned(), json!(spec.map));
        payload.insert("seed".to_owned(), json!(spec.seed));
        payload.insert("players".to_owned(), Value::Array(players_json(game)));
        payload.insert("snapshot".to_owned(), json!(snapshot));
        payload.insert("trace".to_owned(), json!(trace));
        Ok(Value::Object(payload))
    }

    /// Advances `frames` logic frames (writing any due snapshot block and trace lines).
    pub fn step(&mut self, frames: u32) -> Result<Value, String> {
        let game = self.game.as_mut().ok_or_else(not_loaded)?;
        let mut snap = self.snap.take();
        let mut trace = self.trace.take();
        let res = trace::run_frames(game, frames, snap.as_mut(), trace.as_mut());
        self.snap = snap;
        self.trace = trace;
        res.map_err(|e| format!("step: writing the snapshot/trace: {e}"))?;
        let game = self.game.as_ref().ok_or_else(not_loaded)?;
        Ok(Value::Object(state_map(game)))
    }

    /// Advances until the sim's frame is `frame`.
    pub fn step_until(&mut self, frame: u32) -> Result<Value, String> {
        let current = self.game.as_ref().ok_or_else(not_loaded)?.frame();
        if frame < current {
            return Err(format!("step: until frame {frame} is before the current frame {current}"));
        }
        self.step(frame - current)
    }

    /// Queues a logic message (`Game::submit`: applied at the start of the next step).
    pub fn submit(&mut self, player: u8, command: Command) -> Result<Value, String> {
        let game = self.game.as_mut().ok_or_else(not_loaded)?;
        let count = game.players().len();
        if usize::from(player) >= count {
            return Err(format!("command: player {player} does not exist ({count} players)"));
        }
        game.submit(player, command);
        Ok(json!({"player": player, "frame": game.frame()}))
    }

    /// The current frame and CRC.
    pub fn crc(&self) -> Result<Value, String> {
        let game = self.game.as_ref().ok_or_else(not_loaded)?;
        Ok(Value::Object(state_map(game)))
    }

    /// Runs a query; only reads the simulation.
    pub fn query(&self, q: &Query) -> Result<Value, String> {
        let game = self.game.as_ref().ok_or_else(not_loaded)?;
        let mut payload = state_map(game);
        payload.insert("what".to_owned(), json!(q.what.name()));
        match q.what {
            What::Objects => insert_objects(&mut payload, game, q),
            What::Players => {
                payload.insert("players".to_owned(), Value::Array(players_query_json(game, q)));
            }
            What::Ai => {
                payload.insert("ai".to_owned(), Value::Array(ai_json(game, q)));
            }
            What::Scripts => {
                payload.insert("scripts".to_owned(), scripts_json(game));
            }
            What::All => {
                insert_objects(&mut payload, game, q);
                payload.insert("players".to_owned(), Value::Array(players_query_json(game, q)));
                payload.insert("ai".to_owned(), Value::Array(ai_json(game, q)));
                payload.insert("scripts".to_owned(), scripts_json(game));
            }
        };
        Ok(Value::Object(payload))
    }

    /// Opens (or replaces) the streaming snapshot writer and writes the current frame's block when
    /// that frame is due. Later [`step`](Self::step) calls write the due blocks.
    pub fn start_snapshot(&mut self, spec: &SnapshotSpec) -> Result<Value, String> {
        let game = self.game.as_ref().ok_or_else(not_loaded)?;
        let file = File::create(&spec.path).map_err(|e| format!("snapshot: creating {}: {e}", spec.path.display()))?;
        let mut snap = Snapshot::new(BufWriter::new(file), spec.every.max(1), spec.bits);
        snap.write_if_due(game).map_err(|e| format!("snapshot: writing {}: {e}", spec.path.display()))?;
        snap.finish().map_err(|e| format!("snapshot: flushing {}: {e}", spec.path.display()))?;
        self.snap = Some(snap);
        self.snap_path = Some(spec.path.clone());
        Ok(json!({"snapshot": spec.path, "every": spec.every, "bits": spec.bits, "frame": game.frame()}))
    }

    /// Stops the streaming writer (flush). Nothing was open is not an error.
    pub fn stop_snapshot(&mut self) -> Result<Value, String> {
        let Some(mut snap) = self.snap.take() else {
            self.snap_path = None;
            return Ok(json!({"snapshot": null}));
        };
        let path = self.snap_path.take();
        let msg = path.as_ref().map_or_else(|| "snapshot".to_owned(), |p| format!("snapshot: flushing {}", p.display()));
        snap.finish().map_err(|e| format!("{msg}: {e}"))?;
        Ok(json!({"snapshot": null, "was": path}))
    }

    /// Writes one block for the current frame to `path` (truncating it) without starting a stream.
    pub fn snapshot_once(&self, path: &Path, bits: bool) -> Result<Value, String> {
        let game = self.game.as_ref().ok_or_else(not_loaded)?;
        let file = File::create(path).map_err(|e| format!("snapshot: creating {}: {e}", path.display()))?;
        let mut out = BufWriter::new(file);
        snapshot::write_frame(&mut out, game.logic(), game.crc(), bits)
            .map_err(|e| format!("snapshot: writing {}: {e}", path.display()))?;
        out.flush().map_err(|e| format!("snapshot: flushing {}: {e}", path.display()))?;
        Ok(json!({"snapshot": path, "frame": game.frame(), "once": true, "bits": bits}))
    }

    /// Opens (or replaces) the streaming per-object trace writer and writes the current frame's lines
    /// when the frame is due. Later [`step`](Self::step) calls write the due frames.
    pub fn start_trace(&mut self, spec: &TraceSpec) -> Result<Value, String> {
        // Flush a writer being replaced before its file is reopened.
        self.stop_trace().ok();
        let game = self.game.as_ref().ok_or_else(not_loaded)?;
        let file = File::create(&spec.path).map_err(|e| format!("trace: creating {}: {e}", spec.path.display()))?;
        let mut tr = Trace::new(BufWriter::new(file), spec.ids.clone(), spec.frames);
        tr.write_if_due(game).map_err(|e| format!("trace: writing {}: {e}", spec.path.display()))?;
        tr.finish().map_err(|e| format!("trace: flushing {}: {e}", spec.path.display()))?;
        let ids = match &spec.ids {
            IdFilter::All => "all".to_owned(),
            IdFilter::Ids(ids) => ids.iter().map(u32::to_string).collect::<Vec<_>>().join(","),
        };
        let frames = if spec.frames == FrameRange::ALL {
            "all".to_owned()
        } else {
            format!("{}-{}", spec.frames.first, spec.frames.last)
        };
        self.trace = Some(tr);
        self.trace_path = Some(spec.path.clone());
        Ok(json!({"trace": spec.path, "ids": ids, "frames": frames, "frame": game.frame()}))
    }

    /// Stops the streaming trace writer (flush). Nothing open is not an error.
    pub fn stop_trace(&mut self) -> Result<Value, String> {
        let Some(mut tr) = self.trace.take() else {
            self.trace_path = None;
            return Ok(json!({"trace": null}));
        };
        let path = self.trace_path.take();
        let msg = path.as_ref().map_or_else(|| "trace".to_owned(), |p| format!("trace: flushing {}", p.display()));
        tr.finish().map_err(|e| format!("{msg}: {e}"))?;
        Ok(json!({"trace": null, "was": path}))
    }

    /// Starts the per-draw RNG trace at `path` (created/truncated) or stops it with `None`. The pid
    /// stream is `zr-sim`'s logic generator (`ZR_RNG_TRACE` format, see `tools/rng_diff.py`).
    pub fn set_rng_trace(&mut self, path: Option<&Path>) -> Result<Value, String> {
        let game = self.game.as_mut().ok_or_else(not_loaded)?;
        let frame = game.frame();
        let rng = game.logic_mut().rng_mut();
        rng.set_trace_file(path, frame).map_err(|e| match path {
            Some(p) => format!("rng_trace: creating {}: {e}", p.display()),
            None => format!("rng_trace: {e}"),
        })?;
        Ok(json!({"rng_trace": path, "traced": rng.is_traced(), "frame": frame}))
    }
}

fn not_loaded() -> String {
    "no game loaded: send a \"load\" request first".to_owned()
}

/// The `ZR_TRACE`/`ZR_TRACE_IDS`/`ZR_TRACE_FRAMES` settings for a `load`, or `None` when `ZR_TRACE` is
/// unset (the same names and format as the C++ side).
fn env_trace_spec() -> Option<TraceSpec> {
    let s = trace::env_settings()?;
    Some(TraceSpec { path: s.path, ids: s.ids, frames: s.frames })
}

fn state_map(game: &Game) -> Map<String, Value> {
    let crc = game.crc();
    let mut m = Map::new();
    m.insert("frame".to_owned(), json!(game.frame()));
    m.insert("crc".to_owned(), json!(format!("{crc:08x}")));
    m.insert("crc32".to_owned(), json!(crc));
    m.insert("objects".to_owned(), json!(game.logic().object_count()));
    m
}

fn kind_name(kind: PlayerKind) -> &'static str {
    match kind {
        PlayerKind::Neutral => "neutral",
        PlayerKind::Civilian => "civilian",
        PlayerKind::Skirmish => "skirmish",
        PlayerKind::Observer => "observer",
    }
}

fn players_json(game: &Game) -> Vec<Value> {
    game.players().iter().map(|p| {
        let (production, consumption) = game.logic().energy_tally(p.index);
        json!({
            "index": p.index,
            "name": p.name,
            "kind": kind_name(p.kind),
            "faction": p.faction,
            "side": p.side,
            "money": game.player_money(p.index).unwrap_or(p.money),
            "is_ai": p.is_ai,
            "team": p.team,
            "color": p.color,
            "start": p.start_position,
            "power": {
                "production": production,
                "consumption": consumption,
                "sufficient": game.logic().has_sufficient_power(p.index),
                "sabotaged": game.logic().is_power_sabotaged(p.index),
            },
            "rank": game.logic().player_rank_level(p.index),
            "skill_points": game.logic().player_skill_points(p.index),
            "science_points": game.logic().player_science_purchase_points(p.index),
            "sciences": game.logic().player_sciences(p.index).map(|s| s.iter().cloned().collect::<Vec<_>>()).unwrap_or_default(),
            "score": game.logic().score_keeper(p.index).map_or(0, |s| s.calculate_score()),
        })
    }).collect()
}

fn players_query_json(game: &Game, q: &Query) -> Vec<Value> {
    players_json(game)
        .into_iter()
        .filter(|v| q.player.is_none_or(|p| v.get("index").and_then(Value::as_u64) == Some(u64::from(p))))
        .collect()
}

fn insert_objects(payload: &mut Map<String, Value>, game: &Game, q: &Query) {
    let objects = objects_json(game, q);
    payload.insert("count".to_owned(), json!(objects.len()));
    payload.insert("objects".to_owned(), Value::Array(objects));
}

fn objects_json(game: &Game, q: &Query) -> Vec<Value> {
    let mut out = Vec::new();
    for o in game.logic().objects() {
        if let Some(ids) = &q.ids {
            if !ids.contains(&o.id.0) {
                continue;
            }
        }
        if let Some(owners) = &q.owners {
            if !owners.contains(&o.player) {
                continue;
            }
        }
        if let Some(t) = &q.template {
            if o.template.name != *t {
                continue;
            }
        }
        if let Some(bits) = q.kind {
            if !o.is_kind_of(bits) {
                continue;
            }
        }
        out.push(object_json(o));
        if q.limit.is_some_and(|n| out.len() >= n) {
            break;
        }
    }
    out
}

fn object_json(o: &zr_sim::Object) -> Value {
    let attack = match o.combat.attack.as_ref().map(|a| a.target) {
        None => Value::Null,
        Some(AttackTarget::Object(id)) => json!({"target": id.0}),
        Some(AttackTarget::Ground(p)) => json!({"target": "ground", "pos": coords(p)}),
    };
    let guard = match o.combat.guard {
        None => Value::Null,
        Some(GuardTarget::Object(id)) => json!({"target": id.0}),
        Some(GuardTarget::Point(p)) => json!({"pos": coords(p)}),
    };
    let guard_mode = o.combat.guard.map(|_| match o.combat.guard_mode {
        GuardMode::Normal => "normal",
        GuardMode::WithoutPursuit => "without_pursuit",
        GuardMode::FlyingUnitsOnly => "flying_units_only",
    });
    let ai = if o.template.ai_update.is_some() { Value::String(format!("{:?}", o.ai.machine.current)) } else { Value::Null };
    let health = o.body.as_ref().map(|_| r3(o.health()));
    let construction = o.under_construction.then(|| r3(o.construction_percent));
    json!({
        "id": o.id.0,
        "template": o.template.name,
        "owner": o.player,
        "pos": coords(o.position),
        "angle": r4(o.angle),
        "health": health,
        "ai": ai,
        "attack": attack,
        "attack_phase": o.combat.attack.as_ref().map(|a| format!("{:?}", a.phase)),
        "guard": guard,
        "guard_mode": guard_mode,
        "move": o.ai.move_goal.map(coords),
        "attack_move": o.combat.attack_move,
        "hunt": o.combat.hunt,
        "build": o.build_target.map_or(0, |b| b.0),
        "under_construction": o.under_construction,
        "construction_percent": construction,
        "queue": o.production.as_ref().map_or(0, |p| p.queue.len()),
        "contained_by": o.contained_by.map_or(0, |c| c.0),
        "boxes": o.boxes,
    })
}

fn ai_json(game: &Game, q: &Query) -> Vec<Value> {
    let mut out = Vec::new();
    for p in game.players() {
        if q.player.is_some_and(|want| want != p.index) {
            continue;
        }
        let Some(ai) = game.logic().ai_player(p.index) else { continue };
        out.push(json!({
            "player": p.index,
            "base_center": [r4(ai.base_center.0), r4(ai.base_center.1)],
            "base_radius": r4(ai.base_radius),
            "current_enemy": ai.current_enemy,
            "structures_built": ai.structures_built,
            "attacks_launched": ai.attacks_launched,
            "teams_built": ai.teams_built,
            "base_defenses_built": ai.base_defenses_built,
            "powers_fired": ai.powers_fired,
            "sciences_bought": ai.sciences_bought,
            "team_candidates": ai.team_candidates,
            "team_pc_fail": ai.team_pc_fail,
            "team_aff_fail": ai.team_aff_fail,
            "frame_to_check_enemy": ai.frame_to_check_enemy,
            "structure_timer": ai.structure_timer,
            "build_delay": ai.build_delay,
            "ready_to_build_structure": ai.ready_to_build_structure,
            "team_timer": ai.team_timer,
            "script_team_timer": ai.script_team_timer,
            "team_delay": ai.team_delay,
            "ready_to_build_team": ai.ready_to_build_team,
            "attack_group": ai.attack_group.iter().map(|id| id.0).collect::<Vec<_>>(),
            "built": ai.built.iter().map(|b| b.map_or(0, |id| id.0)).collect::<Vec<_>>(),
            "build_queue": team_queue_json(&ai.team_build_queue),
            "ready_queue": team_queue_json(&ai.team_ready_queue),
            "supply": ai.supply.iter().map(|s| json!({
                "is_supply": s.is_supply,
                "desired": s.desired,
                "current": s.current,
                "object": s.object.map_or(0, |o| o.0),
            })).collect::<Vec<_>>(),
        }));
    }
    out
}

fn team_queue_json(list: &[TeamInQueue]) -> Value {
    Value::Array(
        list.iter()
            .map(|t| {
                json!({
                    "key": t.key,
                    "priority": t.priority_build,
                    "started": t.frame_started,
                    "reinforcement": t.reinforcement,
                    "recruit": t.reinforcement_id.map_or(0, |id| id.0),
                    "sent": t.sent_to_start_location,
                    "work": t.work_orders.iter().map(|w| json!({
                        "thing": w.thing,
                        "completed": w.completed,
                        "required_count": w.required_count,
                        "required": w.required,
                        "resource_gatherer": w.resource_gatherer,
                        "factory": w.factory.map_or(0, |f| f.0),
                    })).collect::<Vec<_>>(),
                })
            })
            .collect(),
    )
}

fn scripts_json(game: &Game) -> Value {
    let s = game.logic().scripts();
    let mut counters = Map::new();
    for (name, c) in &s.counters {
        counters.insert(name.clone(), json!(c.value));
    }
    let mut named = Map::new();
    for (name, id) in &s.named_objects {
        named.insert(name.clone(), json!(id.0));
    }
    json!({
        "script_driven": s.script_driven,
        "counters": counters,
        "flags": s.flags,
        "named_objects": named,
        "actions_executed": s.actions_executed,
        "conditions_evaluated": s.conditions_evaluated,
        "warnings": s.warnings,
        "teams_created": s.teams.values().filter(|t| t.created).count(),
        "team_prototypes": s.team_protos.len(),
        "trigger_areas": s.trigger_areas.keys().cloned().collect::<Vec<_>>(),
    })
}

fn coords(p: zr_sim::Coord3D) -> Value {
    json!([r4(p.x), r4(p.y), r4(p.z)])
}

/// 4 decimal places, like the snapshot's positions.
fn r4(v: f32) -> f64 {
    (f64::from(v) * 1e4).round() / 1e4
}

/// 3 decimal places, like the snapshot's health and construction percent.
fn r3(v: f32) -> f64 {
    (f64::from(v) * 1e3).round() / 1e3
}

fn opt_u32(o: &Map<String, Value>, key: &str) -> Result<Option<u32>, String> {
    match o.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => Ok(Some(
            v.as_u64().and_then(|n| u32::try_from(n).ok()).ok_or_else(|| format!("\"{key}\" must be an unsigned 32-bit integer"))?,
        )),
    }
}

fn opt_u8(o: &Map<String, Value>, key: &str) -> Result<Option<u8>, String> {
    match o.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => Ok(Some(u8_field(v, key)?)),
    }
}

fn u8_field(v: &Value, key: &str) -> Result<u8, String> {
    v.as_u64().and_then(|n| u8::try_from(n).ok()).ok_or_else(|| format!("\"{key}\" must be an integer in 0..=255"))
}

fn opt_i32(o: &Map<String, Value>, key: &str) -> Result<Option<i32>, String> {
    match o.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => Ok(Some(
            v.as_i64().and_then(|n| i32::try_from(n).ok()).ok_or_else(|| format!("\"{key}\" must be a 32-bit integer"))?,
        )),
    }
}

fn opt_bool(o: &Map<String, Value>, key: &str) -> Result<Option<bool>, String> {
    match o.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => Ok(Some(v.as_bool().ok_or_else(|| format!("\"{key}\" must be a boolean"))?)),
    }
}

fn opt_str<'a>(o: &'a Map<String, Value>, key: &str) -> Result<Option<&'a str>, String> {
    match o.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_str().map(Some).ok_or_else(|| format!("\"{key}\" must be a string")),
    }
}

fn u32_list(v: &Value, key: &str) -> Result<Vec<u32>, String> {
    let values: Vec<&Value> = match v {
        Value::Array(a) => {
            if a.len() > MAX_FILTER {
                return Err(format!("query: \"{key}\" has {} entries; at most {MAX_FILTER}", a.len()));
            }
            a.iter().collect()
        }
        other => vec![other],
    };
    values
        .into_iter()
        .map(|v| v.as_u64().and_then(|n| u32::try_from(n).ok()).ok_or_else(|| format!("query: \"{key}\" must be unsigned 32-bit integers")))
        .collect()
}

fn u8_list(v: &Value, key: &str) -> Result<Vec<u8>, String> {
    let values: Vec<&Value> = match v {
        Value::Array(a) => {
            if a.len() > MAX_FILTER {
                return Err(format!("query: \"{key}\" has {} entries; at most {MAX_FILTER}", a.len()));
            }
            a.iter().collect()
        }
        other => vec![other],
    };
    values.into_iter().map(|v| u8_field(v, key)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn obj(v: Value) -> Map<String, Value> {
        v.as_object().expect("object").clone()
    }

    #[test]
    fn load_spec_defaults_difficulty_and_cash() {
        let spec = LoadSpec::from_json(&obj(json!({
            "cash": 7500,
            "players": [
                {"faction": "FactionGLA"},
                {"faction": "FactionAmerica", "start": 1, "ai": false, "difficulty": "hard", "team": 2, "color": 3, "cash": 5000}
            ]
        })))
        .expect("load spec");
        assert_eq!(spec.map, DEFAULT_MAP);
        assert_eq!(spec.seed, DEFAULT_SEED);
        assert_eq!(spec.players.len(), 2);
        let first = &spec.players[0];
        assert_eq!((first.faction.as_str(), first.ai, first.start, first.team, first.color), ("FactionGLA", true, 0, 0, -1));
        assert_eq!(first.difficulty, AiDifficulty::Normal);
        assert_eq!(first.cash, Some(7500), "the top-level cash fills the slot");
        let second = &spec.players[1];
        assert_eq!((second.ai, second.start, second.team, second.color), (false, 1, 2, 3));
        assert_eq!(second.difficulty, AiDifficulty::Hard);
        assert_eq!(second.cash, Some(5000), "a per-slot cash wins");
        let setups = spec.setups();
        assert_eq!(setups[0].starting_cash, Some(7500));
        assert_eq!(setups[1].starting_cash, Some(5000));
        assert!(!setups[1].is_ai);
    }

    #[test]
    fn load_spec_rejects_bad_input() {
        for (v, want) in [
            (json!({}), "players"),
            (json!({"players": []}), "not be empty"),
            (json!({"players": [{}]}), "faction"),
            (json!({"players": [{"faction": "F", "difficulty": "insane"}]}), "not easy"),
            (json!({"players": [{"faction": "F", "ai": 1}]}), "boolean"),
            (json!({"players": [{"faction": "F", "start": 300}]}), "0..=255"),
            (json!({"players": [{"faction": "F"}, "x"]}), "players[1]"),
            (json!({"players": [{"faction": "F"}], "seed": -1}), "unsigned"),
        ] {
            let e = LoadSpec::from_json(&obj(v.clone())).expect_err(&format!("{v} must be rejected"));
            assert!(e.contains(want), "{v}: {e:?} does not mention {want:?}");
        }
    }

    #[test]
    fn query_spec_filters() {
        let q = Query::from_json(&obj(json!({"what": "objects", "id": 7, "owner": [2, 3], "limit": 5}))).expect("query");
        assert_eq!(q.what, What::Objects);
        assert_eq!(q.ids, Some(vec![7]));
        assert_eq!(q.owners, Some(vec![2, 3]));
        assert_eq!(q.limit, Some(5));
        let q = Query::from_json(&obj(json!({"what": "AI", "kind": "structure", "id": [1, 2]}))).expect("query");
        assert_eq!(q.what, What::Ai);
        assert_eq!(q.kind, Some(kind_of::STRUCTURE));
        assert_eq!(q.ids, Some(vec![1, 2]));
        assert_eq!(Query::from_json(&obj(json!({"what": "all"}))).expect("query").what, What::All);
        for (v, want) in [
            (json!({}), "missing string field \"what\""),
            (json!({"what": "nope"}), "unknown"),
            (json!({"what": "objects", "kind": "NOPE"}), "unknown kind"),
            (json!({"what": "objects", "limit": -1}), "non-negative"),
            (json!({"what": "objects", "player": 300}), "0..=255"),
        ] {
            let e = Query::from_json(&obj(v.clone())).expect_err(&format!("{v} must be rejected"));
            assert!(e.contains(want), "{v}: {e:?} does not mention {want:?}");
        }
    }

    #[test]
    fn snapshot_spec_defaults_and_errors() {
        assert_eq!(SnapshotSpec::from_json(None), Ok(None));
        assert_eq!(SnapshotSpec::from_json(Some(&Value::Null)), Ok(None));
        let s = SnapshotSpec::from_json(Some(&json!({"path": "run.snap"}))).expect("spec").expect("some");
        assert_eq!(s.path, PathBuf::from("run.snap"));
        assert_eq!(s.every, snapshot::DEFAULT_EVERY);
        assert!(!s.bits);
        let s = SnapshotSpec::from_json(Some(&json!({"path": "run.snap", "every": 60, "bits": true}))).expect("spec").expect("some");
        assert_eq!((s.every, s.bits), (60, true));
        for (v, want) in [
            (json!({}), "path"),
            (json!({"path": ""}), "path"),
            (json!({"path": "x", "every": 0}), "positive"),
            (json!({"path": "x", "bits": "yes"}), "boolean"),
        ] {
            let e = SnapshotSpec::from_json(Some(&v)).expect_err(&format!("{v} must be rejected"));
            assert!(e.contains(want), "{v}: {e:?} does not mention {want:?}");
        }
    }
}
