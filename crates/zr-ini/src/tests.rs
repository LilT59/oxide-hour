use super::*;

fn one(text: &str) -> Block {
    let mut v = parse("t.ini", text).unwrap();
    assert_eq!(v.len(), 1, "{v:?}");
    v.remove(0)
}

fn kv(b: &Block) -> Vec<(&str, &str)> {
    b.fields()
        .map(|f| (f.key.as_str(), f.value.as_str()))
        .collect()
}

#[test]
fn fields_keep_order_and_duplicates() {
    let b = one("Armor TankArmor\n  Armor = DEFAULT 100%\n  Armor = CRUSH 50%\n  Armor = DEFAULT 80%\nEnd\n");
    assert_eq!((b.kind.as_str(), b.name()), ("Armor", Some("TankArmor")));
    assert_eq!(
        kv(&b),
        [
            ("Armor", "DEFAULT 100%"),
            ("Armor", "CRUSH 50%"),
            ("Armor", "DEFAULT 80%")
        ]
    );
    assert_eq!(b.fields_named("Armor").count(), 3);
    assert_eq!(
        b.value("Armor"),
        Some("DEFAULT 80%"),
        "last duplicate wins for scalars"
    );
}

#[test]
fn separator_spacing() {
    let b = one("Weapon W\n A=1\n B = 2\n C   3\n D= 4\n E =5\n F\n G ==  7\nEnd");
    assert_eq!(
        kv(&b),
        [
            ("A", "1"),
            ("B", "2"),
            ("C", "3"),
            ("D", "4"),
            ("E", "5"),
            ("F", ""),
            ("G", "7")
        ]
    );
    assert_eq!(b.field("B").unwrap().tokens(), ["2"]);
}

#[test]
fn value_tokens_and_inner_equals() {
    let b = one("Weapon W\n  DamageType = a  b\tc=d  R:1 G:2\nEnd");
    assert_eq!(
        b.field("DamageType").unwrap().tokens(),
        ["a", "b", "c", "d", "R:1", "G:2"]
    );
}

#[test]
fn comments() {
    let b = one(
        "; header\n// slash header\nWeapon W ; trailing\n  A = 1 ; comment\n  // whole line\n  B = 2 // trailing slash\n  Url = http://x/y\n;End\nEnd\n",
    );
    assert_eq!(kv(&b), [("A", "1"), ("B", "2"), ("Url", "http://x/y")]);
}

#[test]
fn end_is_case_insensitive_and_crlf_tolerant() {
    let v = parse(
        "t.ini",
        "Weapon A\r\n X = 1\r\nEND\r\nWeapon B\r\n  Y = 2\r\n end \r\nWeapon C\r\nEnd\r\n",
    )
    .unwrap();
    assert_eq!(v.len(), 3);
    assert_eq!(kv(&v[0]), [("X", "1")]);
    assert_eq!(kv(&v[1]), [("Y", "2")]);
    assert!(v[2].items.is_empty());
}

#[test]
fn tabs_are_whitespace() {
    let b = one("Weapon W\n\tA\t=\t1\nEnd");
    assert_eq!(kv(&b), [("A", "1")]);
}

