//! Per-frame state snapshots for diffing the port against the C++ build (schema and field meanings in
//! `docs/HARNESS.md`; the diff is `tools/snap_diff.py`).
//!
//! `ZR_SNAPSHOT=<file>` turns the writer on ([`FileSnapshot::from_env`]). `ZR_SNAPSHOT_EVERY=<N>` sets the
//! frame interval (default [`DEFAULT_EVERY`]) and `ZR_SNAPSHOT_BITS=1` adds the hex bit patterns of the
//! floats. Off, nothing here runs. On, the writer only reads the sim, so the CRC of a run is unchanged.

use std::borrow::Cow;
use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::PathBuf;

use zr_sim::production::{ProductionKind, ProductionState};
use zr_sim::script::ScriptEngine;
use zr_sim::skirmish_ai::TeamInQueue;
use zr_sim::{GameLogic, ObjectId};

use crate::game::Game;

/// Frame interval when `ZR_SNAPSHOT_EVERY` is not set.
pub const DEFAULT_EVERY: u32 = 30;

/// `CONSTRUCTION_COMPLETE` (BuildAssistant.h:46): what `Object::m_constructionPercent` holds once an object is
/// built (Object.cpp:283 sets it for every new object, DozerAIUpdate.cpp:568 on completion). The port prints it
/// for objects that are not under construction.
pub const CONSTRUCTION_COMPLETE: f32 = -1.0;

/// A snapshot writer over a file (the `ZR_SNAPSHOT` target).
pub type FileSnapshot = Snapshot<BufWriter<File>>;

/// The writer settings read from the environment (see [`settings`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub path: PathBuf,
    pub every: u32,
    pub bits: bool,
}

/// The writer settings for the given environment values (`None` = unset). No path (unset or empty) gives `Ok(None)`:
/// the writer is off. `every` must be a positive integer (surrounding spaces allowed) and defaults to
/// [`DEFAULT_EVERY`]. `bits` is on only for the value `1`.
pub fn settings(path: Option<&OsStr>, every: Option<&str>, bits: Option<&str>) -> io::Result<Option<Settings>> {
    let Some(path) = path.filter(|p| !p.is_empty()) else {
        return Ok(None);
    };
    let every = match every {
        None => DEFAULT_EVERY,
        Some(v) => match v.trim().parse::<u32>() {
            Ok(n) if n > 0 => n,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("ZR_SNAPSHOT_EVERY={v}: want a positive frame count"),
                ))
            }
        },
    };
    Ok(Some(Settings { path: PathBuf::from(path), every, bits: bits == Some("1") }))
}

/// `Energy::getProduction` (Energy.cpp:67-73): no power is produced while the grid is sabotaged. Only the printed
/// value is gated; the sim's own tally is unchanged.
pub fn printed_production(production: i32, sabotaged: bool) -> i32 {
    if sabotaged {
        0
    } else {
        production
    }
}

/// Writes one snapshot block for every frame whose number is a multiple of `every`.
pub struct Snapshot<W: Write> {
    out: W,
    every: u32,
    bits: bool,
    last: Option<u32>,
}

impl FileSnapshot {
    /// The writer named by the environment (`ZR_SNAPSHOT`, `ZR_SNAPSHOT_EVERY`, `ZR_SNAPSHOT_BITS`), or `None`
    /// when `ZR_SNAPSHOT` is unset or empty. Creates (truncates) the file.
    pub fn from_env() -> io::Result<Option<Self>> {
        let path = std::env::var_os("ZR_SNAPSHOT");
        let every = std::env::var("ZR_SNAPSHOT_EVERY").ok();
        let bits = std::env::var("ZR_SNAPSHOT_BITS").ok();
        let Some(s) = settings(path.as_deref(), every.as_deref(), bits.as_deref())? else {
            return Ok(None);
        };
        Ok(Some(Snapshot::new(BufWriter::new(File::create(&s.path)?), s.every, s.bits)))
    }
}

