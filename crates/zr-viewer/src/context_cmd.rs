//! Right-click context commands: a port of `CommandTranslator::evaluateContextCommand` and the
//! cursor it selects.
//!
//! `CommandXlat::evaluateContextCommand` (`Core/GameEngine/Source/GameClient/MessageStream/CommandXlat.cpp:2374`)
//! decides what a right-click on the world does, in a fixed priority order, using the
//! `ActionManager` queries. This module ports that decision tree (the pure [`evaluate`] function),
//! maps each outcome to the retail mouse cursor (`InGameUI::createCommandHint`,
//! `GeneralsMD/.../GameClient/InGameUI.cpp:2854`) and submits the matching [`Command`]s. The
//! per-action gates themselves already live in `zr-sim` (`enter_actions`, `dozer`, and the
//! `ActionManager` ports); this module only picks between them and chooses which selected units get
//! the order.
//!
//! The retail order (`CommandXlat.cpp:2430-2526`), all skipped in force-attack mode except the
//! attack test itself:
//!
//! 1. resume construction (`ACTIONTYPE_RESUME_CONSTRUCTION`, `SELECTION_ANY`)
//! 2. dock at (`ACTIONTYPE_DOCK_AT`, `SELECTION_ALL`)
//! 3. repair object (`ACTIONTYPE_REPAIR_OBJECT`, `SELECTION_ANY`)
//! 4. get repaired at (`ACTIONTYPE_GET_REPAIRED_AT`, `SELECTION_ANY`)
//! 5. get healed at (`ACTIONTYPE_GET_HEALED_AT`, `SELECTION_ANY`)
//! 6. hijack vehicle (`ACTIONTYPE_HIJACK_VEHICLE`, `SELECTION_ANY`)
//! 7. convert object to car bomb (`ACTIONTYPE_CONVERT_OBJECT_TO_CARBOMB`, `SELECTION_ANY`)
//! 8. sabotage building (`ACTIONTYPE_SABOTAGE_BUILDING`, `SELECTION_ANY`)
//! 9. salvage (`canSelectionSalvage`; not modelled by the sim)
//! 10. enter object (`ACTIONTYPE_ENTER_OBJECT`, `SELECTION_ANY`, `CHECK_CAPACITY`)
//! 11. attack object (`getCanSelectedObjectsAttack`, `SELECTION_ANY`)
//! 12. capture building (`ACTIONTYPE_CAPTURE_BUILDING`, `SELECTION_ANY`)
//! 13. Black Lotus hacks (not modelled)
//! 14. set rally point (no object under the cursor)
//! 15. invalid shot
//! 16. default move
//!
//! `SELECTION_ANY` means the action is offered if *any* selected object can do it; only those
//! qualifying objects get the order (the engine sends one message for the selection team and each
//! object's `private*` re-checks). `SELECTION_ALL` requires every selected object to qualify.
//!
//! `handleDefaultMoveCommand` (`CommandXlat.cpp:2326`) only issues a move when the cursor is not
//! over an object (`draw == nullptr`); right-clicking an own object with no specific action does
//! nothing (it just shows the select cursor). A locally controlled mine is treated as a move
//! location (`resolveContextTarget`, `:1561`), which [`evaluate`] mirrors.
//!
//! Not ported (the sim has no data for them): stealth/masked `resolveContextTarget` handling,
//! `canSelectionSalvage`, the Black Lotus hacking actions, waypoint mode, the shrubbery exception
//! and the `KINDOF_IGNORED_IN_GUI`/`KINDOF_MOB_NEXUS` special cases. The cursor art itself lives in
//! [`crate::cursor`]: `update_cursor` only picks the [`CursorKind`] and asks for the matching
//! `Mouse.ini` cursor, which is applied as a Bevy custom cursor (and animated from the `.ANI`).

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use zr_game::Game;
use zr_sim::damage::Relationship;
use zr_sim::template::kind_of;
use zr_sim::{Command, Coord3D, GameLogic, ObjectId, PlayerIndex};

use crate::camera::{FreeCam, Interactive};
use crate::cursor::{CursorRequest, MouseCursorName};
use crate::sim_view::{ray_ground, ray_sphere, SimRes};
use crate::MapRes;

/// The mouse cursor chosen for a context action (`InGameUI::createCommandHint`'s `Mouse::*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CursorKind {
    #[default]
    Arrow,
    MoveTo,
    Waypoint,
    Selecting,
    AttackObject,
    Outrange,
    ForceAttackObject,
    ForceAttackGround,
    GetRepaired,
    Dock,
    GetHealed,
    DoRepair,
    ResumeConstruction,
    EnterFriendly,
    EnterAggressively,
    CaptureBuilding,
    GenericInvalid,
    SetRallyPoint,
}

impl CursorKind {
    /// The `Mouse::*` name from the retail source, for the on-screen debug label.
    pub fn label(self) -> &'static str {
        match self {
            CursorKind::Arrow => "ARROW",
            CursorKind::MoveTo => "MOVETO",
            CursorKind::Waypoint => "WAYPOINT",
            CursorKind::Selecting => "SELECTING",
            CursorKind::AttackObject => "ATTACK",
            CursorKind::Outrange => "OUTRANGE",
            CursorKind::ForceAttackObject => "FORCE ATTACK",
            CursorKind::ForceAttackGround => "FORCE GROUND",
            CursorKind::GetRepaired => "GET REPAIRED",
            CursorKind::Dock => "DOCK",
            CursorKind::GetHealed => "GET HEALED",
            CursorKind::DoRepair => "REPAIR",
            CursorKind::ResumeConstruction => "RESUME",
            CursorKind::EnterFriendly => "ENTER",
            CursorKind::EnterAggressively => "ENTER!",
            CursorKind::CaptureBuilding => "CAPTURE",
            CursorKind::SetRallyPoint => "SET RALLY",
            CursorKind::GenericInvalid => "INVALID",
        }
    }

    /// The retail `MouseCursor` name this kind maps to (`InGameUI::createCommandHint`'s
    /// `setMouseCursor(Mouse::*)` calls, `InGameUI.cpp:2947`), for the loaded `Mouse.ini` art.
    pub fn mouse(self) -> crate::cursor::MouseCursorName {
        use crate::cursor::MouseCursorName as M;
        match self {
            CursorKind::Arrow => M::Arrow,
            CursorKind::MoveTo => M::Move,
            CursorKind::Waypoint => M::Waypoint,
            CursorKind::Selecting => M::Select,
            CursorKind::AttackObject => M::AttackObj,
            CursorKind::Outrange => M::OutRange,
            CursorKind::ForceAttackObject => M::ForceAttackObj,
            CursorKind::ForceAttackGround => M::ForceAttackGround,
            CursorKind::GetRepaired => M::GetRepaired,
            CursorKind::Dock => M::Dock,
            CursorKind::GetHealed => M::GetHealed,
            CursorKind::DoRepair => M::DoRepair,
            CursorKind::ResumeConstruction => M::ResumeConstruction,
            CursorKind::EnterFriendly => M::EnterFriendly,
            CursorKind::EnterAggressively => M::EnterAggressive,
            CursorKind::CaptureBuilding => M::CaptureBuilding,
            CursorKind::SetRallyPoint => M::SetRallyPoint,
            CursorKind::GenericInvalid => M::GenericInvalid,
        }
    }
}

