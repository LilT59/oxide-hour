//! The weapon-group orders (`MSG_DO_WEAPON`, `MSG_DO_WEAPON_AT_OBJECT`, `MSG_DO_WEAPON_AT_LOCATION`),
//! `MSG_SET_MINE_CLEARING_DETAIL`, `MSG_INTERNET_HACK` (`HackInternetAIUpdate`) and
//! `MSG_DO_SPECIAL_POWER_AT_LOCATION`'s build angle. Synthetic data only.
//!
//! Ports under test: `GameLogicDispatch.cpp` (`onDoWeapon`, `onDoWeaponAtObject`,
//! `onSetMineClearingDetail`, `onDoWeaponAtLocation`, `onInternetHack`, `onDoSpecialPowerAtLocation`),
//! `AIGroup.cpp` (`groupAttackPosition`, `groupAttackObjectPrivate`, `setWeaponLockForGroup`,
//! `setMineClearingDetail`, `groupHackInternet`), `HackInternetAIUpdate.cpp` and
//! `ObjectCreationList.cpp` (`doStuffToObj`, `LIKE_EXISTING`).

use std::collections::BTreeMap;
use std::sync::Arc;

use zr_sim::ai_states::AiStateType;
use zr_sim::combat::AttackTarget;
use zr_sim::damage::DamageType;
use zr_sim::disabled::DisabledType;
use zr_sim::hack_internet::{HackInternetTemplate, HackPhase};
use zr_sim::ocl::{disposition, CreateObjectTemplate, OclNugget, OclTemplate};
use zr_sim::special_power::{SimSpecialPower, SimSpecialPowerTemplate, SpecialPowerEffect, SpecialPowerTarget};
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::weapon::{CombatTemplate, WeaponSetTemplate, WeaponTemplate, WEAPONSET_MINE_CLEARING_DETAIL};
use zr_sim::{Command, Coord3D, GameLogic, ObjectId, ObjectTemplate, TemplateSource, TerrainLogic};

struct FixedStore(Vec<Arc<ObjectTemplate>>);

impl TemplateSource for FixedStore {
    fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
        self.0.iter().find(|t| t.name == name).cloned()
    }
}

fn world(templates: Vec<ObjectTemplate>) -> GameLogic {
    let store = FixedStore(templates.into_iter().map(Arc::new).collect());
    let mut g = GameLogic::new(TerrainLogic::flat(500, 500, 0), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_template_source(Arc::new(store));
    g
}

/// A ground unit with a locomotor and the given weapon set.
fn tank(name: &str, weapons: [Option<WeaponTemplate>; 3]) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
    t.geometry.major_radius = 5.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 60.0, 300.0, 300.0, 360.0));
    let mut set = WeaponSetTemplate::default();
    for (i, w) in weapons.into_iter().enumerate() {
        set.weapons[i] = w.map(Arc::new);
    }
    t.combat = CombatTemplate { weapon_set: Some(set), ..CombatTemplate::default() };
    t
}

/// A gun that fires every `delay` frames from a clip of `clip` shots, which does not reload during a
/// test.
fn gun(name: &str, clip: i32) -> WeaponTemplate {
    let mut w = WeaponTemplate::new(name);
    w.primary_damage = 10.0;
    w.damage_type = DamageType::Explosion;
    w.attack_range = 100.0;
    w.min_delay_between_shots = 5;
    w.max_delay_between_shots = 5;
    w.clip_size = clip;
    w.clip_reload_time = 100_000;
    w
}

fn ammo(g: &GameLogic, id: ObjectId, slot: usize) -> i32 {
    g.find_object(id).unwrap().combat.weapons.weapon(slot).unwrap().ammo_in_clip()
}

fn hacker_template() -> HackInternetTemplate {
    HackInternetTemplate {
        unpack_frames: 3,
        pack_frames: 2,
        cash_update_delay: 6,
        cash_update_delay_fast: 2,
        regular_cash: 10,
        veteran_cash: 20,
        elite_cash: 0,
        heroic_cash: 0,
        xp_per_cash_update: 0,
        pack_unpack_variation: 0.0,
    }
}

