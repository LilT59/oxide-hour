//! Tunnel networks: `TunnelContain` (`GameLogic/Object/Contain/TunnelContain.cpp`) and the
//! per-player `TunnelTracker` (`Common/RTS/TunnelTracker.cpp`).
//!
//! A whole network shares one passenger list: every `TunnelContain` a player owns stores its
//! passengers in the player's tracker, so a unit can enter at one entrance and leave through any
//! other. Capacity comes from `GlobalData::MaxTunnelCapacity` (not the module's `ContainMax`).
//! Passengers are healed while inside (`TimeForFullHeal`); when the last tunnel dies the network
//! caves in and kills everyone. Selling the last tunnel instead kicks everyone out first.
//!
//! Where it differs (documented): the tunnel-network guard AI (`AITNGuard`) that consumes the
//! "nemesis" target is not ported, so the nemesis is tracked for script/state purposes only.

use crate::contain::ContainKind;
use crate::logic::GameLogic;
use crate::math::LOGICFRAMES_PER_SECOND;
use crate::object::{ObjectId, PlayerIndex};
use crate::template::kind_of;

/// `TunnelTracker`: the communal passenger list of all of one player's tunnels.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TunnelTracker {
    /// `m_tunnelIDs`: the live tunnel entrances, in registration order.
    pub tunnel_ids: Vec<ObjectId>,
    /// `m_containList`: the shared passengers, in insertion order.
    pub passengers: Vec<ObjectId>,
    /// `m_heroUnitsContained`.
    pub hero_units_contained: u32,
    /// `m_framesForFullHeal`: the minimum `TimeForFullHeal` among the player's tunnels.
    pub frames_for_full_heal: f32,
    /// `m_needsFullHealTimeUpdate`.
    pub needs_full_heal_time_update: bool,
    /// `m_curNemesisID`.
    pub nemesis: Option<ObjectId>,
    /// `m_nemesisTimestamp`.
    pub nemesis_timestamp: u32,
}

impl TunnelTracker {
    pub(crate) fn crc(&self, c: &mut crate::crc::Crc) {
        c.add_u32(self.tunnel_ids.len() as u32);
        for id in &self.tunnel_ids {
            c.add_u32(id.0);
        }
        c.add_u32(self.passengers.len() as u32);
        for id in &self.passengers {
            c.add_u32(id.0);
        }
        c.add_u32(self.hero_units_contained);
        c.add_f32(self.frames_for_full_heal);
        c.add_bool(self.needs_full_heal_time_update);
        c.add_u32(self.nemesis.map_or(0xffff_ffff, |x| x.0));
        c.add_u32(self.nemesis_timestamp);
    }
}

impl GameLogic {
    /// Whether `id` is a `TunnelContain` object (even if it is already marked destroyed).
    pub fn is_tunnel_container(&self, id: ObjectId) -> bool {
        self.objects
            .get(&id)
            .and_then(|o| o.template.contain.as_ref())
            .is_some_and(|c| c.kind == ContainKind::Tunnel)
    }

    /// The player a tunnel belongs to, if it is a tunnel.
    fn tunnel_player(&self, id: ObjectId) -> Option<PlayerIndex> {
        self.objects.get(&id).filter(|o| o.template.contain.as_ref().is_some_and(|c| c.kind == ContainKind::Tunnel)).map(|o| o.player)
    }

    /// The shared passenger list of `player`'s tunnel network (empty when they have none).
    pub fn tunnel_passengers(&self, player: PlayerIndex) -> &[ObjectId] {
        self.tunnel_trackers.get(&player).map_or(&[], |t| t.passengers.as_slice())
    }

    /// The shared tunnel tracker of `player`, when they have one.
    pub fn tunnel_tracker(&self, player: PlayerIndex) -> Option<&TunnelTracker> {
        self.tunnel_trackers.get(&player)
    }

    fn tunnel_tracker_mut(&mut self, player: PlayerIndex) -> &mut TunnelTracker {
        self.tunnel_trackers.entry(player).or_default()
    }

