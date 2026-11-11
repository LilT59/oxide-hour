//! Lane 126: `DAMAGE_KILL_GARRISONED` (`ActiveBody::attemptDamage`, ActiveBody.cpp:445-478) and the garrison block of
//! the projectile behaviors (`DumbProjectileBehavior::projectileHandleCollision`, DumbProjectileBehavior.cpp:485-515 /
//! `MissileAIUpdate::projectileHandleCollision`, MissileAIUpdate.cpp:333-364). Synthetic data only.

use std::sync::Arc;

use zr_sim::body::DamageInfo;
use zr_sim::contain::{ContainKind, ContainTemplate, GarrisonFirePoints};
use zr_sim::damage::DamageType;
use zr_sim::projectile::{DumbProjectileParams, GarrisonHitKill, ProjectileBehavior, ProjectileTemplate};
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::weapon::{CombatTemplate, WeaponSetTemplate, WeaponTemplate};
use zr_sim::{Command, Coord3D, GameLogic, ObjectId, ObjectTemplate, TerrainLogic};

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

fn building(name: &str, contain: ContainTemplate) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE | kind_of::SELECTABLE;
    t.geometry.major_radius = 30.0;
    t.geometry.height = 40.0;
    t.max_health = 1000.0;
    t.initial_health = 1000.0;
    t.contain = Some(Arc::new(contain));
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

fn world() -> GameLogic {
    GameLogic::new(TerrainLogic::flat(300, 300, 0), 1)
}

/// Put `n` infantry of player 0 (spawned next to `at`) into `garrison` and run the enter order through.
fn garrison_with(g: &mut GameLogic, garrison: ObjectId, at: Coord3D, n: usize) -> Vec<ObjectId> {
    let mut ids = Vec::new();
    for i in 0..n {
        let id = g.create_object(&Arc::new(infantry("GI")), Coord3D::new(at.x + 25.0 + 8.0 * i as f32, at.y, 0.0), 0.0, 0);
        ids.push(id);
    }
    g.submit(0, Command::Enter { ids: ids.clone(), container: garrison });
    g.run_frames(20);
    for id in &ids {
        assert!(g.is_contained(*id), "the infantry enters the garrison");
    }
    ids
}

