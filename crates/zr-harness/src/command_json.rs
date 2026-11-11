//! Validated JSON -> [`Command`] decoding for the control protocol (`docs/HARNESS.md`).
//!
//! The wire form is one object per command: `{"type": "<name>", ...fields}`. Type names are the
//! [`Command`] variant names in `snake_case`; the matcher ignores case and `_`/`-`, so `"MoveTo"`,
//! `"move_to"` and `"moveto"` are the same command. Every field is checked here (types, ranges,
//! finite floats, non-empty names, a bounded id list) so bad input is an error reply, never a panic
//! in the sim. Unknown fields are ignored (forward compatible); unknown types are an error.
//!
//! Positions are `[x, y, z]` or `{"x":..,"y":..,"z":..}`. Object ids are unsigned 32-bit integers.
//! `target` (an object id) and `pos` (a position) are alternatives where a command takes either.

use serde_json::{Map, Value};

use zr_sim::{CombatDropTarget, Command, Coord3D, GuardMode, GuardTarget, ObjectId, SpecialPowerTarget};

/// Longest accepted `ids` list (a group order; the engine's selection is far smaller).
pub const MAX_IDS: usize = 4096;
/// Longest accepted template / upgrade / science string, in bytes.
pub const MAX_NAME_LEN: usize = 256;
/// Largest accepted group-order slot / queue index.
const MAX_INDEX: u32 = 1_000_000;

/// Decodes one command object. Errors name the command and field at fault.
pub fn decode(v: &Value) -> Result<Command, String> {
    let o = v.as_object().ok_or_else(|| "command: want a JSON object".to_owned())?;
    let ty = o
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| "command: missing string field \"type\"".to_owned())?;
    let key: String = ty.chars().filter(|c| !matches!(c, '_' | '-' | ' ')).flat_map(char::to_lowercase).collect();
    let cmd = match key.as_str() {
        "moveto" => Command::MoveTo { ids: ids(o, "ids")?, pos: pos(o, "pos")? },
        "attack" => Command::Attack { ids: ids(o, "ids")?, target: id(o, "target")? },
        "attackground" => Command::AttackGround { ids: ids(o, "ids")?, pos: pos(o, "pos")? },
        "forceattack" => Command::ForceAttack { ids: ids(o, "ids")?, target: id(o, "target")? },
        "stop" => Command::Stop { ids: ids(o, "ids")? },
        "attackmove" => Command::AttackMove { ids: ids(o, "ids")?, pos: pos(o, "pos")? },
        "guard" => Command::Guard { ids: ids(o, "ids")?, target: guard_target(o)?, mode: guard_mode(o)? },
        "sell" => Command::Sell { building: id(o, "building")? },
        "enter" => Command::Enter { ids: ids(o, "ids")?, container: id(o, "container")? },
        "exit" => Command::Exit { container: id(o, "container")?, ids: ids(o, "ids")? },
        "evacuate" => Command::Evacuate { container: id(o, "container")? },
        "queueunit" => Command::QueueUnit { factory: id(o, "factory")?, template: name(o, "template")? },
        "queueupgrade" => Command::QueueUpgrade { building: id(o, "building")?, upgrade: name(o, "upgrade")? },
        "cancelunit" => Command::CancelUnit { factory: id(o, "factory")?, index: index(o, "index")? },
        "setrallypoint" => Command::SetRallyPoint { factory: id(o, "factory")?, pos: pos(o, "pos")? },
        "construct" => Command::Construct {
            dozer: id(o, "dozer")?,
            template: name(o, "template")?,
            pos: pos(o, "pos")?,
            angle: opt_f32(o, "angle")?.unwrap_or(0.0),
        },
        "cancelconstruct" => Command::CancelConstruct { dozer: id(o, "dozer")? },
        "repair" => Command::Repair { dozer: id(o, "dozer")?, target: id(o, "target")? },
        "resumeconstruction" => Command::ResumeConstruction { dozer: id(o, "dozer")?, target: id(o, "target")? },
        "dock" => Command::Dock { ids: ids(o, "ids")?, target: id(o, "target")? },
        "getrepaired" => Command::GetRepaired { ids: ids(o, "ids")?, target: id(o, "target")? },
        "gethealed" => Command::GetHealed { ids: ids(o, "ids")?, target: id(o, "target")? },
        "forcemove" => Command::ForceMove { ids: ids(o, "ids")?, pos: pos(o, "pos")? },
        "addwaypoint" => Command::AddWaypoint { ids: ids(o, "ids")?, pos: pos(o, "pos")? },
        "scatter" => Command::Scatter { ids: ids(o, "ids")? },
        "toggleovercharge" => Command::ToggleOvercharge { ids: ids(o, "ids")? },
        "switchweapons" => Command::SwitchWeapons { ids: ids(o, "ids")?, slot: index(o, "slot")? },
        "cancelupgrade" => Command::CancelUpgrade { building: id(o, "building")?, upgrade: name(o, "upgrade")? },
        "combatdrop" => Command::CombatDrop { ids: ids(o, "ids")?, target: drop_target(o)? },
        "doweapon" => Command::DoWeapon { ids: ids(o, "ids")?, slot: index(o, "slot")?, max_shots: i32_field(o, "max_shots")? },
        "doweaponatobject" => Command::DoWeaponAtObject {
            ids: ids(o, "ids")?,
            slot: index(o, "slot")?,
            target: id(o, "target")?,
            max_shots: i32_field(o, "max_shots")?,
        },
        "doweaponatlocation" => Command::DoWeaponAtLocation {
            ids: ids(o, "ids")?,
            slot: index(o, "slot")?,
            pos: pos(o, "pos")?,
            max_shots: i32_field(o, "max_shots")?,
        },
        "hackinternet" => Command::HackInternet { ids: ids(o, "ids")? },
        "setmineclearingdetail" => Command::SetMineClearingDetail { ids: ids(o, "ids")? },
        "dospecialpower" => Command::DoSpecialPower { source: id(o, "source")?, power: name(o, "power")?, target: power_target(o)? },
        "purchasescience" => Command::PurchaseScience { science: name(o, "science")? },
        "dospecialpoweratlocation" => Command::DoSpecialPowerAtLocation {
            source: id(o, "source")?,
            power: name(o, "power")?,
            pos: pos(o, "pos")?,
            angle: opt_f32(o, "angle")?,
        },
        _ => {
            return Err(format!(
                "command: unknown type \"{ty}\" (want one of: MoveTo, Attack, AttackGround, ForceAttack, Stop, AttackMove, Guard, \
                 Sell, Enter, Exit, Evacuate, QueueUnit, QueueUpgrade, CancelUnit, SetRallyPoint, Construct, CancelConstruct, \
                 Repair, ResumeConstruction, Dock, GetRepaired, GetHealed, ForceMove, AddWaypoint, Scatter, ToggleOvercharge, \
                 SwitchWeapons, CancelUpgrade, CombatDrop, DoWeapon, DoWeaponAtObject, DoWeaponAtLocation, HackInternet, \
                 SetMineClearingDetail, DoSpecialPower, PurchaseScience, DoSpecialPowerAtLocation)"
            ))
        }
    };
    Ok(cmd)
}

