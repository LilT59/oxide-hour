//! The adapter seam between the simulation and template data (`ThingTemplate`, `LocomotorTemplate`).
//!
//! `zr-sim` does not depend on the INI crate. A thin adapter turns the typed template database
//! into [`ObjectTemplate`] values (already unit-converted: per-frame velocities, radians) and
//! hands them to [`GameLogic::create_object`](crate::GameLogic::create_object), usually via an
//! implementation of [`TemplateSource`].

use std::collections::BTreeMap;
use std::sync::Arc;

/// `KindOfType` bit set. The engine's `s_bitNameList` has 124 entries; the sim stores a 64-bit
/// subset of the kinds it actually tests (or that script/condition code queries). The adapter
/// (`zr-game::adapter::kind_of_mask_bits`) maps a parsed `zr_templates::KindOfMask` onto these by
/// name, dropping kinds the sim does not model. Extend the list below freely (bit positions are
/// internal; only the names are a contract).
pub mod kind_of {
    macro_rules! kinds {
        ($($n:ident),* $(,)?) => {
            kinds!(@def 0u64; $($n),*);
            /// The `s_bitNameList` names this subset mirrors.
            pub const NAMES: &[&str] = &[$(stringify!($n)),*];
            /// The bit for a `s_bitNameList` index, if modelled.
            pub fn bit_for_index(i: usize) -> Option<u64> {
                if i < NAMES.len() { Some(1u64 << i) } else { None }
            }
            /// The bit for an INI `KindOf` name (case-insensitive).
            pub fn bit_for_name(name: &str) -> Option<u64> {
                NAMES.iter().position(|n| n.eq_ignore_ascii_case(name)).map(|i| 1u64 << i)
            }
        };
        (@def $bit:expr; $head:ident $(, $tail:ident)*) => {
            pub const $head: u64 = 1u64 << $bit;
            kinds!(@def $bit + 1; $($tail),*);
        };
        (@def $bit:expr;) => {};
    }
    kinds!(
        SELECTABLE, IMMOBILE, CAN_ATTACK, STICK_TO_TERRAIN_SLOPE, CAN_CAST_REFLECTIONS,
        SHRUBBERY, STRUCTURE, INFANTRY, VEHICLE, AIRCRAFT, HUGE_VEHICLE, DOZER, HARVESTER,
        COMMANDCENTER, LINEBUILD, SALVAGER, WEAPON_SALVAGER, TRANSPORT, BRIDGE, LANDMARK_BRIDGE,
        BRIDGE_TOWER, PROJECTILE, NO_GARRISON, WAVEGUIDE, WAVE_EFFECT, NO_COLLIDE, REPAIR_PAD,
        HEAL_PAD, STEALTH_GARRISON, CASH_GENERATOR, MP_COUNT_FOR_VICTORY, REBUILD_HOLE, SCORE,
        NO_HEAL_ICON, CAN_RAPPEL, PARACHUTABLE, CRATE, CAPTURABLE, SMALL_MISSILE, ALWAYS_VISIBLE,
        UNATTACKABLE, MINE, CLEANUP_HAZARD, PORTABLE_STRUCTURE, FORCEATTACKABLE, TECH_BUILDING,
        POWERED, DRONE, SUPPLY_SOURCE, REVEAL_TO_ALL, INERT, HERO, DONT_AUTO_CRUSH_INFANTRY,
        FS_POWER, FS_FACTORY, FS_BASE_DEFENSE, FS_TECHNOLOGY, FS_SUPERWEAPON, FS_STRATEGY_CENTER,
        MONEY_HACKER, FS_INTERNET_CENTER, GARRISONABLE_UNTIL_DESTROYED, SPAWNS_ARE_THE_WEAPONS,
        WALK_ON_TOP_OF_WALL,
    );
    /// Sim-only: `TerrainLogic` waypoints are not `KINDOF` in the engine; the sim marks them so
    /// map/scouting code can tell them apart. (`NAMES` intentionally omits this one.)
    pub const WAYPOINT: u64 = 1u64 << 62;
}

/// A second `KindOf` bit set for the kinds the `Collide` modules test that do not fit in the full
/// 64-bit [`kind_of`] set. The adapter maps these by name (`kind_of_mask_bits` pattern); bit
/// positions are internal, only the names are a contract.
pub mod collide_kind {
    macro_rules! kinds {
        ($($n:ident),* $(,)?) => {
            kinds!(@def 0u32; $($n),*);
            /// The `s_bitNameList` names this subset mirrors.
            pub const NAMES: &[&str] = &[$(stringify!($n)),*];
            /// The bit for an INI `KindOf` name (case-insensitive).
            pub fn bit_for_name(name: &str) -> Option<u32> {
                NAMES.iter().position(|n| n.eq_ignore_ascii_case(name)).map(|i| 1u32 << i)
            }
        };
        (@def $bit:expr; $head:ident $(, $tail:ident)*) => {
            pub const $head: u32 = 1u32 << $bit;
            kinds!(@def $bit + 1; $($tail),*);
        };
        (@def $bit:expr;) => {};
    }
    kinds!(
        BOAT, AIRCRAFT_CARRIER, IMMUNE_TO_CAPTURE, PARACHUTE, ARMOR_SALVAGER, FS_FAKE, FS_BARRACKS,
        FS_WARFACTORY, FS_AIRFIELD, FS_SUPPLY_CENTER, FS_SUPPLY_DROPZONE, PRODUCED_AT_HELIPAD,
        // `KINDOF_CAN_SEE_THROUGH_STRUCTURE` (`KindOf.h:125`): a structure that does not block a line
        // of sight (`PathfindCell::setTypeAsObstacle`, `AIPathfind.cpp:2035`).
        CAN_SEE_THROUGH_STRUCTURE,
        // `KINDOF_TECH_BASE_DEFENSE` is the 65th engine kind and does not fit the 64-bit set; it is
        // tested by `AIPlayer::getPlayerSuperweaponValue` (sneak-attack avoidance).
        TECH_BASE_DEFENSE,
        // `KINDOF_SCORE_CREATE` / `KINDOF_SCORE_DESTROY` (`ScoreKeeper.cpp`): the 64-bit set is full.
        SCORE_CREATE, SCORE_DESTROY,
    );
}

/// `GeometryType`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GeometryShape {
    #[default]
    Sphere,
    Cylinder,
    Box,
}

/// `GeometryInfo`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geometry {
    pub shape: GeometryShape,
    pub major_radius: f32,
    pub minor_radius: f32,
    pub height: f32,
    pub is_small: bool,
}

impl Default for Geometry {
    fn default() -> Self {
        Self { shape: GeometryShape::Sphere, major_radius: 1.0, minor_radius: 1.0, height: 1.0, is_small: false }
    }
}

impl Geometry {
    /// `GeometryInfo::getBoundingCircleRadius`: radius used for partition registration.
    pub fn bounding_circle_radius(&self) -> f32 {
        match self.shape {
            GeometryShape::Box => crate::math::sqrt(self.major_radius * self.major_radius + self.minor_radius * self.minor_radius),
            _ => self.major_radius,
        }
    }

    /// `GeometryInfo::getBoundingSphereRadius`: the 3D radius used for airborne collisions.
    pub fn bounding_sphere_radius(&self) -> f32 {
        let h = self.height * 0.5;
        match self.shape {
            GeometryShape::Box => crate::math::sqrt(self.major_radius * self.major_radius + self.minor_radius * self.minor_radius + h * h),
            _ => crate::math::sqrt(self.major_radius * self.major_radius + h * h),
        }
    }

