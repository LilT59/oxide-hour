//! Script tree stored in the `PlayerScriptsList` chunk (inside `SidesList`).
//!
//! Structural decoding only; nothing here executes scripts. Ported from the readers in
//! `GeneralsMD/Code/GameEngine/Source/GameLogic/ScriptEngine/Scripts.cpp`:
//! `ScriptList::ParseScriptsDataChunk` / `ParseScriptListDataChunk`, `ScriptGroup::ParseGroupDataChunk`,
//! `Script::ParseScript`, `OrCondition::ParseOrConditionDataChunk`, `Condition::ParseConditionDataChunk`,
//! `ScriptAction::ParseAction`, `Parameter::ReadParameter`.
//!
//! Layout (chunk nesting):
//! ```text
//! PlayerScriptsList            one ScriptList per side, in side order
//!   ScriptList
//!     Script*  ScriptGroup*    (the engine keeps the two kinds in separate chains)
//!       ScriptGroup: name, active, [subroutine if v2], then Script*
//!         Script: name, comment, condition comment, action comment, active, one-shot,
//!                 easy, normal, hard, subroutine, [delay seconds if v2], then
//!           OrCondition*       each holds Condition* (AND-ed); the ORs are OR-ed
//!           ScriptAction*      run when the condition is true
//!           ScriptActionFalse* run when it is false
//! ```
//! Condition and action chunks: `i32 type`, [`name key` if version >= 4 (conditions) /
//! >= 2 (actions)], `i32 numParms`, then parameters.
//!
//! Not reproduced (engine-side "heal old files" fix-ups): padding of missing parameters for
//! old chunk versions, `Fundamentalist` -> `GLA` renames, and the template-arity check that
//! turns mismatched conditions / actions into `CONDITION_FALSE` / `NO_OP`. Parameters are
//! exposed exactly as stored; compare against [`Condition::name`] / [`Action::name`] if needed.

use crate::chunk::{open, Chunk, ChunkError, ChunkIter, Cursor, Dict, Names, Result};
use crate::map::count;
use crate::script_names::{ACTION_NAMES, CONDITION_NAMES, PARAMETER_NAMES};

/// `MAX_PARMS` in `Scripts.h`: the engine's fixed parameter array size.
pub const MAX_PARMS: usize = 12;

/// `Parameter::ParameterType` values worth special-casing.
pub mod param {
    pub const INT: i32 = 0;
    pub const REAL: i32 = 1;
    /// Any script name.
    pub const SCRIPT: i32 = 2;
    pub const TEAM: i32 = 3;
    pub const COUNTER: i32 = 4;
    pub const FLAG: i32 = 5;
    pub const SIDE: i32 = 11;
    pub const SCRIPT_SUBROUTINE: i32 = 13;
    pub const COORD3D: i32 = 16;
}

/// `"<This Team>"`: a team parameter that is never qualified (`Parameter::qualify`).
pub const THIS_TEAM: &str = "<This Team>";

/// Parameter payload: `COORD3D` stores three reals, every other type stores all of
/// `int`, `real`, `string` (only the field matching the type is meaningful).
#[derive(Debug, Clone, PartialEq)]
pub enum ParamValue {
    Coord3D([f32; 3]),
    Plain { int: i32, real: f32, string: String },
}

/// One script parameter (`Parameter::ReadParameter`).
#[derive(Debug, Clone, PartialEq)]
pub struct Parameter {
    /// Raw `Parameter::ParameterType` id.
    pub kind: i32,
    pub value: ParamValue,
}