/// `ActionManager::getCanAttackObject`'s result (`WeaponSet.h:184`). Ordered worst to best so
/// `SELECTION_ANY` can take the max, like `InGameUI::getCanSelectedObjectsAttack`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AttackResult {
    NotPossible,
    /// "Not a clear shot" (`WeaponSet.h:188`). The sim does not model line-of-fire, so
    /// [`GameLogic`]'s impl never returns it; the evaluator still handles it faithfully.
    #[allow(dead_code)]
    InvalidShot,
    PossibleAfterMoving,
    Possible,
}

/// The command a context evaluation resolves to.
#[derive(Clone, Debug, PartialEq)]
pub enum ContextCommand {
    /// `MSG_RESUME_CONSTRUCTION`; the sim takes one dozer per command, so `ids` is the qualifying
    /// dozer subset.
    ResumeConstruction { ids: Vec<ObjectId>, target: ObjectId },
    /// `MSG_DOCK` (`handleDockAtCommand`, `CommandXlat.cpp:1857`).
    Dock { ids: Vec<ObjectId>, target: ObjectId },
    /// `MSG_DO_REPAIR` (`handleRepairObjectCommand`, `:1894`).
    Repair { ids: Vec<ObjectId>, target: ObjectId },
    /// `MSG_GET_REPAIRED` (`handleGetRepairedAtCommand`, `:1927`).
    GetRepaired { ids: Vec<ObjectId>, target: ObjectId },
    /// `MSG_GET_HEALED` (`handleGetHealedAtCommand`, `:1962`).
    GetHealed { ids: Vec<ObjectId>, target: ObjectId },
    /// `MSG_ENTER` (`createEnterMessage`, `:1408`). `aggressive` is true for the
    /// hijack/car-bomb/sabotage variants, whose cursor is `Mouse::ENTER_AGGRESSIVELY`.
    Enter { ids: Vec<ObjectId>, target: ObjectId, aggressive: bool },
    /// `MSG_SET_RALLY_POINT` (`handleSetRallyPointCommand`, `CommandXlat.cpp:2240`): one message per
    /// selected object, each with the ground position.
    SetRallyPoint { ids: Vec<ObjectId>, pos: Coord3D },
    /// `MSG_DO_ATTACK_OBJECT` / `MSG_DO_FORCE_ATTACK_OBJECT`.
    Attack { ids: Vec<ObjectId>, target: ObjectId, force: bool },
    /// `MSG_DO_SPECIAL_POWER_AT_OBJECT` for the capture ability (`handleCaptureBuildingCommand`).
    CaptureBuilding { source: ObjectId, target: ObjectId, power: String },
    /// `MSG_DO_MOVETO`.
    MoveTo { ids: Vec<ObjectId>, pos: Coord3D },
    /// `MSG_DO_FORCEMOVETO` (`evaluateContextCommand`'s force-move mode drops the object and the
    /// move carries the force flag; `CommandXlat.cpp:1010`).
    ForceMove { ids: Vec<ObjectId>, pos: Coord3D },
    /// `MSG_ADD_WAYPOINT` (`handleWaypointModeCommand`, `CommandXlat.cpp:1636`).
    AddWaypoint { ids: Vec<ObjectId>, pos: Coord3D },
    /// `MSG_DO_FORCE_ATTACK_GROUND` (`evaluateForceAttack`).
    AttackGround { ids: Vec<ObjectId>, pos: Coord3D },
}

/// The meta modes a right-click evaluates under (`InGameUI`'s force-attack / force-move / waypoint
/// / prefer-selection flags), set by `meta_keys`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ContextModes {
    pub force_attack: bool,
    pub force_move: bool,
    pub waypoints: bool,
    pub prefer_selection: bool,
}

/// The outcome of a context evaluation: the command (if any) and the cursor to show.
#[derive(Clone, Debug, PartialEq)]
pub struct ContextEval {
    pub command: Option<ContextCommand>,
    pub cursor: CursorKind,
}

/// The `ActionManager` queries the evaluator needs. Implemented for [`GameLogic`], and by a stub in
/// the tests so the decision tree and the `SELECTION_ANY` / `SELECTION_ALL` rules can be exercised
/// without a simulation. Defaults return `false` (no action).
pub trait ContextChecks {
    /// `ActionManager::canResumeConstructionOf`.
    fn can_resume_construction(&self, _unit: ObjectId, _target: ObjectId) -> bool {
        false
    }
    /// `ActionManager::canDockAt`.
    fn can_dock_at(&self, _unit: ObjectId, _target: ObjectId) -> bool {
        false
    }
    /// `ActionManager::canRepairObject`.
    fn can_repair_object(&self, _unit: ObjectId, _target: ObjectId) -> bool {
        false
    }
    /// `ActionManager::canGetRepairedAt`.
    fn can_get_repaired_at(&self, _unit: ObjectId, _target: ObjectId) -> bool {
        false
    }
    /// `ActionManager::canGetHealedAt`.
    fn can_get_healed_at(&self, _unit: ObjectId, _target: ObjectId) -> bool {
        false
    }
    /// `ActionManager::canHijackVehicle`.
    fn can_hijack_vehicle(&self, _unit: ObjectId, _target: ObjectId) -> bool {
        false
    }
    /// `ActionManager::canConvertObjectToCarBomb`.
    fn can_convert_object_to_carbomb(&self, _unit: ObjectId, _target: ObjectId) -> bool {
        false
    }
    /// `ActionManager::canSabotageBuilding`.
    fn can_sabotage_building(&self, _unit: ObjectId, _target: ObjectId) -> bool {
        false
    }
    /// `ActionManager::canEnterObject(..., CHECK_CAPACITY)`.
    fn can_enter_object(&self, _unit: ObjectId, _target: ObjectId) -> bool {
        false
    }
    /// `ActionManager::canCaptureBuilding`.
    fn can_capture_building(&self, _unit: ObjectId, _target: ObjectId) -> bool {
        false
    }
    /// `GameLogic::capture_power_for`: the capture ability's `SpecialPowerTemplate` name.
    fn capture_power_for(&self, _unit: ObjectId) -> Option<String> {
        None
    }
    /// `ActionManager::getCanAttackObject`.
    fn attack_result(&self, _unit: ObjectId, _target: ObjectId, _force: bool) -> AttackResult {
        AttackResult::NotPossible
    }
    /// `CanSelectDrawable(draw, FALSE) && obj->isLocallyControlled()` for the move hint.
    fn is_selectable_own(&self, _id: ObjectId, _local: PlayerIndex) -> bool {
        false
    }
    /// `obj->isKindOf(KINDOF_STRUCTURE) && obj->isLocallyControlled()`.
    fn is_own_structure(&self, _id: ObjectId, _local: PlayerIndex) -> bool {
        false
    }
    /// `obj->isLocallyControlled() && obj->isKindOf(KINDOF_MINE)`.
    fn is_own_mine(&self, _id: ObjectId, _local: PlayerIndex) -> bool {
        false
    }
    /// `obj->isKindOf(KINDOF_AUTO_RALLYPOINT) && obj->isLocallyControlled()` (`InGameUI.cpp:4776`).
    fn is_rally_capable(&self, _id: ObjectId, _local: PlayerIndex) -> bool {
        false
    }
}

