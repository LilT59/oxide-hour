//! Block tables: which tokens open top-level blocks and which keys open
//! nested ones.
//!
//! # Decision: explicit opener table
//!
//! Syntax alone cannot say whether `Key = Value` is a field or opens a block
//! that needs its own `End`: the C++ field parse procedure decides (e.g.
//! `Draw = W3DModelDraw Tag` calls `initFromINI` recursively, `Armor = Foo`
//! inside `ActiveBody` does not). Indentation is unreliable too. So
//! [`opens_block`] encodes the openers explicitly, keyed on the *enclosing*
//! block's kind (and for module blocks, its module type), because the same
//! key is a block in one place and a plain field in another (`Turret`,
//! `Armor`, `Weapon`, `Locomotor`, `Animation`, ...).
//!
//! The table was built from the C++ field tables (parse procs that recurse
//! into `INI::initFromINI`) and then verified against all shipped INI files:
//! a wrong entry shows up immediately as an unknown top-level token or a
//! missing `End`, so `examples/parse_all.rs` doubles as the regression check.

/// The top-level block types: `theTypeTable` in
/// `Core/GameEngine/Source/Common/INI/INI.cpp`. Matching is case-sensitive
/// (`findBlockParse` uses `strcmp`).
pub const BLOCK_TYPES: &[&str] = &[
    "AIData",
    "Animation",
    "Armor",
    "AudioEvent",
    "AudioSettings",
    "BenchProfile",
    "Bridge",
    "Campaign",
    "ChallengeGenerals",
    "CommandButton",
    "CommandMap",
    "CommandSet",
    "ControlBarResizer",
    "ControlBarScheme",
    "CrateData",
    "Credits",
    "DamageFX",
    "DialogEvent",
    "DrawGroupInfo",
    "DynamicGameLOD",
    "EvaEvent",
    "FXList",
    "GameData",
    "HeaderTemplate",
    "InGameUI",
    "LODPreset",
    "Language",
    "Locomotor",
    "MapCache",
    "MapData",
    "MappedImage",
    "MiscAudio",
    "Mouse",
    "MouseCursor",
    "MultiplayerColor",
    "MultiplayerSettings",
    "MultiplayerStartingMoneyChoice",
    "MusicTrack",
    "Object",
    "ObjectCreationList",
    "ObjectReskin",
    "OnlineChatColors",
    "ParticleSystem",
    "PlayerTemplate",
    "Rank",
    "ReallyLowMHz",
    "Road",
    "Science",
    "ScriptAction",
    "ScriptCondition",
    "ShellMenuScheme",
    "SpecialPower",
    "StaticGameLOD",
    "Terrain",
    "Upgrade",
    "Video",
    "WaterSet",
    "WaterTransparency",
    "Weapon",
    "Weather",
    "WebpageURL",
    "WindowTransition",
];

pub fn is_block_type(token: &str) -> bool {
    BLOCK_TYPES.contains(&token)
}

/// Top-level types whose parse procs read their operands from the opening line
/// and never call `initFromINI`, so they have no body and no `End`
/// (`INI::parseBenchProfile`, `INI::parseLODPreset`, `parseReallyLowMHz` in
/// `GameLOD.cpp`).
pub fn is_line_block(kind: &str) -> bool {
    matches!(kind, "BenchProfile" | "LODPreset" | "ReallyLowMHz")
}

/// Whether `key` inside a block of kind `parent` (with header tokens
/// `header`) opens a nested block terminated by its own `End`.
pub fn opens_block(parent: &str, header: &[String], key: &str) -> bool {
    let _ = header;
    match parent {
        // ThingTemplate field table (ThingTemplate.cpp): module blocks, the
        // per-unit sound/FX tables, and the module-editing wrappers whose
        // bodies reuse the Object field table.
        "Object"
        | "ObjectReskin"
        | "AddModule"
        | "ReplaceModule"
        | "InheritableModule"
        | "OverrideableByLikeKind" => matches!(
            key,
            "Behavior"
                | "Body"
                | "Draw"
                | "ClientUpdate"
                | "ArmorSet"
                | "WeaponSet"
                | "Prerequisites"
                | "UnitSpecificSounds"
                | "UnitSpecificFX"
                | "AddModule"
                | "ReplaceModule"
                | "InheritableModule"
                | "OverrideableByLikeKind"
        ),
        // W3DModelDrawModuleData::parseConditionState. `AliasConditionState`
        // is a one-line reference to the previous state and has no `End`.
        "Draw" => matches!(
            key,
            "ConditionState" | "DefaultConditionState" | "TransitionState"
        ),
        // AIUpdateModuleData / TurretAIData and the RadiusDecalTemplate users.
        "Behavior" => matches!(
            key,
            "Turret"
                | "AltTurret"
                | "AttackAreaDecal"
                | "TargetingReticleDecal"
                | "GridDecalTemplate"
                | "DeliveryDecal"
        ),
        // AI.cpp: parseSideInfo, parseSkillSet, parseSkirmishBuildList, parseStructure.
        "AIData" => matches!(key, "SideInfo" | "SkirmishBuildList"),
        "SideInfo" => key.starts_with("SkillSet"),
        "SkirmishBuildList" => key == "Structure",
        "Campaign" => key == "Mission",
        "ChallengeGenerals" => key.starts_with("GeneralPersona"),
        "EvaEvent" => key == "SideSounds",
        // FXList.cpp: one nugget parser per effect kind.
        "FXList" => matches!(
            key,
            "ParticleSystem"
                | "Sound"
                | "ViewShake"
                | "LightPulse"
                | "Tracer"
                | "TerrainScorch"
                | "RayEffect"
                | "FXListAtBonePos"
        ),
        "ObjectCreationList" => matches!(
            key,
            "CreateObject"
                | "CreateDebris"
                | "DeliverPayload"
                | "ApplyRandomForce"
                | "Attack"
                | "FireWeapon"
        ),
        "DeliverPayload" | "Attack" => key == "DeliveryDecal",
        "InGameUI" => key.ends_with("RadiusCursor"),
        "WindowTransition" => key == "Window",
        "ControlBarScheme" => matches!(key, "ImagePart" | "AnimatingPart"),
        "AnimatingPart" => key == "ImagePart",
        "ShellMenuScheme" => matches!(key, "ImagePart" | "LinePart"),
        _ => false,
    }
}