impl<W: Write> Snapshot<W> {
    pub fn new(out: W, every: u32, bits: bool) -> Self {
        Snapshot { out, every: every.max(1), bits, last: None }
    }

    /// Writes the block for the current frame when that frame is due and was not written yet.
    pub fn write_if_due(&mut self, g: &Game) -> io::Result<()> {
        let frame = g.frame();
        if frame % self.every != 0 || self.last == Some(frame) {
            return Ok(());
        }
        self.last = Some(frame);
        write_frame(&mut self.out, g.logic(), g.crc(), self.bits)
    }

    /// Flushes and returns the underlying writer.
    pub fn into_inner(mut self) -> io::Result<W> {
        self.out.flush()?;
        Ok(self.out)
    }

    /// Flushes buffered lines. (A dropped `BufWriter` flushes too, but its errors are lost.)
    pub fn finish(&mut self) -> io::Result<()> {
        self.out.flush()
    }
}

/// `Game::run_frames(n)`, writing the due blocks while it runs: one before the first frame if that frame is
/// due, then one after each frame that is due. With `None` it is exactly `g.run_frames(n)`.
pub fn run_frames<W: Write>(g: &mut Game, n: u32, snap: Option<&mut Snapshot<W>>) -> io::Result<()> {
    let Some(snap) = snap else {
        g.run_frames(n);
        return Ok(());
    };
    snap.write_if_due(g)?;
    for _ in 0..n {
        g.step();
        snap.write_if_due(g)?;
    }
    Ok(())
}

/// A string value as one token: `%`, space and `=` are percent-escaped (team names contain spaces).
pub fn esc(s: &str) -> Cow<'_, str> {
    if s.chars().any(|c| matches!(c, '%' | ' ' | '=')) {
        Cow::Owned(s.replace('%', "%25").replace(' ', "%20").replace('=', "%3D"))
    } else {
        Cow::Borrowed(s)
    }
}

/// The AI state the C++ snapshot's `ai=` field prints (`aiStateText`, `GameLogicSnapshot.cpp:263-285`, from
/// `getCurrentStateID()`). While an attack-move/hunt/attack-area/attack-follow machine runs its attack, the port's
/// main state is the attack (it collapses the C++ sub-machine into `attack_return`), but the C++ state ID is the
/// **enclosing** state: the attack-move's machine is `AIAttackMoveStateMachine` inside `AI_ATTACK_MOVE_TO`
/// (`AIStates.cpp:1999-2006`), the hunt's and the attack-area's likewise (`:7122`, `:7329`). A direct attack order
/// has `attack_return == Idle` and keeps its own name. The ZR_TRACE `ai` field uses the same rule (`trace.rs`).
pub fn shown_state(m: &zr_sim::ai_states::AiMachine) -> zr_sim::ai_states::AiStateType {
    use zr_sim::ai_states::AiStateType;
    let enclosing = matches!(
        m.attack_return,
        AiStateType::AttackMoveTo | AiStateType::Hunt | AiStateType::AttackArea | AiStateType::AttackFollowWaypointPath
    );
    let attacking = matches!(
        m.current,
        AiStateType::AttackObject | AiStateType::AttackPosition | AiStateType::ForceAttackObject | AiStateType::AttackAndFollowObject
    );
    if enclosing && attacking {
        m.attack_return
    } else {
        m.current
    }
}

/// The `atk` field (`victimText`, GameLogicSnapshot.cpp:292-309): the current victim id; else `ground` when the
/// enclosing state (`shown_state`, the `ai=` rule) is `AI_ATTACK_POSITION` and an attack runs (`getCurrentVictimPos`,
/// AIUpdate.cpp:4280); else `0`. `has_attack` proxies C++'s `OBJECT_STATUS_IS_ATTACKING` (set only on a CONTINUE
/// return of `AIAttackState::onEnter`, AIStates.cpp:5586-5590) with `combat.attack.is_some()`. The port has no status
/// bit for it; the proxy is diagnostic output only and has no sim effect.
pub fn victim_text(m: &zr_sim::ai_states::AiMachine, has_attack: bool) -> String {
    use zr_sim::ai_states::AiStateType;
    if m.current_victim != ObjectId::INVALID {
        m.current_victim.0.to_string()
    } else if shown_state(m) == AiStateType::AttackPosition && has_attack {
        "ground".to_owned()
    } else {
        "0".to_owned()
    }
}

