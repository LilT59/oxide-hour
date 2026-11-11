//! Logic messages (`GameMessage` / `TheCommandList`), the only way input reaches the sim.
//!
//! `GameLogic::processCommandList` applies messages in submission order at the start of a frame.
//! With feature `possession`, the `Possess*` variants are the possession protocol from
//! docs/POSSESSION.md, implemented in `crate::possession`. The move vector is in world XY (the
//! client converts camera-relative input).

use crate::math::Coord3D;
use crate::object::{ObjectId, PlayerIndex};

/// The payload of `MSG_COMBATDROP_AT_OBJECT` / `MSG_COMBATDROP_AT_LOCATION`
/// (`GameLogicDispatch.cpp:993,1017`): a building to drop into, or a ground position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CombatDropTarget {
    Object(ObjectId),
    Position(Coord3D),
}

/// Possession button bits for [`PossessInput::buttons`].
#[cfg(feature = "possession")]
pub mod buttons {
    pub const FIRE: u8 = 1 << 0;
    pub const ALT_FIRE: u8 = 1 << 1;
    /// Climb / increase altitude (aircraft, hover). Held while Space is down.
    pub const ASCEND: u8 = 1 << 2;
    /// Descend / decrease altitude. Held while Ctrl is down.
    pub const DESCEND: u8 = 1 << 3;
}