/// `CommandTranslator::evaluateContextCommand` (`CommandXlat.cpp:2374`): the first action the
/// selected objects can perform on `target`, in retail priority order. `target` is the object
/// under the cursor (`None` = terrain); `ground` is the terrain point. `force_attack` is the Ctrl
/// modifier (`evaluateForceAttack`), `force_move` the force-move mode, `waypoints` the waypoint
/// mode (which overrides every action) and `prefer_selection` the held-Shift mode that makes an own
/// object non-actionable.
#[allow(clippy::too_many_arguments)]
pub fn evaluate<C: ContextChecks + ?Sized>(
    checks: &C,
    local: PlayerIndex,
    selected: &[ObjectId],
    target: Option<ObjectId>,
    ground: Option<Coord3D>,
    force_attack: bool,
    force_move: bool,
    waypoints: bool,
    prefer_selection: bool,
) -> ContextEval {
    // `resolveContextTarget`: force-move and a locally controlled mine both drop the object and
    // evaluate the position instead (`CommandXlat.cpp:1561-1573`).
    let target = if force_move {
        None
    } else {
        target.filter(|&t| force_attack || !checks.is_own_mine(t, local))
    };

    // `evaluateContextCommand`'s prefer-selection early-out: an own object is not actioned.
    if prefer_selection && target.is_some_and(|t| checks.is_selectable_own(t, local)) {
        return ContextEval { command: None, cursor: CursorKind::Selecting };
    }

    // Waypoint mode overrides every other command (`handleWaypointModeCommand`, `:2407`).
    if waypoints {
        return ContextEval {
            command: ground.map(|pos| ContextCommand::AddWaypoint { ids: selected.to_vec(), pos }),
            cursor: CursorKind::Waypoint,
        };
    }

    // No object under the cursor: force attack the ground, else move there.
    let Some(t) = target else {
        if force_attack {
            return ContextEval {
                command: ground.map(|pos| ContextCommand::AttackGround { ids: selected.to_vec(), pos }),
                cursor: CursorKind::ForceAttackGround,
            };
        }
        // 14. Set rally point (`ACTIONTYPE_SET_RALLY_POINT`, `SELECTION_ALL`, no object under the cursor;
        // `CommandXlat.cpp:2515`, which runs before the default move).
        if !force_move && !selected.is_empty() && selected.iter().all(|&u| checks.is_rally_capable(u, local)) {
            return ContextEval { command: ground.map(|pos| ContextCommand::SetRallyPoint { ids: selected.to_vec(), pos }), cursor: CursorKind::SetRallyPoint };
        }
        let command = ground.map(|pos| {
            if force_move {
                ContextCommand::ForceMove { ids: selected.to_vec(), pos }
            } else {
                ContextCommand::MoveTo { ids: selected.to_vec(), pos }
            }
        });
        return ContextEval { command, cursor: CursorKind::MoveTo };
    };

    if !force_attack {
        // 1. Resume construction (`SELECTION_ANY`).
        let ids: Vec<ObjectId> = selected.iter().copied().filter(|&u| checks.can_resume_construction(u, t)).collect();
        if !ids.is_empty() {
            return ContextEval { command: Some(ContextCommand::ResumeConstruction { ids, target: t }), cursor: CursorKind::ResumeConstruction };
        }
        // 2. Dock (`SELECTION_ALL`).
        if !selected.is_empty() && selected.iter().all(|&u| checks.can_dock_at(u, t)) {
            return ContextEval { command: Some(ContextCommand::Dock { ids: selected.to_vec(), target: t }), cursor: CursorKind::Dock };
        }
        // 3. Repair (`SELECTION_ANY`). The engine also requires the target to have no other healer
        //    (`canSelectedObjectsDoAction`'s `getSoleHealingBenefactor`); the sim does not track a
        //    healing benefactor, so that extra test is skipped (the sim's `Command::Repair` runs the
        //    faithful check again).
        let ids: Vec<ObjectId> = selected.iter().copied().filter(|&u| checks.can_repair_object(u, t)).collect();
        if !ids.is_empty() {
            return ContextEval { command: Some(ContextCommand::Repair { ids, target: t }), cursor: CursorKind::DoRepair };
        }
        // 4. Get repaired (`SELECTION_ANY`).
        let ids: Vec<ObjectId> = selected.iter().copied().filter(|&u| checks.can_get_repaired_at(u, t)).collect();
        if !ids.is_empty() {
            return ContextEval { command: Some(ContextCommand::GetRepaired { ids, target: t }), cursor: CursorKind::GetRepaired };
        }
        // 5. Get healed (`SELECTION_ANY`).
        let ids: Vec<ObjectId> = selected.iter().copied().filter(|&u| checks.can_get_healed_at(u, t)).collect();
        if !ids.is_empty() {
            return ContextEval { command: Some(ContextCommand::GetHealed { ids, target: t }), cursor: CursorKind::GetHealed };
        }
        // 6. Hijack (`SELECTION_ANY`), an aggressive enter.
        let ids: Vec<ObjectId> = selected.iter().copied().filter(|&u| checks.can_hijack_vehicle(u, t)).collect();
        if !ids.is_empty() {
            return ContextEval { command: Some(ContextCommand::Enter { ids, target: t, aggressive: true }), cursor: CursorKind::EnterAggressively };
        }
        // 7. Convert to car bomb.
        let ids: Vec<ObjectId> = selected.iter().copied().filter(|&u| checks.can_convert_object_to_carbomb(u, t)).collect();
        if !ids.is_empty() {
            return ContextEval { command: Some(ContextCommand::Enter { ids, target: t, aggressive: true }), cursor: CursorKind::EnterAggressively };
        }
        // 8. Sabotage.
        let ids: Vec<ObjectId> = selected.iter().copied().filter(|&u| checks.can_sabotage_building(u, t)).collect();
        if !ids.is_empty() {
            return ContextEval { command: Some(ContextCommand::Enter { ids, target: t, aggressive: true }), cursor: CursorKind::EnterAggressively };
        }
        // 10. Enter object (`CHECK_CAPACITY`).
        let ids: Vec<ObjectId> = selected.iter().copied().filter(|&u| checks.can_enter_object(u, t)).collect();
        if !ids.is_empty() {
            return ContextEval { command: Some(ContextCommand::Enter { ids, target: t, aggressive: false }), cursor: CursorKind::EnterFriendly };
        }
    }

    // 11. Attack (`SELECTION_ANY`): the best result over the selection.
    let best = selected.iter().map(|&u| checks.attack_result(u, t, force_attack)).max().unwrap_or(AttackResult::NotPossible);
    match best {
        AttackResult::Possible | AttackResult::PossibleAfterMoving => {
            let cursor = if force_attack {
                CursorKind::ForceAttackObject
            } else if best == AttackResult::Possible {
                CursorKind::AttackObject
            } else {
                CursorKind::Outrange
            };
            return ContextEval { command: Some(ContextCommand::Attack { ids: selected.to_vec(), target: t, force: force_attack }), cursor };
        }
        AttackResult::InvalidShot => {
            // `handleInvalidShotCommand` (`CommandXlat.cpp:2519`): no command, invalid cursor.
            return ContextEval { command: None, cursor: CursorKind::GenericInvalid };
        }
        AttackResult::NotPossible => {}
    }

    if !force_attack {
        // 12. Capture building (`SELECTION_ANY`): issued as a special power from the first unit
        //     whose command set has the capture ability.
        if let Some(&source) = selected.iter().find(|&&u| checks.can_capture_building(u, t)) {
            if let Some(power) = checks.capture_power_for(source) {
                return ContextEval { command: Some(ContextCommand::CaptureBuilding { source, target: t, power }), cursor: CursorKind::CaptureBuilding };
            }
        }
    }

    // 16. Default move. `handleDefaultMoveCommand` only issues a move when there is no object under
    //     the cursor; otherwise the click does nothing (the cursor is still hinted).
    let cursor = if checks.is_selectable_own(t, local) {
        CursorKind::Selecting
    } else if checks.is_own_structure(t, local) {
        CursorKind::GenericInvalid
    } else {
        CursorKind::MoveTo
    };
    ContextEval { command: None, cursor }
}

