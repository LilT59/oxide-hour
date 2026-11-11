//! Skirmish AI data (`GameLogic/AI/AI.cpp`: `TheAIFieldParseTable`, `AI::parseSideInfo`,
//! `AI::parseSkirmishBuildList`; `GameLogic/Map/SidesList.cpp`: `BuildListInfo::parseStructure`).
//!
//! `AIData` holds the global AI tunables plus per-side `SideInfo` (resource gatherer counts, base
//! defense structure, skill sets) and per-faction `SkirmishBuildList`s (the structures the AI
//! builds, in order). Values are copied as written; the sim converts the frame-based ones.

use zr_ini::{Block, Item};

use crate::diag::{Diag, WarningKind};
use crate::value::{parse_coord2d, Args};

/// One `Structure <template>` entry of a `SkirmishBuildList` (`BuildListInfo`).
#[derive(Debug, Clone, PartialEq)]
pub struct BuildListEntry {
    /// The `Structure` header token: the `ThingTemplate` name.
    pub template_name: String,
    /// `Name`.
    pub name: String,
    /// `Location` (`X: Y:`).
    pub location: (f32, f32),
    /// `Rebuilds` (default 0).
    pub rebuilds: i32,
    /// `Angle`, radians.
    pub angle: f32,
    /// `InitiallyBuilt`.
    pub initially_built: bool,
    /// `RallyPointOffset`.
    pub rally_point_offset: (f32, f32),
    /// `AutomaticallyBuild` (default true).
    pub automatically_build: bool,
}

/// One `SkirmishBuildList <faction>` block.
#[derive(Debug, Clone, PartialEq)]
pub struct AiSideBuildList {
    pub faction: String,
    pub structures: Vec<BuildListEntry>,
}

/// One `SideInfo <side>` block.
#[derive(Debug, Clone, PartialEq)]
pub struct AiSideInfo {
    pub side: String,
    pub resource_gatherers_easy: i32,
    pub resource_gatherers_normal: i32,
    pub resource_gatherers_hard: i32,
    /// `BaseDefenseStructure1`.
    pub base_defense_structure1: String,
    /// `SkillSet1..5`, each a list of `Science` names.
    pub skill_sets: Vec<Vec<String>>,
}

/// The parsed `AIData` block.
#[derive(Debug, Clone, PartialEq)]
pub struct AiData {
    /// `StructureSeconds` (seconds between structure builds).
    pub structure_seconds: f32,
    /// `TeamSeconds`.
    pub team_seconds: f32,
    /// `Wealthy` / `Poor` money thresholds.
    pub resources_wealthy: i32,
    pub resources_poor: i32,
    /// `ForceIdleMSEC`, frames.
    pub force_idle_frames: u32,
    pub structures_wealthy_mod: f32,
    pub team_wealthy_mod: f32,
    pub structures_poor_mod: f32,
    pub team_poor_mod: f32,
    /// `TeamResourcesToStart`.
    pub team_resources_to_build: f32,
    /// `RebuildDelayTimeSeconds`.
    pub rebuild_delay_seconds: i32,
    /// `RotateSkirmishBases`: `adjustBuildList` adds the 3x3-grid rotation only when set
    /// (`AISkirmishPlayer.cpp:1000`); the default is false (`AI.cpp:940`).
    pub rotate_skirmish_bases: bool,
    pub side_infos: Vec<AiSideInfo>,
    pub build_lists: Vec<AiSideBuildList>,
    /// The remaining AI tunables (`GuardEnemyScanRate`, `MaxRecruitRadius`, ...), as written.
    pub extra: std::collections::BTreeMap<String, String>,
}

impl Default for AiData {
    fn default() -> Self {
        AiData {
            structure_seconds: 10.0,
            team_seconds: 20.0,
            resources_wealthy: 2000,
            resources_poor: 500,
            force_idle_frames: 0,
            structures_wealthy_mod: 1.0,
            team_wealthy_mod: 1.0,
            structures_poor_mod: 1.0,
            team_poor_mod: 1.0,
            team_resources_to_build: 0.0,
            rebuild_delay_seconds: 10,
            rotate_skirmish_bases: false,
            side_infos: Vec::new(),
            build_lists: Vec::new(),
            extra: std::collections::BTreeMap::new(),
        }
    }
}