    /// `TunnelTracker::onTunnelCreated`: register a newly built entrance and invalidate the cached
    /// full-heal time.
    pub fn on_tunnel_created(&mut self, id: ObjectId) {
        let Some(player) = self.tunnel_player(id) else { return };
        self.on_tunnel_created_for(player, id);
    }

    /// As [`on_tunnel_created`](Self::on_tunnel_created) but for an explicit player. A capture
    /// changes the object's owner before `onCapture` runs, so the tracker must be addressed
    /// directly (`TunnelContain::onCapture`).
    pub(crate) fn on_tunnel_created_for(&mut self, player: PlayerIndex, id: ObjectId) {
        let tr = self.tunnel_tracker_mut(player);
        if !tr.tunnel_ids.contains(&id) {
            tr.tunnel_ids.push(id);
            tr.needs_full_heal_time_update = true;
        }
    }

    /// `TunnelTracker::onTunnelDestroyed`: drop the entrance. The last one caves in and kills every
    /// passenger; otherwise the survivors are repointed at a still-valid entrance.
    pub fn on_tunnel_destroyed(&mut self, dead: ObjectId) {
        let Some(player) = self.objects.get(&dead).map(|o| o.player) else { return };
        self.on_tunnel_destroyed_for(player, dead);
    }

    /// As [`on_tunnel_destroyed`](Self::on_tunnel_destroyed) but for an explicit player (used by
    /// capture, where the owner has already changed).
    pub(crate) fn on_tunnel_destroyed_for(&mut self, player: PlayerIndex, dead: ObjectId) {
        let mut to_destroy: Vec<ObjectId> = Vec::new();
        let mut reassign: Option<(Vec<ObjectId>, ObjectId)> = None;
        if let Some(tr) = self.tunnel_trackers.get_mut(&player) {
            let Some(pos) = tr.tunnel_ids.iter().position(|&x| x == dead) else { return };
            tr.tunnel_ids.remove(pos);
            tr.needs_full_heal_time_update = true;
            if tr.tunnel_ids.is_empty() {
                // Kill everyone in our contain list. Cave in!
                to_destroy = std::mem::take(&mut tr.passengers);
                tr.hero_units_contained = 0;
            } else {
                reassign = Some((tr.passengers.clone(), tr.tunnel_ids[0]));
            }
        }
        if let Some((passengers, valid)) = reassign {
            for pid in passengers {
                if self.objects.get(&pid).and_then(|o| o.contained_by) == Some(dead) {
                    if let Some(o) = self.objects.get_mut(&pid) {
                        o.contained_by = Some(valid);
                    }
                }
            }
        }
        for pid in to_destroy {
            if let Some(o) = self.objects.get_mut(&pid) {
                o.contained_by = None;
            }
            self.destroy_object(pid);
        }
    }

    /// `TunnelContain::onSelling`: selling the last entrance kicks everyone out first, then the
    /// entrance unregisters (so it does not cave in on them). A non-last entrance just unregisters.
    pub(crate) fn on_tunnel_selling(&mut self, id: ObjectId) {
        let Some(player) = self.tunnel_player(id) else { return };
        let last = self
            .tunnel_trackers
            .get(&player)
            .is_some_and(|t| t.tunnel_ids.len() == 1 && t.tunnel_ids.first() == Some(&id));
        if last {
            let passengers: Vec<ObjectId> = self
                .tunnel_trackers
                .get_mut(&player)
                .map(|t| std::mem::take(&mut t.passengers))
                .unwrap_or_default();
            for pid in passengers {
                self.remove_from_contain(id, pid);
            }
        }
        self.on_tunnel_destroyed(id);
    }

    /// `TunnelTracker::isValidContainerFor`: every non-aircraft object may use a tunnel, up to
    /// `GlobalData::MaxTunnelCapacity` (counted in objects, not transport slots).
    pub(crate) fn tunnel_is_valid_container_for(&self, container: ObjectId, unit: ObjectId, check_capacity: bool) -> bool {
        let Some(u) = self.objects.get(&unit) else { return false };
        if u.template.kind_of & kind_of::AIRCRAFT != 0 {
            return false;
        }
        if check_capacity {
            let player = self.objects.get(&container).map_or(0, |c| c.player);
            let count = self.tunnel_trackers.get(&player).map_or(0, |t| t.passengers.len() as i32);
            if count >= self.rules.max_tunnel_capacity {
                return false;
            }
        }
        true
    }

