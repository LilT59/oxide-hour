//! Object template -> default-state model name.
//!
//! Mirrors `W3DModelDraw` parsing: a `W3D*Draw` module holds
//! `DefaultConditionState` / `ConditionState` blocks whose `Model` field names
//! the render object. (W3DModelDraw.cpp, `parseConditionState`.)

use zr_ini::{Block, IniDb};

fn model_of(state: &Block) -> Option<String> {
    let m = state.field("Model")?.tokens().first().copied()?.to_string();
    (!m.eq_ignore_ascii_case("none")).then_some(m)
}

fn model_in_object(obj: &Block) -> Option<String> {
    for draw in obj.children_named("Draw") {
        let module = draw.header.first().map(String::as_str).unwrap_or("");
        if !(module.starts_with("W3D") && module.ends_with("Draw")) {
            continue;
        }
        // W3DTreeDraw / W3DPropDraw name the model directly.
        if let Some(m) = model_of(draw).or_else(|| {
            let v = draw.field("ModelName")?.tokens().first().copied()?;
            (!v.eq_ignore_ascii_case("none")).then(|| v.to_string())
        }) {
            return Some(m);
        }
        if let Some(m) = draw.children_named("DefaultConditionState").find_map(model_of) {
            return Some(m);
        }
        if let Some(m) = draw.children_named("ConditionState").find_map(model_of) {
            return Some(m);
        }
    }
    None
}

/// Default-state model of an `Object` (following `ObjectReskin` to its parent
/// when the reskin defines no model of its own).
pub fn model_for_template(ini: &IniDb, template: &str) -> Option<String> {
    let mut name = template.to_string();
    for _ in 0..16 {
        let obj = ini.resolved("Object", &name)?;
        if let Some(m) = model_in_object(&obj) {
            return Some(m);
        }
        let parent = ini
            .definitions("Object", &name)
            .into_iter()
            .find(|d| d.kind == "ObjectReskin")
            .and_then(|d| d.header.get(1).cloned())?;
        name = parent;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db(text: &str) -> IniDb {
        let mut db = IniDb::new();
        db.extend(zr_ini::parse("test.ini", text).unwrap());
        db
    }

    #[test]
    fn default_state_and_reskin() {
        let db = db("Object Tank\n  Draw = W3DModelDraw ModuleTag_01\n    ConditionState = DAMAGED\n      Model = TankD\n    End\n    DefaultConditionState\n      Model = TankA\n    End\n  End\nEnd\nObjectReskin TankSkin Tank\n  Side = X\nEnd\nObject Empty\n  Draw = W3DModelDraw ModuleTag_01\n    ConditionState = A\n      Model = Only\n    End\n  End\nEnd\nObject Nothing\n  Draw = W3DModelDraw ModuleTag_01\n    DefaultConditionState\n      Model = NONE\n    End\n  End\nEnd\n");
        assert_eq!(model_for_template(&db, "Tank").as_deref(), Some("TankA"));
        assert_eq!(model_for_template(&db, "TankSkin").as_deref(), Some("TankA"));
        assert_eq!(model_for_template(&db, "Empty").as_deref(), Some("Only"));
        assert_eq!(model_for_template(&db, "Nothing"), None);
        assert_eq!(model_for_template(&db, "Missing"), None);
    }
}
