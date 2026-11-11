//! `AssistedTargetingUpdate` and the weapon assist request (lane 192, wave 22).
//!
//! Ported from `Weapon::privateFireWeapon` (`Weapon.cpp:2593`: a shot at an object asks for help first),
//! `Weapon::processRequestAssistance` and `makeAssistanceRequest` (`Weapon.cpp:2960-3031`), and
//! `AssistedTargetingUpdate` (`AssistedTargetingUpdate.cpp:86-148`: `isFreeToAssist`, `assistAttack`,
//! `makeFeedbackLaser`).
//!
//! The feedback lasers are client records. As with `Weapon::createLaser` (`laser_beam.rs`), the laser object's id is
//! allocated (`TheThingFactory->newObject`, `AssistedTargetingUpdate.cpp:124`) and its lifetime modules roll their
//! delays; nothing else of the laser object is simulated.

use crate::ai_states::CommandSource;
use crate::laser_beam::{LaserBeam, LaserSpec};
use crate::logic::GameLogic;
use crate::object::{ObjectId, PlayerIndex};
use crate::weapon::{WeaponLockType, WeaponStatus};

/// `AssistedTargetingUpdateModuleData` (AssistedTargetingUpdate.cpp:50-62), in sim units.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AssistedTargetingTemplate {
    /// `AssistingClipSize`: the shots an assist fires (`aiAttackObject`'s `maxShots`, `AssistedTargetingUpdate.cpp:108`).
    pub clip_size: i32,
    /// `AssistingWeaponSlot`: the weapon slot the assist locks (`setWeaponLock`, `:107`).
    pub weapon_slot: usize,
    /// `LaserFromAssisted`: the feedback beam from the requesting unit to this one (`:110-111`); `None` when the
    /// laser template does not exist (`m_laserFromAssisted` is null).
    pub laser_from_assisted: Option<LaserSpec>,
    /// `LaserToTarget`: the feedback beam from this unit to the victim (`:112-113`).
    pub laser_to_target: Option<LaserSpec>,
}

impl GameLogic {
    /// `Weapon::processRequestAssistance` (Weapon.cpp:3018-3031): every object of the requester's player that is
    /// equivalent to it and in range may assist the shot at `victim`. The port's objects are walked in id order;
    /// C++ walks the player's teams and their member lists (`Player::iterateObjects`, Player.cpp:1762), a
    /// known approximation until team member order is kept.
    pub(crate) fn process_request_assistance(&mut self, requesting: ObjectId, victim: ObjectId, range: f32) {
        let Some(player) = self.objects.get(&requesting).map(|o| o.player) else { return };
        let range_sq = range * range;
        let candidates = self.player_objects_in_engine_order(player);
        for id in candidates {
            self.make_assistance_request(id, requesting, victim, range_sq);
        }
    }

    /// `Player::iterateObjects` (Player.cpp:1762): the player's team prototypes in `m_playerTeamPrototypes` order
    /// (`ScriptEngine::team_order`, load order), and each team's `m_memberList` in the engine's order (newest first,
    /// `TeamInstance::members`, `Object.cpp:898`). The port keeps one live instance per prototype name, so C++'s
    /// per-prototype instance list collapses to that one instance.
    fn player_objects_in_engine_order(&self, player: PlayerIndex) -> Vec<ObjectId> {
        let mut out = Vec::new();
        for key in &self.scripts.team_order {
            let Some(team) = self.scripts.teams.get(key) else { continue };
            if team.owner != Some(player) {
                continue;
            }
            for id in &team.members {
                if self.objects.get(id).is_some_and(|o| !o.destroyed) {
                    out.push(*id);
                }
            }
        }
        out
    }

