//! Veterancy, weapon bonuses and upgrades, synthetic data only.

use std::sync::Arc;
use zr_sim::armor::ArmorTemplate;
use zr_sim::bonus::condition;
use zr_sim::body::MaxHealthChangeType;
use zr_sim::damage::DamageType;
use zr_sim::experience::VeterancyLevel;
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::upgrade::{SetFlagBits, SimUpgradeModule, UpgradeEffect};
use zr_sim::weapon::{CombatTemplate, WeaponSetTemplate, WeaponTemplate};
use zr_sim::{Command, Coord3D, GameLogic, ObjectId, ObjectTemplate, TerrainLogic};

const PU: u32 = 4; // an arbitrary PLAYER_UPGRADE condition bit

fn flat() -> TerrainLogic {
    TerrainLogic::flat(100, 100, 0)
}

fn armor(coeff: f32) -> Arc<ArmorTemplate> {
    let mut a = ArmorTemplate::new("A");
    a.coefficients[DamageType::Explosion.index()] = coeff;
    Arc::new(a)
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

fn tank(name: &str, w: WeaponTemplate) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
    t.geometry.major_radius = 8.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    t.combat = CombatTemplate { weapon_set: Some(set_of(w)), ..CombatTemplate::default() };
    t
}

#[test]
fn a_kill_grants_experience_and_promotes() {
    let mut g = GameLogic::new(flat(), 1);
    let mut a = tank("A", cannon(150.0));
    a.is_trainable = true;
    a.experience_required = [0, 10, 20, 30];
    let mut b = tank("B", cannon(0.0));
    b.experience_value = [10, 0, 0, 0];
    b.max_health = 100.0;
    b.initial_health = 100.0;
    let a = g.create_object(&Arc::new(a), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(b), Coord3D::new(200.0, 100.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(0, Command::Attack { ids: vec![a], target: b });
    for _ in 0..400 {
        g.update();
        if g.find_object(b).is_none() {
            break;
        }
    }
    assert!(g.find_object(b).is_none(), "the victim should die");
    let level = g.find_object(a).unwrap().experience.level;
    assert_eq!(level, VeterancyLevel::Veteran, "one kill should promote to veteran");
}

#[test]
fn veteran_weapon_bonus_increases_damage() {
    let mut w = cannon(50.0);
    // VETERAN DAMAGE 200%.
    w.weapon_bonus.entries.push((condition::VETERAN, 0, 2.0));
    let mut a = tank("A", w);
    a.is_trainable = true;
    a.experience_required = [0, 0, 0, 0];
    // The set-flag bits the adapter supplies from `zr-templates` (`WeaponSetType` order).
    a.combat.weapon_flag_bits = SetFlagBits { veteran: 1 << 0, elite: 1 << 1, hero: 1 << 2, player_upgrade: 1 << 3 };
    let mut g = GameLogic::new(flat(), 1);
    let ia = g.create_object(&Arc::new(a), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    g.set_veterancy_level(ia, VeterancyLevel::Veteran);
    let b = g.create_object(&Arc::new(tank("B", cannon(0.0))), Coord3D::new(200.0, 100.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(0, Command::Attack { ids: vec![ia], target: b });
    g.run_frames(4);
    // 50 damage * 2 = 100, so the target dies in one shot.
    assert!(g.find_object(b).is_none(), "the veteran bonus should double the damage");
}

#[test]
fn armor_upgrade_switches_the_armor_set() {
    let mut t = tank("A", cannon(10.0));
    t.combat.armor = Some(armor(1.0));
    t.combat.armor_flag_bits = SetFlagBits { player_upgrade: PU, ..SetFlagBits::default() };
    t.armor_sets = vec![(0, Some(armor(1.0))), (PU, Some(armor(0.5)))];
    t.upgrade_modules = vec![Arc::new(SimUpgradeModule {
        triggered_by: vec!["Upgrade_A".into()],
        conflicts_with: vec![],
        removes_upgrades: vec![],
        requires_all_triggers: false,
        effect: UpgradeEffect::Armor { flag_bit: PU },
    })];
    let mut g = GameLogic::new(flat(), 1);
    let id = g.create_object(&Arc::new(t), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    g.give_upgrade(id, "Upgrade_A");
    use zr_sim::body::DamageInfo;
    let hit = |kind| DamageInfo { source: ObjectId::INVALID, amount: 40.0, kind };
    assert_eq!(g.damage_object(id, hit(DamageType::Explosion)), 20.0, "the upgrade halves explosion damage");
}

#[test]
fn max_health_upgrade_adds_max_health() {
    let mut t = tank("A", cannon(10.0));
    t.upgrade_modules = vec![Arc::new(SimUpgradeModule {
        triggered_by: vec!["Upgrade_H".into()],
        conflicts_with: vec![],
        removes_upgrades: vec![],
        requires_all_triggers: false,
        effect: UpgradeEffect::MaxHealth { add_max_health: 100.0, change_type: MaxHealthChangeType::SameCurrentHealth },
    })];
    let mut g = GameLogic::new(flat(), 1);
    let id = g.create_object(&Arc::new(t), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    assert_eq!(g.find_object(id).unwrap().body.as_ref().unwrap().max_health(), 100.0);
    g.give_upgrade(id, "Upgrade_H");
    assert_eq!(g.find_object(id).unwrap().body.as_ref().unwrap().max_health(), 200.0);
}

#[test]
fn weapon_set_upgrade_selects_the_upgraded_set() {
    let mut t = tank("A", cannon(10.0));
    t.combat.weapon_flag_bits = SetFlagBits { player_upgrade: PU, ..SetFlagBits::default() };
    t.weapon_sets = vec![(0, set_of(cannon(10.0))), (PU, set_of(cannon(100.0)))];
    t.upgrade_modules = vec![Arc::new(SimUpgradeModule {
        triggered_by: vec!["Upgrade_W".into()],
        conflicts_with: vec![],
        removes_upgrades: vec![],
        requires_all_triggers: false,
        effect: UpgradeEffect::WeaponSet { flag_bit: PU },
    })];
    let mut g = GameLogic::new(flat(), 1);
    let id = g.create_object(&Arc::new(t), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    g.give_upgrade(id, "Upgrade_W");
    let w = g.find_object(id).unwrap().combat.weapons.weapon(0).unwrap().template.clone();
    assert_eq!(w.primary_damage, 100.0, "the PLAYER_UPGRADE weapon set should be selected");
}

#[test]
fn player_upgrade_applies_to_owned_units() {
    let mut t = tank("A", cannon(10.0));
    t.upgrade_modules = vec![Arc::new(SimUpgradeModule {
        triggered_by: vec!["Upgrade_P".into()],
        conflicts_with: vec![],
        removes_upgrades: vec![],
        requires_all_triggers: false,
        effect: UpgradeEffect::MaxHealth { add_max_health: 50.0, change_type: MaxHealthChangeType::SameCurrentHealth },
    })];
    let mut g = GameLogic::new(flat(), 1);
    let id = g.create_object(&Arc::new(t), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    g.apply_player_upgrade(0, "Upgrade_P");
    assert_eq!(g.find_object(id).unwrap().body.as_ref().unwrap().max_health(), 150.0);
    assert!(g.player_upgrades(0).any(|u| u == "Upgrade_P"));
}

#[test]
fn global_weapon_bonus_applies() {
    // VETERAN DAMAGE 200% lives in the global GameData set, not the weapon.
    let mut g = GameLogic::new(flat(), 1);
    g.set_global_bonuses(zr_sim::bonus::WeaponBonusSet { entries: vec![(condition::VETERAN, 0, 2.0)] }, [1.0; 4]);
    let mut a = tank("A", cannon(50.0));
    a.is_trainable = true;
    a.experience_required = [0, 0, 0, 0];
    a.combat.weapon_flag_bits = SetFlagBits { veteran: 1 << 0, elite: 1 << 1, hero: 1 << 2, player_upgrade: 1 << 3 };
    let ia = g.create_object(&Arc::new(a), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    g.set_veterancy_level(ia, VeterancyLevel::Veteran);
    let b = g.create_object(&Arc::new(tank("B", cannon(0.0))), Coord3D::new(200.0, 100.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(0, Command::Attack { ids: vec![ia], target: b });
    g.run_frames(4);
    assert!(g.find_object(b).is_none(), "the global VETERAN damage bonus should apply");
}

#[test]
fn rate_of_fire_bonus_shortens_the_delay() {
    let mut t = cannon(1.0);
    t.min_delay_between_shots = 10;
    t.max_delay_between_shots = 10;
    let mut rng = zr_sim::GameRandom::new(1);
    let mut w = zr_sim::weapon::Weapon::new(Arc::new(t), 0);
    w.load_ammo_now(0);
    w.consume_shot_with(0, &mut rng, 2.0);
    assert_eq!(w.when_we_can_fire_again(), 5, "RATE_OF_FIRE 200% halves the 10-frame delay");
}

#[test]
fn veterancy_health_bonus_scales_max_health() {
    let mut t = tank("A", cannon(1.0));
    t.is_trainable = true;
    t.experience_required = [0, 0, 0, 0];
    let mut g = GameLogic::new(flat(), 1);
    g.set_global_bonuses(Default::default(), [1.0, 1.5, 2.0, 2.0]);
    let id = g.create_object(&Arc::new(t), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    assert_eq!(g.find_object(id).unwrap().body.as_ref().unwrap().max_health(), 100.0);
    g.set_veterancy_level(id, VeterancyLevel::Veteran);
    assert_eq!(g.find_object(id).unwrap().body.as_ref().unwrap().max_health(), 150.0);
    g.set_veterancy_level(id, VeterancyLevel::Elite);
    assert!((g.find_object(id).unwrap().body.as_ref().unwrap().max_health() - 200.0).abs() < 1e-3);
}

fn factory_template() -> ObjectTemplate {
    let mut t = tank("Factory", cannon(0.0));
    t.production = Some(zr_sim::production::ProductionTemplate {
        max_queue_entries: 5,
        quantity_modifiers: vec![],
        exit: zr_sim::production::ProductionExit::default(),
        ..Default::default()
    });
    t
}

#[test]
fn queue_upgrade_charges_and_cancels() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 1000);
    g.set_upgrade_info([("Upgrade_X".to_string(), zr_sim::upgrade::UpgradeInfo { cost: 300, build_time_frames: 5 })].into_iter().collect());
    let f = g.create_object(&Arc::new(factory_template()), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    assert!(g.queue_upgrade(0, f, "Upgrade_X"));
    assert_eq!(g.player(0).unwrap().money, 700, "the cost is withdrawn up front");
    assert!(!g.queue_upgrade(0, f, "Upgrade_X"), "a duplicate is blocked while queued");
    assert!(g.cancel_unit(0, f, 0));
    assert_eq!(g.player(0).unwrap().money, 1000, "cancel refunds the cost");
}

#[test]
fn queued_upgrade_researches_and_applies() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 1000);
    g.set_upgrade_info([("Upgrade_X".to_string(), zr_sim::upgrade::UpgradeInfo { cost: 300, build_time_frames: 5 })].into_iter().collect());
    let f = g.create_object(&Arc::new(factory_template()), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    assert!(g.queue_upgrade(0, f, "Upgrade_X"));
    g.run_frames(7);
    assert!(g.player_upgrades(0).any(|u| u == "Upgrade_X"), "research should complete");
    assert!(!g.queue_upgrade(0, f, "Upgrade_X"), "an owned upgrade cannot be re-queued");
}

/// `ProductionUpdate::queueUpgrade` sets neither quantity field (`ProductionUpdate.cpp:303-313`) and
/// does not touch `m_exitDoor`, which `ProductionEntry`'s constructor leaves alone
/// (`ProductionUpdate.cpp:153-167`): the entry comes from the zeroed memory pool
/// (`MemoryPool::allocateBlockImplementation`, GameMemory.cpp:1708), so it reads `DOOR_1` (0) and
/// `total=0` (the snapshot's `Q total` / `Q door` fields for an upgrade).
#[test]
fn an_upgrade_entry_keeps_the_fresh_entry_values() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 1000);
    g.set_upgrade_info([("Upgrade_X".to_string(), zr_sim::upgrade::UpgradeInfo { cost: 300, build_time_frames: 50 })].into_iter().collect());
    let f = g.create_object(&Arc::new(factory_template()), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    assert!(g.queue_upgrade(0, f, "Upgrade_X"));
    let e = &g.find_object(f).unwrap().production.as_ref().unwrap().queue[0];
    assert_eq!(e.quantity_total, 0, "m_productionQuantityTotal stays at the constructor's 0");
    assert_eq!(e.door, Some(0), "m_exitDoor reads the zeroed allocation's DOOR_1");
}

/// `ProductionUpdate::queueUpgrade` (ProductionUpdate.cpp:288-290): a player upgrade already in production at
/// one of the player's factories cannot be queued at another, and its cost is taken once.
#[test]
fn a_player_upgrade_is_produced_at_one_factory_only() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 1000);
    g.set_upgrade_info([("Upgrade_X".to_string(), zr_sim::upgrade::UpgradeInfo { cost: 300, build_time_frames: 50 })].into_iter().collect());
    let f1 = g.create_object(&Arc::new(factory_template()), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let f2 = g.create_object(&Arc::new(factory_template()), Coord3D::new(200.0, 100.0, 0.0), 0.0, 0);
    assert!(g.queue_upgrade(0, f1, "Upgrade_X"));
    assert!(!g.queue_upgrade(0, f2, "Upgrade_X"), "the player is already producing it at f1");
    assert_eq!(g.player(0).unwrap().money, 700, "the cost is taken once");
}