/// One snapshot block: the `FRAME` header, then players, objects, production queues, AI team queues and script
/// teams, in the order `docs/HARNESS.md` lists. `crc` is `Game::crc` (informational; the C++ CRC differs).
pub fn write_frame<W: Write>(out: &mut W, logic: &GameLogic, crc: u32, bits: bool) -> io::Result<()> {
    let f = logic.frame();
    writeln!(out, "SNAP f={f} FRAME crc={crc:08x} objs={} players={}", logic.object_count(), logic.players().count())?;
    for p in logic.players() {
        let (production, cons) = logic.energy_tally(p.index);
        let prod = printed_production(production, logic.is_power_sabotaged(p.index));
        let power = if logic.has_sufficient_power(p.index) { '+' } else { '-' };
        let ai = u8::from(logic.ai_player(p.index).is_some());
        writeln!(out, "SNAP f={f} P p={} money={} power={power} prod={prod} cons={cons} ai={ai}", p.index, p.money)?;
    }
    // The engine's writer walks `m_objList` (`GameLogicSnapshot.cpp:631-648`), which still holds objects
    // destroyed earlier in the frame (they leave at `processDestroyList`), so the frame-0 block lists the
    // two command centres `startNewGame` replaced.
    for o in logic.object_list() {
        // `victimText` (GameLogicSnapshot.cpp:292-309): the AI's current victim (`getCurrentVictim`, set by the attack's
        // update passes), else `ground` when `getCurrentStateID() == AI_ATTACK_POSITION` (the enclosing state, as in
        // `ai=`, GameLogicSnapshot.cpp:263-285) and an attack is running (`getCurrentVictimPos`, AIUpdate.cpp:4280), else `0`.
        let attack = victim_text(&o.ai.machine, o.combat.attack.is_some());
        let queued = o.production.as_ref().map_or(0, |p| p.queue.len());
        let build = o.build_target.map_or(0, |b| b.0);
        let (x, y, z) = (o.position.x, o.position.y, o.position.z);
        // `-` for no body module (`getBodyModule()` is null) and no AIUpdate module (`getAIUpdateInterface()` is null).
        let hp = o.body.as_ref().map(|_| o.health());
        let hp_txt = hp.map_or_else(|| "-".to_owned(), |h| format!("{h:.3}"));
        let ai = if o.template.ai_update.is_some() { format!("{:?}", shown_state(&o.ai.machine)) } else { "-".to_owned() };
        let cons = if o.under_construction { o.construction_percent } else { CONSTRUCTION_COMPLETE };
        write!(
            out,
            "SNAP f={f} O id={} tmpl={} owner={} x={x:.4} y={y:.4} z={z:.4} hp={hp_txt} ai={ai} cons={cons:.3} q={queued} atk={attack} bld={build}",
            o.id.0,
            esc(&o.template.name),
            o.player
        )?;
        if bits {
            let hpb = hp.map_or_else(|| "-".to_owned(), |h| format!("{:08x}", h.to_bits()));
            write!(
                out,
                " xb={:08x} yb={:08x} zb={:08x} hpb={hpb} consb={:08x}",
                x.to_bits(),
                y.to_bits(),
                z.to_bits(),
                cons.to_bits()
            )?;
        }
        writeln!(out)?;
    }
    for o in logic.object_list() {
        if let Some(pr) = &o.production {
            write_production(out, f, o.id, pr)?;
        }
    }
    for p in logic.players() {
        let Some(ai) = logic.ai_player(p.index) else { continue };
        for (queue, list) in [("build", &ai.team_build_queue), ("ready", &ai.team_ready_queue)] {
            for (i, t) in list.iter().enumerate() {
                write_team_queue(out, f, p.index, queue, i, t, logic.scripts())?;
            }
        }
    }
    // Created teams (`Team::isActive`: the team was activated), by their `TeamFactory` id.
    let mut teams: Vec<_> = logic.scripts().teams.values().filter(|t| t.created).collect();
    teams.sort_by_key(|t| t.seq);
    for t in teams {
        write_script_team(out, f, t.seq, &t.proto, &t.members)?;
    }
    Ok(())
}