#[test]
fn object_modules_nest() {
    let src = "\
Object AmericaTankCrusader
  DisplayName = OBJECT:Crusader
  Draw = W3DTankDraw ModuleTag_01
    DefaultConditionState
      Model = AVLeopard
      Turret = Turret
    End
    ConditionState = REALLYDAMAGED
      Model = AVLeopard_D
    End
    AliasConditionState = RUBBLE
    TransitionState = A B
      Animation = X
    End
    TrackMarks = EXTnkTrack.tga
  End
  Body = ActiveBody ModuleTag_02
    MaxHealth = 480
  End
  Behavior = AIUpdateInterface ModuleTag_03
    Turret
      TurretTurnRate = 180
    End
    AltTurret
      TurretTurnRate = 90
    End
    AutoAcquireEnemiesWhenIdle = Yes
  End
  Locomotor = SET_NORMAL CrusaderLocomotor
  WeaponSet
    Conditions = None
    Weapon = PRIMARY CrusaderTankGun
  End
  ArmorSet
    Armor = TankArmor
  End
  Prerequisites
    Object = AmericaWarFactory
  End
End";
    let o = one(src);
    assert_eq!(o.name(), Some("AmericaTankCrusader"));
    assert_eq!(o.children().count(), 6);
    let draw = o.child("Draw").unwrap();
    assert_eq!(draw.header, ["W3DTankDraw", "ModuleTag_01"]);
    let states: Vec<_> = draw
        .children()
        .map(|c| (c.kind.as_str(), c.header.join(" ")))
        .collect();
    assert_eq!(
        states,
        [
            ("DefaultConditionState", ""),
            ("ConditionState", "REALLYDAMAGED"),
            ("TransitionState", "A B")
        ]
        .map(|(k, h)| (k, h.to_owned()))
    );
    // `Turret = bone` inside a condition state is a field, not a block.
    assert_eq!(
        draw.child("DefaultConditionState").unwrap().value("Turret"),
        Some("Turret")
    );
    assert_eq!(
        draw.fields().map(|f| f.key.as_str()).collect::<Vec<_>>(),
        ["AliasConditionState", "TrackMarks"]
    );
    // ...but a `Turret` line inside an AI update is a block.
    let ai = o.children_named("Behavior").next().unwrap();
    assert!(ai.child("Turret").is_some() && ai.child("AltTurret").is_some());
    assert_eq!(ai.value("AutoAcquireEnemiesWhenIdle"), Some("Yes"));
    // `Locomotor = ...` and `Armor = ...` are fields inside an Object.
    assert_eq!(o.value("Locomotor"), Some("SET_NORMAL CrusaderLocomotor"));
    assert_eq!(
        o.child("ArmorSet").unwrap().value("Armor"),
        Some("TankArmor")
    );
    assert_eq!(o.descendant_count(), 6 + 3 + 2);
}

#[test]
fn reskin_and_module_editing() {
    let src = "\
ObjectReskin AmericaTankCrusaderSkin AmericaTankCrusader
  Draw = W3DModelDraw ModuleTag_01
    DefaultConditionState
      Model = X
    End
  End
  RemoveModule = ModuleTag_09
  AddModule
    Behavior = FooBehavior ModuleTag_20
      A = 1
    End
  End
  ReplaceModule ModuleTag_03
    Behavior = BarBehavior ModuleTag_03
    End
  End
End
Object Default
  InheritableModule
    Behavior = AutoHealBehavior ModuleTag_Heal
      HealingAmount = 10
    End
  End
  OverrideableByLikeKind
    Behavior = StealthUpdate ModuleTag_Stealth
    End
  End
  Scale = 1.0
End";
    let v = parse("t.ini", src).unwrap();
    assert_eq!(v[0].kind, "ObjectReskin");
    assert_eq!(
        v[0].header,
        ["AmericaTankCrusaderSkin", "AmericaTankCrusader"]
    );
    assert_eq!(v[0].value("RemoveModule"), Some("ModuleTag_09"));
    let add = v[0].child("AddModule").unwrap();
    assert_eq!(add.child("Behavior").unwrap().value("A"), Some("1"));
    assert_eq!(
        v[0].child("ReplaceModule").unwrap().header,
        ["ModuleTag_03"]
    );
    assert_eq!(
        v[1].child("InheritableModule")
            .unwrap()
            .child("Behavior")
            .unwrap()
            .header[0],
        "AutoHealBehavior"
    );
    assert_eq!(
        v[1].child("OverrideableByLikeKind")
            .unwrap()
            .children()
            .count(),
        1
    );
    assert_eq!(v[1].value("Scale"), Some("1.0"));
}

