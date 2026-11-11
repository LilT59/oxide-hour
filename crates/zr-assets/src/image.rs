//! Decoded image shared by the terrain and model crates.

/// Tightly packed 8-bit RGBA pixels, row-major, top row first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
    /// The file's own mip levels below `pixels`, largest first (a DDS's authored chain; the engine loads
    /// these for compressed textures instead of generating them). Empty when the file has none (TGA) or
    /// when the pixels were changed after decoding, so a consumer must regenerate the chain.
    pub mips: Vec<RgbaImage>,
}

impl RgbaImage {
    pub fn new(width: u32, height: u32) -> Self {
        Self { width, height, pixels: vec![0; width as usize * height as usize * 4], mips: Vec::new() }
    }

    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = (y as usize * self.width as usize + x as usize) * 4;
        self.pixels[i..i + 4].try_into().unwrap()
    }

    pub fn set_pixel(&mut self, x: u32, y: u32, rgba: [u8; 4]) {
        let i = (y as usize * self.width as usize + x as usize) * 4;
        self.pixels[i..i + 4].copy_from_slice(&rgba);
    }
}
