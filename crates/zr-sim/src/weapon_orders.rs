//! Weapon-group orders: the `FIRE_WEAPON` family (`MSG_DO_WEAPON`, `MSG_DO_WEAPON_AT_OBJECT`,
//! `MSG_DO_WEAPON_AT_LOCATION`) and `MSG_SET_MINE_CLEARING_DETAIL`.
//!
//! Ported from `GameLogicDispatch.cpp` (`onDoWeapon` :979, `onDoWeaponAtObject` :1038,
//! `onSetMineClearingDetail` :1072, `onDoWeaponAtLocation` :1111), `AIGroup.cpp`
//! (`groupAttackObjectPrivate` :2163, `groupAttackPosition` :2266, `setMineClearingDetail` :3059,
//! `setWeaponLockForGroup` :3071), `Object.cpp` (`setWeaponSetFlag` :3101) and `WeaponSet.cpp`
//! (`setWeaponLock`, `updateWeaponSet`).
//!
//! The weapon lock is the members' `WeaponSet::temporary_lock` (`GameLogic::set_weapon_lock`), and the
//! shot limit is the attack order's `max_shots` (`AttackOrder::with_limit`), so the attack machinery in
//! `combat.rs` runs unchanged.

use crate::ai_states::AiStateType;
use crate::combat::AttackTarget;
use crate::logic::GameLogic;
use crate::math::Coord3D;
use crate::object::{ObjectId, PlayerIndex};
use crate::weapon::WEAPONSET_MINE_CLEARING_DETAIL;

/// The target of a weapon-group order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum WeaponOrderTarget {
    /// `MSG_DO_WEAPON`: each unit attacks its own position (`groupAttackPosition(nullptr, ...)`).
    OwnPosition,
    /// `MSG_DO_WEAPON_AT_LOCATION`.
    Ground(Coord3D),
    /// `MSG_DO_WEAPON_AT_OBJECT`.
    Object(ObjectId),
}

/// `maxShotsToFire` -> the attack order's shot limit: a negative count is unlimited.
fn shot_limit(max_shots: i32) -> Option<u32> {
    if max_shots < 0 {
        None
    } else {
        Some(max_shots as u32)
    }
}

impl GameLogic {
    /// `MSG_DO_WEAPON` -> `onDoWeapon` (`GameLogicDispatch.cpp:979`).
    pub(crate) fn command_do_weapon(&mut self, player: PlayerIndex, ids: &[ObjectId], slot: usize, max_shots: i32) {
        self.weapon_group_order(player, ids, slot, max_shots, WeaponOrderTarget::OwnPosition);
    }

    /// `MSG_DO_WEAPON_AT_OBJECT` -> `onDoWeaponAtObject` (`GameLogicDispatch.cpp:1038`): a missing
    /// target is refused before any unit is locked (`return false`).
    pub(crate) fn command_do_weapon_at_object(&mut self, player: PlayerIndex, ids: &[ObjectId], slot: usize, target: ObjectId, max_shots: i32) {
        if !self.objects.get(&target).is_some_and(|o| !o.destroyed) {
            return;
        }
        self.weapon_group_order(player, ids, slot, max_shots, WeaponOrderTarget::Object(target));
    }

    /// `MSG_DO_WEAPON_AT_LOCATION` -> `onDoWeaponAtLocation` (`GameLogicDispatch.cpp:1111`).
    pub(crate) fn command_do_weapon_at_location(&mut self, player: PlayerIndex, ids: &[ObjectId], slot: usize, pos: Coord3D, max_shots: i32) {
        self.weapon_group_order(player, ids, slot, max_shots, WeaponOrderTarget::Ground(pos));
    }

