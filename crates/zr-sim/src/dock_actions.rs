//! The `ActionManager` dock / get-repaired / get-healed queries and the player commands that use
//! them.
//!
//! These are the checks that let the client map a right-click on a building to the retail action
//! (`CommandXlat::evaluateContextCommand`, `Core/.../CommandXlat.cpp:2430`), and the sim commands
//! that then drive the units. `Command::Dock` covers a supply truck / worker ordered to a
//! warehouse or own supply center, `Command::GetRepaired` a damaged vehicle sent to a repair pad
//! (or an airfield for aircraft), and `Command::GetHealed` an infantry sent to a heal pad.
//!
//! Ported from:
//!
//! * `ActionManager::canDockAt` (`Common/RTS/ActionManager.cpp:286`) and its
//!   `canTransferSuppliesAt` (`:207`, already in [`crate::supply`])
//! * `ActionManager::canGetRepairedAt` (`Common/RTS/ActionManager.cpp:142`)
//! * `ActionManager::canGetHealedAt` (`Common/RTS/ActionManager.cpp:327`)
//! * `GameLogic::onDock` / `onGetRepaired` / `onGetHealed`
//!   (`Core/.../GameLogicDispatch.cpp:1586,1571,1601`) and the `AIGroup::groupDock` /
//!   `groupGetRepaired` / `groupGetHealed` fan-out (`GeneralsMD/.../AIGroup.cpp:2443,2411,2395`)
//! * `AIUpdateInterface::privateDock` / `privateGetRepaired` / `privateGetHealed`
//!   (`AIUpdate.cpp:3801,3760,3744`)
//!
//! `privateDock` (`AIUpdate.cpp:3801`) does *not* re-check `canDockAt` — the client only issues
//! `MSG_DOCK` for a legal dock — so [`GameLogic::command_dock`] matches it and docks
//! unconditionally. `privateGetRepaired` / `privateGetHealed` *do* re-check their `ActionManager`
//! queries, so those commands gate on them.
//!
//! Reductions: `OBJECT_STATUS_SOLD` is not modelled (selling removes the building at once), the
//! `RailedTransportDockUpdate` branch of `canDockAt` is not modelled (the shipped skirmish maps
//! have no railed transport), and `isObjectShroudedForAction` only applies to a human source, as in
//! the engine.

use crate::ai_states::CommandSource;
use crate::damage::Relationship;
use crate::logic::GameLogic;
use crate::object::{ObjectId, PlayerIndex};
use crate::shroud::ObjectShroudStatus;
use crate::template::{collide_kind, kind_of};

impl GameLogic {
    /// `ActionManager::canDockAt` (`ActionManager.cpp:286`): can `source` dock with `target` for
    /// any reason? The target must carry a `DockUpdateInterface` (a supply warehouse, supply
    /// center or repair dock) and `source` must be able to transfer supplies there (or be a
    /// vehicle/infantry docking with a railed transport, which the sim does not model).
    ///
    /// `DockUpdate::isDockOpen` is deliberately *not* tested — the engine has it commented out
    /// (`ActionManager.cpp:299-303`); a temporarily closed dock is a state failure, not a legality
    /// failure.
    pub fn can_dock_at(&self, source: ObjectId, target: ObjectId) -> bool {
        // no dock update interface, can't possibly dock
        if !self.has_dock_interface(target) {
            return false;
        }
        // transferring supplies is a valid docking action
        if self.can_transfer_supplies_at(source, target) {
            return true;
        }
        // `RailedTransportDockUpdate` (units and infantry docking a railed transport) is not
        // modelled: the shipped skirmish maps have none.
        false
    }

    /// `ActionManager::canGetRepairedAt` (`ActionManager.cpp:142`): can `source` go get repaired at
    /// `target`? `ai` skips the human-shroud gate (`isObjectShroudedForAction` is only true for a
    /// human source, `ActionManager.cpp:96`).
    pub fn can_get_repaired_at(&self, source: ObjectId, target: ObjectId) -> bool {
        self.can_get_repaired_at_from(source, target, false)
    }

    /// As [`can_get_repaired_at`](Self::can_get_repaired_at), with the AI/script shroud bypass.
    pub fn can_get_repaired_at_from(&self, source: ObjectId, target: ObjectId, ai: bool) -> bool {
        let (Some(s), Some(t)) = (self.objects.get(&source), self.objects.get(&target)) else { return false };
        // only available by our allies
        if self.relationship(s.player, t.player) != Relationship::Allies {
            return false;
        }
        // dead objects cannot be repaired (`ActionManager.cpp:158`: `isEffectivelyDead()`)
        if s.is_dying() {
            return false;
        }
        // if I can't move, I can't get repaired
        if !self.is_mobile(source) {
            return false;
        }
        // nothing can be done with things that are under construction
        if s.under_construction || t.under_construction {
            return false;
        }
        // `repairDest->testStatus(OBJECT_STATUS_SOLD)` is not modelled (selling removes the object).
        // only vehicles can go get repaired at something
        if !s.is_kind_of(kind_of::VEHICLE) {
            return false;
        }
        // vehicles can only be repaired at something that is designated as a repair pad
        if s.is_kind_of(kind_of::AIRCRAFT) {
            // aircraft require an airfield, and must be airborne to be accepted.
            if !self.above_terrain(source) || !t.template.is_collide_kind_of(collide_kind::FS_AIRFIELD) {
                return false;
            }
        } else if !t.is_kind_of(kind_of::REPAIR_PAD) {
            return false;
        }
        // if I am at full health, I can't get repair there
        let Some(body) = &s.body else { return false };
        if body.health() == body.max_health() {
            return false;
        }
        // if the target is in the shroud, we can't do anything. Own/allied objects are always
        // visible to their owner, so the relationship guard mirrors the engine's own status.
        if !ai
            && !self.is_ai_player(s.player)
            && self.relationship(s.player, t.player) != Relationship::Allies
            && self.object_shroud_status(s.player, target) >= ObjectShroudStatus::Fogged
        {
            return false;
        }
        true
    }

