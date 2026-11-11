//! Lane 184 (wave 21): area guards and the trigger-area change clock.
//!
//! `TEAM_GUARD_AREA` (`ScriptActions::doTeamGuardArea`, ScriptActions.cpp:2010) gives every member
//! `AIUpdateInterface::privateGuardArea` (AIUpdate.cpp:4149): the guard location is the polygon's centre and the
//! polygon itself is `m_areaToGuard`, which `AIGuardMachine` reads in three places:
//! - `lookForInnerTarget` (AIGuard.cpp:275-290): the scan is centred on the polygon, its radius is the scan radius,
//!   only objects inside the polygon qualify, and the scan runs only while `TheGameLogic->getFrameObjectsChangedTriggerAreas()`
//!   is within `m_guardEnemyScanRate` frames (AIGuard.cpp:277-280);
//! - `AIGuardOuterState::onEnter` (AIGuard.cpp:549-550): the outer circle grows to the polygon's radius;
//! - `AIGuardReturnState` / `AIGuardInnerState`: the centre (already the location).
//!
//! The clock is `GameLogic::m_frameObjectsChangedTriggerAreas` (GameLogic.h:198-199), stamped when a non-projectile,
//! non-inert object is created or destroyed (Object.cpp:581, :606) or enters or leaves a polygon
//! (`Object::setTriggerAreaFlagsForChangeInPosition`, Object.cpp:2632-2712). The chain is `Thing::setPosition`,
//! `setOrientation` and `setTransformMatrix` (Thing.cpp:191, :249, :270), each calling `reactToTransformChange`, whose
//! `posDiff` block is Object.cpp:1818-1850.

use std::sync::Arc;

use crate::ai_states::{AiStateType, CommandSource, GuardTargetType};
use crate::logic::GameLogic;
use crate::math::Coord3D;
use crate::object::{ObjectId, PlayerIndex};
use crate::orders::GuardMode;
use crate::template::kind_of;
use zr_map::PolygonTrigger;

/// `Object::m_triggerInfo` (Object.cpp:2632-2712): the polygons the object is inside, as indices into
/// [`TriggerAreas::polygons`], and whether its position changed since they were last checked. A new object is
/// pending so its first check runs (its `setPosition` at creation has a different `m_iPos`).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct TriggerMembership {
    pub(crate) inside: Vec<u32>,
    pub(crate) pending: bool,
}

/// `GameLogic::m_frameObjectsChangedTriggerAreas` and the map's polygon triggers (`TheTerrainLogic`'s list).
#[derive(Clone, Debug, Default)]
pub(crate) struct TriggerAreas {
    pub(crate) polygons: Vec<Arc<PolygonTrigger>>,
    pub(crate) frame_changed: u32,
}

/// The integer bounds of a polygon's points (`PolygonTrigger::updateBounds`, PolygonTrigger.cpp:285-291).
fn bounds(t: &PolygonTrigger) -> ((i32, i32), (i32, i32)) {
    let mut lo = (i32::MAX, i32::MAX);
    let mut hi = (i32::MIN, i32::MIN);
    for p in &t.points {
        lo = (lo.0.min(p[0]), lo.1.min(p[1]));
        hi = (hi.0.max(p[0]), hi.1.max(p[1]));
    }
    if t.points.is_empty() {
        return ((0, 0), (0, 0));
    }
    (lo, hi)
}

/// `PolygonTrigger::getRadius` (PolygonTrigger.cpp:445-449) returns `m_radius`, which `updateBounds` sets from the
/// radius sum at PolygonTrigger.cpp:282-296. That sum takes `halfHeight` from `(hi.y + lo.y) / 2`, not the difference,
/// so the radius is not the bounding box's half diagonal. The port keeps that value (a faithful quirk, not a fix).
pub(crate) fn polygon_radius(t: &PolygonTrigger) -> f32 {
    let (lo, hi) = bounds(t);
    let half_w = (hi.0 - lo.0) as f32 / 2.0;
    let half_h = (hi.1 + lo.1) as f32 / 2.0;
    (half_h * half_h + half_w * half_w).sqrt()
}

/// `PolygonTrigger::getCenterPoint` (PolygonTrigger.cpp:429-441): the centre of the integer bounds, at the ground height.
pub(crate) fn polygon_center(t: &PolygonTrigger, terrain: &crate::terrain::TerrainLogic) -> Coord3D {
    let (lo, hi) = bounds(t);
    let x = (lo.0 + hi.0) as f32 / 2.0;
    let y = (lo.1 + hi.1) as f32 / 2.0;
    Coord3D::new(x, y, terrain.ground_height(x, y))
}

impl GameLogic {
    /// Installs the map's polygon triggers in file order. Triggers with fewer than two points are dropped, as
    /// PolygonTrigger.cpp:189-194 does, inside `#if !(RTS_GENERALS && RETAIL_COMPATIBLE_CRC)`. Whether that branch
    /// applies to the Zero Hour build is unverified; the port filters unconditionally.
    pub fn set_polygon_triggers(&mut self, triggers: &zr_map::PolygonTriggers) {
        self.trigger_areas.polygons = triggers.triggers.iter().filter(|t| t.is_valid()).map(|t| Arc::new(t.clone())).collect();
    }

    /// `GameLogic::updateObjectsChangedTriggerAreas` (GameLogic.h:198): stamps the current frame.
    pub(crate) fn note_trigger_area_change(&mut self) {
        self.trigger_areas.frame_changed = self.frame;
    }