    /// `GeometryInfo::expandFootprint`: grow both 2D radii by `radius` (negative shrinks).
    pub fn expand_footprint(&mut self, radius: f32) {
        self.major_radius += radius;
        self.minor_radius += radius;
    }

    /// `GeometryInfo::getFootprintArea`.
    pub fn footprint_area(&self) -> f32 {
        match self.shape {
            GeometryShape::Box => 4.0 * self.major_radius * self.minor_radius,
            _ => crate::math::PI * self.bounding_circle_radius() * self.bounding_circle_radius(),
        }
    }

    /// `GeometryInfo::isPointInFootprint` (2D).
    pub fn is_point_in_footprint(&self, center: &crate::math::Coord3D, pt: &crate::math::Coord3D) -> bool {
        match self.shape {
            GeometryShape::Box => (pt.x - center.x).abs() <= self.major_radius && (pt.y - center.y).abs() <= self.minor_radius,
            _ => {
                let (dx, dy) = (pt.x - center.x, pt.y - center.y);
                crate::math::sqrt(dx * dx + dy * dy) <= self.major_radius
            }
        }
    }

    /// `GeometryInfo::getMaxHeightAbovePosition` (`Geometry.cpp`): a sphere's radius, else the height.
    /// The line-of-sight tests measure from the top of the collision shape (`PartitionManager.cpp:4591`).
    pub fn max_height_above_position(&self) -> f32 {
        match self.shape {
            GeometryShape::Sphere => self.major_radius,
            GeometryShape::Box | GeometryShape::Cylinder => self.height,
        }
    }

    /// `GeometryInfo::getZDeltaToCenterPosition` (`Geometry.cpp`): a sphere's centre is its position,
    /// a box's or cylinder's is half its height up.
    pub fn z_delta_to_center_position(&self) -> f32 {
        match self.shape {
            GeometryShape::Sphere => 0.0,
            GeometryShape::Box | GeometryShape::Cylinder => self.height * 0.5,
        }
    }
}

/// `LocomotorAppearance` (`TheLocomotorAppearanceNames`). `Climber` is `CLIMBER`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocomotorKind {
    Legs,
    Wheels,
    Treads,
    Hover,
    Thrust,
    Wings,
    Climber,
}

/// `ZAxisBehavior` (`TheLocomotorBehaviorZNames`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ZBehavior {
    #[default]
    NoZMotiveForce,
    SurfaceRelativeHeight,
    AbsoluteHeight,
    FixedSurfaceRelativeHeight,
    FixedAbsoluteHeight,
    FixedRelativeToGroundAndBuildings,
    RelativeToHighestLayer,
}

/// `LocomotorSetType` (`AIUpdate.h`), the sets a unit can switch between.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default, Hash)]
pub enum LocomotorSet {
    #[default]
    Normal,
    NormalUpgraded,
    Freefall,
    Wander,
    Panic,
    Taxiing,
    Supersonic,
    Sluggish,
}

/// Locomotor parameters, in sim units: distance per *frame*, radians per *frame*.
/// (Adapter: INI `Speed` is world units per second -> divide by `LOGICFRAMES_PER_SECOND`;
/// `Acceleration`/`Braking` are per second² -> divide by the frame rate squared;
/// `TurnRate` is degrees per second -> radians per frame. See [`LocomotorParams::from_ini`].)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LocomotorParams {
    pub kind: LocomotorKind,
    pub max_speed: f32,
    pub max_speed_damaged: f32,
    pub min_speed: f32,
    pub max_turn_rate: f32,
    pub max_turn_rate_damaged: f32,
    /// `MinTurnSpeed`: turn radius is `speed / turn_rate`, so below this a turn stalls.
    pub min_turn_speed: f32,
    /// `TurnPivotOffset` (`Locomotor::getTurnPivotOffset`): the turn pivot sits `offset * bounding radius` ahead of
    /// the centre, so turning moves the object (`rotateObjAroundLocoPivot`, Locomotor.cpp:2134).
    pub turn_pivot_offset: f32,
    /// `CanMoveBackwards`: wheeled units may reverse for a 3-point turn.
    pub can_move_backward: bool,
    /// `Extra2DFriction` (per frame), fed to `PhysicsBehavior::setExtraFriction`.
    pub extra_2d_friction: f32,
    pub acceleration: f32,
    pub acceleration_damaged: f32,
    pub braking: f32,
    /// `Lift`: vertical acceleration available (hover/thrust).
    pub lift: f32,
    /// `LiftDamaged` (`Locomotor::getMaxLift`'s damaged branch, `Locomotor.cpp:852-861`).
    pub lift_damaged: f32,
    /// `AirborneTargetingHeight` (`Locomotor::getAirborneTargetingHeight`, `Locomotor.h:284`): an object more than
    /// this far above the ground has `OBJECT_STATUS_AIRBORNE_TARGET` (`AIUpdate.cpp:2296`). `i32::MAX` by default.
    pub airborne_targeting_height: f32,
    pub preferred_height: f32,
    pub preferred_height_damping: f32,
    /// `MaxThrustAngle`: how far the thrust vector may deviate from the facing (radians).
    pub max_thrust_angle: f32,
    pub circling_radius: f32,
    pub behavior_z: ZBehavior,
    pub close_enough_dist: f32,
    /// `LocomotorTemplate::m_isCloseEnoughDist3D` (`CloseEnoughDist3D`): the dist-to-goal is 3D, for
    /// units that must move straight down (`getLocomotorDistanceToGoal`, `AIUpdate.cpp:2468`).
    pub close_enough_dist_3d: bool,
    /// `Locomotor::ULTRA_ACCURATE` (`setUltraAccurate`): a *runtime* flag set by the jet AI while
    /// taxiing / taking off / landing. [`Object::locomotor`] fills it from the object's
    /// [`LocoRuntime`]; the template value is always `false`.
    pub ultra_accurate: bool,
    /// `LocomotorTemplate::m_ultraAccurateSlideIntoPlaceFactor` (`SlideIntoPlaceTime`, frames): how
    /// close (in frames of travel) an ultra-accurate unit must be to slide into place.
    pub slide_into_place_factor: f32,
    /// `Locomotor::PRECISE_Z_POS` (`setUsePreciseZPos`): runtime, like `ultra_accurate`.
    pub precise_z: bool,
    /// Keep the object glued to the terrain height (ground locomotors).
    pub stick_to_ground: bool,
    /// `LocomotorTemplate::getValidSurfaces` bits (`GROUND` / `WATER` / `CLIFF` / `AIR` / `RUBBLE`).
    pub surfaces_ground: bool,
    pub surfaces_air: bool,
    pub surfaces_water: bool,
    pub surfaces_cliff: bool,
    pub surfaces_rubble: bool,
    pub allow_motive_force_while_airborne: bool,
    pub apply_2d_friction_when_airborne: bool,
    pub speed_limit_z: f32,
}

