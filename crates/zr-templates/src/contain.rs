//! Containment module data: `OpenContain`, `TransportContain`, `GarrisonContain`, `OverlordContain`,
//! `HelixContain`, `TunnelContain`, `MobNexusContain`, `CaveContain` and the rest of
//! `GameLogic/Object/Contain/`.
//!
//! Ported from `GameLogic/Object/Contain/*.cpp` `buildFieldParse` tables and constructor defaults.
//! Durations are frames, percents fractions, angular velocities radians per frame.

use zr_ini::{Block, Item};

use crate::diag::{Diag, WarningKind};
use crate::flags::{KindOfMask, Mask, NameEnum};
use crate::names::{LocomotorSetType, ObjectStatus, WeaponSetType};
use crate::KindOf;
use crate::value::Args;

/// Which contain module class this is; decides capacity source and a few rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainKind {
    Open,
    Transport,
    Garrison,
    Overlord,
    Helix,
    Tunnel,
    Heal,
    /// `ParachuteContain` (an `OpenContain` subclass).
    Parachute,
    /// `InternetHackContain` (a `TransportContain` subclass).
    InternetHack,
    /// `RailedTransportContain` (uses `TransportContainModuleData`).
    RailedTransport,
    /// `RiderChangeContain` (a `TransportContain` subclass).
    RiderChange,
    /// `MobNexusContain` (an `OpenContain` subclass with its own `Slots` capacity).
    MobNexus,
    /// `CaveContain` (an `OpenContain` subclass; the cave system shares one list per `CaveIndex`).
    Cave,
}

impl ContainKind {
    pub fn from_class_name(n: &str) -> Option<Self> {
        Some(match n {
            "OpenContain" => ContainKind::Open,
            "TransportContain" => ContainKind::Transport,
            "GarrisonContain" => ContainKind::Garrison,
            "OverlordContain" => ContainKind::Overlord,
            "HelixContain" => ContainKind::Helix,
            "TunnelContain" => ContainKind::Tunnel,
            "HealContain" => ContainKind::Heal,
            "ParachuteContain" => ContainKind::Parachute,
            "InternetHackContain" => ContainKind::InternetHack,
            "RailedTransportContain" => ContainKind::RailedTransport,
            "RiderChangeContain" => ContainKind::RiderChange,
            "MobNexusContain" => ContainKind::MobNexus,
            "CaveContain" => ContainKind::Cave,
            _ => return None,
        })
    }
    /// Transport-family containers (own units only, slot capacity).
    pub fn is_transport(self) -> bool {
        matches!(
            self,
            ContainKind::Transport
                | ContainKind::Overlord
                | ContainKind::Helix
                | ContainKind::InternetHack
                | ContainKind::RailedTransport
                | ContainKind::RiderChange
        )
    }
}

/// One `RiderChangeContain` `RiderN` line (`RiderChangeContainModuleData::parseRiderInfo`,
/// `RiderChangeContain.cpp:69`): template, model condition, weapon set flag, object status,
/// command set, locomotor set. The model condition and command set stay names (`NONE` = none).
#[derive(Debug, Clone, PartialEq)]
pub struct RiderInfo {
    pub template_name: String,
    /// `ModelConditionFlagType` name; `NONE` when unset.
    pub model_condition: String,
    /// `WeaponSetFlags`: one `WEAPON_RIDERn` bit (empty for `NONE`).
    pub weapon_set: Mask<WeaponSetType>,
    /// `ObjectStatusMaskType` (empty for `NONE`).
    pub object_status: Mask<ObjectStatus>,
    /// `CommandSet` override name; `NONE` when unset.
    pub command_set: String,
    pub locomotor_set: LocomotorSetType,
}

impl Default for RiderInfo {
    fn default() -> Self {
        RiderInfo {
            template_name: String::new(),
            model_condition: "NONE".to_string(),
            weapon_set: Mask::EMPTY,
            object_status: Mask::EMPTY,
            command_set: "NONE".to_string(),
            locomotor_set: LocomotorSetType::SetNormal,
        }
    }
}

