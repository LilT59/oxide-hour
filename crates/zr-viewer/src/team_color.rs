//! Team (house) colours: which colour an object gets and how model parts are tinted with it.
//!
//! **Colour of an object** (`GameLogic::startNewGame`, `Object::getIndicatorColor`,
//! `Player::init`/`initFromDict`): `originalOwner` names a team (`validateTeam`); the team's
//! `teamOwner` is a player; the player's colour is the sides dict `playerColor` if present, else the
//! `PreferredColor` of the `PlayerTemplate` named by `playerFaction`; the neutral player is white
//! (`NEUTRAL_PLAYER_COLOR` 0xffffffff). Unknown owners fall back to neutral.
//!
//! **Tinting** (`W3DAssetManager::Recolor_Mesh` / `Remap_Palette`):
//! * a mesh whose name (or the part after the `.`) starts with `HOUSECOLOR` gets the colour as its
//!   vertex-material ambient/diffuse, which modulates the texture -> `StandardMaterial::base_color`;
//! * a texture whose file name starts with `ZHC` is remapped on the CPU:
//!   `ZHCA*` — pixels with alpha < 255 are hue-shifted to the team hue (saturation scaled by the
//!   team's saturation), then made opaque; `ZHCD*` — only the 16-colour palette in the top row is
//!   replaced by the team colour scaled by `HOUSE_COLOR_SCALE`; any other `ZHC*` — pixels equal to one
//!   of the top-row palette entries are replaced by the matching scaled team colour.
//!
//! Tinted copies are built per (template, colour) by the caller; untinted models are shared.

use std::collections::HashMap;

use zr_assets::RgbaImage;
use zr_ini::IniDb;
use zr_map::{DictValue, MapFile, MapObject};

use crate::source::ModelPart;

/// `houseColorScale` in `W3DAssetManager.cpp`.
const HOUSE_COLOR_SCALE: [u32; 16] = [255, 239, 223, 211, 195, 174, 167, 151, 135, 123, 107, 91, 79, 63, 47, 35];

pub const NEUTRAL: [u8; 3] = [255, 255, 255];

/// Player colour resolution for one map.
pub struct TeamColors {
    /// lower-case team name -> colour.
    teams: HashMap<String, [u8; 3]>,
}

fn parse_rgb(s: &str) -> Option<[u8; 3]> {
    let mut c = [None; 3];
    for t in s.split_whitespace() {
        let (k, v) = t.split_once(':')?;
        let i = match k.to_ascii_uppercase().as_str() {
            "R" => 0,
            "G" => 1,
            "B" => 2,
            _ => continue,
        };
        c[i] = Some(v.parse::<i32>().ok()?.clamp(0, 255) as u8);
    }
    Some([c[0]?, c[1]?, c[2]?])
}

impl TeamColors {
    pub fn new(map: &MapFile, ini: &IniDb) -> Self {
        let mut teams = HashMap::new();
        let Some(sides) = &map.sides else { return Self { teams } };
        let player_color = |name: &str| -> [u8; 3] { Self::player_color_of(map, ini, name) };
        for t in &sides.teams {
            let (Some(name), Some(owner)) = (t.str("teamName"), t.str("teamOwner")) else { continue };
            teams.insert(name.to_ascii_lowercase(), player_color(owner));
        }
        Self { teams }
    }

    fn player_color_of(map: &MapFile, ini: &IniDb, name: &str) -> [u8; 3] {
        let Some(sides) = &map.sides else { return NEUTRAL };
        {
            let Some(side) = sides.sides.iter().find(|s| s.dict.str("playerName").is_some_and(|n| n.eq_ignore_ascii_case(name))) else {
                return NEUTRAL;
            };
            if let Some(DictValue::Int(c)) = side.dict.get("playerColor") {
                return [(c >> 16) as u8, (c >> 8) as u8, *c as u8];
            }
            side.dict
                .str("playerFaction")
                .and_then(|f| ini.get("PlayerTemplate", f))
                .and_then(|b| b.value("PreferredColor"))
                .and_then(parse_rgb)
                .unwrap_or(NEUTRAL)
        }
    }

