//! Survey of the shipped `Collide` module families: which templates use each class and how many,
//! plus the module list of the key crate/tree templates.
//!
//! `cargo run --release -p zr-game --example collide_survey` (needs a Zero Hour install, or
//! `ZR_ZEROHOUR_DIR`).

use std::collections::BTreeMap;

use zr_assets::{Installs, Vfs};
use zr_game::GameData;

fn main() {
    let installs = Installs::detect();
    let Some(zh) = &installs.zero_hour else {
        eprintln!("no Zero Hour install found; set ZR_ZEROHOUR_DIR");
        return;
    };
    let vfs = Vfs::zero_hour(zh).expect("mounting Zero Hour");
    let data = GameData::load(&vfs);

    let mut class_templates: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for t in data.templates().things() {
        for m in &t.modules {
            if m.name.ends_with("Collide") || m.name == "PhysicsBehavior" {
                class_templates.entry(m.name.clone()).or_default().push(t.name.clone());
            }
        }
    }
    for (class, templates) in &class_templates {
        let sample: Vec<&str> = templates.iter().take(4).map(String::as_str).collect();
        println!("{class}: {} templates {sample:?}", templates.len());
    }

    for name in ["SmallLevelUpCrate", "SalvageCrate", "1000DollarCrate", "GenericTree", "GLAInfantrySaboteur"] {
        if let Some(t) = data.templates().thing(name) {
            let mods: Vec<&str> = t.modules.iter().map(|m| m.name.as_str()).collect();
            println!("{name} modules: {mods:?}");
        }
    }
}
