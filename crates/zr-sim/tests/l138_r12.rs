//! Lane 138: review R12's sim-side findings F1, F2, F4, F5, F7, F8 and F9, each checked against
//! the C++ it cites. Synthetic data only.
//!
//! * F1 `AIGuardRetaliateState::isAttack` is always false (no sub-state overrides `State::isAttack`).
//! * F2 `Object::isAbleToAttack`'s `OBJECT_STATUS_NO_ATTACK` test (`Object.cpp:3239-3241`), which
//!   `ProneUpdate` sets.
//! * F4 `MobNexusContain::isValidContainerFor` (ALLIES, non-zero slot count).
//! * F5 `ActionManager::canEnterObject`'s heal-container full-health rejection.
//! * F7 `HelixContain::isValidContainerFor`'s portable-structure acceptance.
//! * F8 the payload of `OverlordContain`/`HelixContain`/`MobNexusContain`/`GarrisonContain` is
//!   created in `onObjectCreated`, not on the first update.
//! * F9 `AIEnterState::update`'s "goal contained and airborne" bail.

use std::sync::Arc;

use zr_sim::ai_states::AiStateType;
use zr_sim::body::DamageInfo;
use zr_sim::contain::{ContainKind, ContainTemplate, GarrisonFirePoints};
use zr_sim::damage::DamageType;
use zr_sim::modules2::ProneTemplate;
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::weapon::{CombatTemplate, WeaponSetTemplate, WeaponTemplate};
use zr_sim::{Command, Coord3D, GameLogic, ObjectId, ObjectTemplate, TemplateSource, TerrainLogic};

struct Store(Vec<Arc<ObjectTemplate>>);

impl TemplateSource for Store {
    fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
        self.0.iter().find(|t| t.name == name).cloned()
    }
}

/// A two-player world; `templates` is the name lookup the payload code (`create_payload`) uses.
fn world(templates: Vec<ObjectTemplate>) -> GameLogic {
    let store = Store(templates.into_iter().map(Arc::new).collect());
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 2000);
    g.add_player(1, 2000);
    g.set_template_source(Arc::new(store));
    g
}

fn contain(kind: ContainKind, capacity: i32) -> ContainTemplate {
    ContainTemplate {
        kind,
        capacity,
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
        tunnel_frames_for_full_heal: 1.0,
        is_enclosing_container: true,
        fire_points: GarrisonFirePoints::default(),
        passengers_in_turret: false,
        weapon_bonus_passed_to_passengers: false,
        immune_to_clear_building_attacks: false,
    }
}

fn vehicle(name: &str, c: ContainTemplate) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 10.0;
    t.max_health = 200.0;
    t.initial_health = 200.0;
    t.transport_slot_count = 2;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    t.contain = Some(Arc::new(c));
    t
}

fn infantry(name: &str, slot_count: u32) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::INFANTRY | kind_of::SELECTABLE;
    t.geometry.major_radius = 4.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.transport_slot_count = slot_count;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Legs, 20.0, 60.0, 120.0, 180.0));
    t
}

/// `KINDOF_PORTABLE_STRUCTURE` with `TransportSlotCount = 0` (the Helix payload shape).
fn portable_structure() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Portable");
    t.kind_of = kind_of::STRUCTURE | kind_of::PORTABLE_STRUCTURE;
    t.geometry.major_radius = 6.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.transport_slot_count = 0;
    t
}

/// An infantry unit that carries a weapon and goes prone when damaged (`ProneUpdate`).
fn rifleman(name: &str) -> ObjectTemplate {
    let mut t = infantry(name, 1);
    t.kind_of |= kind_of::CAN_ATTACK;
    t.vision_range = 200.0;
    let mut w = WeaponTemplate::new("Rifle");
    w.primary_damage = 5.0;
    w.damage_type = DamageType::SmallArms;
    w.attack_range = 100.0;
    w.min_delay_between_shots = 10;
    w.max_delay_between_shots = 10;
    let mut set = WeaponSetTemplate::default();
    set.weapons[0] = Some(Arc::new(w));
    t.combat = CombatTemplate { weapon_set: Some(set), ..CombatTemplate::default() };
    t.prone = Some(ProneTemplate { damage_to_frames_ratio: 1.0 });
    t
}

/// An armed vehicle: `vision` sets the guard-scan radius (`GuardInnerModifierHuman` 1.8 for the human
/// player), `range` the weapon's reach (the same fixture as `l136_guard_retaliate.rs`).
fn armed(name: &str, vision: f32, range: f32) -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
    t.geometry.major_radius = 8.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.vision_range = vision;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 60.0, 120.0, 120.0, 360.0));
    let mut w = WeaponTemplate::new("Cannon");
    w.primary_damage = 20.0;
    w.damage_type = DamageType::ArmorPiercing;
    w.attack_range = range;
    w.min_delay_between_shots = 10;
    w.max_delay_between_shots = 10;
    let mut set = WeaponSetTemplate::default();
    set.weapons[0] = Some(Arc::new(w));
    t.combat = CombatTemplate { weapon_set: Some(set), ..CombatTemplate::default() };
    Arc::new(t)
}

