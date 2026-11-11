// Generated once from the reference name tables (variant `Self` is spelled `SelfTarget`).
//! Name tables ported from the reference sources. Each enum lists the engine's index-list names in order;
//! the discriminant is the engine's index (bit index for flag enums).

use crate::flags::name_enum;

name_enum! {
    /// Object kind flags (`KindOfMaskType::s_bitNameList`, `Common/KindOf.h`/`KindOf.cpp`). Order = bit index.
    pub enum KindOf {
        Obstacle = "OBSTACLE",
        Selectable = "SELECTABLE",
        Immobile = "IMMOBILE",
        CanAttack = "CAN_ATTACK",
        StickToTerrainSlope = "STICK_TO_TERRAIN_SLOPE",
        CanCastReflections = "CAN_CAST_REFLECTIONS",
        Shrubbery = "SHRUBBERY",
        Structure = "STRUCTURE",
        Infantry = "INFANTRY",
        Vehicle = "VEHICLE",
        Aircraft = "AIRCRAFT",
        HugeVehicle = "HUGE_VEHICLE",
        Dozer = "DOZER",
        Harvester = "HARVESTER",
        Commandcenter = "COMMANDCENTER",
        Linebuild = "LINEBUILD",
        Salvager = "SALVAGER",
        WeaponSalvager = "WEAPON_SALVAGER",
        Transport = "TRANSPORT",
        Bridge = "BRIDGE",
        LandmarkBridge = "LANDMARK_BRIDGE",
        BridgeTower = "BRIDGE_TOWER",
        Projectile = "PROJECTILE",
        Preload = "PRELOAD",
        NoGarrison = "NO_GARRISON",
        Waveguide = "WAVEGUIDE",
        WaveEffect = "WAVE_EFFECT",
        NoCollide = "NO_COLLIDE",
        RepairPad = "REPAIR_PAD",
        HealPad = "HEAL_PAD",
        StealthGarrison = "STEALTH_GARRISON",
        CashGenerator = "CASH_GENERATOR",
        DrawableOnly = "DRAWABLE_ONLY",
        MpCountForVictory = "MP_COUNT_FOR_VICTORY",
        RebuildHole = "REBUILD_HOLE",
        Score = "SCORE",
        ScoreCreate = "SCORE_CREATE",
        ScoreDestroy = "SCORE_DESTROY",
        NoHealIcon = "NO_HEAL_ICON",
        CanRappel = "CAN_RAPPEL",
        Parachutable = "PARACHUTABLE",
        CanBeRepulsed = "CAN_BE_REPULSED",
        MobNexus = "MOB_NEXUS",
        IgnoredInGui = "IGNORED_IN_GUI",
        Crate = "CRATE",
        Capturable = "CAPTURABLE",
        ClearedByBuild = "CLEARED_BY_BUILD",
        SmallMissile = "SMALL_MISSILE",
        AlwaysVisible = "ALWAYS_VISIBLE",
        Unattackable = "UNATTACKABLE",
        Mine = "MINE",
        CleanupHazard = "CLEANUP_HAZARD",
        PortableStructure = "PORTABLE_STRUCTURE",
        AlwaysSelectable = "ALWAYS_SELECTABLE",
        AttackNeedsLineOfSight = "ATTACK_NEEDS_LINE_OF_SIGHT",
        WalkOnTopOfWall = "WALK_ON_TOP_OF_WALL",
        DefensiveWall = "DEFENSIVE_WALL",
        FsPower = "FS_POWER",
        FsFactory = "FS_FACTORY",
        FsBaseDefense = "FS_BASE_DEFENSE",
        FsTechnology = "FS_TECHNOLOGY",
        AircraftPathAround = "AIRCRAFT_PATH_AROUND",
        LowOverlappable = "LOW_OVERLAPPABLE",
        Forceattackable = "FORCEATTACKABLE",
        AutoRallypoint = "AUTO_RALLYPOINT",
        TechBuilding = "TECH_BUILDING",
        Powered = "POWERED",
        ProducedAtHelipad = "PRODUCED_AT_HELIPAD",
        Drone = "DRONE",
        CanSeeThroughStructure = "CAN_SEE_THROUGH_STRUCTURE",
        BallisticMissile = "BALLISTIC_MISSILE",
        ClickThrough = "CLICK_THROUGH",
        SupplySourceOnPreview = "SUPPLY_SOURCE_ON_PREVIEW",
        Parachute = "PARACHUTE",
        GarrisonableUntilDestroyed = "GARRISONABLE_UNTIL_DESTROYED",
        Boat = "BOAT",
        ImmuneToCapture = "IMMUNE_TO_CAPTURE",
        Hulk = "HULK",
        ShowPortraitWhenControlled = "SHOW_PORTRAIT_WHEN_CONTROLLED",
        SpawnsAreTheWeapons = "SPAWNS_ARE_THE_WEAPONS",
        CannotBuildNearSupplies = "CANNOT_BUILD_NEAR_SUPPLIES",
        SupplySource = "SUPPLY_SOURCE",
        RevealToAll = "REVEAL_TO_ALL",
        Disguiser = "DISGUISER",
        Inert = "INERT",
        Hero = "HERO",
        IgnoresSelectAll = "IGNORES_SELECT_ALL",
        DontAutoCrushInfantry = "DONT_AUTO_CRUSH_INFANTRY",
        CliffJumper = "CLIFF_JUMPER",
        FsSupplyDropzone = "FS_SUPPLY_DROPZONE",
        FsSuperweapon = "FS_SUPERWEAPON",
        FsBlackMarket = "FS_BLACK_MARKET",
        FsSupplyCenter = "FS_SUPPLY_CENTER",
        FsStrategyCenter = "FS_STRATEGY_CENTER",
        MoneyHacker = "MONEY_HACKER",
        ArmorSalvager = "ARMOR_SALVAGER",
        RevealsEnemyPaths = "REVEALS_ENEMY_PATHS",
        BoobyTrap = "BOOBY_TRAP",
        FsFake = "FS_FAKE",
        FsInternetCenter = "FS_INTERNET_CENTER",
        BlastCrater = "BLAST_CRATER",
        Prop = "PROP",
        OptimizedTree = "OPTIMIZED_TREE",
        FsAdvancedTech = "FS_ADVANCED_TECH",
        FsBarracks = "FS_BARRACKS",
        FsWarfactory = "FS_WARFACTORY",
        FsAirfield = "FS_AIRFIELD",
        AircraftCarrier = "AIRCRAFT_CARRIER",
        NoSelect = "NO_SELECT",
        RejectUnmanned = "REJECT_UNMANNED",
        CannotRetaliate = "CANNOT_RETALIATE",
        TechBaseDefense = "TECH_BASE_DEFENSE",
        EmpHardened = "EMP_HARDENED",
        Demotrap = "DEMOTRAP",
        ConservativeBuilding = "CONSERVATIVE_BUILDING",
        IgnoreDockingBones = "IGNORE_DOCKING_BONES",
        NoAttackWarning = "NO_ATTACK_WARNING",
    }
}

