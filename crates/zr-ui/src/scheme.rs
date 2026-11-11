//! `ControlBarScheme` INI (`GameClient/GUI/ControlBar/ControlBarScheme.cpp`).
//!
//! A scheme is the metallic frame the engine draws under/over `ControlBar.wnd`:
//! a `Side` and `ScreenCreationRes`, then `ImagePart` blocks (`Position`, `Size`,
//! `ImageName`, `Layer` 0-5). `drawBackground` renders layers 5..3 (under the
//! windows); `drawForeground` renders layers 2..0 (over them)
//! (`ControlBarScheme::drawBackground` / `drawForeground`). The scheme is chosen
//! by the local player's side, largest `ScreenCreationRes.x` winning, falling
//! back to `Default` (`ControlBarSchemeManager::setControlBarSchemeByPlayer`).

use std::collections::BTreeMap;

use zr_ini::IniDb;

use crate::Rgba;

/// `MAX_CONTROL_BAR_SCHEME_IMAGE_LAYERS`.
pub const MAX_LAYERS: usize = 6;
/// `CONTROL_BAR_SCHEME_FOREGROUND_IMAGE_LAYERS` (layers 0..2 are foreground).
pub const FOREGROUND_LAYERS: usize = 3;

/// One `ImagePart` (`ControlBarSchemeImage`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SchemeImage {
    pub position: (i32, i32),
    pub size: (i32, i32),
    /// `ImageName` (a `MappedImage`), `None` if missing.
    pub image: Option<String>,
    /// 0 (top) .. 5 (bottom); layers 0-2 foreground, 3-5 background.
    pub layer: i32,
}

/// One `ControlBarScheme`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ControlBarScheme {
    pub name: String,
    pub screen_creation_res: (i32, i32),
    pub side: String,
    pub images: Vec<SchemeImage>,
    pub queue_button_image: Option<String>,
    pub right_hud_image: Option<String>,
    pub exp_bar_foreground_image: Option<String>,
    pub command_bar_border_color: Option<Rgba>,
    /// `PowerBarUL` / `PowerBarLR`.
    pub power_bar: Option<((i32, i32), (i32, i32))>,
    /// `MoneyUL` / `MoneyLR`.
    pub money: Option<((i32, i32), (i32, i32))>,
}

impl ControlBarScheme {
    /// `drawBackground`: layers `MAX-1`..`FOREGROUND`, top of that range first.
    pub fn background_images(&self) -> impl Iterator<Item = &SchemeImage> {
        (FOREGROUND_LAYERS..MAX_LAYERS)
            .rev()
            .flat_map(move |l| self.images.iter().filter(move |i| i.layer == l as i32))
    }

    /// `drawForeground`: layers `FOREGROUND-1`..0.
    pub fn foreground_images(&self) -> impl Iterator<Item = &SchemeImage> {
        (0..FOREGROUND_LAYERS)
            .rev()
            .flat_map(move |l| self.images.iter().filter(move |i| i.layer == l as i32))
    }
}

/// All schemes, name-indexed.
#[derive(Debug, Clone, Default)]
pub struct SchemeDb {
    schemes: Vec<ControlBarScheme>,
    index: BTreeMap<String, usize>,
}

fn coord(v: &str) -> (i32, i32) {
    let n = crate::value::ints(v);
    (n.first().copied().unwrap_or(0), n.get(1).copied().unwrap_or(0))
}

fn color(v: &str) -> Rgba {
    let n = crate::value::ints(v);
    Rgba {
        r: n.first().copied().unwrap_or(0).clamp(0, 255) as u8,
        g: n.get(1).copied().unwrap_or(0).clamp(0, 255) as u8,
        b: n.get(2).copied().unwrap_or(0).clamp(0, 255) as u8,
        a: n.get(3).copied().unwrap_or(255).clamp(0, 255) as u8,
    }
}

