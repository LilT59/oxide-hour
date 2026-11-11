//! Diagnostic: list enclosing garrisons whose `FIREPOINT` bones the adapter failed to resolve, with
//! the reason (no `GARRISONED` state, model/hierarchy missing, or bones live in a sub-object).
//! `cargo run -p zr-game --example firepoint_diag`.

use std::sync::Arc;

use zr_assets::{Installs, Vfs};
use zr_game::GameData;
use zr_templates::{ContainKind as TplContainKind, ModuleData};

fn fp_count(vfs: &Vfs, name: &str) -> usize {
    let Ok(h) = zr_w3d::load_hierarchy(vfs, name) else { return usize::MAX };
    let mut n = 0;
    for i in 1..=99 {
        if h.bone_index(&format!("FIREPOINT{i:02}")).is_some() {
            n += 1;
        } else {
            break;
        }
    }
    n
}

fn main() {
    let installs = Installs::detect();
    let Some(zh) = &installs.zero_hour else {
        eprintln!("no Zero Hour install found; set ZR_ZEROHOUR_DIR");
        return;
    };
    let vfs = Vfs::zero_hour(zh).expect("mounting Zero Hour");
    if let Some(model) = std::env::args().nth(1) {
        if let Ok(h) = zr_w3d::load_hierarchy(&vfs, &model) {
            let pivots: Vec<&str> = h.pivots.iter().map(|p| p.name.as_str()).filter(|n| n.to_ascii_uppercase().contains("FIRE") || n.to_ascii_uppercase().contains("STATION")).collect();
            println!("{model}: {} pivots, matching: {pivots:?}", h.pivot_count());
        } else {
            println!("{model}: no hierarchy chunk");
        }
        match zr_w3d::load_model(&vfs, &model) {
            Ok(m) => {
                let names: Vec<&str> = m.meshes.iter().map(|x| x.name.as_str()).filter(|n| n.to_ascii_uppercase().contains("FIRE") || n.to_ascii_uppercase().contains("STATION")).collect();
                println!("{model}: {} meshes, matching: {names:?}", m.meshes.len());
            }
            Err(e) => println!("{model}: load_model err {e}"),
        }
        return;
    }
    let data = Arc::new(GameData::load(&vfs));
    let db = data.ini_db().expect("ini db");
    let fire_map = zr_game::firepoints::garrison_fire_points(&db, &vfs, data.templates());

    let is_enclosing = |t: &zr_templates::ThingTemplate| {
        t.modules.iter().any(|m| matches!(&m.data, ModuleData::Contain(c) if c.kind == TplContainKind::Garrison && c.is_enclosing_container))
    };
    let garrisons: Vec<_> = data.templates().things().iter().filter(|t| is_enclosing(t)).collect();
    let failing: Vec<_> = garrisons.iter().filter(|t| fire_map.get(&t.name).is_none_or(|f| f.conditions[0].is_empty())).collect();
    println!("enclosing garrisons: {}/{} pristine FIREPOINT", garrisons.len() - failing.len(), garrisons.len());
    let non_enclosing: Vec<_> = data
        .templates()
        .things()
        .iter()
        .filter(|t| t.modules.iter().any(|m| matches!(&m.data, ModuleData::Contain(c) if c.kind == TplContainKind::Garrison && !c.is_enclosing_container)))
        .collect();
    println!("non-enclosing garrisons: {}", non_enclosing.len());
    for t in &non_enclosing {
        let n = fire_map.get(&t.name).map_or(0, |f| f.station_points.len());
        println!("  {}: {n} STATION bones", t.name);
    }
    let turret_containers: Vec<_> = data
        .templates()
        .things()
        .iter()
        .filter(|t| t.modules.iter().any(|m| matches!(&m.data, ModuleData::Contain(c) if c.passengers_in_turret)))
        .collect();
    println!("PassengersInTurret containers: {}", turret_containers.len());
    for t in &turret_containers {
        let f = fire_map.get(&t.name);
        println!(
            "  {}: {} turret FIREPOINT bones, pivot {:?}",
            t.name,
            f.map_or(0, |f| f.turret_points.len()),
            f.and_then(|f| f.turret_pivot)
        );
    }
    for t in &failing {
        let Some(states) = zr_w3d::states_for_template(&db, &t.name) else {
            println!("{}: NO DRAW STATES", t.name);
            continue;
        };
        let g = states.find_best_state(&zr_w3d::ConditionFlags::from_names(["GARRISONED"]));
        let has_g_state = g.is_some_and(|s| s.conditions.iter().any(|c| c.contains("GARRISONED")));
        let model = g.and_then(|s| s.model.clone()).unwrap_or_default();
        let direct = fp_count(&vfs, &model);
        let mut anim_hier = Vec::new();
        if let Some(s) = g {
            for anim in &s.animations {
                if let Ok(a) = zr_w3d::load_animation(&vfs, &anim.name) {
                    anim_hier.push((a.hierarchy.clone(), fp_count(&vfs, &a.hierarchy)));
                }
            }
        }
        let meshes = zr_w3d::load_model(&vfs, &model).map(|m| {
            m.meshes.iter().filter(|x| x.name.to_ascii_uppercase().contains("FIRE") || x.name.to_ascii_uppercase().contains("POINT")).map(|x| x.name.clone()).collect::<Vec<_>>()
        }).unwrap_or_default();
        // Raw file string scan: catches `FIREPOINT` null sub-objects the mesh loader skips.
        let raw = vfs
            .read(&format!("art\\w3d\\{}.w3d", model.trim_end_matches(".w3d")))
            .map(|b| {
                let up: Vec<u8> = b.iter().map(u8::to_ascii_uppercase).collect();
                let count = |n: &[u8]| up.windows(n.len()).filter(|w| *w == n).count();
                (count(b"FIREPOINT"), count(b"STATION"))
            })
            .unwrap_or((0, 0));
        println!(
            "{}: model={model:?} g_state={has_g_state} direct_fp={:?} anim_hier={anim_hier:?} fire_meshes={meshes:?} raw(fire,station)={raw:?}",
            t.name,
            if direct == usize::MAX { "no-hierarchy".to_string() } else { direct.to_string() },
        );
    }
}