name_enum! {
    /// Damage types (`DamageTypeFlags::s_bitNameList`, `Core/.../Damage.cpp`; Zero Hour list, no `FLESHY_SNIPER`).
    pub enum DamageType {
        Explosion = "EXPLOSION",
        Crush = "CRUSH",
        ArmorPiercing = "ARMOR_PIERCING",
        SmallArms = "SMALL_ARMS",
        Gattling = "GATTLING",
        Radiation = "RADIATION",
        Flame = "FLAME",
        Laser = "LASER",
        Sniper = "SNIPER",
        Poison = "POISON",
        Healing = "HEALING",
        Unresistable = "UNRESISTABLE",
        Water = "WATER",
        Deploy = "DEPLOY",
        Surrender = "SURRENDER",
        Hack = "HACK",
        KillPilot = "KILL_PILOT",
        Penalty = "PENALTY",
        Falling = "FALLING",
        Melee = "MELEE",
        Disarm = "DISARM",
        HazardCleanup = "HAZARD_CLEANUP",
        ParticleBeam = "PARTICLE_BEAM",
        Toppling = "TOPPLING",
        InfantryMissile = "INFANTRY_MISSILE",
        AuroraBomb = "AURORA_BOMB",
        LandMine = "LAND_MINE",
        JetMissiles = "JET_MISSILES",
        StealthjetMissiles = "STEALTHJET_MISSILES",
        MolotovCocktail = "MOLOTOV_COCKTAIL",
        ComancheVulcan = "COMANCHE_VULCAN",
        SubdualMissile = "SUBDUAL_MISSILE",
        SubdualVehicle = "SUBDUAL_VEHICLE",
        SubdualBuilding = "SUBDUAL_BUILDING",
        SubdualUnresistable = "SUBDUAL_UNRESISTABLE",
        Microwave = "MICROWAVE",
        KillGarrisoned = "KILL_GARRISONED",
        Status = "STATUS",
    }
}

