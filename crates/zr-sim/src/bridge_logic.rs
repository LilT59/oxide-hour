//! Bridge object behavior (`BridgeBehavior.cpp`, `BridgeTowerBehavior.cpp`, `Bridge::updateDamageState`
//! in `Core/.../Map/TerrainLogic.cpp`).
//!
//! A bridge is represented by a logic object (`ThingTemplate::isBridge`, e.g. Alpine Assault's
//! `MonumentTrainBridge`). Its body damage state drives the terrain `Bridge`: on `BODY_RUBBLE` the
//! crossing is re-blocked and every object standing on the layer dies (`DAMAGE_FALLING` /
//! `DEATH_SPLATTED`, `HUGE_DAMAGE_AMOUNT`); repairing the body back above rubble re-enables it.
//! Damage is rescaled by health percentage and propagated between a bridge and its towers
//! (`BridgeBehavior::onDamage` / `BridgeTowerBehavior::onDamage`); a propagated hit from a
//! bridge/tower does not bounce back.

use crate::body::DamageInfo;
use crate::condition::BodyDamageType;
use crate::damage::{DamageType, DeathType, HUGE_DAMAGE_AMOUNT};
use crate::logic::GameLogic;
use crate::math::Coord3D;
use crate::object::ObjectId;

impl GameLogic {
    /// `TerrainLogic::addLandmarkBridgeToLogic` / `BridgeInfo::bridgeObjectID`: link a created bridge
    /// object to the terrain `Bridge` whose rectangle contains `pos`.
    pub fn link_bridge_object(&mut self, id: ObjectId, pos: &Coord3D) -> bool {
        if let Some(b) = self.terrain.bridges_mut().iter_mut().find(|b| b.contains_2d(pos.x, pos.y)) {
            b.bridge_object = Some(id);
            true
        } else {
            false
        }
    }

    /// `BridgeTowerBehavior::setBridge`: link a tower object to its bridge.
    pub fn link_bridge_tower(&mut self, tower: ObjectId, bridge: ObjectId) {
        if let Some(o) = self.objects.get_mut(&tower) {
            o.bridge = Some(bridge);
        }
    }

    /// `getBodyModule()->getDamageState()`: the body's stored state, or `None` when it has no body.
    pub fn body_damage_state(&self, id: ObjectId) -> Option<BodyDamageType> {
        let o = self.objects.get(&id)?;
        let b = o.body.as_ref()?;
        Some(b.damage_state())
    }

    /// The terrain bridge index whose `bridge_object` is `id`, if any.
    pub fn bridge_index_of_object(&self, id: ObjectId) -> Option<usize> {
        self.terrain.bridges().iter().position(|b| b.bridge_object == Some(id))
    }

    /// `BridgeBehavior::onDamage` / `BridgeTowerBehavior::onDamage`: propagate a hit on a bridge or
    /// tower to the counterpart, rescaling by health percentage. `source` marks the originator, so a
    /// hit whose source is already a bridge/tower does not propagate again.
    pub(crate) fn on_bridge_damage(&mut self, id: ObjectId, dealt: f32, source: ObjectId) {
        if dealt <= 0.0 {
            return;
        }
        let (is_bridge, tower_of, max_health) = match self.objects.get(&id) {
            Some(o) => (o.template.is_bridge, o.bridge, o.body.as_ref().map_or(0.0, |b| b.max_health())),
            None => return,
        };
        if max_health <= 0.0 {
            return;
        }
        // `onDamage` skips when the source is a bridge or bridge tower (breaks the loop).
        if self.objects.get(&source).is_some_and(|s| s.template.is_bridge || s.bridge.is_some()) {
            return;
        }
        let pct = dealt / max_health;
        if is_bridge {
            let towers: Vec<ObjectId> =
                self.objects.values().filter(|o| o.bridge == Some(id) && !o.destroyed).map(|o| o.id).collect();
            for t in towers {
                let tmax = self.objects.get(&t).and_then(|o| o.body.as_ref()).map_or(0.0, |b| b.max_health());
                let amount = pct * tmax;
                if amount > 0.0 {
                    self.damage_object_with_death(t, DamageInfo { source: id, amount, kind: DamageType::Explosion }, DeathType::Normal);
                }
            }
        } else if let Some(bridge) = tower_of {
            let bmax = self.objects.get(&bridge).and_then(|o| o.body.as_ref()).map_or(0.0, |b| b.max_health());
            let amount = pct * bmax;
            if amount > 0.0 {
                self.damage_object_with_death(bridge, DamageInfo { source: id, amount, kind: DamageType::Explosion }, DeathType::Normal);
            }
        }
    }

    /// `Bridge::updateDamageState`: sync the terrain `Bridge` to its object's body state. Returns
    /// `true` when the bridge just became destroyed (Rubble).
    pub(crate) fn update_bridge_state(&mut self, id: ObjectId) -> bool {
        let Some(state) = self.body_damage_state(id) else { return false };
        let Some(idx) = self.bridge_index_of_object(id) else { return false };
        let was_destroyed = self.terrain.bridges()[idx].destroyed;
        let now_destroyed = state == BodyDamageType::Rubble;
        if now_destroyed == was_destroyed {
            return false;
        }
        if now_destroyed {
            // The bridge is still marked healthy here, so `object_interacts_with_bridge_layer` works.
            self.destroy_bridge_occupants(idx, id);
        }
        self.terrain.bridges_mut()[idx].destroyed = now_destroyed;
        self.pathfinder.refresh_bridges(&self.terrain);
        now_destroyed
    }

    /// `Bridge::updateDamageState` on `BODY_RUBBLE`: kill every object standing on the layer
    /// (`DAMAGE_FALLING` / `DEATH_SPLATTED`).
    fn destroy_bridge_occupants(&mut self, idx: usize, bridge_id: ObjectId) {
        let Some(layer) = self.terrain.bridges().get(idx).map(|b| b.layer) else { return };
        let victims: Vec<ObjectId> = self
            .objects
            .values()
            .filter(|o| !o.destroyed && !o.template.is_bridge && o.bridge.is_none())
            .filter(|o| self.terrain.object_interacts_with_bridge_layer(&o.position, 0.0, layer))
            .map(|o| o.id)
            .collect();
        for v in victims {
            self.damage_object_with_death(
                v,
                DamageInfo { source: bridge_id, amount: HUGE_DAMAGE_AMOUNT, kind: DamageType::Falling },
                DeathType::Splatted,
            );
        }
    }

    /// `Object::attemptHealing` for a bridge/repair target, restoring the crossing when the bridge
    /// leaves `BODY_RUBBLE` (`TerrainLogic::isBridgeRepaired`).
    pub fn heal_object(&mut self, id: ObjectId, amount: f32) -> f32 {
        let healed = match self.objects.get_mut(&id).and_then(|o| o.body.as_mut()) {
            Some(b) => b.attempt_healing(amount),
            None => 0.0,
        };
        if healed > 0.0 && self.objects.get(&id).is_some_and(|o| o.template.is_bridge) {
            self.update_bridge_state(id);
        }
        healed
    }
}