impl ContextChecks for GameLogic {
    fn can_resume_construction(&self, unit: ObjectId, target: ObjectId) -> bool {
        GameLogic::can_resume_construction(self, unit, target)
    }
    fn can_dock_at(&self, unit: ObjectId, target: ObjectId) -> bool {
        GameLogic::can_dock_at(self, unit, target)
    }
    fn can_repair_object(&self, unit: ObjectId, target: ObjectId) -> bool {
        GameLogic::can_repair_object(self, unit, target)
    }
    fn can_get_repaired_at(&self, unit: ObjectId, target: ObjectId) -> bool {
        GameLogic::can_get_repaired_at(self, unit, target)
    }
    fn can_get_healed_at(&self, unit: ObjectId, target: ObjectId) -> bool {
        // `canSelectedObjectsDoAction`'s extra test: a target that is a `HealContain` (a heal-only
        // container, "NOT a transport") is rejected. The sim has no `HealContain` kind, so the
        // `ActionManager::canGetHealedAt` result stands.
        GameLogic::can_get_healed_at(self, unit, target)
    }
    fn can_hijack_vehicle(&self, unit: ObjectId, target: ObjectId) -> bool {
        GameLogic::can_hijack_vehicle(self, unit, target)
    }
    fn can_convert_object_to_carbomb(&self, unit: ObjectId, target: ObjectId) -> bool {
        GameLogic::can_convert_object_to_carbomb(self, unit, target)
    }
    fn can_sabotage_building(&self, unit: ObjectId, target: ObjectId) -> bool {
        GameLogic::can_sabotage_building(self, unit, target)
    }
    fn can_enter_object(&self, unit: ObjectId, target: ObjectId) -> bool {
        // `ACTIONTYPE_ENTER_OBJECT` passes `additionalChecking = true` -> `CHECK_CAPACITY`.
        GameLogic::can_enter_object(self, unit, target, true)
    }
    fn can_capture_building(&self, unit: ObjectId, target: ObjectId) -> bool {
        GameLogic::can_capture_building(self, unit, target)
    }
    fn capture_power_for(&self, unit: ObjectId) -> Option<String> {
        GameLogic::capture_power_for(self, unit)
    }
    fn attack_result(&self, unit: ObjectId, target: ObjectId, force: bool) -> AttackResult {
        attack_result_in_slot(self, unit, target, force, 0)
    }
    fn is_selectable_own(&self, id: ObjectId, local: PlayerIndex) -> bool {
        self.find_object(id).is_some_and(|o| o.player == local && !o.is_destroyed() && o.is_kind_of(kind_of::SELECTABLE))
    }
    fn is_own_structure(&self, id: ObjectId, local: PlayerIndex) -> bool {
        self.find_object(id).is_some_and(|o| o.player == local && o.is_kind_of(kind_of::STRUCTURE))
    }
    fn is_own_mine(&self, id: ObjectId, local: PlayerIndex) -> bool {
        self.find_object(id).is_some_and(|o| o.player == local && o.is_kind_of(kind_of::MINE))
    }
    fn is_rally_capable(&self, id: ObjectId, local: PlayerIndex) -> bool {
        self.find_object(id).is_some_and(|o| o.player == local && !o.is_destroyed() && o.template.auto_rally_point)
    }
}

// ---- viewer glue -------------------------------------------------------------------------------

/// The cursor chosen for the frame's hover, for the debug label and the system cursor.
#[derive(Resource, Default)]
pub struct ContextCursor(pub CursorKind);

/// A forced hover target, set by `ZR_CONTEXT_DEMO` so a screenshot shows a context cursor even
/// though screenshot runs are non-interactive. `None` means "pick under the real cursor".
#[derive(Resource, Default)]
pub struct ForcedHover(pub Option<ObjectId>);

#[derive(Resource, Default)]
pub(crate) struct CursorLabel(Option<Entity>);

pub fn add_systems(app: &mut App) {
    app.init_resource::<ContextCursor>();
    app.init_resource::<ForcedHover>();
    app.init_resource::<CursorLabel>();
    app.add_systems(Update, update_cursor.after(crate::sim_view::sim_step).run_if(in_state(crate::shell::ShellState::InGame)));
}

/// `ActionManager::getCanAttackObject` (`WeaponSet.h:184`) for the weapon in `slot`: the attack
/// possibility of `unit` against `target`, range included. `force` skips the relationship test
/// (`getAbleToAttackSpecificObject`'s enemy requirement). [`ContextChecks::attack_result`] is the
/// primary-weapon case.
pub(crate) fn attack_result_in_slot(logic: &GameLogic, unit: ObjectId, target: ObjectId, force: bool, slot: usize) -> AttackResult {
    let (Some(u), Some(t)) = (logic.find_object(unit), logic.find_object(target)) else { return AttackResult::NotPossible };
    if unit == target || t.is_dying() {
        return AttackResult::NotPossible;
    }
    if !logic.is_able_to_attack(unit) {
        return AttackResult::NotPossible;
    }
    if !force && logic.relationship(u.player, t.player) != Relationship::Enemies {
        return AttackResult::NotPossible;
    }
    let Some(w) = u.combat.weapons.weapon(slot) else { return AttackResult::NotPossible };
    let range = w.template.attack_range();
    let dx = u.position.x - t.position.x;
    let dy = u.position.y - t.position.y;
    let dist = (dx * dx + dy * dy).sqrt() - u.template.geometry.bounding_circle_radius() - t.template.geometry.bounding_circle_radius();
    if range <= 0.0 || dist <= range {
        AttackResult::Possible
    } else {
        AttackResult::PossibleAfterMoving
    }
}

/// Nearest object under the ray, ignoring ownership (the context target can be any object).
pub(crate) fn pick_any(sim: &SimRes, o: Vec3, d: Vec3) -> Option<ObjectId> {
    let mut best: Option<(f32, ObjectId)> = None;
    for (id, snap) in &sim.cur {
        if sim.game.object(*id).is_none() {
            continue;
        }
        let r = sim.game.object_radius(*id).unwrap_or(5.0).max(8.0);
        let c = crate::convert::to_bevy(snap.pos) + Vec3::Y * r * 0.5;
        if let Some(t) = ray_sphere(o, d, c, r) {
            if best.is_none_or(|(bt, _)| t < bt) {
                best = Some((t, *id));
            }
        }
    }
    best.map(|(_, id)| id)
}

/// The terrain point under a view ray, as the sim's order position (`TheTacticalView->screenToTerrain`).
pub(crate) fn ground_point(map: &MapRes, o: Vec3, d: Vec3) -> Option<Coord3D> {
    ray_ground(o, d, &map.ground()).map(|p| Coord3D::new(p.x, -p.z, 0.0))
}