impl LocomotorParams {
    fn base(kind: LocomotorKind) -> Self {
        LocomotorParams {
            kind,
            max_speed: 0.0,
            max_speed_damaged: 0.0,
            min_speed: 0.0,
            max_turn_rate: 0.0,
            max_turn_rate_damaged: 0.0,
            min_turn_speed: 0.0,
            turn_pivot_offset: 0.0,
            can_move_backward: false,
            extra_2d_friction: 0.0,
            acceleration: 0.0,
            acceleration_damaged: 0.0,
            braking: 99999.0,
            lift: 0.0,
            lift_damaged: 0.0,
            airborne_targeting_height: i32::MAX as f32,
            preferred_height: 0.0,
            preferred_height_damping: 1.0,
            max_thrust_angle: 0.0,
            circling_radius: 0.0,
            behavior_z: ZBehavior::NoZMotiveForce,
            close_enough_dist: 1.0,
            close_enough_dist_3d: false,
            ultra_accurate: false,
            slide_into_place_factor: 0.0,
            precise_z: false,
            // `Locomotor::Locomotor` leaves `m_stickToGround = false` (Locomotor.cpp:334); the INI's
            // `StickToGround` sets it.
            stick_to_ground: false,
            surfaces_ground: matches!(kind, LocomotorKind::Legs | LocomotorKind::Wheels | LocomotorKind::Treads | LocomotorKind::Hover),
            surfaces_air: matches!(kind, LocomotorKind::Thrust | LocomotorKind::Wings),
            surfaces_water: kind == LocomotorKind::Hover,
            surfaces_cliff: kind == LocomotorKind::Climber,
            surfaces_rubble: false,
            allow_motive_force_while_airborne: false,
            apply_2d_friction_when_airborne: false,
            speed_limit_z: 999999.0,
        }
    }

    /// Convert INI-style values (per second, degrees) into per-frame sim units.
    pub fn from_ini(kind: LocomotorKind, speed: f32, accel: f32, braking: f32, turn_deg_per_sec: f32) -> Self {
        let fps = crate::math::LOGICFRAMES_PER_SECOND as f32;
        let mut p = Self::base(kind);
        p.max_speed = speed / fps;
        p.max_speed_damaged = p.max_speed;
        p.max_turn_rate = turn_deg_per_sec * (crate::math::PI / 180.0) / fps;
        p.max_turn_rate_damaged = p.max_turn_rate;
        p.acceleration = accel / (fps * fps);
        p.acceleration_damaged = p.acceleration;
        p.braking = braking / (fps * fps);
        p
    }

    /// Build from values that are *already* in engine units (per-frame speed, per-frame^2
    /// acceleration/braking, radians per frame), as produced by the template layer
    /// (`zr-templates`). No conversion is applied; contrast [`from_ini`](Self::from_ini).
    pub fn from_engine_units(
        kind: LocomotorKind,
        max_speed: f32,
        min_speed: f32,
        acceleration: f32,
        braking: f32,
        max_turn_rate: f32,
        stick_to_ground: bool,
    ) -> Self {
        let mut p = Self::base(kind);
        p.max_speed = max_speed;
        p.max_speed_damaged = max_speed;
        p.min_speed = min_speed;
        p.acceleration = acceleration;
        p.acceleration_damaged = acceleration;
        p.braking = braking;
        p.max_turn_rate = max_turn_rate;
        p.max_turn_rate_damaged = max_turn_rate;
        p.stick_to_ground = stick_to_ground;
        p
    }

    /// `isAirborneTarget`: air locomotors ignore the ground pathfinder.
    pub fn is_airborne(&self) -> bool {
        self.surfaces_air || matches!(self.kind, LocomotorKind::Thrust | LocomotorKind::Wings)
    }

    /// Can this locomotor cross water cells? (hover, or an explicit WATER surface.)
    pub fn can_cross_water(&self) -> bool {
        self.surfaces_water || self.kind == LocomotorKind::Hover
    }
}

/// `HijackerUpdateModuleData` (`HijackerUpdate.h`): the hijacker rides a hijacked vehicle until it
/// dies, then is ejected (under a parachute when the vehicle was airborne).
#[derive(Clone, Debug)]
pub struct HijackerTemplate {
    /// `AttachToTargetBone` (client art).
    pub attach_to_bone: String,
    /// `ParachuteName`: the container the ejected hijacker is put into when the vehicle was
    /// airborne (`TheThingFactory->newObject(putInContainerTmpl, ...)`).
    pub parachute: Option<std::sync::Arc<ObjectTemplate>>,
}

/// `TechBuildingBehaviorModuleData` (`TechBuildingBehavior.cpp:52-74`): the pulse FX list and its rate. The FX itself
/// is a client effect (`FXList::doFXObj`), recorded for the viewer; the rate is the module's `update` sleep while the
/// building is captured.
#[derive(Clone, Debug, PartialEq)]
pub struct TechBuildingTemplate {
    /// `PulseFX` (the FX list name).
    pub pulse_fx: Option<String>,
    /// `PulseFXRate` (frames, from `INI::parseDurationUnsignedInt`).
    pub pulse_fx_rate: u32,
}

/// `AutoFindHealingUpdateModuleData` (`AutoFindHealingUpdate.cpp:55-77`): a hurt unit scans for a
/// `KINDOF_HEAL_PAD` object and orders itself healed there. `ScanRange` is world units, `ScanRate` is the
/// frames between scans (the module's update returns `UPDATE_SLEEP_NONE`, so the countdown is its own).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AutoFindHealingTemplate {
    /// `ScanRate` in frames.
    pub scan_frames: u32,
    pub scan_range: f32,
    /// `NeverHeal`: above this fraction of max health the unit does not look for healing.
    pub never_heal: f32,
    /// `AlwaysHeal`: parsed, but the engine's `update` returns before the branch that reads it
    /// (`AutoFindHealingUpdate.cpp:130-138`).
    pub always_heal: f32,
}

/// `DynamicShroudClearingRangeUpdate`'s module data (`DynamicShroudClearingRangeUpdateModuleData`, its field parse at
/// DynamicShroudClearingRangeUpdate.cpp:60-78). The frame counts are the INI durations converted like the engine
/// (`parseDurationUnsignedInt`, rounding up); the vision values are world units.
#[derive(Clone, Debug, PartialEq)]
pub struct DynamicShroudTemplate {
    /// `ChangeInterval`.
    pub change_interval: u32,
    /// `GrowInterval`.
    pub grow_interval: u32,
    /// `ShrinkDelay`.
    pub shrink_delay: u32,
    /// `ShrinkTime`.
    pub shrink_time: u32,
    /// `GrowDelay`.
    pub grow_delay: u32,
    /// `GrowTime`.
    pub grow_time: u32,
    /// `FinalVision`.
    pub final_vision: f32,
}

/// `GrantUpgradeCreateModuleData` (`Object/Create/GrantUpgradeCreate.cpp:42-62`): the upgrade a
/// finished (or freshly created, not-under-construction) object grants, and to whom.
#[derive(Clone, Debug, PartialEq)]
pub struct GrantUpgradeCreateTemplate {
    /// `UpgradeToGrant`.
    pub upgrade: String,
    /// `UpgradeTemplate::getUpgradeType() == UPGRADE_TYPE_PLAYER`: the controlling player's upgrade
    /// (`Player::addUpgrade`), else the object's own (`Object::giveUpgrade`).
    pub player_upgrade: bool,
    /// `ExemptStatus` contains `OBJECT_STATUS_UNDER_CONSTRUCTION`: `onCreate` does nothing for a
    /// building that is still under construction; `onBuildComplete` grants it later instead. (The
    /// module only tests that one status bit, GrantUpgradeCreate.cpp:88-95.)
    pub exempt_under_construction: bool,
}

