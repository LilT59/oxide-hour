//! Minimal Targa reader matching what the terrain loader accepts.
//!
//! Ported from `WorldHeightMap::countTiles` / `readTiles`
//! (`Core/GameEngineDevice/Source/W3DDevice/GameClient/WorldHeightMap.cpp`): only
//! uncompressed (type 2) or RLE (type 10) true-colour images with 24 or 32 bits per pixel.
//! Like the engine, the origin flag (bit 5 of the descriptor) is ignored: row 0 of the
//! returned image is the first row stored in the file.

/// Largest accepted image edge; terrain textures are at most 10 tiles (640 px).
const MAX_EDGE: usize = 4096;

#[derive(Debug, Clone)]
pub struct Tga {
    pub width: usize,
    pub height: usize,
    /// RGBA, file row order (row 0 = first stored row).
    pub pixels: Vec<[u8; 4]>,
}

/// Decodes `data`; `None` for anything the engine would reject or that is malformed.
/// Truncated pixel data is padded with zeros (the engine reads past EOF as zeros).
pub fn decode(data: &[u8]) -> Option<Tga> {
    let hdr = data.get(..18)?;
    let (id_len, cmap_type, image_type) = (hdr[0] as usize, hdr[1], hdr[2]);
    let width = u16::from_le_bytes([hdr[12], hdr[13]]) as usize;
    let height = u16::from_le_bytes([hdr[14], hdr[15]]) as usize;
    let depth = hdr[16];
    if cmap_type != 0 || !(image_type == 2 || image_type == 10) || !(24..=32).contains(&depth) {
        return None;
    }
    if width == 0 || height == 0 || width > MAX_EDGE || height > MAX_EDGE {
        return None;
    }
    let bpp = (depth as usize).div_ceil(8);
    if !(3..=4).contains(&bpp) {
        return None;
    }
    let body = data.get(18 + id_len..).unwrap_or(&[]);
    let total = width * height;
    let mut pixels = vec![[0, 0, 0, 0]; total];
    let px = |b: &[u8]| [b[2], b[1], b[0], if bpp == 4 { b[3] } else { 255 }];
    let mut pos = 0;
    if image_type == 2 {
        for p in pixels.iter_mut() {
            match body.get(pos..pos + bpp) {
                Some(b) => *p = px(b),
                None => break,
            }
            pos += bpp;
        }
    } else {
        let mut i = 0;
        while i < total {
            let Some(&flag) = body.get(pos) else { break };
            pos += 1;
            let n = (flag & 0x7f) as usize + 1;
            if flag & 0x80 != 0 {
                let Some(b) = body.get(pos..pos + bpp) else { break };
                pos += bpp;
                let v = px(b);
                for p in pixels.iter_mut().skip(i).take(n) {
                    *p = v;
                }
            } else {
                for k in 0..n {
                    let Some(b) = body.get(pos..pos + bpp) else { return Some(Tga { width, height, pixels }) };
                    pos += bpp;
                    if let Some(p) = pixels.get_mut(i + k) {
                        *p = px(b);
                    }
                }
            }
            i += n;
        }
    }
    Some(Tga { width, height, pixels })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(ty: u8, w: u16, h: u16, depth: u8) -> Vec<u8> {
        let mut v = vec![0, 0, ty, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        v.extend(w.to_le_bytes());
        v.extend(h.to_le_bytes());
        v.extend([depth, 0]);
        v
    }

    #[test]
    fn raw_and_rle() {
        let mut raw = header(2, 2, 1, 24);
        raw.extend([1, 2, 3, 4, 5, 6]); // BGR
        let t = decode(&raw).unwrap();
        assert_eq!(t.pixels, vec![[3, 2, 1, 255], [6, 5, 4, 255]]);

        let mut rle = header(10, 3, 1, 32);
        rle.extend([0x81, 10, 20, 30, 40]); // run of 2
        rle.extend([0x00, 1, 2, 3, 4]); // 1 literal
        let t = decode(&rle).unwrap();
        assert_eq!(t.pixels, vec![[30, 20, 10, 40], [30, 20, 10, 40], [3, 2, 1, 4]]);
    }

    #[test]
    fn rejects_and_survives_garbage() {
        assert!(decode(&[]).is_none());
        assert!(decode(&header(1, 2, 2, 8)).is_none());
        assert!(decode(&header(2, 0, 2, 24)).is_none());
        assert!(decode(&header(2, 60000, 60000, 24)).is_none());
        // truncated body: padded, no panic
        let mut t = header(10, 4, 4, 24);
        t.extend([0x7f, 1, 2]);
        assert!(decode(&t).is_some());
        for n in 0..40 {
            let mut d = header(10, 3, 3, 24);
            d.extend([0x82, 1, 2, 3, 0x01, 1, 2, 3, 4, 5, 6]);
            d.truncate(n);
            let _ = decode(&d);
        }
    }
}