    /// Colour of a map-defined player (`playerName`), neutral when unknown.
    pub fn named_player(map: &MapFile, ini: &IniDb, name: &str) -> [u8; 3] {
        Self::player_color_of(map, ini, name)
    }

    /// PlayerTemplate `PreferredColor` of a faction (skirmish players have no map side).
    pub fn faction_color(ini: &IniDb, faction: &str) -> [u8; 3] {
        ini.get("PlayerTemplate", faction).and_then(|b| b.value("PreferredColor")).and_then(parse_rgb).unwrap_or(NEUTRAL)
    }

    /// House colour of a map object (neutral when the owner is unknown or empty).
    pub fn object_color(&self, o: &MapObject) -> [u8; 3] {
        o.props.str("originalOwner").and_then(|n| self.teams.get(&n.to_ascii_lowercase())).copied().unwrap_or(NEUTRAL)
    }
}

fn base_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

fn is_house_mesh(name: &str) -> bool {
    let n = name.split_once('.').map_or(name, |(_, b)| b);
    n.len() >= 10 && n[..10].eq_ignore_ascii_case("HOUSECOLOR")
}

fn is_house_texture(path: &str) -> bool {
    let b = base_name(path);
    b.len() >= 3 && b.is_char_boundary(3) && b[..3].eq_ignore_ascii_case("ZHC")
}

/// Whether tinting changes this part at all (so callers can share untinted parts).
pub fn uses_house_color(p: &ModelPart) -> bool {
    is_house_mesh(&p.name) || p.texture_name.as_deref().is_some_and(is_house_texture)
}

/// A copy of `p` with house colour `rgb` applied.
pub fn tint(p: &ModelPart, rgb: [u8; 3]) -> ModelPart {
    let mut out = p.clone();
    if is_house_mesh(&p.name) {
        // Vertex-material colour modulates the texture; Bevy base_color is linear.
        out.color = [srgb_to_linear(rgb[0]), srgb_to_linear(rgb[1]), srgb_to_linear(rgb[2]), p.color[3]];
    }
    if let (Some(img), Some(name)) = (&mut out.texture, p.texture_name.as_deref().filter(|n| is_house_texture(n))) {
        recolor_texture(img, base_name(name), rgb);
    }
    out
}