fn field<'a>(o: &'a Map<String, Value>, key: &str) -> Result<&'a Value, String> {
    o.get(key).ok_or_else(|| format!("command: missing field \"{key}\""))
}

fn ids(o: &Map<String, Value>, key: &str) -> Result<Vec<ObjectId>, String> {
    let v = field(o, key)?;
    let arr = v.as_array().ok_or_else(|| format!("command: \"{key}\" must be an array of object ids"))?;
    if arr.len() > MAX_IDS {
        return Err(format!("command: \"{key}\" has {} entries; at most {MAX_IDS}", arr.len()));
    }
    arr.iter()
        .map(|v| {
            v.as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .map(ObjectId)
                .ok_or_else(|| format!("command: \"{key}\" must contain unsigned 32-bit integers"))
        })
        .collect()
}

fn id(o: &Map<String, Value>, key: &str) -> Result<ObjectId, String> {
    let v = field(o, key)?;
    v.as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .map(ObjectId)
        .ok_or_else(|| format!("command: \"{key}\" must be an unsigned 32-bit object id"))
}

fn index(o: &Map<String, Value>, key: &str) -> Result<usize, String> {
    let v = field(o, key)?;
    let n = v
        .as_u64()
        .filter(|n| *n <= u64::from(MAX_INDEX))
        .ok_or_else(|| format!("command: \"{key}\" must be an integer in 0..={MAX_INDEX}"))?;
    Ok(n as usize)
}

