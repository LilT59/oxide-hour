//! Weapon-set and armor-set condition flags: salvage steps, and that each setter / veterancy change
//! touches only its own bits (synthetic data only).
//!
//! Ported behaviour: `SalvageCrateCollide::doWeaponSet` / `doArmorSet` (`SalvageCrateCollide.cpp:198-227`),
//! `Object::onVeterancyLevelChanged` (`Object.cpp:3150-3185`), `ActiveBody::onVeterancyLevelChanged`
//! (`ActiveBody.cpp:1500-1520`), `Object::setWeaponSetFlag` / `clearWeaponSetFlag` (`Object.cpp:3101-3120`).

use std::sync::Arc;

use zr_sim::armor::{ArmorTemplate, ARMORSET_CRATE_UPGRADE_ONE, ARMORSET_CRATE_UPGRADE_TWO, ARMORSET_PLAYER_UPGRADE, ARMORSET_ELITE, ARMORSET_HERO, ARMORSET_VETERAN};
use zr_sim::body::DamageInfo;
use zr_sim::damage::DamageType;
use zr_sim::experience::VeterancyLevel;
use zr_sim::modules2::{CrateCollideTemplate, CrateEffect};
use zr_sim::physics::PhysicsTemplate;
use zr_sim::template::{collide_kind, kind_of, LocomotorKind, LocomotorParams};
use zr_sim::upgrade::{SetFlagBits, SimUpgradeModule, UpgradeEffect};
use zr_sim::weapon::{
    CombatTemplate, WeaponSetTemplate, WeaponTemplate, WEAPONSET_CARBOMB, WEAPONSET_CRATEUPGRADE_ONE, WEAPONSET_CRATEUPGRADE_TWO,
    WEAPONSET_ELITE, WEAPONSET_HERO, WEAPONSET_MINE_CLEARING_DETAIL, WEAPONSET_PLAYER_UPGRADE, WEAPONSET_VEHICLE_HIJACK,
    WEAPONSET_VETERAN,
};
use zr_sim::{Coord3D, GameLogic, ObjectId, ObjectTemplate, TerrainLogic};

fn flat() -> TerrainLogic {
    TerrainLogic::flat(200, 200, 0)
}

/// A mobile unit with physics, so it can collide with a crate.
fn unit(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 8.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.physics = Some(PhysicsTemplate::default());
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    t
}

fn cannon(damage: f32) -> WeaponTemplate {
    let mut w = WeaponTemplate::new("Cannon");
    w.primary_damage = damage;
    w.damage_type = DamageType::Explosion;
    w.attack_range = 300.0;
    w.min_delay_between_shots = 1000;
    w.max_delay_between_shots = 1000;
    w
}

fn set_of(w: WeaponTemplate) -> WeaponSetTemplate {
    let mut s = WeaponSetTemplate::default();
    s.weapons[0] = Some(Arc::new(w));
    s
}

fn armor(coeff: f32) -> Arc<ArmorTemplate> {
    let mut a = ArmorTemplate::new("A");
    a.coefficients[DamageType::Explosion.index()] = coeff;
    Arc::new(a)
}

/// The condition bits the adapter would supply for a unit with the veterancy and player-upgrade sets.
fn weapon_bits() -> SetFlagBits {
    SetFlagBits { veteran: WEAPONSET_VETERAN, elite: WEAPONSET_ELITE, hero: WEAPONSET_HERO, player_upgrade: WEAPONSET_PLAYER_UPGRADE }
}

fn armor_bits() -> SetFlagBits {
    SetFlagBits {
        veteran: ARMORSET_VETERAN,
        elite: ARMORSET_ELITE,
        hero: ARMORSET_HERO,
        player_upgrade: ARMORSET_PLAYER_UPGRADE,
    }
}

/// A vehicle whose weapon set is chosen by condition: the default is 10 damage, `CRATEUPGRADE_ONE` 20,
/// `CRATEUPGRADE_TWO` 30, `CARBOMB` 50, `VETERAN` 20, `CARBOMB|VETERAN` 70, `CARBOMB|ELITE` 50.
fn weapon_set_unit() -> ObjectTemplate {
    let mut t = unit("Weapons");
    let default = set_of(cannon(10.0));
    t.combat = CombatTemplate { weapon_set: Some(default.clone()), weapon_flag_bits: weapon_bits(), ..CombatTemplate::default() };
    t.weapon_sets = vec![
        (0, default),
        (WEAPONSET_CRATEUPGRADE_ONE, set_of(cannon(20.0))),
        (WEAPONSET_CRATEUPGRADE_TWO, set_of(cannon(30.0))),
        (WEAPONSET_CARBOMB, set_of(cannon(50.0))),
        (WEAPONSET_VETERAN, set_of(cannon(20.0))),
        (WEAPONSET_CARBOMB | WEAPONSET_VETERAN, set_of(cannon(70.0))),
        (WEAPONSET_PLAYER_UPGRADE, set_of(cannon(40.0))),
    ];
    t
}