    /// `ActionManager::canGetHealedAt` (`ActionManager.cpp:327`): can `source` go get healed at
    /// `target`? Only infantry at a `KINDOF_HEAL_PAD` (a hospital / heal building).
    pub fn can_get_healed_at(&self, source: ObjectId, target: ObjectId) -> bool {
        self.can_get_healed_at_from(source, target, false)
    }

    /// As [`can_get_healed_at`](Self::can_get_healed_at), with the AI/script shroud bypass.
    pub fn can_get_healed_at_from(&self, source: ObjectId, target: ObjectId, ai: bool) -> bool {
        let (Some(s), Some(t)) = (self.objects.get(&source), self.objects.get(&target)) else { return false };
        // only available by our allies
        if self.relationship(s.player, t.player) != Relationship::Allies {
            return false;
        }
        // dead objects cannot be healed (`ActionManager.cpp:343`: `isEffectivelyDead()`)
        if t.is_dying() {
            return false;
        }
        // nothing can be done with things that are under construction
        if s.under_construction || t.under_construction {
            return false;
        }
        // `healDest->testStatus(OBJECT_STATUS_SOLD)` is not modelled.
        // only infantry can go get "healed" somewhere (vehicles get "repaired")
        if !s.is_kind_of(kind_of::INFANTRY) {
            return false;
        }
        // infantry can only be healed at something that is designated as a heal pad
        if !t.is_kind_of(kind_of::HEAL_PAD) {
            return false;
        }
        // if the target is in the shroud, we can't do anything. Own/allied objects are always
        // visible to their owner, so the relationship guard mirrors the engine's own status.
        if !ai
            && !self.is_ai_player(s.player)
            && self.relationship(s.player, t.player) != Relationship::Allies
            && self.object_shroud_status(s.player, target) >= ObjectShroudStatus::Fogged
        {
            return false;
        }
        // no point in healing if you have full health
        if let Some(body) = &s.body {
            if body.health() == body.max_health() {
                return false;
            }
        }
        true
    }

    /// `Object::isAboveTerrain` (`Thing.h:129`): `getHeightAboveTerrain() > 0`.
    fn above_terrain(&self, id: ObjectId) -> bool {
        let Some(o) = self.objects.get(&id) else { return false };
        let ground = self.terrain.surface_height(o.position.x, o.position.y, o.position.z);
        o.position.z > ground + 0.001
    }

    // ---- commands --------------------------------------------------------------------------

    /// `MSG_DOCK` -> `AIGroup::groupDock` -> `AIUpdateInterface::privateDock`
    /// (`AIUpdate.cpp:3801`): dock `ids` with `target`. Only ownership and mobility are checked;
    /// the client is responsible for having called [`can_dock_at`](Self::can_dock_at).
    pub(crate) fn command_dock(&mut self, player: PlayerIndex, ids: &[ObjectId], target: ObjectId) {
        for &id in ids {
            let Some(o) = self.objects.get(&id) else { continue };
            if o.destroyed || o.player != player || o.ai.suspended {
                continue;
            }
            // `SupplyTruckAIUpdate::privateDock` remembers a player's dock as the preferred dock.
            self.ai_dock_command(id, target, CommandSource::Player);
        }
    }

    /// `MSG_GET_REPAIRED` -> `AIGroup::groupGetRepaired` -> `AIUpdateInterface::privateGetRepaired`
    /// (`AIUpdate.cpp:3760`): `privateGetRepaired` bails unless `canGetRepairedAt` passes, then
    /// `aiDock`s the repair depot.
    pub(crate) fn command_get_repaired(&mut self, player: PlayerIndex, ids: &[ObjectId], target: ObjectId) {
        for &id in ids {
            let Some(o) = self.objects.get(&id) else { continue };
            if o.destroyed || o.player != player || o.ai.suspended {
                continue;
            }
            if !self.can_get_repaired_at(id, target) {
                continue;
            }
            self.ai_dock_command(id, target, CommandSource::Player);
        }
    }

    /// `MSG_GET_HEALED` -> `AIGroup::groupGetHealed` -> `AIUpdateInterface::privateGetHealed`
    /// (`AIUpdate.cpp:3744`): `privateGetHealed` bails unless `canGetHealedAt` passes, then
    /// `aiEnter`s the heal depot (the `HealContain` heals the occupant).
    pub(crate) fn command_get_healed(&mut self, player: PlayerIndex, ids: &[ObjectId], target: ObjectId) {
        for &id in ids {
            let Some(o) = self.objects.get(&id) else { continue };
            if o.destroyed || o.player != player || o.ai.suspended {
                continue;
            }
            if !self.can_get_healed_at(id, target) {
                continue;
            }
            // `aiEnter(healDepot, cmdSource)` -> `privateEnter` (`AIUpdate.cpp:3776`).
            self.command_enter_from(player, &[id], target, CommandSource::Player);
        }
    }
}
