//! Projectile behavior module data: `DumbProjectileBehavior` (Bezier ballistic arc) and
//! `MissileAIUpdate` (a locomotor-driven homing missile).
//!
//! Ported from `GameLogic/Object/Behavior/DumbProjectileBehavior.cpp` (constructor defaults and
//! `buildFieldParse`) and `GameLogic/Object/Update/AIUpdate/MissileAIUpdate.cpp`. Durations are
//! frames, velocities per frame, percents fractions; see [`crate::value`].

use zr_ini::{Block, Item};

use crate::diag::{Diag, WarningKind};
use crate::flags::{KindOfMask, Mask};
use crate::value::{Args, LOGICFRAMES_PER_SECOND};

/// `DumbProjectileBehaviorModuleData`. The four heights/indents define the cubic Bezier: the first
/// and last control points are the firer and victim, the two inner ones sit this high above the
/// highest intervening terrain, this far along the target line.
#[derive(Debug, Clone, PartialEq)]
pub struct DumbProjectileData {
    pub first_height: f32,
    pub second_height: f32,
    pub first_percent_indent: f32,
    pub second_percent_indent: f32,
    /// `MaxLifespan`, frames (default 10 s).
    pub max_lifespan: u32,
    pub tumble_randomly: bool,
    pub orient_to_flight_path: bool,
    pub detonate_calls_kill: bool,
    pub garrison_hit_kill_count: i32,
    pub garrison_hit_kill_kindof: KindOfMask,
    pub garrison_hit_kill_kindof_not: KindOfMask,
    pub garrison_hit_kill_fx: Option<String>,
    /// `FlightPathAdjustDistPerSecond`, distance per frame.
    pub flight_path_adjust_dist_per_frame: f32,
}

impl Default for DumbProjectileData {
    fn default() -> Self {
        DumbProjectileData {
            first_height: 0.0,
            second_height: 0.0,
            first_percent_indent: 0.0,
            second_percent_indent: 0.0,
            max_lifespan: 10 * LOGICFRAMES_PER_SECOND,
            tumble_randomly: false,
            orient_to_flight_path: true,
            detonate_calls_kill: false,
            garrison_hit_kill_count: 0,
            garrison_hit_kill_kindof: Mask::EMPTY,
            garrison_hit_kill_kindof_not: Mask::EMPTY,
            garrison_hit_kill_fx: None,
            flight_path_adjust_dist_per_frame: 0.0,
        }
    }
}

impl DumbProjectileData {
    /// One row of `buildFieldParse`; `Ok(false)` is an unknown key.
    fn field(&mut self, key: &str, a: &mut Args) -> Result<bool, String> {
        match key {
            "MaxLifespan" => self.max_lifespan = a.duration_frames()?,
            "TumbleRandomly" => self.tumble_randomly = a.boolean()?,
            "DetonateCallsKill" => self.detonate_calls_kill = a.boolean()?,
            "OrientToFlightPath" => self.orient_to_flight_path = a.boolean()?,
            "FirstHeight" => self.first_height = a.real()?,
            "SecondHeight" => self.second_height = a.real()?,
            "FirstPercentIndent" => self.first_percent_indent = a.percent()?,
            "SecondPercentIndent" => self.second_percent_indent = a.percent()?,
            "GarrisonHitKillRequiredKindOf" => self.garrison_hit_kill_kindof = self.garrison_hit_kill_kindof.parse(a.rest())?,
            "GarrisonHitKillForbiddenKindOf" => self.garrison_hit_kill_kindof_not = self.garrison_hit_kill_kindof_not.parse(a.rest())?,
            "GarrisonHitKillCount" => self.garrison_hit_kill_count = a.uint()? as i32,
            "GarrisonHitKillFX" => self.garrison_hit_kill_fx = fx_or_none(a.string()),
            "FlightPathAdjustDistPerSecond" => self.flight_path_adjust_dist_per_frame = a.velocity()?,
            _ => return Ok(false),
        }
        Ok(true)
    }

    pub(crate) fn parse(block: &Block, d: &mut Diag) -> Self {
        let mut x = Self::default();
        for item in &block.items {
            let Item::Field(f) = item else {
                d.warn(WarningKind::Structure, "", 0, "unexpected nested block in DumbProjectileBehavior");
                continue;
            };
            let mut a = Args::new(&f.value);
            match x.field(&f.key, &mut a) {
                Ok(true) => {}
                Ok(false) => d.unknown_field(f),
                Err(e) => d.bad_value(f, e),
            }
        }
        x
    }
}