    /// `TunnelTracker::addToContainList`.
    pub(crate) fn tunnel_add_to_contain(&mut self, container: ObjectId, unit: ObjectId) {
        let Some(player) = self.tunnel_player(container) else { return };
        let is_hero = self.objects.get(&unit).is_some_and(|o| o.is_kind_of(kind_of::HERO));
        let tr = self.tunnel_tracker_mut(player);
        tr.passengers.push(unit);
        if is_hero {
            tr.hero_units_contained += 1;
        }
    }

    /// `TunnelTracker::removeFromContain` (the list half).
    pub(crate) fn tunnel_remove_from_contain(&mut self, unit: ObjectId) {
        for tr in self.tunnel_trackers.values_mut() {
            if let Some(pos) = tr.passengers.iter().position(|&x| x == unit) {
                tr.passengers.remove(pos);
                if self.objects.get(&unit).is_some_and(|o| o.is_kind_of(kind_of::HERO)) && tr.hero_units_contained > 0 {
                    tr.hero_units_contained -= 1;
                }
                return;
            }
        }
    }

    /// `TunnelTracker::updateNemesis`: remember the enemy that hit a tunnel, for the (unported)
    /// tunnel-network guard AI.
    pub fn update_tunnel_nemesis(&mut self, tunnel: ObjectId, attacker: ObjectId) {
        let Some(player) = self.tunnel_player(tunnel) else { return };
        let Some(a) = self.objects.get(&attacker) else { return };
        if self.relationship(a.player, player) != crate::damage::Relationship::Enemies {
            return;
        }
        if a.template.kind_of & (kind_of::VEHICLE | kind_of::STRUCTURE | kind_of::INFANTRY | kind_of::AIRCRAFT) == 0 {
            return;
        }
        let now = self.frame;
        let tr = self.tunnel_tracker_mut(player);
        if tr.nemesis.is_none() {
            tr.nemesis = Some(attacker);
            tr.nemesis_timestamp = now;
        } else if tr.nemesis == Some(attacker) {
            tr.nemesis_timestamp = now;
        }
    }

    /// `TunnelTracker::getCurNemesis`: the enemy a tunnel was recently attacked by, or `None` once
    /// 4 seconds pass or the target is gone/stealthed-and-undetected. Non-mutating (the engine
    /// clears its cached id on expiry; here the timestamp check is enough).
    pub fn tunnel_nemesis(&self, player: PlayerIndex) -> Option<ObjectId> {
        let tr = self.tunnel_trackers.get(&player)?;
        let id = tr.nemesis?;
        if tr.nemesis_timestamp + 4 * LOGICFRAMES_PER_SECOND < self.frame {
            return None;
        }
        let target = self.find_object(id)?;
        if target.is_dying() {
            return None;
        }
        if self.is_stealthed(id) {
            return None;
        }
        Some(id)
    }