/// `CommandTranslator::evaluateContextCommand` for an explicit target and point, without submitting
/// anything. The left click asks this first in regular mouse mode (`contextCommandForNewSelection`).
pub(crate) fn eval_at(sim: &SimRes, target: Option<ObjectId>, ground: Option<Coord3D>, modes: ContextModes) -> ContextEval {
    evaluate(sim.game.logic(), sim.local, &sim.selected, target, ground, modes.force_attack, modes.force_move, modes.waypoints, modes.prefer_selection)
}

/// Submits an evaluated context command with its marker and voice. Returns false (and does nothing)
/// when the evaluation has no command.
pub(crate) fn submit_eval(commands: &mut Commands, sim: &mut SimRes, assets: &crate::rts_input::RtsAssets, audio: &mut crate::audio::AudioQueue, eval: &ContextEval) -> bool {
    if eval.command.is_none() {
        return false;
    }
    apply(commands, sim, assets, audio, eval);
    true
}

/// Right-click order: evaluate the context command and submit it. `modes` are the meta-key modes
/// (force-attack / force-move / waypoint / prefer-selection). Returns the cursor that was chosen.
#[allow(clippy::too_many_arguments)]
pub fn order_context(
    commands: &mut Commands,
    sim: &mut SimRes,
    assets: &crate::rts_input::RtsAssets,
    map: &MapRes,
    audio: &mut crate::audio::AudioQueue,
    o: Vec3,
    d: Vec3,
    modes: ContextModes,
) -> CursorKind {
    let target = pick_any(sim, o, d);
    let eval = eval_at(sim, target, ground_point(map, o, d), modes);
    apply(commands, sim, assets, audio, &eval);
    eval.cursor
}

/// Submits a context command as [`Command`]s. This is the only place a context order writes to the
/// simulation; it is shared by the live right-click and the build-time `ZR_CONTEXT_DEMO` setup.
pub(crate) fn submit(game: &mut Game, local: PlayerIndex, cmd: &ContextCommand) {
    match cmd {
        ContextCommand::MoveTo { ids, pos } => game.submit(local, Command::MoveTo { ids: ids.clone(), pos: *pos }),
        ContextCommand::ForceMove { ids, pos } => game.submit(local, Command::ForceMove { ids: ids.clone(), pos: *pos }),
        ContextCommand::AddWaypoint { ids, pos } => game.submit(local, Command::AddWaypoint { ids: ids.clone(), pos: *pos }),
        ContextCommand::AttackGround { ids, pos } => game.submit(local, Command::AttackGround { ids: ids.clone(), pos: *pos }),
        ContextCommand::Attack { ids, target, force } => {
            if *force {
                game.submit(local, Command::ForceAttack { ids: ids.clone(), target: *target });
            } else {
                game.submit(local, Command::Attack { ids: ids.clone(), target: *target });
            }
        }
        ContextCommand::Enter { ids, target, .. } => game.submit(local, Command::Enter { ids: ids.clone(), container: *target }),
        ContextCommand::Repair { ids, target } => {
            for &dozer in ids {
                game.submit(local, Command::Repair { dozer, target: *target });
            }
        }
        ContextCommand::ResumeConstruction { ids, target } => {
            for &dozer in ids {
                game.submit(local, Command::ResumeConstruction { dozer, target: *target });
            }
        }
        ContextCommand::CaptureBuilding { source, target, power } => {
            game.submit(local, Command::DoSpecialPower { source: *source, power: power.clone(), target: zr_sim::special_power::SpecialPowerTarget::Object(*target) });
        }
        ContextCommand::Dock { ids, target } => game.submit(local, Command::Dock { ids: ids.clone(), target: *target }),
        ContextCommand::GetRepaired { ids, target } => game.submit(local, Command::GetRepaired { ids: ids.clone(), target: *target }),
        ContextCommand::GetHealed { ids, target } => game.submit(local, Command::GetHealed { ids: ids.clone(), target: *target }),
        ContextCommand::SetRallyPoint { ids, pos } => {
            for &factory in ids {
                game.submit(local, Command::SetRallyPoint { factory, pos: *pos });
            }
        }
    }
}

/// Submits the evaluated command, with the order marker and voice feedback (client-side only).
fn apply(commands: &mut Commands, sim: &mut SimRes, assets: &crate::rts_input::RtsAssets, audio: &mut crate::audio::AudioQueue, eval: &ContextEval) {
    let local = sim.local;
    let Some(cmd) = &eval.command else { return };
    // Object orders get a marker at the target; ground orders at the point.
    let target_pos = |sim: &SimRes, id: ObjectId| sim.cur.get(&id).map(|s| s.pos);
    match cmd {
        ContextCommand::MoveTo { ids, pos } => {
            crate::rts_input::spawn_marker(commands, assets, [pos.x, pos.y, pos.z], false);
            crate::rts_input::order_voice(sim, audio, ids, "VoiceMove");
        }
        ContextCommand::ForceMove { ids, pos } | ContextCommand::AddWaypoint { ids, pos } => {
            crate::rts_input::spawn_marker(commands, assets, [pos.x, pos.y, pos.z], false);
            crate::rts_input::order_voice(sim, audio, ids, "VoiceMove");
        }
        ContextCommand::AttackGround { ids, pos } => {
            crate::rts_input::spawn_marker(commands, assets, [pos.x, pos.y, pos.z], true);
            crate::rts_input::order_voice(sim, audio, ids, "VoiceAttack");
        }
        ContextCommand::Attack { ids, target, .. } => {
            if let Some(p) = target_pos(sim, *target) {
                crate::rts_input::spawn_marker(commands, assets, p, true);
            }
            crate::rts_input::order_voice(sim, audio, ids, "VoiceAttack");
        }
        ContextCommand::Enter { ids, target, .. } => {
            if let Some(p) = target_pos(sim, *target) {
                crate::rts_input::spawn_marker(commands, assets, p, eval.cursor == CursorKind::EnterAggressively);
            }
            crate::rts_input::order_voice(sim, audio, ids, "VoiceEnter");
        }
        ContextCommand::Dock { target, .. } | ContextCommand::GetRepaired { target, .. } | ContextCommand::GetHealed { target, .. } => {
            if let Some(p) = target_pos(sim, *target) {
                crate::rts_input::spawn_marker(commands, assets, p, false);
            }
        }
        ContextCommand::SetRallyPoint { pos, .. } => {
            crate::rts_input::spawn_marker(commands, assets, [pos.x, pos.y, pos.z], false);
        }
        ContextCommand::Repair { .. } | ContextCommand::ResumeConstruction { .. } | ContextCommand::CaptureBuilding { .. } => {}
    }
    submit(&mut sim.game, local, cmd);
}

/// Evaluates the context command for an explicit target and submits it. Used by the build-time
/// `ZR_CONTEXT_DEMO` setup (the live right-click uses [`order_context`]).
pub(crate) fn order_target(game: &mut Game, local: PlayerIndex, selected: &[ObjectId], target: ObjectId) {
    let ground = game.object(target).map(|o| o.position);
    let eval = evaluate(game.logic(), local, selected, Some(target), ground, false, false, false, false);
    if let Some(cmd) = &eval.command {
        submit(game, local, cmd);
    }
}