name_enum! {
    /// Death types (`TheDeathNames`, `Damage.h`).
    pub enum DeathType {
        Normal = "NORMAL",
        None = "NONE",
        Crushed = "CRUSHED",
        Burned = "BURNED",
        Exploded = "EXPLODED",
        Poisoned = "POISONED",
        Toppled = "TOPPLED",
        Flooded = "FLOODED",
        Suicided = "SUICIDED",
        Lasered = "LASERED",
        Detonated = "DETONATED",
        Splatted = "SPLATTED",
        PoisonedBeta = "POISONED_BETA",
        Extra2 = "EXTRA_2",
        Extra3 = "EXTRA_3",
        Extra4 = "EXTRA_4",
        Extra5 = "EXTRA_5",
        Extra6 = "EXTRA_6",
        Extra7 = "EXTRA_7",
        Extra8 = "EXTRA_8",
        PoisonedGamma = "POISONED_GAMMA",
    }
}

name_enum! {
    /// Weapon-set condition flags (`WeaponSetFlags::s_bitNameList`, `WeaponSet.cpp`).
    pub enum WeaponSetType {
        Veteran = "VETERAN",
        Elite = "ELITE",
        Hero = "HERO",
        PlayerUpgrade = "PLAYER_UPGRADE",
        CrateupgradeOne = "CRATEUPGRADE_ONE",
        CrateupgradeTwo = "CRATEUPGRADE_TWO",
        VehicleHijack = "VEHICLE_HIJACK",
        Carbomb = "CARBOMB",
        MineClearingDetail = "MINE_CLEARING_DETAIL",
        WeaponRider1 = "WEAPON_RIDER1",
        WeaponRider2 = "WEAPON_RIDER2",
        WeaponRider3 = "WEAPON_RIDER3",
        WeaponRider4 = "WEAPON_RIDER4",
        WeaponRider5 = "WEAPON_RIDER5",
        WeaponRider6 = "WEAPON_RIDER6",
        WeaponRider7 = "WEAPON_RIDER7",
        WeaponRider8 = "WEAPON_RIDER8",
    }
}

name_enum! {
    /// Armor-set condition flags (`ArmorSetFlags::s_bitNameList`, `BitFlags.cpp`).
    pub enum ArmorSetType {
        Veteran = "VETERAN",
        Elite = "ELITE",
        Hero = "HERO",
        PlayerUpgrade = "PLAYER_UPGRADE",
        WeakVersusBasedefenses = "WEAK_VERSUS_BASEDEFENSES",
        SecondLife = "SECOND_LIFE",
        CrateUpgradeOne = "CRATE_UPGRADE_ONE",
        CrateUpgradeTwo = "CRATE_UPGRADE_TWO",
    }
}

