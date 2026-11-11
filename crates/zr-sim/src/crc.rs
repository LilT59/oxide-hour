//! State hashing in the style of the original's `XferCRC` (Common/Xfer/XferCRC.cpp):
//! `crc = (crc << 1) + word + (crc >> 31)` over 32-bit words.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Crc(u32);

impl Crc {
    pub fn new() -> Self {
        Self(0)
    }
    /// `XferCRC::addCRC`.
    pub fn add_u32(&mut self, v: u32) {
        self.0 = (self.0 << 1).wrapping_add(v).wrapping_add((self.0 >> 31) & 1);
    }
    pub fn add_i32(&mut self, v: i32) {
        self.add_u32(v as u32);
    }
    pub fn add_bool(&mut self, v: bool) {
        self.add_u32(u32::from(v));
    }
    /// Floats are hashed by bit pattern, so `-0.0 != 0.0` and NaN payloads matter (as in the original).
    pub fn add_f32(&mut self, v: f32) {
        self.add_u32(v.to_bits());
    }
    pub fn add_coord(&mut self, c: &crate::math::Coord3D) {
        self.add_f32(c.x);
        self.add_f32(c.y);
        self.add_f32(c.z);
    }
    /// `xferImplementation`: whole big-endian words, then the 1-3 byte tail packed big-endian.
    pub fn add_bytes(&mut self, data: &[u8]) {
        let mut chunks = data.chunks_exact(4);
        for c in &mut chunks {
            self.add_u32(u32::from_be_bytes([c[0], c[1], c[2], c[3]]));
        }
        let rest = chunks.remainder();
        if !rest.is_empty() {
            let mut b = [0u8; 4];
            b[..rest.len()].copy_from_slice(rest);
            self.add_u32(u32::from_be_bytes(b));
        }
    }
    pub fn add_str(&mut self, s: &str) {
        self.add_u32(s.len() as u32);
        self.add_bytes(s.as_bytes());
    }
    pub fn get(&self) -> u32 {
        self.0
    }
}
