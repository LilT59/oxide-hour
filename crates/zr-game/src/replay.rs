//! Replays: record the logic-message stream with frame numbers, the game setup and seed, and a CRC
//! per interval, then play it back (`Recorder.cpp` / `GameLogic::processCommandList`).
//!
//! The format is a small, versioned, dependency-free binary blob ([`Replay::encode`] /
//! [`Replay::decode`]); the command stream is the same [`Command`] the sim applies, so a playback
//! that replays every event before the matching [`Game::step`] reproduces the recording bit for bit
//! (verified by the CRCs).
//!
//! [`Game::step`]: crate::Game::step

use std::collections::BTreeMap;

use zr_sim::{Command, Coord3D, ObjectId};
#[cfg(feature = "possession")]
use zr_sim::PossessInput;

use crate::{AiDifficulty, PlayerSetup};

/// Magic + version at the start of every replay file.
const MAGIC: &[u8; 8] = b"ZRRPLY\0\x01";
/// Current format version. Version 2 adds command tags 34-39 (the weapon-group orders, the internet
/// hack, the mine-clearing detail and the special power at a location with its angle); version 3 adds
/// tag 40 (`EnableRetaliationMode`). Tags 0-39 are unchanged, so version 1 and 2 replays still decode.
pub const FORMAT_VERSION: u32 = 3;
/// The oldest version [`Replay::decode`] accepts.
const MIN_FORMAT_VERSION: u32 = 1;

/// A decode failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayError {
    NotAReplay,
    UnsupportedVersion(u32),
    Truncated,
    BadCommand(u8),
    BadUtf8,
}

impl std::fmt::Display for ReplayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReplayError::NotAReplay => f.write_str("not a replay file"),
            ReplayError::UnsupportedVersion(v) => write!(f, "unsupported replay version {v}"),
            ReplayError::Truncated => f.write_str("truncated replay"),
            ReplayError::BadCommand(t) => write!(f, "unknown command tag {t}"),
            ReplayError::BadUtf8 => f.write_str("invalid UTF-8"),
        }
    }
}

impl std::error::Error for ReplayError {}

/// The game setup a replay was recorded with.
#[derive(Clone, Debug, PartialEq)]
pub struct ReplayHeader {
    pub map: String,
    pub seed: u32,
    pub starting_cash: u32,
    pub superweapon_restriction: u8,
    pub slots: Vec<PlayerSetup>,
}

/// One recorded command, to be submitted before stepping `frame`.
#[derive(Clone, Debug, PartialEq)]
pub struct ReplayEvent {
    pub frame: u32,
    pub player: u8,
    pub command: Command,
}

/// A recorded game: setup, command stream and periodic state CRCs.
#[derive(Clone, Debug, PartialEq)]
pub struct Replay {
    pub header: ReplayHeader,
    pub events: Vec<ReplayEvent>,
    /// Frames between CRCs.
    pub crc_interval: u32,
    /// `(frame, crc)` checkpoints.
    pub crcs: Vec<(u32, u32)>,
}

impl Replay {
    pub fn new(header: ReplayHeader, crc_interval: u32) -> Replay {
        Replay { header, events: Vec::new(), crc_interval: crc_interval.max(1), crcs: Vec::new() }
    }

    /// Records a command that must be submitted before `frame`.
    pub fn record(&mut self, frame: u32, player: u8, command: Command) {
        self.events.push(ReplayEvent { frame, player, command });
    }

    /// Records a state checkpoint.
    pub fn record_crc(&mut self, frame: u32, crc: u32) {
        self.crcs.push((frame, crc));
    }

    /// Events grouped by the frame they apply to, in submission order.
    pub fn by_frame(&self) -> BTreeMap<u32, Vec<&ReplayEvent>> {
        let mut m: BTreeMap<u32, Vec<&ReplayEvent>> = BTreeMap::new();
        for e in &self.events {
            m.entry(e.frame).or_default().push(e);
        }
        m
    }

