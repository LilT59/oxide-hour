//! Production demo on Alpine Assault: America queues a Dozer from its starting Command Center,
//! checks the INI cost / build time, waits for it to appear and watches it walk to a rally point.
//!
//! `cargo run --release -p zr-game --example production_demo` (needs a Zero Hour install, or
//! `ZR_ZEROHOUR_DIR`).

use std::sync::Arc;
use zr_assets::{Installs, Vfs};
use zr_game::{Game, GameData, PlayerSetup};
use zr_sim::{Command, Coord3D, TemplateSource};

fn main() {
    let installs = Installs::detect();
    let Some(zh) = &installs.zero_hour else {
        eprintln!("no Zero Hour install found; set ZR_ZEROHOUR_DIR");
        return;
    };
    let vfs = Vfs::zero_hour(zh).expect("mounting Zero Hour");
    let data = Arc::new(GameData::load(&vfs));

    let setups = [PlayerSetup::new("FactionAmerica", 0), PlayerSetup::new("FactionChina", 1)];
    let mut g = Game::new_skirmish_with(data.clone(), &vfs, "Alpine Assault", &setups, 1).expect("start skirmish");

    let p0 = g.skirmish_player(0).expect("player0").index;
    let factory = g
        .spawned()
        .iter()
        .find(|s| s.player == p0 && s.role == "StartingBuilding")
        .expect("player 0 starting building")
        .id;
    let factory_name = g.object(factory).expect("building").template.to_owned();

    let dozer = data.store.find_template("AmericaVehicleDozer").expect("dozer template");
    let penalty = g.logic().rules().low_power_penalty(g.logic().energy_supply_ratio(p0));
    println!(
        "{factory_name} (id {}) builds AmericaVehicleDozer: cost {} (INI 1000), build time {} frames (INI 5.0 s = 150; x{:.2} with no power -> {})",
        factory.0,
        dozer.build_cost,
        dozer.build_time_frames,
        1.0 / penalty,
        (dozer.build_time_frames as f32 / penalty) as u32
    );

    // The starting set already contains a dozer; remember it so we can spot the produced one.
    let starting_dozers: Vec<u32> =
        g.objects().filter(|o| o.owner == p0 && o.template == "AmericaVehicleDozer").map(|o| o.id.0).collect();

    let before = g.player_money(p0).expect("money");
    g.submit(p0, Command::QueueUnit { factory, template: "AmericaVehicleDozer".into() });
    g.run_frames(1);
    let after = g.player_money(p0).expect("money");
    println!("money {before} -> {after} (withdrew {})", before - after);
    assert_eq!(before - after, 1000);

    // Walk to a rally point south of the base (the natural exit rally is only ~70 units away).
    let fpos = g.object(factory).expect("building").position;
    let rally = Coord3D::new(fpos.x, fpos.y - 250.0, 0.0);
    g.submit(p0, Command::SetRallyPoint { factory, pos: rally });

    let mut dozer_id = None;
    for _ in 0..500 {
        g.step();
        if dozer_id.is_none() {
            if let Some(o) = g
                .objects()
                .find(|o| o.owner == p0 && o.template == "AmericaVehicleDozer" && !starting_dozers.contains(&o.id.0))
            {
                dozer_id = Some(o.id);
                println!("dozer id {} appeared at frame {} at ({:.1}, {:.1})", o.id.0, g.frame(), o.position.x, o.position.y);
            }
        }
    }
    let id = dozer_id.expect("dozer built");
    let start = g.object(id).expect("dozer alive").position;
    for sec in 1..=4 {
        g.run_frames(30);
        let p = g.object(id).expect("dozer alive").position;
        println!("  t={sec}s ({:.1}, {:.1}) distance to rally {:.1}", p.x, p.y, p.dist_2d(&rally));
    }
    let end = g.object(id).expect("dozer alive").position;
    println!(
        "dozer moved from ({:.1}, {:.1}) to ({:.1}, {:.1}); distance to rally {:.1} (was {:.1})",
        start.x, start.y, end.x, end.y, end.dist_2d(&rally), start.dist_2d(&rally)
    );
    assert!(end.dist_2d(&rally) < start.dist_2d(&rally), "the dozer should walk toward the rally point");
    println!("crc {:08x}", g.crc());
}