    /// `TunnelTracker::healObjects(Real frames)` (`TunnelTracker.cpp:333-336`, the
    /// `RETAIL_COMPATIBLE_CRC` branch) for one tunnel's update: every passenger is healed by
    /// `maxHealth / frames`, or to full once it has been inside for `frames`. The frames come from the
    /// tunnel whose slot runs, so several entrances heal faster (retail stacking).
    pub(crate) fn tunnel_heal_objects(&mut self, player: PlayerIndex, frames: f32) {
        let now = self.frame;
        if frames <= 0.0 {
            return;
        }
        let passengers: Vec<ObjectId> = self.tunnel_trackers.get(&player).map_or(Vec::new(), |t| t.passengers.clone());
        for pid in passengers {
            let Some(o) = self.objects.get(&pid) else { continue };
            if o.destroyed || o.is_dying() {
                continue;
            }
            let max = o.body.as_ref().map_or(0.0, |b| b.max_health());
            if max <= 0.0 {
                continue;
            }
            let amount = if now.saturating_sub(o.contained_frame) as f32 >= frames { max } else { max / frames };
            self.heal_object(pid, amount);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contain::{ContainTemplate, GarrisonFirePoints};
    use crate::template::{kind_of, LocomotorKind, LocomotorParams};
    use crate::{Command, Coord3D, GameRules, ObjectTemplate, TerrainLogic};
    use std::sync::Arc;

    fn tunnel_template() -> ObjectTemplate {
        let mut t = ObjectTemplate::new("Tunnel");
        t.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE | kind_of::SELECTABLE;
        t.geometry.major_radius = 20.0;
        t.max_health = 1000.0;
        t.initial_health = 1000.0;
        t.contain = Some(Arc::new(ContainTemplate {
            kind: ContainKind::Tunnel,
            capacity: -1,
            allow_inside_kind_of: None,
            forbid_inside_kind_of: 0,
            allow_allies_inside: true,
            allow_enemies_inside: true,
            allow_neutral_inside: true,
            passengers_allowed_to_fire: false,
            damage_percentage_to_units: 0.0,
            number_of_exit_paths: 1,
            exit_delay: 0,
            door_open_time: 1,
            scatter_nearby_on_exit: true,
            orient_like_container_on_exit: false,
            reset_mood_check_on_exit: false,
            health_regen: 0.0,
            do_i_heal_objects: false,
            heal_frames: 0.0,
            initial_payload: Vec::new(),
            tunnel_frames_for_full_heal: 30.0,
            is_enclosing_container: false,
            fire_points: GarrisonFirePoints::default(),
            passengers_in_turret: false,
            weapon_bonus_passed_to_passengers: false,
            immune_to_clear_building_attacks: false,
        }));
        t
    }

    fn infantry_template() -> ObjectTemplate {
        let mut t = ObjectTemplate::new("Infantry");
        t.kind_of = kind_of::INFANTRY | kind_of::SELECTABLE;
        t.geometry.major_radius = 4.0;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.transport_slot_count = 1;
        t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Legs, 20.0, 60.0, 120.0, 180.0));
        t
    }

    #[test]
    fn capture_reregisters_with_the_new_owners_tunnel_system() {
        let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
        g.set_rules(GameRules { max_tunnel_capacity: 2, ..GameRules::default() });
        let ta = Arc::new(tunnel_template());
        let a = g.create_object(&ta, Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
        let b = g.create_object(&ta, Coord3D::new(600.0, 200.0, 0.0), 0.0, 0);
        let u = g.create_object(&Arc::new(infantry_template()), Coord3D::new(225.0, 200.0, 0.0), 0.0, 0);
        g.submit(0, Command::Enter { ids: vec![u], container: a });
        // `AIEnterState` only contains a unit once its move has succeeded and it is within the
        // container's radius (`AIStates.cpp:6390-6417`); the unit starts 25 units out and has to walk.
        g.run_frames(60);
        assert_eq!(g.tunnel_tracker(0).unwrap().tunnel_ids, vec![a, b]);
        assert!(g.is_contained(u));
        // `TunnelContain::onCapture`: the owner changes first, then the module re-registers.
        g.objects.get_mut(&a).unwrap().player = 1;
        g.on_object_captured(a, 0, 1);
        assert_eq!(g.tunnel_tracker(0).unwrap().tunnel_ids, vec![b], "old owner no longer lists the captured entrance");
        assert_eq!(g.tunnel_tracker(1).unwrap().tunnel_ids, vec![a], "new owner's tunnel system lists it");
        // The engine asserts a captured tunnel is empty; the passengers stay in the old owner's
        // shared list and are repointed at its surviving entrance (`TunnelTracker::onTunnelDestroyed`).
        assert!(g.find_object(u).is_some(), "a non-last capture does not cave in");
        assert_eq!(g.container_of(u), Some(b), "repointed at the old owner's surviving entrance");
    }
}