/// Union of the contain module data fields. Fields not used by a given kind keep their default.
#[derive(Debug, Clone, PartialEq)]
pub struct ContainData {
    pub kind: ContainKind,
    // OpenContain
    /// `ContainMax` (-1 = unlimited).
    pub contain_max: i32,
    pub enter_sound: Option<String>,
    pub exit_sound: Option<String>,
    pub passengers_allowed_to_fire: bool,
    pub passengers_in_turret: bool,
    pub number_of_exit_paths: i32,
    pub damage_percentage_to_units: f32,
    pub is_burned_death_to_units: bool,
    pub door_open_time: u32,
    pub allow_inside_kind_of: KindOfMask,
    pub forbid_inside_kind_of: KindOfMask,
    pub weapon_bonus_passed_to_passengers: bool,
    pub allow_allies_inside: bool,
    pub allow_enemies_inside: bool,
    pub allow_neutral_inside: bool,
    // TransportContain
    pub slot_capacity: i32,
    pub exit_pitch_rate: f32,
    pub exit_bone: String,
    pub initial_payload: Option<(String, i32)>,
    pub health_regen: f32,
    pub exit_delay: u32,
    pub scatter_nearby_on_exit: bool,
    pub orient_like_container_on_exit: bool,
    pub keep_container_velocity_on_exit: bool,
    pub go_aggressive_on_exit: bool,
    pub armed_riders_upgrade_weapon_set: bool,
    pub reset_mood_check_time_on_exit: bool,
    pub destroy_riders_who_are_not_free_to_exit: bool,
    pub is_delay_exit_in_air: bool,
    // GarrisonContain
    pub do_i_heal_objects: bool,
    /// `GarrisonContainModuleData::m_framesForFullHeal` (`TimeForFullHeal`, `parseDurationReal`,
    /// GarrisonContain.h:53,70), in frames.
    pub frames_for_full_heal: f32,
    pub mobile_garrison: bool,
    pub immune_to_clear_building_attacks: bool,
    pub is_enclosing_container: bool,
    pub initial_roster: Option<(String, i32)>,
    // TunnelContain
    /// `TunnelContainModuleData::m_framesForFullHeal` (`TimeForFullHeal`, `parseDurationReal`,
    /// TunnelContain.h:46,68), in frames.
    pub tunnel_frames_for_full_heal: f32,
    // HealContain
    /// `HealContainModuleData::m_framesForFullHeal` (`TimeForFullHeal`,
    /// `parseDurationUnsignedInt`, HealContain.h:46, HealContain.cpp:62), in whole frames.
    pub heal_frames: u32,
    // CaveContain
    /// `CaveContainModuleData::m_caveIndexData` (`CaveIndex`, CaveContain.h:49,61): caves with the
    /// same index share one `TunnelTracker`.
    pub cave_index: i32,
    // Overlord / Helix
    pub payload_template_names: Vec<String>,
    pub experience_sink_for_rider: bool,
    pub should_draw_pips: bool,
    // ParachuteContain
    pub pitch_rate_max: f32,
    pub roll_rate_max: f32,
    pub low_altitude_damping: f32,
    pub para_open_dist: f32,
    pub kill_when_landing_in_water_slop: f32,
    pub free_fall_damage_percent: f32,
    pub parachute_open_sound: Option<String>,
    // RiderChangeContain
    pub riders: Vec<RiderInfo>,
    pub scuttle_frames: u32,
    pub scuttle_state: String,
}

fn all_kinds() -> KindOfMask {
    (0..KindOf::count()).fold(Mask::EMPTY, |m, i| m.with(KindOf::from_index(i)))
}