/// Per-frame possession input (`MSG_POSSESS_INPUT`). The move vector is quantized to signed
/// 1/32767 units (`-32767..=32767` maps to `-1.0..=1.0`) so replays are byte-exact.
#[cfg(feature = "possession")]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PossessInput {
    pub move_x: i16,
    pub move_y: i16,
    pub aim: Coord3D,
    pub buttons: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// `MSG_DO_MOVETO` for a group.
    MoveTo { ids: Vec<ObjectId>, pos: Coord3D },
    /// `MSG_DO_ATTACK_OBJECT`: move into range, aim and fire until the target is dead.
    Attack { ids: Vec<ObjectId>, target: ObjectId },
    /// Attack a ground position (`MSG_DO_FORCEATTACKGROUND`): primary weapon only.
    AttackGround { ids: Vec<ObjectId>, pos: Coord3D },
    /// `MSG_DO_FORCE_ATTACK_OBJECT`: attack `target` even if it is an ally / neutral object.
    ForceAttack { ids: Vec<ObjectId>, target: ObjectId },
    /// `MSG_DO_STOP`.
    Stop { ids: Vec<ObjectId> },
    /// `MSG_DO_ATTACKMOVETO`: move to `pos`, auto-acquiring targets on the way (`AIAttackMoveState`).
    AttackMove { ids: Vec<ObjectId>, pos: Coord3D },
    /// `MSG_DO_GUARD_*`: hold `target`, engaging enemies within the guard range (`AIGuardState`).
    Guard { ids: Vec<ObjectId>, target: crate::orders::GuardTarget, mode: crate::orders::GuardMode },
    /// `MSG_SELL`: refund part of a structure's cost and remove it (`BuildAssistant::sellObject`).
    Sell { building: ObjectId },
    /// `MSG_DO_ENTER`: walk `ids` to `container` and board it (`AIEnter`).
    ///
    /// Retail sends this same message for the enter-type abilities too
    /// (`CommandXlat::createEnterMessage`): a Hijacker's enemy vehicle, a Terrorist's car bomb and
    /// a Saboteur's building have no contain module — they reach their effect through the collide
    /// modules. See `crate::enter_actions`.
    Enter { ids: Vec<ObjectId>, container: ObjectId },
    /// `MSG_DO_EXIT`: `ids` leave `container` (`AIExit`).
    Exit { container: ObjectId, ids: Vec<ObjectId> },
    /// `MSG_DO_EVACUATE`: every passenger leaves `container`.
    Evacuate { container: ObjectId },
    /// `MSG_QUEUE_UNIT_CREATE`: queue `template` (a `ThingTemplate` name) at `factory`.
    QueueUnit { factory: ObjectId, template: String },
    /// `MSG_QUEUE_UPGRADE`: research `upgrade` at `building` (a `ProductionUpdate`).
    QueueUpgrade { building: ObjectId, upgrade: String },
    /// `MSG_CANCEL_UNIT_CREATE`: cancel the queued item at `index` (refunding its cost).
    CancelUnit { factory: ObjectId, index: usize },
    /// `MSG_SET_RALLY_POINT`: the produced units walk here after exiting `factory`.
    SetRallyPoint { factory: ObjectId, pos: Coord3D },
    /// `MSG_DOZER_CONSTRUCT`: `dozer` builds `template` (a `ThingTemplate` name) at `pos`, facing
    /// `angle` (radians). Cost is withdrawn at placement; the building starts under construction.
    Construct { dozer: ObjectId, template: String, pos: Coord3D, angle: f32 },
    /// `MSG_DOZER_CANCEL`: cancel `dozer`'s current construction, refunding the cost.
    CancelConstruct { dozer: ObjectId },
    /// `MSG_DO_REPAIR`: `dozer` repairs `target`. Gated by `ActionManager::canRepairObject`, so a
    /// structure only (bridges are rejected as in retail; the low-level `command_repair` keeps the
    /// bridge mechanics for scripts/tests).
    Repair { dozer: ObjectId, target: ObjectId },
    /// `MSG_RESUME_CONSTRUCTION`: `dozer` resumes the unfinished `target`
    /// (`DozerAIUpdate::privateResumeConstruction`, `ActionManager::canResumeConstructionOf`).
    ResumeConstruction { dozer: ObjectId, target: ObjectId },
    /// `MSG_DOCK`: `ids` dock with `target`, a supply warehouse or own supply center
    /// (`AIGroup::groupDock` -> `AIUpdateInterface::privateDock`). The client only issues this for
    /// a legal dock (`ActionManager::canDockAt`), so the sim does not re-check it, as in the
    /// engine's `privateDock`.
    Dock { ids: Vec<ObjectId>, target: ObjectId },
    /// `MSG_GET_REPAIRED`: `ids` dock at `target` (a `KINDOF_REPAIR_PAD`, or a
    /// `KINDOF_FS_AIRFIELD` for aircraft) and get healed (`AIUpdateInterface::privateGetRepaired`,
    /// gated by `ActionManager::canGetRepairedAt`).
    GetRepaired { ids: Vec<ObjectId>, target: ObjectId },
    /// `MSG_GET_HEALED`: `ids` enter `target` (a `KINDOF_HEAL_PAD`) to be healed
    /// (`AIUpdateInterface::privateGetHealed`, gated by `ActionManager::canGetHealedAt`).
    GetHealed { ids: Vec<ObjectId>, target: ObjectId },
    /// `MSG_POSSESS`: take control of an owned, living, non-structure unit.
    #[cfg(feature = "possession")]
    Possess { id: ObjectId },
    /// `MSG_POSSESS_INPUT`: this frame's input for the issuing player's possessed unit.
    #[cfg(feature = "possession")]
    PossessInput(PossessInput),
    /// `MSG_RELEASE_POSSESSION`: hand the unit back to the AI.
    #[cfg(feature = "possession")]
    Release,
    /// `MSG_DO_SPECIAL_POWER`: fire `power` from `source` at `target`.
    DoSpecialPower { source: ObjectId, power: String, target: crate::special_power::SpecialPowerTarget },
    /// `MSG_PURCHASE_SCIENCE`: spend science purchase points on `science`.
    PurchaseScience { science: String },
    /// `MSG_DO_FORCEMOVETO`: the engine dispatches it exactly like `MSG_DO_MOVETO`
    /// (`GameLogicDispatch.cpp:1302` `onDoForcemoveto` calls `groupMoveToPosition(&dest, false, ...)`,
    /// identical to `onDoMoveto`). Kept as its own message for the client's force-move mode, which
    /// skips context commands (enter / dock) so the click is always a plain move.
    ForceMove { ids: Vec<ObjectId>, pos: Coord3D },
    /// `MSG_ADD_WAYPOINT` (shift-queued): append `pos` to the group's move path
    /// (`AIGroup::groupMoveToPosition(&dest, addWaypoint = true, ...)` -> `aiFollowPathAppend`,
    /// `AIGroup.cpp:1591` / `AIUpdate.cpp:3358`).
    AddWaypoint { ids: Vec<ObjectId>, pos: Coord3D },
    /// `MSG_DO_SCATTER`: each selected mobile unit moves four bounding radii outward from the group
    /// centre (`AIGroup::groupScatter`, `AIGroup.cpp:1801`).
    Scatter { ids: Vec<ObjectId> },
    /// `MSG_TOGGLE_OVERCHARGE`: toggle the power plant's `OverchargeBehavior`
    /// (`AIGroup::groupToggleOvercharge`, `AIGroup.cpp:2862`).
    ToggleOvercharge { ids: Vec<ObjectId> },
    /// `MSG_SWITCH_WEAPONS`: permanently lock each unit to weapon `slot`
    /// (`AIGroup::setWeaponLockForGroup(slot, LOCKED_PERMANENTLY)`, `AIGroup.cpp:3071`).
    SwitchWeapons { ids: Vec<ObjectId>, slot: usize },
    /// `MSG_CANCEL_UPGRADE`: cancel the queued upgrade named `upgrade` at `building` and refund its
    /// cost (`ProductionUpdate::cancelUpgrade`, `GameLogicDispatch.cpp:1780`).
    CancelUpgrade { building: ObjectId, upgrade: String },
    /// `MSG_COMBATDROP_AT_OBJECT` / `MSG_COMBATDROP_AT_LOCATION`: order a Chinook to drop its
    /// passengers (`AIGroup::groupCombatDrop`, `GameLogicDispatch.cpp:993,1017`).
    CombatDrop { ids: Vec<ObjectId>, target: CombatDropTarget },
    /// `MSG_DO_WEAPON` (the `FIRE_WEAPON` button with no target): lock `slot` temporarily and
    /// attack each unit's own position for at most `max_shots` shots (a negative value is
    /// unlimited, the sim's form of `NO_MAX_SHOTS_LIMIT`). `GameLogicDispatch.cpp:979`,
    /// `AIGroup::groupAttackPosition(nullptr, ...)`.
    DoWeapon { ids: Vec<ObjectId>, slot: usize, max_shots: i32 },
    /// `MSG_DO_WEAPON_AT_OBJECT`: lock `slot` and attack `target` for at most `max_shots` shots
    /// (`GameLogicDispatch.cpp:1038`, `AIGroup::groupAttackObject`).
    DoWeaponAtObject { ids: Vec<ObjectId>, slot: usize, target: ObjectId, max_shots: i32 },
    /// `MSG_DO_WEAPON_AT_LOCATION`: lock `slot` and attack the ground at `pos` for at most
    /// `max_shots` shots (`GameLogicDispatch.cpp:1111`, `AIGroup::groupAttackPosition`).
    DoWeaponAtLocation { ids: Vec<ObjectId>, slot: usize, pos: Coord3D, max_shots: i32 },
    /// `MSG_INTERNET_HACK` (the `HACK_INTERNET` button): `ids` start `HackInternetAIUpdate`
    /// (`GameLogicDispatch.cpp:1559`, `AIGroup::groupHackInternet`). Units without the module
    /// have their AI cleared, as `privateHackInternet` does before it looks the module up.
    HackInternet { ids: Vec<ObjectId> },
    /// `MSG_SET_MINE_CLEARING_DETAIL`: set `WEAPONSET_MINE_CLEARING_DETAIL` on each unit, which
    /// re-selects its weapon set (`GameLogicDispatch.cpp:1072`, `AIGroup::setMineClearingDetail`).
    /// Retail takes no argument and only ever sets the flag.
    SetMineClearingDetail { ids: Vec<ObjectId> },
    /// `MSG_ENABLE_RETALIATION_MODE`: the issuing player turns its own logical retaliation mode on or off
    /// (`GameLogicDispatch.cpp:1082-1108`). Retail carries the player index as argument 0; the maintained
    /// engine's fix reads the message's player instead, which is what the sim does.
    EnableRetaliationMode { enabled: bool },
    /// `MSG_DO_SPECIAL_POWER_AT_LOCATION` with its angle argument: fire `power` from `source` at
    /// `pos`, facing `angle` (radians, `None` = `INVALID_ANGLE`). The angle reaches the power's
    /// `ObjectCreationList` (`OCLSpecialPower::doSpecialPowerAtLocation`, `OCLSpecialPower.cpp:149`),
    /// where an object with `Disposition = LIKE_EXISTING` takes it as its orientation. Retail's
    /// `SPECIAL_POWER_CONSTRUCT` placement sends the ghost's angle here
    /// (`PlaceEventTranslator.cpp:252`); other powers send `None`. `DoSpecialPower` with a
    /// position is the same message with `None`.
    DoSpecialPowerAtLocation { source: ObjectId, power: String, pos: Coord3D, angle: Option<f32> },
}

/// A command plus the issuing player (`GameMessage::getPlayerIndex`) and the engine's command source
/// (`CommandSourceType`, which the dispatch functions pass on, e.g. `onDoMoveto`'s `CMD_FROM_PLAYER` or a
/// script action's `CMD_FROM_SCRIPT`).
#[derive(Clone, Debug, PartialEq)]
pub struct GameMessage {
    pub player: PlayerIndex,
    pub command: Command,
    /// `CommandSourceType` of the order.
    pub source: crate::ai_states::CommandSource,
}
