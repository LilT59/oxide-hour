//! Lane 173 (wave 20): R19-8 — `Object::kill`'s damage carries no source (`Object.cpp:2010-2024`: `m_sourceID =
//! INVALID_ID`), so nothing is credited to the object itself.

use super::*;
use crate::template::ObjectTemplate;
use crate::terrain::TerrainLogic;
use std::sync::Arc;

/// A `kill_object` hit records no last attacker: `Object::kill` builds its `DamageInfo` with `m_sourceID = INVALID_ID`
/// (`Object.cpp:2017`), so `ActiveBody::attemptDamage`'s last-attacker block (`:527-600`) is skipped.
/// Revert check: pass `id` as the damage's source again; the object records itself as its last attacker.
#[test]
fn a_killed_object_is_not_its_own_killer() {
    let mut t = ObjectTemplate::new("Dud");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.geometry.major_radius = 4.0;
    // `FXListDie` keeps the object alive after the lethal hit (`Object::onDie` runs, the object is not destroyed), so
    // the recorded attacker can be read back.
    t.die = vec![crate::die::DieModuleTemplate {
        mux: crate::die::DieMux {
            death_types: 1u32 << (DeathType::Normal as u32),
            veterancy_levels: 1u32 << 31, // REGULAR
            ..Default::default()
        },
        kind: crate::die::DieTemplate::FxList { default_death_fx: None, orient_to_object: false, initially_active: true },
    }];
    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
    let id = g.create_object(&Arc::new(t), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
    g.kill_object(id);
    let o = g.find_object(id).expect("the FxList die leaves the object in place");
    assert_eq!(o.last_damage_info.source, ObjectId::INVALID, "`Object::kill`'s damage has no source (`Object.cpp:2017`)");
}