impl Default for ContainData {
    fn default() -> Self {
        ContainData {
            kind: ContainKind::Open,
            contain_max: -1,
            enter_sound: None,
            exit_sound: None,
            passengers_allowed_to_fire: false,
            passengers_in_turret: false,
            number_of_exit_paths: 1,
            damage_percentage_to_units: 0.0,
            is_burned_death_to_units: true,
            door_open_time: 1,
            allow_inside_kind_of: all_kinds(),
            forbid_inside_kind_of: Mask::EMPTY,
            weapon_bonus_passed_to_passengers: false,
            allow_allies_inside: true,
            allow_enemies_inside: true,
            allow_neutral_inside: true,
            slot_capacity: 0,
            exit_pitch_rate: 0.0,
            exit_bone: String::new(),
            initial_payload: None,
            health_regen: 0.0,
            exit_delay: 0,
            scatter_nearby_on_exit: true,
            orient_like_container_on_exit: false,
            keep_container_velocity_on_exit: false,
            go_aggressive_on_exit: false,
            armed_riders_upgrade_weapon_set: false,
            reset_mood_check_time_on_exit: true,
            destroy_riders_who_are_not_free_to_exit: false,
            is_delay_exit_in_air: false,
            do_i_heal_objects: false,
            frames_for_full_heal: 0.0,
            mobile_garrison: false,
            immune_to_clear_building_attacks: false,
            is_enclosing_container: false,
            initial_roster: None,
            tunnel_frames_for_full_heal: 1.0,
            heal_frames: 0,
            cave_index: 0,
            payload_template_names: Vec::new(),
            experience_sink_for_rider: true,
            should_draw_pips: true,
            pitch_rate_max: 0.0,
            roll_rate_max: 0.0,
            low_altitude_damping: 0.2,
            para_open_dist: 0.0,
            kill_when_landing_in_water_slop: 10.0,
            free_fall_damage_percent: 0.5,
            parachute_open_sound: None,
            riders: Vec::new(),
            scuttle_frames: 0,
            // `RiderChangeContainModuleData` constructor: `MODELCONDITION_TOPPLED`.
            scuttle_state: "TOPPLED".to_string(),
        }
    }
}