/// `ActiveBody::attemptDamage`'s `DAMAGE_KILL_GARRISONED` case: the hit kills `floor(amount)` occupants in contain
/// order, scores them for the damager, and the building itself takes no health damage (`alreadyHandled`). Reverting
/// the case (no kill) leaves the occupant inside and fails this test.
#[test]
fn a_kill_garrisoned_hit_clears_one_occupant_and_leaves_the_building_alone() {
    let mut g = world();
    let bld = g.create_object(&Arc::new(building("Bunker", contain(ContainKind::Garrison, 5))), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let _ = garrison_with(&mut g, bld, Coord3D::new(200.0, 200.0, 0.0), 3);
    // The first occupant in contain (enter) order, see `a_kill_garrisoned_hit_kills_floor_of_its_amount`.
    let order = g.contained_units(bld).to_vec();
    let hp = g.find_object(bld).unwrap().health();
    let damager = g.create_object(&Arc::new(infantry("Microwave")), Coord3D::new(300.0, 200.0, 0.0), 0.0, 1);
    g.damage_object(bld, DamageInfo { source: damager, amount: 1.0, kind: DamageType::KillGarrisoned });
    g.run_frames(1);
    assert!(g.find_object(order[0]).is_none(), "the first occupant is killed");
    assert!(g.is_contained(order[1]) && g.is_contained(order[2]), "one kill only");
    assert_eq!(g.find_object(bld).unwrap().health(), hp, "the building takes no health damage");
    assert_eq!(g.contained_units(bld).len(), 2);
}

/// `REAL_TO_INT_FLOOR(m_amount)`: a 2.9 hit kills two, in contain order.
#[test]
fn a_kill_garrisoned_hit_kills_floor_of_its_amount() {
    let mut g = world();
    let bld = g.create_object(&Arc::new(building("Bunker", contain(ContainKind::Garrison, 5))), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let _ = garrison_with(&mut g, bld, Coord3D::new(200.0, 200.0, 0.0), 3);
    // The victims are the first two in the contain list, which is the enter order (`OpenContain::addToContainList`,
    // OpenContain.cpp:407, push_back), not the spawn order: the walk to the door decides which unit enters first.
    let order = g.contained_units(bld).to_vec();
    g.damage_object(bld, DamageInfo { source: ObjectId::INVALID, amount: 2.9, kind: DamageType::KillGarrisoned });
    g.run_frames(1);
    assert!(g.find_object(order[0]).is_none() && g.find_object(order[1]).is_none(), "two occupants die");
    assert!(g.is_contained(order[2]), "the third survives");
}

/// The case only applies to a garrison (`isGarrisonable`): a transport's passengers are untouched, and an empty
/// garrison loses nobody.
#[test]
fn only_a_garrison_is_cleared() {
    let mut g = world();
    let truck = g.create_object(&Arc::new(building("Truck", contain(ContainKind::Transport, 5))), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let units = garrison_with(&mut g, truck, Coord3D::new(200.0, 200.0, 0.0), 2);
    g.damage_object(truck, DamageInfo { source: ObjectId::INVALID, amount: 5.0, kind: DamageType::KillGarrisoned });
    g.run_frames(1);
    assert!(g.is_contained(units[0]) && g.is_contained(units[1]), "a transport is not a garrison");
    let empty = g.create_object(&Arc::new(building("Empty", contain(ContainKind::Garrison, 5))), Coord3D::new(400.0, 200.0, 0.0), 0.0, 0);
    let hp = g.find_object(empty).unwrap().health();
    g.damage_object(empty, DamageInfo { source: ObjectId::INVALID, amount: 5.0, kind: DamageType::KillGarrisoned });
    assert_eq!(g.find_object(empty).unwrap().health(), hp);
}

fn shell_weapon(hit_kill: Option<GarrisonHitKill>) -> WeaponTemplate {
    let mut w = WeaponTemplate::new("FlashBang");
    w.primary_damage = 50.0;
    w.primary_damage_radius = 5.0;
    w.secondary_damage_radius = 5.0;
    w.attack_range = 2000.0;
    w.min_delay_between_shots = 1000;
    w.max_delay_between_shots = 1000;
    w.damage_type = DamageType::Explosion;
    w.weapon_speed = 10.0;
    w.has_projectile = true;
    w.projectile = Some(Arc::new(ProjectileTemplate {
        behavior: ProjectileBehavior::Dumb(DumbProjectileParams { max_lifespan: 300, ..DumbProjectileParams::default() }),
        bounding_radius: 3.0,
        small_missile: false,
        neutron_warhead: None,
        neutron_blast: None,
        garrison_hit_kill: hit_kill,
    }));
    w
}

fn shell_attacker(hit_kill: Option<GarrisonHitKill>) -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Ranger");
    t.kind_of = kind_of::INFANTRY | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
    t.geometry.major_radius = 5.0;
    t.max_health = 200.0;
    t.initial_health = 200.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Legs, 20.0, 60.0, 120.0, 180.0));
    let mut set = WeaponSetTemplate::default();
    set.weapons[0] = Some(Arc::new(shell_weapon(hit_kill)));
    t.combat = CombatTemplate { weapon_set: Some(set), ..CombatTemplate::default() };
    t
}