/// What the sim needs to know about a `ThingTemplate`.
#[derive(Clone, Debug)]
pub struct ObjectTemplate {
    pub name: String,
    pub kind_of: u64,
    /// `KINDOF_AUTO_RALLYPOINT`: a selected object of this kind takes a right-click rally point
    /// (`InGameUI::canSelectedObjectsDoAction` `ACTIONTYPE_SET_RALLY_POINT`). Kept out of the 64-bit
    /// `kind_of` set, which is full.
    pub auto_rally_point: bool,
    pub geometry: Geometry,
    /// `ActiveBody` `MaxHealth`. 0 means "no body" (indestructible scenery such as waypoints).
    pub max_health: f32,
    pub initial_health: f32,
    /// `ActiveBodyModuleData::m_subdualDamageCap` (0: the body cannot be subdued, `ActiveBody::canBeSubdued`),
    /// `m_subdualDamageHealRate` (frames) and `m_subdualDamageHealAmount`. Read by `ActiveBody::with_subdual`.
    pub subdual_damage_cap: f32,
    pub subdual_heal_rate: u32,
    pub subdual_heal_amount: f32,
    /// The `SET_NORMAL` locomotor (a shortcut for the common case; see `locomotor_sets`).
    pub locomotor: Option<LocomotorParams>,
    /// Every `Locomotor = SET_x name` entry (`AIUpdateModuleData::m_locomotorTemplates`).
    pub locomotor_sets: BTreeMap<LocomotorSet, LocomotorParams>,
    /// The resolved entries behind each set of [`locomotor_sets`](Self::locomotor_sets). A set switch builds one
    /// `Locomotor` per entry (`chooseLocomotorSetExplicit`, AIUpdate.cpp:830-841, `LocomotorSet::addLocomotor`), and
    /// each ctor draws (Locomotor.cpp:673-675). The port keeps one params value per set, so the count lives here.
    pub locomotor_entries: BTreeMap<LocomotorSet, u8>,
    /// `Some(n)` when the object has an `AIUpdate` module: `n` is the number of resolved `SET_NORMAL`
    /// locomotor templates. Creation draws the RNG for them (`AIUpdateInterface::AIUpdateInterface`,
    /// AIUpdate.cpp:296, then `Locomotor::Locomotor`, Locomotor.cpp:673-675) and for the state machines
    /// (`AIUpdateInterface::onObjectCreated`, AIUpdate.cpp:629). `None` for everything else.
    pub ai_update: Option<u8>,
    /// `true` when the `AIUpdate` module comes before the stealth detector in the template (`Object::Object`
    /// builds the modules in definition order, so the locomotor draws of the AI come first then).
    pub ai_before_stealth: bool,
    pub vision_range: f32,
    /// `ThingTemplate::getShroudClearingRange`; `-1` means "use `vision_range`" (the engine's
    /// `m_shroudClearingRange` default).
    pub shroud_clearing_range: f32,
    /// `ThingTemplate::getShroudRevealToAllRange`; `-1` means no extra reveal-to-all circle.
    pub shroud_reveal_to_all_range: f32,
    /// Armor, weapon set, turret and auto-acquire settings.
    pub combat: crate::weapon::CombatTemplate,
    /// Die modules in definition order (`Object::m_behaviors` order).
    pub die: Vec<crate::die::DieModuleTemplate>,
    /// The template's behaviour modules that are sleepy-heap update modules in C++, in INI (`ModuleInfo`) order
    /// (`Object::Object`'s behaviour loop, Object.cpp:405-410, and the `registerObject` push that follows). The
    /// object's helper modules come before these. See [`SleepyModuleKind`].
    pub update_module_order: Vec<SleepyModuleKind>,
    /// `RadarUpdateModuleData::m_radarExtendTime` (`RadarExtendTime`, ms -> frames, real): how long
    /// `RadarUpdate::extendRadar` takes to finish (120 frames on the shipped command centers).
    pub radar_extend_time: f32,
    /// `true` when any die module is a `SpecialPowerCompletionDie`.
    pub has_special_power_completion: bool,
    /// `ThingTemplate::calcCostToBuild` (power/handicap factors at 1.0).
    pub build_cost: i32,
    /// `ThingTemplate::getPlacementViewAngle`, radians (`PlacementViewAngle`).
    pub placement_view_angle: f32,
    /// `ThingTemplate::calcTimeToBuild` in logic frames (`BuildTime` seconds * 30, truncated).
    pub build_time_frames: u32,
    /// `Prerequisites` for building this object.
    pub prerequisites: Vec<crate::production::Prerequisite>,
    /// The `ProductionUpdate` + exit update, when this object is a factory.
    pub production: Option<crate::production::ProductionTemplate>,
    /// The contain module, when this object can hold passengers.
    pub contain: Option<std::sync::Arc<crate::contain::ContainTemplate>>,
    /// `RiderChangeContain` rider table (a combat bike's `Rider1..8`). `contain` holds the transport
    /// rules; the table adds the rider-specific changes (see [`crate::rider`]).
    pub rider_change: Option<std::sync::Arc<crate::rider::RiderChangeTemplate>>,
    /// `ThingTemplate::getTransportSlotCount`: how many slots this object takes in a transport.
    pub transport_slot_count: u32,
    /// `ThingTemplate::getEnergyProduction`: positive produces power, negative consumes it.
    pub energy_production: i32,
    /// `ThingTemplate::getEnergyBonus`: added to a producer's power while `PowerPlantUpgrade` or
    /// `OverchargeBehavior` is active (`Object.cpp:3900`).
    pub energy_bonus: i32,
    /// The supply dock module, when this object is a warehouse or supply center.
    pub dock: Option<crate::supply::DockTemplate>,
    /// `DockStart` / `DockAction` / `DockEnd` / `DockWaiting` bones of a dock, object-local; zero
    /// when the model's bones were not resolved (the C++ "no bones" path).
    pub dock_bones: crate::dock::DockBones,
    /// The supply truck module, when this object harvests supplies.
    pub supply_truck: Option<crate::supply::SupplyTruckTemplate>,
    /// The `ChinookAIUpdate` combat-drop module, when this object can rappel troops down.
    pub chinook: Option<crate::chinook::ChinookTemplate>,
    /// The `AssaultTransportAIUpdate` fields (the troop crawler), when this object is an assault transport.
    pub assault_transport: Option<crate::assault_transport::AssaultTransportTemplate>,
    /// The object has a `DeployStyleAIUpdate` module (sentry drone, deploy-style artillery). Only its sleep return is
    /// ported: `update` reports `UPDATE_SLEEP_NONE` regardless (`DeployStyleAIUpdate.cpp:274-277`). The deploy/undeploy
    /// state machine (`DeployStyleAIUpdate.cpp:95-277`) is not ported.
    pub deploy_style_ai: bool,
    /// The `DozerAIUpdate` / `WorkerAIUpdate` fields, when this object is a dozer/worker.
    pub dozer: Option<crate::dozer::DozerTemplate>,
    /// The `HackInternetAIUpdate` fields, when this object can hack the internet (`HACK_INTERNET`).
    pub hack_internet: Option<crate::hack_internet::HackInternetTemplate>,
    /// `SkillPointValue` per veterancy level (`-999` = use the experience value, see
    /// [`ObjectTemplate::skill_point_value_at`]).
    pub skill_point_value: [i32; 4],
    /// `ExperienceValue` / `ExperienceRequired` per veterancy level, and `IsTrainable`.
    pub experience_value: [i32; 4],
    pub experience_required: [i32; 4],
    pub is_trainable: bool,
    /// Armor sets with their condition masks (`ArmorTemplateSet::conditions`).
    pub armor_sets: Vec<(u32, Option<std::sync::Arc<crate::armor::ArmorTemplate>>)>,
    /// Weapon sets with their condition masks (`WeaponTemplateSet::conditions`).
    pub weapon_sets: Vec<(u32, crate::weapon::WeaponSetTemplate)>,
    /// Typed upgrade modules (`giveUpgrade` / `updateUpgradeModules`).
    pub upgrade_modules: Vec<std::sync::Arc<crate::upgrade::SimUpgradeModule>>,
    /// Special power modules (`OCLSpecialPower`, `FireWeaponPower`, `CashHackSpecialPower`).
    pub special_powers: Vec<std::sync::Arc<crate::special_power::SimSpecialPower>>,
    /// Unit special abilities (`SpecialAbilityUpdate`).
    pub special_abilities: Vec<std::sync::Arc<crate::special_power::SpecialAbilityTemplate>>,
    /// Has a `SpecialPowerCreate` module, whose `onBuildComplete` starts the powers' recharges
    /// (`SpecialPowerCreate.cpp:57`).
    pub special_power_create: bool,
    /// `GrantUpgradeCreate` modules (`Object/Create/GrantUpgradeCreate.cpp`), in INI order.
    pub grant_upgrade_create: Vec<GrantUpgradeCreateTemplate>,
    /// `PhysicsBehavior` parameters, when the object has a physics module.
    pub physics: Option<crate::physics::PhysicsTemplate>,
    /// `ThingTemplate::isBridge`: bridge objects are not pathfinder obstacles (units drive over).
    pub is_bridge: bool,
    /// `ThingTemplate::getFenceWidth` (`FenceWidth`). A non-zero width makes the object a fence:
    /// `classifyFence` marks a thin oriented strip instead of a footprint (`AIPathfind.cpp:4172`).
    pub fence_width: f32,
    /// `ThingTemplate::getFenceXOffset` (`FenceXOffset`).
    pub fence_x_offset: f32,
    /// `KINDOF_DEFENSIVE_WALL`: fences on these objects are not classified (they are footprints).
    /// The kind-of bit set is full, so the adapter maps this by name.
    pub defensive_wall: bool,
    /// `KINDOF_BALLISTIC_MISSILE` (`getVictimAntiMask`, `WeaponSet.cpp:396`): not in the 64-bit kind set, so the
    /// adapter maps it by name.
    pub ballistic_missile: bool,
    /// `KINDOF_DEMOTRAP` (`getVictimAntiMask`, `WeaponSet.cpp:404`): not in the 64-bit kind set, mapped by name.
    pub demotrap: bool,
    /// `KINDOF_ATTACK_NEEDS_LINE_OF_SIGHT`: `findAttackPath` tests the firing line (`AIPathfind.cpp:9475`).
    pub attack_needs_line_of_sight: bool,
    /// `KINDOF_AIRCRAFT_PATH_AROUND`: `getAircraftPath` clips its destination around such a building
    /// (`circleClipsTallBuilding`, `AIPathfind.cpp:9628-9645`) and inserts detour nodes for one on its
    /// segment (`segmentIntersectsTallBuilding`, `:9570-9624`). The kind-of bit set is full, so the
    /// adapter maps this by name.
    pub aircraft_path_around: bool,
    /// `ThingTemplate::getFactoryExitWidth` (`FactoryExitWidth`): the width of the exit rectangle a
    /// factory keeps clear, in front of its body (`BuildAssistant::isLocationClearOfObjects`, `:780`).
    pub factory_exit_width: f32,
    /// `ThingTemplate::getFactoryExtraBibWidth` (`FactoryExtraBibWidth`): grows the body box this object
    /// blocks builds with (`BuildAssistant.cpp:838-843`).
    pub factory_extra_bib_width: f32,
    /// `KINDOF_CLEARED_BY_BUILD`: removed when a building is placed over it (`isRemovableForConstruction`).
    /// The kind-of bit set is full, so the adapter maps this by name.
    pub cleared_by_build: bool,
    /// `KINDOF_ALWAYS_SELECTABLE`: a UI feedback object that `clearRemovableForConstruction` never destroys.
    pub always_selectable: bool,
    /// `KINDOF_IGNORED_IN_GUI`: with `INFANTRY` this marks an angry-mob member, which is exempt from
    /// the mood matrix (`AIUpdateInterface::getMoodMatrixActionAdjustment`, AIUpdate.cpp:4376).
    pub ignored_in_gui: bool,
    /// The [`collide_kind`] bits for this template (the kinds the `Collide` modules test that do
    /// not fit in the full 64-bit `kind_of` set).
    pub collide_kinds: u32,
    /// `AutoHealBehavior` modules (heal self/allies over time), one per module in the template's INI order. An
    /// object may carry several (the GLA Scorpion Tank has two, lane 178).
    pub auto_heal: Vec<crate::behaviors::AutoHealTemplate>,
    /// `FlammableUpdate` (flame damage over time).
    pub flammable: Option<crate::behaviors::FlammableTemplate>,
    /// `PoisonedBehavior` (poison damage over time).
    pub poisoned: Option<crate::behaviors::PoisonedTemplate>,
    /// `FireWeaponWhenDeadBehavior` (death weapon + its `DieMuxData` mask).
    pub fire_weapon_when_dead: Option<crate::behaviors::DeathWeapon>,
    /// `LifetimeUpdate` (`MinLifetime`, `MaxLifetime` frames); kills at the rolled frame.
    pub lifetime: Option<(u32, u32)>,
    /// `DeletionUpdate` (`MinLifetime`, `MaxLifetime` frames); *destroys* at the rolled frame
    /// (`DeletionUpdate.cpp`) rather than killing, so no death effects run.
    pub deletion: Option<(u32, u32)>,
    /// `DynamicShroudClearingRangeUpdate`: ramps the object's shroud clearing range (`DynamicShroudClearingRangeUpdate.cpp`).
    pub dynamic_shroud: Option<DynamicShroudTemplate>,
    /// `StealthUpdate`.
    pub stealth: Option<crate::behaviors::StealthTemplate>,
    /// `StealthDetectorUpdate`.
    pub stealth_detector: Option<crate::behaviors::StealthDetectorTemplate>,
    /// `CrusherLevel`/`CrushableLevel`/`SquishCollide` (crush decisions).
    pub crush: crate::modules2::CrushInfo,
    /// `ToppleUpdate`.
    pub topple: Option<crate::modules2::ToppleTemplate>,
    /// `AutoDepositUpdate` (oil derrick / black market income).
    pub auto_deposit: Option<crate::modules2::AutoDepositTemplate>,
    /// `FireWeaponUpdate` (periodic self-fired weapon).
    pub fire_weapon: Option<crate::modules2::FireWeaponTemplate>,
    /// `HordeUpdate`.
    pub horde: Option<crate::modules2::HordeTemplate>,
    /// `BaseRegenerateUpdate`.
    pub base_regenerate: bool,
    /// `SpawnBehavior`: the first batch of slaves its update creates (`SpawnNumber`, `SpawnTemplateName`, ...).
    pub spawn: Option<crate::modules2::SpawnTemplate>,
    /// `SpawnBehaviorModuleData::m_dieMuxData` (`SpawnBehavior.cpp:137-140`): which deaths let the module's `onDie`
    /// notify and kill its spawns.
    pub spawn_die_mux: Option<crate::die::DieMux>,
    /// `SlavedUpdate`.
    pub slaved: Option<crate::modules2::SlavedTemplate>,
    /// `CrateCollide` modules (an object may carry several: the GLA saboteur has six sabotage
    /// variants). All are dispatched on collision, in template order.
    pub crate_collides: Vec<crate::modules2::CrateCollideTemplate>,
    /// `FireWeaponCollide` (shoot whatever collides with me).
    pub fire_weapon_collide: Option<crate::modules2::FireWeaponCollideTemplate>,
    /// `CreateCrateDie` (spawn these crates on death).
    pub create_crate: Option<Vec<String>>,
    /// `EjectPilotDie`.
    pub eject_pilot: Option<crate::modules2::EjectPilotTemplate>,
    /// `VeterancyGainCreate`.
    pub veterancy_gain: Option<crate::modules2::VeterancyGainTemplate>,
    /// `PropagandaTowerBehavior` (aura heal + bonus).
    pub propaganda: Option<crate::modules2::PropagandaTemplate>,
    /// `RepairDockUpdate`.
    pub repair_dock: Option<crate::modules2::RepairDockTemplate>,
    /// `OverchargeBehavior` (the toggle and its health drain; `PowerPlantUpdate` is [`power_plant`]).
    pub overcharge: Option<crate::modules2::OverchargeTemplate>,
    /// `PowerPlantUpdate`: the rod-extension timer `extendRods` wakes on (`PowerPlantUpdateModuleData::m_rodsExtendTime`).
    pub power_plant: Option<crate::modules2::PowerPlantTemplate>,
    /// `TechBuildingBehavior`: the pulse FX and its rate (`PulseFX`/`PulseFXRate`).
    pub tech_building: Option<TechBuildingTemplate>,
    /// `AutoFindHealingUpdate` (`ScanRate`/`ScanRange`/`NeverHeal`/`AlwaysHeal`).
    pub auto_find_healing: Option<AutoFindHealingTemplate>,
    /// `ProneUpdate`.
    pub prone: Option<crate::modules2::ProneTemplate>,
    /// `AssistedTargetingUpdate` (the first module, which `Object::findUpdateModule` returns; assist.rs).
    pub assisted_targeting: Option<crate::assist::AssistedTargetingTemplate>,
    /// `CountermeasuresBehavior` (flares vs missiles).
    pub countermeasures: Option<crate::modules2::CountermeasuresTemplate>,
    /// `JetAIUpdate` (airfield state machine).
    pub jet: Option<crate::modules2::JetAiTemplate>,
    /// `ParkingPlaceBehavior` (airfield parking/runways).
    pub parking_place: Option<crate::modules2::ParkingPlaceTemplate>,
    /// The airfield's parking-space / runway bones (resolved from the model by the adapter).
    pub parking_bones: crate::jet_ai::ParkingBones,
    /// `TransitionDamageFX` (client FX hook on body state change).
    pub transition_damage: Option<crate::modules2::TransitionDamageTemplate>,
    /// `KeepObjectDie`: the object stays (as rubble) instead of being removed.
    pub keep_object_die: bool,
    /// `MinefieldBehavior` (detonation).
    pub minefield: Option<crate::modules2::MinefieldTemplate>,
    /// `RebuildHoleBehavior` (GLA structures rebuild after death).
    pub rebuild_hole: Option<crate::modules4::RebuildHoleTemplate>,
    /// `DemoTrapUpdate` (proximity detonation).
    pub demo_trap: Option<crate::modules4::DemoTrapTemplate>,
    /// `CleanupHazardUpdate` (targets `KINDOF_CLEANUP_HAZARD`).
    pub cleanup_hazard: Option<crate::modules4::CleanupHazardTemplate>,
    /// `CommandButtonHuntUpdate`.
    pub command_button_hunt: Option<crate::modules4::CommandButtonHuntTemplate>,
    /// `GenerateMinefieldBehavior` (lay a minefield around this object).
    pub generate_minefield: Option<crate::modules4::GenerateMinefieldTemplate>,
    /// `HeightDieUpdate` (die when descending past a height).
    pub height_die: Option<crate::superweapon::HeightDieTemplate>,
    /// `NeutronMissileUpdate` (the nuclear missile's flight).
    pub neutron_missile: Option<crate::superweapon::NeutronMissileTemplate>,
    /// `SpyVisionUpdate` (reveal enemy units, ClientUpdate-adjacent but sim state).
    pub spy_vision: Option<crate::superweapon::SpyVisionTemplate>,
    /// `ParticleUplinkCannonUpdate` (America superweapon).
    pub particle_uplink: Option<crate::superweapon::ParticleUplinkTemplate>,
    /// `SpectreGunshipDeploymentUpdate` (Command Center power).
    pub spectre_deployment: Option<crate::superweapon::SpectreDeploymentTemplate>,
    /// `SpectreGunshipUpdate` (the orbiting gunship).
    pub spectre_gunship: Option<crate::superweapon::SpectreGunshipTemplate>,
    /// `MissileLauncherBuildingUpdate` (superweapon door model conditions).
    pub missile_launcher: Option<crate::superweapon::MissileLauncherBuildingTemplate>,
    /// `CashBountyPower` (passive): the player's cash-bounty fraction while this object lives.
    pub cash_bounty: Option<f32>,
    /// The `CashBountyPower` module's `SpecialPowerTemplate` name: its `RequiredScience` gates the creation hook
    /// (`CashBountyPower::onObjectCreated`, CashBountyPower.cpp:129-137).
    pub cash_bounty_power: Option<String>,
    /// `HijackerUpdate`: the hijacker rides a hijacked vehicle until it dies.
    pub hijacker: Option<HijackerTemplate>,
    /// `RailroadBehavior`: the train's carriage list (and whether this object is the locomotive).
    pub railroad: Option<Railroad>,
}