fn i32_field(o: &Map<String, Value>, key: &str) -> Result<i32, String> {
    let v = field(o, key)?;
    v.as_i64()
        .and_then(|n| i32::try_from(n).ok())
        .ok_or_else(|| format!("command: \"{key}\" must be a 32-bit integer"))
}

fn f32_of(v: &Value, what: &str) -> Result<f32, String> {
    let n = v.as_f64().ok_or_else(|| format!("command: {what} must be a number"))?;
    let f = n as f32;
    if !f.is_finite() {
        return Err(format!("command: {what} is out of range for a 32-bit float"));
    }
    Ok(f)
}

fn opt_f32(o: &Map<String, Value>, key: &str) -> Result<Option<f32>, String> {
    match o.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => f32_of(v, &format!("\"{key}\"")).map(Some),
    }
}

fn name(o: &Map<String, Value>, key: &str) -> Result<String, String> {
    let s = field(o, key)?.as_str().ok_or_else(|| format!("command: \"{key}\" must be a string"))?;
    if s.is_empty() {
        return Err(format!("command: \"{key}\" must not be empty"));
    }
    if s.len() > MAX_NAME_LEN {
        return Err(format!("command: \"{key}\" is {} bytes; at most {MAX_NAME_LEN}", s.len()));
    }
    Ok(s.to_owned())
}

fn pos_val(v: &Value) -> Result<Coord3D, String> {
    if let Some(a) = v.as_array() {
        if a.len() != 3 {
            return Err("command: a position must have exactly 3 numbers ([x, y, z])".to_owned());
        }
        return Ok(Coord3D::new(f32_of(&a[0], "position x")?, f32_of(&a[1], "position y")?, f32_of(&a[2], "position z")?));
    }
    if let Some(o) = v.as_object() {
        let c = |k: &str| o.get(k).ok_or_else(|| format!("command: position is missing \"{k}\"")).and_then(|v| f32_of(v, &format!("position {k}")));
        return Ok(Coord3D::new(c("x")?, c("y")?, c("z")?));
    }
    Err("command: a position must be [x, y, z] or {\"x\":..,\"y\":..,\"z\":..}".to_owned())
}

fn pos(o: &Map<String, Value>, key: &str) -> Result<Coord3D, String> {
    pos_val(field(o, key)?)
}

fn guard_target(o: &Map<String, Value>) -> Result<GuardTarget, String> {
    match (o.get("target"), o.get("pos")) {
        (Some(t), None) => t
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .map(|n| GuardTarget::Object(ObjectId(n)))
            .ok_or_else(|| "command: Guard \"target\" must be an unsigned 32-bit object id".to_owned()),
        (None, Some(p)) => Ok(GuardTarget::Point(pos_val(p)?)),
        _ => Err("command: Guard needs exactly one of \"target\" (an object id) and \"pos\" (a position)".to_owned()),
    }
}

fn guard_mode(o: &Map<String, Value>) -> Result<GuardMode, String> {
    let Some(v) = o.get("mode") else { return Ok(GuardMode::Normal) };
    let s = v.as_str().ok_or_else(|| "command: Guard \"mode\" must be a string".to_owned())?;
    let key: String = s.chars().filter(|c| !matches!(c, '_' | '-' | ' ')).flat_map(char::to_lowercase).collect();
    match key.as_str() {
        "normal" => Ok(GuardMode::Normal),
        "withoutpursuit" => Ok(GuardMode::WithoutPursuit),
        "flyingunitsonly" => Ok(GuardMode::FlyingUnitsOnly),
        _ => Err(format!("command: Guard \"mode\" {s:?} is not normal, without_pursuit or flying_units_only")),
    }
}

fn drop_target(o: &Map<String, Value>) -> Result<CombatDropTarget, String> {
    match (o.get("target"), o.get("pos")) {
        (Some(t), None) => t
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .map(|n| CombatDropTarget::Object(ObjectId(n)))
            .ok_or_else(|| "command: CombatDrop \"target\" must be an unsigned 32-bit object id".to_owned()),
        (None, Some(p)) => Ok(CombatDropTarget::Position(pos_val(p)?)),
        _ => Err("command: CombatDrop needs exactly one of \"target\" (an object id) and \"pos\" (a position)".to_owned()),
    }
}