    /// `makeAssistanceRequest` (Weapon.cpp:2976-3010): a neighbour answers when it is not the requester, is
    /// equivalent to it, is close enough (`FROM_CENTER_2D`), has an assisted-targeting module that is free.
    fn make_assistance_request(&mut self, id: ObjectId, requesting: ObjectId, victim: ObjectId, range_sq: f32) {
        // Don't ask ourselves (Weapon.cpp:2982).
        if id == requesting {
            return;
        }
        let (Some(me), Some(req)) = (self.objects.get(&id), self.objects.get(&requesting)) else { return };
        // `isEquivalentTo` (Weapon.cpp:2986): the port compares template names (command_button.rs makes the same
        // approximation).
        if me.template.name != req.template.name {
            return;
        }
        // `ThePartitionManager->getDistanceSquared(requestOf, requester, FROM_CENTER_2D)` (Weapon.cpp:2991-2993).
        let (dx, dy) = (req.position.x - me.position.x, req.position.y - me.position.y);
        if dx * dx + dy * dy > range_sq {
            return;
        }
        // `findUpdateModule(AssistedTargetingUpdate)` (Weapon.cpp:2996-2998): the template's first such module.
        let Some(cfg) = me.template.assisted_targeting.clone() else { return };
        // `isFreeToAssist` (Weapon.cpp:3007, AssistedTargetingUpdate.cpp:86-95).
        if !self.is_free_to_assist(id) {
            return;
        }
        self.assist_attack(id, victim, &cfg);
    }

    /// `AssistedTargetingUpdate::isFreeToAssist` (AssistedTargetingUpdate.cpp:86-95): the unit can attack, and its
    /// current weapon is ready to fire (the two weapon reloads are tied, so one status speaks for both).
    fn is_free_to_assist(&self, id: ObjectId) -> bool {
        // `if (!me->isAbleToAttack()) return FALSE;` (:90-91).
        if !self.is_able_to_attack(id) {
            return false;
        }
        let Some(o) = self.objects.get(&id) else { return false };
        // `me->getCurrentWeapon() && getStatus() == READY_TO_FIRE` (:93-94).
        o.combat.weapons.weapon(o.combat.current_weapon_slot).is_some_and(|w| w.status(self.frame) == WeaponStatus::ReadyToFire)
    }

    /// `AssistedTargetingUpdate::assistAttack` (AssistedTargetingUpdate.cpp:99-114).
    fn assist_attack(&mut self, me: ObjectId, victim: ObjectId, cfg: &AssistedTargetingTemplate) {
        // `if (!me->getAI()) return;` (:103-104).
        if !self.objects.get(&me).is_some_and(|o| o.ai_handle.is_some()) {
            return;
        }
        // "lock it just till the weapon is empty or the attack is done" (:106-107).
        self.set_weapon_lock(me, cfg.weapon_slot, WeaponLockType::Temporarily);
        // `me->getAI()->aiAttackObject(victimObject, md->m_clipSize, CMD_FROM_AI)` (:108).
        self.ai_attack_object_from(me, victim, false, false, Some((cfg.clip_size, cfg.weapon_slot)), CommandSource::Ai);
        // `makeFeedbackLaser(m_laserFromAssisted, requestingObject, me)` (:110-111), then
        // `makeFeedbackLaser(m_laserToTarget, me, victimObject)` (:112-113).
        if let Some(spec) = &cfg.laser_from_assisted {
            // `makeFeedbackLaser(m_laserFromAssisted, requestingObject, me)`: the laser's end is `me` (the assistant).
            self.make_feedback_laser(me, spec, me);
        }
        if let Some(spec) = &cfg.laser_to_target {
            self.make_feedback_laser(me, spec, victim);
        }
    }