/// `DumbProjectileBehavior::projectileHandleCollision`'s garrison block: a shell with `GarrisonHitKillCount` clears
/// that many matching occupants, plays `GarrisonHitKillFX` on the building and is *destroyed with no explosion* — the
/// building takes no detonation damage. Reverting the block lets the 50-damage detonation hit the building and fails.
#[test]
fn a_flash_bang_shell_clears_the_garrison_instead_of_exploding() {
    let mut g = world();
    let at = Coord3D::new(600.0, 300.0, 0.0);
    let bld = g.create_object(&Arc::new(building("Bunker", contain(ContainKind::Garrison, 5))), at, 0.0, 0);
    let _ = garrison_with(&mut g, bld, at, 3);
    // The victims are the first two in contain (enter) order, see `a_kill_garrisoned_hit_kills_floor_of_its_amount`.
    let units = g.contained_units(bld).to_vec();
    let gunner = g.create_object(&Arc::new(shell_attacker(Some(GarrisonHitKill { count: 2, kind_of: kind_of::INFANTRY, kind_of_not: 0, fx: Some("FX_TestGarrison".into()) }))), Coord3D::new(100.0, 300.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    let hp = g.find_object(bld).unwrap().health();
    g.submit(1, Command::Attack { ids: vec![gunner], target: bld });
    let mut fx = None;
    let mut seen = false;
    for _ in 0..200 {
        g.run_frames(1);
        if !g.projectiles().is_empty() {
            seen = true;
        } else if seen {
            fx = g.fx_events().iter().find(|e| e.name == "FX_TestGarrison").map(|e| e.pos);
            break;
        }
    }
    assert!(seen, "the shell is fired");
    assert!(g.find_object(units[0]).is_none() && g.find_object(units[1]).is_none(), "two occupants are cleared");
    assert!(g.is_contained(units[2]), "the third is left");
    assert_eq!(g.find_object(bld).unwrap().health(), hp, "no explosion: the building keeps its health");
    assert!(g.projectiles().is_empty(), "the shell is removed");
    assert_eq!(fx, Some(at), "the garrison FX plays on the building, not at the shell");
}

/// The kind-of mask filters the occupants (`isKindOfMulti`): a mask that requires a VEHICLE matches no infantry, so
/// the shell explodes normally and the building takes its 50 damage.
#[test]
fn the_garrison_kind_of_mask_filters_its_victims() {
    let mut g = world();
    let at = Coord3D::new(600.0, 300.0, 0.0);
    let bld = g.create_object(&Arc::new(building("Bunker", contain(ContainKind::Garrison, 5))), at, 0.0, 0);
    let unit = garrison_with(&mut g, bld, at, 1)[0];
    let gunner = g.create_object(&Arc::new(shell_attacker(Some(GarrisonHitKill { count: 1, kind_of: kind_of::VEHICLE, kind_of_not: 0, fx: None }))), Coord3D::new(100.0, 300.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    let hp = g.find_object(bld).unwrap().health();
    g.submit(1, Command::Attack { ids: vec![gunner], target: bld });
    let mut fx = None;
    let mut seen = false;
    for _ in 0..200 {
        g.run_frames(1);
        if !g.projectiles().is_empty() {
            seen = true;
        } else if seen {
            fx = g.fx_events().iter().find(|e| e.name == "FX_TestGarrison").map(|e| e.pos);
            break;
        }
    }
    assert!(seen, "the shell is fired");
    assert!(g.is_contained(unit), "the infantry does not match the mask");
    assert!(g.find_object(bld).unwrap().health() < hp, "the shell explodes instead: the building is damaged");
    assert_eq!(fx, None, "no garrison FX");
}

/// A garrison building whose module sets `ImmuneToClearBuildingAttacks` (the China Bunker, GLA Palace and
/// America FireBase in the shipped data) is not a clearable garrison: `ActiveBody::attemptDamage`'s
/// `DAMAGE_KILL_GARRISONED` case checks `!contain->isImmuneToClearBuildingAttacks()` (ActiveBody.cpp:456).
/// Reverting the check (ignoring the flag) kills the occupants and fails this test.
#[test]
fn an_immune_garrison_is_not_cleared_by_kill_garrisoned() {
    let mut g = world();
    let mut c = contain(ContainKind::Garrison, 5);
    c.immune_to_clear_building_attacks = true;
    let bld = g.create_object(&Arc::new(building("Bunker", c)), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let units = garrison_with(&mut g, bld, Coord3D::new(200.0, 200.0, 0.0), 2);
    let hp = g.find_object(bld).unwrap().health();
    g.damage_object(bld, DamageInfo { source: ObjectId::INVALID, amount: 5.0, kind: DamageType::KillGarrisoned });
    g.run_frames(1);
    assert!(g.is_contained(units[0]) && g.is_contained(units[1]), "an immune garrison loses nobody");
    assert_eq!(g.find_object(bld).unwrap().health(), hp, "and the building takes no damage");
}

/// The projectile garrison block has the same immunity check (`DumbProjectileBehavior.cpp:483`,
/// `MissileAIUpdate.cpp:338`): a flash-bang on an immune bunker does not clear it and explodes normally.
#[test]
fn an_immune_garrison_is_not_cleared_by_a_flash_bang() {
    let mut g = world();
    let at = Coord3D::new(600.0, 300.0, 0.0);
    let mut c = contain(ContainKind::Garrison, 5);
    c.immune_to_clear_building_attacks = true;
    let bld = g.create_object(&Arc::new(building("Bunker", c)), at, 0.0, 0);
    let units = garrison_with(&mut g, bld, at, 2);
    let gunner = g.create_object(&Arc::new(shell_attacker(Some(GarrisonHitKill { count: 2, kind_of: kind_of::INFANTRY, kind_of_not: 0, fx: Some("FX_TestGarrison".into()) }))), Coord3D::new(100.0, 300.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    let hp = g.find_object(bld).unwrap().health();
    g.submit(1, Command::Attack { ids: vec![gunner], target: bld });
    let mut fx = None;
    let mut seen = false;
    for _ in 0..200 {
        g.run_frames(1);
        if !g.projectiles().is_empty() {
            seen = true;
        } else if seen {
            fx = g.fx_events().iter().find(|e| e.name == "FX_TestGarrison").map(|e| e.pos);
            break;
        }
    }
    assert!(seen, "the shell is fired");
    assert!(g.is_contained(units[0]) && g.is_contained(units[1]), "nobody is cleared");
    assert!(g.find_object(bld).unwrap().health() < hp, "the shell explodes normally");
    assert_eq!(fx, None, "no garrison-cleared FX");
}
