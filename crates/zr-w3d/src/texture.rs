//! Texture lookup and decoding.
//!
//! W3D files name textures like `avcrusader.tga`, but the engine
//! (`ddsfile.cpp`) swaps the extension to `.dds` and Zero Hour keeps them
//! under `Art\Textures\`. We try `.dds` then `.tga`, under `art\textures\`
//! then the given directory.

use crate::error::Error;
use crate::{dds, tga};
use zr_assets::{normalize_path, RgbaImage, Vfs};

/// Candidate VFS paths for a texture reference, best first.
pub(crate) fn candidates(name: &str) -> Vec<String> {
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

/// Resolves a texture reference to the path that exists in the VFS.
pub(crate) fn resolve(vfs: &Vfs, name: &str) -> Option<String> {
    candidates(name).into_iter().find(|p| vfs.read(p).is_some())
}

/// Loads and decodes a texture. `name` may be a bare W3D reference
/// (`foo.tga`) or a VFS path (`art\textures\foo.dds`).
pub fn load_texture(vfs: &Vfs, name: &str) -> Result<RgbaImage, Error> {
    let path = resolve(vfs, name).ok_or_else(|| Error::NotFound(name.to_string()))?;
    let bytes = vfs.read(&path).ok_or_else(|| Error::NotFound(path.clone()))?;
    if path.ends_with(".dds") {
        dds::decode(&bytes)
    } else {
        tga::decode(&bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_order() {
        let c = candidates("AVCrusader.TGA");
        assert_eq!(c[0], "art\\textures\\avcrusader.dds");
        assert!(c.contains(&"art\\textures\\avcrusader.tga".to_string()));
        let c = candidates("art\\textures\\x.dds");
        assert_eq!(c[0], "art\\textures\\x.dds");
        assert_eq!(c.iter().filter(|s| s.as_str() == "art\\textures\\x.dds").count(), 1);
    }
}
