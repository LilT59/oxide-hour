//! Survey of the shipped projectile weapons (lane 60): for every weapon with a `ProjectileObject`, the
//! projectile's behavior (`MissileAIUpdate` or `DumbProjectileBehavior`), its `PhysicsBehavior`, the
//! `SET_NORMAL` locomotor fields the missile uses (`Speed`, `Acceleration`, `MinSpeed`,
//! `PreferredHeight`, `MaxThrustAngle`, `Braking`, `BehaviorZ`), and the objects whose normal weapon
//! set fires it.
//!
//! `cargo run --release -p zr-game --example missile_survey` (needs a Zero Hour install, or
//! `ZR_ZEROHOUR_DIR`).

use std::collections::BTreeMap;

use zr_assets::{Installs, Vfs};
use zr_game::GameData;
use zr_sim::TemplateSource;
use zr_templates::{LocomotorSetType, ModuleData, WeaponSetFlags};

fn main() {
    let installs = Installs::detect();
    let Some(zh) = &installs.zero_hour else {
        eprintln!("no Zero Hour install found; set ZR_ZEROHOUR_DIR");
        return;
    };
    let vfs = Vfs::zero_hour(zh).expect("mounting Zero Hour");
    let data = GameData::load(&vfs);
    let t = data.templates();

    // Weapon name -> firing object names (normal weapon set, all three slots).
    let mut fired_by: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for thing in t.things() {
        if let Some(ws) = thing.weapon_set_for(WeaponSetFlags::EMPTY) {
            for r in ws.weapons.iter().flatten() {
                fired_by.entry(r.name.clone()).or_default().push(thing.name.clone());
            }
        }
    }

    let mut missiles = 0usize;
    let mut dumbs = 0usize;
    let mut neutrons = 0usize;
    let mut dumb_with_physics = 0usize;
    let mut missile_with_physics = 0usize;
    for w in t.weapons() {
        let Some(pref) = &w.projectile_object else { continue };
        let Some(pid) = pref.id else {
            println!("{} -> unresolved ProjectileObject {}", w.name, pref.name);
            continue;
        };
        let p = t.thing_by_id(pid);
        let physics = p.modules.iter().any(|m| matches!(m.data, ModuleData::Physics(_)));
        let dumb = p.modules.iter().find_map(|m| match &m.data {
            ModuleData::DumbProjectile(d) => Some(d),
            _ => None,
        });
        let missile = p.ai().and_then(|a| a.missile.as_deref());
        let neutron = p.modules.iter().any(|m| m.name == "NeutronMissileUpdate");
        let firers = fired_by.get(&w.name).map_or_else(String::new, |v| {
            let mut v = v.clone();
            v.sort();
            v.dedup();
            v.join(",")
        });
        let kind = if let Some(m) = missile {
            missiles += 1;
            if physics {
                missile_with_physics += 1;
            }
            let loco = p
                .ai()
                .and_then(|a| a.locomotor_sets.get(&LocomotorSetType::SetNormal))
                .and_then(|refs| refs.first())
                .and_then(|r| t.locomotor(&r.name));
            let l = match loco {
                Some(l) => format!(
                    "loco {} speed {:.3} accel {:.3} minSpeed {:.3} prefH {} damp {} thrustAng {:.1}deg braking {} behZ {:?} app {:?}",
                    loco.map_or("", |x| x.name.as_str()),
                    l.max_speed,
                    l.acceleration,
                    l.min_speed,
                    l.preferred_height,
                    l.preferred_height_damping,
                    l.max_thrust_angle.to_degrees(),
                    l.braking,
                    l.behavior_z,
                    l.appearance
                ),
                None => "no SET_NORMAL locomotor".to_string(),
            };
            format!(
                "MISSILE dive {} lock {} tryFollow {} useWeaponSpeed {} fuel {} ignition {} initVel {} initDist {} physics {} | {}",
                m.dive_distance, m.lock_distance, m.try_to_follow_target, m.use_weapon_speed, m.fuel_lifetime, m.ignition_delay,
                m.initial_velocity, m.initial_dist, physics, l
            )
        } else if let Some(d) = dumb {
            dumbs += 1;
            if physics {
                dumb_with_physics += 1;
            }
            format!("DUMB physics {physics} maxLife {} path {} {} {} {}", d.max_lifespan, d.first_height, d.second_height, d.first_percent_indent, d.second_percent_indent)
        } else if neutron {
            neutrons += 1;
            format!("NEUTRON physics {physics}")
        } else {
            format!("OTHER ({} modules) physics {physics}", p.modules.len())
        };
        let order: Vec<&str> = p.modules.iter().map(|m| m.name.as_str()).collect();
        println!(
            "{} speed {} minSpeed {} scale {} proj {} | {} | modules {:?} | fired by [{}]",
            w.name, w.weapon_speed, w.min_weapon_speed, w.scale_weapon_speed, p.name, kind, order, firers
        );
    }
    println!(
        "\nmissiles {missiles} (with PhysicsBehavior {missile_with_physics}), dumb shells {dumbs} (with PhysicsBehavior {dumb_with_physics}), neutron {neutrons}"
    );

    // Launch bones (lane 60 item 5): the barrels whose bind-pose rest rotation is not the identity, with the angle
    // their X axis turns through (`quat_rotate` of (1,0,0)), and the weapon slot.
    let mut bones = 0usize;
    let mut turned = 0usize;
    for thing in t.things() {
        let Some(o) = data.store.find_template(&thing.name) else { continue };
        for (slot, barrels) in o.combat.launch_bones.slots.iter().enumerate() {
            for (i, b) in barrels.iter().enumerate() {
                bones += 1;
                let [x, y, z, w] = b.rot;
                let x_axis_y = 2.0 * (x * y + z * w);
                let x_axis_z = 2.0 * (x * z - y * w);
                let x_axis_x = 1.0 - 2.0 * (y * y + z * z);
                let turn = x_axis_x.clamp(-1.0f32, 1.0).acos().to_degrees();
                if (x_axis_y.abs() + x_axis_z.abs()) > 1e-4 || x_axis_x < 0.9999 {
                    turned += 1;
                    if turned <= 400 {
                        println!("  turned barrel: {} slot {slot} barrel {i} rot {:?} X-axis {:.1} deg from +X", thing.name, b.rot, turn);
                    }
                }
            }
        }
    }
    println!("launch barrels: {bones}, with a non-identity rest rotation: {turned}");

    // Lane 60 wave 11b: shells' tumble and orientation flags and PhysicsBehavior friction (Forward, Lateral,
    // Aerodynamic), counted over the projectile objects that fire from shipped weapons.
    let mut tumbling = 0usize;
    let mut shell_friction: BTreeMap<String, usize> = BTreeMap::new();
    for w in t.weapons() {
        let Some(pid) = w.projectile_object.as_ref().and_then(|r| r.id) else { continue };
        let p = t.thing_by_id(pid);
        let Some(d) = p.modules.iter().find_map(|m| match &m.data {
            ModuleData::DumbProjectile(d) => Some(d),
            _ => None,
        }) else {
            continue;
        };
        if d.tumble_randomly {
            tumbling += 1;
            println!("  tumbling shell: {} (weapon {})", p.name, w.name);
        }
        let phys = p.modules.iter().find_map(|m| match &m.data {
            ModuleData::Physics(ph) => Some((ph.forward_friction, ph.lateral_friction, ph.aerodynamic_friction)),
            _ => None,
        });
        *shell_friction.entry(format!("{phys:?} orient {}", d.orient_to_flight_path)).or_default() += 1;
    }
    println!("shells with TumbleRandomly: {tumbling}");
    println!("shell friction (forward, lateral, aero) and orient:");
    for (k, n) in &shell_friction {
        println!("  {n} x {k}");
    }

    // Turret art (lane 60 item 5): the default state's TurretArtAngle / TurretArtPitch for every object.
    let ini = data.ini_db().expect("ini db");
    let mut art = 0usize;
    let mut any_state_art = 0usize;
    for thing in t.things() {
        let Some(states) = zr_w3d::states_for_template(&ini, &thing.name) else { continue };
        if states.states.iter().any(|s| s.turret_art_angle != 0.0 || s.turret_art_pitch != 0.0) {
            any_state_art += 1;
        }
        let Some(state) = states.default_state() else { continue };
        if state.turret_art_angle != 0.0 || state.turret_art_pitch != 0.0 {
            art += 1;
            if art <= 60 {
                println!(
                    "  turret art: {} angle {:.2} deg pitch {:.2} deg",
                    thing.name,
                    state.turret_art_angle.to_degrees(),
                    state.turret_art_pitch.to_degrees()
                );
            }
        }
    }
    println!("objects whose default state sets TurretArtAngle/Pitch: {art}");
    println!("objects whose any state sets TurretArtAngle/Pitch: {any_state_art}");
}
