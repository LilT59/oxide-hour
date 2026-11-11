//! Tunnel networks (`TunnelContain`/`TunnelTracker`) and passenger firing
//! (`PassengersAllowedToFire`), on synthetic data only.

use std::sync::Arc;

use zr_sim::contain::{ContainKind, ContainTemplate, GarrisonFirePoints};
use zr_sim::damage::DamageType;
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::weapon::{CombatTemplate, WeaponSetTemplate, WeaponTemplate};
use zr_sim::{Command, Coord3D, GameLogic, GameRules, ObjectId, ObjectTemplate, TerrainLogic};

fn flat() -> TerrainLogic {
    TerrainLogic::flat(100, 100, 0)
}

fn infantry() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Infantry");
    t.kind_of = kind_of::INFANTRY | kind_of::SELECTABLE;
    t.geometry.major_radius = 4.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.transport_slot_count = 1;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Legs, 20.0, 60.0, 120.0, 180.0));
    t
}

fn contain(kind: ContainKind, capacity: i32, fire: bool, full_heal_frames: f32) -> ContainTemplate {
    ContainTemplate {
        kind,
        capacity,
        allow_inside_kind_of: None,
        forbid_inside_kind_of: 0,
        allow_allies_inside: true,
        allow_enemies_inside: true,
        allow_neutral_inside: true,
        passengers_allowed_to_fire: fire,
        damage_percentage_to_units: 0.0,
        number_of_exit_paths: 1,
        exit_delay: 0,
        door_open_time: 1,
        scatter_nearby_on_exit: true,
        orient_like_container_on_exit: false,
        reset_mood_check_on_exit: false,
        health_regen: 0.0,
        do_i_heal_objects: false,
        heal_frames: full_heal_frames,
        initial_payload: Vec::new(),
        tunnel_frames_for_full_heal: full_heal_frames,
        is_enclosing_container: kind == ContainKind::Garrison,
        fire_points: GarrisonFirePoints::default(),
        passengers_in_turret: false,
        weapon_bonus_passed_to_passengers: false,
        immune_to_clear_building_attacks: false,
    }
}

fn tunnel(full_heal_frames: f32) -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Tunnel");
    t.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE | kind_of::SELECTABLE;
    t.geometry.major_radius = 20.0;
    t.max_health = 1000.0;
    t.initial_health = 1000.0;
    t.contain = Some(Arc::new(contain(ContainKind::Tunnel, -1, false, full_heal_frames)));
    t
}

fn rifle() -> WeaponTemplate {
    let mut w = WeaponTemplate::new("Rifle");
    w.primary_damage = 20.0;
    w.damage_type = DamageType::SmallArms;
    w.attack_range = 100.0;
    w.min_delay_between_shots = 30;
    w.max_delay_between_shots = 30;
    w
}

fn rifle_infantry() -> ObjectTemplate {
    let mut t = infantry();
    t.kind_of |= kind_of::CAN_ATTACK;
    t.vision_range = 200.0;
    let mut set = WeaponSetTemplate::default();
    set.weapons[0] = Some(Arc::new(rifle()));
    t.combat = CombatTemplate { weapon_set: Some(set), auto_acquire_enemies: true, ..CombatTemplate::default() };
    t
}

fn garrison(fire: bool) -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Garrison");
    t.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE | kind_of::SELECTABLE;
    t.geometry.major_radius = 30.0;
    t.max_health = 1000.0;
    t.initial_health = 1000.0;
    t.vision_range = 300.0; // the building reveals the area to its owner (idle auto-acquire fog check)
    let mut c = contain(ContainKind::Garrison, 5, fire, 1.0);
    // Four synthetic FIREPOINT bones (N/E/S/W) for the pristine condition; damaged conditions have
    // none, so they fall back to the centre (like a model with no bones).
    c.fire_points = GarrisonFirePoints {
        conditions: [
            vec![
                Coord3D::new(0.0, 30.0, 0.0),
                Coord3D::new(30.0, 0.0, 0.0),
                Coord3D::new(0.0, -30.0, 0.0),
                Coord3D::new(-30.0, 0.0, 0.0),
            ],
            Vec::new(),
            Vec::new(),
        ],
        ..GarrisonFirePoints::default()
    };
    t.contain = Some(Arc::new(c));
    t
}