    /// Serialises the replay to the versioned binary format.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::default();
        w.bytes(MAGIC);
        w.u32(FORMAT_VERSION);
        w.string(&self.header.map);
        w.u32(self.header.seed);
        w.u32(self.header.starting_cash);
        w.u8(self.header.superweapon_restriction);
        w.u32(self.header.slots.len() as u32);
        for s in &self.header.slots {
            write_setup(&mut w, s);
        }
        w.u32(self.crc_interval);
        w.u32(self.events.len() as u32);
        for e in &self.events {
            w.u32(e.frame);
            w.u8(e.player);
            write_command(&mut w, &e.command);
        }
        w.u32(self.crcs.len() as u32);
        for (f, c) in &self.crcs {
            w.u32(*f);
            w.u32(*c);
        }
        w.0
    }

    /// Parses a replay; malformed input is an error, never a panic.
    pub fn decode(data: &[u8]) -> Result<Replay, ReplayError> {
        let mut r = Reader { data, pos: 0 };
        if r.bytes(8)? != MAGIC {
            return Err(ReplayError::NotAReplay);
        }
        let version = r.u32()?;
        if !(MIN_FORMAT_VERSION..=FORMAT_VERSION).contains(&version) {
            return Err(ReplayError::UnsupportedVersion(version));
        }
        let map = r.string()?;
        let seed = r.u32()?;
        let starting_cash = r.u32()?;
        let superweapon_restriction = r.u8()?;
        let n = r.u32()? as usize;
        let mut slots = Vec::with_capacity(n.min(64));
        for _ in 0..n {
            slots.push(read_setup(&mut r)?);
        }
        let crc_interval = r.u32()?.max(1);
        let m = r.u32()? as usize;
        let mut events = Vec::with_capacity(m.min(1 << 20));
        for _ in 0..m {
            let frame = r.u32()?;
            let player = r.u8()?;
            let command = read_command(&mut r)?;
            events.push(ReplayEvent { frame, player, command });
        }
        let k = r.u32()? as usize;
        let mut crcs = Vec::with_capacity(k.min(1 << 20));
        for _ in 0..k {
            crcs.push((r.u32()?, r.u32()?));
        }
        Ok(Replay { header: ReplayHeader { map, seed, starting_cash, superweapon_restriction, slots }, events, crc_interval, crcs })
    }
}

fn write_setup(w: &mut Writer, s: &PlayerSetup) {
    w.string(&s.faction);
    w.u8(s.start_position);
    w.u8(u8::from(s.is_ai));
    w.u8(match s.difficulty {
        AiDifficulty::Easy => 0,
        AiDifficulty::Normal => 1,
        AiDifficulty::Hard => 2,
    });
    w.u8(s.team);
    w.i32(s.color);
    match s.starting_cash {
        Some(c) => {
            w.u8(1);
            w.u32(c);
        }
        None => w.u8(0),
    }
}

fn read_setup(r: &mut Reader<'_>) -> Result<PlayerSetup, ReplayError> {
    let faction = r.string()?;
    let start_position = r.u8()?;
    let is_ai = r.u8()? != 0;
    let difficulty = match r.u8()? {
        0 => AiDifficulty::Easy,
        2 => AiDifficulty::Hard,
        _ => AiDifficulty::Normal,
    };
    let team = r.u8()?;
    let color = r.i32()?;
    let starting_cash = if r.u8()? != 0 { Some(r.u32()?) } else { None };
    let mut s = if is_ai { PlayerSetup::ai(&faction, start_position) } else { PlayerSetup::new(&faction, start_position) };
    s.difficulty = difficulty;
    s.team = team;
    s.color = color;
    s.starting_cash = starting_cash;
    Ok(s)
}

fn write_ids(w: &mut Writer, ids: &[ObjectId]) {
    w.u32(ids.len() as u32);
    for id in ids {
        w.u32(id.0);
    }
}

fn read_ids(r: &mut Reader<'_>) -> Result<Vec<ObjectId>, ReplayError> {
    let n = r.u32()? as usize;
    let mut out = Vec::with_capacity(n.min(1 << 16));
    for _ in 0..n {
        out.push(ObjectId(r.u32()?));
    }
    Ok(out)
}

