//! Tunnel network demo on Alpine Assault: two GLA tunnel entrances share one passenger list, a unit
//! enters at one and leaves through the other, passengers heal inside, a non-last collapse keeps
//! them, and the last one caves in.
//!
//! `cargo run --release -p zr-game --example tunnel_demo` (needs a Zero Hour install, or `ZR_ZEROHOUR_DIR`).

use std::sync::Arc;

use zr_assets::{Installs, Vfs};
use zr_game::{Game, GameData, PlayerSetup};
use zr_sim::{Command, Coord3D};
use zr_templates::{ContainKind as TplContainKind, KindOf, ModuleData, ThingTemplate};

fn find(data: &GameData, pred: impl Fn(&ThingTemplate) -> bool) -> Option<String> {
    data.templates().things().iter().find(|t| pred(t)).map(|t| t.name.clone())
}

fn main() {
    let installs = Installs::detect();
    let Some(zh) = &installs.zero_hour else {
        eprintln!("no Zero Hour install found; set ZR_ZEROHOUR_DIR");
        return;
    };
    let vfs = Vfs::zero_hour(zh).expect("mounting Zero Hour");
    let data = Arc::new(GameData::load(&vfs));
    let setups = [PlayerSetup::new("FactionGLA", 0), PlayerSetup::new("FactionAmerica", 1)];
    let mut g = Game::new_skirmish_with(data.clone(), &vfs, "Alpine Assault", &setups, 1).expect("start skirmish");
    let p0 = g.skirmish_player(0).expect("player0").index;

    let Some(tunnel) = find(&data, |t| {
        t.modules.iter().any(|m| matches!(&m.data, ModuleData::Contain(c) if c.kind == TplContainKind::Tunnel))
    }) else {
        eprintln!("no tunnel network template found");
        return;
    };
    let Some(infantry) = find(&data, |t| t.is_kind_of(KindOf::Infantry) && !t.is_kind_of(KindOf::Hero) && t.transport_slot_count > 0) else {
        eprintln!("no infantry template found");
        return;
    };
    println!("tunnel {tunnel}, infantry {infantry}, max capacity {}", g.logic().rules().max_tunnel_capacity);

    let base = g
        .spawned()
        .iter()
        .find(|s| s.player == p0)
        .map(|s| s.pos)
        .expect("player 0 start position");
    let apos = Coord3D::new(base.x + 200.0, base.y + 200.0, base.z);
    let bpos = Coord3D::new(base.x + 600.0, base.y + 200.0, base.z);
    let Some(a) = g.spawn_object(&tunnel, apos, 0.0, p0) else { eprintln!("could not spawn tunnel A"); return };
    let Some(b) = g.spawn_object(&tunnel, bpos, 0.0, p0) else { eprintln!("could not spawn tunnel B"); return };

    let u1 = g.spawn_object(&infantry, Coord3D::new(apos.x + 20.0, apos.y, apos.z), 0.0, p0).unwrap();
    let u2 = g.spawn_object(&infantry, Coord3D::new(apos.x + 20.0, apos.y + 20.0, apos.z), 0.0, p0).unwrap();
    g.submit(p0, Command::Enter { ids: vec![u1, u2], container: a });
    let mut boarded = None;
    for _ in 0..300 {
        g.step();
        if g.logic().contained_units(a).len() == 2 {
            boarded = Some(g.frame());
            break;
        }
    }
    println!(
        "boarded {} passengers at frame {:?}; A sees {}, B sees {} (shared network list)",
        g.logic().contained_units(a).len(),
        boarded,
        g.logic().contained_units(a).len(),
        g.logic().contained_units(b).len()
    );

    // Heal: damage one passenger, then let the network heal it.
    let before = g.object(u1).unwrap().health;
    g.damage_object(u1, 50.0);
    let hurt = g.object(u1).unwrap().health;
    g.run_frames(60);
    println!("passenger u1 health {before:.0} -> {hurt:.0} (damaged) -> {:.0} (healed in the tunnel)", g.object(u1).unwrap().health);

    // Evacuate through the *other* entrance.
    g.submit(p0, Command::Evacuate { container: b });
    g.run_frames(5);
    if let Some(o) = g.object(u2) {
        println!(
            "evacuated via B: u2 at ({:.0}, {:.0}), {:.0} from B and {:.0} from A",
            o.position.x,
            o.position.y,
            o.position.dist_2d(&bpos),
            o.position.dist_2d(&apos)
        );
    }

    // Enter at B, destroy B (not the last) -> survivors repointed; destroy A (last) -> cave in.
    g.submit(p0, Command::Enter { ids: vec![u1], container: b });
    for _ in 0..300 {
        g.step();
        if g.logic().container_of(u1) == Some(b) {
            break;
        }
    }
    g.damage_object(b, 1.0e9);
    g.run_frames(3);
    println!("after B dies: u1 alive {}, repointed to A {}", g.object(u1).is_some(), g.logic().container_of(u1) == Some(a));
    g.damage_object(a, 1.0e9);
    g.run_frames(3);
    println!("after the last tunnel A dies: u1 alive {}", g.object(u1).is_some());
    println!("tunnel passengers left: {}", g.logic().tunnel_passengers(p0).len());
}
