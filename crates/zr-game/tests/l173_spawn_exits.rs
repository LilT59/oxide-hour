//! Lane 173 (wave 20): R19 suspicion 1 — which exit an object with a `SpawnBehavior` has. The engine's `createSpawn`
//! uses `parent->getObjectExitInterface()` (`SpawnBehavior.cpp:601-610`), i.e. whatever module provides `ExitInterface`
//! (a `ProductionUpdate`'s exit update, or the contain class), not `template.production` alone. The port keys its
//! routing on `template.production` (`modules2::spawn_create_spawn`), which misses a spawn point exit module on a
//! template with no `ProductionUpdate`. This test pins the shipped data and records which objects that affects.
//! Skips when no install is found.

use zr_assets::{Installs, Vfs};
use zr_templates::{ModuleData, ProductionExitKind, Templates};

/// Every shipped `SpawnBehavior` object's exit provider. The port's routing covers each case except the spawn-point
/// exits of templates without a `ProductionUpdate`: those are ring-placed instead of leaving through their bone. The
/// list is campaign/demo/cinematic data (no skirmish stinger site is in it), and routing them through the real exit
/// would need the exit module's occupancy outside `ProductionState`.
#[test]
fn the_spawn_behavior_exit_modules_are_the_known_ones() {
    let Some(zh) = Installs::detect().zero_hour else { return };
    let Ok(vfs) = Vfs::zero_hour(&zh) else { return };
    let loaded = zr_ini::load_zero_hour(&vfs);
    let (t, _) = Templates::from_ini(&loaded.db);
    let mut spawners = 0;
    let mut supply_without_production = Vec::new();
    let mut spawn_point_without_production = Vec::new();
    let mut tunnels = 0;
    let mut no_exit = Vec::new();
    for thing in t.things() {
        let mut spawn = false;
        let mut production = false;
        let mut exit: Option<ProductionExitKind> = None;
        let mut contain_kind = None;
        for m in &thing.modules {
            match &m.data {
                ModuleData::Spawn(_) => spawn = true,
                ModuleData::Production(_) => production = true,
                ModuleData::ProductionExit(e) => exit = Some(e.kind),
                ModuleData::Contain(c) => contain_kind = Some(c.kind),
                _ => {}
            }
        }
        if !spawn {
            continue;
        }
        spawners += 1;
        match exit {
            Some(ProductionExitKind::SupplyCenter) => {
                if !production {
                    supply_without_production.push(thing.name.clone());
                }
            }
            Some(ProductionExitKind::SpawnPoint) => {
                if !production {
                    spawn_point_without_production.push(thing.name.clone());
                }
            }
            Some(ProductionExitKind::Queue) => {}
            Some(ProductionExitKind::Default) | None => {
                if matches!(contain_kind, Some(zr_templates::ContainKind::Tunnel)) {
                    tunnels += 1;
                } else {
                    no_exit.push(thing.name.clone());
                }
            }
        }
    }
    println!(
        "{spawners} SpawnBehavior objects: {tunnels} tunnel networks, {} spawn-point exits without a ProductionUpdate \
         ({spawn_point_without_production:?}), {} supply-center exits without one ({supply_without_production:?}), \
         no exit at all: {no_exit:?}",
        spawn_point_without_production.len(),
        supply_without_production.len()
    );
    assert!(spawners > 0, "the install has SpawnBehavior objects");
    assert!(tunnels > 0, "a tunnel network is a spawner with a contain exit");
    assert!(
        supply_without_production.is_empty(),
        "a supply center's spawner always has a ProductionUpdate, so the port's routing covers it"
    );
    // The campaign/demo/cinematic stinger sites (and the no-hole variant): a spawn point exit module and no
    // `ProductionUpdate`. Any new name here means the port's routing misses a new object.
    for name in &spawn_point_without_production {
        assert!(
            name.contains("GLAStingerSite"),
            "unexpected spawn-point spawner without a ProductionUpdate: {name}"
        );
    }
}
