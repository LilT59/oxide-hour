//! Image -> texture resolution against the VFS.
//!
//! A window draw-data slot names a `MappedImage`; that names a texture file
//! (`.tga` in the INI) which the engine's `GameFileClass::Set_Name`
//! (`W3DDevice/GameClient/W3DFileSystem.cpp`) finds, in order:
//!
//! 1. `Data/<language>/Art/Textures/<name>` — localized art (the `Data/%s/Art/Textures/`
//!    branch, taken before anything else for image files);
//! 2. `Art/Textures/<name>` (`TGA_DIR_PATH`) — the normal archive location;
//! 3. the bare name.
//!
//! `WW3D`'s texture loader swaps `.tga` for `.dds` (the shipped textures are usually
//! compressed), so each directory is tried with `.dds` then `.tga` (see `zr-w3d`).

use zr_assets::normalize_path;

use crate::image::ImageDb;

/// File stem (no directory, no extension) of a texture reference.
fn stem_of(name: &str) -> String {
    let n = normalize_path(name.trim());
    let file = match n.rfind('\\') {
        Some(i) => &n[i + 1..],
        None => n.as_str(),
    };
    file.rsplit_once('.').map_or(file, |(s, _)| s).to_owned()
}

/// Outcome of resolving one window image reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageResolution {
    /// `NoImage` (no reference).
    NoImage,
    Found { image: String, texture: String, path: String },
    /// No `MappedImage` block with this name.
    MissingImage { image: String },
    /// The image exists but its texture is not in the VFS.
    MissingTexture { image: String, texture: String, candidates: Vec<String> },
}

/// Candidate VFS paths for a texture reference, best first (no localization).
pub fn texture_candidates(name: &str) -> Vec<String> {
    let n = normalize_path(name.trim());
    let (dir, file) = match n.rfind('\\') {
        Some(i) => (&n[..i + 1], &n[i + 1..]),
        None => ("", n.as_str()),
    };
    let stem = file.rsplit_once('.').map_or(file, |(s, _)| s);
    let mut out: Vec<String> = Vec::new();
    let mut push = |s: String| {
        if !out.contains(&s) {
            out.push(s);
        }
    };
    if !dir.is_empty() {
        push(format!("{dir}{stem}.dds"));
        push(format!("{dir}{stem}.tga"));
    }
    push(format!("art\\textures\\{stem}.dds"));
    push(format!("art\\textures\\{stem}.tga"));
    push(format!("{stem}.dds"));
    push(format!("{stem}.tga"));
    out
}

/// [`texture_candidates`] with the engine's localized `Data/<language>/Art/Textures/`
/// directory tried first (`GameFileClass::Set_Name`, image branch).
pub fn texture_candidates_lang(name: &str, language: &str) -> Vec<String> {
    let mut out = Vec::new();
    if !language.is_empty() {
        let lang = normalize_path(language);
        let stem = stem_of(name);
        out.push(format!("data\\{lang}\\art\\textures\\{stem}.dds"));
        out.push(format!("data\\{lang}\\art\\textures\\{stem}.tga"));
    }
    for c in texture_candidates(name) {
        if !out.contains(&c) {
            out.push(c);
        }
    }
    out
}

impl ImageResolution {
    /// Whether this is a fully resolved (image + texture) reference.
    pub fn resolve_ok(&self) -> bool {
        matches!(self, ImageResolution::Found { .. })
    }
}

impl ImageDb {
    /// Resolves `name` to the texture path that exists, via `exists`. `language` is the
    /// `Data/<language>` directory the engine would use for localized art.
    pub fn resolve(&self, name: &str, language: &str, exists: &dyn Fn(&str) -> bool) -> ImageResolution {
        let Some(img) = self.get(name) else {
            return ImageResolution::MissingImage { image: name.to_owned() };
        };
        let candidates = texture_candidates_lang(&img.texture, language);
        match candidates.iter().find(|p| exists(p)) {
            Some(path) => ImageResolution::Found {
                image: img.name.clone(),
                texture: img.texture.clone(),
                path: path.clone(),
            },
            None => ImageResolution::MissingTexture {
                image: img.name.clone(),
                texture: img.texture.clone(),
                candidates,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_prefer_art_textures_dds_then_tga() {
        assert_eq!(texture_candidates("Buttons.tga")[0], "art\\textures\\buttons.dds");
        assert_eq!(texture_candidates("Buttons.tga")[1], "art\\textures\\buttons.tga");
    }

    #[test]
    fn localized_dir_is_tried_first() {
        let c = texture_candidates_lang("SAUserInterface512_005.tga", "English");
        assert_eq!(c[0], "data\\english\\art\\textures\\sauserinterface512_005.dds");
        assert_eq!(c[1], "data\\english\\art\\textures\\sauserinterface512_005.tga");
        assert!(c.contains(&"art\\textures\\sauserinterface512_005.tga".to_string()));
    }
}