/// One `Q` record per entry of a factory's production queue (`ProductionUpdate::m_productionQueue`), in queue order.
pub fn write_production<W: Write>(out: &mut W, f: u32, factory: ObjectId, pr: &ProductionState) -> io::Result<()> {
    for (i, e) in pr.queue.iter().enumerate() {
        let (kind, tmpl) = match &e.kind {
            ProductionKind::Unit(t) => ("unit", esc(&t.name)),
            ProductionKind::Upgrade(name) => ("upgrade", esc(name)),
        };
        let door = e.door.map_or_else(|| "-".to_owned(), |d| d.to_string());
        writeln!(
            out,
            "SNAP f={f} Q obj={} i={i} kind={kind} tmpl={tmpl} total={} done={} frames={} door={door}",
            factory.0, e.quantity_total, e.quantity_produced, e.frames
        )?;
    }
    Ok(())
}

/// One `S` record per created script team: `id` is the `TeamFactory` id (`Team::getID`, `m_uniqueTeamID`), `proto` the
/// prototype name (lowercase), `members` the member object ids in ascending order (the C++ side sorts its member list:
/// `Object.cpp:898` prepends each member, so the list order is an insertion detail).
pub fn write_script_team<W: Write>(out: &mut W, f: u32, id: u32, proto: &str, members: &[ObjectId]) -> io::Result<()> {
    let mut ids: Vec<u32> = members.iter().map(|m| m.0).collect();
    ids.sort_unstable();
    let ids: Vec<String> = ids.iter().map(u32::to_string).collect();
    let proto = proto.to_ascii_lowercase();
    writeln!(out, "SNAP f={f} S id={id} proto={} members={}", esc(&proto), ids.join(","))
}

/// One `T` record per entry of a player's AI team queue (`queue` is `build` or `ready`), `i` its index in the queue.
/// `scripts` resolves the entry's team instance key to its prototype name (the `key` field).
pub fn write_team_queue<W: Write>(out: &mut W, f: u32, player: u8, queue: &str, i: usize, t: &TeamInQueue, scripts: &ScriptEngine) -> io::Result<()> {
    // `TeamInQueue::m_team->getPrototype()->getName()` (AIPlayer.h): the prototype of the team the
    // entry fills, not the instance key (`proto#id`) the port stores it under. Names are lowercase on
    // both sides (team prototype names keep their script case in C++, `TeamFactory::createTeamPrototype`).
    let key = scripts.team_proto_name(&t.key).to_ascii_lowercase();
    let work: Vec<String> = t
        .work_orders
        .iter()
        .map(|w| {
            let factory = w.factory.map_or(0, |fac| fac.0);
            format!("{}:{}/{}:{}:{factory}", esc(&w.thing), w.completed, w.required_count, u8::from(w.required))
        })
        .collect();
    writeln!(
        out,
        "SNAP f={f} T p={player} q={queue} i={i} key={} prio={} started={} reinf={} recruit={} sent={} work={}",
        esc(&key),
        u8::from(t.priority_build),
        t.frame_started,
        u8::from(t.reinforcement),
        t.reinforcement_id.map_or(0, |id| id.0),
        u8::from(t.sent_to_start_location),
        work.join(",")
    )
}
