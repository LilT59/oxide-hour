//! JSON-lines protocol: one request object per line, one reply object per line.
//!
//! Requests: `load`, `step`, `query`, `command`, `snapshot`, `rng_trace`, `crc`, `quit` and `help`
//! (fields in `docs/HARNESS.md` "Control interface"). Every reply is an object; failures are
//! `{"ok":false,"error":"..."}` and never end the session. A panic while handling a request is
//! caught and reported the same way, then the session stops (its state may be half-updated).

use std::path::PathBuf;

use serde_json::{json, Map, Value};

use crate::command_json;
use crate::session::{LoadSpec, Query, Session, SnapshotSpec, TraceSpec};

/// `--help` text.
pub const HELP: &str = "\
zr-harness: headless JSON-lines control interface over the simulation.

Reads one JSON object per line from stdin and writes one JSON reply per line to stdout.
Requests ({...} fields; [...] optional):
  {\"cmd\":\"load\",\"map\":\"Alpine Assault\",\"seed\":1,\"cash\":10000,\"players\":[{\"faction\":\"FactionGLA\",\"ai\":true,
     \"difficulty\":\"normal\",\"start\":0,\"team\":0,\"color\":-1,\"cash\":10000}],
     \"snapshot\":{\"path\":\"out.snap\",\"every\":30,\"bits\":false}}      start a skirmish
  {\"cmd\":\"step\",\"frames\":30}                                        advance N logic frames
  {\"cmd\":\"step\",\"until\":300}                                        advance to frame 300
  {\"cmd\":\"query\",\"what\":\"objects\"|\"players\"|\"ai\"|\"scripts\"|\"all\",
     \"id\":N|\"[N,...]\",\"owner\":N|\"[N,...]\",\"template\":\"...\",\"kind\":\"STRUCTURE\",
     \"player\":N,\"limit\":N}                                            read state
  {\"cmd\":\"command\",\"player\":2,\"command\":{\"type\":\"move_to\",\"ids\":[211],\"pos\":[1100,1900,0]}}
  {\"cmd\":\"snapshot\",\"path\":\"cur.snap\",\"bits\":false}             write the current frame
  {\"cmd\":\"snapshot\",\"path\":\"run.snap\",\"every\":30}               start a streaming writer
  {\"cmd\":\"snapshot\"}                                                 stop the streaming writer
  {\"cmd\":\"trace\",\"path\":\"obj.trace\",\"ids\":\"243,209\",\"frames\":\"1550-1700\"}
                                                                        start a per-object trace (ids: a comma
                                                                        list or all; frames: a-b or one frame)
  {\"cmd\":\"trace\"}                                                   stop the trace writer
  {\"cmd\":\"rng_trace\",\"path\":\"rng.txt\"} / {\"cmd\":\"rng_trace\",\"off\":true}
  {\"cmd\":\"crc\"} / {\"cmd\":\"quit\"} / {\"cmd\":\"help\"}

Full protocol, command types and field meanings: docs/HARNESS.md.
";

/// Handles one input line: returns the reply and whether the session should stop.
pub fn handle_line(session: &mut Session, line: &str) -> (Value, bool) {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| dispatch(session, line))) {
        Ok(out) => out,
        Err(_) => (err("internal panic while handling the request; session stopped"), true),
    }
}

fn dispatch(session: &mut Session, line: &str) -> (Value, bool) {
    let value: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return (err(format!("invalid JSON: {e}")), false),
    };
    let Some(o) = value.as_object() else {
        return (err("request: want a JSON object"), false);
    };
    let Some(cmd) = o.get("cmd").and_then(Value::as_str) else {
        return (err("request: missing string field \"cmd\""), false);
    };
    let cmd = match cmd.to_ascii_lowercase() {
        c if c == "rngtrace" => "rng_trace".to_owned(),
        c => c,
    };
    if cmd == "quit" {
        return (ok("quit", Value::Null), true);
    }
    let reply: Result<Value, String> = match cmd.as_str() {
        "load" => load(session, o),
        "step" => step(session, o),
        "query" => query(session, o),
        "command" => command(session, o),
        "snapshot" => snapshot(session, o),
        "trace" => trace(session, o),
        "rng_trace" => rng_trace(session, o),
        "crc" => session.crc(),
        "help" => Ok(json!({"protocol": "json-lines", "commands": ["load", "step", "query", "command", "snapshot", "trace", "rng_trace", "crc", "quit", "help"]})),
        other => Err(format!(
            "request: unknown \"cmd\" {other:?} (want load, step, query, command, snapshot, trace, rng_trace, crc, quit or help)"
        )),
    };
    match reply {
        Ok(payload) => (ok(&cmd, payload), false),
        Err(e) => (err(e), false),
    }
}

fn load(session: &mut Session, o: &Map<String, Value>) -> Result<Value, String> {
    let spec = LoadSpec::from_json(o)?;
    let snap = SnapshotSpec::from_json(o.get("snapshot"))?;
    session.load(&spec, snap)
}

fn step(session: &mut Session, o: &Map<String, Value>) -> Result<Value, String> {
    let frames = opt_u32(o, "frames")?;
    let until = opt_u32(o, "until")?;
    match (frames, until) {
        (Some(n), None) => session.step(n),
        (None, Some(f)) => session.step_until(f),
        (None, None) => Err("step: give \"frames\" (a count) or \"until\" (a frame number)".to_owned()),
        (Some(_), Some(_)) => Err("step: give \"frames\" or \"until\", not both".to_owned()),
    }
}

fn query(session: &Session, o: &Map<String, Value>) -> Result<Value, String> {
    let q = Query::from_json(o)?;
    session.query(&q)
}

fn command(session: &mut Session, o: &Map<String, Value>) -> Result<Value, String> {
    let player = match o.get("player") {
        None => return Err("command: missing \"player\" (the issuing player index)".to_owned()),
        Some(v) => v
            .as_u64()
            .and_then(|n| u8::try_from(n).ok())
            .ok_or_else(|| "command: \"player\" must be an integer in 0..=255".to_owned())?,
    };
    let command = command_json::decode(o.get("command").ok_or("command: missing \"command\" (the command object)")?)?;
    session.submit(player, command)
}

fn snapshot(session: &mut Session, o: &Map<String, Value>) -> Result<Value, String> {
    let path = opt_path(o, "path")?;
    let bits = opt_bool(o, "bits")?.unwrap_or(false);
    let once = opt_bool(o, "once")?.unwrap_or(false);
    match path {
        None => session.stop_snapshot(),
        // `once` (or no `every`) writes the current frame only; `every` starts a streaming writer.
        Some(path) if once || o.get("every").is_none() => session.snapshot_once(&path, bits),
        Some(path) => {
            let every = opt_u32(o, "every")?.unwrap_or(zr_game::snapshot::DEFAULT_EVERY);
            if every == 0 {
                return Err("snapshot: \"every\" must be a positive frame count".to_owned());
            }
            session.start_snapshot(&SnapshotSpec { path, every, bits })
        }
    }
}

fn trace(session: &mut Session, o: &Map<String, Value>) -> Result<Value, String> {
    let off = opt_bool(o, "off")?.unwrap_or(false);
    let path = opt_path(o, "path")?;
    if off && path.is_some() {
        return Err("trace: give \"path\" (start) or \"off\": true (stop), not both".to_owned());
    }
    match path {
        None => session.stop_trace(),
        Some(path) => session.start_trace(&TraceSpec::from_json(o, path)?),
    }
}

fn rng_trace(session: &mut Session, o: &Map<String, Value>) -> Result<Value, String> {
    let off = opt_bool(o, "off")?.unwrap_or(false);
    let path = opt_path(o, "path")?;
    if off && path.is_some() {
        return Err("rng_trace: give \"path\" (start) or \"off\": true (stop), not both".to_owned());
    }
    match path {
        Some(p) => session.set_rng_trace(Some(&p)),
        None => session.set_rng_trace(None),
    }
}

/// Attaches `ok`, `cmd` and the session payload (a flat object) to a reply.
fn ok(cmd: &str, payload: Value) -> Value {
    let mut m = match payload {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    m.insert("ok".to_owned(), Value::Bool(true));
    m.insert("cmd".to_owned(), Value::String(cmd.to_owned()));
    Value::Object(m)
}

fn err(message: impl Into<String>) -> Value {
    json!({"ok": false, "error": message.into()})
}

fn opt_u32(o: &Map<String, Value>, key: &str) -> Result<Option<u32>, String> {
    match o.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => Ok(Some(
            v.as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| format!("\"{key}\" must be an unsigned 32-bit integer"))?,
        )),
    }
}

fn opt_bool(o: &Map<String, Value>, key: &str) -> Result<Option<bool>, String> {
    match o.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => Ok(Some(v.as_bool().ok_or_else(|| format!("\"{key}\" must be a boolean"))?)),
    }
}

fn opt_path(o: &Map<String, Value>, key: &str) -> Result<Option<PathBuf>, String> {
    match o.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => Ok(Some(PathBuf::from(
            v.as_str().filter(|s| !s.is_empty()).ok_or_else(|| format!("\"{key}\" must be a non-empty path string"))?,
        ))),
    }
}