fn world() -> GameLogic {
    let mut g = GameLogic::new(flat(), 1);
    g.set_rules(GameRules { max_tunnel_capacity: 2, ..GameRules::default() });
    g
}

// ---- tunnels -------------------------------------------------------------------------------

#[test]
fn tunnel_network_shares_one_list_and_capacity() {
    let mut g = world();
    let ta = Arc::new(tunnel(30.0));
    let a = g.create_object(&ta, Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let b = g.create_object(&ta, Coord3D::new(400.0, 200.0, 0.0), 0.0, 0);
    let u = |g: &mut GameLogic, x: f32| g.create_object(&Arc::new(infantry()), Coord3D::new(x, 200.0, 0.0), 0.0, 0);
    let u1 = u(&mut g, 225.0);
    let u2 = u(&mut g, 425.0);
    let u3 = u(&mut g, 230.0);
    g.submit(0, Command::Enter { ids: vec![u1], container: a });
    g.submit(0, Command::Enter { ids: vec![u2], container: b });
    g.run_frames(6);
    assert_eq!(g.contained_units(a), &[u1, u2], "both entrances see the shared network list");
    assert_eq!(g.contained_units(b), &[u1, u2], "the whole network shares one list");
    assert_eq!(g.tunnel_passengers(0).len(), 2);
    // The third unit exceeds MaxTunnelCapacity (2), so it cannot board either entrance.
    g.submit(0, Command::Enter { ids: vec![u3], container: a });
    g.run_frames(30);
    assert!(!g.is_contained(u3), "the network is full");
}

#[test]
fn tunnel_enter_at_one_and_leave_through_another() {
    let mut g = world();
    let ta = Arc::new(tunnel(30.0));
    let a = g.create_object(&ta, Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let b = g.create_object(&ta, Coord3D::new(600.0, 200.0, 0.0), 0.0, 0);
    let u1 = g.create_object(&Arc::new(infantry()), Coord3D::new(225.0, 200.0, 0.0), 0.0, 0);
    g.submit(0, Command::Enter { ids: vec![u1], container: a });
    g.run_frames(6);
    assert_eq!(g.container_of(u1), Some(a), "the entered entrance is remembered");
    // Evacuate through the *other* entrance: the passenger is placed at B.
    g.submit(0, Command::Evacuate { container: b });
    g.run_frames(3);
    assert!(!g.is_contained(u1));
    let up = g.find_object(u1).unwrap().position;
    let bp = g.find_object(b).unwrap().position;
    assert!(up.dist_2d(&bp) < 60.0, "the passenger left through the second tunnel ({up:?} vs {bp:?})");
    assert!(up.dist_2d(&g.find_object(a).unwrap().position) > 300.0, "not at the first tunnel");
}

#[test]
fn last_tunnel_death_caves_in_and_kills_passengers() {
    let mut g = world();
    let ta = Arc::new(tunnel(30.0));
    let a = g.create_object(&ta, Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let b = g.create_object(&ta, Coord3D::new(400.0, 200.0, 0.0), 0.0, 0);
    let u1 = g.create_object(&Arc::new(infantry()), Coord3D::new(425.0, 200.0, 0.0), 0.0, 0);
    g.submit(0, Command::Enter { ids: vec![u1], container: b });
    g.run_frames(6);
    assert!(g.is_contained(u1));
    // Destroying a non-last entrance leaves the network alive and repoints the passenger.
    g.destroy_object(b);
    g.run_frames(3);
    assert!(g.find_object(u1).is_some(), "the passenger survives a non-last collapse");
    assert_eq!(g.container_of(u1), Some(a), "repointed at the surviving entrance");
    // Destroying the last entrance caves in and kills everyone.
    g.destroy_object(a);
    g.run_frames(3);
    assert!(g.find_object(u1).is_none(), "the last tunnel caves in on its passengers");
}

#[test]
fn tunnels_heal_their_passengers() {
    let mut g = world();
    let ta = Arc::new(tunnel(30.0));
    let a = g.create_object(&ta, Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let u1 = g.create_object(&Arc::new(infantry()), Coord3D::new(225.0, 200.0, 0.0), 0.0, 0);
    g.submit(0, Command::Enter { ids: vec![u1], container: a });
    g.run_frames(6);
    assert!(g.is_contained(u1));
    let before = g.find_object(u1).unwrap().health();
    g.damage_object(u1, zr_sim::body::DamageInfo { source: ObjectId::INVALID, amount: 50.0, kind: DamageType::Unresistable });
    assert_eq!(g.find_object(u1).unwrap().health(), before - 50.0);
    g.run_frames(40);
    assert_eq!(g.find_object(u1).unwrap().health(), 100.0, "TimeForFullHeal restores full health");
}

// ---- passenger firing ----------------------------------------------------------------------

#[test]
fn garrisoned_passenger_fires_from_a_fire_point() {
    let mut g = world();
    let bld = g.create_object(&Arc::new(garrison(true)), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let ranger = g.create_object(&Arc::new(rifle_infantry()), Coord3D::new(225.0, 200.0, 0.0), 0.0, 0);
    let enemy = g.create_object(&Arc::new(rifle_infantry()), Coord3D::new(200.0, 300.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(0, Command::Enter { ids: vec![ranger], container: bld });
    g.run_frames(4);
    assert!(g.is_contained(ranger), "the ranger boarded the building");
    let hp = g.find_object(enemy).unwrap().health();
    g.run_frames(60);
    // The target is to the north, so the north FIREPOINT (0, +30 local) is the closest free point.
    let rp = g.find_object(ranger).unwrap().position;
    let bp = g.find_object(bld).unwrap().position;
    assert!((rp.y - (bp.y + 30.0)).abs() < 0.01 && (rp.x - bp.x).abs() < 0.01, "the ranger fires from the north FIREPOINT ({rp:?})");
    assert!(g.find_object(enemy).is_some_and(|e| e.health() < hp), "the ranger has been shooting");
    g.run_frames(200);
    assert!(g.find_object(enemy).is_none(), "the garrisoned ranger killed the attacker (was {hp})");
}

#[test]
fn transport_without_fire_permission_does_not_fire() {
    let mut g = world();
    let mut t = infantry();
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 10.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    t.contain = Some(Arc::new(contain(ContainKind::Transport, 2, false, 1.0)));
    let humvee = g.create_object(&Arc::new(t), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let ranger = g.create_object(&Arc::new(rifle_infantry()), Coord3D::new(215.0, 200.0, 0.0), 0.0, 0);
    let enemy = g.create_object(&Arc::new(rifle_infantry()), Coord3D::new(200.0, 280.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(0, Command::Enter { ids: vec![ranger], container: humvee });
    g.run_frames(4);
    assert!(g.is_contained(ranger));
    let before = g.find_object(enemy).unwrap().health();
    g.run_frames(120);
    assert_eq!(g.find_object(enemy).unwrap().health(), before, "PassengersAllowedToFire = No: no shots");
}

#[test]
fn ordered_container_propagates_attack_to_passengers() {
    let mut g = world();
    let bld = g.create_object(&Arc::new(garrison(true)), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let ranger = g.create_object(&Arc::new(rifle_infantry()), Coord3D::new(225.0, 200.0, 0.0), 0.0, 0);
    let enemy = g.create_object(&Arc::new(rifle_infantry()), Coord3D::new(200.0, 280.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(0, Command::Enter { ids: vec![ranger], container: bld });
    g.run_frames(4);
    assert!(g.is_contained(ranger));
    // The building itself has no weapons; ordering it to attack must reach the passenger.
    g.submit(0, Command::Attack { ids: vec![bld], target: enemy });
    g.run_frames(1);
    assert!(g.find_object(ranger).unwrap().combat.attack.is_some(), "the order propagated to the passenger");
    g.run_frames(240);
    assert!(g.find_object(enemy).is_none(), "the garrisoned passenger killed the ordered target");
}