fn write_coord(w: &mut Writer, c: &Coord3D) {
    w.f32(c.x);
    w.f32(c.y);
    w.f32(c.z);
}

fn read_coord(r: &mut Reader<'_>) -> Result<Coord3D, ReplayError> {
    Ok(Coord3D::new(r.f32()?, r.f32()?, r.f32()?))
}

/// A `Command` tag + fields (see the module doc for the tag list).
pub fn write_command(w: &mut Writer, c: &Command) {
    match c {
        Command::MoveTo { ids, pos } => {
            w.u8(0);
            write_ids(w, ids);
            write_coord(w, pos);
        }
        Command::Attack { ids, target } => {
            w.u8(1);
            write_ids(w, ids);
            w.u32(target.0);
        }
        Command::AttackGround { ids, pos } => {
            w.u8(2);
            write_ids(w, ids);
            write_coord(w, pos);
        }
        Command::ForceAttack { ids, target } => {
            w.u8(3);
            write_ids(w, ids);
            w.u32(target.0);
        }
        Command::Stop { ids } => {
            w.u8(4);
            write_ids(w, ids);
        }
        Command::AttackMove { ids, pos } => {
            w.u8(5);
            write_ids(w, ids);
            write_coord(w, pos);
        }
        Command::Guard { ids, target, mode } => {
            w.u8(6);
            write_ids(w, ids);
            match target {
                zr_sim::GuardTarget::Point(p) => {
                    w.u8(0);
                    write_coord(w, p);
                }
                zr_sim::GuardTarget::Object(id) => {
                    w.u8(1);
                    w.u32(id.0);
                }
            }
            w.u8(match mode {
                zr_sim::GuardMode::Normal => 0,
                zr_sim::GuardMode::WithoutPursuit => 1,
                zr_sim::GuardMode::FlyingUnitsOnly => 2,
            });
        }
        Command::Sell { building } => {
            w.u8(7);
            w.u32(building.0);
        }
        Command::Enter { ids, container } => {
            w.u8(8);
            write_ids(w, ids);
            w.u32(container.0);
        }
        Command::Exit { container, ids } => {
            w.u8(9);
            w.u32(container.0);
            write_ids(w, ids);
        }
        Command::Evacuate { container } => {
            w.u8(10);
            w.u32(container.0);
        }
        Command::QueueUnit { factory, template } => {
            w.u8(11);
            w.u32(factory.0);
            w.string(template);
        }
        Command::QueueUpgrade { building, upgrade } => {
            w.u8(12);
            w.u32(building.0);
            w.string(upgrade);
        }
        Command::CancelUnit { factory, index } => {
            w.u8(13);
            w.u32(factory.0);
            w.u32(*index as u32);
        }
        Command::SetRallyPoint { factory, pos } => {
            w.u8(14);
            w.u32(factory.0);
            write_coord(w, pos);
        }
        Command::Construct { dozer, template, pos, angle } => {
            w.u8(15);
            w.u32(dozer.0);
            w.string(template);
            write_coord(w, pos);
            w.f32(*angle);
        }
        Command::CancelConstruct { dozer } => {
            w.u8(16);
            w.u32(dozer.0);
        }
        Command::Repair { dozer, target } => {
            w.u8(17);
            w.u32(dozer.0);
            w.u32(target.0);
        }
        // Codes 18-20 are the possession messages; a build without the feature rejects them.
        #[cfg(feature = "possession")]
        Command::Possess { id } => {
            w.u8(18);
            w.u32(id.0);
        }
        #[cfg(feature = "possession")]
        Command::PossessInput(p) => {
            w.u8(19);
            w.i16(p.move_x);
            w.i16(p.move_y);
            write_coord(w, &p.aim);
            w.u8(p.buttons);
        }
        #[cfg(feature = "possession")]
        Command::Release => w.u8(20),
        Command::DoSpecialPower { source, power, target } => {
            w.u8(21);
            w.u32(source.0);
            w.string(power);
            match target {
                zr_sim::special_power::SpecialPowerTarget::None => w.u8(0),
                zr_sim::special_power::SpecialPowerTarget::Position(p) => {
                    w.u8(1);
                    write_coord(w, p);
                }
                zr_sim::special_power::SpecialPowerTarget::Object(id) => {
                    w.u8(2);
                    w.u32(id.0);
                }
            }
        }
        Command::PurchaseScience { science } => {
            w.u8(22);
            w.string(science);
        }
        Command::ResumeConstruction { dozer, target } => {
            w.u8(23);
            w.u32(dozer.0);
            w.u32(target.0);
        }
        Command::Dock { ids, target } => {
            w.u8(24);
            write_ids(w, ids);
            w.u32(target.0);
        }
        Command::GetRepaired { ids, target } => {
            w.u8(25);
            write_ids(w, ids);
            w.u32(target.0);
        }
        Command::GetHealed { ids, target } => {
            w.u8(26);
            write_ids(w, ids);
            w.u32(target.0);
        }
        Command::ForceMove { ids, pos } => {
            w.u8(27);
            write_ids(w, ids);
            write_coord(w, pos);
        }
        Command::AddWaypoint { ids, pos } => {
            w.u8(28);
            write_ids(w, ids);
            write_coord(w, pos);
        }
        Command::Scatter { ids } => {
            w.u8(29);
            write_ids(w, ids);
        }
        Command::ToggleOvercharge { ids } => {
            w.u8(30);
            write_ids(w, ids);
        }
        Command::SwitchWeapons { ids, slot } => {
            w.u8(31);
            write_ids(w, ids);
            w.u32(*slot as u32);
        }
        Command::CancelUpgrade { building, upgrade } => {
            w.u8(32);
            w.u32(building.0);
            w.string(upgrade);
        }
        Command::DoWeapon { ids, slot, max_shots } => {
            w.u8(34);
            write_ids(w, ids);
            w.u32(*slot as u32);
            w.i32(*max_shots);
        }
        Command::DoWeaponAtObject { ids, slot, target, max_shots } => {
            w.u8(35);
            write_ids(w, ids);
            w.u32(*slot as u32);
            w.u32(target.0);
            w.i32(*max_shots);
        }
        Command::DoWeaponAtLocation { ids, slot, pos, max_shots } => {
            w.u8(36);
            write_ids(w, ids);
            w.u32(*slot as u32);
            write_coord(w, pos);
            w.i32(*max_shots);
        }
        Command::HackInternet { ids } => {
            w.u8(37);
            write_ids(w, ids);
        }
        Command::SetMineClearingDetail { ids } => {
            w.u8(38);
            write_ids(w, ids);
        }
        Command::DoSpecialPowerAtLocation { source, power, pos, angle } => {
            w.u8(39);
            w.u32(source.0);
            w.string(power);
            write_coord(w, pos);
            match angle {
                Some(a) => {
                    w.u8(1);
                    w.f32(*a);
                }
                None => w.u8(0),
            }
        }
        Command::CombatDrop { ids, target } => {
            w.u8(33);
            write_ids(w, ids);
            match target {
                zr_sim::CombatDropTarget::Position(p) => {
                    w.u8(0);
                    write_coord(w, p);
                }
                zr_sim::CombatDropTarget::Object(id) => {
                    w.u8(1);
                    w.u32(id.0);
                }
            }
        }
        Command::EnableRetaliationMode { enabled } => {
            w.u8(40);
            w.u8(u8::from(*enabled));
        }
    }
}