impl Parameter {
    /// `Parameter::ParameterType` name for [`Self::kind`], e.g. `"WAYPOINT"`.
    pub fn kind_name(&self) -> Option<&'static str> {
        usize::try_from(self.kind).ok().and_then(|i| PARAMETER_NAMES.get(i).copied())
    }
    /// The string field (names of scripts, teams, waypoints, ...), if this isn't a coordinate.
    pub fn string(&self) -> Option<&str> {
        match &self.value {
            ParamValue::Plain { string, .. } => Some(string),
            ParamValue::Coord3D(_) => None,
        }
    }
    pub fn int(&self) -> Option<i32> {
        match &self.value {
            ParamValue::Plain { int, .. } => Some(*int),
            ParamValue::Coord3D(_) => None,
        }
    }
    pub fn real(&self) -> Option<f32> {
        match &self.value {
            ParamValue::Plain { real, .. } => Some(*real),
            ParamValue::Coord3D(_) => None,
        }
    }
    pub fn coord(&self) -> Option<[f32; 3]> {
        match &self.value {
            ParamValue::Coord3D(c) => Some(*c),
            ParamValue::Plain { .. } => None,
        }
    }

    fn read(r: &mut Cursor) -> Result<Self> {
        let kind = r.i32()?;
        let value = if kind == param::COORD3D {
            ParamValue::Coord3D([r.f32()?, r.f32()?, r.f32()?])
        } else {
            ParamValue::Plain { int: r.i32()?, real: r.f32()?, string: r.ascii()? }
        };
        Ok(Self { kind, value })
    }

    /// `Parameter::qualify`: per-player qualification of skirmish script parameters. `SCRIPT`,
    /// `COUNTER`, `FLAG`, `SCRIPT_SUBROUTINE` and (non-`<This Team>`) `TEAM` get the qualifier
    /// appended; a `SIDE` equal to `<template player name> + qualifier` becomes `new_player_name`.
    pub fn qualify(&mut self, qualifier: &str, player_template_name: &str, new_player_name: &str) {
        match self.kind {
            param::SCRIPT | param::COUNTER | param::FLAG | param::SCRIPT_SUBROUTINE => self.append(qualifier),
            param::TEAM => {
                if self.string() != Some(THIS_TEAM) {
                    self.append(qualifier);
                }
            }
            param::SIDE => {
                let qualified = format!("{}{qualifier}", self.string().unwrap_or(""));
                if qualified == player_template_name {
                    self.set_string(new_player_name);
                }
            }
            _ => {}
        }
    }

    fn append(&mut self, suffix: &str) {
        if let ParamValue::Plain { string, .. } = &mut self.value {
            string.push_str(suffix);
        }
    }

    fn set_string(&mut self, s: &str) {
        if let ParamValue::Plain { string, .. } = &mut self.value {
            *string = s.to_owned();
        }
    }
}

fn read_params(r: &mut Cursor) -> Result<Vec<Parameter>> {
    // Smallest parameter: type + int + real + empty string = 14 bytes.
    let n = count(r, 14)?;
    if n > MAX_PARMS {
        return Err(ChunkError::Corrupt("script parameter count exceeds MAX_PARMS"));
    }
    (0..n).map(|_| Parameter::read(r)).collect()
}

macro_rules! call_type {
    ($(#[$m:meta])* $name:ident, $table:ident, $min_key_version:expr, $what:literal) => {
        $(#[$m])*
        #[derive(Debug, Clone, PartialEq)]
        pub struct $name {
            pub version: u16,
            /// Raw type id as stored (index into the engine's enum, see [`Self::name`]).
            pub type_id: i32,
            /// The template's internal name stored by newer chunk versions. The engine trusts
            /// this over `type_id` and re-resolves the id from it when they disagree.
            pub internal_name: Option<String>,
            pub params: Vec<Parameter>,
        }

        impl $name {
            fn parse(c: &Chunk, names: &Names) -> Result<Self> {
                let mut r = c.cursor(names);
                let type_id = r.i32()?;
                let internal_name = if c.version >= $min_key_version { r.name_key()? } else { None };
                let params = read_params(&mut r)?;
                Ok(Self { version: c.version, type_id, internal_name, params })
            }

            /// Header enum name for `type_id` (position in the C++ enum), if in range.
            pub fn enum_name(&self) -> Option<&'static str> {
                usize::try_from(self.type_id).ok().and_then(|i| $table.get(i).copied())
            }

            /// Best name: the stored internal name, else the enum name.
            pub fn name(&self) -> Option<&str> {
                self.internal_name.as_deref().or_else(|| self.enum_name())
            }
        }
    };
}

call_type!(
    /// A `Condition` chunk. Type ids index `Condition::ConditionType` in `Scripts.h`.
    Condition, CONDITION_NAMES, 4, "condition"
);
call_type!(
    /// A `ScriptAction` / `ScriptActionFalse` chunk. Type ids index `ScriptAction::ScriptActionType`.
    Action, ACTION_NAMES, 2, "action"
);

/// Qualifies every parameter of a condition/action (`Condition::duplicateAndQualify` /
/// `ScriptAction::duplicateAndQualify`).
fn qualify_params(params: &mut [Parameter], qualifier: &str, player_template_name: &str, new_player_name: &str) {
    for p in params {
        p.qualify(qualifier, player_template_name, new_player_name);
    }
}

impl Condition {
    /// `Condition::duplicateAndQualify`.
    pub fn duplicate_and_qualify(&self, qualifier: &str, player_template_name: &str, new_player_name: &str) -> Self {
        let mut c = self.clone();
        qualify_params(&mut c.params, qualifier, player_template_name, new_player_name);
        c
    }
}

impl Action {
    /// `ScriptAction::duplicateAndQualify`.
    pub fn duplicate_and_qualify(&self, qualifier: &str, player_template_name: &str, new_player_name: &str) -> Self {
        let mut a = self.clone();
        qualify_params(&mut a.params, qualifier, player_template_name, new_player_name);
        a
    }
}

/// One `OrCondition` chunk: conditions that must all hold (logical AND).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OrCondition {
    pub conditions: Vec<Condition>,
}