fn srgb_to_linear(b: u8) -> f32 {
    let c = f32::from(b) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// `W3DAssetManager::Recolor_Texture_One_Time`: the 4th character of the name selects the mode.
fn recolor_texture(img: &mut RgbaImage, name: &str, rgb: [u8; 3]) {
    // The recoloured top level no longer matches the file's authored mips, so the chain is regenerated.
    img.mips.clear();
    let w = img.width as usize;
    match name.as_bytes().get(3).map(u8::to_ascii_lowercase) {
        Some(b'd') => {
            for (x, s) in HOUSE_COLOR_SCALE.iter().enumerate().take(w) {
                set_scaled(&mut img.pixels[x * 4..x * 4 + 4], rgb, *s);
            }
        }
        Some(b'a') => remap_alpha(img, rgb),
        _ => remap_palette(img, rgb),
    }
}

fn set_scaled(px: &mut [u8], rgb: [u8; 3], scale: u32) {
    for c in 0..3 {
        px[c] = (u32::from(rgb[c]) * scale / 255) as u8;
    }
    px[3] = 255;
}

/// `remapTexture32Bit`: top row is a 16-entry palette; matching pixels below it are replaced.
fn remap_palette(img: &mut RgbaImage, rgb: [u8; 3]) {
    let w = img.width as usize;
    if w < 16 || img.height < 2 {
        return;
    }
    let pal: Vec<[u8; 3]> = (0..16).map(|x| [img.pixels[x * 4], img.pixels[x * 4 + 1], img.pixels[x * 4 + 2]]).collect();
    let mut new = [[0u8; 4]; 16];
    for (n, s) in new.iter_mut().zip(HOUSE_COLOR_SCALE) {
        set_scaled(n, rgb, s);
    }
    for px in img.pixels[w * 4..].chunks_exact_mut(4) {
        if let Some(i) = pal.iter().position(|p| p[..] == px[..3]) {
            px.copy_from_slice(&new[i]);
        }
    }
    for (x, n) in new.iter().enumerate() {
        img.pixels[x * 4..x * 4 + 4].copy_from_slice(n);
    }
}

/// `remapAlphaTexture32Bit` with `DO_HUE_SHIFT`.
fn remap_alpha(img: &mut RgbaImage, rgb: [u8; 3]) {
    let team = rgb_to_hsv(rgb.map(|c| f32::from(c) / 255.0));
    for px in img.pixels.chunks_exact_mut(4) {
        if px[3] != 255 {
            let mut hsv = rgb_to_hsv([px[0], px[1], px[2]].map(|c| f32::from(c) / 255.0));
            hsv[0] = team[0];
            hsv[1] *= team[1];
            let c = hsv_to_rgb(hsv);
            for i in 0..3 {
                px[i] = (c[i] * 255.0) as u8;
            }
        }
        px[3] = 255;
    }
}

/// Hue in 0..1 (WW3D `RGB_To_HSV` convention).
fn rgb_to_hsv([r, g, b]: [f32; 3]) -> [f32; 3] {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let v = max;
    let s = if max > 0.0 { d / max } else { 0.0 };
    let h = if d <= 0.0 {
        0.0
    } else if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    } / 6.0;
    [h, s, v]
}

fn hsv_to_rgb([h, s, v]: [f32; 3]) -> [f32; 3] {
    let h6 = h.rem_euclid(1.0) * 6.0;
    let i = h6.floor();
    let f = h6 - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    match i as i32 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_parse() {
        assert_eq!(parse_rgb("R:10 G:20 B:300"), Some([10, 20, 255]));
        assert_eq!(parse_rgb("R:1 G:2"), None);
    }

    #[test]
    fn mesh_and_texture_names() {
        assert!(is_house_mesh("HOUSECOLOR01"));
        assert!(is_house_mesh("AVTANK.housecolor02"));
        assert!(!is_house_mesh("AVTANK"));
        assert!(is_house_texture("art\\textures\\zhca_tank.dds"));
        assert!(!is_house_texture("art\\textures\\avtank.dds"));
    }

    #[test]
    fn hsv_round_trip() {
        let c = [0.8, 0.3, 0.1];
        let r = hsv_to_rgb(rgb_to_hsv(c));
        for i in 0..3 {
            assert!((r[i] - c[i]).abs() < 1e-5);
        }
    }

    #[test]
    fn alpha_mask_recolors_masked_pixels_only() {
        let mut img = RgbaImage { width: 2, height: 1, pixels: vec![200, 50, 50, 0, 10, 20, 30, 255], mips: Vec::new() };
        remap_alpha(&mut img, [0, 0, 255]);
        assert_eq!(img.pixels[3], 255);
        assert!(img.pixels[2] > img.pixels[0]); // masked pixel became blue-ish
        assert_eq!(&img.pixels[4..], &[10, 20, 30, 255]);
    }

    #[test]
    fn palette_remap() {
        let mut px = vec![0u8; 16 * 4 * 2];
        for x in 0..16 {
            px[x * 4..x * 4 + 3].copy_from_slice(&[x as u8 + 1, 0, 0]);
        }
        px[16 * 4..16 * 4 + 3].copy_from_slice(&[3, 0, 0]); // second row uses palette entry 2
        let mut img = RgbaImage { width: 16, height: 2, pixels: px, mips: Vec::new() };
        remap_palette(&mut img, [255, 0, 0]);
        assert_eq!(img.pixels[16 * 4], 223);
    }
}