    /// `AssistedTargetingUpdate::makeFeedbackLaser` (AssistedTargetingUpdate.cpp:118-141): a laser object from
    /// `from` to `to`, initialised by `me`'s update (`initLaser(getObject(), to, from->pos, to->pos)`, :140). The
    /// object id is allocated when the laser is created (`newObject`, :124), and its lifetime modules roll their
    /// delays in template order (as `Weapon::createLaser` does, `laser_beam.rs`).
    #[track_caller]
    fn make_feedback_laser(&mut self, me: ObjectId, spec: &LaserSpec, to: ObjectId) {
        // LaserUpdate::initLaser with a non-null parent (`me`) ignores its `startPos` and takes the parent's position
        // (LaserUpdate.cpp:295-313, `updateStartPos`), so the beam starts at the assistant for both lasers.
        let (Some(me_pos), Some(to_pos)) =
            (self.objects.get(&me).map(|o| o.position), self.objects.get(&to).map(|o| o.position))
        else {
            return;
        };
        let id = self.allocate_object_id(&spec.template_name, std::panic::Location::caller()).0;
        let now = self.frame;
        let mut die = u32::MAX;
        for &(_, lo, hi) in &spec.rolls {
            let delay = (self.rng.value(lo as i32, hi as i32) as u32).max(1);
            die = die.min(now.wrapping_add(delay));
        }
        self.combat.lasers.beams.push(LaserBeam {
            id,
            source: me,
            template_name: spec.template_name.clone(),
            bone_name: spec.bone_name.clone(),
            start: me_pos,
            end: to_pos,
            die_frame: die,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use crate::math::Coord3D;
    use crate::template::{kind_of, ObjectTemplate, SleepyModuleKind};
    use crate::terrain::TerrainLogic;
    use crate::weapon::{WeaponSetTemplate, WeaponTemplate};

    fn logic() -> GameLogic {
        GameLogic::new(TerrainLogic::flat(100, 100, 0), 1)
    }

    /// Two sides with their default teams, so created objects join a team's member list (`Player::iterateObjects`
    /// walks those lists, Player.cpp:1762).
    fn sides(g: &mut GameLogic) {
        g.add_player(0, 0);
        g.add_player(1, 0);
        g.scripts_mut().set_player_name("p0", 0);
        g.scripts_mut().set_player_name("p1", 1);
        g.scripts_mut().load_default_team_protos(&[("teamP0", "p0"), ("teamP1", "p1")]);
        g.init_from_sides(&[("teamP0", 0), ("teamP1", 1)]);
    }

    /// An America Patriot-like battery: a structure with one weapon, an AI and an assisted-targeting module.
    fn battery(laser: bool) -> ObjectTemplate {
        let mut t = ObjectTemplate::new("PatriotBattery");
        t.kind_of = kind_of::STRUCTURE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
        t.geometry.major_radius = 8.0;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.ai_update = Some(0);
        let mut w = WeaponTemplate::new("PatriotMissile");
        w.primary_damage = 10.0;
        w.attack_range = 300.0;
        w.request_assist_range = 150.0;
        let mut set = WeaponSetTemplate::default();
        set.weapons[0] = Some(Arc::new(w));
        t.combat.weapon_set = Some(set);
        let spec = |name: &str| LaserSpec { template_name: name.to_string(), bone_name: String::new(), rolls: vec![] };
        t.assisted_targeting = Some(AssistedTargetingTemplate {
            clip_size: 2,
            weapon_slot: 0,
            laser_from_assisted: laser.then(|| spec("FromAssisted")),
            laser_to_target: laser.then(|| spec("ToTarget")),
        });
        t.update_module_order = vec![SleepyModuleKind::AssistedTargeting];
        t
    }

    /// A shot at `victim` by `requester` makes the free, equivalent battery in range assist: its weapon slot is locked
    /// temporarily and it attacks the victim (AssistedTargetingUpdate.cpp:106-108). Revert check: returning early
    /// from `process_request_assistance` leaves the assistant without an attack and without the lock.
    #[test]
    fn a_free_equivalent_battery_in_range_assists_the_shot() {
        let mut g = logic();
        sides(&mut g);
        let requester = g.create_object(&Arc::new(battery(false)), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
        let assistant = g.create_object(&Arc::new(battery(false)), Coord3D::new(220.0, 200.0, 0.0), 0.0, 0);
        let victim = g.create_object(&Arc::new(battery(false)), Coord3D::new(260.0, 200.0, 0.0), 0.0, 1);
        g.run_frames(2);
        g.process_request_assistance(requester, victim, 150.0);
        let o = &g.objects[&assistant];
        assert_eq!(o.combat.attack.as_ref().map(|a| a.target), Some(crate::combat::AttackTarget::Object(victim)));
        assert_eq!(o.combat.weapons.temporary_lock, Some(0));
    }

    /// A battery out of range, or one of another template, does not assist (Weapon.cpp:2986, :2991-2993). Revert
    /// check: dropping the range or the equivalence test lets the far unit assist.
    #[test]
    fn a_far_or_different_unit_does_not_assist() {
        let mut g = logic();
        sides(&mut g);
        let requester = g.create_object(&Arc::new(battery(false)), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
        let far =g.create_object(&Arc::new(battery(false)), Coord3D::new(600.0, 200.0, 0.0), 0.0, 0);
        let mut other = battery(false);
        other.name = "OtherBattery".to_string();
        let different = g.create_object(&Arc::new(other), Coord3D::new(210.0, 200.0, 0.0), 0.0, 0);
        let victim = g.create_object(&Arc::new(battery(false)), Coord3D::new(260.0, 200.0, 0.0), 0.0, 1);
        g.run_frames(2);
        g.process_request_assistance(requester, victim, 150.0);
        assert!(g.objects[&far].combat.attack.is_none(), "out of range");
        assert!(g.objects[&different].combat.attack.is_none(), "not equivalent");
    }

    /// `Player::iterateObjects` walks the team's `m_memberList`, which `Object::setOrRestoreTeam` prepends to
    /// (Object.cpp:898), so the newest unit is walked first. Revert check: the id-order walk returns the older unit
    /// first and fails.
    #[test]
    fn the_assist_walk_is_the_engine_team_order_newest_first() {
        let mut g = logic();
        sides(&mut g);
        let older = g.create_object(&Arc::new(battery(false)), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
        let newer = g.create_object(&Arc::new(battery(false)), Coord3D::new(210.0, 200.0, 0.0), 0.0, 0);
        let other_side = g.create_object(&Arc::new(battery(false)), Coord3D::new(220.0, 200.0, 0.0), 0.0, 1);
        assert_eq!(g.player_objects_in_engine_order(0), vec![newer, older]);
        assert_eq!(g.player_objects_in_engine_order(1), vec![other_side]);
    }

    /// Each feedback laser allocates an object id and records a beam from `from` to `to`
    /// (AssistedTargetingUpdate.cpp:124, :140). Revert check: without the laser calls no beam is recorded.
    #[test]
    fn the_feedback_lasers_allocate_ids_and_record_beams() {
        let mut g = logic();
        sides(&mut g);
        let requester = g.create_object(&Arc::new(battery(false)), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
        let assistant = g.create_object(&Arc::new(battery(true)),Coord3D::new(220.0, 200.0, 0.0), 0.0, 0);
        let victim = g.create_object(&Arc::new(battery(false)), Coord3D::new(260.0, 200.0, 0.0), 0.0, 1);
        g.run_frames(2);
        g.process_request_assistance(requester, victim, 150.0);
        assert_eq!(g.combat.lasers.beams.len(), 2);
        // The laser objects take ids after the three batteries (newObject allocates, AssistedTargetingUpdate.cpp:124).
        assert!(g.combat.lasers.beams[0].id > victim.0 && g.combat.lasers.beams[1].id > g.combat.lasers.beams[0].id);
        assert_eq!(g.combat.lasers.beams[0].source, assistant);
        // LaserFromAssisted runs from the assistant to the assistant (initLaser's parent position, LaserUpdate.cpp:306-313);
        // LaserToTarget runs from the assistant to the victim.
        assert_eq!(g.combat.lasers.beams[0].start, g.objects[&assistant].position, "the from-laser starts at the assistant");
        assert_eq!(g.combat.lasers.beams[0].end, g.objects[&assistant].position, "and ends at the assistant");
        assert_eq!(g.combat.lasers.beams[1].start, g.objects[&assistant].position, "the to-laser starts at the assistant");
        assert_eq!(g.combat.lasers.beams[1].end, g.objects[&victim].position, "and ends at the victim");
    }

    /// `assistAttack` passes `AssistingClipSize` as the shot limit (`aiAttackObject(victim, clip, CMD_FROM_AI)`,
    /// AssistedTargetingUpdate.cpp:108), which `privateAttackObject` sets as the current weapon's max shot count
    /// (AIUpdate.cpp:3446-3448). Revert check: passing `None` for the limit fails the assert.
    #[test]
    fn the_assist_limits_its_attack_to_the_clip_size() {
        let mut g = logic();
        sides(&mut g);
        let requester = g.create_object(&Arc::new(battery(false)), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
        let assistant = g.create_object(&Arc::new(battery(false)), Coord3D::new(220.0, 200.0, 0.0), 0.0, 0);
        let victim = g.create_object(&Arc::new(battery(false)), Coord3D::new(260.0, 200.0, 0.0), 0.0, 1);
        g.run_frames(2);
        g.process_request_assistance(requester, victim, 150.0);
        let attack = g.objects[&assistant].combat.attack.as_ref().expect("the assist attacks");
        assert_eq!(attack.max_shots, Some(2), "AssistingClipSize is the shot limit");
    }

    /// `WeaponSet::setWeaponLock` makes the locked slot `m_curWeapon` (WeaponSet.cpp:1088-1100); `is_free_to_assist`
    /// reads the current weapon (AssistedTargetingUpdate.cpp:93-94). Revert check: without the update in the lock
    /// wrapper the current slot stays 0 and the first assert fails.
    #[test]
    fn a_weapon_lock_makes_the_locked_slot_the_current_weapon() {
        let mut g = logic();
        g.add_player(0, 0);
        let mut t = battery(false);
        let mut set = WeaponSetTemplate::default();
        set.weapons[0] = Some(Arc::new(WeaponTemplate::new("Primary")));
        set.weapons[1] = Some(Arc::new(WeaponTemplate::new("Secondary")));
        t.combat.weapon_set = Some(set);
        let id = g.create_object(&Arc::new(t), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
        assert_eq!(g.objects[&id].combat.current_weapon_slot, 0);
        assert!(g.set_weapon_lock(id, 1, crate::weapon::WeaponLockType::Temporarily));
        assert_eq!(g.objects[&id].combat.current_weapon_slot, 1, "the locked slot is the current weapon");
    }

    /// `Weapon::privateFireWeapon` asks for assistance before any target check (Weapon.cpp:2593), so an unattackable
    /// victim (which `target_info` refuses) still brings the assistants in: the assistant's weapon is locked and it is
    /// ordered to attack (AssistedTargetingUpdate.cpp:106-108). C++ then fails that attack on entry: the attack machine's
    /// default state AIM_AT_TARGET has `cannotPossiblyAttackObject` (AIStates.cpp:265-281), and its CONTINUE enter is
    /// followed by the transition check (StateMachine.cpp:626-647), so no attack remains. Revert check: moving the request
    /// back below the target lookup leaves the assistant without the lock.
    #[test]
    fn an_unattackable_victim_still_asks_for_assistance() {
        let mut g = logic();
        sides(&mut g);
        let requester = g.create_object(&Arc::new(battery(false)), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
        let assistant = g.create_object(&Arc::new(battery(false)), Coord3D::new(220.0, 200.0, 0.0), 0.0, 0);
        let mut victim_t = battery(false);
        victim_t.kind_of |= crate::template::kind_of::UNATTACKABLE;
        let victim = g.create_object(&Arc::new(victim_t), Coord3D::new(260.0, 200.0, 0.0), 0.0, 1);
        g.run_frames(2);
        assert!(!g.fire_weapon(requester, 0, crate::combat::AttackTarget::Object(victim)), "no target, so no shot");
        assert_eq!(g.objects[&assistant].combat.weapons.temporary_lock, Some(0), "the assist asked: its weapon is locked");
        assert!(g.objects[&assistant].combat.attack.is_none(), "the unattackable victim fails the attack on entry");
    }
}