/// The damage of the unit's current primary weapon.
fn primary_damage(g: &GameLogic, id: ObjectId) -> f32 {
    g.find_object(id).unwrap().combat.weapons.weapon(0).unwrap().template.primary_damage
}

fn flags(g: &GameLogic, id: ObjectId) -> u32 {
    g.find_object(id).unwrap().weapon_set_flags
}

/// A salvage crate that gives the weapon upgrade on every pickup (`weapon_chance` 1.0, no level or money).
fn salvage_crate() -> ObjectTemplate {
    let mut crate_t = unit("Crate");
    crate_t.locomotor = None;
    crate_t.crate_collides = vec![CrateCollideTemplate {
        effect: CrateEffect::Salvage { weapon_chance: 1.0, level_chance: 0.0, money_chance: 0.0, min_money: 0, max_money: 0 },
        ..Default::default()
    }];
    crate_t
}

/// Drop a salvage crate onto the salvager at (100, 100) and let the collision resolve.
fn pick_up_salvage_crate(g: &mut GameLogic) {
    g.create_object(&Arc::new(salvage_crate()), Coord3D::new(103.0, 100.0, 0.0), 0.0, 0);
    g.run_frames(2);
}

/// A weapon salvager with a default weapon set plus the crate-upgrade sets.
fn weapon_salvager() -> ObjectTemplate {
    let mut t = weapon_set_unit();
    t.name = "Salvager".into();
    t.kind_of |= kind_of::SALVAGER | kind_of::WEAPON_SALVAGER;
    t
}

/// Salvage: the first crate selects `CRATEUPGRADE_ONE`, the second `CRATEUPGRADE_TWO` (`doWeaponSet`).
#[test]
fn salvage_crate_steps_the_weapon_set_one_then_two() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    let sid = g.create_object(&Arc::new(weapon_salvager()), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    assert_eq!(primary_damage(&g, sid), 10.0, "the default weapon set before any crate");

    pick_up_salvage_crate(&mut g);
    assert_eq!(flags(&g, sid) & (WEAPONSET_CRATEUPGRADE_ONE | WEAPONSET_CRATEUPGRADE_TWO), WEAPONSET_CRATEUPGRADE_ONE);
    assert_eq!(primary_damage(&g, sid), 20.0, "CRATEUPGRADE_ONE selects its weapon set");

    pick_up_salvage_crate(&mut g);
    assert_eq!(flags(&g, sid) & (WEAPONSET_CRATEUPGRADE_ONE | WEAPONSET_CRATEUPGRADE_TWO), WEAPONSET_CRATEUPGRADE_TWO);
    assert_eq!(primary_damage(&g, sid), 30.0, "CRATEUPGRADE_TWO selects its weapon set");

    // A fully upgraded salvager is no longer eligible for the weapon set; the crate falls through.
    pick_up_salvage_crate(&mut g);
    assert_eq!(flags(&g, sid) & (WEAPONSET_CRATEUPGRADE_ONE | WEAPONSET_CRATEUPGRADE_TWO), WEAPONSET_CRATEUPGRADE_TWO);
    assert_eq!(primary_damage(&g, sid), 30.0);
}