/// `ZR_CONTEXT_DEMO=<repair|enter|attack|rally>`: build-time setup for a screenshot. Spawns the units and
/// damages the structure the way the other debug flags do (all sim writes happen before the game
/// starts; the order itself is submitted as a [`Command`]). Returns the selection and the forced
/// hover target for the cursor label.
pub(crate) fn demo_setup(game: &mut Game, local: PlayerIndex, case: &str) -> Option<(Vec<ObjectId>, ObjectId)> {
    let base = game.spawned().iter().find(|s| s.player == local).map(|s| s.pos).unwrap_or(Coord3D::new(0.0, 0.0, 0.0));
    match case {
        "repair" => {
            let structure = game.objects().find(|o| o.owner == local && crate::sim_view::is_structure(game, o.id)).map(|o| o.id)?;
            let dozer = game.spawned().iter().find(|s| s.player == local && s.template.contains("Dozer")).map(|s| s.id)?;
            let max = game.object(structure).map(|o| o.health).unwrap_or(1.0);
            game.damage_object(structure, max * 0.5);
            Some((vec![dozer], structure))
        }
        "enter" => {
            let chinook = game.spawn_unit(local, "AmericaVehicleChinook", Coord3D::new(base.x + 60.0, base.y + 40.0, 0.0), 0.0)?;
            let ranger = game.spawn_unit(local, "AmericaInfantryRanger", Coord3D::new(base.x + 120.0, base.y + 40.0, 0.0), 0.0)?;
            Some((vec![ranger], chinook))
        }
        "rally" => {
            // A finished barracks with a rally point set (`Command::SetRallyPoint`), selected alone: the
            // route and the `RallyPointMarker` show (`rally_view`).
            let factory = game.spawn_unit(local, "AmericaBarracks", Coord3D::new(base.x + 120.0, base.y + 60.0, 0.0), 0.0)?;
            let at = game.object(factory)?.position;
            let rally = Coord3D::new(at.x + 150.0, at.y - 90.0, game.logic().terrain().ground_height(at.x + 150.0, at.y - 90.0));
            game.submit(local, Command::SetRallyPoint { factory, pos: rally });
            Some((vec![factory], factory))
        }
        "attack" => {
            let enemy = game.players().iter().find(|p| p.index != local && matches!(p.kind, zr_game::PlayerKind::Skirmish)).map(|p| p.index)?;
            let crusader = game.spawn_unit(local, "AmericaTankCrusader", Coord3D::new(base.x + 120.0, base.y + 40.0, 0.0), 0.0)?;
            let target = game.spawn_unit(enemy, "ChinaTankBattleMaster", Coord3D::new(base.x + 220.0, base.y + 40.0, 0.0), 0.0)?;
            Some((vec![crusader], target))
        }
        _ => None,
    }
}

/// Recomputes the cursor every frame and asks [`crate::cursor`] to apply the matching `Mouse.ini` art.
/// A pending command (`placement::OrderMode`) owns the cursor, as `createCommandHint`'s
/// `MOUSEMODE_GUI_COMMAND` branch does (`InGameUI.cpp:3061`), and otherwise the context evaluation
/// decides. A window, an overlay or a scroll takes precedence over both. The text label is only drawn
/// for the `ZR_CONTEXT_DEMO` / `ZR_CURSOR_LABEL` screenshot aids, because the OS cursor is not part of
/// the framebuffer.
#[allow(clippy::too_many_arguments)]
pub(crate) fn update_cursor(
    mut commands: Commands,
    window: Query<&Window, With<PrimaryWindow>>,
    cam: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    map: Res<MapRes>,
    interactive: Res<Interactive>,
    free: Res<FreeCam>,
    mode: Res<crate::placement::OrderMode>,
    modal: crate::ui_block::ModalOpen,
    forced: Res<ForcedHover>,
    scroll: Res<crate::camera::ScrollDir>,
    modes: Res<crate::meta_keys::MetaModes>,
    mut cursor: ResMut<ContextCursor>,
    mut label: ResMut<CursorLabel>,
    mut requested: ResMut<crate::cursor::RequestedCursor>,
    sim: Option<NonSend<SimRes>>,
) {
    let Some(sim) = sim else { return };
    let Ok(w) = window.single() else { return };
    let Ok((camera, gt)) = cam.single() else { return };
    let busy = free.active || sim.possessed().is_some() || modal.get();
    let forced_target = forced.0;
    // A window under the cursor (control bar, radar, menu) forces the regular arrow: retail
    // `createCommandHint` sets `Mouse::ARROW` and returns (`InGameUI.cpp:2944`, `:3065` for a pending command), and the window
    // manager consumes the click before `CommandXlat` sees it (see `ui_block`). The forced hover
    // for screenshots bypasses this.
    let over_ui = modal.over_ui() && forced_target.is_none();
    // Edge-panning / right-drag scrolling shows the 8-direction `Scroll` cursor (`InGameUI::setScrolling`).
    // `createCommandHint` returns early while scrolling, so it wins over every hint.
    let scrolling = scroll.0.filter(|_| !busy && !over_ui && forced_target.is_none());

    let mut kind = CursorKind::Arrow;
    let mut request = CursorRequest::from(CursorKind::Arrow.mouse());
    let mut text = CursorKind::Arrow.label();
    let mut screen = None;
    if busy || over_ui {
        // Free camera, possession, an overlay or a window: the arrow.
    } else if let Some(order) = mode.0.as_ref() {
        // A pending command or placement (`MOUSEMODE_GUI_COMMAND` / `MOUSEMODE_BUILD_PLACE`).
        let (object, at) = pending_hover(&sim, camera, gt, w.cursor_position(), interactive.0, forced_target, crate::pending_cursor::order_forced(order));
        request = crate::pending_cursor::pending_request(order, &sim, object);
        text = request_label(request);
        screen = at;
    } else if sim.selected.is_empty() || (!interactive.0 && forced_target.is_none()) {
        // No selection (no context command) or a screenshot without a forced hover: the arrow.
    } else if let Some(id) = forced_target {
        // Forced hover (screenshot): evaluate the real checks against the forced target.
        let target = Some(id);
        let ground = sim.cur.get(&id).map(|s| Coord3D::new(s.pos[0], s.pos[1], s.pos[2]));
        kind = evaluate(sim.game.logic(), sim.local, &sim.selected, target, ground, modes.force_attack, modes.force_move, modes.waypoints, modes.prefer_selection).cursor;
        request = CursorRequest::from(kind.mouse());
        text = kind.label();
        screen = sim.cur.get(&id).and_then(|s| camera.world_to_viewport(gt, crate::convert::to_bevy(s.pos)).ok());
    } else if let Some(pos) = w.cursor_position() {
        if let Ok(ray) = camera.viewport_to_world(gt, pos) {
            let target = pick_any(&sim, ray.origin, *ray.direction);
            let ground = ray_ground(ray.origin, *ray.direction, &map.ground()).map(|p| Coord3D::new(p.x, -p.z, 0.0));
            kind = evaluate(sim.game.logic(), sim.local, &sim.selected, target, ground, modes.force_attack, modes.force_move, modes.waypoints, modes.prefer_selection).cursor;
            request = CursorRequest::from(kind.mouse());
            text = kind.label();
            screen = Some(pos);
        }
    }
    cursor.0 = kind;

    requested.0 = Some(match scrolling {
        Some(dir) => (CursorRequest::from(MouseCursorName::Scroll), scroll_dir_index(dir, 8)),
        None => (request, 0),
    });

    // Debug label near the pointer / forced target.
    if !label_enabled() {
        hide_label(&mut commands, &mut label);
        return;
    }
    update_label(&mut commands, &mut label, text, screen);
}

