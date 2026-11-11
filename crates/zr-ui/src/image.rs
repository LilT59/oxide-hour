//! `MappedImage` INI blocks (`GameClient/System/Image.cpp`,
//! `Image::m_imageFieldParseTable`), with the engine's UV and image-size maths.
//!
//! Images are loaded from `Data\INI\MappedImages\TextureSize_512` and
//! `Data\INI\MappedImages\HandCreated` (both in `zr_ini::LOAD_ORDER`), so the
//! merged [`IniDb`] already holds every block. Lookup is case-insensitive
//! (`ImageCollection::findImageByName` -> `nameToLowercaseKey`).

use std::collections::BTreeMap;

use zr_ini::IniDb;

/// `ImageStatus` bit positions (`Image.h`).
pub mod image_status {
    pub const ROTATED_90_CLOCKWISE: u32 = 0x0001;
    pub const RAW_TEXTURE: u32 = 0x0002;

    pub const NAMES: &[&str] = &["ROTATED_90_CLOCKWISE", "RAW_TEXTURE"];
}

/// One page rectangle inside a texture atlas.
#[derive(Debug, Clone, PartialEq)]
pub struct MappedImage {
    pub name: String,
    pub texture: String,
    pub texture_width: i32,
    pub texture_height: i32,
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub status: u32,
    /// `(lo.x, lo.y, hi.x, hi.y)` in 0..1, or the identity when the texture size is 0.
    pub uv: [f32; 4],
    /// `getImageWidth` / `getImageHeight`, after the rotated status swap.
    pub width: i32,
    pub height: i32,
}

impl Default for MappedImage {
    fn default() -> Self {
        MappedImage {
            name: String::new(),
            texture: String::new(),
            texture_width: 0,
            texture_height: 0,
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
            status: 0,
            uv: [0.0, 0.0, 1.0, 1.0],
            width: 0,
            height: 0,
        }
    }
}

impl MappedImage {
    /// `Image::parseImageCoords` + `parseImageStatus`.
    pub fn recompute(&mut self) {
        self.uv = [
            if self.texture_width != 0 { self.left as f32 / self.texture_width as f32 } else { 0.0 },
            if self.texture_height != 0 { self.top as f32 / self.texture_height as f32 } else { 0.0 },
            if self.texture_width != 0 { self.right as f32 / self.texture_width as f32 } else { 1.0 },
            if self.texture_height != 0 { self.bottom as f32 / self.texture_height as f32 } else { 1.0 },
        ];
        let (w, h) = (self.right - self.left, self.bottom - self.top);
        if self.status & image_status::ROTATED_90_CLOCKWISE != 0 {
            self.width = h;
            self.height = w;
        } else {
            self.width = w;
            self.height = h;
        }
    }

    pub fn is_rotated(&self) -> bool {
        self.status & image_status::ROTATED_90_CLOCKWISE != 0
    }
}

/// All `MappedImage` blocks, with a case-insensitive index.
#[derive(Debug, Clone, Default)]
pub struct ImageDb {
    images: Vec<MappedImage>,
    index: BTreeMap<String, usize>,
}

impl ImageDb {
    /// Builds the collection from the merged database (`ImageCollection::load`).
    pub fn from_ini(db: &IniDb) -> ImageDb {
        let mut out = ImageDb::default();
        for block in db.of_kind("MappedImage") {
            let mut img = MappedImage { name: block.name().unwrap_or("").to_owned(), ..Default::default() };
            if let Some(v) = block.value("Texture") {
                img.texture = crate::value::trimmed(v);
            }
            if let Some(v) = block.value("TextureWidth") {
                img.texture_width = crate::value::int(v);
            }
            if let Some(v) = block.value("TextureHeight") {
                img.texture_height = crate::value::int(v);
            }
            if let Some(v) = block.value("Coords") {
                let n = crate::value::ints(v);
                img.left = n.first().copied().unwrap_or(0);
                img.top = n.get(1).copied().unwrap_or(0);
                img.right = n.get(2).copied().unwrap_or(0);
                img.bottom = n.get(3).copied().unwrap_or(0);
            }
            if let Some(v) = block.value("Status") {
                img.status = crate::value::bits(v, image_status::NAMES);
            }
            img.recompute();
            let key = img.name.to_ascii_lowercase();
            if out.index.contains_key(&key) {
                out.images[out.index[&key]] = img;
            } else {
                out.index.insert(key, out.images.len());
                out.images.push(img);
            }
        }
        out
    }

    /// `findImageByName`: case-insensitive.
    pub fn get(&self, name: &str) -> Option<&MappedImage> {
        self.index.get(&name.to_ascii_lowercase()).map(|&i| &self.images[i])
    }

    pub fn images(&self) -> &[MappedImage] {
        &self.images
    }

    pub fn len(&self) -> usize {
        self.images.len()
    }

    pub fn is_empty(&self) -> bool {
        self.images.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zr_ini::IniDb;

    fn db(ini: &str) -> IniDb {
        let mut d = IniDb::new();
        d.extend(zr_ini::parse("images.ini", ini).unwrap());
        d
    }

    #[test]
    fn parses_uv_and_size() {
        let d = db("MappedImage Buttons-Left\n  Texture = Buttons.tga\n  TextureWidth = 512\n  TextureHeight = 256\n  Coords = Left:64 Top:0 Right:128 Bottom:32\n  Status = NONE\nEnd\n");
        let images = ImageDb::from_ini(&d);
        let i = images.get("buttons-left").unwrap();
        assert_eq!(i.texture, "Buttons.tga");
        assert_eq!((i.width, i.height), (64, 32));
        assert_eq!(i.uv, [64.0 / 512.0, 0.0, 128.0 / 512.0, 32.0 / 256.0]);
        assert!(!i.is_rotated());
    }

    #[test]
    fn rotated_swaps_width_and_height() {
        let d = db("MappedImage R\n  TextureWidth = 100\n  TextureHeight = 50\n  Coords = Left:0 Top:0 Right:20 Bottom:10\n  Status = ROTATED_90_CLOCKWISE\nEnd\n");
        let images = ImageDb::from_ini(&d);
        let i = images.get("R").unwrap();
        assert_eq!((i.width, i.height), (10, 20));
    }

    #[test]
    fn later_definition_wins() {
        let d = db("MappedImage X\n  Texture = a.tga\nEnd\nMappedImage X\n  Texture = b.tga\nEnd\n");
        let images = ImageDb::from_ini(&d);
        assert_eq!(images.len(), 1);
        assert_eq!(images.get("x").unwrap().texture, "b.tga");
    }
}