impl AiData {
    /// Parses one `AIData` block.
    pub(crate) fn from_block(b: &Block, d: &mut Diag) -> AiData {
        let mut ai = AiData::default();
        d.push_scope("AIData");
        for item in &b.items {
            match item {
                Item::Field(f) => {
                    let mut a = Args::new(&f.value);
                    let r: Result<bool, String> = (|| {
                        match f.key.as_str() {
                            "StructureSeconds" => ai.structure_seconds = a.real()?,
                            "TeamSeconds" => ai.team_seconds = a.real()?,
                            "Wealthy" => ai.resources_wealthy = a.int()?,
                            "Poor" => ai.resources_poor = a.int()?,
                            "ForceIdleMSEC" => ai.force_idle_frames = a.duration_frames()?,
                            "StructuresWealthyRate" => ai.structures_wealthy_mod = a.real()?,
                            "TeamsWealthyRate" => ai.team_wealthy_mod = a.real()?,
                            "StructuresPoorRate" => ai.structures_poor_mod = a.real()?,
                            "TeamsPoorRate" => ai.team_poor_mod = a.real()?,
                            "TeamResourcesToStart" => ai.team_resources_to_build = a.real()?,
                            "RebuildDelayTimeSeconds" => ai.rebuild_delay_seconds = a.int()?,
                            "RotateSkirmishBases" => ai.rotate_skirmish_bases = a.boolean()?,
                            // The remaining tunables are kept as written.
                            _ => {
                                ai.extra.insert(f.key.clone(), f.value.clone());
                                return Ok(true);
                            }
                        }
                        Ok(true)
                    })();
                    match r {
                        Ok(true) => {}
                        Ok(false) => d.unknown_field(f),
                        Err(e) => d.bad_value(f, e),
                    }
                }
                Item::Block(nb) if nb.kind == "SideInfo" => ai.side_infos.push(parse_side_info(nb, d)),
                Item::Block(nb) if nb.kind == "SkirmishBuildList" => ai.build_lists.push(parse_build_list(nb, d)),
                Item::Block(nb) => d.warn(WarningKind::Structure, &nb.kind, nb.line, "unexpected AIData block"),
            }
        }
        d.pop_scope();
        ai
    }
}

fn parse_side_info(b: &Block, d: &mut Diag) -> AiSideInfo {
    let side = b.header.first().cloned().unwrap_or_default();
    let mut s = AiSideInfo {
        side: side.clone(),
        resource_gatherers_easy: 0,
        resource_gatherers_normal: 0,
        resource_gatherers_hard: 0,
        base_defense_structure1: String::new(),
        skill_sets: Vec::new(),
    };
    d.push_scope(format!("SideInfo {side}"));
    for item in &b.items {
        match item {
            Item::Block(sk) if sk.kind.starts_with("SkillSet") => {
                let mut sciences = Vec::new();
                for it in &sk.items {
                    if let Item::Field(f) = it {
                        if f.key == "Science" {
                            let n = Args::new(&f.value).string();
                            if !n.is_empty() {
                                sciences.push(n);
                            }
                        }
                    }
                }
                s.skill_sets.push(sciences);
            }
            Item::Field(f) => {
                let mut a = Args::new(&f.value);
                let r: Result<bool, String> = (|| {
                    match f.key.as_str() {
                        "ResourceGatherersEasy" => s.resource_gatherers_easy = a.int()?,
                        "ResourceGatherersNormal" => s.resource_gatherers_normal = a.int()?,
                        "ResourceGatherersHard" => s.resource_gatherers_hard = a.int()?,
                        "BaseDefenseStructure1" => s.base_defense_structure1 = a.string(),
                        _ => return Ok(false),
                    }
                    Ok(true)
                })();
                match r {
                    Ok(true) => {}
                    Ok(false) => d.unknown_field(f),
                    Err(e) => d.bad_value(f, e),
                }
            }
            Item::Block(_) => {}
        }
    }
    d.pop_scope();
    s
}

fn parse_build_list(b: &Block, d: &mut Diag) -> AiSideBuildList {
    let faction = b.header.first().cloned().unwrap_or_default();
    let mut out = AiSideBuildList { faction: faction.clone(), structures: Vec::new() };
    d.push_scope(format!("SkirmishBuildList {faction}"));
    for item in &b.items {
        if let Item::Block(sb) = item {
            if sb.kind == "Structure" {
                out.structures.push(parse_structure(sb, d));
            } else {
                d.warn(WarningKind::Structure, &sb.kind, sb.line, "unexpected SkirmishBuildList block");
            }
        }
    }
    d.pop_scope();
    out
}

fn parse_structure(b: &Block, d: &mut Diag) -> BuildListEntry {
    let template_name = b.header.first().cloned().unwrap_or_default();
    let mut e = BuildListEntry {
        template_name: template_name.clone(),
        name: String::new(),
        location: (0.0, 0.0),
        rebuilds: 0,
        angle: 0.0,
        initially_built: false,
        rally_point_offset: (0.0, 0.0),
        automatically_build: true,
    };
    d.push_scope(format!("Structure {template_name}"));
    for item in &b.items {
        let Item::Field(f) = item else { continue };
        let mut a = Args::new(&f.value);
        let r: Result<bool, String> = (|| {
            match f.key.as_str() {
                "Name" => e.name = a.string(),
                "Location" => e.location = parse_coord2d(&f.value)?,
                "Rebuilds" => e.rebuilds = a.int()?,
                "Angle" => e.angle = a.angle()?,
                "InitiallyBuilt" => e.initially_built = a.boolean()?,
                "RallyPointOffset" => e.rally_point_offset = parse_coord2d(&f.value)?,
                "AutomaticallyBuild" => e.automatically_build = a.boolean()?,
                _ => return Ok(false),
            }
            Ok(true)
        })();
        match r {
            Ok(true) => {}
            Ok(false) => d.unknown_field(f),
            Err(e2) => d.bad_value(f, e2),
        }
    }
    d.pop_scope();
    e
}