/// The `RailroadBehavior` fields the train logic reads (`RailroadGuideAIUpdate.cpp`).
#[derive(Clone, Debug, PartialEq)]
pub struct Railroad {
    /// `IsLocomotive`: the lead car; only a locomotive creates its carriages.
    pub is_locomotive: bool,
    /// `CarriageTemplateName`, in INI order: the carriages behind the locomotive, front to back.
    pub carriage_templates: Vec<String>,
    /// `SpeedMax` (locomotive).
    pub speed_max: f32,
    /// `Acceleration`: the per-frame multiplier while the conductor accelerates.
    pub acceleration: f32,
    /// `Braking`: the per-frame multiplier while the conductor applies the brakes.
    pub braking: f32,
    /// `Friction`: the per-frame multiplier of a coasting lead carriage.
    pub friction: f32,
    /// `WaitAtStationTime` in logic frames.
    pub wait_at_station_time: u32,
    /// `RunningGarrisonSpeedMax`: below this speed at a station (or while coasting) a unit may board
    /// through the train (`onCollide`, `RailroadGuideAIUpdate.cpp:286`, `:340`).
    pub running_garrison_speed_max: f32,
    /// `KillSpeedMin`: at or above this speed a unit hit by the locomotive is killed instead of
    /// crushed (`RailroadGuideAIUpdate.cpp:369`).
    pub kill_speed_min: f32,
    /// The `PhysicsBehaviorModuleData` half of the block: `RailroadBehaviorModuleData` derives from
    /// `PhysicsBehaviorModuleData` (`RailroadGuideAIUpdate.h:43`) and its field parse runs that table
    /// first (`:51`), so `Mass`, `AllowBouncing`, ... live in the RailroadBehavior block.
    ///
    /// This is *not* `ObjectTemplate::physics`: `Object::m_physics` is only filled for the module named
    /// `PhysicsBehavior` (`Object.cpp:445-449`), so `getPhysics()` returns null for a train — its
    /// `PhysicsBehavior` half is the `RailroadBehavior` module itself, and `RailroadBehavior::update`
    /// overrides `PhysicsBehavior::update` (`RailroadGuideAIUpdate.h:224`). Nothing reads the train's
    /// own physics data in the engine; the port keeps it to carry the block faithfully.
    pub physics: crate::physics::PhysicsTemplate,
}

