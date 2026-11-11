//! Standing per-object trace for diffing the port against the C++ build (format and field meanings in
//! `docs/HARNESS.md`, "Per-object trace (`ZR_TRACE`)"); the diff is `tools/trace_diff.py`.
//!
//! `ZR_TRACE=<file>` turns the writer on ([`FileTrace::from_env`]); `ZR_TRACE_IDS=<comma ids | all>`
//! filters the objects and `ZR_TRACE_FRAMES=<a-b>` the frames, with the C++ writer's grammar
//! ([`settings`]). A malformed value or an unwritable path prints one warning and leaves the trace
//! off, as the C++ writer does. Off, nothing here runs. On, the writer only reads the sim, so the CRC
//! of a run is unchanged.
//!
//! One line per traced object per frame, at the snapshot writer's print point (after `Game::step`),
//! flushed per frame like the C++ `fflush`: `TRACE f=<frame> id=<id> <name>=<value> ...`. The C++
//! side prints the same line from `GameLogicTrace.cpp`; a field one side cannot read is `-` (the gaps,
//! and the six fields `trace_diff.py` skips by default, are in `docs/HARNESS.md`).

use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::PathBuf;

use zr_sim::ai_states::AiStateType;
use zr_sim::combat::AttackPhase;
use zr_sim::math::Coord3D;
use zr_sim::weapon::WeaponStatus;
use zr_sim::{GameLogic, Object, ObjectId};

use crate::game::Game;
use crate::snapshot::{esc, Snapshot};

/// Which objects the trace includes (`ZR_TRACE_IDS`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IdFilter {
    All,
    /// Ascending, no duplicates; `0` (an invalid id) is rejected by [`IdFilter::parse`].
    Ids(Vec<u32>),
}

impl IdFilter {
    /// `None`, an empty value or `all` is every object; otherwise a comma-separated id list.
    pub fn parse(value: Option<&str>) -> io::Result<Self> {
        let Some(value) = value.map(str::trim) else { return Ok(IdFilter::All) };
        if value.is_empty() || value.eq_ignore_ascii_case("all") {
            return Ok(IdFilter::All);
        }
        let mut ids = Vec::new();
        for token in value.split(',') {
            let token = token.trim();
            if token.is_empty() {
                continue;
            }
            match token.parse::<u32>() {
                Ok(id) if id != 0 => ids.push(id),
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("ZR_TRACE_IDS={value}: want comma-separated object ids or all"),
                    ))
                }
            }
        }
        if ids.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("ZR_TRACE_IDS={value}: want comma-separated object ids or all"),
            ));
        }
        ids.sort_unstable();
        ids.dedup();
        Ok(IdFilter::Ids(ids))
    }

    pub fn contains(&self, id: u32) -> bool {
        match self {
            IdFilter::All => true,
            IdFilter::Ids(ids) => ids.binary_search(&id).is_ok(),
        }
    }
}

/// The inclusive frame range the trace covers (`ZR_TRACE_FRAMES`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameRange {
    pub first: u32,
    pub last: u32,
}

impl FrameRange {
    pub const ALL: FrameRange = FrameRange { first: 0, last: u32::MAX };

    /// `None` or an empty value is every frame; otherwise `a-b` (inclusive) or a single `a`.
    pub fn parse(value: Option<&str>) -> io::Result<Self> {
        let Some(value) = value.map(str::trim) else { return Ok(FrameRange::ALL) };
        if value.is_empty() {
            return Ok(FrameRange::ALL);
        }
        let bad = || {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("ZR_TRACE_FRAMES={value}: want a-b (inclusive) or a single frame"),
            )
        };
        let (a, b) = match value.split_once('-') {
            Some((a, b)) => (a.trim().parse::<u32>().map_err(|_| bad())?, b.trim().parse::<u32>().map_err(|_| bad())?),
            None => {
                let a = value.parse::<u32>().map_err(|_| bad())?;
                (a, a)
            }
        };
        if b < a {
            return Err(bad());
        }
        Ok(FrameRange { first: a, last: b })
    }

    pub fn contains(&self, frame: u32) -> bool {
        frame >= self.first && frame <= self.last
    }
}