name_enum! {
    /// Weapon bonus conditions (`TheWeaponBonusNames`, `Weapon.h`).
    pub enum WeaponBonusCondition {
        Garrisoned = "GARRISONED",
        Horde = "HORDE",
        ContinuousFireMean = "CONTINUOUS_FIRE_MEAN",
        ContinuousFireFast = "CONTINUOUS_FIRE_FAST",
        Nationalism = "NATIONALISM",
        PlayerUpgrade = "PLAYER_UPGRADE",
        DroneSpotting = "DRONE_SPOTTING",
        DemoralizedObsolete = "DEMORALIZED_OBSOLETE",
        Enthusiastic = "ENTHUSIASTIC",
        Veteran = "VETERAN",
        Elite = "ELITE",
        Hero = "HERO",
        BattleplanBombardment = "BATTLEPLAN_BOMBARDMENT",
        BattleplanHoldtheline = "BATTLEPLAN_HOLDTHELINE",
        BattleplanSearchanddestroy = "BATTLEPLAN_SEARCHANDDESTROY",
        Subliminal = "SUBLIMINAL",
        SoloHumanEasy = "SOLO_HUMAN_EASY",
        SoloHumanNormal = "SOLO_HUMAN_NORMAL",
        SoloHumanHard = "SOLO_HUMAN_HARD",
        SoloAiEasy = "SOLO_AI_EASY",
        SoloAiNormal = "SOLO_AI_NORMAL",
        SoloAiHard = "SOLO_AI_HARD",
        TargetFaerieFire = "TARGET_FAERIE_FIRE",
        Fanaticism = "FANATICISM",
        FrenzyOne = "FRENZY_ONE",
        FrenzyTwo = "FRENZY_TWO",
        FrenzyThree = "FRENZY_THREE",
    }
}

name_enum! {
    /// Weapon bonus fields (`TheWeaponBonusFieldNames`).
    pub enum WeaponBonusField {
        Damage = "DAMAGE",
        Radius = "RADIUS",
        Range = "RANGE",
        RateOfFire = "RATE_OF_FIRE",
        PreAttack = "PRE_ATTACK",
    }
}

name_enum! {
    /// Object status bits (`ObjectStatusMaskType::s_bitNameList`).
    pub enum ObjectStatus {
        None = "NONE",
        Destroyed = "DESTROYED",
        CanAttack = "CAN_ATTACK",
        UnderConstruction = "UNDER_CONSTRUCTION",
        Unselectable = "UNSELECTABLE",
        NoCollisions = "NO_COLLISIONS",
        NoAttack = "NO_ATTACK",
        AirborneTarget = "AIRBORNE_TARGET",
        Parachuting = "PARACHUTING",
        Repulsor = "REPULSOR",
        Hijacked = "HIJACKED",
        Aflame = "AFLAME",
        Burned = "BURNED",
        Wet = "WET",
        IsFiringWeapon = "IS_FIRING_WEAPON",
        IsBraking = "IS_BRAKING",
        Stealthed = "STEALTHED",
        Detected = "DETECTED",
        CanStealth = "CAN_STEALTH",
        Sold = "SOLD",
        UndergoingRepair = "UNDERGOING_REPAIR",
        Reconstructing = "RECONSTRUCTING",
        Masked = "MASKED",
        IsAttacking = "IS_ATTACKING",
        UsingAbility = "USING_ABILITY",
        IsAimingWeapon = "IS_AIMING_WEAPON",
        NoAttackFromAi = "NO_ATTACK_FROM_AI",
        IgnoringStealth = "IGNORING_STEALTH",
        IsCarbomb = "IS_CARBOMB",
        DeckHeightOffset = "DECK_HEIGHT_OFFSET",
        StatusRider1 = "STATUS_RIDER1",
        StatusRider2 = "STATUS_RIDER2",
        StatusRider3 = "STATUS_RIDER3",
        StatusRider4 = "STATUS_RIDER4",
        StatusRider5 = "STATUS_RIDER5",
        StatusRider6 = "STATUS_RIDER6",
        StatusRider7 = "STATUS_RIDER7",
        StatusRider8 = "STATUS_RIDER8",
        FaerieFire = "FAERIE_FIRE",
        KillingSelf = "KILLING_SELF",
        ReassignParking = "REASSIGN_PARKING",
        BoobyTrapped = "BOOBY_TRAPPED",
        Immobile = "IMMOBILE",
        Disguised = "DISGUISED",
        Deployed = "DEPLOYED",
    }
}