/// A weaponless, very tough vehicle: the aggressor (never fires back).
fn tough_target(name: &str) -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 8.0;
    t.max_health = 1000.0;
    t.initial_health = 1000.0;
    t.vision_range = 40.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 240.0));
    Arc::new(t)
}

// ---- F1 --------------------------------------------------------------------------------------

/// `AIUpdateInterface::isAttacking` is `getStateMachine()->isInAttackState()` (`AIUpdate.cpp:3131`),
/// i.e. the current state's `isAttack()`. `AIGuardRetaliateState::isAttack` forwards to its machine
/// (`AIStates.cpp:6867-6874`), but none of that machine's states overrides the base `State::isAttack`
/// (`StateMachine.h:151`; no `isAttack` in `AIGuardRetaliate.{h,cpp}` — the machine overrides only
/// `isIdle`, `AIGuardRetaliate.h:110`), so a retaliating unit is never "attacking", even while its
/// `AttackAggressor` sub-state runs an `AIAttackState` (`m_attackState` present,
/// `AIGuardRetaliate.h:233-246`). `AIGuard`'s three states do forward (`AIGuard.h:144/194/255`),
/// which is why the port's `S::Guard` arm is right and this one is not.
/// Revert check: with `S::GuardRetaliate => o.ai.machine.retaliate.attack_active` the assert fails.
#[test]
fn a_guard_retaliating_unit_is_never_is_attacking() {
    let mut g = world(Vec::new());
    g.set_enemies(0, 1);
    let victim = g.create_object(&tough_target("Victim"), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    // A 50-unit vision gives a 90-unit standard guard circle (GuardInnerModifierHuman 1.8); the
    // 70-unit weapon lets the ally fire from inside it. The damager is 150 units away: outside the
    // scan, inside the aggressor state's own reach.
    let ally = g.create_object(&armed("Ally", 50.0, 70.0), Coord3D::new(260.0, 200.0, 0.0), 0.0, 0);
    let damager = g.create_object(&tough_target("Damager"), Coord3D::new(410.0, 200.0, 0.0), 0.0, 1);
    g.submit(0, Command::EnableRetaliationMode { enabled: true });
    g.update();
    g.damage_object_with_death(victim, DamageInfo { source: damager, amount: 10.0, kind: DamageType::ArmorPiercing }, zr_sim::damage::DeathType::Normal);
    let o = g.find_object(ally).unwrap();
    assert_eq!(o.ai.machine.current, AiStateType::GuardRetaliate, "the assist entered AI_GUARD_RETALIATE");
    assert!(o.ai.machine.retaliate.attack_active, "the AttackAggressor sub-state built its AIAttackState");
    assert!(
        !g.is_ai_attacking(ally),
        "AI_GUARD_RETALIATE's isAttack is always false in the engine, even inside its attack sub-state"
    );
    // And it stays false while the sub-machine's attack actually runs (a few frames of the approach).
    for _ in 0..10 {
        g.update();
        if g.find_object(ally).is_none_or(|a| a.ai.machine.current != AiStateType::GuardRetaliate) {
            break;
        }
        assert!(!g.is_ai_attacking(ally));
    }
}

// ---- F2 --------------------------------------------------------------------------------------

/// `Object::isAbleToAttack`'s first test is `OBJECT_STATUS_NO_ATTACK` (`Object.cpp:3239-3241`),
/// which `ProneUpdate` sets while a unit is prone (`ProneUpdate.cpp:105-119`). It gates every new
/// attack path; `AIGroup::groupAttackMoveToPosition` picks a plain move for a unit that cannot
/// attack (`AIGroup.cpp:2335`), which is observable as the state the order enters.
/// Revert check: without the status test the prone unit enters `AttackMoveTo` instead of `MoveTo`.
#[test]
fn a_prone_unit_is_not_able_to_attack() {
    let mut g = world(Vec::new());
    g.set_enemies(0, 1);
    let ranger = Arc::new(rifleman("Ranger"));
    let u = g.create_object(&ranger, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    g.create_object(&Arc::new(rifleman("Enemy")), Coord3D::new(300.0, 100.0, 0.0), 0.0, 1);
    g.damage_object(u, DamageInfo { source: ObjectId::INVALID, amount: 20.0, kind: DamageType::SmallArms });
    assert!(g.find_object(u).unwrap().behavior.prone_frames > 0, "the hit made the unit prone");
    assert!(!g.is_able_to_attack(u), "a prone unit cannot start a new attack");
    g.submit(0, Command::AttackMove { ids: vec![u], pos: Coord3D::new(200.0, 300.0, 0.0) });
    g.run_frames(1);
    assert_eq!(
        g.find_object(u).unwrap().ai.machine.current,
        AiStateType::MoveTo,
        "an attack-move on a unit that cannot attack becomes a plain move (AIGroup.cpp:2335)"
    );
    // When the prone timer runs out, `stopProneEffects` clears the status and the same order attacks.
    g.run_frames(30);
    assert!(g.is_able_to_attack(u), "not prone any more");
    g.submit(0, Command::AttackMove { ids: vec![u], pos: Coord3D::new(200.0, 300.0, 0.0) });
    g.run_frames(1);
    assert_eq!(g.find_object(u).unwrap().ai.machine.current, AiStateType::AttackMoveTo);
}

// ---- F4 --------------------------------------------------------------------------------------

/// `MobNexusContain::isValidContainerFor` (`MobNexusContain.cpp:138-192`): after
/// `OpenContain::isValidContainerFor`, the rider's relationship to the container must be ALLIES
/// (independent of `AllowEnemiesInside`), a rider with `TransportSlotCount == 0` is not MobNexusable,
/// and capacity counts transport slots.
/// Revert check: without the `is_mob_nexus` branch the generic arm accepts the zero-slot ally
/// (`transport_slot_count.max(1)`) and the hostile rider (the INI flag allows enemies).
#[test]
fn a_mob_nexus_only_takes_allied_units_with_a_slot_count() {
    let mut g = world(Vec::new());
    g.set_enemies(0, 1);
    let nexus_t = Arc::new(vehicle("Nexus", contain(ContainKind::MobNexus, 3)));
    let ranger_t = Arc::new(infantry("Ranger", 1));
    let zero_t = Arc::new(infantry("ZeroSlot", 0));
    let nexus = g.create_object(&nexus_t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let ally = g.create_object(&ranger_t, Coord3D::new(120.0, 100.0, 0.0), 0.0, 0);
    let ally_zero = g.create_object(&zero_t, Coord3D::new(120.0, 110.0, 0.0), 0.0, 0);
    let enemy = g.create_object(&ranger_t, Coord3D::new(120.0, 120.0, 0.0), 0.0, 1);
    assert!(g.is_valid_container_for(nexus, ally, true), "an allied slot-1 unit is MobNexusable");
    assert!(!g.is_valid_container_for(nexus, ally_zero, true), "`TransportSlotCount == 0` is not MobNexusable");
    assert!(
        !g.is_valid_container_for(nexus, enemy, true),
        "`rider->getRelationship(container) == ALLIES` regardless of AllowEnemiesInside"
    );
}

// ---- F5 --------------------------------------------------------------------------------------

/// `ActionManager::canEnterObject` (`ActionManager.cpp:660-667`): a heal container "is only used for
/// the purposes of healing and we cannot enter it with full health", whatever the capacity mode.
/// Revert check: without the test both asserts fail (the hospital is a valid container).
#[test]
fn a_heal_container_rejects_a_full_health_unit() {
    let mut g = world(Vec::new());
    let hospital_t = Arc::new(vehicle("Hospital", contain(ContainKind::Heal, -1)));
    let ranger_t = Arc::new(infantry("Ranger", 1));
    let hospital = g.create_object(&hospital_t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let ranger = g.create_object(&ranger_t, Coord3D::new(140.0, 100.0, 0.0), 0.0, 0);
    assert!(!g.can_enter_object(ranger, hospital, true), "full health: rejected with CHECK_CAPACITY");
    assert!(!g.can_enter_object(ranger, hospital, false), "full health: rejected with DONT_CHECK_CAPACITY");
    g.damage_object(ranger, DamageInfo { source: ObjectId::INVALID, amount: 30.0, kind: DamageType::Explosion });
    assert!(g.can_enter_object(ranger, hospital, true), "a hurt unit may enter the hospital");
}

// ---- F7 --------------------------------------------------------------------------------------

/// `HelixContain::isValidContainerFor` (`HelixContain.cpp:308-314`): a portable structure is accepted
/// while the Helix carries none, before `TransportContain::isValidContainerFor` (which would reject
/// its zero slot count). `createPayload` (`:148-189`) then loads it instead of leaving it in the
/// world. Revert check: the direct query turns false and the `PayloadTemplateName` payload stays out.
#[test]
fn a_helix_accepts_its_portable_structure_payload() {
    let mut hc = contain(ContainKind::Helix, 1);
    hc.initial_payload = vec![("Portable".to_string(), 1)];
    let mut g = world(vec![vehicle("Helix", hc.clone()), portable_structure()]);
    let helix = g.create_object(&Arc::new(vehicle("Helix", hc)), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let free = g.create_object(&Arc::new(portable_structure()), Coord3D::new(140.0, 100.0, 0.0), 0.0, 0);
    assert!(g.is_valid_container_for(helix, free, true), "the portable structure is accepted while none is carried");
    g.run_frames(1);
    let carried = *g.contained_units(helix).first().expect("the Helix loaded its PayloadTemplateName payload");
    assert!(g.find_object(carried).unwrap().is_kind_of(kind_of::PORTABLE_STRUCTURE));
}

// ---- F8 --------------------------------------------------------------------------------------

/// `Object::initObject` runs every module's `onObjectCreated` before `registerObject`
/// (`Object.cpp:473-484`), and `OverlordContain` (`:107-111`), `HelixContain` (`:108-112`),
/// `MobNexusContain` (`:308-333`) and `GarrisonContain` (`:1762-1784`) create their payload there, so
/// the payload's id follows the container's and no other object is created in between.
/// `TransportContain::createPayload` stays on the first update (`TransportContain.cpp:468-526`).
/// Revert check: with the hook removed the garrison has no occupants right after `create_object` and
/// the payload ids are no longer container+1/+2; the transport half still passes either way.
#[test]
fn payloads_are_created_at_object_creation() {
    let mut gc = contain(ContainKind::Garrison, 4);
    gc.initial_payload = vec![("Ranger".to_string(), 2)];
    let mut tc = contain(ContainKind::Transport, 2);
    tc.initial_payload = vec![("Ranger".to_string(), 1)];
    let mut g = world(vec![vehicle("Bunker", gc.clone()), vehicle("Truck", tc.clone()), infantry("Ranger", 1)]);
    let bunker = g.create_object(&Arc::new(vehicle("Bunker", gc)), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    assert_eq!(
        g.contained_units(bunker),
        &[ObjectId(bunker.0 + 1), ObjectId(bunker.0 + 2)],
        "GarrisonContain::onObjectCreated created the roster inside createObject, taking the next ids"
    );
    let truck = g.create_object(&Arc::new(vehicle("Truck", tc)), Coord3D::new(200.0, 100.0, 0.0), 0.0, 0);
    assert!(g.contained_units(truck).is_empty(), "TransportContain::createPayload waits for the first update");
    // The slot's first update is frame 1 (`registerObject` clamps the creation frame's wake to 1,
    // GameLogic.cpp:4018-4045), so two updates.
    g.run_frames(2);
    assert_eq!(g.contained_units(truck).len(), 1, "the transport payload appears on the first update");
}

// ---- F9 --------------------------------------------------------------------------------------

/// `AIEnterState::update` (`AIStates.cpp:6326-6331`): "if our goal is contained by something else,
/// give up ... tell some rangers to enter a humvee / tell the humvee to enter a chinook / fly the
/// chinook around; the rangers follow the chinook like dopes". The goal must be contained, above the
/// terrain, and the owner not. Revert check: the ranger stays in `Enter` (chasing) instead of ending
/// in `Idle`.
#[test]
fn an_enter_gives_up_when_its_goal_is_contained_and_airborne() {
    let mut cc = contain(ContainKind::Transport, 4);
    cc.initial_payload = vec![("Humvee".to_string(), 1)];
    let mut g = world(vec![
        vehicle("Chinook", cc.clone()),
        vehicle("Humvee", contain(ContainKind::Transport, 2)),
        infantry("Ranger", 1),
    ]);
    let chinook = g.create_object(&Arc::new(vehicle("Chinook", cc)), Coord3D::new(300.0, 300.0, 0.0), 0.0, 0);
    // Fly the chinook after its payload is aboard: `Object::isAboveTerrain` only reads the position, and
    // `OpenContain::update` re-anchors the rider to the container every frame.
    g.set_object_position(chinook, Coord3D::new(300.0, 300.0, 60.0));
    let mut humvee = None;
    for _ in 0..5 {
        g.update();
        if let Some(&h) = g.contained_units(chinook).first() {
            humvee = Some(h);
            break;
        }
    }
    let humvee = humvee.expect("the humvee boarded the chinook");
    assert!(g.find_object(humvee).unwrap().position.z > 1.0, "and rides above the terrain");
    let ranger = g.create_object(&Arc::new(infantry("Ranger", 1)), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    g.submit(0, Command::Enter { ids: vec![ranger], container: humvee });
    g.run_frames(2);
    assert_ne!(
        g.find_object(ranger).unwrap().ai.machine.current,
        AiStateType::Enter,
        "a goal contained by something airborne fails the enter instead of being chased"
    );
    assert_eq!(g.find_object(ranger).unwrap().ai.machine.current, AiStateType::Idle);
}
