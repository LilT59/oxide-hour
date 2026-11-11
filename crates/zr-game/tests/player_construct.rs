//! A human player's `Command::Construct` on real data (the Alpine skirmish start, as the viewer's
//! `ZR_CARD_DEMO=underconstruction` places it): the player's dozer walks to the site and the building
//! reaches 100%. Skips (passes) when no Zero Hour install is found.

use std::sync::Arc;
use zr_assets::{Installs, Vfs};
use zr_game::{Game, GameData, PlayerSetup};
use zr_sim::{Command, Coord3D, ObjectId};

fn vfs() -> Option<Vfs> {
    let zh = Installs::detect().zero_hour?;
    Vfs::zero_hour(&zh).ok()
}

/// Steps the skirmish until the order's building finishes, returning the trace when it stalls.
fn build_to_completion(template: &str, offset: (f32, f32)) -> Result<(), String> {
    let Some(vfs) = vfs() else { return Ok(()) };
    let data = Arc::new(GameData::load(&vfs));
    let setups = [PlayerSetup::new("FactionAmerica", 0), PlayerSetup::new("FactionChina", 1)];
    let mut g = Game::new_skirmish_with(data, &vfs, "Alpine Assault", &setups, 1).map_err(|e| format!("start: {e:#}"))?;
    let p0 = g.skirmish_player(0).expect("human").index;
    let start = g.spawned().iter().find(|s| s.player == p0 && s.role == "StartingBuilding").map(|s| s.pos).expect("command centre");
    let dozer: ObjectId = g.spawned().iter().find(|s| s.player == p0 && s.template.contains("Dozer")).map(|s| s.id).expect("dozer");
    let at = Coord3D::new(start.x + offset.0, start.y + offset.1, 0.0);
    let site = Coord3D::new(at.x, at.y, g.logic().terrain().ground_height(at.x, at.y));
    g.submit(p0, Command::Construct { dozer, template: template.into(), pos: site, angle: 0.0 });
    g.run_frames(1);
    let Some(building) = g.logic().objects().filter(|o| o.template.name == template && o.under_construction).map(|o| o.id).max() else {
        return Err(format!("{template}: the order created no building"));
    };
    let mut trace = Vec::new();
    for frame in 1..=1500u32 {
        g.run_frames(1);
        let Some(b) = g.logic().find_object(building) else { return Err("building vanished".into()) };
        if frame % 150 == 0 {
            let d = g.logic().find_object(dozer).expect("dozer");
            trace.push(format!(
                "f{frame}: {:.1}% dozer ({:.0},{:.0}) {:?} moving={} sub={:?}",
                b.construction_percent,
                d.position.x,
                d.position.y,
                d.ai.machine.current,
                d.ai.is_moving(),
                d.ai.dozer_action.sub
            ));
        }
        if !b.under_construction {
            return Ok(());
        }
    }
    Err(format!("{template} stalled: {}", trace.join(" | ")))
}

#[test]
fn player_ordered_power_plant_builds_to_completion() {
    if let Err(e) = build_to_completion("AmericaPowerPlant", (150.0, 40.0)) {
        panic!("{e}");
    }
}

/// The viewer's `ZR_CARD_DEMO=underconstruction` orders a Chinese power plant with the American dozer.
#[test]
fn american_dozer_builds_a_chinese_power_plant() {
    if let Err(e) = build_to_completion("ChinaPowerPlant", (150.0, 40.0)) {
        panic!("{e}");
    }
}