impl Default for Railroad {
    /// `RailroadBehaviorModuleData::RailroadBehaviorModuleData`
    /// (`RailroadGuideAIUpdate.cpp:87-94`).
    fn default() -> Self {
        Railroad {
            is_locomotive: false,
            carriage_templates: Vec::new(),
            speed_max: 4.0,
            acceleration: 1.01,
            braking: 0.99,
            friction: 0.97,
            wait_at_station_time: 150,
            running_garrison_speed_max: 1.0,
            kill_speed_min: 1.0,
            physics: crate::physics::PhysicsTemplate::default(),
        }
    }
}

/// A behaviour module that is a sleepy-heap `UpdateModule` in C++ (`registerObject` pushes it, GameLogic.cpp:4165-4200),
/// named by the C++ class it stands for. Lane 110 derived the order of these from the template's INI module list;
/// [`ObjectTemplate::update_module_order`] holds them in that order. Each variant is one C++ class (or one family of
/// them, as noted). Behaviour modules with no variant here are not heap entries in the sim yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SleepyModuleKind {
    /// `AIUpdateInterface` and its subclasses (`DozerAIUpdate`, `WorkerAIUpdate`, ...): the object's AI slot.
    Ai,
    /// `AutoHealBehavior`.
    AutoHeal,
    /// `AutoDepositUpdate`.
    AutoDeposit,
    /// `AutoFindHealingUpdate`: the scan that sends a hurt unit to a `KINDOF_HEAL_PAD` object
    /// (`AutoFindHealingUpdate.cpp:102-147`).
    AutoFindHealing,
    /// `BaseRegenerateUpdate`.
    BaseRegenerate,
    /// `BridgeBehavior` (and the bridge towers' `BridgeTowerBehavior`).
    Bridge,
    /// `SupplyWarehouseDockUpdate` / `SupplyCenterDockUpdate` (the `DockUpdate` family, minus `RepairDockUpdate`).
    Dock,
    /// `FlammableUpdate`.
    Flammable,
    /// `GarrisonContain` (`ContainKind::Garrison`).
    GarrisonContain,
    /// `HealContain` (`ContainKind::Heal`).
    HealContain,
    /// `OpenContain`.
    OpenContain,
    /// `TransportContain`.
    TransportContain,
    /// `TunnelContain`.
    TunnelContain,
    /// `OverlordContain`.
    OverlordContain,
    /// `HelixContain`.
    HelixContain,
    /// `ParachuteContain`.
    ParachuteContain,
    /// `InternetHackContain`.
    InternetHackContain,
    /// `RailedTransportContain`.
    RailedTransportContain,
    /// `RiderChangeContain`.
    RiderChangeContain,
    /// `MobNexusContain`.
    MobNexusContain,
    /// `CaveContain`.
    CaveContain,
    /// `PhysicsBehavior` (and the `RailroadBehavior` that derives from it, which has its own variant).
    Physics,
    /// `PoisonedBehavior`.
    Poisoned,
    /// `PowerPlantUpdate` (`PowerPlantUpgrade` is not a heap module) — `OverchargeBehavior` has its own variant.
    PowerPlant,
    /// `OverchargeBehavior` (the power-plant toggle and health drain; `PowerPlantUpdate` has its own variant).
    Overcharge,
    /// `ProductionUpdate`.
    Production,
    /// `DefaultProductionExitUpdate`, `QueueProductionExitUpdate`, `SupplyCenterProductionExitUpdate`,
    /// `SpawnPointProductionExitUpdate`.
    ProductionExit,
    /// `PropagandaTowerBehavior`.
    Propaganda,
    /// `RadarUpdate`.
    Radar,
    /// `RepairDockUpdate`.
    RepairDock,
    /// `RailroadBehavior` (the train's guide update, `RailroadGuideAIUpdate`).
    Railroad,
    /// `SpawnBehavior`.
    Spawn,
    /// `SlowDeathBehavior` and its `JetSlowDeath` / `HelicopterSlowDeath` / `BattleBusSlowDeath` / `NeutronMissileSlowDeath`
    /// subclasses: one heap entry per slow-death die module. `die` is the index into [`ObjectTemplate::die`].
    SlowDeath { die: usize },
    /// `SpectreGunshipDeploymentUpdate`.
    Spectre,
    /// `StealthUpdate`.
    Stealth,
    /// `StealthDetectorUpdate`.
    StealthDetector,
    /// `TechBuildingBehavior`.
    TechBuilding,
    /// `ToppleUpdate`.
    Topple,
    /// `LifetimeUpdate`.
    Lifetime,
    /// `DeletionUpdate`: a rolled lifetime that *destroys* the object (`DeletionUpdate.cpp:60-73`).
    Deletion,
    /// `DynamicShroudClearingRangeUpdate`: ramps the shroud clearing range every frame.
    DynamicShroud,
    /// `SpecialAbilityUpdate` (Black Lotus, Jarmen Kell, Burton, the GLA rebel, ...): one heap entry per
    /// `SpecialAbilityUpdate` module. The `SpecialAbility` module it pairs with is a `SpecialPowerModule`, not an
    /// `UpdateModule`, so it is not in this list (`SpecialAbilityUpdate.h:169`).
    SpecialAbility,
    /// `SlavedUpdate`.
    Slaved,
    /// `HordeUpdate`.
    Horde,
    /// `AssistedTargetingUpdate` (AssistedTargetingUpdate.cpp:145-148): one heap entry per module (AmericaPatriotBattery).
    AssistedTargeting,
    /// `FireWeaponUpdate`.
    FireWeapon,
    /// `ParkingPlaceBehavior`.
    ParkingPlace,
    /// `DumbProjectileBehavior`.
    DumbProjectile,
    /// `CountermeasuresBehavior`.
    Countermeasures,
    /// `MinefieldBehavior`.
    Minefield,
    /// `GenerateMinefieldBehavior`: an `UpdateModule` (and a die module) of its own, one heap entry per module
    /// (GenerateMinefieldBehavior.h:68-70).
    GenerateMinefield,
    /// `ProneUpdate`.
    Prone,
    /// `DemoTrapUpdate`.
    DemoTrap,
    /// `CleanupHazardUpdate` (the Humvee's hazard cleanup, `CleanupHazardUpdate.cpp`).
    CleanupHazard,
    /// `CommandButtonHuntUpdate` (the GLA rebel's and the US ranger's hunt; `CommandButtonHuntUpdate.cpp`).
    CommandButtonHunt,
}

