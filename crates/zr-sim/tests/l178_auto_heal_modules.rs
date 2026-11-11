//! An object with two `AutoHealBehavior` modules keeps both (lane 178). The retail GLA Scorpion Tank carries the
//! inherited veterancy module (`DefaultThingTemplate`'s `ModuleTag_DefaultAutoHealBehavior`) and its own
//! `ModuleTag_05` (`TriggeredBy = Upgrade_GLAJunkRepair`); C++ `Object::Object` builds one module per INI entry, each
//! with its own `UpgradeMux` and heap slot (glavehicle.ini, DefaultThingTemplate in default/object.ini,
//! AutoHealBehavior.cpp:90-112).

use std::sync::Arc;

use zr_sim::behaviors::AutoHealTemplate;
use zr_sim::body::DamageInfo;
use zr_sim::damage::DamageType;
use zr_sim::{Coord3D, GameLogic, ObjectId, ObjectTemplate, TerrainLogic};

fn heal(trigger: &str, amount: f32) -> AutoHealTemplate {
    AutoHealTemplate {
        healing_amount: amount,
        healing_delay: 5,
        start_healing_delay: 0,
        radius: 0.0,
        affects_whole_player: false,
        skip_self_for_healing: false,
        single_burst: false,
        kind_of: 0,
        forbidden_kind_of: 0,
        starts_active: false,
        triggered_by: vec![trigger.to_owned()],
        conflicts_with: vec![],
    }
}

/// The veterancy module first (the inherited one comes first in the template's module order), then the own module.
fn scorpion_like() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("TwoHeals");
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.auto_heal = vec![heal("Upgrade_Veterancy_ELITE", 10.0), heal("Upgrade_GLAJunkRepair", 2.0)];
    t
}

fn hurt_unit(g: &mut GameLogic) -> ObjectId {
    let id = g.create_object(&Arc::new(scorpion_like()), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    g.damage_object(id, DamageInfo { source: ObjectId::INVALID, amount: 50.0, kind: DamageType::Explosion });
    id
}

/// The own module (the second one) heals when its own upgrade lands, and the veterancy module stays off for it.
/// Revert check: keep only the first module (`family_registrations` registers `template.auto_heal` up to one entry, or
/// the adapter keeps only the first AutoHeal): the second module has no slot, so the Junk Repair grant heals nothing.
#[test]
fn the_second_auto_heal_module_heals_on_its_own_upgrade() {
    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
    let id = hurt_unit(&mut g);
    let hurt = g.find_object(id).unwrap().health();
    g.run_frames(10);
    assert_eq!(g.find_object(id).unwrap().health(), hurt, "neither module is active before its upgrade");
    g.give_upgrade(id, "Upgrade_GLAJunkRepair");
    g.run_frames(10);
    let healed = g.find_object(id).unwrap().health();
    assert!(healed > hurt, "healed by the own module after its upgrade (health {healed}, was {hurt})");
    // Only the own module's amount is applied per pulse: 2 HP, never the veterancy module's 10.
    assert!(healed - hurt <= 2.0 * 3.0 + 0.001, "the veterancy module's 10 HP pulse must not run (gained {})", healed - hurt);
}

/// The veterancy module activates on its own upgrade, independently of the other one.
/// Revert check: as above, the first module must be registered too.
#[test]
fn the_veterancy_module_still_heals_on_its_own_upgrade() {
    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
    let id = hurt_unit(&mut g);
    let hurt = g.find_object(id).unwrap().health();
    g.give_upgrade(id, "Upgrade_Veterancy_ELITE");
    g.run_frames(10);
    assert!(g.find_object(id).unwrap().health() > hurt, "healed by the veterancy module after its upgrade");
}