impl OrCondition {
    /// `OrCondition::duplicateAndQualify`.
    pub fn duplicate_and_qualify(&self, qualifier: &str, player_template_name: &str, new_player_name: &str) -> Self {
        OrCondition {
            conditions: self
                .conditions
                .iter()
                .map(|c| c.duplicate_and_qualify(qualifier, player_template_name, new_player_name))
                .collect(),
        }
    }
}

/// A `Script` chunk.
#[derive(Debug, Clone, PartialEq)]
pub struct Script {
    pub version: u16,
    pub name: String,
    pub comment: String,
    pub condition_comment: String,
    pub action_comment: String,
    pub active: bool,
    pub one_shot: bool,
    pub easy: bool,
    pub normal: bool,
    pub hard: bool,
    pub subroutine: bool,
    /// Version >= 2, else 0.
    pub delay_seconds: i32,
    /// OR of ANDs: the script fires when any `OrCondition` has all its conditions true.
    pub or_conditions: Vec<OrCondition>,
    pub actions: Vec<Action>,
    pub actions_false: Vec<Action>,
}

impl Script {
    fn parse(c: &Chunk, names: &Names) -> Result<Self> {
        let mut r = c.cursor(names);
        let mut s = Self {
            version: c.version,
            name: r.ascii()?,
            comment: r.ascii()?,
            condition_comment: r.ascii()?,
            action_comment: r.ascii()?,
            active: r.bool()?,
            one_shot: r.bool()?,
            easy: r.bool()?,
            normal: r.bool()?,
            hard: r.bool()?,
            subroutine: r.bool()?,
            delay_seconds: if c.version >= 2 { r.i32()? } else { 0 },
            or_conditions: Vec::new(),
            actions: Vec::new(),
            actions_false: Vec::new(),
        };
        for child in r.chunks() {
            let child = child?;
            match child.name {
                "OrCondition" => {
                    let mut or = OrCondition::default();
                    for cond in child.children(names) {
                        let cond = cond?;
                        if cond.name == "Condition" {
                            or.conditions.push(Condition::parse(&cond, names)?);
                        }
                    }
                    s.or_conditions.push(or);
                }
                "ScriptAction" => s.actions.push(Action::parse(&child, names)?),
                "ScriptActionFalse" => s.actions_false.push(Action::parse(&child, names)?),
                _ => {}
            }
        }
        Ok(s)
    }

    /// `Script::duplicateAndQualify`: the script name and every condition/action parameter are
    /// qualified (script names themselves get the qualifier appended).
    pub fn duplicate_and_qualify(&self, qualifier: &str, player_template_name: &str, new_player_name: &str) -> Self {
        let mut s = self.clone();
        s.name.push_str(qualifier);
        for or in &mut s.or_conditions {
            *or = or.duplicate_and_qualify(qualifier, player_template_name, new_player_name);
        }
        for a in &mut s.actions {
            *a = a.duplicate_and_qualify(qualifier, player_template_name, new_player_name);
        }
        for a in &mut s.actions_false {
            *a = a.duplicate_and_qualify(qualifier, player_template_name, new_player_name);
        }
        s
    }
}

/// A `ScriptGroup` chunk: a named, switchable bundle of scripts.
#[derive(Debug, Clone, PartialEq)]
pub struct ScriptGroup {
    pub version: u16,
    pub name: String,
    pub active: bool,
    /// Only stored by version 2.
    pub subroutine: bool,
    pub scripts: Vec<Script>,
}

impl ScriptGroup {
    fn parse(c: &Chunk, names: &Names) -> Result<Self> {
        let mut r = c.cursor(names);
        let name = r.ascii()?;
        let active = r.bool()?;
        let subroutine = if c.version == 2 { r.bool()? } else { false };
        let mut scripts = Vec::new();
        for child in r.chunks() {
            let child = child?;
            if child.name == "Script" {
                scripts.push(Script::parse(&child, names)?);
            }
        }
        Ok(Self { version: c.version, name, active, subroutine, scripts })
    }