    /// `Object::setTriggerAreaFlagsForChangeInPosition` (Object.cpp:2632-2712), run for every object whose position
    /// changed since the last pass, in id order. An object that entered or left any polygon stamps the change clock.
    /// Projectiles and inert objects never trigger areas (Object.cpp:2636-2637). The pass runs before an area scan
    /// reads the clock (so every earlier move of the frame counts) and at the end of each frame.
    pub(crate) fn trigger_membership_pass(&mut self) {
        let pending: Vec<ObjectId> = self.objects.iter().filter(|(_, o)| o.trigger.pending).map(|(id, _)| *id).collect();
        let mut changed = false;
        for id in pending {
            let polygons = &self.trigger_areas.polygons;
            let Some(o) = self.objects.get_mut(&id) else { continue };
            o.trigger.pending = false;
            if o.is_kind_of(kind_of::PROJECTILE) || o.is_kind_of(kind_of::INERT) {
                continue;
            }
            // `iPos.x = getPosition()->x` (Object.cpp:2642): the integer position truncates.
            let (x, y) = (o.position.x as i32, o.position.y as i32);
            let inside: Vec<u32> = polygons.iter().enumerate().filter(|(_, t)| t.contains(x, y)).map(|(i, _)| i as u32).collect();
            if inside != o.trigger.inside {
                o.trigger.inside = inside;
                changed = true;
            }
        }
        if changed {
            self.note_trigger_area_change();
        }
    }

    /// The area test of `AIGuardMachine::lookForInnerTarget` (AIGuard.cpp:275-280): `checkFrame = frameChanged +
    /// m_guardEnemyScanRate`, and `if (frame > checkFrame) return false`. True while the scan may run.
    pub(crate) fn guard_area_scan_open(&mut self) -> bool {
        self.trigger_membership_pass();
        self.frame <= self.trigger_areas.frame_changed + self.ai_tunables.guard_enemy_scan_rate
    }

    /// The guard position a state reads where C++ calls `area->getCenterPoint` after its
    /// `targetToGuard ? ... : getPositionToGuard()` (`lookForInnerTarget` AIGuard.cpp:282-283,
    /// `AIGuardOuterState::onEnter` :549-551, `AIGuardReturnState::onEnter` :656-657). An area guard reads the
    /// polygon's centre. Other guards read the guarded object or the location (`ai_guard_target`).
    pub(crate) fn ai_guard_centre(&self, id: ObjectId) -> Option<crate::math::Coord3D> {
        match self.objects.get(&id).and_then(|o| o.ai.machine.guard.area.as_ref()) {
            Some(area) => Some(polygon_center(area, &self.terrain)),
            None => self.ai_guard_target(id),
        }
    }

    /// `AIGuardOuterState::onEnter` (AIGuard.cpp:549-550): with an area guard the outer range is at least the
    /// polygon's radius. Any other guard keeps `range`.
    pub(crate) fn guard_area_outer_range(&self, id: ObjectId, range: f32) -> f32 {
        match self.objects.get(&id).and_then(|o| o.ai.machine.guard.area.as_ref()) {
            Some(area) => {
                let radius = polygon_radius(area);
                if range < radius { radius } else { range }
            }
            None => range,
        }
    }

    /// `AIUpdateInterface::privateGuardArea` (AIUpdate.cpp:4149-4167): a mobile, non-projectile unit clears its
    /// machine, takes the polygon's centre as its location, forgets any object to guard, and starts `AI_GUARD` with
    /// the area kept for the guard machine's scans. The guard's own state then runs as for a point guard.
    pub(crate) fn ai_set_guard_area(&mut self, id: ObjectId, area: Arc<PolygonTrigger>, mode: GuardMode, source: CommandSource) {
        if !self.is_mobile(id) || self.objects.get(&id).is_some_and(|o| o.is_kind_of(kind_of::PROJECTILE)) {
            return;
        }
        // `getStateMachine()->clear()` (AIUpdate.cpp:4169), with no jet exemption: a flying jet reaches this base
        // function (JetAIUpdate.cpp:2666-2677); a grounded jet's default branch clears instead (see `jet_base_command_clears`).
        if self.jet_base_command_clears(id) {
            self.ai_clear(id);
        }
        let center = polygon_center(&area, &self.terrain);
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.guard_target_type = GuardTargetType::Area;
            o.ai.machine.goal_position = center;
            o.ai.machine.goal_object = ObjectId::INVALID;
            o.ai.machine.guard.area = Some(area);
            // `combat.guard` holds only point and object guards (the combat and CRC readers know no area).
            o.combat.guard = None;
            o.ai.machine.guard.mode = mode;
            o.combat.guard_mode = mode;
            o.ai.machine.last_command_source = source;
            o.combat.hunt = false;
            o.combat.attack_move = false;
            o.combat.attack = None;
        }
        self.ai_command_set_state(id, AiStateType::Guard);
    }

    /// `AIGroup::groupGuardArea` (AIGroup.cpp:2585) for a script's `TEAM_GUARD_AREA`: every live member of the owner
    /// takes `CMD_FROM_SCRIPT`, as the script's group does.
    pub(crate) fn command_guard_area(&mut self, player: PlayerIndex, ids: &[ObjectId], area: Arc<PolygonTrigger>, mode: GuardMode) {
        for &id in ids {
            let Some(obj) = self.objects.get(&id) else { continue };
            if obj.destroyed || obj.player != player || obj.ai_handle.is_none() {
                continue;
            }
            self.ai_set_guard_area(id, area.clone(), mode, CommandSource::Script);
        }
    }
}