/// The writer settings read from the environment (see [`settings`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub path: PathBuf,
    pub ids: IdFilter,
    pub frames: FrameRange,
}

/// The trace settings for the given environment values (`None` = unset). No path (unset or empty)
/// gives `Ok(None)`: the trace is off.
///
/// The grammar is the C++ writer's (`GameLogicTrace.cpp`, `readTraceSettings`): `ZR_TRACE_IDS` is
/// `all` (ASCII case-insensitive, surrounding whitespace allowed) or a comma-separated list of
/// decimal object ids (`1..=u32::MAX`; empty tokens are skipped, and a list with no id is
/// malformed); `ZR_TRACE_FRAMES` is `a-b` (inclusive, 0 allowed) or a single `a`, empty = all frames.
/// A malformed value is an error here; [`env_settings`] turns it into the C++ writer's warning + off.
pub fn settings(path: Option<&OsStr>, ids: Option<&str>, frames: Option<&str>) -> io::Result<Option<Settings>> {
    let Some(path) = path.filter(|p| !p.is_empty()) else {
        return Ok(None);
    };
    Ok(Some(Settings { path: PathBuf::from(path), ids: IdFilter::parse(ids)?, frames: FrameRange::parse(frames)? }))
}

/// [`settings`] for the process environment: a malformed `ZR_TRACE_IDS`/`ZR_TRACE_FRAMES` prints one
/// warning and turns the trace off, like the C++ writer (`GameLogicTrace.cpp`, `readTraceSettings`),
/// instead of failing the run.
pub fn env_settings() -> Option<Settings> {
    let path = std::env::var_os("ZR_TRACE");
    let ids = std::env::var("ZR_TRACE_IDS").ok();
    let frames = std::env::var("ZR_TRACE_FRAMES").ok();
    match settings(path.as_deref(), ids.as_deref(), frames.as_deref()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("ZR_TRACE: {e} (trace off)");
            None
        }
    }
}

/// A trace writer over a file (the `ZR_TRACE` target).
pub type FileTrace = Trace<BufWriter<File>>;

impl FileTrace {
    /// The writer named by the environment (`ZR_TRACE`, `ZR_TRACE_IDS`, `ZR_TRACE_FRAMES`), or `None`
    /// when `ZR_TRACE` is unset or empty. Creates (truncates) the file. A malformed value or an
    /// unwritable path prints a warning and gives `None`, matching the C++ writer.
    pub fn from_env() -> io::Result<Option<Self>> {
        let Some(s) = env_settings() else { return Ok(None) };
        match File::create(&s.path) {
            Ok(f) => Ok(Some(Trace::new(BufWriter::new(f), s.ids, s.frames))),
            Err(e) => {
                eprintln!("ZR_TRACE={}: {e} (trace off)", s.path.display());
                Ok(None)
            }
        }
    }
}

/// Writes the traced objects' lines for every due frame, once each.
pub struct Trace<W: Write> {
    out: W,
    ids: IdFilter,
    frames: FrameRange,
    last: Option<u32>,
}

impl<W: Write> Trace<W> {
    pub fn new(out: W, ids: IdFilter, frames: FrameRange) -> Self {
        Trace { out, ids, frames, last: None }
    }