    /// `if (currentlySelectedGroup && setWeaponLockForGroup(slot, LOCKED_TEMPORARILY))` then
    /// `groupAttack*` (`GameLogicDispatch.cpp:524, 599, 652`). The lock is taken on every member that has
    /// the weapon, and the group attacks when any did (`setWeaponLockForGroup` returns `any`). Then every
    /// member gets the attack, as `groupAttackPosition` and `groupAttackObjectPrivate` do. A member without
    /// the weapon falls back to its best weapon (`choose_weapon` only honours a locked slot the unit has).
    fn weapon_group_order(&mut self, player: PlayerIndex, ids: &[ObjectId], slot: usize, max_shots: i32, target: WeaponOrderTarget) {
        let members: Vec<ObjectId> = ids
            .iter()
            .copied()
            .filter(|id| self.objects.get(id).is_some_and(|o| !o.destroyed && o.player == player))
            .collect();
        // `AIGroup::setWeaponLockForGroup(slot, LOCKED_TEMPORARILY)` (`AIGroup.cpp:3071-3081`): every member with the
        // weapon takes the temporary lock on its weapon set; the attack runs only when one did (`any`).
        let mut locked = false;
        for &id in &members {
            locked |= self.set_weapon_lock(id, slot, crate::weapon::WeaponLockType::Temporarily);
        }
        if !locked {
            return;
        }
        // `groupAttackObjectPrivate` skips a victim that is the same unit, and the target must
        // still be attackable for this player (the same gate as `command_attack`).
        let attack_target = match target {
            WeaponOrderTarget::Object(t) => {
                if self.target_info(&AttackTarget::Object(t), player, false).is_none() {
                    return;
                }
                Some(AttackTarget::Object(t))
            }
            WeaponOrderTarget::Ground(p) => Some(AttackTarget::Ground(p)),
            WeaponOrderTarget::OwnPosition => None,
        };
        let limit = Some((max_shots, slot));
        for id in members {
            let Some(o) = self.objects.get(&id) else { continue };
            if o.ai.suspended {
                continue;
            }
            // `groupAttackPosition` gives each member its own position for the null target.
            let own_target = match attack_target {
                Some(t) => t,
                None => AttackTarget::Ground(o.position),
            };
            // `groupAttackObjectPrivate` (`theUnit != victim`): the victim does not attack itself.
            let is_victim = matches!(own_target, AttackTarget::Object(t) if t == id);
            let has_weapons = !o.combat.weapons.is_empty();
            let passengers = if self.is_passenger_allowed_to_fire(id) { self.contained_units(id).to_vec() } else { Vec::new() };
            if has_weapons && !is_victim {
                self.end_attack(id);
                self.ai_start_attack(id, own_target, false, false, limit, AiStateType::Idle);
            }
            // Garrisoned passengers that can fire get the same target (`groupAttackObjectPrivate`,
            // `groupAttackPosition`) but no weapon lock, which the group applies to its own members.
            for p in passengers {
                if !self.objects.get(&p).is_some_and(|q| !q.destroyed && !q.ai.suspended && !q.combat.weapons.is_empty()) {
                    continue;
                }
                if matches!(own_target, AttackTarget::Object(v) if v == p) {
                    continue;
                }
                self.end_attack(p);
                self.ai_start_attack(p, own_target, false, false, None, AiStateType::Idle);
                if let Some(a) = self.objects.get_mut(&p).and_then(|q| q.combat.attack.as_mut()) {
                    a.max_shots = shot_limit(max_shots);
                }
            }
        }
    }

    /// `MSG_SET_MINE_CLEARING_DETAIL` -> `onSetMineClearingDetail` (`GameLogicDispatch.cpp:1072`):
    /// `AIGroup::setMineClearingDetail(true)`.
    pub(crate) fn command_set_mine_clearing_detail(&mut self, player: PlayerIndex, ids: &[ObjectId]) {
        for &id in ids {
            if self.objects.get(&id).is_some_and(|o| !o.destroyed && o.player == player) {
                self.set_weapon_set_flag(id, WEAPONSET_MINE_CLEARING_DETAIL);
            }
        }
    }

}