/// Armor salvage: `ARMORSET_CRATE_UPGRADE_ONE`, then `_TWO` (`doArmorSet`, no chance roll).
#[test]
fn salvage_crate_steps_the_armor_set_one_then_two() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    let mut t = unit("ArmorSalvager");
    t.kind_of |= kind_of::SALVAGER;
    t.collide_kinds |= collide_kind::ARMOR_SALVAGER;
    t.combat = CombatTemplate { armor: Some(armor(1.0)), armor_flag_bits: armor_bits(), ..CombatTemplate::default() };
    t.armor_sets = vec![
        (0, Some(armor(1.0))),
        (ARMORSET_CRATE_UPGRADE_ONE, Some(armor(0.5))),
        (ARMORSET_CRATE_UPGRADE_TWO, Some(armor(0.25))),
    ];
    let sid = g.create_object(&Arc::new(t), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let hit = |g: &mut GameLogic| g.damage_object(sid, DamageInfo { source: ObjectId::INVALID, amount: 40.0, kind: DamageType::Explosion });

    pick_up_salvage_crate(&mut g);
    let flags = g.find_object(sid).unwrap().armor_set_flags;
    assert_eq!(flags & (ARMORSET_CRATE_UPGRADE_ONE | ARMORSET_CRATE_UPGRADE_TWO), ARMORSET_CRATE_UPGRADE_ONE);
    assert_eq!(hit(&mut g), 20.0, "CRATE_UPGRADE_ONE halves explosion damage");

    pick_up_salvage_crate(&mut g);
    let flags = g.find_object(sid).unwrap().armor_set_flags;
    assert_eq!(flags & (ARMORSET_CRATE_UPGRADE_ONE | ARMORSET_CRATE_UPGRADE_TWO), ARMORSET_CRATE_UPGRADE_TWO);
    assert_eq!(hit(&mut g), 10.0, "CRATE_UPGRADE_TWO quarters explosion damage");
}

/// A carbomb keeps `WEAPONSET_CARBOMB` across a level-up, and the veterancy bit still selects its set.
#[test]
fn carbomb_bit_survives_a_level_up() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    let id = g.create_object(&Arc::new(weapon_set_unit()), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    g.set_weapon_set_flag(id, WEAPONSET_CARBOMB);
    assert_eq!(primary_damage(&g, id), 50.0, "the carbomb set");

    g.set_veterancy_level(id, VeterancyLevel::Veteran);
    assert_ne!(flags(&g, id) & WEAPONSET_CARBOMB, 0, "CARBOMB survives the veterancy change");
    assert_ne!(flags(&g, id) & WEAPONSET_VETERAN, 0, "VETERAN is set");
    assert_eq!(primary_damage(&g, id), 70.0, "the most specific set, CARBOMB|VETERAN");

    g.set_veterancy_level(id, VeterancyLevel::Elite);
    assert_ne!(flags(&g, id) & WEAPONSET_CARBOMB, 0, "CARBOMB survives the next level");
    assert_eq!(flags(&g, id) & WEAPONSET_VETERAN, 0, "VETERAN is cleared");
    assert_ne!(flags(&g, id) & WEAPONSET_ELITE, 0, "ELITE is set");
    assert_eq!(primary_damage(&g, id), 50.0, "CARBOMB with no ELITE set");
}

/// A crate-upgraded unit keeps `CRATEUPGRADE_*` (weapon) and `CRATE_UPGRADE_*` (armor) across a level-up.
#[test]
fn crate_upgrade_bits_survive_a_level_up() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    let mut t = weapon_set_unit();
    t.combat.armor = Some(armor(1.0));
    t.combat.armor_flag_bits = armor_bits();
    t.armor_sets = vec![(0, Some(armor(1.0))), (ARMORSET_CRATE_UPGRADE_TWO, Some(armor(0.25)))];
    let id = g.create_object(&Arc::new(t), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    g.set_weapon_set_flag(id, WEAPONSET_CRATEUPGRADE_TWO);
    g.set_armor_set_flag(id, ARMORSET_CRATE_UPGRADE_TWO);
    assert_eq!(primary_damage(&g, id), 30.0);

    g.set_veterancy_level(id, VeterancyLevel::Veteran);
    g.set_veterancy_level(id, VeterancyLevel::Heroic);
    assert_ne!(flags(&g, id) & WEAPONSET_CRATEUPGRADE_TWO, 0, "CRATEUPGRADE_TWO survives");
    assert_eq!(flags(&g, id) & (WEAPONSET_VETERAN | WEAPONSET_ELITE), 0);
    assert_ne!(flags(&g, id) & WEAPONSET_HERO, 0);
    assert_ne!(g.find_object(id).unwrap().armor_set_flags & ARMORSET_CRATE_UPGRADE_TWO, 0, "armor CRATE_UPGRADE_TWO survives");
    assert_ne!(g.find_object(id).unwrap().armor_set_flags & ARMORSET_HERO, 0);
}

