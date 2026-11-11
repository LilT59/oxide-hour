//! Lane 138 survey (real data, skips without an install): every `W3D*Draw` module of every shipped object, by class,
//! so the client can be checked against `Drawable::draw`'s loop over all draw modules (Drawable.cpp:2604-2653).

use std::collections::BTreeMap;

use zr_assets::{Installs, Vfs};
use zr_game::GameData;

fn vfs() -> Option<Vfs> {
    let zh = Installs::detect().zero_hour?;
    Vfs::zero_hour(&zh).ok()
}

#[test]
fn synthetic_module_flags_are_listed() {
    let Some(vfs) = vfs() else { return };
    let mut db = zr_ini::IniDb::new();
    db.extend(
        zr_ini::parse(
            "synthetic.ini",
            "Object TestParts\n  Draw = W3DModelDraw ModuleTag_01\n    DefaultConditionState\n      Model = A\n    End\n  End\n  Draw = W3DModelDraw ModuleTag_02\n    OkToChangeModelColor = Yes\n    MinLODRequired = Medium\n    DefaultConditionState\n      Model = B\n    End\n  End\n  Draw = W3DDependencyModelDraw ModuleTag_03\n    AttachToBoneInContainer = CHASSIS\n    DefaultConditionState\n      Model = C\n    End\n  End\nEnd\n",
        )
        .expect("synthetic INI"),
    );
    let modules = zr_game::launch_bones::weapon_draw_modules(&db, &vfs, "TestParts");
    assert_eq!(modules.len(), 3, "every W3D*Draw module is listed: {modules:?}");
    assert_eq!(modules[0].states.module, "W3DModelDraw");
    assert!(!modules[0].ok_to_change_model_color);
    assert!(modules[1].ok_to_change_model_color, "OkToChangeModelColor = Yes");
    assert_eq!(modules[1].min_lod.as_deref(), Some("Medium"), "MinLODRequired = Medium");
    assert_eq!(modules[2].states.module, "W3DDependencyModelDraw");
    assert_eq!(modules[2].attach_container_bone.as_deref(), Some("CHASSIS"));
    for (i, m) in modules.iter().enumerate() {
        assert_eq!(m.index, i, "the module index counts every Draw block");
        assert_eq!(m.states.find_best_state(&zr_w3d::ConditionFlags::new()).and_then(|s| s.model.as_deref()), Some(["A", "B", "C"][i]));
    }
}

