//! Lane 135: the adapter names every `GameLogic/Object/Contain/*` class as its own
//! [`SleepyModuleKind`], in the template's INI module order, and converts it to the matching sim
//! [`ContainKind`]. Each class derives from `OpenContain : UpdateModule` (`OpenContain.h:80`), so
//! `registerObject` pushes one heap slot per module (GameLogic.cpp:4013-4046).
//!
//! Synthetic INI only.

use zr_game::adapter::convert_thing;
use zr_ini::IniDb;
use zr_sim::contain::ContainKind;
use zr_sim::template::SleepyModuleKind;
use zr_templates::Templates;

fn templates(text: &str) -> Templates {
    let blocks = zr_ini::parse("test.ini", text).expect("synthetic INI parses");
    let mut db = IniDb::new();
    db.extend(blocks);
    Templates::from_ini(&db).0
}

fn convert(text: &str, name: &str) -> zr_sim::ObjectTemplate {
    let all = templates(text);
    convert_thing(&all, all.thing(name).expect("the template exists"))
}

/// Every contain class in INI order maps to its own `SleepyModuleKind` and sim `ContainKind`.
/// Revert check: the old catch-all arm mapped everything but Garrison/Heal to `OpenContain` (and
/// `Heal` to `Open`), so the expected vectors differ.
#[test]
fn every_contain_class_keeps_its_ini_position_and_class() {
    use SleepyModuleKind as K;
    let classes = [
        ("OpenContain", K::OpenContain, ContainKind::Open),
        ("TransportContain", K::TransportContain, ContainKind::Transport),
        ("GarrisonContain", K::GarrisonContain, ContainKind::Garrison),
        ("TunnelContain", K::TunnelContain, ContainKind::Tunnel),
        ("HealContain", K::HealContain, ContainKind::Heal),
        ("OverlordContain", K::OverlordContain, ContainKind::Overlord),
        ("HelixContain", K::HelixContain, ContainKind::Helix),
        ("ParachuteContain", K::ParachuteContain, ContainKind::Parachute),
        ("InternetHackContain", K::InternetHackContain, ContainKind::InternetHack),
        ("RailedTransportContain", K::RailedTransportContain, ContainKind::RailedTransport),
        ("RiderChangeContain", K::RiderChangeContain, ContainKind::RiderChange),
    ];
    for (i, (class, sleepy, sim)) in classes.iter().enumerate() {
        let ini = format!(
            "
Object Cont{i}
  KindOf = VEHICLE
  Behavior = {class} ModuleTag_01
    Slots = 4
    ContainMax = 4
  End
End
"
        );
        let t = convert(&ini, &format!("Cont{i}"));
        assert_eq!(t.update_module_order, vec![*sleepy], "{class} is its own heap class");
        let c = t.contain.as_ref().unwrap_or_else(|| panic!("{class} converts to a ContainTemplate"));
        assert_eq!(c.kind, *sim, "{class} maps to {sim:?}");
        assert_eq!(c.kind.class_name(), *class, "the heap dump prints the C++ class name");
    }
}

/// A template with several contain blocks keeps their INI order in `update_module_order` (the sim
/// keeps the first in `contain`, as `Object::getContain()` finds the first module that answers).
#[test]
fn several_contain_modules_keep_their_ini_order() {
    use SleepyModuleKind as K;
    let ini = "
Object MultiContain
  KindOf = VEHICLE
  Behavior = TransportContain ModuleTag_01
    Slots = 2
  End
  Behavior = HealContain ModuleTag_02
    ContainMax = 1
  End
  Behavior = TunnelContain ModuleTag_03
  End
  Behavior = AIUpdateInterface ModuleTag_04
  End
End
";
    let t = convert(ini, "MultiContain");
    assert_eq!(
        t.update_module_order,
        vec![K::TransportContain, K::HealContain, K::TunnelContain, K::Ai]
    );
    assert_eq!(t.contain.as_ref().map(|c| c.kind), Some(ContainKind::Transport));
}