name_enum! {
    /// `RadiusDamageAffects` flags (`TheWeaponAffectsMaskNames`).
    pub enum WeaponAffects {
        SelfTarget = "SELF",
        Allies = "ALLIES",
        Enemies = "ENEMIES",
        Neutrals = "NEUTRALS",
        Suicide = "SUICIDE",
        NotSimilar = "NOT_SIMILAR",
        NotAirborne = "NOT_AIRBORNE",
    }
}

name_enum! {
    /// `ProjectileCollidesWith` flags (`TheWeaponCollideMaskNames`).
    pub enum WeaponCollide {
        Allies = "ALLIES",
        Enemies = "ENEMIES",
        Structures = "STRUCTURES",
        Shrubbery = "SHRUBBERY",
        Projectiles = "PROJECTILES",
        Walls = "WALLS",
        SmallMissiles = "SMALL_MISSILES",
        BallisticMissiles = "BALLISTIC_MISSILES",
        ControlledStructures = "CONTROLLED_STRUCTURES",
    }
}

name_enum! {
    /// `AutoReloadsClip` (`TheWeaponReloadNames`).
    pub enum WeaponReload {
        Yes = "YES",
        No = "NO",
        ReturnToBase = "RETURN_TO_BASE",
    }
}

name_enum! {
    /// `PreAttackType` (`TheWeaponPrefireNames`).
    pub enum WeaponPrefire {
        PerShot = "PER_SHOT",
        PerAttack = "PER_ATTACK",
        PerClip = "PER_CLIP",
    }
}

name_enum! {
    /// Weapon slots (`TheWeaponSlotTypeNames`).
    pub enum WeaponSlot {
        Primary = "PRIMARY",
        Secondary = "SECONDARY",
        Tertiary = "TERTIARY",
    }
}

name_enum! {
    /// `AutoChooseSources` flags (`TheCommandSourceMaskNames`).
    pub enum CommandSource {
        FromPlayer = "FROM_PLAYER",
        FromScript = "FROM_SCRIPT",
        FromAi = "FROM_AI",
        FromDozer = "FROM_DOZER",
        DefaultSwitchWeapon = "DEFAULT_SWITCH_WEAPON",
    }
}

name_enum! {
    /// `Surfaces` flags (`TheLocomotorSurfaceTypeNames`).
    pub enum LocomotorSurface {
        Ground = "GROUND",
        Water = "WATER",
        Cliff = "CLIFF",
        Air = "AIR",
        Rubble = "RUBBLE",
    }
}

name_enum! {
    /// `Appearance` (`TheLocomotorAppearanceNames`).
    pub enum LocomotorAppearance {
        TwoLegs = "TWO_LEGS",
        FourWheels = "FOUR_WHEELS",
        Treads = "TREADS",
        Hover = "HOVER",
        Thrust = "THRUST",
        Wings = "WINGS",
        Climber = "CLIMBER",
        Other = "OTHER",
        Motorcycle = "MOTORCYCLE",
    }
}