/// The object under a pending command's pointer, and the screen position of its label. A forced hover
/// (screenshot) uses the forced object; a screenshot with no pointer uses the `--place-pos` point.
#[allow(clippy::too_many_arguments)]
fn pending_hover(
    sim: &SimRes,
    camera: &Camera,
    gt: &GlobalTransform,
    cursor: Option<Vec2>,
    interactive: bool,
    forced: Option<ObjectId>,
    forced_point: Option<Coord3D>,
) -> (Option<ObjectId>, Option<Vec2>) {
    if let Some(id) = forced {
        let at = sim.cur.get(&id).map(|s| s.pos);
        let screen = at.and_then(|p| camera.world_to_viewport(gt, crate::convert::to_bevy(p)).ok());
        return (Some(id), screen);
    }
    if !interactive {
        // Screenshot: no pointer on the ground, so the label sits on the order's point, else the selected unit.
        let at = forced_point.map(|p| [p.x, p.y, p.z]).or_else(|| sim.selected.first().and_then(|id| sim.cur.get(id)).map(|s| s.pos));
        let screen = at.and_then(|p| camera.world_to_viewport(gt, crate::convert::to_bevy(p)).ok());
        return (None, screen);
    }
    let Some(pos) = cursor else { return (None, None) };
    let Ok(ray) = camera.viewport_to_world(gt, pos) else { return (None, Some(pos)) };
    (pick_any(sim, ray.origin, *ray.direction), Some(pos))
}

/// The debug label text for a pending request: the retail cursor name, or `CROSS` for `Mouse::CROSS`.
fn request_label(request: CursorRequest) -> &'static str {
    match request {
        CursorRequest::Named(name) => name,
        CursorRequest::Cross => "CROSS",
    }
}

/// The index of the `Scroll` cursor direction best matching a screen-space pan direction
/// (`W3DMouse::setCursorDirection`, `W3DMouse.cpp:671`: frame 0 points right and continues
/// clockwise).
pub(crate) fn scroll_dir_index(dir: Vec2, n: usize) -> usize {
    if n == 0 || dir == Vec2::ZERO {
        return 0;
    }
    let theta = dir.y.atan2(dir.x).rem_euclid(std::f32::consts::TAU);
    let step = std::f32::consts::TAU / n as f32;
    (((theta / step) + 0.5) as usize) % n
}

/// The text label is a screenshot aid: `ZR_CONTEXT_DEMO` implies it (the demo shows a cursor that
/// the OS does not capture); `ZR_CURSOR_LABEL` forces it on.
fn label_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var("ZR_CONTEXT_DEMO").is_ok_and(|v| !v.is_empty()) || std::env::var_os("ZR_CURSOR_LABEL").is_some()
    })
}

fn hide_label(commands: &mut Commands, label: &mut CursorLabel) {
    if let Some(e) = label.0 {
        commands.entity(e).insert(Visibility::Hidden);
    }
}