#[test]
fn context_decides_field_vs_block() {
    // Same keys, different parents: Weapon/Armor/Object are block types at top
    // level but plain fields elsewhere.
    let src = "\
Weapon W
  Nuggets = x
End
Armor A
  Armor = DEFAULT 1%
End
Object O
  Prerequisites
    Object = Other
    Science = SCIENCE_X
  End
End";
    let v = parse("t.ini", src).unwrap();
    assert_eq!(
        v.iter().map(|b| b.kind.as_str()).collect::<Vec<_>>(),
        ["Weapon", "Armor", "Object"]
    );
    assert_eq!(v[2].child("Prerequisites").unwrap().fields().count(), 2);
}

#[test]
fn fxlist_and_ocl_nuggets() {
    let src = "\
FXList FX_Boom
  Sound
    Name = Boom
  End
  ParticleSystem
    Name = PS
    Offset = X:0 Y:0 Z:1
  End
End
ObjectCreationList OCL_Test
  CreateObject
    ObjectNames = Foo
  End
  DeliverPayload
    Transport = T
    DeliveryDecal
      Texture = SCCDecal
    End
  End
End";
    let v = parse("t.ini", src).unwrap();
    assert_eq!(
        v[0].children().map(|c| c.kind.as_str()).collect::<Vec<_>>(),
        ["Sound", "ParticleSystem"]
    );
    let dp = v[1].child("DeliverPayload").unwrap();
    assert_eq!(dp.value("Transport"), Some("T"));
    assert_eq!(
        dp.child("DeliveryDecal").unwrap().value("Texture"),
        Some("SCCDecal")
    );
}

#[test]
fn single_line_blocks_have_no_end() {
    let v = parse(
        "t.ini",
        "ReallyLowMHz = 400\nBenchProfile = P4 2189 6.1 15.1 9.4\nLODPreset = MEDIUM K7 1500 GF4 256\nWeapon W\nEnd\n",
    )
    .unwrap();
    assert_eq!(v.len(), 4);
    assert_eq!(v[0].header, ["400"]);
    assert_eq!(v[1].header, ["P4", "2189", "6.1", "15.1", "9.4"]);
    assert_eq!(v[2].name(), Some("MEDIUM"));
    assert!(v[3].items.is_empty());
}

#[test]
fn unnamed_blocks() {
    let b = one("GameData\n  A = 1\nEnd");
    assert!(b.header.is_empty() && b.name().is_none());
}

#[test]
fn define_macros() {
    let b = one("#define COST 900\n#define W CrusaderTankGun\nObject X\n  BuildCost = COST\n  Weapon = PRIMARY W\n  Other = COSTLY\nEnd");
    assert_eq!(
        kv(&b),
        [
            ("BuildCost", "900"),
            ("Weapon", "PRIMARY CrusaderTankGun"),
            ("Other", "COSTLY")
        ]
    );
}

#[test]
fn include_via_loader() {
    let mut seen = Vec::new();
    let mut loader = |p: &str| {
        seen.push(p.to_owned());
        (p == "Data\\INI\\Part.ini").then(|| "#define N 5\nWeapon Inc\n  V = N\nEnd\n".to_owned())
    };
    let v = parse_with(
        "main.ini",
        "#include \"Data\\INI\\Part.ini\"\nWeapon After\n  V = N\nEnd\n",
        &mut loader,
    )
    .unwrap();
    assert_eq!(seen, ["Data\\INI\\Part.ini"]);
    assert_eq!(v[0].value("V"), Some("5"));
    assert_eq!(
        v[1].value("V"),
        Some("5"),
        "defines persist after the include"
    );
}

#[test]
fn errors_report_file_and_line() {
    let e = parse("a.ini", "Weapon W\nEnd\nBogus X\nEnd\n").unwrap_err();
    assert_eq!((e.file.as_str(), e.line), ("a.ini", 3));
    assert_eq!(e.kind, ErrorKind::UnknownBlock("Bogus".into()));

    let e = parse("a.ini", "Weapon W\n  A = 1\n").unwrap_err();
    assert_eq!(
        e.kind,
        ErrorKind::MissingEnd {
            kind: "Weapon".into(),
            opened_at: 1
        }
    );

    // Block types are case-sensitive, like `findBlockParse`'s strcmp.
    assert!(parse("a.ini", "weapon W\nEnd\n").is_err());

    let e = parse("a.ini", "#include \"nope.ini\"\n").unwrap_err();
    assert!(matches!(e.kind, ErrorKind::Include(_)));
    assert_eq!(
        e.to_string(),
        "a.ini:1: include failed: cannot load `nope.ini`"
    );

    // Include errors in a nested file name that file.
    let mut l = |_: &str| Some("Bogus\n".to_owned());
    let e = parse_with("main.ini", "#include \"x.ini\"\n", &mut l).unwrap_err();
    assert_eq!((e.file.as_str(), e.line), ("x.ini", 1));
}

