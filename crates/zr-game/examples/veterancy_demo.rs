//! Veterancy and upgrades with real Zero Hour data: a Crusader kills rebels to promote, and an
//! America upgrade raises its max health.
//!
//! `cargo run --release -p zr-game --example veterancy_demo` (needs a Zero Hour install, or `ZR_ZEROHOUR_DIR`).

use std::sync::Arc;
use zr_assets::{Installs, Vfs};
use zr_game::GameData;
use zr_sim::{Coord3D, GameLogic, ObjectId, TemplateSource, TerrainLogic};

fn main() {
    let installs = Installs::detect();
    let Some(zh) = &installs.zero_hour else {
        eprintln!("no Zero Hour install found; set ZR_ZEROHOUR_DIR");
        return;
    };
    let vfs = Vfs::zero_hour(zh).expect("mounting Zero Hour");
    let data = Arc::new(GameData::load(&vfs));

    let Some(crusader) = data.store.find_template("AmericaTankCrusader") else {
        eprintln!("no AmericaTankCrusader template");
        return;
    };
    println!("Crusader: trainable {} experience_value {:?} experience_required {:?}",
        crusader.is_trainable, crusader.experience_value, crusader.experience_required);

    // ---- veterancy: kill rebels until promoted ----
    use zr_sim::body::DamageInfo;
    use zr_sim::damage::{DamageType, DeathType};
    let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
    {
        let t = data.templates();
        g.set_global_bonuses(zr_game::adapter::global_weapon_bonus(t), t.health_bonus);
        g.set_upgrade_info(zr_game::adapter::upgrade_info(t));
        println!("global WeaponBonus entries {}, HealthBonus {:?}", t.weapon_bonus.len(), t.health_bonus);
    }
    let a = g.create_object(&crusader, Coord3D::new(600.0, 1000.0, 0.0), 0.0, 0);
    let rebel = data.store.find_template("GLAInfantryRebel").or_else(|| data.store.find_template("ChinaInfantryRedguard"));
    let Some(rebel) = rebel else {
        eprintln!("no rebel template");
        return;
    };
    println!("rebel experience_value {:?}", rebel.experience_value);
    let weapon = g.find_object(a).and_then(|o| o.combat.weapons.weapon(0)).map(|w| w.template.clone());
    let stats = |g: &GameLogic, id: ObjectId| {
        weapon.as_ref().map(|w| {
            let b = g.combined_weapon_bonus(id, w);
            (
                w.primary_damage * b.get(zr_sim::bonus::WeaponBonusField::Damage),
                w.attack_range() * b.get(zr_sim::bonus::WeaponBonusField::Range),
                b.get(zr_sim::bonus::WeaponBonusField::RateOfFire),
            )
        })
    };
    if let Some((d, r, rof)) = stats(&g, a) {
        println!("regular: damage {d:.1}, range {r:.1}, rate x{rof:.2}");
    }
    let mut kills = 0;
    for _ in 0..20 {
        let level_before = g.find_object(a).map(|o| o.experience.level);
        let b = g.create_object(&rebel, Coord3D::new(720.0, 1000.0, 0.0), 0.0, 1);
        // The Crusader kills the rebel (a real template kill feeds the experience tracker).
        g.damage_object_with_death(b, DamageInfo { source: a, amount: 1_000_000.0, kind: DamageType::Unresistable }, DeathType::Normal);
        g.update();
        kills += 1;
        let level_after = g.find_object(a).unwrap().experience.level;
        if Some(level_after) != level_before {
            println!("after {kills} kills: promoted to {level_after:?}");
        }
        if level_after as u8 >= 2 {
            break;
        }
    }
    if let Some(o) = g.find_object(a) {
        println!("Crusader final: level {:?}, {} exp", o.experience.level, o.experience.current);
    }
    if let Some((d, r, rof)) = stats(&g, a) {
        println!("promoted: damage {d:.1}, range {r:.1}, rate x{rof:.2}");
    }

    // ---- upgrade: America composite armor / max health ----
    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
    let a: ObjectId = g.create_object(&crusader, Coord3D::new(300.0, 300.0, 0.0), 0.0, 0);
    let before = g.find_object(a).unwrap().body.as_ref().unwrap().max_health();
    for name in ["Upgrade_AmericaCompositeArmor", "Upgrade_AmericaAdvancedTraining", "Upgrade_AmericaSupplyLines"] {
        g.apply_player_upgrade(0, name);
    }
    let after = g.find_object(a).unwrap().body.as_ref().unwrap().max_health();
    println!("Crusader max health {before} -> {after} after player upgrades");
}