impl ObjectTemplate {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            kind_of: 0,
            auto_rally_point: false,
            geometry: Geometry::default(),
            max_health: 100.0,
            initial_health: 100.0,
            subdual_damage_cap: 0.0,
            subdual_heal_rate: 0,
            subdual_heal_amount: 0.0,
            locomotor: None,
            locomotor_sets: BTreeMap::new(),
            locomotor_entries: BTreeMap::new(),
            ai_update: None,
            ai_before_stealth: true,
            vision_range: 0.0,
            shroud_clearing_range: -1.0,
            shroud_reveal_to_all_range: -1.0,
            combat: crate::weapon::CombatTemplate::default(),
            die: Vec::new(),
            update_module_order: Vec::new(),
            radar_extend_time: 0.0,
            has_special_power_completion: false,
            build_cost: 0,
            placement_view_angle: 0.0,
            build_time_frames: 0,
            prerequisites: Vec::new(),
            production: None,
            contain: None,
            rider_change: None,
            transport_slot_count: 0,
            energy_production: 0,
            energy_bonus: 0,
            dock: None,
            dock_bones: Default::default(),
            supply_truck: None,
            chinook: None,
            assault_transport: None,
            deploy_style_ai: false,
            dozer: None,
            hack_internet: None,
            skill_point_value: [-999; 4],
            experience_value: [0; 4],
            experience_required: [0; 4],
            is_trainable: false,
            armor_sets: Vec::new(),
            weapon_sets: Vec::new(),
            upgrade_modules: Vec::new(),
            special_powers: Vec::new(),
            special_abilities: Vec::new(),
            special_power_create: false,
            grant_upgrade_create: Vec::new(),
            physics: None,
            is_bridge: false,
            fence_width: 0.0,
            fence_x_offset: 0.0,
            defensive_wall: false,
            ballistic_missile: false,
            demotrap: false,
            attack_needs_line_of_sight: false,
            aircraft_path_around: false,
            ignored_in_gui: false,
            factory_exit_width: 0.0,
            factory_extra_bib_width: 0.0,
            cleared_by_build: false,
            always_selectable: false,
            collide_kinds: 0,
            auto_heal: Vec::new(),
            flammable: None,
            poisoned: None,
            fire_weapon_when_dead: None,
            lifetime: None,
            deletion: None,
            dynamic_shroud: None,
            stealth: None,
            stealth_detector: None,
            crush: crate::modules2::CrushInfo::default(),
            topple: None,
            auto_deposit: None,
            fire_weapon: None,
            horde: None,
            base_regenerate: false,
            spawn: None,
            spawn_die_mux: None,
            slaved: None,
            crate_collides: Vec::new(),
            fire_weapon_collide: None,
            create_crate: None,
            eject_pilot: None,
            veterancy_gain: None,
            propaganda: None,
            repair_dock: None,
            overcharge: None,
            power_plant: None,
            tech_building: None,
            auto_find_healing: None,
            prone: None,
            assisted_targeting: None,
            countermeasures: None,
            jet: None,
            parking_place: None,
            parking_bones: Default::default(),
            transition_damage: None,
            keep_object_die: false,
            minefield: None,
            rebuild_hole: None,
            demo_trap: None,
            cleanup_hazard: None,
            command_button_hunt: None,
            generate_minefield: None,
            height_die: None,
            neutron_missile: None,
            spy_vision: None,
            particle_uplink: None,
            spectre_deployment: None,
            spectre_gunship: None,
            missile_launcher: None,
            cash_bounty: None,
            cash_bounty_power: None,
            hijacker: None,
            railroad: None,
        }
    }
    pub fn is_kind_of(&self, bits: u64) -> bool {
        self.kind_of & bits == bits
    }

    /// `ThingTemplate::getSkillPointValue` (ThingTemplate.cpp:1423): the `SkillPointValue` for `level`, or
    /// the experience value when it is the `USE_EXP_VALUE_FOR_SKILL_VALUE` (-999) default.
    pub fn skill_point_value_at(&self, level: usize) -> i32 {
        let v = self.skill_point_value[level];
        if v == -999 {
            self.experience_value[level]
        } else {
            v
        }
    }
    /// [`collide_kind`] test.
    pub fn is_collide_kind_of(&self, bits: u32) -> bool {
        self.collide_kinds & bits == bits
    }

    /// The locomotor for `set`, falling back to the `SET_NORMAL` shortcut.
    pub fn locomotor_for(&self, set: LocomotorSet) -> Option<LocomotorParams> {
        self.locomotor_sets.get(&set).copied().or(self.locomotor)
    }

    /// How many `Locomotor`s a switch to `set` builds (one per resolved entry, `AIUpdate.cpp:830-841`). A template
    /// that records no entry count for the set (one built by hand) has one locomotor when there is a params value.
    pub fn locomotor_entry_count(&self, set: LocomotorSet) -> u32 {
        match self.locomotor_entries.get(&set) {
            Some(&n) => u32::from(n),
            None => u32::from(self.locomotor_for(set).is_some()),
        }
    }
}

