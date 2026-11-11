//! The R9 review follow-ups (wave 15, lane 116): `DAMAGE_STATUS` with a zero duration and on the
//! `WEAPON_KILLS_SELF` path, the subdual hit's last-damage bookkeeping, `onSubdualChange`'s passenger orders,
//! the `FiringTracker`'s per-caller coverage and the zero `PRE_ATTACK` bonus. Synthetic data only.

use std::collections::BTreeMap;
use std::sync::Arc;

use zr_sim::ai_states::AiStateType;
use zr_sim::body::DamageInfo;
use zr_sim::contain::{ContainKind, ContainTemplate, GarrisonFirePoints};
use zr_sim::damage::DamageType;
use zr_sim::disabled::DisabledType;
use zr_sim::hack_internet::{HackInternetTemplate, HackPhase};
use zr_sim::object::status;
use zr_sim::ocl::{DeliverPayloadTemplate, OclNugget, OclTemplate};
use zr_sim::special_power::{SimSpecialPower, SimSpecialPowerTemplate, SpecialPowerEffect, SpecialPowerTarget};
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::weapon::{affects, CombatTemplate, Weapon, WeaponSetTemplate, WeaponStatus, WeaponTemplate};
use zr_sim::{Command, Coord3D, GameLogic, ObjectId, ObjectTemplate, TerrainLogic};

fn flat() -> TerrainLogic {
    TerrainLogic::flat(200, 200, 0)
}

fn weapon_set(w: WeaponTemplate) -> CombatTemplate {
    let mut set = WeaponSetTemplate::default();
    set.weapons[0] = Some(Arc::new(w));
    CombatTemplate { weapon_set: Some(set), ..CombatTemplate::default() }
}

fn tank(name: &str, w: WeaponTemplate) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
    t.geometry.major_radius = 8.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    t.combat = weapon_set(w);
    t
}

fn dummy(name: &str, health: f32) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 8.0;
    t.max_health = health;
    t.initial_health = health;
    t
}

fn cannon() -> WeaponTemplate {
    let mut w = WeaponTemplate::new("Cannon");
    w.primary_damage = 10.0;
    w.damage_type = DamageType::Explosion;
    w.attack_range = 300.0;
    w.min_delay_between_shots = 1;
    w.max_delay_between_shots = 1;
    w
}

fn subdue(amount: f32) -> DamageInfo {
    DamageInfo { source: ObjectId::INVALID, amount, kind: DamageType::SubdualVehicle }
}

// ---------------------------------------------------------------------------------------------------------
// The zero PRE_ATTACK bonus (`WeaponTemplate::getPreAttackDelay`, Weapon.cpp:528-531).
// ---------------------------------------------------------------------------------------------------------

/// `WeaponTemplate::getPreAttackDelay` is `m_preAttackDelay * bonus.PRE_ATTACK` with no fallback, and
/// `Weapon::preFireWeapon` starts a pre-attack only when the delay is positive (Weapon.cpp:2795-2807). A
/// `PRE_ATTACK` bonus of 0 (a `WeaponBonusSet` entry at 0%) therefore gives no pre-attack at all; the old
/// `1.0` substitution gave the full `PreAttackDelay`.
#[test]
fn a_zero_pre_attack_bonus_gives_no_pre_attack() {
    let mut t = WeaponTemplate::new("Slow");
    t.pre_attack_delay = 15;
    t.clip_size = 3;
    t.attack_range = 100.0;
    let mut w = Weapon::new(Arc::new(t), 0);
    w.load_ammo_now(0);
    assert_eq!(w.pre_attack_delay_with(0, 0.0), 0, "0 * PreAttackDelay is 0, not the base delay");
    assert_eq!(w.pre_attack_delay_with(0, 1.0), 15, "the default bonus is 1.0 (WeaponBonus::clear)");
    assert_eq!(w.pre_attack_delay_with(0, 2.0), 30);
    w.pre_fire_with(0, 0, 0.0);
    assert_ne!(w.status(0), WeaponStatus::PreAttack, "no delay, no pre-attack state");
    w.pre_fire_with(0, 0, 1.0);
    assert_eq!(w.status(0), WeaponStatus::PreAttack);
}