#[test]
fn survey_every_draw_module() {
    let Some(vfs) = vfs() else { return };
    let data = GameData::load(&vfs);
    let ini = data.ini_db().expect("ini");
    let mut classes: BTreeMap<String, usize> = BTreeMap::new();
    let mut templates_with_extra = 0usize;
    let mut extra_modules = 0usize;
    let mut first_ok = 0usize;
    let mut first_total = 0usize;
    let mut first_container = 0usize;
    let mut extra_with_model = 0usize;
    let mut extra_default_model = 0usize;
    let mut templates_default_model = 0usize;
    let mut extra_lift = 0usize;
    let mut default_examples: Vec<String> = Vec::new();
    let mut extra_weapon = 0usize;
    let mut extra_attach = 0usize;
    let mut extra_container = 0usize;
    let mut ok_to_change = 0usize;
    let mut min_lod: BTreeMap<String, usize> = BTreeMap::new();
    let mut feedback = 0usize;
    let mut dyn_lights = 0usize;
    let mut power = 0usize;
    let mut examples: Vec<String> = Vec::new();
    for t in data.templates().things() {
        let modules = zr_w3d::draw_states_by_module(&ini, &t.name);
        if let Some((first_index, _)) = modules.first() {
            first_total += 1;
            let (ok, container) = ini.resolved("Object", &t.name).map_or((false, false), |obj| {
                let block = obj.children_named("Draw").nth(*first_index);
                (
                    block.and_then(|b| b.value("OkToChangeModelColor")).is_some_and(|v| v.trim().eq_ignore_ascii_case("yes")),
                    block
                        .and_then(|b| b.value("AttachToBoneInContainer"))
                        .is_some_and(|v| !v.trim().is_empty() && !v.trim().eq_ignore_ascii_case("none")),
                )
            });
            if ok {
                first_ok += 1;
            }
            if container {
                first_container += 1;
            }
        }
        if modules.len() <= 1 {
            continue;
        }
        templates_with_extra += 1;
        let Some(obj) = ini.resolved("Object", &t.name) else { continue };
        let draws: Vec<_> = obj.children_named("Draw").collect();
        let mut has_default = false;
        for (i, m) in modules.iter().enumerate() {
            *classes.entry(m.1.module.clone()).or_default() += 1;
            if i == 0 {
                continue;
            }
            extra_modules += 1;
            let has_model = m.1.states.iter().any(|s| s.model.is_some());
            if has_model {
                extra_with_model += 1;
            }
            if let Some(st) = m.1.find_best_state(&zr_w3d::ConditionFlags::new()) {
                if let Some(model) = &st.model {
                    extra_default_model += 1;
                    has_default = true;
                    if default_examples.len() < 40 {
                        default_examples.push(format!("{}[{}] {}", t.name, m.0, model));
                    }
                }
                if st.flags.iter().any(|f| f.eq_ignore_ascii_case("ADJUST_HEIGHT_BY_CONSTRUCTION_PERCENT")) {
                    extra_lift += 1;
                }
            }
            let weapon = m.1.states.iter().any(|s| {
                s.weapon_fire_fx_bone.iter().any(Option::is_some)
                    || s.weapon_recoil_bone.iter().any(Option::is_some)
                    || s.weapon_muzzle_flash.iter().any(Option::is_some)
                    || s.weapon_launch_bone.iter().any(Option::is_some)
            });
            if weapon {
                extra_weapon += 1;
            }
            let block = draws.get(m.0);
            let field = |k: &str| block.and_then(|b| b.value(k)).map(str::trim).unwrap_or("");
            if !field("AttachToBoneInAnotherModule").is_empty() {
                extra_attach += 1;
            }
            if !field("AttachToBoneInContainer").is_empty() {
                extra_container += 1;
            }
            if field("OkToChangeModelColor").eq_ignore_ascii_case("yes") {
                ok_to_change += 1;
            }
            let lod = field("MinLODRequired");
            if !lod.is_empty() {
                *min_lod.entry(lod.to_string()).or_default() += 1;
            }
            if !field("ProjectileBoneFeedbackEnabledSlots").is_empty() {
                feedback += 1;
            }
            if field("ReceivesDynamicLights").eq_ignore_ascii_case("yes") {
                dyn_lights += 1;
            }
            if field("AnimationsRequirePower").eq_ignore_ascii_case("yes") {
                power += 1;
            }
            if examples.len() < 40 && has_model {
                examples.push(format!("{}[{}] {}{}", t.name, m.0, m.1.module, if weapon { " (weapon)" } else { "" }));
            }
        }
        if has_default {
            templates_default_model += 1;
        }
    }
    println!("templates with >1 draw module: {templates_with_extra}");
    println!("first modules OkToChangeModelColor: {first_ok}/{first_total}");
    println!("first modules AttachToBoneInContainer (dependency riders): {first_container}");
    println!("extra modules: {extra_modules} (with a model: {extra_with_model}; weapon bones: {extra_weapon})");
    println!("extra with a default-state model: {extra_default_model}; templates with a default-visible extra part: {templates_default_model}; default state ADJUST_HEIGHT_BY_CONSTRUCTION_PERCENT: {extra_lift}");
    println!("extra AttachToBoneInAnotherModule: {extra_attach}; AttachToBoneInContainer: {extra_container}");
    println!("extra OkToChangeModelColor: {ok_to_change}; ProjectileBoneFeedback slots: {feedback}; ReceivesDynamicLights: {dyn_lights}; AnimationsRequirePower: {power}");
    println!("extra MinLODRequired: {min_lod:?}");
    println!("all modules by class: {classes:?}");
    for e in &examples {
        println!("  {e}");
    }
    println!("default-state model examples:");
    for e in &default_examples {
        println!("  {e}");
    }
    assert!(templates_with_extra > 100, "the shipped multi-module objects: {templates_with_extra}");
    assert!(extra_with_model > 100, "the extra modules with a model: {extra_with_model}");
    assert_eq!(first_container, 26, "the shipped W3DDependencyModelDraw riders attached to a carrier bone");
    assert!(extra_modules >= 800 && extra_with_model >= 800, "the extra module coverage: {extra_modules}/{extra_with_model}");
}