impl SchemeDb {
    /// Builds the database from the merged [`IniDb`] (`ControlBarSchemeManager::init`).
    pub fn from_ini(db: &IniDb) -> SchemeDb {
        let mut out = SchemeDb::default();
        for b in db.of_kind("ControlBarScheme") {
            let name = b.name().unwrap_or("").to_owned();
            let Some(block) = db.resolved("ControlBarScheme", &name) else { continue };
            let s = |k: &str| block.value(k).map(crate::value::trimmed).filter(|v| !v.is_empty());
            let mut scheme = ControlBarScheme {
                name: name.clone(),
                screen_creation_res: block.value("ScreenCreationRes").map_or((0, 0), coord),
                side: s("Side").unwrap_or_default(),
                queue_button_image: s("QueueButtonImage"),
                right_hud_image: s("RightHUDImage"),
                exp_bar_foreground_image: s("ExpBarForegroundImage"),
                command_bar_border_color: block.value("CommandBarBorderColor").map(color),
                ..Default::default()
            };
            let (ul, lr) = (s("PowerBarUL"), s("PowerBarLR"));
            if let (Some(a), Some(b)) = (ul, lr) {
                scheme.power_bar = Some((coord(&a), coord(&b)));
            }
            let (ul, lr) = (s("MoneyUL"), s("MoneyLR"));
            if let (Some(a), Some(b)) = (ul, lr) {
                scheme.money = Some((coord(&a), coord(&b)));
            }
            for part in block.children_named("ImagePart") {
                scheme.images.push(parse_image_part(part));
            }
            // `AnimatingPart` adds its `ImagePart` too (`parseAnimatingPart`).
            for anim in block.children_named("AnimatingPart") {
                if let Some(part) = anim.child("ImagePart") {
                    scheme.images.push(parse_image_part(part));
                }
            }
            out.index.insert(name, out.schemes.len());
            out.schemes.push(scheme);
        }
        out
    }

    /// `findControlBarScheme` (exact name).
    pub fn get(&self, name: &str) -> Option<&ControlBarScheme> {
        self.index.get(name).map(|&i| &self.schemes[i])
    }

    /// `setControlBarSchemeByPlayer`: match `Side` case-insensitively, take the
    /// scheme with the largest `ScreenCreationRes.x`, else `Default`.
    pub fn by_side(&self, side: &str) -> Option<&ControlBarScheme> {
        let mut best: Option<&ControlBarScheme> = None;
        for s in &self.schemes {
            if s.side.eq_ignore_ascii_case(side)
                && best.is_none_or(|b| b.screen_creation_res.0 < s.screen_creation_res.0)
            {
                best = Some(s);
            }
        }
        best.or_else(|| self.get("Default"))
    }

    pub fn schemes(&self) -> &[ControlBarScheme] {
        &self.schemes
    }

    pub fn len(&self) -> usize {
        self.schemes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.schemes.is_empty()
    }
}

fn parse_image_part(part: &zr_ini::Block) -> SchemeImage {
    SchemeImage {
        position: part.value("Position").map_or((0, 0), coord),
        size: part.value("Size").map_or((0, 0), coord),
        image: part.value("ImageName").map(crate::value::trimmed).filter(|v| !v.is_empty()),
        layer: part.value("Layer").map_or(0, crate::value::int),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_db(ini: &str) -> IniDb {
        let mut db = IniDb::new();
        db.extend(zr_ini::parse("test.ini", ini).unwrap());
        db
    }

    #[test]
    fn parses_image_parts_and_layers() {
        let db = parse_db(
            r#"
ControlBarScheme America8x6
  ScreenCreationRes X:800 Y:600
  Side America
  MoneyUL X:360 Y:438
  MoneyLR X:439 Y:457
  CommandBarBorderColor R:0 G:21 B:126 A:255
  ImagePart
    Position X:0 Y:408
    Size X:800 Y:191
    ImageName InGameUIAmericaBase
    Layer 4
  End
  ImagePart
    Position X:10 Y:10
    Size X:20 Y:20
    ImageName TopThing
    Layer 1
  End
End
"#,
        );
        let schemes = SchemeDb::from_ini(&db);
        let s = schemes.by_side("america").unwrap();
        assert_eq!(s.screen_creation_res, (800, 600));
        assert_eq!(s.money, Some(((360, 438), (439, 457))));
        assert_eq!(s.command_bar_border_color, Some(Rgba { r: 0, g: 21, b: 126, a: 255 }));
        assert_eq!(s.images.len(), 2);
        assert_eq!(s.background_images().map(|i| i.image.as_deref()).collect::<Vec<_>>(), [Some("InGameUIAmericaBase")]);
        assert_eq!(s.foreground_images().map(|i| i.image.as_deref()).collect::<Vec<_>>(), [Some("TopThing")]);
    }

    #[test]
    fn side_match_is_case_insensitive_and_falls_back_to_default() {
        let db = parse_db(
            r#"
ControlBarScheme America8x6
  Side America
  ScreenCreationRes X:800 Y:600
End
ControlBarScheme Default
  Side Observer
  ScreenCreationRes X:640 Y:480
End
"#,
        );
        let schemes = SchemeDb::from_ini(&db);
        assert_eq!(schemes.by_side("AMERICA").unwrap().name, "America8x6");
        assert_eq!(schemes.by_side("GLA").unwrap().name, "Default");
    }
}
