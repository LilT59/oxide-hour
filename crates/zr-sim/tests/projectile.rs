//! Real projectile behaviors and turret pitch, on synthetic data only.

use std::sync::Arc;
use zr_sim::damage::DamageType;
use zr_sim::projectile::{DumbProjectileParams, MissileParams, NeutronParams, ProjectileBehavior, ProjectileTemplate};
use zr_sim::superweapon::NeutronMissileTemplate;
use zr_sim::template::{kind_of, Geometry, GeometryShape, LocomotorKind, LocomotorParams};
use zr_sim::weapon::{CombatTemplate, WeaponSetTemplate, WeaponTemplate};
use zr_sim::{Command, Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

fn flat() -> TerrainLogic {
    TerrainLogic::flat(100, 100, 0)
}

fn weapon_with(proj: Option<ProjectileTemplate>, speed: f32) -> WeaponTemplate {
    let mut w = WeaponTemplate::new("W");
    w.primary_damage = 50.0;
    w.primary_damage_radius = 5.0;
    w.secondary_damage_radius = 5.0;
    w.attack_range = 400.0;
    w.min_delay_between_shots = 1000;
    w.max_delay_between_shots = 1000;
    w.damage_type = DamageType::Explosion;
    w.weapon_speed = speed;
    if let Some(p) = proj {
        w.has_projectile = true;
        w.projectile = Some(Arc::new(p));
    }
    w
}

fn tank(name: &str, w: Option<WeaponTemplate>) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
    t.geometry.major_radius = 8.0;
    t.max_health = 200.0;
    t.initial_health = 200.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    t.combat = CombatTemplate {
        weapon_set: w.map(|w| {
            let mut s = WeaponSetTemplate::default();
            s.weapons[0] = Some(Arc::new(w));
            s
        }),
        ..CombatTemplate::default()
    };
    t
}

fn wall() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Wall");
    t.kind_of = kind_of::STRUCTURE;
    t.geometry = Geometry { shape: GeometryShape::Box, major_radius: 20.0, minor_radius: 20.0, height: 20.0, is_small: false };
    t
}

fn dumb(first_height: f32, second_height: f32) -> ProjectileTemplate {
    ProjectileTemplate {
        behavior: ProjectileBehavior::Dumb(DumbProjectileParams { first_height, second_height, ..DumbProjectileParams::default() }),
        bounding_radius: 3.0,
        small_missile: false,
        neutron_warhead: None,
        neutron_blast: None,
        garrison_hit_kill: None,
    }
}