// ---------------------------------------------------------------------------------------------------------
// Finding 3: the `WEAPON_KILLS_SELF` path is an `attemptDamage` hit too.
// ---------------------------------------------------------------------------------------------------------

/// `WeaponTemplate::dealDamageInternal`'s no-radius `WEAPON_KILLS_SELF` branch (Weapon.cpp:1401-1412) builds a
/// `DamageInfo` with the weapon's damage type *and* status type, so `ActiveBody::attemptDamage`'s `DAMAGE_STATUS`
/// case (ActiveBody.cpp:483-490) applies it as a status, not as health damage. The port applied
/// `DamageType::Status` as health damage, which the body drops (0), so the status never appeared.
#[test]
fn a_status_self_hit_is_status_damage() {
    let mut g = GameLogic::new(flat(), 1);
    let mut w = WeaponTemplate::new("SelfStun");
    w.primary_damage = 500.0;
    w.damage_type = DamageType::Status;
    w.damage_status_mask = status::IMMOBILE;
    w.attack_range = 200.0;
    w.min_delay_between_shots = 1000;
    w.max_delay_between_shots = 1000;
    w.affects = affects::ENEMIES | affects::KILLS_SELF;
    let a = g.create_object(&Arc::new(tank("A", w)), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(dummy("B", 1000.0)), Coord3D::new(160.0, 100.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(0, Command::Attack { ids: vec![a], target: b });
    g.run_frames(20);
    let a_obj = g.find_object(a).unwrap();
    assert!(a_obj.status_mask(g.frame()) & status::IMMOBILE != 0, "the self-hit sets the status for its huge duration");
    assert_eq!(a_obj.health(), 100.0, "DAMAGE_STATUS does no health damage, not even to the shooter");
    assert_eq!(g.find_object(b).unwrap().health(), 1000.0, "the KILLS_SELF branch returns before the victim loop");
}

// ---------------------------------------------------------------------------------------------------------
// Finding 3: a status hit falls through to the last-damage record.
// ---------------------------------------------------------------------------------------------------------

/// `ActiveBody::attemptDamage`'s `DAMAGE_STATUS` case leaves `alreadyHandled` true but still runs the
/// `amount > 0` block (`ActiveBody.cpp:527-600`), which records `m_lastDamageInfo` and calls `setAttackedBy`.
/// The port applied the status but never recorded the attacker, so a guard could not react to a jammer.
#[test]
fn a_status_hit_records_the_last_damage() {
    let mut g = GameLogic::new(flat(), 1);
    let mut w = WeaponTemplate::new("Jam");
    w.primary_damage = 2000.0;
    w.damage_type = DamageType::Status;
    w.damage_status_mask = status::IMMOBILE;
    w.attack_range = 200.0;
    w.min_delay_between_shots = 1000;
    w.max_delay_between_shots = 1000;
    let a = g.create_object(&Arc::new(tank("A", w)), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(dummy("B", 1000.0)), Coord3D::new(160.0, 100.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(0, Command::Attack { ids: vec![a], target: b });
    g.run_frames(20);
    let bo = g.find_object(b).unwrap();
    assert!(bo.status_mask(g.frame()) & status::IMMOBILE != 0, "the status hit landed");
    assert_eq!(bo.health(), 1000.0, "no health damage");
    assert_eq!(bo.last_damage_info.source, a, "the status hit is the victim's last damage");
}

// ---------------------------------------------------------------------------------------------------------
// Finding 2: a subdual hit falls through to the last-damage record.
// ---------------------------------------------------------------------------------------------------------

/// `ActiveBody::attemptDamage`'s record block is gated on `amount > 0 || kill` (`ActiveBody.cpp:527`), so a hit whose
/// armour-adjusted amount is zero (or that lands on an already-dead body, which returns at `:357`) keeps the previous
/// attacker. The port recorded every hit that had a source.
#[test]
fn a_zero_damage_hit_does_not_record_the_attacker() {
    let mut g = GameLogic::new(flat(), 1);
    let id = g.create_object(&Arc::new(dummy("Target", 100.0)), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let attacker = g.create_object(&Arc::new(dummy("Attacker", 100.0)), Coord3D::new(150.0, 100.0, 0.0), 0.0, 1);
    g.damage_object(id, DamageInfo { source: attacker, amount: 0.0, kind: DamageType::SmallArms });
    assert_eq!(g.find_object(id).unwrap().last_damage_info.source, ObjectId::INVALID, "a zero hit is not a damage record");
    g.damage_object(id, DamageInfo { source: attacker, amount: 10.0, kind: DamageType::SmallArms });
    assert_eq!(g.find_object(id).unwrap().last_damage_info.source, attacker);
}

/// `ActiveBody::attemptDamage` sets `alreadyHandled` for subdual damage and then falls through to the
/// `amount > 0` block (ActiveBody.cpp:495-600), which records `m_lastDamageInfo`/`m_lastDamageTimestamp` and
/// `setAttackedBy`; only `internalChangeHealth` (and the health-decrease damage callbacks) are skipped. The port
/// skipped the record for every subdual hit, so a guard could not tell who subdued it.
#[test]
fn a_subdual_hit_records_the_last_damage() {
    let mut g = GameLogic::new(flat(), 1);
    let mut t = dummy("Subduable", 100.0);
    t.subdual_damage_cap = 1000.0;
    t.subdual_heal_rate = 100;
    t.subdual_heal_amount = 50.0;
    let id = g.create_object(&Arc::new(t), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let attacker = g.create_object(&Arc::new(dummy("Attacker", 100.0)), Coord3D::new(150.0, 100.0, 0.0), 0.0, 1);
    g.damage_object(id, DamageInfo { source: attacker, amount: 100.0, kind: DamageType::SubdualVehicle });
    let o = g.find_object(id).unwrap();
    assert_eq!(o.last_damage_info.source, attacker, "the subdual hit is the last damage");
    assert_eq!(o.health(), 100.0, "the hit still does no health damage");
    assert!(o.is_disabled_by_type(DisabledType::Subdued));
    // A heal is a negative subdual hit: the C++ `amount > 0` gate keeps it out of the record, so the attacker stays.
    let other = g.create_object(&Arc::new(dummy("Healer", 100.0)), Coord3D::new(60.0, 100.0, 0.0), 0.0, 1);
    g.damage_object(id, DamageInfo { source: other, amount: -500.0, kind: DamageType::SubdualUnresistable });
    assert_eq!(g.find_object(id).unwrap().last_damage_info.source, attacker, "a subdual heal does not record");
}

// ---------------------------------------------------------------------------------------------------------
// Finding 4: the strafe is an `Object::fireCurrentWeapon(pos)` path.
// ---------------------------------------------------------------------------------------------------------

/// `DeliverPayloadAIUpdate::update` locks the strafing slot and calls `getObject()->fireCurrentWeapon(&strafePoint)`
/// (DeliverPayloadAIUpdate.cpp:224-226). `Object::fireCurrentWeapon` only fires a `READY_TO_FIRE` weapon
/// (Object.cpp:1505 and :1528) and records the shot with `INVALID_ID` as the victim (a position shot, Object.cpp:1532).
/// The port fired every frame it was in range and recorded the delivery's target object as the victim.
#[test]
fn a_strafe_waits_for_its_weapon_and_tracks_a_position_shot() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(1, 0);
    install_power(&mut g, "Strafe", 0);
    let mut gun = WeaponTemplate::new("StrafeGun");
    gun.primary_damage = 10.0;
    gun.damage_type = DamageType::Explosion;
    gun.primary_damage_radius = 30.0;
    gun.secondary_damage_radius = 30.0;
    gun.attack_range = 300.0;
    gun.min_delay_between_shots = 1000;
    gun.max_delay_between_shots = 1000;
    gun.clip_size = 1;
    gun.auto_reloads_clip = false;
    let mut set = WeaponSetTemplate::default();
    set.weapons[0] = Some(Arc::new(gun));
    let mut transport = tank("Transport", cannon());
    transport.combat = CombatTemplate { weapon_set: Some(set), ..CombatTemplate::default() };
    let ocl = Arc::new(OclTemplate {
        name: "Strafe".into(),
        nuggets: vec![OclNugget::DeliverPayload(Arc::new(DeliverPayloadTemplate {
            transport: Some(Arc::new(transport.clone())),
            payload: Vec::new(),
            delivery_distance: 300.0,
            drop_delay: 5,
            delay_delivery_frames_max: 0,
            drop_offset: (0.0, 0.0, 0.0),
            drop_variance: (0.0, 0.0, 0.0),
            is_parachute_directly: false,
            self_destruct_object: false,
            fire_weapon: true,
            strafing_weapon_slot: 0,
            strafe_length: 0.0,
        }))],
    });
    transport.special_powers = vec![Arc::new(SimSpecialPower {
        power: "Strafe".into(),
        effect: SpecialPowerEffect::Ocl { ocl: Some(ocl), create_at_target: true, create_location: zr_sim::ocl::CreateLocation::AtLocation },
        starts_paused: false,
        power_module: true,
    })];
    let src = g.create_object(&Arc::new(transport), Coord3D::new(50.0, 100.0, 0.0), 0.0, 0);
    // The target is at x=600 so the transport's start, `DeliveryDistance * 1.5` back along the approach
    // (ObjectCreationList.cpp:348-352), is on the map (x=150). A transport that is off the map once its exit is
    // ordered is removed (`HeadOffMapState::update`, DeliverPayloadAIUpdate.cpp:1177-1181), so an off-map start
    // would never strafe (lane 189).
    let b = g.create_object(&Arc::new(dummy("Target", 100_000.0)), Coord3D::new(600.0, 100.0, 0.0), 0.0, 1);
    g.submit(0, Command::DoSpecialPower { source: src, power: "Strafe".into(), target: SpecialPowerTarget::Object(b) });
    let mut shots = 0;
    // The transport starts `DeliveryDistance * 1.5` before the primary point (ObjectCreationList.cpp:348-352, lane 185), so
    // its approach to the target is longer than the old 300-unit start: the window covers the flight and the exit.
    for _ in 0..600 {
        g.run_frames(1);
        if let Some(t) = g.objects().find(|o| o.template.name == "Transport" && o.id != src) {
            shots = shots.max(t.combat.helpers.firing.consecutive_shots);
        }
    }
    let target = g.find_object(b).unwrap();
    assert_eq!(target.health(), 100_000.0 - 10.0, "a one-shot clip fires once, not on every in-range frame");
    let t = g.objects().find(|o| o.template.name == "Transport" && o.id != src).expect("the transport exists for the run");
    assert_eq!(t.combat.weapons.weapon(0).unwrap().ammo_in_clip(), 0, "the clip is spent once, never driven negative");
    assert_eq!(t.combat.helpers.firing.victim, ObjectId::INVALID, "a position shot has no tracker victim");
    assert!(shots <= 1, "at most one tracked shot, got {shots}");
}

// ---------------------------------------------------------------------------------------------------------
// `FireWeaponPower`'s `reloadAllAmmo(TRUE)`.
// ---------------------------------------------------------------------------------------------------------

fn install_power(g: &mut GameLogic, name: &str, reload: u32) {
    let mut map = BTreeMap::new();
    map.insert(
        name.to_string(),
        Arc::new(SimSpecialPowerTemplate {
            name: name.to_string(),
            kind: String::new(),
            reload_time: reload,
            required_science: None,
            shared_synced_timer: false,
            public_timer: false,
            radius_cursor_radius: 0.0,
        }),
    );
    g.set_special_powers(map);
}

/// `FireWeaponPower::doSpecialPowerAtObject` calls `self->reloadAllAmmo(TRUE)` before `aiAttackObject`
/// (FireWeaponPower.cpp:161-162), so the power always fires a full clip (`WeaponSet::reloadAllAmmo(obj, TRUE)`
/// -> `Weapon::loadAmmoNow`, WeaponSet.cpp:995-1008). The port fired with whatever was left in the clip.
#[test]
fn the_fire_weapon_power_reloads_its_clip_first() {
    let mut g = GameLogic::new(flat(), 1);
    install_power(&mut g, "P", 1);
    let mut gun = cannon();
    gun.clip_size = 3;
    gun.auto_reloads_clip = false;
    gun.min_delay_between_shots = 2;
    gun.max_delay_between_shots = 2;
    let mut a = tank("A", gun);
    a.special_powers = vec![Arc::new(SimSpecialPower {
        power: "P".into(),
        effect: SpecialPowerEffect::FireWeapon,
        starts_paused: false,
        power_module: true,
    })];
    let ia = g.create_object(&Arc::new(a), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(dummy("B", 1000.0)), Coord3D::new(160.0, 100.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(0, Command::Attack { ids: vec![ia], target: b });
    g.run_frames(200);
    assert_eq!(g.find_object(ia).unwrap().combat.weapons.weapon(0).unwrap().ammo_in_clip(), 0, "the attack emptied the 3-shot clip");
    assert_eq!(g.find_object(b).unwrap().health(), 970.0);
    g.submit(0, Command::DoSpecialPower { source: ia, power: "P".into(), target: SpecialPowerTarget::Object(b) });
    g.run_frames(3);
    assert_eq!(g.find_object(b).unwrap().health(), 960.0, "the power fired");
    assert_eq!(g.find_object(ia).unwrap().combat.weapons.weapon(0).unwrap().ammo_in_clip(), 2, "reload 3 then fire one");
}

// ---------------------------------------------------------------------------------------------------------
// Finding 9: `onSubdualChange`'s passenger orders.
// ---------------------------------------------------------------------------------------------------------

fn container_contain(kind: ContainKind) -> ContainTemplate {
    ContainTemplate {
        kind,
        capacity: 5,
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
        heal_frames: 1.0,
        initial_payload: Vec::new(),
        tunnel_frames_for_full_heal: 1.0,
        is_enclosing_container: true,
        fire_points: GarrisonFirePoints::default(),
        passengers_in_turret: false,
        weapon_bonus_passed_to_passengers: false,
        immune_to_clear_building_attacks: false,
    }
}

/// A 100-health, 1000-cap container that heals 300 subdual damage every frame.
fn subduable_building(name: &str, kind: u64, contain: ContainKind) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE | kind_of::SELECTABLE | kind;
    t.geometry.major_radius = 30.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.subdual_damage_cap = 1000.0;
    t.subdual_heal_rate = 1;
    t.subdual_heal_amount = 300.0;
    t.contain = Some(Arc::new(container_contain(contain)));
    t
}

fn infantry(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::INFANTRY | kind_of::SELECTABLE;
    t.geometry.major_radius = 4.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.transport_slot_count = 1;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Legs, 20.0, 60.0, 120.0, 180.0));
    t
}

/// `ActiveBody::onSubdualChange`'s `isNowSubdued` branch (ActiveBody.cpp:1285-1292): `setDisabled(DISABLED_SUBDUED)`
/// then `contain->orderAllPassengersToIdle(CMD_FROM_AI)` (`OpenContain.cpp:1479-1491`): every passenger with an AI
/// goes idle. The port only set the disable, so a garrisoned unit kept its attack order.
#[test]
fn a_subdued_container_orders_its_passengers_idle() {
    let mut g = GameLogic::new(flat(), 1);
    let bld = g.create_object(&Arc::new(subduable_building("Bunker", 0, ContainKind::Garrison)), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let mut ranger = infantry("Ranger");
    ranger.combat = weapon_set(cannon());
    let p = g.create_object(&Arc::new(ranger), Coord3D::new(140.0, 100.0, 0.0), 0.0, 0);
    let victim = g.create_object(&Arc::new(dummy("Victim", 1000.0)), Coord3D::new(60.0, 100.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(0, Command::Enter { ids: vec![p], container: bld });
    g.run_frames(200);
    assert!(g.is_contained(p), "the ranger boarded");
    g.submit(0, Command::Attack { ids: vec![p], target: victim });
    g.run_frames(1);
    assert!(g.find_object(p).unwrap().combat.attack.is_some(), "the passenger has an attack order");
    g.damage_object(bld, subdue(100.0));
    let po = g.find_object(p).unwrap();
    assert_eq!(po.combat.attack, None, "the subdued container's passenger drops its order");
    assert_eq!(po.ai.machine.current, AiStateType::Idle);
    // The heal takes the container out of subdual and clears its disable (the sleepy loop's `on_subdual_change`).
    for _ in 0..10 {
        g.run_frames(1);
        if !g.find_object(bld).unwrap().is_disabled_by_type(DisabledType::Subdued) {
            break;
        }
    }
    assert!(!g.find_object(bld).unwrap().is_disabled_by_type(DisabledType::Subdued), "the heal clears DISABLED_SUBDUED");
    assert!(!g.find_object(bld).unwrap().body.as_ref().unwrap().is_subdued());
}

/// `ActiveBody::onSubdualChange`'s cleared branch (ActiveBody.cpp:1301-1311, patch 1.01): an internet center that
/// stops being subdued orders its contained money hackers to hack again (`orderAllPassengersToHackInternet`,
/// `OpenContain.cpp:1494-1509`). The port omitted the order, so a hacker stayed idle.
#[test]
fn an_internet_center_leaving_subdual_restarts_its_hackers() {
    fn money_hacker() -> ObjectTemplate {
        let mut t = infantry("Hacker");
        t.kind_of |= kind_of::MONEY_HACKER;
        t.hack_internet = Some(HackInternetTemplate { unpack_frames: 2, pack_frames: 2, cash_update_delay: 30, cash_update_delay_fast: 10, regular_cash: 1, ..HackInternetTemplate::default() });
        t
    }
    let mut g = GameLogic::new(flat(), 1);
    let center = g.create_object(
        &Arc::new(subduable_building("InternetCenter", kind_of::FS_INTERNET_CENTER, ContainKind::Transport)),
        Coord3D::new(100.0, 100.0, 0.0),
        0.0,
        0,
    );
    let h = g.create_object(&Arc::new(money_hacker()), Coord3D::new(140.0, 100.0, 0.0), 0.0, 0);
    g.submit(0, Command::Enter { ids: vec![h], container: center });
    g.run_frames(200);
    assert!(g.is_contained(h), "the hacker went inside");
    assert_eq!(g.find_object(h).unwrap().hack_internet.as_ref().map(|s| s.phase), None, "the port does not start a hack on containing");
    g.damage_object(center, subdue(100.0));
    assert!(g.find_object(center).unwrap().is_disabled_by_type(DisabledType::Subdued));
    let mut restarted = false;
    for _ in 0..10 {
        g.run_frames(1);
        if let Some(phase) = g.find_object(h).unwrap().hack_internet.as_ref().map(|s| s.phase) {
            restarted = !matches!(phase, HackPhase::Idle);
            if restarted {
                break;
            }
        }
    }
    assert!(!g.find_object(center).unwrap().is_disabled_by_type(DisabledType::Subdued), "the center healed out of subdual");
    assert!(restarted, "the center ordered its contained money hacker to hack again");
}