    /// `ScriptGroup::duplicateAndQualify`: the group name and its scripts are qualified.
    pub fn duplicate_and_qualify(&self, qualifier: &str, player_template_name: &str, new_player_name: &str) -> Self {
        let mut g = self.clone();
        g.name.push_str(qualifier);
        for s in &mut g.scripts {
            *s = s.duplicate_and_qualify(qualifier, player_template_name, new_player_name);
        }
        g
    }
}

/// One side's `ScriptList`: free scripts and groups.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ScriptList {
    pub scripts: Vec<Script>,
    pub groups: Vec<ScriptGroup>,
}

impl ScriptList {
    fn parse(c: &Chunk, names: &Names) -> Result<Self> {
        let mut l = Self::default();
        for child in c.children(names) {
            let child = child?;
            match child.name {
                "Script" => l.scripts.push(Script::parse(&child, names)?),
                "ScriptGroup" => l.groups.push(ScriptGroup::parse(&child, names)?),
                _ => {}
            }
        }
        Ok(l)
    }

    /// Every script in the list, group members included.
    pub fn all_scripts(&self) -> impl Iterator<Item = &Script> {
        self.scripts.iter().chain(self.groups.iter().flat_map(|g| g.scripts.iter()))
    }

    /// `ScriptList::duplicateAndQualify`: a deep per-player copy with qualified names.
    pub fn duplicate_and_qualify(&self, qualifier: &str, player_template_name: &str, new_player_name: &str) -> Self {
        ScriptList {
            scripts: self
                .scripts
                .iter()
                .map(|s| s.duplicate_and_qualify(qualifier, player_template_name, new_player_name))
                .collect(),
            groups: self
                .groups
                .iter()
                .map(|g| g.duplicate_and_qualify(qualifier, player_template_name, new_player_name))
                .collect(),
        }
    }
}

/// `Player::initFromDict`'s per-player team qualification: the team name and every script key get
/// the qualifier appended, and the owner is renamed to `new_player_name`.
pub fn qualify_team_dict(d: &Dict, qualifier: &str, new_player_name: &str) -> Dict {
    const SCRIPT_KEYS: [&str; 7] = [
        "teamOnCreateScript",
        "teamOnIdleScript",
        "teamOnUnitDestroyedScript",
        "teamOnDestroyedScript",
        "teamEnemySightedScript",
        "teamAllClearScript",
        "teamProductionCondition",
    ];
    let mut out = d.clone();
    if let Some(name) = d.str("teamName") {
        out.set_ascii("teamName", &format!("{name}{qualifier}"));
    }
    out.set_ascii("teamOwner", new_player_name);
    for key in SCRIPT_KEYS {
        if let Some(s) = d.str(key) {
            if !s.is_empty() {
                out.set_ascii(key, &format!("{s}{qualifier}"));
            }
        }
    }
    for i in 0..16 {
        let key = format!("teamGenericScriptHook{i}");
        if let Some(s) = d.str(&key) {
            if !s.is_empty() {
                out.set_ascii(&key, &format!("{s}{qualifier}"));
            }
        }
    }
    out
}

/// Decoded `PlayerScriptsList` chunk.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerScripts {
    pub version: u16,
    /// One list per side, in `SidesList` order.
    pub lists: Vec<ScriptList>,
}

impl PlayerScripts {
    pub(crate) fn parse(c: &Chunk, names: &Names) -> Result<Self> {
        let mut lists = Vec::new();
        for child in c.children(names) {
            let child = child?;
            if child.name == "ScriptList" {
                lists.push(ScriptList::parse(&child, names)?);
            }
        }
        Ok(Self { version: c.version, lists })
    }

    /// Totals over all sides.
    pub fn stats(&self) -> ScriptStats {
        let mut s = ScriptStats { lists: self.lists.len(), ..Default::default() };
        for l in &self.lists {
            s.groups += l.groups.len();
            for sc in l.all_scripts() {
                s.scripts += 1;
                s.or_conditions += sc.or_conditions.len();
                s.conditions += sc.or_conditions.iter().map(|o| o.conditions.len()).sum::<usize>();
                s.actions += sc.actions.len();
                s.actions_false += sc.actions_false.len();
            }
        }
        s
    }
}