    /// Writes the current frame's lines when the frame is in range and was not written yet, then
    /// flushes: the C++ writer calls `fflush` after every block (`GameLogicTrace.cpp:720`), so a live
    /// (or killed) run leaves the same complete frames on both sides.
    pub fn write_if_due(&mut self, g: &Game) -> io::Result<()> {
        let frame = g.frame();
        if !self.frames.contains(frame) || self.last == Some(frame) {
            return Ok(());
        }
        self.last = Some(frame);
        write_frame(&mut self.out, g.logic(), &self.ids)?;
        self.out.flush()
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

/// `Game::run_frames(n)` with the snapshot writer and/or the trace writer driven from the same print
/// point: once before the first frame (frame 0, like the C++ zero block) and after each frame. With
/// both `None` it is exactly `g.run_frames(n)`. The snapshot is written before the trace, matching
/// `GameLogic::update`'s call order.
pub fn run_frames<S: Write, T: Write>(
    g: &mut Game,
    n: u32,
    snap: Option<&mut Snapshot<S>>,
    trace: Option<&mut Trace<T>>,
) -> io::Result<()> {
    let Some(trace) = trace else {
        return crate::snapshot::run_frames(g, n, snap);
    };
    let mut snap = snap;
    if let Some(s) = snap.as_mut() {
        s.write_if_due(g)?;
    }
    trace.write_if_due(g)?;
    for _ in 0..n {
        g.step();
        if let Some(s) = snap.as_mut() {
            s.write_if_due(g)?;
        }
        trace.write_if_due(g)?;
    }
    Ok(())
}

/// One line per traced object, in object-list (id) order.
pub fn write_frame<W: Write>(out: &mut W, logic: &GameLogic, ids: &IdFilter) -> io::Result<()> {
    let frame = logic.frame();
    for o in logic.object_list() {
        if !ids.contains(o.id.0) {
            continue;
        }
        writeln!(out, "{}", object_line(frame, o))?;
    }
    Ok(())
}

/// The `ai` state name (the enclosing state, see [`crate::snapshot::shown_state`]).
fn ai_state_name(o: &Object) -> String {
    format!("{:?}", crate::snapshot::shown_state(&o.ai.machine))
}

/// `x,y,z` with 4 decimal places each: the value of a packed position field.
pub fn pos3(p: Coord3D) -> String {
    format!("{:.4},{:.4},{:.4}", p.x, p.y, p.z)
}

fn bits(v: f32) -> String {
    format!("{:08x}", v.to_bits())
}

/// The sub-machine state the current AI state runs (dock / guard / attack), or `-`.
///
/// The attack names are the `AttackStateMachine` state ids (`GameLogicTrace.cpp:352-362`): a
/// position attack's `CHASE_TARGET` state *is* `AIAttackApproachTargetState` (`AIStates.cpp:367-376`),
/// so the port's `Approach` phase prints `Pursue` there. `AI_ATTACK_MOVE_TO`/`AI_HUNT`/`AI_ATTACK_AREA`
/// print `-`: their immediate sub-machine is the attack-move/hunt/area machine, not the attack
/// machine, and the C++ writer names only the four `AI_ATTACK_*` states (`GameLogicTrace.cpp:443-466`).
fn substate(o: &Object) -> String {
    if o.template.ai_update.is_none() {
        return "-".to_owned();
    }
    let machine = &o.ai.machine;
    match crate::snapshot::shown_state(machine) {
        AiStateType::Dock => format!("{:?}", machine.dock_sub),
        AiStateType::Guard
        | AiStateType::GuardInner
        | AiStateType::GuardOuter
        | AiStateType::GuardReturn
        | AiStateType::GuardIdle
        | AiStateType::GuardGetCrate => format!("{:?}", machine.guard_sub),
        AiStateType::AttackObject | AiStateType::ForceAttackObject | AiStateType::AttackAndFollowObject => {
            match o.combat.attack.as_ref() {
                Some(a) => format!("{:?}", a.phase),
                None => "-".to_owned(),
            }
        }
        // `AI_ATTACK_POSITION`: the chase state is the approach state, so `Approach` prints `Pursue`.
        AiStateType::AttackPosition => match o.combat.attack.as_ref() {
            Some(a) => match a.phase {
                AttackPhase::Approach => "Pursue".to_owned(),
                other => format!("{other:?}"),
            },
            None => "-".to_owned(),
        },
        _ => "-".to_owned(),
    }
}

/// The snapshot writer's `atk` mapping (`snapshot.rs`, `victimText`): the AI's current victim, else `ground` for an
/// attack on a position, else `0`.
fn victim(o: &Object) -> String {
    if o.ai.machine.current_victim != ObjectId::INVALID {
        o.ai.machine.current_victim.0.to_string()
    } else if o.ai.machine.current == AiStateType::AttackPosition && o.combat.attack.is_some() {
        "ground".to_owned()
    } else {
        "0".to_owned()
    }
}

fn weapon_status(status: WeaponStatus) -> &'static str {
    match status {
        WeaponStatus::ReadyToFire => "ReadyToFire",
        WeaponStatus::BetweenFiringShots => "BetweenFiringShots",
        WeaponStatus::ReloadingClip => "ReloadingClip",
        WeaponStatus::PreAttack => "PreAttack",
        WeaponStatus::OutOfAmmo => "OutOfAmmo",
    }
}

/// One object's `TRACE` line (format in `docs/HARNESS.md`). Every field is printed; a value the port
/// has no state for is `-`, like the C++ side.
pub fn object_line(frame: u32, o: &Object) -> String {
    let mut line = format!("TRACE f={frame} id={} tmpl={}", o.id.0, esc(&o.template.name));
    let p = o.position;
    line.push_str(&format!(
        " x={:.4} y={:.4} z={:.4} xb={} yb={} zb={}",
        p.x,
        p.y,
        p.z,
        bits(p.x),
        bits(p.y),
        bits(p.z)
    ));
    line.push_str(&format!(" ang={:.4} angb={}", o.angle, bits(o.angle)));
    match &o.physics {
        Some(ph) => {
            let v = ph.velocity;
            line.push_str(&format!(
                " vx={:.4} vy={:.4} vz={:.4} vxb={} vyb={} vzb={} vmag={:.4} pfall={} pffree={} pfstun={} pfcoll={} pfturn={}",
                v.x,
                v.y,
                v.z,
                bits(v.x),
                bits(v.y),
                bits(v.z),
                ph.velocity_magnitude(),
                u8::from(ph.allow_to_fall),
                u8::from(ph.is_in_freefall),
                u8::from(ph.is_stunned),
                u8::from(ph.allow_collide_force),
                ph.turning
            ));
        }
        None => line.push_str(" vx=- vy=- vz=- vxb=- vyb=- vzb=- vmag=- pfall=- pffree=- pfstun=- pfcoll=- pfturn=-"),
    }
    let has_ai = o.template.ai_update.is_some();
    // The locomotor's runtime flags (`Locomotor::IS_BRAKING`, `m_brakingFactor`, `m_donutTimer`): the
    // port keeps them on `Physics`; they print only when the object has an `AIUpdateInterface` (the
    // port's `template.ai_update`) and a current locomotor, like `ai ? ai->getCurLocomotor() : nullptr`
    // (`GameLogicTrace.cpp:560-573`).
    let loco = has_ai.then(|| o.locomotor()).flatten();
    match (&o.physics, loco) {
        (Some(ph), Some(_)) => line.push_str(&format!(
            " lbrake={} lbf={:.4} ldonut={}",
            u8::from(ph.braking),
            ph.braking_factor,
            ph.donut_timer
        )),
        _ => line.push_str(" lbrake=- lbf=- ldonut=-"),
    }
    let ai = if has_ai { ai_state_name(o) } else { "-".to_owned() };
    line.push_str(&format!(" ai={ai}"));
    if has_ai {
        line.push_str(&format!(
            " aisleep={} aiwake=- aiphase=- sub={} goaltype={:?} gx={}",
            o.ai.machine.sleep_till,
            substate(o),
            o.ai.locomotor_goal_type,
            pos3(o.ai.locomotor_goal_data)
        ));
    } else {
        line.push_str(" aisleep=- aiwake=- aiphase=- sub=- goaltype=- gx=-");
    }
    match o.ai.raw_path.as_ref().filter(|p| !p.nodes.is_empty()) {
        Some(path) => {
            let first = &path.nodes[0];
            let last = path.nodes.last().expect("non-empty");
            let next = first.next_optimized.and_then(|i| path.nodes.get(i));
            line.push_str(&format!(
                " pcount={} pfirst={} plast={} pnext={}",
                path.nodes.len(),
                pos3(first.pos),
                pos3(last.pos),
                next.map_or_else(|| "-".to_owned(), |n| pos3(n.pos))
            ));
        }
        None => line.push_str(" pcount=0 pfirst=- plast=- pnext=-"),
    }
    line.push_str(&format!(" victim={}", victim(o)));
    // `Object::getCurrentWeapon(&slot)` (`Object.cpp:1428-1436`) returns null when the current slot is
    // empty, even with other slots filled (`WeaponSet::updateWeaponSet` keeps `m_curWeapon` at
    // PRIMARY deliberately, WeaponSet.cpp:349-357), and the trace then prints `-` for the slot.
    if o.combat.weapons.is_empty() {
        line.push_str(" wslot=- wstat=- wnext=-");
    } else {
        let slot = o.combat.current_weapon_slot;
        match o.combat.weapons.weapon(slot) {
            Some(w) => line.push_str(&format!(
                " wslot={slot} wstat={} wnext={}",
                weapon_status(w.status(frame)),
                w.when_we_can_fire_again()
            )),
            None => line.push_str(" wslot=- wstat=- wnext=-"),
        }
    }
    // `dtask`: `DozerAIUpdate::m_currentTask` is not modelled by the port (gap).
    // `dbuild`: the build sub-task (`DozerAIUpdate::getBuildSubTask`), the build action machine.
    let dozer = o.template.dozer.is_some();
    line.push_str(" dtask=- dbuild=");
    if dozer {
        use zr_sim::dozer::DozerActionSub;
        let name = match o.ai.dozer_action.sub {
            DozerActionSub::SelectBuildDock => "SelectBuildDock",
            DozerActionSub::MovingToBuildDock => "MovingToBuildDock",
            DozerActionSub::DoBuildAtDock => "DoBuildAtDock",
            _ => "-",
        };
        line.push_str(name);
        line.push_str(&format!(" dact={:?}", o.ai.dozer_action.sub));
    } else {
        line.push_str("- dact=-");
    }
    // The supply state machine's state: a plain supply truck and the Chinook (`ChinookAIUpdate` derives
    // from `SupplyTruckAIUpdate` and calls its `update`) both run it, so both print it here. A worker
    // implements the supply interface without a machine, and a real jet has no supply interface; C++
    // prints `-` for both (the interface default `-1`), and so does the port.
    let has_supply_machine = o.template.supply_truck.is_some() && o.template.dozer.is_none() && o.template.jet.is_none();
    if has_supply_machine {
        line.push_str(&format!(" ssub={:?}", o.supply_ai.state));
    } else {
        line.push_str(" ssub=-");
    }
    line.push_str(&format!(" cont={}", o.contained_by.map_or(0, |c| c.0)));
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_filter_parses_all_and_lists() {
        assert_eq!(IdFilter::parse(None).unwrap(), IdFilter::All);
        assert_eq!(IdFilter::parse(Some("")).unwrap(), IdFilter::All);
        assert_eq!(IdFilter::parse(Some(" ALL ")).unwrap(), IdFilter::All);
        assert_eq!(IdFilter::parse(Some("9, 3,9")).unwrap(), IdFilter::Ids(vec![3, 9]));
        assert!(IdFilter::parse(Some("0")).is_err());
        assert!(IdFilter::parse(Some("3,x")).is_err());
        assert!(IdFilter::parse(Some(",")).is_err());
        let f = IdFilter::Ids(vec![3, 9]);
        assert!(!f.contains(0) && f.contains(3) && f.contains(9) && !f.contains(10));
    }

    #[test]
    fn frame_range_parses_ranges_and_single_frames() {
        assert_eq!(FrameRange::parse(None).unwrap(), FrameRange::ALL);
        assert_eq!(FrameRange::parse(Some("")).unwrap(), FrameRange::ALL);
        assert_eq!(FrameRange::parse(Some("1550-1700")).unwrap(), FrameRange { first: 1550, last: 1700 });
        assert_eq!(FrameRange::parse(Some("42")).unwrap(), FrameRange { first: 42, last: 42 });
        assert!(FrameRange::parse(Some("1700-1550")).is_err());
        assert!(FrameRange::parse(Some("a-b")).is_err());
        let r = FrameRange { first: 1550, last: 1700 };
        assert!(!r.contains(1549) && r.contains(1550) && r.contains(1700) && !r.contains(1701));
    }

    #[test]
    fn settings_off_without_a_path() {
        assert_eq!(settings(None, Some("1"), Some("0-1")).unwrap(), None);
        assert_eq!(settings(Some(OsStr::new("")), None, None).unwrap(), None);
        let s = settings(Some(OsStr::new("t.txt")), None, None).unwrap().unwrap();
        assert_eq!(s.path, PathBuf::from("t.txt"));
        assert_eq!(s.ids, IdFilter::All);
        assert_eq!(s.frames, FrameRange::ALL);
        assert!(settings(Some(OsStr::new("t.txt")), Some("x"), None).is_err());
    }
}
