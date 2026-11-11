//! Continuous fire and PerAttack prefire through the attack command, on synthetic data only.
//!
//! `shotFired` runs inside every weapon shot (`Object::fireCurrentWeapon`, Object.cpp:1510), so the
//! `FiringTracker`'s MEAN level lifts the damage (`Weapon::computeBonus` reads the object's bonus flags), and the
//! PerAttack prefire reads the tracker's count at the victim (`Weapon::getPreAttackDelay`, Weapon.cpp:2949).

use std::sync::Arc;
use zr_sim::bonus::{condition, WeaponBonusField, WeaponBonusSet};
use zr_sim::damage::DamageType;
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::weapon::{CombatTemplate, Prefire, WeaponSetTemplate, WeaponTemplate};
use zr_sim::{Command, Coord3D, GameLogic, ObjectId, ObjectTemplate, TerrainLogic};

fn flat() -> TerrainLogic {
    TerrainLogic::flat(100, 100, 0)
}

/// A unit with `weapon` in its first slot and no armour.
fn shooter(name: &str, weapon: WeaponTemplate) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
    t.geometry.major_radius = 8.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.vision_range = 200.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    let mut set = WeaponSetTemplate::default();
    set.weapons[0] = Some(Arc::new(weapon));
    t.combat = CombatTemplate { weapon_set: Some(set), ..CombatTemplate::default() };
    t
}

/// A target that survives the whole test, with no weapon.
fn sturdy_target() -> ObjectTemplate {
    let mut t = shooter("Target", WeaponTemplate::new("None"));
    t.combat = CombatTemplate::default();
    t.max_health = 10_000.0;
    t.initial_health = 10_000.0;
    t
}

/// 10 damage every 30 frames, range 150. Three shots at one victim reach the first continuous-fire level and six
/// the second; the MEAN level has a damage bonus of 1.5, and the coast is long enough that a run never spins down.
fn continuous_gun() -> WeaponTemplate {
    let mut w = WeaponTemplate::new("Gatling");
    w.primary_damage = 10.0;
    w.damage_type = DamageType::SmallArms;
    w.attack_range = 150.0;
    w.min_delay_between_shots = 30;
    w.max_delay_between_shots = 30;
    w.continuous_fire_one_shots_needed = 3;
    w.continuous_fire_two_shots_needed = 6;
    w.continuous_fire_coast_frames = 300;
    w.weapon_bonus = WeaponBonusSet { entries: vec![(condition::CONTINUOUS_FIRE_MEAN, WeaponBonusField::Damage.index() as u8, 1.5)] };
    w
}

/// An instant gun with no delay between shots, so the attack fires every frame until it is stopped. PerAttack
/// prefire of 20 frames: the first shot of an attack pays it, a later shot at the same victim does not.
fn prefire_gun() -> WeaponTemplate {
    let mut w = WeaponTemplate::new("Pistol");
    w.primary_damage = 1.0;
    w.damage_type = DamageType::SmallArms;
    w.attack_range = 150.0;
    w.prefire = Prefire::PerAttack;
    w.pre_attack_delay = 20;
    w
}

/// Runs frames until the victim has lost `count` health steps. Returns each step's size and the frame it was seen on.
fn shots_on(g: &mut GameLogic, victim: ObjectId, count: usize, max_frames: u32) -> Vec<(f32, u32)> {
    let mut out = Vec::new();
    let mut last = g.find_object(victim).unwrap().health();
    for _ in 0..max_frames {
        g.run_frames(1);
        let h = g.find_object(victim).unwrap().health();
        if h < last {
            out.push((last - h, g.frame()));
            last = h;
            if out.len() == count {
                break;
            }
        }
    }
    out
}

/// The shooter and the target, 60 apart on a flat map.
fn duel(shooter_t: ObjectTemplate) -> (GameLogic, ObjectId, ObjectId) {
    let mut g = GameLogic::new(flat(), 1);
    let a = g.create_object(&Arc::new(shooter_t), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(sturdy_target()), Coord3D::new(260.0, 200.0, 0.0), 0.0, 1);
    (g, a, b)
}

/// `FiringTracker::shotFired` (FiringTracker.cpp:136-158) lifts the shooter to MEAN after the fourth shot and to FAST
/// after the seventh, and `Weapon::computeBonus` reads that MEAN flag (`weapon_bonus_flags`, upgrade.rs). The
/// bonus is computed before the shot's own `shotFired`, so shots 5, 6 and 7 hit at 1.5x and shot 8 (FAST, no
/// entry) at 1x. Without the `shotFired` call in `fire_weapon` no flag is ever set and every shot is 10; without
/// the `weapon_bonus_flags` OR the flag is set but never read, and every shot is again 10.
#[test]
fn the_mean_level_follows_the_shot_count_and_raises_the_damage() {
    let (mut g, a, b) = duel(shooter("Gatling", continuous_gun()));
    g.submit(0, Command::Attack { ids: vec![a], target: b });
    let dmg: Vec<f32> = shots_on(&mut g, b, 8, 600).into_iter().map(|(d, _)| d).collect();
    assert_eq!(dmg, vec![10.0, 10.0, 10.0, 10.0, 15.0, 15.0, 15.0, 10.0]);
}

/// PerAttack prefire (Weapon.cpp:2949): the attack's first shot pays the 20-frame pre-attack. After a stop, a new
/// attack on the same victim finds the tracker's count of 1 and does not pay it again. The order's own count is
/// zero for the new order, so counting by order would make the second shot wait the full 20 frames.
#[test]
fn a_new_attack_on_the_same_victim_skips_the_per_attack_prefire() {
    let (mut g, a, b) = duel(shooter("Pistol", prefire_gun()));
    g.submit(0, Command::Attack { ids: vec![a], target: b });
    let first = shots_on(&mut g, b, 1, 200);
    assert_eq!(first.len(), 1);
    let first_frame = first[0].1;
    assert!(first_frame >= 20, "the first attack pays its prefire (shot on frame {first_frame})");

    g.submit(0, Command::Stop { ids: vec![a] });
    g.run_frames(2);
    let t0 = g.frame();
    g.submit(0, Command::Attack { ids: vec![a], target: b });
    let second = shots_on(&mut g, b, 1, 200);
    assert_eq!(second.len(), 1);
    let wait = second[0].1 - t0;
    assert!(wait < 20, "a second attack on the same victim skips the prefire (waited {wait} frames)");
}
