//! The temporary weapon lock lives on the object's `WeaponSet` (`WeaponSet::setWeaponLock`, `WeaponSet.cpp:1078`),
//! not on the attack. An attack's end keeps it. The releases are the spent clip (`Object::fireCurrentWeapon`,
//! `Object.cpp:1534`), the internet hack (`MSG_INTERNET_HACK`, `GameLogicDispatch.cpp:1564`) and a weapon-set change.
//! Synthetic data only.

use std::sync::Arc;
use zr_sim::damage::DamageType;
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::upgrade::{SetFlagBits, SimUpgradeModule, UpgradeEffect};
use zr_sim::weapon::{CombatTemplate, WeaponSetTemplate, WeaponTemplate};
use zr_sim::{Command, Coord3D, GameLogic, ObjectId, ObjectTemplate, TerrainLogic};

const PU: u32 = 4; // an arbitrary PLAYER_UPGRADE condition bit

/// A gun with a clip of one that reloads on every shot, so each shot is a spent clip.
fn one_shot_gun() -> WeaponTemplate {
    let mut w = WeaponTemplate::new("Gun");
    w.primary_damage = 10.0;
    w.damage_type = DamageType::Explosion;
    w.attack_range = 300.0;
    w.min_delay_between_shots = 1;
    w.max_delay_between_shots = 1;
    w.clip_size = 1;
    w.clip_reload_time = 30;
    w.auto_reloads_clip = true;
    w
}

/// A gun with a long clip, so it never reloads in these tests.
fn gun() -> WeaponTemplate {
    let mut w = one_shot_gun();
    w.clip_size = 1000;
    w.auto_reloads_clip = false;
    w.min_delay_between_shots = 1000;
    w.max_delay_between_shots = 1000;
    w
}

fn set_of(slots: &[Option<WeaponTemplate>]) -> WeaponSetTemplate {
    let mut s = WeaponSetTemplate::default();
    for (i, w) in slots.iter().enumerate() {
        s.weapons[i] = w.clone().map(Arc::new);
    }
    s
}

/// A vehicle with two guns in slots 0 and 1 (slot 1 is absent when `two` is false).
fn tank(two: bool, w0: WeaponTemplate) -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Tank");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
    t.geometry.major_radius = 8.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    let slots = if two { vec![Some(w0), Some(gun())] } else { vec![Some(w0)] };
    t.combat = CombatTemplate { weapon_set: Some(set_of(&slots)), ..CombatTemplate::default() };
    t
}

/// A durable enemy in range of the shooters.
fn target() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Target");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 8.0;
    t.max_health = 100_000.0;
    t.initial_health = 100_000.0;
    t
}