/// One `ScriptsPlayers` entry: a side's player name plus its optional dict (v2).
#[derive(Debug, Clone, PartialEq)]
pub struct ScriptPlayer {
    pub name: String,
    pub dict: Option<Dict>,
}

/// A decoded `.scb` scripts file (`Data\Scripts\SkirmishScripts.scb` / `MultiplayerScripts.scb`):
/// the same `CkMp` container as a map, with `PlayerScriptsList`, `ScriptsPlayers` and `ScriptTeams`
/// top-level chunks. Ported from `SidesList.cpp` `prepareForMP_or_Skirmish`,
/// `ParsePlayersDataChunk` and `ParseTeamsDataChunk`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ScriptsFile {
    pub scripts: Option<PlayerScripts>,
    /// `ScriptsPlayers`, in order (matched to skirmish sides by name).
    pub players: Vec<ScriptPlayer>,
    /// `ScriptTeams`, the skirmish team definitions.
    pub teams: Vec<Dict>,
}

impl ScriptsFile {
    /// Every script in the file, group members included.
    pub fn all_scripts(&self) -> impl Iterator<Item = &Script> {
        self.scripts.iter().flat_map(|p| p.lists.iter()).flat_map(|l| l.all_scripts())
    }
}

/// Parses a standalone `.scb` scripts file. Never panics; malformed data yields `Err`.
pub fn parse_scripts_file(data: &[u8]) -> Result<ScriptsFile> {
    let (names, body) = open(data)?;
    let mut f = ScriptsFile::default();
    for c in ChunkIter::new(body, &names) {
        let c = c?;
        match c.name {
            "PlayerScriptsList" => f.scripts = Some(PlayerScripts::parse(&c, &names)?),
            "ScriptsPlayers" => {
                let mut r = c.cursor(&names);
                let read_dicts = c.version >= 2 && r.i32()? != 0;
                for _ in 0..count(&mut r, 2)? {
                    let name = r.ascii()?;
                    let dict = if read_dicts { Some(r.dict()?) } else { None };
                    f.players.push(ScriptPlayer { name, dict });
                }
            }
            "ScriptTeams" => {
                let mut r = c.cursor(&names);
                while !r.at_end() {
                    f.teams.push(r.dict()?);
                }
            }
            _ => {}
        }
    }
    Ok(f)
}