/// `ThingFactory::findTemplate`. The adapter over the real template database implements this.
pub trait TemplateSource {
    fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>>;
    /// Every `KINDOF_HARVESTER` template in `ThingFactory::firstTemplate` order (newest first), for
    /// `AIPlayer::queueSupplyTruck`'s template scan. Empty when the source cannot enumerate.
    fn harvester_templates(&self) -> Vec<String> {
        Vec::new()
    }
    /// Every `KINDOF_DOZER` template in `ThingFactory::firstTemplate` order (newest first), for
    /// `AIPlayer::queueDozer`'s scan (AIPlayer.cpp:3169-3236): the first one a factory can build is queued. Empty when
    /// the source cannot enumerate.
    fn dozer_templates(&self) -> Vec<String> {
        Vec::new()
    }
    /// `BuildAssistant::isPossibleToMakeUnit(builder, unit)`: does the builder's command set build
    /// `unit`? `None` when the source has no command-set data.
    fn can_make_unit(&self, _builder: &str, _unit: &str) -> Option<bool> {
        None
    }
    /// `AIPlayer::buildUpgrade`'s `canUpgradeHere` (`AIPlayer.cpp:1822-1836`): does the builder's command
    /// set hold a button for player `upgrade` (`CommandButton::getUpgradeTemplate`)? `None` when the
    /// source has no command-set data.
    fn can_build_upgrade(&self, _builder: &str, _upgrade: &str) -> Option<bool> {
        None
    }
}

/// Trivial in-memory source (tests, examples).
#[derive(Default)]
pub struct SimpleTemplateStore(BTreeMap<String, Arc<ObjectTemplate>>);

impl SimpleTemplateStore {
    pub fn add(&mut self, t: ObjectTemplate) -> Arc<ObjectTemplate> {
        let a = Arc::new(t);
        self.0.insert(a.name.to_ascii_lowercase(), a.clone());
        a
    }
}

impl TemplateSource for SimpleTemplateStore {
    fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
        self.0.get(&name.to_ascii_lowercase()).cloned()
    }
}