/// The mine-clearing detail (`MSG_SET_MINE_CLEARING_DETAIL`) is not a veterancy bit and survives a level-up.
#[test]
fn mine_clearing_bit_survives_a_level_up() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    let mut t = weapon_set_unit();
    t.weapon_sets.push((WEAPONSET_MINE_CLEARING_DETAIL, set_of(cannon(60.0))));
    let id = g.create_object(&Arc::new(t), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    g.set_weapon_set_flag(id, WEAPONSET_MINE_CLEARING_DETAIL);
    assert_eq!(primary_damage(&g, id), 60.0);

    g.set_veterancy_level(id, VeterancyLevel::Elite);
    assert_ne!(flags(&g, id) & WEAPONSET_MINE_CLEARING_DETAIL, 0, "the mine-clearing bit survives");
    assert_eq!(primary_damage(&g, id), 60.0, "the mine-clearing set is still selected");
}

/// `clearWeaponSetFlag` clears only its own bit, and the set is re-selected from what remains.
#[test]
fn clear_weapon_set_flag_clears_only_that_bit() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    let id = g.create_object(&Arc::new(weapon_set_unit()), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    g.set_weapon_set_flag(id, WEAPONSET_CARBOMB);
    g.set_weapon_set_flag(id, WEAPONSET_VEHICLE_HIJACK);
    g.clear_weapon_set_flag(id, WEAPONSET_CARBOMB);
    assert_eq!(flags(&g, id) & WEAPONSET_CARBOMB, 0);
    assert_ne!(flags(&g, id) & WEAPONSET_VEHICLE_HIJACK, 0, "the hijack bit is untouched");
    assert_eq!(primary_damage(&g, id), 10.0, "no set condition matches any more, so the default is selected");
}

/// `WeaponBonusUpgrade` sets the weapon-bonus `PLAYER_UPGRADE` condition (`WeaponBonusUpgrade.cpp:91`),
/// not a weapon-set bit, so it selects no `PLAYER_UPGRADE` weapon set.
#[test]
fn weapon_bonus_upgrade_does_not_select_a_weapon_set() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    let mut t = weapon_set_unit();
    t.upgrade_modules = vec![Arc::new(SimUpgradeModule {
        triggered_by: vec!["Upgrade_B".into()],
        conflicts_with: vec![],
        removes_upgrades: vec![],
        requires_all_triggers: false,
        effect: UpgradeEffect::WeaponBonus,
    })];
    let id = g.create_object(&Arc::new(t), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    g.give_upgrade(id, "Upgrade_B");
    assert!(g.find_object(id).unwrap().weapon_bonus_player_upgrade, "the bonus condition is set");
    assert_eq!(flags(&g, id) & WEAPONSET_PLAYER_UPGRADE, 0, "no weapon-set bit");
    assert_eq!(primary_damage(&g, id), 10.0, "the PLAYER_UPGRADE weapon set is not selected");
}

/// A `WeaponSetUpgrade`-style player upgrade sets only `PLAYER_UPGRADE`, and a later veterancy change
/// does not clear it (the retail upgrade runs once and nothing clears its flag).
#[test]
fn player_upgrade_bit_is_kept_by_upgrades_and_veterancy() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    let mut t = weapon_set_unit();
    t.upgrade_modules = vec![Arc::new(SimUpgradeModule {
        triggered_by: vec!["Upgrade_W".into()],
        conflicts_with: vec![],
        removes_upgrades: vec![],
        requires_all_triggers: false,
        effect: UpgradeEffect::WeaponSet { flag_bit: WEAPONSET_PLAYER_UPGRADE },
    })];
    let id = g.create_object(&Arc::new(t), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    g.set_weapon_set_flag(id, WEAPONSET_CARBOMB);
    g.give_upgrade(id, "Upgrade_W");
    assert_ne!(flags(&g, id) & WEAPONSET_CARBOMB, 0, "the upgrade keeps the carbomb bit");
    assert_ne!(flags(&g, id) & WEAPONSET_PLAYER_UPGRADE, 0, "the upgrade sets its bit");

    g.set_veterancy_level(id, VeterancyLevel::Veteran);
    assert_ne!(flags(&g, id) & WEAPONSET_PLAYER_UPGRADE, 0, "the veterancy change keeps PLAYER_UPGRADE");
    assert_ne!(flags(&g, id) & WEAPONSET_CARBOMB, 0);
    assert_ne!(flags(&g, id) & WEAPONSET_VETERAN, 0);
}
