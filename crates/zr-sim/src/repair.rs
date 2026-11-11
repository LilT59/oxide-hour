//! Dozer / worker repair (`DozerAIUpdate::privateRepair` / `DOZER_TASK_REPAIR`,
//! `BridgeBehavior::isScaffoldInMotion` gate).
//!
//! `Command::Repair` sends a dozer to a structure or bridge object and heals it by a percentage of
//! its max health per second. A bridge tower is resolved to its bridge (`BridgeTowerBehavior::
//! getBridgeID`) and the healing is rescaled to the bridge's max health. Healing a bridge out of
//! `BODY_RUBBLE` re-enables the crossing (`TerrainLogic::isBridgeRepaired`).
//!
//! Reduced: the scaffold animation (`BridgeScaffoldBehavior`) is not simulated, so the
//! `isScaffoldInMotion` heal gate is a no-op. The engine's own `canRepairObject` disables bridge
//! repair in retail (`// GS repairing bridges cut 12/12/02`); the mechanics are implemented here so
//! they can be enabled.

use crate::build::approach_point;
use crate::logic::GameLogic;
use crate::object::ObjectId;
use crate::template::kind_of;

impl GameLogic {
    /// `MSG_DO_REPAIR` / `DozerAIUpdate::privateRepair`: `dozer` repairs `target`.
    ///
    /// The heal rate is `DozerAIUpdateModuleData::RepairHealthPercentPerSecond` (parsed into
    /// [`crate::dozer::DozerTemplate`]); a dozer whose template has no rate heals nothing, like the
    /// engine's 0.0 default. Unlike the player command path, this low-level entry still accepts
    /// bridges (the mechanics below), which retail's `canRepairObject` disables.
    pub fn command_repair(&mut self, player: crate::object::PlayerIndex, dozer: ObjectId, target: ObjectId) -> bool {
        let (dozer_radius, can) = match self.objects.get(&dozer) {
            Some(o) if !o.destroyed && o.player == player && o.template.locomotor.is_some() => {
                (o.template.geometry.bounding_circle_radius(), true)
            }
            _ => (0.0, false),
        };
        if !can {
            return false;
        }
        // `DozerAIUpdate::privateRepair`: if this object is already actively being repaired by
        // another dozer, do not double up (`Object::getSoleHealingBenefactor`).
        if self
            .objects
            .values()
            .any(|o| o.id != dozer && !o.destroyed && !o.is_dying() && o.repair_target == Some(target))
        {
            return false;
        }
        let (site, radius) = match self.objects.get(&target) {
            // `ActionManager::canRepairObject`: structures and bridges (retail then rejects bridges).
            Some(t) if !t.destroyed && (t.template.is_bridge || t.is_kind_of(kind_of::STRUCTURE)) => {
                (t.position, t.template.geometry.bounding_circle_radius())
            }
            _ => return false,
        };
        let approach = approach_point(&site, &self.objects[&dozer].position, radius + dozer_radius + 5.0);
        if let Some(o) = self.objects.get_mut(&dozer) {
            o.repair_target = Some(target);
        }
        self.ai_move_to_position_ignoring(dozer, approach, target);
        true
    }

    /// Advance one dozer's repair task (`DozerActionDoActionState`, `DOZER_TASK_REPAIR`), called from the
    /// dozer module's update (`DozerAIUpdate::update` after its AI update, `DozerAIUpdate.cpp:1666`).
    pub(crate) fn dozer_repair_step(&mut self, dozer: ObjectId, target: ObjectId) {
        // `DozerPrimaryStateMachine`'s idle conditions prefer BUILD over REPAIR
        // (`DozerAIUpdate.cpp:1292`): a pending build preempts healing and the repair task resumes
        // when the build completes.
        if self.objects.get(&dozer).is_none_or(|o| o.build_target.is_some()) {
            return;
        }
        let Some(t) = self.objects.get(&target) else {
            // The goal is gone: the action machine cancels the task (`DozerAIUpdate.cpp:170-179`).
            self.dozer_repair_finished(dozer);
            return;
        };
        // Heal once the dozer is at its action position, just outside the footprint
        // (`DozerActionMoveToActionPosState` arrival). The kinematic locomotor stops on the
        // approach point and can wobble slightly, so the tolerance includes the dozer's own
        // radius; the engine's `MIN_ACTION_TOLERANCE` is 70.
        let dozer_radius = self.objects.get(&dozer).map_or(0.0, |o| o.template.geometry.bounding_circle_radius());
        let reach = t.template.geometry.bounding_circle_radius() + dozer_radius + 20.0;
        let near = self.objects.get(&dozer).is_some_and(|o| o.position.dist_2d(&t.position) <= reach);
        if !near {
            return;
        }
        let (max, cur) = t.body.as_ref().map_or((0.0, 0.0), |b| (b.max_health(), b.health()));
        if max <= 0.0 || cur >= max {
            // "check for fully repaired": `complete = TRUE` (`DozerAIUpdate.cpp:663-670`), so the
            // primary machine returns to IDLE.
            self.dozer_repair_finished(dozer);
            return;
        }
        // `DozerAIUpdateModuleData::RepairHealthPercentPerSecond` (a fraction of max health per
        // second), through `getRepairHealthPerSecond`.
        let rate = self.objects.get(&dozer).and_then(|o| o.template.dozer).map_or(0.0, |d| d.repair_health_percent_per_second);
        let step = max * rate / crate::math::LOGICFRAMES_PER_SECOND as f32;
        self.heal_object(target, step);
        // `BridgeTowerBehavior::onHealing`: a healed tower heals its bridge by the same percentage.
        if let Some(bridge) = self.objects.get(&target).and_then(|o| o.bridge) {
            if let Some(bmax) = self.objects.get(&bridge).and_then(|o| o.body.as_ref()).map(|b| b.max_health()) {
                self.heal_object(bridge, bmax * rate / crate::math::LOGICFRAMES_PER_SECOND as f32);
            }
        }
    }
}
