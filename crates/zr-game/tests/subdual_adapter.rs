//! The `ActiveBody` subdual numbers reach the sim template (`adapter::convert_thing`), on synthetic INI data.

use zr_game::adapter::convert_thing;
use zr_ini::IniDb;
use zr_templates::Templates;

const INI: &str = "
Object SubdualTank
  KindOf = VEHICLE
  Body = ActiveBody ModuleTag_Body
    MaxHealth = 480.0
    InitialHealth = 400.0
    SubdualDamageCap = 1000
    SubdualDamageHealRate = 500
    SubdualDamageHealAmount = 50
  End
End
Object PlainTank
  KindOf = VEHICLE
  Body = ActiveBody ModuleTag_Body
    MaxHealth = 480.0
    InitialHealth = 400.0
  End
End
";

#[test]
fn the_subdual_numbers_reach_the_sim_template() {
    let blocks = zr_ini::parse("test.ini", INI).expect("synthetic INI parses");
    let mut db = IniDb::new();
    db.extend(blocks);
    let (all, _warnings) = Templates::from_ini(&db);
    let t = convert_thing(&all, all.thing("SubdualTank").expect("the template exists"));
    // `SubdualDamageHealRate` is 500 ms, which is 15 logic frames (rounded up, `duration_frames`).
    assert_eq!((t.subdual_damage_cap, t.subdual_heal_rate, t.subdual_heal_amount), (1000.0, 15, 50.0));
    // Without the fields the body cannot be subdued.
    let plain = convert_thing(&all, all.thing("PlainTank").expect("the template exists"));
    assert_eq!((plain.subdual_damage_cap, plain.subdual_heal_rate, plain.subdual_heal_amount), (0.0, 0, 0.0));
}