name_enum! {
    /// `ZAxisBehavior` (`TheLocomotorBehaviorZNames`).
    pub enum LocomotorBehaviorZ {
        NoZMotiveForce = "NO_Z_MOTIVE_FORCE",
        SeaLevel = "SEA_LEVEL",
        SurfaceRelativeHeight = "SURFACE_RELATIVE_HEIGHT",
        AbsoluteHeight = "ABSOLUTE_HEIGHT",
        FixedSurfaceRelativeHeight = "FIXED_SURFACE_RELATIVE_HEIGHT",
        FixedAbsoluteHeight = "FIXED_ABSOLUTE_HEIGHT",
        FixedRelativeToGroundAndBuildings = "FIXED_RELATIVE_TO_GROUND_AND_BUILDINGS",
        RelativeToHighestLayer = "RELATIVE_TO_HIGHEST_LAYER",
    }
}

name_enum! {
    /// `GroupMovementPriority` (`TheLocomotorPriorityNames`).
    pub enum LocomotorPriority {
        MovesBack = "MOVES_BACK",
        MovesMiddle = "MOVES_MIDDLE",
        MovesFront = "MOVES_FRONT",
    }
}

name_enum! {
    /// Locomotor set names (`TheLocomotorSetNames`, `AIUpdate.h`).
    pub enum LocomotorSetType {
        SetNormal = "SET_NORMAL",
        SetNormalUpgraded = "SET_NORMAL_UPGRADED",
        SetFreefall = "SET_FREEFALL",
        SetWander = "SET_WANDER",
        SetPanic = "SET_PANIC",
        SetTaxiing = "SET_TAXIING",
        SetSupersonic = "SET_SUPERSONIC",
        SetSluggish = "SET_SLUGGISH",
    }
}

name_enum! {
    /// `AutoAcquireEnemiesWhenIdle` flags (`TheAutoAcquireEnemiesNames`).
    pub enum AutoAcquire {
        Yes = "YES",
        Stealthed = "STEALTHED",
        No = "NO",
        Notwhileattacking = "NOTWHILEATTACKING",
        AttackBuildings = "ATTACK_BUILDINGS",
    }
}

name_enum! {
    /// `EditorSorting` (`EditorSortingNames`, `ThingSort.h`).
    pub enum EditorSorting {
        None = "NONE",
        Structure = "STRUCTURE",
        Infantry = "INFANTRY",
        Vehicle = "VEHICLE",
        Shrubbery = "SHRUBBERY",
        MiscManMade = "MISC_MAN_MADE",
        MiscNatural = "MISC_NATURAL",
        Debris = "DEBRIS",
        System = "SYSTEM",
        Audio = "AUDIO",
        Test = "TEST",
        ForReview = "FOR_REVIEW",
        Road = "ROAD",
        Waypoint = "WAYPOINT",
    }
}

name_enum! {
    /// `RadarPriority` (`RadarPriorityNames`).
    pub enum RadarPriority {
        Invalid = "INVALID",
        NotOnRadar = "NOT_ON_RADAR",
        Structure = "STRUCTURE",
        Unit = "UNIT",
        LocalUnitOnly = "LOCAL_UNIT_ONLY",
    }
}

name_enum! {
    /// `Buildable` (`BuildableStatusNames`).
    pub enum BuildableStatus {
        Yes = "Yes",
        IgnorePrerequisites = "Ignore_Prerequisites",
        No = "No",
        OnlyByAi = "Only_By_AI",
    }
}

name_enum! {
    /// `BuildCompletion` (`BuildCompletionNames`).
    pub enum BuildCompletion {
        Invalid = "INVALID",
        AppearsAtRallyPoint = "APPEARS_AT_RALLY_POINT",
        PlacedByPlayer = "PLACED_BY_PLAYER",
    }
}

name_enum! {
    /// `Geometry` (`GeometryNames`).
    pub enum GeometryType {
        Sphere = "SPHERE",
        Cylinder = "CYLINDER",
        Box = "BOX",
    }
}

