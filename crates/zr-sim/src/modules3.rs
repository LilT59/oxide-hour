//! Round-3 gameplay: aura/repair healing (`PropagandaTowerBehavior`, `RepairDockUpdate`,
//! `HealContain`), `OverchargeBehavior`, `ProneUpdate`, the `JetAIUpdate` airfield state machine
//! (`ParkingPlaceBehavior`), `MinefieldBehavior` and the `TransitionDamageFX` client hook.

use crate::logic::GameLogic;
use crate::object::ObjectId;

impl GameLogic {
    /// `PropagandaTowerBehavior`: heal nearby allied units.
    fn update_propaganda(&mut self, id: ObjectId, now: u32) {
        let Some(t) = self.objects.get(&id).and_then(|o| o.template.propaganda.clone()) else { return };
        let next = match self.objects.get(&id) {
            Some(o) if !o.is_dying() => o.behavior.propaganda_next,
            _ => return,
        };
        if next != 0 && now < next {
            return;
        }
        let (pos, player) = match self.objects.get(&id) {
            Some(o) => (o.position, o.player),
            None => return,
        };
        let ids: Vec<ObjectId> = self
            .objects
            .values()
            .filter(|o| o.id != id || t.affects_self)
            .filter(|o| !o.destroyed && !o.is_dying() && o.player == player)
            .filter(|o| o.template.kind_of & crate::template::kind_of::STRUCTURE == 0)
            .filter(|o| o.position.dist_2d(&pos) <= t.radius)
            .map(|o| o.id)
            .collect();
        for v in ids {
            let max = self.objects.get(&v).and_then(|o| o.body.as_ref()).map_or(0.0, |b| b.max_health());
            self.heal_object(v, max * t.heal_percent_each_second / 30.0);
        }
        if let Some(o) = self.objects.get_mut(&id) {
            o.behavior.propaganda_next = now.saturating_add(t.delay_between_updates.max(1));
        }
    }

    /// `OverchargeBehavior::toggle` / `enable` (OverchargeBehavior.cpp:165-231) for a power plant. Enabling extends
    /// the rods through the object's `PowerPlantUpdateInterface` (`:218`) and adds the power bonus; disabling
    /// retracts them (`:189`) and removes it. Both are edge-triggered (`m_overchargeActive`), and the module's wake
    /// follows: `UPDATE_SLEEP_NONE` when enabled, `UPDATE_SLEEP_FOREVER` when disabled (`:222-228`).
    pub fn set_overcharge(&mut self, id: ObjectId, on: bool) -> bool {
        let Some(o) = self.objects.get(&id) else { return false };
        if o.template.overcharge.is_none() {
            return false;
        }
        if on == o.behavior.overcharge {
            return true;
        }
        let now = self.frame;
        if let Some(o) = self.objects.get_mut(&id) {
            o.behavior.overcharge = on;
        }
        self.extend_power_plant_rods(id, on);
        self.wake_family(id, crate::sleepy_families::FamilyKind::Overcharge, now, if on {
            now.saturating_add(crate::module::UPDATE_SLEEP_NONE)
        } else {
            crate::module::UPDATE_SLEEP_FOREVER
        });
        true
    }

    /// `ProneUpdate::update` (ProneUpdate.cpp:78-88): count down the prone timer; at zero
    /// `stopProneEffects` clears the `NO_ATTACK` status the start added.
    pub(crate) fn update_prone(&mut self, id: ObjectId, _now: u32) {
        let frames = match self.objects.get(&id) {
            Some(o) if o.template.prone.is_some() => o.behavior.prone_frames,
            _ => return,
        };
        if frames == 0 {
            return;
        }
        let next = frames - 1;
        if let Some(o) = self.objects.get_mut(&id) {
            o.behavior.prone_frames = next;
        }
        if next == 0 {
            // `stopProneEffects` (`ProneUpdate.cpp:114-119`): clear the model condition and
            // `OBJECT_STATUS_NO_ATTACK`.
            if let Some(o) = self.objects.get_mut(&id) {
                o.status_bits &= !crate::object::status::NO_ATTACK;
            }
        }
    }

    /// `ProneUpdate::goProne` (ProneUpdate.cpp:92-101): add prone frames for the damage just dealt and
    /// start the effects on the first one (`startProneEffects`: `OBJECT_STATUS_NO_ATTACK`).
    pub(crate) fn on_prone_damage(&mut self, id: ObjectId, dealt: f32) {
        let Some(ratio) = self.objects.get(&id).and_then(|o| o.template.prone).map(|p| p.damage_to_frames_ratio) else {
            return;
        };
        let was_prone = self.objects.get(&id).is_some_and(|o| o.behavior.prone_frames > 0);
        if let Some(o) = self.objects.get_mut(&id) {
            o.behavior.prone_frames = o.behavior.prone_frames.saturating_add((dealt * ratio) as u32);
            if !was_prone && o.behavior.prone_frames > 0 {
                o.status_bits |= crate::object::status::NO_ATTACK;
            }
        }
    }

    /// `TransitionDamageFX::onBodyDamageStateChange`: record the FX for a worse body state.
    pub(crate) fn on_transition_damage(&mut self, id: ObjectId, old_state: u8, new_state: u8) {
        if new_state <= old_state {
            return;
        }
        let Some(t) = self.objects.get(&id).and_then(|o| o.template.transition_damage.clone()) else { return };
        let fx = match new_state {
            1 => &t.damaged_fx,
            2 => &t.really_damaged_fx,
            _ => &t.rubble_fx,
        };
        for name in fx {
            self.record_fx(id, name.clone());
        }
    }

    /// The round-3 periodic pass. (The jet airfield cycle is `crate::jet_ai`.)
    pub(crate) fn update_modules3(&mut self) {
        let now = self.frame;
        let ids: Vec<ObjectId> = self.objects.values().filter(|o| !o.destroyed).map(|o| o.id).collect();
        for id in ids {
            self.update_propaganda(id, now);
            self.update_minefield(id);
        }
    }

    /// `MinefieldBehavior::onCollide` reduced: an enemy stepping on the mine detonates it.
    fn update_minefield(&mut self, id: ObjectId) {
        let Some(t) = self.objects.get(&id).and_then(|o| o.template.minefield.clone()) else { return };
        let (pos, player) = match self.objects.get(&id) {
            Some(o) => (o.position, o.player),
            None => return,
        };
        let trigger = self.objects.values().any(|o| {
            o.id != id
                && !o.destroyed
                && !o.is_dying()
                && o.player != player
                && o.template.locomotor.is_some()
                && o.position.dist_2d(&pos) <= o.template.geometry.bounding_circle_radius() + 12.0
                && (t.workers_detonate || o.template.kind_of & crate::template::kind_of::INFANTRY == 0)
        });
        if trigger {
            if let Some(w) = t.detonation_weapon.clone() {
                self.fire_temp_weapon(id, w);
            }
            if !t.regenerates {
                self.destroy_object(id);
            }
        }
    }
}