#[test]
fn dumb_projectile_arcs_over_a_wall() {
    let mut g = GameLogic::new(flat(), 1);
    let a = g.create_object(&Arc::new(tank("A", Some(weapon_with(Some(dumb(100.0, 100.0)), 20.0)))), Coord3D::new(100.0, 300.0, 0.0), 0.0, 0);
    let w = g.create_object(&Arc::new(wall()), Coord3D::new(250.0, 300.0, 0.0), 0.0, 1);
    let b = g.create_object(&Arc::new(tank("B", None)), Coord3D::new(400.0, 300.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(0, Command::Attack { ids: vec![a], target: b });
    g.run_frames(300);
    assert!(g.find_object(b).is_none_or(|o| o.health() < 200.0), "the arc should clear the wall and hit the target");
    assert_eq!(g.find_object(w).unwrap().health(), 100.0, "the wall is cleared");
}

#[test]
fn dumb_projectile_hits_a_wall_en_route() {
    let mut g = GameLogic::new(flat(), 1);
    let a = g.create_object(&Arc::new(tank("A", Some(weapon_with(Some(dumb(0.0, 0.0)), 10.0)))), Coord3D::new(100.0, 300.0, 0.0), 0.0, 0);
    let w = g.create_object(&Arc::new(wall()), Coord3D::new(250.0, 300.0, 0.0), 0.0, 1);
    let b = g.create_object(&Arc::new(tank("B", None)), Coord3D::new(400.0, 300.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(0, Command::Attack { ids: vec![a], target: b });
    g.run_frames(200);
    assert!(g.find_object(w).unwrap().health() < 100.0, "the shell hits the wall");
    assert_eq!(g.find_object(b).unwrap().health(), 200.0, "the wall shields the target");
}

#[test]
fn missile_homes_onto_a_moving_target() {
    let mut g = GameLogic::new(flat(), 1);
    let missile = ProjectileTemplate {
        behavior: ProjectileBehavior::Missile(MissileParams {
            try_to_follow_target: true,
            lock_distance: 25.0,
            initial_velocity: 12.0,
            ..MissileParams::default()
        }),
        bounding_radius: 3.0,
        small_missile: false,
        neutron_warhead: None,
        neutron_blast: None,
        garrison_hit_kill: None,
    };
    let a = g.create_object(&Arc::new(tank("A", Some(weapon_with(Some(missile), 12.0)))), Coord3D::new(100.0, 300.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(tank("B", None)), Coord3D::new(400.0, 300.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(0, Command::Attack { ids: vec![a], target: b });
    g.run_frames(5);
    // The target starts moving after the missile is in flight.
    g.submit(1, Command::MoveTo { ids: vec![b], pos: Coord3D::new(400.0, 700.0, 0.0) });
    g.run_frames(300);
    assert!(g.find_object(b).is_none_or(|o| o.health() < 200.0), "the missile should track and hit the mover");
}

#[test]
fn missile_detonates_when_out_of_fuel() {
    let mut g = GameLogic::new(flat(), 1);
    let missile = ProjectileTemplate {
        behavior: ProjectileBehavior::Missile(MissileParams {
            try_to_follow_target: true,
            lock_distance: 1.0,
            initial_velocity: 2.0,
            fuel_lifetime: 5,
            detonate_on_no_fuel: true,
            ..MissileParams::default()
        }),
        bounding_radius: 3.0,
        small_missile: false,
        neutron_warhead: None,
        neutron_blast: None,
        garrison_hit_kill: None,
    };
    let a = g.create_object(&Arc::new(tank("A", Some(weapon_with(Some(missile), 2.0)))), Coord3D::new(100.0, 300.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(tank("B", None)), Coord3D::new(300.0, 300.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(0, Command::Attack { ids: vec![a], target: b });
    g.run_frames(30);
    assert_eq!(g.combat_state().projectile_count(), 0, "the missile detonates out of fuel");
    assert_eq!(g.find_object(b).unwrap().health(), 200.0, "it never reached the target");
}

#[test]
fn turret_pitch_gates_firing() {
    let mut g = GameLogic::new(flat(), 1);
    let mut t = tank("T", Some(weapon_with(None, 999_999.0)));
    t.combat.turret_turn_rate = Some(1.0); // angle snaps immediately
    t.combat.turret_pitch_rate = Some(0.01); // 0.5 rad needs ~50 frames
    t.combat.turret_allows_pitch = true;
    t.combat.turret_fire_pitch = 0.5;
    t.combat.turret_min_pitch = 0.0;
    t.combat.turret_natural_pitch = 0.0;
    t.combat.turret_max_height = 10.0;
    let a = g.create_object(&Arc::new(t), Coord3D::new(100.0, 300.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(tank("B", None)), Coord3D::new(250.0, 300.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(0, Command::Attack { ids: vec![a], target: b });
    g.run_frames(20);
    assert_eq!(g.find_object(b).unwrap().health(), 200.0, "the turret has not pitched up yet");
    g.run_frames(60);
    assert!(g.find_object(b).unwrap().health() < 200.0, "fires once the pitch aligns");
}

#[test]
fn neutron_missile_climbs_to_the_intermediate_point_then_dives() {
    let mut g = GameLogic::new(flat(), 1);
    let neutron = ProjectileTemplate {
        behavior: ProjectileBehavior::Neutron(NeutronParams {
            template: NeutronMissileTemplate {
                distance_to_travel_before_turning: 50.0,
                max_turn_rate: 10.0,
                forward_damping: 0.1,
                relative_speed: 8.0,
                target_from_directly_above: 100.0,
                special_speed_time: 0,
                special_speed_height: 0.0,
                special_accel_factor: 1.0,
                special_jitter_distance: 0.0,
                launch_fx: None,
                ignition_fx: None,
            },
            dive_height: 10.0,
        }),
        bounding_radius: 3.0,
        small_missile: false,
        neutron_warhead: None,
        neutron_blast: None,
        garrison_hit_kill: None,
    };
    let a = g.create_object(&Arc::new(tank("A", Some(weapon_with(Some(neutron), 8.0)))), Coord3D::new(100.0, 300.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(tank("B", None)), Coord3D::new(300.0, 300.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    g.submit(0, Command::Attack { ids: vec![a], target: b });
    let mut max_z = 0.0f32;
    let mut seen = false;
    let mut dived = false;
    for _ in 0..400 {
        g.run_frames(1);
        let live = g.projectiles();
        if !live.is_empty() {
            seen = true;
            for p in live {
                max_z = max_z.max(p.pos.z);
            }
        } else if seen {
            dived = true;
            break;
        }
    }
    assert!(seen, "the missile was fired");
    assert!(max_z > 50.0, "the missile climbs toward the intermediate point (got {max_z})");
    assert!(dived, "the missile eventually detonates (the dive reaches the ground)");
}