impl ContainData {
    /// `OpenContainModuleData::buildFieldParse` + the subclass tables.
    fn field(&mut self, key: &str, a: &mut Args) -> Result<bool, String> {
        match key {
            // OpenContain
            "ContainMax" => self.contain_max = a.int()?,
            "EnterSound" => self.enter_sound = sound_or_none(a.string()),
            "ExitSound" => self.exit_sound = sound_or_none(a.string()),
            "DamagePercentToUnits" => self.damage_percentage_to_units = a.percent()?,
            "BurnedDeathToUnits" => self.is_burned_death_to_units = a.boolean()?,
            "AllowInsideKindOf" => self.allow_inside_kind_of = self.allow_inside_kind_of.parse(a.rest())?,
            "ForbidInsideKindOf" => self.forbid_inside_kind_of = self.forbid_inside_kind_of.parse(a.rest())?,
            "PassengersAllowedToFire" => self.passengers_allowed_to_fire = a.boolean()?,
            "PassengersInTurret" => self.passengers_in_turret = a.boolean()?,
            "NumberOfExitPaths" => self.number_of_exit_paths = a.int()?,
            "DoorOpenTime" => self.door_open_time = a.duration_frames()?,
            "WeaponBonusPassedToPassengers" => self.weapon_bonus_passed_to_passengers = a.boolean()?,
            "AllowAlliesInside" => self.allow_allies_inside = a.boolean()?,
            "AllowEnemiesInside" => self.allow_enemies_inside = a.boolean()?,
            "AllowNeutralInside" => self.allow_neutral_inside = a.boolean()?,
            // TransportContain
            "Slots" => self.slot_capacity = a.int()?,
            "ScatterNearbyOnExit" => self.scatter_nearby_on_exit = a.boolean()?,
            "OrientLikeContainerOnExit" => self.orient_like_container_on_exit = a.boolean()?,
            "KeepContainerVelocityOnExit" => self.keep_container_velocity_on_exit = a.boolean()?,
            "GoAggressiveOnExit" => self.go_aggressive_on_exit = a.boolean()?,
            "ResetMoodCheckTimeOnExit" => self.reset_mood_check_time_on_exit = a.boolean()?,
            "DestroyRidersWhoAreNotFreeToExit" => self.destroy_riders_who_are_not_free_to_exit = a.boolean()?,
            "ExitBone" => self.exit_bone = a.string(),
            "ExitPitchRate" => self.exit_pitch_rate = a.angular_velocity()?,
            "InitialPayload" => self.initial_payload = Some(parse_name_count(a.rest())),
            "HealthRegen%PerSec" => self.health_regen = a.real()?,
            "ExitDelay" => self.exit_delay = a.duration_frames()?,
            "ArmedRidersUpgradeMyWeaponSet" => self.armed_riders_upgrade_weapon_set = a.boolean()?,
            "DelayExitInAir" => self.is_delay_exit_in_air = a.boolean()?,
            // GarrisonContain
            "MobileGarrison" => self.mobile_garrison = a.boolean()?,
            "HealObjects" => self.do_i_heal_objects = a.boolean()?,
            "TimeForFullHeal" => match self.kind {
                // `TimeForFullHeal` is a different field on each class and with a different scanner:
                // `HealContainModuleData::m_framesForFullHeal` is `INI::parseDurationUnsignedInt`
                // (HealContain.cpp:62), the Garrison and Tunnel ones `INI::parseDurationReal`
                // (GarrisonContain.h:70, TunnelContain.h:68).
                ContainKind::Heal => self.heal_frames = a.duration_frames()?,
                ContainKind::Tunnel => self.tunnel_frames_for_full_heal = a.duration_real()?,
                _ => self.frames_for_full_heal = a.duration_real()?,
            },
            "InitialRoster" => self.initial_roster = Some(parse_name_count(a.rest())),
            "ImmuneToClearBuildingAttacks" => self.immune_to_clear_building_attacks = a.boolean()?,
            "IsEnclosingContainer" => self.is_enclosing_container = a.boolean()?,
            // Overlord / Helix
            "PayloadTemplateName" => {
                for t in a.rest() {
                    self.payload_template_names.push((*t).to_string());
                }
            }
            "ExperienceSinkForRider" => self.experience_sink_for_rider = a.boolean()?,
            "ShouldDrawPips" => self.should_draw_pips = a.boolean()?,
            // ParachuteContain
            "PitchRateMax" => self.pitch_rate_max = a.angular_velocity()?,
            "RollRateMax" => self.roll_rate_max = a.angular_velocity()?,
            "LowAltitudeDamping" => self.low_altitude_damping = a.real()?,
            "ParachuteOpenDist" => self.para_open_dist = a.real()?,
            "KillWhenLandingInWaterSlop" => self.kill_when_landing_in_water_slop = a.real()?,
            "FreeFallDamagePercent" => self.free_fall_damage_percent = a.percent()?,
            "ParachuteOpenSound" => self.parachute_open_sound = sound_or_none(a.string()),
            // RiderChangeContain
            "ScuttleDelay" => self.scuttle_frames = a.duration_frames()?,
            "ScuttleStatus" => self.scuttle_state = a.next()?.to_string(),
            // CaveContain
            "CaveIndex" => self.cave_index = a.int()?,
            k if k.starts_with("Rider") && k[5..].bytes().all(|b| b.is_ascii_digit()) && !k[5..].is_empty() => {
                let idx: usize = k[5..].parse().map_err(|_| "bad rider index".to_string())?;
                if !(1..=8).contains(&idx) {
                    return Err(format!("`{k}` is not a rider slot (Rider1..Rider8)"));
                }
                // `parseRiderInfo` reads six positional tokens, one index list each.
                let toks = a.rest();
                if toks.len() < 6 {
                    return Err(format!("`{k}` needs a template, model condition, weapon set, object status, command set and locomotor set"));
                }
                let riders = &mut self.riders;
                while riders.len() < idx {
                    riders.push(RiderInfo::default());
                }
                riders[idx - 1] = RiderInfo {
                    template_name: toks[0].to_string(),
                    model_condition: toks[1].to_string(),
                    weapon_set: Mask::<WeaponSetType>::default().parse(&toks[2..3])?,
                    object_status: Mask::<ObjectStatus>::default().parse(&toks[3..4])?,
                    command_set: toks[4].to_string(),
                    locomotor_set: LocomotorSetType::from_name(toks[5])
                        .ok_or_else(|| format!("`{}` is not a locomotor set", toks[5]))?,
                };
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    pub(crate) fn parse(class: &str, block: &Block, d: &mut Diag) -> Self {
        let kind = ContainKind::from_class_name(class).expect("typed contain class");
        let mut c = ContainData { kind, ..ContainData::default() };
        // Transport-family, Tunnel and MobNexus default to infantry inside; Garrison also defaults to
        // infantry and is an enclosing container (`GarrisonContainModuleData` constructor); the
        // `MobNexusContainModuleData` constructor sets `KINDOF_INFANTRY` too (MobNexusContain.cpp:60-66);
        // OpenContain allows all kinds.
        if kind.is_transport() || kind == ContainKind::Tunnel || kind == ContainKind::MobNexus {
            c.allow_inside_kind_of = Mask::EMPTY.with(KindOf::Infantry);
        } else if kind == ContainKind::Garrison {
            c.allow_inside_kind_of = Mask::EMPTY.with(KindOf::Infantry);
            c.is_enclosing_container = true;
        }
        for item in &block.items {
            let Item::Field(f) = item else {
                d.warn(WarningKind::Structure, "", 0, "unexpected nested block in contain module");
                continue;
            };
            let mut a = Args::new(&f.value);
            match c.field(&f.key, &mut a) {
                Ok(true) => {}
                Ok(false) => d.unknown_field(f),
                Err(e) => d.bad_value(f, e),
            }
        }
        c
    }

    /// The effective capacity: `Slots` for transports and `MobNexusContain` (its own
    /// `getContainMax` returns `m_slotCapacity`, MobNexusContain.cpp:100-107), `ContainMax`
    /// otherwise (-1 = unlimited).
    pub fn capacity(&self) -> i32 {
        if self.kind.is_transport() || self.kind == ContainKind::MobNexus {
            self.slot_capacity
        } else {
            self.contain_max
        }
    }
}

/// `parseInitialPayload` / `parseInitialRoster`: `Name [count]`.
fn parse_name_count(toks: &[&str]) -> (String, i32) {
    let name = toks.first().copied().unwrap_or("").to_string();
    let count = toks.get(1).and_then(|t| t.parse::<i32>().ok()).unwrap_or(1);
    (name, count)
}

/// `INI::parseAudioEventRTS`: `NoSound`/empty means none.
fn sound_or_none(s: String) -> Option<String> {
    if s.is_empty() || s.eq_ignore_ascii_case("NoSound") {
        None
    } else {
        Some(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ModuleData, Templates, Warning};

    fn build(text: &str) -> (Templates, Vec<Warning>) {
        let blocks = zr_ini::parse("test.ini", text).expect("synthetic INI parses");
        let mut db = zr_ini::IniDb::new();
        db.extend(blocks);
        Templates::from_ini(&db)
    }

    fn contain(t: &Templates, name: &str) -> ContainData {
        let thing = t.thing(name).unwrap_or_else(|| panic!("{name} exists"));
        thing
            .modules
            .iter()
            .find_map(|m| match &m.data {
                ModuleData::Contain(c) => Some((**c).clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{name} has a typed contain module"))
    }

    /// `MobNexusContain` and `CaveContain` are contain classes too (`GameLogic/Object/Contain/`,
    /// both `OpenContain` subclasses): without their names in `ContainKind::from_class_name` they
    /// fall into the `SimpleUpdate` catch-all, get no `ContainTemplate` and no heap slot. Revert
    /// check: both lookups then return `None` and the modules are not `ModuleData::Contain`.
    #[test]
    fn mob_nexus_and_cave_contain_parse_as_their_own_classes() {
        let (t, w) = build(
            "
Object Nexus
  KindOf = VEHICLE
  Behavior = MobNexusContain ModuleTag_01
    Slots = 3
    HealthRegen%PerSec = 5
    InitialPayload = AmericaInfantryRanger 2
  End
End
Object CaveNet
  KindOf = STRUCTURE
  Behavior = CaveContain ModuleTag_01
    CaveIndex = 7
    ContainMax = 4
  End
End
",
        );
        assert!(w.is_empty(), "unexpected warnings: {w:#?}");
        let nexus = contain(&t, "Nexus");
        assert_eq!(nexus.kind, ContainKind::MobNexus);
        // `MobNexusContain::getContainMax` returns `m_slotCapacity` (MobNexusContain.cpp:100-107).
        assert_eq!(nexus.capacity(), 3);
        assert_eq!(nexus.health_regen, 5.0);
        assert_eq!(nexus.initial_payload, Some(("AmericaInfantryRanger".to_string(), 2)));
        // `MobNexusContainModuleData`'s constructor defaults to infantry inside (MobNexusContain.cpp:60-66).
        assert!(nexus.allow_inside_kind_of.contains(KindOf::Infantry));
        assert!(!nexus.allow_inside_kind_of.contains(KindOf::Vehicle));
        let cave = contain(&t, "CaveNet");
        assert_eq!(cave.kind, ContainKind::Cave);
        assert_eq!(cave.cave_index, 7, "CaveIndex (CaveContain.h:49,61)");
        // `CaveContainModuleData` only adds `CaveIndex`; `OpenContain` allows every kind.
        assert_eq!(cave.capacity(), 4);
        assert!(cave.allow_inside_kind_of.contains(KindOf::Vehicle));
    }

    /// `TimeForFullHeal` is a different field with a different scanner per class: Heal is an
    /// `UnsignedInt` of frames (`parseDurationUnsignedInt`), Garrison/Tunnel a `Real`
    /// (GarrisonContain.h:70, TunnelContain.h:68, HealContain.cpp:62). Revert check: the old shared
    /// arm rounded all three to whole frames and wrote the Heal value into the tunnel field.
    #[test]
    fn time_for_full_heal_keeps_each_class_scanner_and_field() {
        let (t, w) = build(
            "
Object Hospital
  KindOf = STRUCTURE
  Behavior = HealContain ModuleTag_01
    TimeForFullHeal = 2500
  End
End
Object Bunker
  KindOf = STRUCTURE
  Behavior = GarrisonContain ModuleTag_01
    HealObjects = Yes
    TimeForFullHeal = 2500
  End
End
Object Tunnel
  KindOf = STRUCTURE
  Behavior = TunnelContain ModuleTag_01
    TimeForFullHeal = 2500
  End
End
",
        );
        assert!(w.is_empty(), "unexpected warnings: {w:#?}");
        let hospital = contain(&t, "Hospital");
        assert_eq!(hospital.heal_frames, 75, "2500 ms ceil to 75 whole frames");
        let bunker = contain(&t, "Bunker");
        assert!(bunker.do_i_heal_objects);
        assert_eq!(bunker.frames_for_full_heal, 75.0, "parseDurationReal gives 75.0 frames");
        assert_eq!(bunker.heal_frames, 0, "a garrison has no HealContain field");
        let tunnel = contain(&t, "Tunnel");
        assert_eq!(tunnel.tunnel_frames_for_full_heal, 75.0);
        assert_eq!(tunnel.frames_for_full_heal, 0.0, "a tunnel has no Garrison field");
    }
}