/// Reads a [`Command`] written by [`write_command`].
pub fn read_command(r: &mut Reader<'_>) -> Result<Command, ReplayError> {
    Ok(match r.u8()? {
        0 => Command::MoveTo { ids: read_ids(r)?, pos: read_coord(r)? },
        1 => Command::Attack { ids: read_ids(r)?, target: ObjectId(r.u32()?) },
        2 => Command::AttackGround { ids: read_ids(r)?, pos: read_coord(r)? },
        3 => Command::ForceAttack { ids: read_ids(r)?, target: ObjectId(r.u32()?) },
        4 => Command::Stop { ids: read_ids(r)? },
        5 => Command::AttackMove { ids: read_ids(r)?, pos: read_coord(r)? },
        6 => {
            let ids = read_ids(r)?;
            let target = match r.u8()? {
                0 => zr_sim::GuardTarget::Point(read_coord(r)?),
                _ => zr_sim::GuardTarget::Object(ObjectId(r.u32()?)),
            };
            let mode = match r.u8()? {
                1 => zr_sim::GuardMode::WithoutPursuit,
                2 => zr_sim::GuardMode::FlyingUnitsOnly,
                _ => zr_sim::GuardMode::Normal,
            };
            Command::Guard { ids, target, mode }
        }
        7 => Command::Sell { building: ObjectId(r.u32()?) },
        8 => Command::Enter { ids: read_ids(r)?, container: ObjectId(r.u32()?) },
        9 => Command::Exit { container: ObjectId(r.u32()?), ids: read_ids(r)? },
        10 => Command::Evacuate { container: ObjectId(r.u32()?) },
        11 => Command::QueueUnit { factory: ObjectId(r.u32()?), template: r.string()? },
        12 => Command::QueueUpgrade { building: ObjectId(r.u32()?), upgrade: r.string()? },
        13 => Command::CancelUnit { factory: ObjectId(r.u32()?), index: r.u32()? as usize },
        14 => Command::SetRallyPoint { factory: ObjectId(r.u32()?), pos: read_coord(r)? },
        15 => Command::Construct { dozer: ObjectId(r.u32()?), template: r.string()?, pos: read_coord(r)?, angle: r.f32()? },
        16 => Command::CancelConstruct { dozer: ObjectId(r.u32()?) },
        17 => Command::Repair { dozer: ObjectId(r.u32()?), target: ObjectId(r.u32()?) },
        #[cfg(feature = "possession")]
        18 => Command::Possess { id: ObjectId(r.u32()?) },
        #[cfg(feature = "possession")]
        19 => Command::PossessInput(PossessInput { move_x: r.i16()?, move_y: r.i16()?, aim: read_coord(r)?, buttons: r.u8()? }),
        #[cfg(feature = "possession")]
        20 => Command::Release,
        21 => {
            let source = ObjectId(r.u32()?);
            let power = r.string()?;
            let target = match r.u8()? {
                1 => zr_sim::special_power::SpecialPowerTarget::Position(read_coord(r)?),
                2 => zr_sim::special_power::SpecialPowerTarget::Object(ObjectId(r.u32()?)),
                _ => zr_sim::special_power::SpecialPowerTarget::None,
            };
            Command::DoSpecialPower { source, power, target }
        }
        22 => Command::PurchaseScience { science: r.string()? },
        23 => Command::ResumeConstruction { dozer: ObjectId(r.u32()?), target: ObjectId(r.u32()?) },
        24 => Command::Dock { ids: read_ids(r)?, target: ObjectId(r.u32()?) },
        25 => Command::GetRepaired { ids: read_ids(r)?, target: ObjectId(r.u32()?) },
        26 => Command::GetHealed { ids: read_ids(r)?, target: ObjectId(r.u32()?) },
        27 => Command::ForceMove { ids: read_ids(r)?, pos: read_coord(r)? },
        28 => Command::AddWaypoint { ids: read_ids(r)?, pos: read_coord(r)? },
        29 => Command::Scatter { ids: read_ids(r)? },
        30 => Command::ToggleOvercharge { ids: read_ids(r)? },
        31 => Command::SwitchWeapons { ids: read_ids(r)?, slot: r.u32()? as usize },
        32 => Command::CancelUpgrade { building: ObjectId(r.u32()?), upgrade: r.string()? },
        33 => {
            let ids = read_ids(r)?;
            let target = match r.u8()? {
                0 => zr_sim::CombatDropTarget::Position(read_coord(r)?),
                _ => zr_sim::CombatDropTarget::Object(ObjectId(r.u32()?)),
            };
            Command::CombatDrop { ids, target }
        }
        34 => {
            let ids = read_ids(r)?;
            Command::DoWeapon { ids, slot: r.u32()? as usize, max_shots: r.i32()? }
        }
        35 => {
            let ids = read_ids(r)?;
            Command::DoWeaponAtObject { ids, slot: r.u32()? as usize, target: ObjectId(r.u32()?), max_shots: r.i32()? }
        }
        36 => {
            let ids = read_ids(r)?;
            Command::DoWeaponAtLocation { ids, slot: r.u32()? as usize, pos: read_coord(r)?, max_shots: r.i32()? }
        }
        37 => Command::HackInternet { ids: read_ids(r)? },
        38 => Command::SetMineClearingDetail { ids: read_ids(r)? },
        39 => {
            let source = ObjectId(r.u32()?);
            let power = r.string()?;
            let pos = read_coord(r)?;
            let angle = if r.u8()? != 0 { Some(r.f32()?) } else { None };
            Command::DoSpecialPowerAtLocation { source, power, pos, angle }
        }
        40 => Command::EnableRetaliationMode { enabled: r.u8()? != 0 },
        t => return Err(ReplayError::BadCommand(t)),
    })
}

