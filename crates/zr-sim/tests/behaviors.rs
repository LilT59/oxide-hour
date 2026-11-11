//! Common gameplay behaviors: `AutoHealBehavior`, `FlammableUpdate`, `PoisonedBehavior`,
//! `FireWeaponWhenDeadBehavior`, `LifetimeUpdate`, stealth and the new upgrade modules. Synthetic.

use std::sync::Arc;

use zr_sim::behaviors::{AutoHealTemplate, FlammableTemplate, PoisonedTemplate, StealthDetectorTemplate, StealthTemplate};
use zr_sim::body::DamageInfo;
use zr_sim::damage::DamageType;
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::upgrade::{SimUpgradeModule, UpgradeEffect};
use zr_sim::weapon::WeaponTemplate;
use zr_sim::{Command, Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

fn flat() -> TerrainLogic {
    TerrainLogic::flat(100, 100, 0)
}

fn unit(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 8.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    t
}

fn explosive(amount: f32) -> DamageInfo {
    DamageInfo { source: zr_sim::ObjectId::INVALID, amount, kind: DamageType::Explosion }
}

/// `LifetimeUpdate`: the object is killed when its lifetime elapses.
#[test]
fn lifetime_update_kills_after_lifetime() {
    let mut g = GameLogic::new(flat(), 1);
    let mut t = unit("Mine");
    t.lifetime = Some((10, 10));
    let id = g.create_object(&Arc::new(t), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    g.run_frames(9);
    assert!(g.find_object(id).is_some(), "still alive before the lifetime");
    g.run_frames(2);
    assert!(g.find_object(id).is_none(), "killed at the lifetime");
}

/// `DeletionUpdate`: the object is *destroyed* (removed without the death pipeline) when its
/// lifetime elapses.
#[test]
fn deletion_update_destroys_after_lifetime() {
    let mut g = GameLogic::new(flat(), 1);
    let mut t = unit("Ping");
    t.deletion = Some((10, 10));
    let id = g.create_object(&Arc::new(t), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    g.run_frames(9);
    assert!(g.find_object(id).is_some(), "still alive before the lifetime");
    g.run_frames(2);
    let removed = g.find_object(id).is_none();
    assert!(removed, "destroyed at the lifetime");
}

/// `AutoHealBehavior`: a damaged object heals itself on the delay. The module must start active: without
/// StartsActive and without a TriggeredBy upgrade it never activates (AutoHealBehavior.cpp:106-109 sleeps it
/// forever, and `UpgradeModule::wouldUpgrade` needs an activation mask, Object/Upgrade/UpgradeModule.cpp:124-150).
#[test]
fn auto_heal_heals_damaged_object() {
    let mut g = GameLogic::new(flat(), 1);
    let mut t = unit("Healer");
    t.auto_heal = vec![AutoHealTemplate {
        healing_amount: 10.0,
        healing_delay: 30,
        start_healing_delay: 0,
        radius: 0.0,
        affects_whole_player: false,
        skip_self_for_healing: false,
        single_burst: false,
        kind_of: u64::MAX,
        forbidden_kind_of: 0,
        starts_active: true,
        triggered_by: vec![],
        conflicts_with: vec![],
    }];
    let id = g.create_object(&Arc::new(t), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    g.damage_object(id, explosive(50.0));
    let before = g.find_object(id).unwrap().health();
    g.run_frames(31);
    assert!(g.find_object(id).unwrap().health() > before, "healed after the delay");
}

/// `FlammableUpdate`: enough flame damage ignites and the burn ticks health down.
#[test]
fn flammable_ignites_and_burns() {
    let mut g = GameLogic::new(flat(), 1);
    let mut t = unit("Flamer");
    t.flammable = Some(FlammableTemplate {
        burned_delay: 0,
        aflame_duration: 300,
        aflame_damage_delay: 30,
        aflame_damage_amount: 10.0,
        flame_damage_limit: 20.0,
        flame_damage_expiration: 60,
    });
    let id = g.create_object(&Arc::new(t), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    // 20 points of flame damage reaches `FlameDamageLimit`, igniting it.
    g.damage_object(id, DamageInfo { source: zr_sim::ObjectId::INVALID, amount: 20.0, kind: DamageType::Flame });
    let after_ignite = g.find_object(id).unwrap().health();
    g.run_frames(31);
    assert!(g.find_object(id).unwrap().health() < after_ignite, "the burn ticks damage");
}

/// `PoisonedBehavior`: poison damage starts a DoT that ticks.
#[test]
fn poisoned_applies_dot() {
    let mut g = GameLogic::new(flat(), 1);
    let mut t = unit("Victim");
    t.poisoned = Some(PoisonedTemplate { poison_damage_interval: 30, poison_duration: 300 });
    let id = g.create_object(&Arc::new(t), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    g.damage_object(id, DamageInfo { source: zr_sim::ObjectId::INVALID, amount: 5.0, kind: DamageType::Poison });
    let after = g.find_object(id).unwrap().health();
    g.run_frames(31);
    assert!(g.find_object(id).unwrap().health() < after, "poison ticks");
}

/// `FireWeaponWhenDeadBehavior`: the death weapon fires on death.
#[test]
fn fire_weapon_when_dead_fires() {
    let mut g = GameLogic::new(flat(), 1);
    let mut w = WeaponTemplate::new("DeathBomb");
    w.primary_damage = 10.0;
    w.damage_type = DamageType::Explosion;
    w.attack_range = 300.0;
    w.has_projectile = true;
    w.weapon_speed = 60.0;
    let mut t = unit("Bomber");
    t.fire_weapon_when_dead = Some(zr_sim::behaviors::DeathWeaponTemplate {
        weapon: Arc::new(w),
        death_types: u32::MAX,
        // StartsActive: without it (and without a TriggeredBy) the module never activates, and so never fires
        // (FireWeaponWhenDeadBehavior.h:50 `m_initiallyActive` false, FireWeaponWhenDeadBehavior.cpp:83 `isUpgradeActive`).
        starts_active: true,
        triggered_by: vec![],
        conflicts_with: vec![],
    });
    let id = g.create_object(&Arc::new(t), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    g.damage_object(id, explosive(200.0));
    assert!(!g.projectiles().is_empty(), "the death weapon spawned a projectile");
}

/// `StealthUpdate` + `StealthDetectorUpdate`: a stealthed enemy is not auto-acquired until detected.
#[test]
fn stealth_hides_from_auto_acquire_and_detector_reveals() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 1000);
    g.add_player(1, 1000);
    g.set_enemies(0, 1);
    // A stealthy enemy sits in range of a gun.
    let mut st = unit("Stealth");
    st.stealth = Some(StealthTemplate {
        stealth_delay: 0,
        move_threshold_speed: 0.0,
        forbidden_conditions: 0,
        required_status: 0,
        forbidden_status: 0,
        innate_stealth: true,
        granted_by_special_power: false,
        disguises_as_team: false,
        reveal_distance_from_target: 0.0,
        order_idle_enemies_to_attack_on_reveal: false,
        use_rider_stealth: false,
    });
    let hidden = g.create_object(&Arc::new(st), Coord3D::new(260.0, 200.0, 0.0), 0.0, 1);
    // The stealth module's first update is frame 1 (StealthUpdate.cpp:157, `setWakeFrame(NONE)` in the constructor),
    // after that frame's AI pass. The enemy is stealthed before the gun exists, so the gun's first scan sees it hidden.
    g.run_frames(2);
    // A gun that auto-acquires.
    let mut gt = unit("Gun");
    gt.kind_of |= kind_of::CAN_ATTACK;
    gt.combat.auto_acquire_enemies = true;
    gt.combat.auto_acquire_buildings = false;
    gt.vision_range = 400.0;
    let mut wt = WeaponTemplate::new("Gun");
    wt.primary_damage = 10.0;
    wt.damage_type = DamageType::Explosion;
    wt.attack_range = 400.0;
    let mut ws = zr_sim::weapon::WeaponSetTemplate::default();
    ws.weapons[0] = Some(Arc::new(wt));
    gt.combat.weapon_set = Some(ws);
    let gun = g.create_object(&Arc::new(gt), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    g.run_frames(10);
    assert!(g.is_stealthed(hidden), "the enemy is stealthed");
    assert!(g.find_object(gun).unwrap().combat.attack.is_none(), "the gun ignores the stealthed enemy");
    // A detector next to the gun reveals it.
    let mut dt = unit("Detector");
    dt.stealth_detector = Some(StealthDetectorTemplate {
        detection_rate: 1,
        detection_range: 200.0,
        initially_disabled: false,
        extra_required_kind_of: 0,
        extra_forbidden_kind_of: 0,
        can_detect_while_garrisoned: false,
        can_detect_while_contained: false,
    });
    dt.kind_of |= kind_of::STRUCTURE | kind_of::IMMOBILE;
    g.create_object(&Arc::new(dt), Coord3D::new(210.0, 200.0, 0.0), 0.0, 0);
    g.run_frames(5);
    assert!(!g.is_stealthed(hidden), "the detector revealed the enemy");
    let _ = gun;
}

/// The new upgrade modules: `ExperienceScalarUpgrade` and `WeaponBonusUpgrade` effects apply.
#[test]
fn upgrade_effects_apply() {
    let mut g = GameLogic::new(flat(), 1);
    let mut t = unit("Upgraded");
    t.upgrade_modules = vec![
        Arc::new(SimUpgradeModule {
            triggered_by: vec!["XP".into()],
            conflicts_with: vec![],
            removes_upgrades: vec![],
            requires_all_triggers: false,
            effect: UpgradeEffect::ExperienceScalar { add_xp_scalar: 0.5 },
        }),
        Arc::new(SimUpgradeModule {
            triggered_by: vec!["Bonus".into()],
            conflicts_with: vec![],
            removes_upgrades: vec![],
            requires_all_triggers: false,
            effect: UpgradeEffect::WeaponBonus,
        }),
    ];
    let id = g.create_object(&Arc::new(t), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    g.give_upgrade(id, "XP");
    g.give_upgrade(id, "Bonus");
    g.update_upgrade_modules(id);
    assert!((g.find_object(id).unwrap().experience.experience_scalar - 1.5).abs() < 1e-4);
    let _ = Command::Stop { ids: vec![id] };
}