fn power_target(o: &Map<String, Value>) -> Result<SpecialPowerTarget, String> {
    match (o.get("target"), o.get("pos")) {
        (None | Some(Value::Null), None) => Ok(SpecialPowerTarget::None),
        (Some(t), None) => t
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .map(|n| SpecialPowerTarget::Object(ObjectId(n)))
            .ok_or_else(|| "command: DoSpecialPower \"target\" must be an unsigned 32-bit object id".to_owned()),
        (None | Some(Value::Null), Some(p)) => Ok(SpecialPowerTarget::Position(pos_val(p)?)),
        (Some(_), Some(_)) => Err("command: DoSpecialPower takes \"target\" or \"pos\", not both".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn d(v: Value) -> Result<Command, String> {
        decode(&v)
    }

    fn c(v: Value) -> Command {
        d(v).expect("decodes")
    }

    #[test]
    fn every_command_type_decodes() {
        let ids = json!([3, 7]);
        let p = json!([1.5, -2.0, 3.25]);
        let cp = Coord3D::new(1.5, -2.0, 3.25);
        let two = vec![ObjectId(3), ObjectId(7)];

        assert_eq!(c(json!({"type":"move_to","ids":ids,"pos":p})), Command::MoveTo { ids: two.clone(), pos: cp });
        assert_eq!(c(json!({"type":"attack","ids":ids,"target":9})), Command::Attack { ids: two.clone(), target: ObjectId(9) });
        assert_eq!(c(json!({"type":"attack_ground","ids":ids,"pos":p})), Command::AttackGround { ids: two.clone(), pos: cp });
        assert_eq!(c(json!({"type":"force_attack","ids":ids,"target":9})), Command::ForceAttack { ids: two.clone(), target: ObjectId(9) });
        assert_eq!(c(json!({"type":"stop","ids":ids})), Command::Stop { ids: two.clone() });
        assert_eq!(c(json!({"type":"attack_move","ids":ids,"pos":p})), Command::AttackMove { ids: two.clone(), pos: cp });
        assert_eq!(
            c(json!({"type":"guard","ids":ids,"pos":p,"mode":"without_pursuit"})),
            Command::Guard { ids: two.clone(), target: GuardTarget::Point(cp), mode: GuardMode::WithoutPursuit }
        );
        assert_eq!(
            c(json!({"type":"guard","ids":ids,"target":4})),
            Command::Guard { ids: two.clone(), target: GuardTarget::Object(ObjectId(4)), mode: GuardMode::Normal }
        );
        assert_eq!(
            c(json!({"type":"guard","ids":ids,"pos":p,"mode":"flying-units-only"})),
            Command::Guard { ids: two.clone(), target: GuardTarget::Point(cp), mode: GuardMode::FlyingUnitsOnly }
        );
        assert_eq!(c(json!({"type":"sell","building":5})), Command::Sell { building: ObjectId(5) });
        assert_eq!(c(json!({"type":"enter","ids":ids,"container":5})), Command::Enter { ids: two.clone(), container: ObjectId(5) });
        assert_eq!(c(json!({"type":"exit","container":5,"ids":ids})), Command::Exit { container: ObjectId(5), ids: two.clone() });
        assert_eq!(c(json!({"type":"evacuate","container":5})), Command::Evacuate { container: ObjectId(5) });
        assert_eq!(
            c(json!({"type":"queue_unit","factory":5,"template":"AmericaTankCrusader"})),
            Command::QueueUnit { factory: ObjectId(5), template: "AmericaTankCrusader".into() }
        );
        assert_eq!(
            c(json!({"type":"queue_upgrade","building":5,"upgrade":"UpgradeAmerica"})),
            Command::QueueUpgrade { building: ObjectId(5), upgrade: "UpgradeAmerica".into() }
        );
        assert_eq!(c(json!({"type":"cancel_unit","factory":5,"index":2})), Command::CancelUnit { factory: ObjectId(5), index: 2 });
        assert_eq!(
            c(json!({"type":"set_rally_point","factory":5,"pos":p})),
            Command::SetRallyPoint { factory: ObjectId(5), pos: cp }
        );
        assert_eq!(
            c(json!({"type":"construct","dozer":5,"template":"AmericaPowerPlant","pos":p,"angle":1.25})),
            Command::Construct { dozer: ObjectId(5), template: "AmericaPowerPlant".into(), pos: cp, angle: 1.25 }
        );
        assert_eq!(
            c(json!({"type":"construct","dozer":5,"template":"AmericaPowerPlant","pos":p})),
            Command::Construct { dozer: ObjectId(5), template: "AmericaPowerPlant".into(), pos: cp, angle: 0.0 }
        );
        assert_eq!(c(json!({"type":"cancel_construct","dozer":5})), Command::CancelConstruct { dozer: ObjectId(5) });
        assert_eq!(
            c(json!({"type":"repair","dozer":5,"target":6})),
            Command::Repair { dozer: ObjectId(5), target: ObjectId(6) }
        );
        assert_eq!(
            c(json!({"type":"resume_construction","dozer":5,"target":6})),
            Command::ResumeConstruction { dozer: ObjectId(5), target: ObjectId(6) }
        );
        for ty in ["dock", "get_repaired", "get_healed"] {
            let cmd = c(json!({"type":ty,"ids":ids,"target":6}));
            match (ty, cmd) {
                ("dock", Command::Dock { target, .. }) => assert_eq!(target, ObjectId(6)),
                ("get_repaired", Command::GetRepaired { target, .. }) => assert_eq!(target, ObjectId(6)),
                ("get_healed", Command::GetHealed { target, .. }) => assert_eq!(target, ObjectId(6)),
                _ => panic!("{ty} decoded to the wrong command"),
            }
        }
        assert_eq!(c(json!({"type":"force_move","ids":ids,"pos":p})), Command::ForceMove { ids: two.clone(), pos: cp });
        assert_eq!(c(json!({"type":"add_waypoint","ids":ids,"pos":p})), Command::AddWaypoint { ids: two.clone(), pos: cp });
        assert_eq!(c(json!({"type":"scatter","ids":ids})), Command::Scatter { ids: two.clone() });
        assert_eq!(c(json!({"type":"toggle_overcharge","ids":ids})), Command::ToggleOvercharge { ids: two.clone() });
        assert_eq!(c(json!({"type":"switch_weapons","ids":ids,"slot":1})), Command::SwitchWeapons { ids: two.clone(), slot: 1 });
        assert_eq!(
            c(json!({"type":"cancel_upgrade","building":5,"upgrade":"UpgradeAmerica"})),
            Command::CancelUpgrade { building: ObjectId(5), upgrade: "UpgradeAmerica".into() }
        );
        assert_eq!(
            c(json!({"type":"combat_drop","ids":ids,"pos":p})),
            Command::CombatDrop { ids: two.clone(), target: CombatDropTarget::Position(cp) }
        );
        assert_eq!(
            c(json!({"type":"combat_drop","ids":ids,"target":4})),
            Command::CombatDrop { ids: two.clone(), target: CombatDropTarget::Object(ObjectId(4)) }
        );
        assert_eq!(c(json!({"type":"do_weapon","ids":ids,"slot":0,"max_shots":-1})), Command::DoWeapon { ids: two.clone(), slot: 0, max_shots: -1 });
        assert_eq!(
            c(json!({"type":"do_weapon_at_object","ids":ids,"slot":2,"target":6,"max_shots":5})),
            Command::DoWeaponAtObject { ids: two.clone(), slot: 2, target: ObjectId(6), max_shots: 5 }
        );
        assert_eq!(
            c(json!({"type":"do_weapon_at_location","ids":ids,"slot":1,"pos":p,"max_shots":0})),
            Command::DoWeaponAtLocation { ids: two.clone(), slot: 1, pos: cp, max_shots: 0 }
        );
        assert_eq!(c(json!({"type":"hack_internet","ids":ids})), Command::HackInternet { ids: two.clone() });
        assert_eq!(c(json!({"type":"set_mine_clearing_detail","ids":ids})), Command::SetMineClearingDetail { ids: two.clone() });
        assert_eq!(
            c(json!({"type":"do_special_power","source":5,"power":"SUPERWEAPON_SCRUD"})),
            Command::DoSpecialPower { source: ObjectId(5), power: "SUPERWEAPON_SCRUD".into(), target: SpecialPowerTarget::None }
        );
        assert_eq!(
            c(json!({"type":"do_special_power","source":5,"power":"P","pos":p})),
            Command::DoSpecialPower { source: ObjectId(5), power: "P".into(), target: SpecialPowerTarget::Position(cp) }
        );
        assert_eq!(
            c(json!({"type":"do_special_power","source":5,"power":"P","target":8})),
            Command::DoSpecialPower { source: ObjectId(5), power: "P".into(), target: SpecialPowerTarget::Object(ObjectId(8)) }
        );
        assert_eq!(c(json!({"type":"purchase_science","science":"SCIENCE_AMERICA"})), Command::PurchaseScience { science: "SCIENCE_AMERICA".into() });
        assert_eq!(
            c(json!({"type":"do_special_power_at_location","source":5,"power":"P","pos":p,"angle":0.5})),
            Command::DoSpecialPowerAtLocation { source: ObjectId(5), power: "P".into(), pos: cp, angle: Some(0.5) }
        );
        assert_eq!(
            c(json!({"type":"do_special_power_at_location","source":5,"power":"P","pos":p})),
            Command::DoSpecialPowerAtLocation { source: ObjectId(5), power: "P".into(), pos: cp, angle: None }
        );
    }

    #[test]
    fn type_names_ignore_case_and_separators() {
        let p = json!([0, 0, 0]);
        for ty in ["MoveTo", "move_to", "move-to", "MOVETO"] {
            assert_eq!(c(json!({"type":ty,"ids":[1],"pos":p})), Command::MoveTo { ids: vec![ObjectId(1)], pos: Coord3D::new(0.0, 0.0, 0.0) });
        }
    }

    #[test]
    fn position_accepts_an_object_form() {
        assert_eq!(
            c(json!({"type":"move_to","ids":[1],"pos":{"x":1,"y":2,"z":3}})),
            Command::MoveTo { ids: vec![ObjectId(1)], pos: Coord3D::new(1.0, 2.0, 3.0) }
        );
    }

    #[test]
    fn bad_input_is_an_error_not_a_panic() {
        let cases = [
            (json!([]), "object"),
            (json!({"ids":[]}), "type"),
            (json!({"type":"nope"}), "unknown type"),
            (json!({"type":"move_to","ids":[1]}), "missing field \"pos\""),
            (json!({"type":"move_to","ids":"1","pos":[0,0,0]}), "array"),
            (json!({"type":"move_to","ids":[-1],"pos":[0,0,0]}), "unsigned"),
            (json!({"type":"move_to","ids":[1],"pos":[0,0]}), "exactly 3"),
            (json!({"type":"move_to","ids":[1],"pos":[0,0,"x"]}), "must be a number"),
            (json!({"type":"move_to","ids":[1],"pos":{"x":0,"y":0}}), "missing \"z\""),
            (json!({"type":"attack","ids":[1],"target":"2"}), "unsigned"),
            (json!({"type":"attack","ids":[1],"target":4294967296u64}), "unsigned"),
            (json!({"type":"queue_unit","factory":1,"template":""}), "must not be empty"),
            (json!({"type":"queue_unit","factory":1,"template":5}), "must be a string"),
            (json!({"type":"cancel_unit","factory":1,"index":-1}), "integer in 0"),
            (json!({"type":"do_weapon","ids":[1],"slot":0,"max_shots":4294967296u64}), "32-bit"),
            (json!({"type":"guard","ids":[1]}), "exactly one"),
            (json!({"type":"guard","ids":[1],"target":2,"pos":[0,0,0]}), "exactly one"),
            (json!({"type":"guard","ids":[1],"pos":[0,0,0],"mode":"sideways"}), "not normal"),
            (json!({"type":"combat_drop","ids":[1]}), "exactly one"),
            (json!({"type":"do_special_power","source":1,"power":"P","target":2,"pos":[0,0,0]}), "not both"),
        ];
        for (v, want) in cases {
            let e = d(v.clone()).expect_err(&format!("{v} must be rejected"));
            assert!(e.contains(want), "{v}: error {e:?} does not mention {want:?}");
        }
    }

    #[test]
    fn oversized_input_is_rejected() {
        let ids: Vec<u32> = (0..(MAX_IDS as u32 + 1)).collect();
        let e = d(json!({"type":"stop","ids":ids})).unwrap_err();
        assert!(e.contains("at most"), "{e}");
        let name = "x".repeat(MAX_NAME_LEN + 1);
        let e = d(json!({"type":"purchase_science","science":name})).unwrap_err();
        assert!(e.contains("at most"), "{e}");
    }

    #[test]
    fn angle_out_of_f32_range_is_rejected() {
        let e = d(json!({"type":"construct","dozer":1,"template":"T","pos":[0,0,0],"angle":1e300})).unwrap_err();
        assert!(e.contains("out of range"), "{e}");
    }
}