/// `MissileAIUpdateModuleData` (extends `AIUpdateModuleData`). Fuel/ignition/lock distances in
/// frames and world units.
#[derive(Debug, Clone, PartialEq)]
pub struct MissileAiData {
    pub try_to_follow_target: bool,
    /// `FuelLifetime`, frames; 0 = infinite.
    pub fuel_lifetime: u32,
    /// `IgnitionDelay`, frames.
    pub ignition_delay: u32,
    /// `InitialVelocity`, distance per frame.
    pub initial_velocity: f32,
    /// `DistanceToTravelBeforeTurning`.
    pub initial_dist: f32,
    /// `DistanceToTargetBeforeDiving`.
    pub dive_distance: f32,
    /// `DistanceToTargetForLock` (default 75).
    pub lock_distance: f32,
    pub use_weapon_speed: bool,
    pub detonate_on_no_fuel: bool,
    /// `DistanceScatterWhenJammed` (default 75).
    pub distance_scatter_when_jammed: f32,
    pub garrison_hit_kill_count: i32,
    pub garrison_hit_kill_kindof: KindOfMask,
    pub garrison_hit_kill_kindof_not: KindOfMask,
    pub garrison_hit_kill_fx: Option<String>,
    pub ignition_fx: Option<String>,
    pub detonate_calls_kill: bool,
    /// `KillSelfDelay`, frames (default 3).
    pub kill_self_delay: u32,
}

impl Default for MissileAiData {
    fn default() -> Self {
        MissileAiData {
            try_to_follow_target: true,
            fuel_lifetime: 0,
            ignition_delay: 0,
            initial_velocity: 0.0,
            initial_dist: 0.0,
            dive_distance: 0.0,
            lock_distance: 75.0,
            use_weapon_speed: false,
            detonate_on_no_fuel: false,
            distance_scatter_when_jammed: 75.0,
            garrison_hit_kill_count: 0,
            garrison_hit_kill_kindof: Mask::EMPTY,
            garrison_hit_kill_kindof_not: Mask::EMPTY,
            garrison_hit_kill_fx: None,
            ignition_fx: None,
            detonate_calls_kill: false,
            kill_self_delay: 3,
        }
    }
}

impl MissileAiData {
    /// One row of `MissileAIUpdateModuleData::buildFieldParse`; `Ok(false)` is an unknown key.
    pub(crate) fn field(&mut self, key: &str, a: &mut Args) -> Result<bool, String> {
        match key {
            "TryToFollowTarget" => self.try_to_follow_target = a.boolean()?,
            "FuelLifetime" => self.fuel_lifetime = a.duration_frames()?,
            "IgnitionDelay" => self.ignition_delay = a.duration_frames()?,
            "InitialVelocity" => self.initial_velocity = a.velocity()?,
            "DistanceToTravelBeforeTurning" => self.initial_dist = a.real()?,
            "DistanceToTargetBeforeDiving" => self.dive_distance = a.real()?,
            "DistanceToTargetForLock" => self.lock_distance = a.real()?,
            "IgnitionFX" => self.ignition_fx = fx_or_none(a.string()),
            "UseWeaponSpeed" => self.use_weapon_speed = a.boolean()?,
            "DetonateOnNoFuel" => self.detonate_on_no_fuel = a.boolean()?,
            "DistanceScatterWhenJammed" => self.distance_scatter_when_jammed = a.real()?,
            "GarrisonHitKillRequiredKindOf" => self.garrison_hit_kill_kindof = self.garrison_hit_kill_kindof.parse(a.rest())?,
            "GarrisonHitKillForbiddenKindOf" => self.garrison_hit_kill_kindof_not = self.garrison_hit_kill_kindof_not.parse(a.rest())?,
            "GarrisonHitKillCount" => self.garrison_hit_kill_count = a.uint()? as i32,
            "GarrisonHitKillFX" => self.garrison_hit_kill_fx = fx_or_none(a.string()),
            "DetonateCallsKill" => self.detonate_calls_kill = a.boolean()?,
            "KillSelfDelay" => self.kill_self_delay = a.duration_frames()?,
            _ => return Ok(false),
        }
        Ok(true)
    }
}

/// `INI::parseFXList` keyword handling: `None` means no effect.
fn fx_or_none(s: String) -> Option<String> {
    if s.is_empty() || s.eq_ignore_ascii_case("None") {
        None
    } else {
        Some(s)
    }
}
