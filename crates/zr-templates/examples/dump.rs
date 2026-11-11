//! Loads the player's Zero Hour install, builds every template and prints counts, a warning
//! summary and one object (default `AmericaTankCrusader`) with its weapon, locomotor and armor.
//!
//! Usage: `cargo run --release -p zr-templates --example dump [-- ObjectName] [--warnings]`

use std::collections::BTreeMap;

use zr_assets::{Installs, Vfs};
use zr_templates::*;

fn main() {
    let mut name = "AmericaTankCrusader".to_string();
    let mut show_warnings = false;
    for a in std::env::args().skip(1) {
        if a == "--warnings" {
            show_warnings = true;
        } else {
            name = a;
        }
    }

    let Some(install) = Installs::detect().zero_hour else {
        eprintln!("Zero Hour install not found (set ZR_ZEROHOUR_DIR)");
        return;
    };
    let vfs = Vfs::zero_hour(&install).expect("mounting Zero Hour");
    let loaded = zr_ini::load_zero_hour(&vfs);
    let (t, warnings) = Templates::from_ini(&loaded.db);

    println!(
        "{} objects ({} reskins), {} weapons, {} locomotors, {} armors, {} upgrades, {} OCLs; gravity {} /frame^2",
        t.things().len(),
        t.things().iter().filter(|x| x.reskinned_from.is_some()).count(),
        t.weapons().len(),
        t.locomotors().len(),
        t.armors().len(),
        t.upgrades().len(),
        t.ocls().len(),
        t.gravity
    );
    if let Some(ai) = &t.ai_data {
        let structures: usize = ai.build_lists.iter().map(|b| b.structures.len()).sum();
        println!(
            "AIData: {} side infos, {} build lists ({structures} structures); StructureSeconds {} TeamSeconds {} RebuildDelay {}s",
            ai.side_infos.len(),
            ai.build_lists.len(),
            ai.structure_seconds,
            ai.team_seconds,
            ai.rebuild_delay_seconds
        );
        for b in &ai.build_lists {
            println!("  {}: {} structures", b.faction, b.structures.len());
        }
    }
    let with_ai = t.things().iter().filter(|x| x.ai().is_some()).count();
    let with_body = t.things().iter().filter(|x| x.body().is_some()).count();
    let raw = t
        .things()
        .iter()
        .flat_map(|x| &x.modules)
        .filter(|m| matches!(m.data, ModuleData::Unparsed { .. }))
        .count();
    let typed = t.things().iter().map(|x| x.modules.len()).sum::<usize>() - raw;
    println!("{with_body} objects with a body, {with_ai} with an AI module; {typed} typed and {raw} raw modules");
    let mut by_kind: BTreeMap<&str, usize> = BTreeMap::new();
    for m in t.things().iter().flat_map(|x| &x.modules) {
        let k = match &m.data {
            ModuleData::Body(_) => "body",
            ModuleData::Ai(_) => "ai",
            ModuleData::Physics(_) => "physics",
            ModuleData::Die(_) => "die",
            ModuleData::Upgrade(_) => "upgrade",
            ModuleData::Production(_) => "production",
            ModuleData::ProductionExit(_) => "production exit",
            ModuleData::Dock(_) => "dock",
            ModuleData::DumbProjectile(_) => "projectile",
            ModuleData::Contain(_) => "contain",
            ModuleData::SpecialPower(_) => "special power",
            ModuleData::SpecialAbility(_) => "special ability",
            ModuleData::AutoHeal(_) => "auto heal",
            ModuleData::Flammable(_) => "flammable",
            ModuleData::Poisoned(_) => "poisoned",
            ModuleData::FireWeaponWhenDead(_) => "fire weapon when dead",
            ModuleData::Lifetime(_) => "lifetime",
            ModuleData::Stealth(_) => "stealth",
            ModuleData::StealthDetector(_) => "stealth detector",
            ModuleData::SquishCollide(_) => "squish collide",
            ModuleData::CrateCollide(_) => "crate collide",
            ModuleData::FireWeaponCollide(_) => "fire weapon collide",
            ModuleData::VeterancyGainCreate(_) => "veterancy gain create",
            ModuleData::Topple(_) => "topple",
            ModuleData::Spawn(_) => "spawn",
            ModuleData::Slaved(_) => "slaved",
            ModuleData::Horde(_) => "horde",
            ModuleData::AutoDeposit(_) => "auto deposit",
            ModuleData::BaseRegenerate(_) => "base regenerate",
            ModuleData::Propaganda(_) => "propaganda",
            ModuleData::FireWeapon(_) => "fire weapon update",
            ModuleData::RepairDock(_) => "repair dock",
            ModuleData::Overcharge(_) => "overcharge",
            ModuleData::Minefield(_) => "minefield",
            ModuleData::SimpleUpdate(_) => "simple update",
            ModuleData::TransitionDamage(_) => "transition damage fx",
            ModuleData::ParkingPlace(_) => "parking place",
            ModuleData::Countermeasures(_) => "countermeasures",
            ModuleData::RebuildHole(_) => "rebuild hole",
            ModuleData::RebuildHoleExpose(_) => "rebuild hole expose",
            ModuleData::GenerateMinefield(_) => "generate minefield",
            ModuleData::Logic(_) => "logic",
            ModuleData::Draw(_) => "draw",
            ModuleData::Unparsed { .. } => "unparsed",
        };
        *by_kind.entry(k).or_default() += 1;
    }
    let breakdown: Vec<String> = by_kind.iter().map(|(k, n)| format!("{k} {n}")).collect();
    println!("  by kind: {}", breakdown.join(", "));
    let nuggets = t.ocls().iter().map(|o| o.nuggets.len()).sum::<usize>();
    let typed_nuggets = t
        .ocls()
        .iter()
        .flat_map(|o| &o.nuggets)
        .filter(|n| !matches!(n, Nugget::Unparsed { .. }))
        .count();
    println!("{nuggets} OCL nuggets, {typed_nuggets} typed and {} raw", nuggets - typed_nuggets);

    // Top unparsed module types by instance count (drives the module-coverage sweep).
    let mut unparsed_by_type: BTreeMap<&str, usize> = BTreeMap::new();
    for m in t.things().iter().flat_map(|x| &x.modules) {
        if let ModuleData::Unparsed { module_type, .. } = &m.data {
            *unparsed_by_type.entry(module_type.as_str()).or_default() += 1;
        }
    }
    let mut top: Vec<(&str, usize)> = unparsed_by_type.into_iter().collect();
    top.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    println!("top unparsed module types ({} distinct):", top.len());
    for (ty, n) in top.iter().take(50) {
        println!("  {n:>5}  {ty}");
    }

    // warning summary
    let mut by_kind: BTreeMap<(WarningKind, String), usize> = BTreeMap::new();
    for w in &warnings {
        let owner = w.scope.split(" > ").next().unwrap_or("").split(' ').next().unwrap_or("").to_string();
        *by_kind.entry((w.kind, owner)).or_default() += 1;
    }
    println!("{} warnings", warnings.len());
    for ((k, owner), n) in &by_kind {
        println!("  {k:?} in {owner}: {n}");
    }
    if show_warnings {
        for w in &warnings {
            println!("  {w}");
        }
    } else {
        for w in warnings.iter().take(15) {
            println!("  {w}");
        }
    }

    let Some(obj) = t.thing(&name) else {
        println!("\nObject {name} not found");
        return;
    };
    println!("\n=== Object {} (id {}) ===", obj.name, obj.id.0);
    print_thing(&t, obj);

    // the raw INI of everything printed above, to check the conversions by eye
    println!("
=== raw INI ===");
    if let Some(ai) = obj.ai() {
        for r in ai.locomotor_sets.values().flatten() {
            if let Some(b) = loaded.db.get("Locomotor", &r.name) {
                println!("{b}");
            }
        }
    }
    for s in &obj.weapon_sets {
        for r in s.weapons.iter().flatten() {
            if let Some(b) = loaded.db.get("Weapon", &r.name) {
                println!("{b}");
            }
        }
    }
    for s in &obj.armor_sets {
        if let Some(r) = &s.armor {
            if let Some(b) = loaded.db.get("Armor", &r.name) {
                println!("{b}");
            }
        }
    }
}

fn per_sec(v: f32) -> f32 {
    v * 30.0
}

fn print_thing(t: &Templates, o: &ThingTemplate) {
    println!("side {}  editor {:?}  command set {}", o.side, o.editor_sorting, o.command_set);
    println!("kind of: {:?}", o.kind_of);
    println!(
        "build cost {}  build time {}s  vision {}  shroud clearing {}  experience value {:?}  required {:?}",
        o.build_cost, o.build_time, o.vision_range, o.shroud_clearing_range, o.experience_value, o.experience_required
    );
    println!("prerequisites: {:?}", o.prerequisites);
    let g = &o.geometry;
    println!(
        "geometry {:?} major {} minor {} height {} small {} (bounding circle {}, sphere {})",
        g.geometry_type, g.major_radius, g.minor_radius, g.height, g.is_small, g.bounding_circle_radius, g.bounding_sphere_radius
    );
    if let Some(b) = o.body() {
        println!("body {:?}: max health {} initial {}", b.class, b.max_health, b.initial_health);
    }
    for m in &o.modules {
        let what = match &m.data {
            ModuleData::Body(_) => "typed body",
            ModuleData::Ai(_) => "typed AI",
            ModuleData::Physics(_) => "typed physics",
            ModuleData::Die(_) => "typed die",
            ModuleData::Upgrade(_) => "typed upgrade",
            ModuleData::Production(_) => "typed production",
            ModuleData::ProductionExit(_) => "typed production exit",
            ModuleData::Dock(_) => "typed dock",
            ModuleData::DumbProjectile(_) => "typed projectile",
            ModuleData::Contain(_) => "typed contain",
            ModuleData::SpecialPower(_) => "typed special power",
            ModuleData::SpecialAbility(_) => "typed special ability",
            ModuleData::AutoHeal(_) => "typed auto heal",
            ModuleData::Flammable(_) => "typed flammable",
            ModuleData::Poisoned(_) => "typed poisoned",
            ModuleData::FireWeaponWhenDead(_) => "typed fire weapon when dead",
            ModuleData::Lifetime(_) => "typed lifetime",
            ModuleData::Stealth(_) => "typed stealth",
            ModuleData::StealthDetector(_) => "typed stealth detector",
            ModuleData::SquishCollide(_) => "typed squish collide",
            ModuleData::CrateCollide(_) => "typed crate collide",
            ModuleData::FireWeaponCollide(_) => "typed fire weapon collide",
            ModuleData::VeterancyGainCreate(_) => "typed veterancy gain create",
            ModuleData::Topple(_) => "typed topple",
            ModuleData::Spawn(_) => "typed spawn",
            ModuleData::Slaved(_) => "typed slaved",
            ModuleData::Horde(_) => "typed horde",
            ModuleData::AutoDeposit(_) => "typed auto deposit",
            ModuleData::BaseRegenerate(_) => "typed base regenerate",
            ModuleData::Propaganda(_) => "typed propaganda",
            ModuleData::FireWeapon(_) => "typed fire weapon update",
            ModuleData::RepairDock(_) => "typed repair dock",
            ModuleData::Overcharge(_) => "typed overcharge",
            ModuleData::Minefield(_) => "typed minefield",
            ModuleData::SimpleUpdate(_) => "typed simple update",
            ModuleData::TransitionDamage(_) => "typed transition damage fx",
            ModuleData::ParkingPlace(_) => "typed parking place",
            ModuleData::Countermeasures(_) => "typed countermeasures",
            ModuleData::RebuildHole(_) => "typed rebuild hole",
            ModuleData::RebuildHoleExpose(_) => "typed rebuild hole expose",
            ModuleData::GenerateMinefield(_) => "typed generate minefield",
            ModuleData::Logic(_) => "typed logic",
            ModuleData::Draw(_) => "typed draw",
            ModuleData::Unparsed { fields, .. } => {
                if fields.is_empty() {
                    "raw, no fields"
                } else {
                    "raw"
                }
            }
        };
        println!(
            "  {:<12} {:<34} {:<40} mask {:#06x}{}{} ({what})",
            m.kind.name(),
            m.name,
            m.tag,
            m.interface_mask,
            if m.inheritable { " inheritable" } else { "" },
            if m.overrideable_by_like_kind { " overrideable" } else { "" },
        );
    }
    if let Some(ai) = o.ai() {
        println!(
            "AI {}: auto-acquire {:?}, mood check every {} frames",
            ai.class_name, ai.auto_acquire_enemies_when_idle, ai.mood_attack_check_rate
        );
        for (label, tur) in [("turret", &ai.turret), ("alt turret", &ai.alt_turret)] {
            if let Some(tur) = tur {
                println!(
                    "  {label}: turn {:.5} rad/frame ({:.1} deg/s), pitch {:.5} rad/frame, natural angle {} rad, fires while turning {}, weapon slots {:?}",
                    tur.turn_rate,
                    tur.turn_rate * 30.0 * 180.0 / std::f32::consts::PI,
                    tur.pitch_rate,
                    tur.natural_turret_angle,
                    tur.fires_while_turning,
                    tur.controlled_weapon_slots
                );
            }
        }
        for (set, locos) in &ai.locomotor_sets {
            for r in locos {
                println!("locomotor set {}: {} ({})", set.name(), r.name, if r.id.is_some() { "resolved" } else { "MISSING" });
                if let Some(l) = r.id.map(|i| t.locomotor_by_id(i)) {
                    print_locomotor(l);
                }
            }
        }
    }
    for s in &o.armor_sets {
        println!("armor set {:?}: {:?} damage fx {:?}", s.conditions, s.armor.as_ref().map(|r| &r.name), s.damage_fx);
        if let Some(a) = s.armor.as_ref().and_then(|r| r.id).map(|i| t.armor_by_id(i)) {
            print_armor(a);
        }
    }
    for s in &o.weapon_sets {
        println!("weapon set {:?}:", s.conditions);
        for (i, w) in s.weapons.iter().enumerate() {
            let Some(r) = w else { continue };
            println!("  slot {:?}: {}", WeaponSlot::ALL[i], r.name);
            if let Some(w) = r.id.map(|i| t.weapon_by_id(i)) {
                print_weapon(t, w);
            }
        }
    }
}

fn print_weapon(t: &Templates, w: &WeaponTemplate) {
    println!(
        "    damage {} radius {} ({:?}, death {:?}); secondary {} radius {}",
        w.primary_damage, w.primary_damage_radius, w.damage_type, w.death_type, w.secondary_damage, w.secondary_damage_radius
    );
    println!(
        "    range {} (min {}), weapon speed {} /frame ({} /s), aim delta {:.5} rad",
        w.attack_range,
        w.minimum_attack_range,
        w.weapon_speed,
        per_sec(w.weapon_speed),
        w.acceptable_aim_delta
    );
    println!(
        "    delay between shots {}..{} frames, clip {} reload {} frames, pre-attack {} frames ({:?}), reloads {:?}",
        w.min_delay_between_shots,
        w.max_delay_between_shots,
        w.clip_size,
        w.clip_reload_time,
        w.pre_attack_delay,
        w.pre_attack_type,
        w.auto_reloads_clip
    );
    println!(
        "    anti {:?}, radius damage affects {:?}, projectile {:?}",
        w.anti_mask,
        w.radius_damage_affects,
        w.projectile_object.as_ref().map(|r| (&r.name, r.id.map(|i| t.thing_by_id(i).name.as_str())))
    );
    println!("    weapon bonus {:?}", w.weapon_bonus);
}

fn print_locomotor(l: &LocomotorTemplate) {
    println!(
        "    surfaces {:?} appearance {:?} z {:?} priority {:?}",
        l.surfaces, l.appearance, l.behavior_z, l.move_priority
    );
    println!(
        "    speed {:.5}/frame ({} /s), damaged {:.5}, min speed {:.5}",
        l.max_speed,
        per_sec(l.max_speed),
        l.max_speed_damaged,
        l.min_speed
    );
    println!(
        "    turn rate {:.5} rad/frame ({:.1} deg/s), acceleration {:.6}/frame^2 ({:.2} /s^2), braking {:.6}/frame^2 ({:.2} /s^2)",
        l.max_turn_rate,
        l.max_turn_rate * 30.0 * 180.0 / std::f32::consts::PI,
        l.acceleration,
        l.acceleration * 900.0,
        l.braking,
        l.braking * 900.0
    );
    println!(
        "    preferred height {} (damping {}), min turn speed {:.5}, close enough {}, suspension {}, backwards {}",
        l.preferred_height, l.preferred_height_damping, l.min_turn_speed, l.close_enough_dist, l.has_suspension, l.can_move_backward
    );
}

fn print_armor(a: &ArmorTemplate) {
    let list: Vec<String> = DamageType::ALL
        .iter()
        .map(|&d| (d, a.coefficient(d)))
        .map(|(d, c)| format!("{}={:.0}%", d.name(), c * 100.0))
        .collect();
    println!("    armor {}: {}", a.name, list.join(" "));
}