name_enum! {
    /// `Shadow` flags (`TheShadowNames`).
    pub enum ShadowType {
        ShadowDecal = "SHADOW_DECAL",
        ShadowVolume = "SHADOW_VOLUME",
        ShadowProjection = "SHADOW_PROJECTION",
        ShadowDynamicProjection = "SHADOW_DYNAMIC_PROJECTION",
        ShadowDirectionalProjection = "SHADOW_DIRECTIONAL_PROJECTION",
        ShadowAlphaDecal = "SHADOW_ALPHA_DECAL",
        ShadowAdditiveDecal = "SHADOW_ADDITIVE_DECAL",
    }
}

name_enum! {
    /// Upgrade scope (`TheUpgradeTypeNames`, `Common/System/Upgrade.cpp`).
    pub enum UpgradeType {
        Player = "PLAYER",
        Object = "OBJECT",
    }
}

name_enum! {
    /// `TheAcademyClassificationTypeNames` (`Core/GameEngine/Source/Common/RTS/AcademyStats.cpp`).
    pub enum AcademyClassification {
        None = "ACT_NONE",
        UpgradeRadar = "ACT_UPGRADE_RADAR",
        Superpower = "ACT_SUPERPOWER",
    }
}

name_enum! {
    /// Veterancy levels (`TheVeterancyNames`, `Core/GameEngine/Source/Common/System/GameCommon.cpp`).
    pub enum VeterancyLevel {
        Regular = "REGULAR",
        Veteran = "VETERAN",
        Elite = "ELITE",
        Heroic = "HEROIC",
    }
}

name_enum! {
    /// Slow-death phases (`TheSlowDeathPhaseNames`, `GameLogic/Module/SlowDeathBehavior.h`).
    pub enum SlowDeathPhase {
        Initial = "INITIAL",
        Midpoint = "MIDPOINT",
        Final = "FINAL",
    }
}

name_enum! {
    /// `DisabledMaskType::s_bitNameList` (`Common/System/DisabledTypes.cpp`); index 0 is `DEFAULT`.
    pub enum DisabledType {
        Default = "DEFAULT",
        Hacked = "DISABLED_HACKED",
        Emp = "DISABLED_EMP",
        Held = "DISABLED_HELD",
        Paralyzed = "DISABLED_PARALYZED",
        Unmanned = "DISABLED_UNMANNED",
        Underpowered = "DISABLED_UNDERPOWERED",
        Freefall = "DISABLED_FREEFALL",
        Awestruck = "DISABLED_AWESTRUCK",
        Brainwashed = "DISABLED_BRAINWASHED",
        Subdued = "DISABLED_SUBDUED",
        ScriptDisabled = "DISABLED_SCRIPT_DISABLED",
        ScriptUnderpowered = "DISABLED_SCRIPT_UNDERPOWERED",
    }
}

name_enum! {
    /// `TheMaxHealthChangeTypeNames` (`GameLogic/Module/BodyModule.h`).
    pub enum MaxHealthChangeType {
        SameCurrentHealth = "SAME_CURRENTHEALTH",
        PreserveRatio = "PRESERVE_RATIO",
        AddCurrentHealthToo = "ADD_CURRENT_HEALTH_TOO",
        FullyHeal = "FULLY_HEAL",
    }
}

name_enum! {
    /// `DebrisDispositionNames` (`ObjectCreationList.cpp`), parsed with `parseBitString32`.
    pub enum DebrisDisposition {
        LikeExisting = "LIKE_EXISTING",
        OnGroundAligned = "ON_GROUND_ALIGNED",
        SendItFlying = "SEND_IT_FLYING",
        SendItUp = "SEND_IT_UP",
        SendItOut = "SEND_IT_OUT",
        RandomForce = "RANDOM_FORCE",
        Floating = "FLOATING",
        InheritVelocity = "INHERIT_VELOCITY",
        Whirling = "WHIRLING",
    }
}