/// A little-endian byte writer.
#[derive(Default)]
pub struct Writer(Vec<u8>);

impl Writer {
    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }
    pub fn bytes(&mut self, b: &[u8]) {
        self.0.extend_from_slice(b);
    }
    pub fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    pub fn i16(&mut self, v: i16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    pub fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    pub fn i32(&mut self, v: i32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    pub fn f32(&mut self, v: f32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    pub fn string(&mut self, s: &str) {
        self.u32(s.len() as u32);
        self.0.extend_from_slice(s.as_bytes());
    }
}

/// A bounds-checked little-endian byte reader.
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Reader { data, pos: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], ReplayError> {
        let end = self.pos.checked_add(n).ok_or(ReplayError::Truncated)?;
        let s = self.data.get(self.pos..end).ok_or(ReplayError::Truncated)?;
        self.pos = end;
        Ok(s)
    }
    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8], ReplayError> {
        self.take(n)
    }
    pub fn u8(&mut self) -> Result<u8, ReplayError> {
        Ok(self.take(1)?[0])
    }
    pub fn i16(&mut self) -> Result<i16, ReplayError> {
        Ok(i16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    pub fn u32(&mut self) -> Result<u32, ReplayError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn i32(&mut self) -> Result<i32, ReplayError> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn f32(&mut self) -> Result<f32, ReplayError> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn string(&mut self) -> Result<String, ReplayError> {
        let n = self.u32()? as usize;
        let b = self.take(n.min(1 << 20))?;
        std::str::from_utf8(b).map(str::to_owned).map_err(|_| ReplayError::BadUtf8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(c: Command) -> Command {
        let mut w = Writer::default();
        write_command(&mut w, &c);
        let bytes = w.into_bytes();
        let mut r = Reader::new(&bytes);
        read_command(&mut r).expect("decodes")
    }

    /// The dock / get-repaired / get-healed tags (24-26) survive an encode/decode round-trip.
    #[test]
    fn new_dock_commands_round_trip() {
        let ids = vec![ObjectId(3), ObjectId(7)];
        assert_eq!(round_trip(Command::Dock { ids: ids.clone(), target: ObjectId(11) }), Command::Dock { ids: ids.clone(), target: ObjectId(11) });
        assert_eq!(
            round_trip(Command::GetRepaired { ids: ids.clone(), target: ObjectId(12) }),
            Command::GetRepaired { ids: ids.clone(), target: ObjectId(12) }
        );
        assert_eq!(round_trip(Command::GetHealed { ids, target: ObjectId(13) }), Command::GetHealed { ids: vec![ObjectId(3), ObjectId(7)], target: ObjectId(13) });
    }

    /// The player-order tags (27-33) survive an encode/decode round-trip.
    #[test]
    fn new_player_order_commands_round_trip() {
        let ids = vec![ObjectId(3), ObjectId(7)];
        let p = Coord3D::new(1.0, 2.0, 3.0);
        assert_eq!(round_trip(Command::ForceMove { ids: ids.clone(), pos: p }), Command::ForceMove { ids: ids.clone(), pos: p });
        assert_eq!(round_trip(Command::AddWaypoint { ids: ids.clone(), pos: p }), Command::AddWaypoint { ids: ids.clone(), pos: p });
        assert_eq!(round_trip(Command::Scatter { ids: ids.clone() }), Command::Scatter { ids: ids.clone() });
        assert_eq!(round_trip(Command::ToggleOvercharge { ids: ids.clone() }), Command::ToggleOvercharge { ids: ids.clone() });
        assert_eq!(round_trip(Command::SwitchWeapons { ids: ids.clone(), slot: 1 }), Command::SwitchWeapons { ids: ids.clone(), slot: 1 });
        assert_eq!(
            round_trip(Command::CancelUpgrade { building: ObjectId(9), upgrade: "UpgradeName".into() }),
            Command::CancelUpgrade { building: ObjectId(9), upgrade: "UpgradeName".into() }
        );
        assert_eq!(
            round_trip(Command::CombatDrop { ids: ids.clone(), target: zr_sim::CombatDropTarget::Position(p) }),
            Command::CombatDrop { ids: ids.clone(), target: zr_sim::CombatDropTarget::Position(p) }
        );
        assert_eq!(
            round_trip(Command::CombatDrop { ids, target: zr_sim::CombatDropTarget::Object(ObjectId(42)) }),
            Command::CombatDrop { ids: vec![ObjectId(3), ObjectId(7)], target: zr_sim::CombatDropTarget::Object(ObjectId(42)) }
        );
    }

    /// The weapon-group, hack, mine-clearing and special-power-at-location tags (34-39) survive an
    /// encode/decode round-trip, including a negative shot count and an absent angle.
    #[test]
    fn new_weapon_hack_and_angle_commands_round_trip() {
        let ids = vec![ObjectId(3), ObjectId(7)];
        let p = Coord3D::new(1.0, 2.0, 3.0);
        let cmds = [
            Command::DoWeapon { ids: ids.clone(), slot: 2, max_shots: -1 },
            Command::DoWeaponAtObject { ids: ids.clone(), slot: 0, target: ObjectId(42), max_shots: 5 },
            Command::DoWeaponAtLocation { ids: ids.clone(), slot: 1, pos: p, max_shots: 0x7fff_ffff },
            Command::HackInternet { ids: ids.clone() },
            Command::SetMineClearingDetail { ids: ids.clone() },
            Command::DoSpecialPowerAtLocation { source: ObjectId(9), power: "Power".into(), pos: p, angle: Some(1.5) },
            Command::DoSpecialPowerAtLocation { source: ObjectId(9), power: "Power".into(), pos: p, angle: None },
            // Tag 40 (format version 3).
            Command::EnableRetaliationMode { enabled: true },
            Command::EnableRetaliationMode { enabled: false },
        ];
        for c in cmds {
            assert_eq!(round_trip(c.clone()), c);
        }
    }

    /// A version-1 file (written before tags 34-39 existed) still decodes under the version-3 reader, and the
    /// current version decodes; a version this reader does not know is rejected.
    #[test]
    fn version_one_replays_still_decode() {
        let header = ReplayHeader { map: "m.map".into(), seed: 7, starting_cash: 0, superweapon_restriction: 0, slots: Vec::new() };
        let mut replay = Replay::new(header, 300);
        replay.record(4, 0, Command::Stop { ids: vec![ObjectId(1)] });
        let mut bytes = replay.encode();
        // The version word follows the 8-byte magic.
        bytes[8..12].copy_from_slice(&1u32.to_le_bytes());
        assert_eq!(Replay::decode(&bytes).expect("v1 decodes"), replay);
        bytes[8..12].copy_from_slice(&3u32.to_le_bytes());
        assert_eq!(Replay::decode(&bytes).expect("the current version decodes"), replay);
        bytes[8..12].copy_from_slice(&4u32.to_le_bytes());
        assert_eq!(Replay::decode(&bytes), Err(ReplayError::UnsupportedVersion(4)));
    }
}