/// Counts for reporting.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScriptStats {
    pub lists: usize,
    pub groups: usize,
    pub scripts: usize,
    pub or_conditions: usize,
    pub conditions: usize,
    pub actions: usize,
    pub actions_false: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::test_util::*;
    use crate::map::MapFile;

    // ids: 1 PlayerScriptsList, 2 ScriptList, 3 ScriptGroup, 4 Script, 5 OrCondition,
    // 6 Condition, 7 ScriptAction, 8 ScriptActionFalse, 9 SidesList, 10 TRUE (name key), 11 Foo
    const NAMES: &[(&str, u32)] = &[
        ("PlayerScriptsList", 1),
        ("ScriptList", 2),
        ("ScriptGroup", 3),
        ("Script", 4),
        ("OrCondition", 5),
        ("Condition", 6),
        ("ScriptAction", 7),
        ("ScriptActionFalse", 8),
        ("SidesList", 9),
        ("CONDITION_TRUE", 10),
        ("Foo", 11),
    ];

    fn plain(kind: i32, int: i32, real: f32, s: &str) -> Vec<u8> {
        let mut d = i32s(&[kind, int]);
        d.extend(real.to_le_bytes());
        d.extend(ascii(s));
        d
    }
    fn coord(x: f32, y: f32, z: f32) -> Vec<u8> {
        let mut d = i32s(&[param::COORD3D]);
        d.extend([x, y, z].iter().flat_map(|f| f.to_le_bytes()));
        d
    }
    fn key(id: u32) -> Vec<u8> {
        (id << 8 | 3).to_le_bytes().to_vec()
    }
    fn condition() -> Vec<u8> {
        let mut d = i32s(&[3]);
        d.extend(key(10));
        d.extend(i32s(&[1]));
        d.extend(plain(2, 0, 0.0, "MyScript"));
        chunk(6, 4, &d)
    }
    fn action(id: u32, type_id: i32, ver: u16) -> Vec<u8> {
        let mut d = i32s(&[type_id]);
        if ver >= 2 {
            d.extend(key(10));
        }
        d.extend(i32s(&[2]));
        d.extend(coord(1.0, 2.0, 3.0));
        d.extend(plain(0, 7, 0.5, ""));
        chunk(id, ver, &d)
    }
    fn script(ver: u16) -> Vec<u8> {
        let mut d = ascii("s");
        d.extend(ascii("c"));
        d.extend(ascii("cc"));
        d.extend(ascii("ac"));
        d.extend([1, 0, 1, 1, 0, 1]);
        if ver >= 2 {
            d.extend(i32s(&[9]));
        }
        d.extend(chunk(5, 1, &condition()));
        d.extend(chunk(5, 1, &[condition(), condition()].concat()));
        d.extend(action(7, 1, 2));
        d.extend(chunk(11, 1, &[1]));
        d.extend(action(8, 4, 1));
        chunk(4, ver, &d)
    }
    fn group(ver: u16) -> Vec<u8> {
        let mut d = ascii("g");
        d.push(1);
        if ver == 2 {
            d.push(1);
        }
        d.extend(script(2));
        chunk(3, ver, &d)
    }
    fn sides_with_scripts() -> Vec<u8> {
        let mut list = script(1);
        list.extend(group(2));
        let mut psl = chunk(2, 1, &list);
        psl.extend(chunk(2, 1, &[]));
        // SidesList v1: 0 sides, then the nested scripts chunk
        let mut s = i32s(&[0]);
        s.extend(chunk(1, 1, &psl));
        file(NAMES, &chunk(9, 1, &s))
    }

    #[test]
    fn decodes_tree() {
        let m = MapFile::parse(&sides_with_scripts()).unwrap();
        assert!(m.decode_errors.is_empty(), "{:?}", m.decode_errors);
        let ps = m.scripts().unwrap();
        assert_eq!(ps.lists.len(), 2);
        let l = &ps.lists[0];
        assert_eq!((l.scripts.len(), l.groups.len(), ps.lists[1].all_scripts().count()), (1, 1, 0));
        let s = &l.scripts[0];
        assert_eq!((s.name.as_str(), s.condition_comment.as_str(), s.action_comment.as_str()), ("s", "cc", "ac"));
        assert_eq!((s.active, s.one_shot, s.easy, s.normal, s.hard, s.subroutine, s.delay_seconds), (true, false, true, true, false, true, 0));
        assert_eq!(s.or_conditions.len(), 2);
        assert_eq!(s.or_conditions[1].conditions.len(), 2);
        let c = &s.or_conditions[0].conditions[0];
        assert_eq!((c.type_id, c.name(), c.enum_name()), (3, Some("CONDITION_TRUE"), Some("CONDITION_TRUE")));
        assert_eq!(c.params[0].kind_name(), Some("SCRIPT"));
        assert_eq!(c.params[0].string(), Some("MyScript"));
        let a = &s.actions[0];
        assert_eq!(a.enum_name(), Some("SET_FLAG"));
        assert_eq!(a.params[0].coord(), Some([1.0, 2.0, 3.0]));
        assert_eq!((a.params[1].int(), a.params[1].real()), (Some(7), Some(0.5)));
        // ScriptActionFalse v1 has no name key; falls back to the enum table
        let f = &s.actions_false[0];
        assert_eq!((f.internal_name.clone(), f.name()), (None, Some("DEFEAT")));
        let g = &l.groups[0];
        assert_eq!((g.name.as_str(), g.active, g.subroutine, g.scripts.len(), g.scripts[0].delay_seconds), ("g", true, true, 1, 9));
        let st = ps.stats();
        assert_eq!((st.lists, st.groups, st.scripts, st.or_conditions, st.conditions, st.actions, st.actions_false), (2, 1, 2, 4, 6, 2, 2));
    }

    #[test]
    fn hostile_input() {
        let full = sides_with_scripts();
        for n in 0..full.len() {
            let _ = MapFile::parse(&full[..n]);
        }
        // parameter count larger than MAX_PARMS / than the chunk
        for bad in [13, i32::MAX, -1] {
            let mut d = i32s(&[3]);
            d.extend(key(10));
            d.extend(i32s(&[bad]));
            d.extend([0u8; 400]);
            let or = chunk(5, 1, &chunk(6, 4, &d));
            let sc = chunk(4, 1, &[ascii("s"), ascii(""), ascii(""), ascii(""), vec![0; 6], or].concat());
            let psl = chunk(1, 1, &chunk(2, 1, &sc));
            let m = MapFile::parse(&file(NAMES, &chunk(9, 1, &[i32s(&[0]), psl].concat()))).unwrap();
            assert!(m.scripts().is_none());
            assert_eq!(m.decode_errors.len(), 1, "{bad}");
        }
    }
}
