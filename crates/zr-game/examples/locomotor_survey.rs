//! Survey of the adapter's locomotor resolution over every shipped object template.
//!
//! For each of the 2115 `ThingTemplate`s it compares the sim locomotor set produced by the
//! adapter (`TemplateStore::find_template` -> `ObjectTemplate::locomotor_sets`) with the engine's
//! `Locomotor = SET_x ...` data, and reports the templates that differ:
//!
//! * `BuildVariations` blueprints (`ThingFactory::newObject` swaps to a variation before creation),
//! * `ObjectReskin` children that lost (or failed to inherit) the parent's set,
//! * a `Locomotor = SET_x <name>` whose locomotor is missing from the store (an unresolved ref),
//! * a `Locomotor = SET_NORMAL` with no name (the engine's `parseLocomotorSet` leaves the set
//!   empty, so the adapter resolving nothing is faithful).
//!
//! `cargo run --release -p zr-game --example locomotor_survey` (needs a Zero Hour install, or
//! `ZR_ZEROHOUR_DIR`).

use std::collections::BTreeMap;

use zr_assets::{Installs, Vfs};
use zr_game::GameData;
use zr_sim::template::LocomotorSet;
use zr_sim::TemplateSource;
use zr_templates::names::LocomotorSetType;

fn main() {
    let installs = Installs::detect();
    let Some(zh) = &installs.zero_hour else {
        eprintln!("no Zero Hour install found; set ZR_ZEROHOUR_DIR");
        return;
    };
    let vfs = Vfs::zero_hour(zh).expect("mounting Zero Hour");
    let data = GameData::load(&vfs);
    let t = data.templates();

    let mut with_ai = 0usize;
    let mut ai_with_normal = 0usize;
    let mut empty_set_templates: Vec<&str> = Vec::new();
    let mut unresolved_refs: Vec<(&str, &str)> = Vec::new();
    let mut blueprints: Vec<(&str, Vec<&str>, bool)> = Vec::new();
    let mut broken_reskins: Vec<(&str, &str)> = Vec::new();
    let mut normal_upgraded = 0usize;
    let mut upgrades_to_upgraded = 0usize;

    for thing in t.things() {
        // The adapter's converted template (build variations already resolved).
        let ot = data.store.find_template(&thing.name);
        let has_normal = ot
            .as_ref()
            .is_some_and(|o| o.locomotor_sets.contains_key(&LocomotorSet::Normal));
        if ot.as_ref().is_some_and(|o| o.locomotor_sets.contains_key(&LocomotorSet::NormalUpgraded)) {
            normal_upgraded += 1;
        }

        if let Some(ai) = thing.ai() {
            with_ai += 1;
            if has_normal {
                ai_with_normal += 1;
            }
            // A key with only empty refs is what the engine stores for a bare `Locomotor = SET_x`.
            if !has_normal && ai.locomotor_sets.contains_key(&LocomotorSetType::SetNormal) {
                empty_set_templates.push(thing.name.as_str());
            }
            // A named ref that failed to resolve is a real conversion gap.
            if let Some(refs) = ai.locomotor_sets.get(&LocomotorSetType::SetNormal) {
                for r in refs {
                    if r.id.is_none() {
                        unresolved_refs.push((thing.name.as_str(), r.name.as_str()));
                    }
                }
            }
        }
        if thing.modules.iter().any(|m| {
            matches!(&m.data, zr_templates::ModuleData::Upgrade(u)
                if matches!(u.kind, zr_templates::UpgradeKind::LocomotorSet))
        }) {
            upgrades_to_upgraded += 1;
        }

        if !thing.build_variations.is_empty() {
            let variations: Vec<&str> = thing.build_variations.iter().map(String::as_str).collect();
            blueprints.push((thing.name.as_str(), variations, has_normal));
        }
        if let Some(parent_name) = &thing.reskinned_from {
            let parent_normal = t.thing(parent_name).is_some_and(|p| {
                data.store
                    .find_template(&p.name)
                    .is_some_and(|o| o.locomotor_sets.contains_key(&LocomotorSet::Normal))
            });
            if parent_normal && !has_normal {
                broken_reskins.push((thing.name.as_str(), parent_name.as_str()));
            }
        }
    }

    println!("templates: {}", t.things().len());
    println!("with AI module: {with_ai}; with an adapter SET_NORMAL: {ai_with_normal}");
    println!("with SET_NORMAL_UPGRADED: {normal_upgraded}; carrying a LocomotorSetUpgrade: {upgrades_to_upgraded}");

    println!("\nBuildVariations blueprints: {}", blueprints.len());
    let mut blueprint_broken = 0;
    for (base, variations, has_normal) in &blueprints {
        if !has_normal {
            blueprint_broken += 1;
        }
        println!("  {base} (adapter SET_NORMAL: {has_normal}) -> {variations:?}");
    }
    println!("  blueprints still resolving no SET_NORMAL: {blueprint_broken}");

    println!("\nbare `Locomotor = SET_x` (empty set: faithful, engine stores nothing): {}", empty_set_templates.len());
    for n in empty_set_templates.iter().take(20) {
        println!("  {n}");
    }

    println!("\nunresolved named locomotor refs: {}", unresolved_refs.len());
    for (o, r) in unresolved_refs.iter().take(20) {
        println!("  {o}: `{r}`");
    }

    println!("\nreskins missing their parent's SET_NORMAL: {}", broken_reskins.len());
    for (child, parent) in broken_reskins.iter().take(20) {
        println!("  {child} <- {parent}");
    }

    let mut per_set: BTreeMap<&'static str, usize> = BTreeMap::new();
    for thing in t.things() {
        if let Some(o) = data.store.find_template(&thing.name) {
            for set in o.locomotor_sets.keys() {
                *per_set.entry(set_name(*set)).or_default() += 1;
            }
        }
    }
    let counts: Vec<String> = per_set.iter().map(|(k, n)| format!("{k} {n}")).collect();
    println!("\nadapter-resolved set counts: {}", counts.join(", "));
}

fn set_name(s: LocomotorSet) -> &'static str {
    match s {
        LocomotorSet::Normal => "SET_NORMAL",
        LocomotorSet::NormalUpgraded => "SET_NORMAL_UPGRADED",
        LocomotorSet::Freefall => "SET_FREEFALL",
        LocomotorSet::Wander => "SET_WANDER",
        LocomotorSet::Panic => "SET_PANIC",
        LocomotorSet::Taxiing => "SET_TAXIING",
        LocomotorSet::Supersonic => "SET_SUPERSONIC",
        LocomotorSet::Sluggish => "SET_SLUGGISH",
    }
}