fn hacker(h: HackInternetTemplate) -> ObjectTemplate {
    let mut t = tank("Hacker", [None, None, None]);
    t.hack_internet = Some(h);
    t
}

/// `MSG_DO_WEAPON_AT_LOCATION`: slot 1 is locked and fires at most `max_shots` times; slot 0 is not
/// used (`onDoWeaponAtLocation` -> `setWeaponLockForGroup` + `groupAttackPosition`).
/// `onInternetHack` releases the temporary locks of the owner-filtered selection (`GameLogicDispatch.cpp:1559-1566`,
/// after the owner filter at `:386-400`). A unit the issuing player does not own keeps its lock.
#[test]
fn an_internet_hack_order_leaves_a_foreign_units_temporary_lock() {
    let t = tank("Tank", [Some(gun("Cannon", 10)), None, None]);
    let mut g = world(vec![t.clone()]);
    let mine = g.create_object(&Arc::new(t.clone()), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    let theirs = g.create_object(&Arc::new(t), Coord3D::new(60.0, 50.0, 0.0), 0.0, 1);
    g.submit(1, Command::DoWeaponAtLocation { ids: vec![theirs], slot: 0, pos: Coord3D::new(110.0, 50.0, 0.0), max_shots: 5 });
    g.run_frames(1);
    assert_eq!(g.find_object(theirs).unwrap().combat.weapons.temporary_lock, Some(0));
    g.submit(0, Command::HackInternet { ids: vec![mine, theirs] });
    g.run_frames(1);
    assert_eq!(g.find_object(theirs).unwrap().combat.weapons.temporary_lock, Some(0), "a foreign unit keeps its lock");
}

#[test]
fn weapon_at_location_locks_the_slot_and_limits_the_shots() {
    let t = tank("Tank", [Some(gun("Cannon", 10)), Some(gun("Missile", 10)), None]);
    let mut g = world(vec![t.clone()]);
    let u = g.create_object(&Arc::new(t), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    g.submit(0, Command::DoWeaponAtLocation { ids: vec![u], slot: 1, pos: Coord3D::new(110.0, 50.0, 0.0), max_shots: 2 });
    g.run_frames(1);
    let a = g.find_object(u).unwrap().combat.attack.expect("the attack is ordered");
    assert_eq!(a.locked_slot, Some(1), "the temporary weapon lock is slot 1");
    assert_eq!(a.max_shots, Some(2));
    g.run_frames(300);
    assert_eq!(ammo(&g, u, 1), 8, "exactly two shots from the locked slot");
    assert_eq!(ammo(&g, u, 0), 10, "the other weapon never fires");
    assert!(g.find_object(u).unwrap().combat.attack.is_none(), "the attack ends after its shots");
}

/// A slot no member carries is refused before any attack (`setWeaponLockForGroup` returns false).
#[test]
fn weapon_order_for_a_missing_slot_is_refused() {
    let t = tank("Tank", [Some(gun("Cannon", 10)), None, None]);
    let mut g = world(vec![t.clone()]);
    let u = g.create_object(&Arc::new(t), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    g.submit(0, Command::DoWeaponAtLocation { ids: vec![u], slot: 2, pos: Coord3D::new(110.0, 50.0, 0.0), max_shots: -1 });
    g.submit(0, Command::DoWeapon { ids: vec![u], slot: 1, max_shots: -1 });
    g.run_frames(3);
    assert!(g.find_object(u).unwrap().combat.attack.is_none());
}

/// `MSG_DO_WEAPON_AT_OBJECT`: the locked slot attacks the enemy object for one shot; a missing target
/// is ignored (`onDoWeaponAtObject` returns false before locking).
#[test]
fn weapon_at_object_fires_the_locked_slot_at_the_target() {
    let t = tank("Tank", [Some(gun("Cannon", 10)), Some(gun("Missile", 10)), None]);
    let mut g = world(vec![t.clone()]);
    let u = g.create_object(&Arc::new(t), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    // An unarmed target, so the enemy cannot kill the shooter before its one shot.
    let target = Arc::new(tank("Target", [None, None, None]));
    let enemy = g.create_object(&target, Coord3D::new(100.0, 50.0, 0.0), 0.0, 1);
    g.submit(0, Command::DoWeaponAtObject { ids: vec![u], slot: 1, target: ObjectId(999_999), max_shots: 1 });
    g.run_frames(2);
    assert!(g.find_object(u).unwrap().combat.attack.is_none(), "a missing target is refused");
    g.submit(0, Command::DoWeaponAtObject { ids: vec![u], slot: 1, target: enemy, max_shots: 1 });
    g.run_frames(1);
    assert!(matches!(g.find_object(u).unwrap().combat.attack.map(|a| a.target), Some(AttackTarget::Object(x)) if x == enemy));
    g.run_frames(300);
    assert_eq!(ammo(&g, u, 1), 9, "one shot from slot 1");
    assert_eq!(ammo(&g, u, 0), 10);
}

/// `MSG_DO_WEAPON`: with no target each unit attacks its own position (`groupAttackPosition(nullptr)`).
#[test]
fn weapon_without_a_target_attacks_the_units_own_position() {
    let t = tank("Tank", [Some(gun("Cannon", 10)), None, None]);
    let mut g = world(vec![t.clone()]);
    let p = Coord3D::new(70.0, 40.0, 0.0);
    let u = g.create_object(&Arc::new(t), p, 0.0, 0);
    g.submit(0, Command::DoWeapon { ids: vec![u], slot: 0, max_shots: -1 });
    g.run_frames(1);
    let a = g.find_object(u).unwrap().combat.attack.expect("ordered");
    assert_eq!(a.target, AttackTarget::Ground(g.find_object(u).unwrap().position));
    assert_eq!(a.locked_slot, Some(0));
    assert_eq!(a.max_shots, None, "a negative count is unlimited");
}

/// `MSG_SET_MINE_CLEARING_DETAIL` sets `WEAPONSET_MINE_CLEARING_DETAIL`, which re-selects the weapon
/// set (`Object::setWeaponSetFlag` -> `WeaponSet::updateWeaponSet`). Another player's unit keeps its
/// set.
#[test]
fn mine_clearing_detail_reselects_the_weapon_set() {
    let mut t = tank("Dozer", [Some(gun("Dozer", 10)), None, None]);
    let mut mines = WeaponSetTemplate::default();
    mines.weapons[0] = Some(Arc::new(gun("MineClear", 10)));
    t.weapon_sets = vec![(0, t.combat.weapon_set.clone().unwrap()), (WEAPONSET_MINE_CLEARING_DETAIL, mines)];
    let mut g = world(vec![t.clone()]);
    let u = g.create_object(&Arc::new(t.clone()), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    let foreign = g.create_object(&Arc::new(t), Coord3D::new(60.0, 50.0, 0.0), 0.0, 1);
    assert_eq!(g.find_object(u).unwrap().combat.weapons.weapon(0).unwrap().template.name, "Dozer");
    g.submit(0, Command::SetMineClearingDetail { ids: vec![u, foreign] });
    g.run_frames(1);
    let o = g.find_object(u).unwrap();
    assert_eq!(o.weapon_set_flags & WEAPONSET_MINE_CLEARING_DETAIL, WEAPONSET_MINE_CLEARING_DETAIL);
    assert_eq!(o.combat.weapons.weapon(0).unwrap().template.name, "MineClear");
    let f = g.find_object(foreign).unwrap();
    assert_eq!(f.weapon_set_flags, 0, "a foreign unit is not changed");
}

/// `MSG_INTERNET_HACK`: unpack, then hack. The first payout waits a full `CashUpdateDelay` after the
/// unpack; later payouts follow every delay (`HackInternetState::update`).
#[test]
fn hacking_unpacks_then_pays_each_delay() {
    let t = hacker(hacker_template());
    let mut g = world(vec![t.clone()]);
    let h = g.create_object(&Arc::new(t), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    g.submit(0, Command::HackInternet { ids: vec![h] });
    g.run_frames(1);
    assert_eq!(phase(&g, h), HackPhase::Unpacking);
    g.run_frames(3);
    assert_eq!(phase(&g, h), HackPhase::Hacking, "3 frames of unpack after the start frame");
    g.run_frames(6);
    assert_eq!(g.player_money(0), 0, "no payout before the first delay (6 frames)");
    g.run_frames(1);
    assert_eq!(g.player_money(0), 10, "the regular amount after one delay");
    g.run_frames(7);
    assert_eq!(g.player_money(0), 20, "and again one delay later");
}

/// A hacker held by `Disabled Hacked` does not hack (`HackInternetState::update`).
#[test]
fn a_hacked_hacker_pays_nothing() {
    let t = hacker(hacker_template());
    let mut g = world(vec![t.clone()]);
    let h = g.create_object(&Arc::new(t), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    g.submit(0, Command::HackInternet { ids: vec![h] });
    g.run_frames(4);
    g.set_disabled(h, DisabledType::Hacked);
    g.run_frames(30);
    assert_eq!(g.player_money(0), 0);
    assert_eq!(phase(&g, h), HackPhase::Hacking);
}

/// A player's move while the hacker is hacking is held: the unit packs (`PackingState`), then the move
/// runs (`aiDoCommand`'s `m_pendingCommand`).
#[test]
fn a_move_is_held_until_the_hacker_has_packed() {
    let t = hacker(hacker_template());
    let mut g = world(vec![t.clone()]);
    let h = g.create_object(&Arc::new(t), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    g.submit(0, Command::HackInternet { ids: vec![h] });
    g.run_frames(4);
    assert_eq!(phase(&g, h), HackPhase::Hacking);
    g.submit(0, Command::MoveTo { ids: vec![h], pos: Coord3D::new(300.0, 50.0, 0.0) });
    g.run_frames(1);
    assert_eq!(phase(&g, h), HackPhase::Packing);
    assert_ne!(g.find_object(h).unwrap().ai.machine.current, AiStateType::MoveTo, "the move is held");
    g.run_frames(2);
    assert_eq!(phase(&g, h), HackPhase::Idle, "packed");
    assert!(pending(&g, h), "the move waits one more frame");
    g.run_frames(1);
    assert_eq!(g.find_object(h).unwrap().ai.machine.current, AiStateType::MoveTo, "the held move runs");
    assert!(!pending(&g, h));
}

/// An order to an unpacking hacker cancels the unpack and runs at once (its move state replaces the
/// hack state, as `AIStateMachine::clear` does).
#[test]
fn a_move_cancels_the_unpack() {
    let t = hacker(hacker_template());
    let mut g = world(vec![t.clone()]);
    let h = g.create_object(&Arc::new(t), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    g.submit(0, Command::HackInternet { ids: vec![h] });
    g.run_frames(1);
    assert_eq!(phase(&g, h), HackPhase::Unpacking);
    g.submit(0, Command::MoveTo { ids: vec![h], pos: Coord3D::new(300.0, 50.0, 0.0) });
    g.run_frames(1);
    assert_eq!(phase(&g, h), HackPhase::Idle);
    assert_eq!(g.find_object(h).unwrap().ai.machine.current, AiStateType::MoveTo);
}

/// The hack order is ignored while the unit is already hacking (the `RETAIL_COMPATIBLE_CRC` bugfix in
/// `HackInternetAIUpdate::aiDoCommand`): the cash clock is not restarted.
#[test]
fn a_second_hack_order_while_hacking_is_ignored() {
    let t = hacker(hacker_template());
    let mut g = world(vec![t.clone()]);
    let h = g.create_object(&Arc::new(t), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    g.submit(0, Command::HackInternet { ids: vec![h] });
    g.run_frames(4);
    g.run_frames(5);
    g.submit(0, Command::HackInternet { ids: vec![h] });
    g.run_frames(1);
    assert_eq!(g.player_money(0), 0);
    g.run_frames(1);
    assert_eq!(g.player_money(0), 10, "the original clock pays on time");
}

/// `MSG_DO_SPECIAL_POWER_AT_LOCATION`'s angle reaches a `LIKE_EXISTING` object of the power's object
/// creation list (`OCLSpecialPower` -> `ObjectCreationList::create` -> `doStuffToObj`). Without an
/// angle the object is at 0, and `DoSpecialPower` with a position is the same as no angle.
#[test]
fn special_power_at_location_passes_the_angle_to_the_created_object() {
    let mut marker = ObjectTemplate::new("Marker");
    marker.max_health = 100.0;
    marker.initial_health = 100.0;
    let ocl = Arc::new(OclTemplate {
        name: "BuildOcl".into(),
        nuggets: vec![OclNugget::CreateObject(CreateObjectTemplate {
            is_object: true,
            names_len: 1,
            objects: vec![Some(Arc::new(marker.clone()))],
            count: 1,
            disposition: disposition::LIKE_EXISTING,
            ..CreateObjectTemplate::default()
        })],
    });
    let mut source = ObjectTemplate::new("Source");
    source.kind_of = kind_of::STRUCTURE;
    source.max_health = 500.0;
    source.initial_health = 500.0;
    source.special_powers = vec![Arc::new(SimSpecialPower {
        power: "Build".into(),
        effect: SpecialPowerEffect::Ocl { ocl: Some(ocl), create_at_target: true, create_location: zr_sim::ocl::CreateLocation::AtLocation },
        starts_paused: false,
        power_module: true,
    })];
    let mut g = world(vec![marker.clone(), source.clone()]);
    let mut powers = BTreeMap::new();
    powers.insert(
        "Build".to_string(),
        Arc::new(SimSpecialPowerTemplate {
            name: "Build".into(),
            kind: String::new(),
            reload_time: 0,
            required_science: None,
            shared_synced_timer: false,
            public_timer: false,
            radius_cursor_radius: 0.0,
        }),
    );
    g.set_special_powers(powers);
    let src = g.create_object(&Arc::new(source), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    let spot = Coord3D::new(200.0, 200.0, 0.0);
    let angles = |g: &GameLogic| -> Vec<f32> { g.objects().filter(|o| o.template.name == "Marker").map(|o| o.angle).collect() };

    g.submit(0, Command::DoSpecialPowerAtLocation { source: src, power: "Build".into(), pos: spot, angle: Some(1.25) });
    g.run_frames(1);
    assert_eq!(angles(&g), vec![1.25]);

    g.submit(0, Command::DoSpecialPowerAtLocation { source: src, power: "Build".into(), pos: spot, angle: None });
    g.run_frames(1);
    assert_eq!(angles(&g), vec![1.25, 0.0], "no angle is INVALID_ANGLE, read as 0");

    g.submit(0, Command::DoSpecialPower { source: src, power: "Build".into(), target: SpecialPowerTarget::Position(spot) });
    g.run_frames(1);
    assert_eq!(angles(&g), vec![1.25, 0.0, 0.0]);
}

fn phase(g: &GameLogic, id: ObjectId) -> HackPhase {
    g.find_object(id).unwrap().hack_internet.as_ref().map_or(HackPhase::Idle, |h| h.phase)
}

fn pending(g: &GameLogic, id: ObjectId) -> bool {
    g.find_object(id).unwrap().hack_internet.as_ref().is_some_and(|h| h.pending.is_some())
}