fn update_label(commands: &mut Commands, label: &mut CursorLabel, text: &'static str, screen: Option<Vec2>) {
    let Some(pos) = screen else {
        hide_label(commands, label);
        return;
    };
    match label.0 {
        Some(e) => {
            commands.entity(e).insert((
                Visibility::Visible,
                Node { position_type: PositionType::Absolute, left: Val::Px(pos.x), top: Val::Px(pos.y), ..default() },
                Text::new(text),
            ));
        }
        None => {
            let e = commands
                .spawn((
                    bevy::picking::Pickable::IGNORE,
                    Text::new(text),
                    TextFont { font_size: FontSize::Px(16.0), ..default() },
                    TextColor(Color::srgb(1.0, 0.95, 0.4)),
                    Node { position_type: PositionType::Absolute, left: Val::Px(pos.x), top: Val::Px(pos.y), ..default() },
                ))
                .id();
            label.0 = Some(e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// A stub `ContextChecks` whose predicates are explicit `(unit, target)` sets.
    #[derive(Default)]
    struct Stub {
        resume: BTreeSet<(u32, u32)>,
        dock: BTreeSet<(u32, u32)>,
        repair: BTreeSet<(u32, u32)>,
        get_repaired: BTreeSet<(u32, u32)>,
        get_healed: BTreeSet<(u32, u32)>,
        hijack: BTreeSet<(u32, u32)>,
        carbomb: BTreeSet<(u32, u32)>,
        sabotage: BTreeSet<(u32, u32)>,
        enter: BTreeSet<(u32, u32)>,
        capture: BTreeSet<(u32, u32)>,
        attacks: BTreeSet<(u32, u32)>,
        selectable: BTreeSet<u32>,
        structures: BTreeSet<u32>,
        mines: BTreeSet<u32>,
        rally: BTreeSet<u32>,
    }

    fn has(s: &BTreeSet<(u32, u32)>, u: ObjectId, t: ObjectId) -> bool {
        s.contains(&(u.0, t.0))
    }

    impl ContextChecks for Stub {
        fn can_resume_construction(&self, u: ObjectId, t: ObjectId) -> bool {
            has(&self.resume, u, t)
        }
        fn can_dock_at(&self, u: ObjectId, t: ObjectId) -> bool {
            has(&self.dock, u, t)
        }
        fn can_repair_object(&self, u: ObjectId, t: ObjectId) -> bool {
            has(&self.repair, u, t)
        }
        fn can_get_repaired_at(&self, u: ObjectId, t: ObjectId) -> bool {
            has(&self.get_repaired, u, t)
        }
        fn can_get_healed_at(&self, u: ObjectId, t: ObjectId) -> bool {
            has(&self.get_healed, u, t)
        }
        fn can_hijack_vehicle(&self, u: ObjectId, t: ObjectId) -> bool {
            has(&self.hijack, u, t)
        }
        fn can_convert_object_to_carbomb(&self, u: ObjectId, t: ObjectId) -> bool {
            has(&self.carbomb, u, t)
        }
        fn can_sabotage_building(&self, u: ObjectId, t: ObjectId) -> bool {
            has(&self.sabotage, u, t)
        }
        fn can_enter_object(&self, u: ObjectId, t: ObjectId) -> bool {
            has(&self.enter, u, t)
        }
        fn can_capture_building(&self, u: ObjectId, t: ObjectId) -> bool {
            has(&self.capture, u, t)
        }
        fn capture_power_for(&self, u: ObjectId) -> Option<String> {
            self.capture.iter().any(|&(a, _)| a == u.0).then(|| "SPECIAL_INFANTRY_CAPTURE_BUILDING".to_owned())
        }
        fn attack_result(&self, u: ObjectId, t: ObjectId, _force: bool) -> AttackResult {
            if has(&self.attacks, u, t) {
                AttackResult::Possible
            } else {
                AttackResult::NotPossible
            }
        }
        fn is_selectable_own(&self, id: ObjectId, _local: PlayerIndex) -> bool {
            self.selectable.contains(&id.0)
        }
        fn is_own_structure(&self, id: ObjectId, _local: PlayerIndex) -> bool {
            self.structures.contains(&id.0)
        }
        fn is_own_mine(&self, id: ObjectId, _local: PlayerIndex) -> bool {
            self.mines.contains(&id.0)
        }
        fn is_rally_capable(&self, id: ObjectId, _local: PlayerIndex) -> bool {
            self.rally.contains(&id.0)
        }
    }

    const U: ObjectId = ObjectId(1);
    const U2: ObjectId = ObjectId(2);
    const T: ObjectId = ObjectId(9);
    const GROUND: Option<Coord3D> = Some(Coord3D { x: 5.0, y: 6.0, z: 0.0 });

    #[test]
    fn resume_beats_repair() {
        let mut s = Stub::default();
        s.resume.insert((1, 9));
        s.repair.insert((1, 9));
        let e = evaluate(&s, 0, &[U], Some(T), GROUND, false, false, false, false);
        assert!(matches!(e.command, Some(ContextCommand::ResumeConstruction { .. })));
        assert_eq!(e.cursor, CursorKind::ResumeConstruction);
    }

    #[test]
    fn dock_requires_every_selected_object() {
        let mut s = Stub::default();
        s.dock.insert((1, 9));
        // Only one of two selected can dock: `SELECTION_ALL` fails, so no dock.
        let e = evaluate(&s, 0, &[U, U2], Some(T), GROUND, false, false, false, false);
        assert!(!matches!(e.command, Some(ContextCommand::Dock { .. })));
        s.dock.insert((2, 9));
        let e = evaluate(&s, 0, &[U, U2], Some(T), GROUND, false, false, false, false);
        assert!(matches!(e.command, Some(ContextCommand::Dock { ids, .. }) if ids == vec![U, U2]));
        assert_eq!(e.cursor, CursorKind::Dock);
    }

    #[test]
    fn repair_beats_get_repaired() {
        let mut s = Stub::default();
        s.repair.insert((1, 9));
        s.get_repaired.insert((1, 9));
        let e = evaluate(&s, 0, &[U], Some(T), GROUND, false, false, false, false);
        assert!(matches!(e.command, Some(ContextCommand::Repair { .. })));
        assert_eq!(e.cursor, CursorKind::DoRepair);
    }

    #[test]
    fn hijack_before_enter_is_aggressive() {
        let mut s = Stub::default();
        s.hijack.insert((1, 9));
        s.enter.insert((1, 9));
        let e = evaluate(&s, 0, &[U], Some(T), GROUND, false, false, false, false);
        assert!(matches!(e.command, Some(ContextCommand::Enter { aggressive: true, .. })));
        assert_eq!(e.cursor, CursorKind::EnterAggressively);
    }

    #[test]
    fn enter_before_attack() {
        let mut s = Stub::default();
        s.enter.insert((1, 9));
        s.attacks.insert((1, 9));
        let e = evaluate(&s, 0, &[U], Some(T), GROUND, false, false, false, false);
        assert!(matches!(e.command, Some(ContextCommand::Enter { aggressive: false, .. })));
        assert_eq!(e.cursor, CursorKind::EnterFriendly);
    }

    /// Right-click on empty ground with only rally-capable (factory) objects selected sets their rally
    /// point (`CommandXlat.cpp:2515`), rather than moving them.
    #[test]
    fn ground_click_sets_rally_point_for_factories() {
        let mut s = Stub::default();
        s.rally.insert(1);
        let e = evaluate(&s, 0, &[U], None, GROUND, false, false, false, false);
        assert!(matches!(e.command, Some(ContextCommand::SetRallyPoint { .. })));
        assert_eq!(e.cursor, CursorKind::SetRallyPoint);
        // A mixed selection (one unit that cannot take a rally point) falls back to a move.
        let e = evaluate(&s, 0, &[U, ObjectId(2)], None, GROUND, false, false, false, false);
        assert!(matches!(e.command, Some(ContextCommand::MoveTo { .. })));
        assert_eq!(e.cursor, CursorKind::MoveTo);
    }

    #[test]
    fn attack_before_capture() {
        let mut s = Stub::default();
        s.attacks.insert((1, 9));
        s.capture.insert((1, 9));
        let e = evaluate(&s, 0, &[U], Some(T), GROUND, false, false, false, false);
        assert!(matches!(e.command, Some(ContextCommand::Attack { force: false, .. })));
        assert_eq!(e.cursor, CursorKind::AttackObject);
    }

    #[test]
    fn capture_when_attack_not_possible() {
        let mut s = Stub::default();
        s.capture.insert((1, 9));
        let e = evaluate(&s, 0, &[U], Some(T), GROUND, false, false, false, false);
        assert!(matches!(e.command, Some(ContextCommand::CaptureBuilding { .. })));
        assert_eq!(e.cursor, CursorKind::CaptureBuilding);
    }

    #[test]
    fn move_on_empty_terrain() {
        let s = Stub::default();
        let e = evaluate(&s, 0, &[U], None, GROUND, false, false, false, false);
        assert!(matches!(e.command, Some(ContextCommand::MoveTo { .. })));
        assert_eq!(e.cursor, CursorKind::MoveTo);
    }

    #[test]
    fn force_attack_on_empty_terrain_is_ground_attack() {
        let s = Stub::default();
        let e = evaluate(&s, 0, &[U], None, GROUND, true, false, false, false);
        assert!(matches!(e.command, Some(ContextCommand::AttackGround { .. })));
        assert_eq!(e.cursor, CursorKind::ForceAttackGround);
    }

    #[test]
    fn own_object_without_action_does_nothing() {
        let mut s = Stub::default();
        s.selectable.insert(9);
        let e = evaluate(&s, 0, &[U], Some(T), GROUND, false, false, false, false);
        assert_eq!(e.command, None);
        assert_eq!(e.cursor, CursorKind::Selecting);
    }

    #[test]
    fn own_mine_is_a_move_location() {
        let mut s = Stub::default();
        s.mines.insert(9);
        let e = evaluate(&s, 0, &[U], Some(T), GROUND, false, false, false, false);
        assert!(matches!(e.command, Some(ContextCommand::MoveTo { .. })));
    }

    #[test]
    fn force_attack_skips_non_attack_actions() {
        let mut s = Stub::default();
        s.repair.insert((1, 9));
        s.attacks.insert((1, 9));
        let e = evaluate(&s, 0, &[U], Some(T), GROUND, true, false, false, false);
        assert!(matches!(e.command, Some(ContextCommand::Attack { force: true, .. })));
        assert_eq!(e.cursor, CursorKind::ForceAttackObject);
    }

    #[test]
    fn waypoint_mode_overrides_actions() {
        let mut s = Stub::default();
        s.attacks.insert((1, 9));
        let e = evaluate(&s, 0, &[U], Some(T), GROUND, false, false, true, false);
        assert!(matches!(e.command, Some(ContextCommand::AddWaypoint { .. })));
        assert_eq!(e.cursor, CursorKind::Waypoint);
    }

    #[test]
    fn force_move_drops_the_object() {
        let mut s = Stub::default();
        s.repair.insert((1, 9));
        let e = evaluate(&s, 0, &[U], Some(T), GROUND, false, true, false, false);
        assert!(matches!(e.command, Some(ContextCommand::ForceMove { .. })));
        assert_eq!(e.cursor, CursorKind::MoveTo);
    }

    #[test]
    fn prefer_selection_skips_own_object_actions() {
        let mut s = Stub::default();
        s.selectable.insert(9);
        s.repair.insert((1, 9));
        let e = evaluate(&s, 0, &[U], Some(T), GROUND, false, false, false, true);
        assert_eq!(e.command, None);
        assert_eq!(e.cursor, CursorKind::Selecting);
    }
}