#[test]
fn include_cycle_is_rejected() {
    let mut l = |_: &str| Some("#include \"self.ini\"\n".to_owned());
    assert!(parse_with("self.ini", "#include \"self.ini\"\n", &mut l).is_err());
}

#[test]
fn display_round_trips() {
    let src = "Object O\n  A = 1 2\n  Draw = W3DModelDraw Tag\n    DefaultConditionState\n      Model = M\n    End\n  End\n  B\nEnd\n";
    let b = one(src);
    let printed = b.to_string();
    assert_eq!(printed, src);
    assert_eq!(one(&printed), b);
}

#[test]
fn block_type_table() {
    for t in [
        "Object",
        "ObjectReskin",
        "Weapon",
        "Locomotor",
        "Armor",
        "CommandButton",
        "Upgrade",
        "FXList",
        "GameData",
    ] {
        assert!(blocks::is_block_type(t), "{t}");
    }
    assert_eq!(blocks::BLOCK_TYPES.len(), 62);
    assert!(!blocks::is_block_type("Behavior"));
}

fn db_of(srcs: &[&str]) -> IniDb {
    let mut db = IniDb::new();
    for s in srcs {
        db.extend(parse("t.ini", s).unwrap());
    }
    db
}

#[test]
fn db_last_definition_wins_and_overlays() {
    let db = db_of(&[
        "Weapon Gun\n  Damage = 10\n  Range = 100\nEnd\nLocomotor L\n  Speed = 1\nEnd",
        "Weapon Gun\n  Damage = 25\nEnd\nWeapon Other\nEnd",
    ]);
    assert_eq!(db.len(), 4);
    assert_eq!(db.get("Weapon", "Gun").unwrap().value("Damage"), Some("25"));
    assert_eq!(db.get("Weapon", "Gun").unwrap().value("Range"), None);
    assert_eq!(db.definitions("Weapon", "Gun").len(), 2);
    let merged = db.resolved("Weapon", "Gun").unwrap();
    assert_eq!(
        (merged.value("Damage"), merged.value("Range")),
        (Some("25"), Some("100"))
    );
    assert!(
        db.get("Weapon", "gun").is_none(),
        "names are case-sensitive"
    );
    assert!(db.get("Armor", "Gun").is_none());
    assert_eq!(db.count_unique("Weapon"), 2);
    let names: Vec<_> = db.of_kind("Weapon").map(|b| b.name().unwrap()).collect();
    assert_eq!(names, ["Gun", "Other"]);
    assert_eq!(
        db.of_kind("Weapon").next().unwrap().value("Damage"),
        Some("25")
    );
    assert_eq!(db.counts(), [("Locomotor".into(), 1), ("Weapon".into(), 3)]);
}

#[test]
fn db_object_and_reskin_share_a_namespace() {
    let db = db_of(&[
        "Object A\n  X = 1\nEnd\nObjectReskin B A\n  X = 2\nEnd",
        "GameData\n  G = 1\nEnd\nGameData\n  G = 2\nEnd",
    ]);
    assert_eq!(db.get("Object", "B").unwrap().kind, "ObjectReskin");
    assert_eq!(db.get("ObjectReskin", "A").unwrap().kind, "Object");
    assert_eq!(db.get("GameData", "").unwrap().value("G"), Some("2"));
    assert_eq!(db.resolved("GameData", "").unwrap().fields().count(), 2);
}