/// Two players, `a` (player 0) at x 100 and a target `b` (player 1) at x 200 (in range, 300).
fn field(template: ObjectTemplate) -> (GameLogic, ObjectId, ObjectId) {
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    let a = g.create_object(&Arc::new(template), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(target()), Coord3D::new(200.0, 100.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    (g, a, b)
}

fn lock(g: &GameLogic, id: ObjectId) -> Option<usize> {
    g.find_object(id).unwrap().combat.weapons.temporary_lock
}

/// `DoWeaponAtObject` (`MSG_DO_WEAPON_AT_OBJECT`): the single-unit special attack that holds the slot.
fn fire_at(g: &mut GameLogic, id: ObjectId, slot: usize, target: ObjectId) {
    g.submit(0, Command::DoWeaponAtObject { ids: vec![id], slot, target, max_shots: -1 });
    g.run_frames(1);
}

/// `setWeaponLock(slot, LOCKED_TEMPORARILY)` on the group, then the attack. The lock survives the attack's
/// end (`AIAttackState::onExit` does not release it, `AIStates.cpp:5714`): a `Stop` ends the attack, the
/// lock stays, and a later plain attack still uses the locked slot (`chooseBestWeaponForTarget`). Reverting
/// `end_attack` to drop the lock (the old attack-order lock) fails the lock asserts.
#[test]
fn a_temporary_lock_survives_the_end_of_its_attack() {
    let (mut g, a, b) = field(tank(true, gun()));
    fire_at(&mut g, a, 1, b);
    assert_eq!(lock(&g, a), Some(1), "the special attack holds slot 1");
    g.submit(0, Command::Stop { ids: vec![a] });
    g.run_frames(1);
    assert!(g.find_object(a).unwrap().combat.attack.is_none(), "the stop ends the attack");
    assert_eq!(lock(&g, a), Some(1), "the attack's end keeps the temporary lock");
    g.submit(0, Command::Attack { ids: vec![a], target: b });
    g.run_frames(3);
    let order = g.find_object(a).unwrap().combat.attack.as_ref().expect("attacking again");
    assert_eq!(order.slot, 1, "a later attack still uses the locked slot");
}

/// A spent clip releases the temporary lock (`Object::fireCurrentWeapon`, `Object.cpp:1534`: `reloaded` releases
/// `LOCKED_TEMPORARILY`). The one-shot gun reloads on its first shot, so the lock is held before and gone after.
/// Reverting the release in the attack's fire phase keeps the lock forever and fails the last assert.
#[test]
fn a_spent_clip_releases_the_temporary_lock() {
    let (mut g, a, b) = field(tank(false, one_shot_gun()));
    fire_at(&mut g, a, 0, b);
    assert_eq!(lock(&g, a), Some(0), "the lock is held before the first shot");
    let mut released = false;
    for _ in 0..200 {
        g.run_frames(1);
        if lock(&g, a).is_none() {
            released = true;
            break;
        }
    }
    assert!(released, "the reloaded clip releases the temporary lock");
}

/// `MSG_DO_WEAPON_AT_OBJECT` locks the weapon on each member that has it (`AIGroup::setWeaponLockForGroup`,
/// `AIGroup.cpp:3071-3081`). A member with no weapon in that slot takes no lock, and the group still attacks.
#[test]
fn the_group_lock_takes_every_member_with_the_weapon() {
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    let with = Arc::new(tank(true, gun()));
    let without = Arc::new(tank(false, gun()));
    let a1 = g.create_object(&with, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let a2 = g.create_object(&with, Coord3D::new(100.0, 120.0, 0.0), 0.0, 0);
    let a3 = g.create_object(&without, Coord3D::new(100.0, 140.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(target()), Coord3D::new(200.0, 100.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(0, Command::DoWeaponAtObject { ids: vec![a1, a2, a3], slot: 1, target: b, max_shots: -1 });
    g.run_frames(1);
    assert_eq!(lock(&g, a1), Some(1));
    assert_eq!(lock(&g, a2), Some(1));
    assert_eq!(lock(&g, a3), None, "a member without slot 1 takes no lock");
}

/// `MSG_INTERNET_HACK` releases the temporary locks first (`GameLogicDispatch.cpp:1564`). A unit that cannot hack
/// is not otherwise touched by the order here, so the release is what changes its lock. Reverting the release in
/// `command_hack_internet` fails the assert.
#[test]
fn an_internet_hack_order_releases_the_temporary_lock() {
    let (mut g, a, b) = field(tank(true, gun()));
    fire_at(&mut g, a, 1, b);
    assert_eq!(lock(&g, a), Some(1));
    g.submit(0, Command::HackInternet { ids: vec![a] });
    g.run_frames(1);
    assert_eq!(lock(&g, a), None, "the hack order releases the temporary lock");
}

/// A weapon-set change rebuilds the weapon set, so both kinds of lock go with it (`updateWeaponSet`,
/// `WeaponSet.cpp:326-329`: `releaseWeaponLock(LOCKED_PERMANENTLY)`). `WeaponLockSharedAcrossSets` is not modelled.
#[test]
fn a_weapon_set_change_releases_the_temporary_lock() {
    let mut t = tank(true, gun());
    t.combat.weapon_flag_bits = SetFlagBits { player_upgrade: PU, ..SetFlagBits::default() };
    let upgraded = set_of(&[Some(one_shot_gun()), Some(gun())]);
    t.weapon_sets = vec![(0, t.combat.weapon_set.clone().unwrap()), (PU, upgraded)];
    t.upgrade_modules = vec![Arc::new(SimUpgradeModule {
        triggered_by: vec!["Upgrade_W".into()],
        conflicts_with: vec![],
        removes_upgrades: vec![],
        requires_all_triggers: false,
        effect: UpgradeEffect::WeaponSet { flag_bit: PU },
    })];
    let (mut g, a, b) = field(t);
    fire_at(&mut g, a, 1, b);
    assert_eq!(lock(&g, a), Some(1));
    g.give_upgrade(a, "Upgrade_W");
    assert_eq!(lock(&g, a), None, "the new weapon set holds no lock");
}
