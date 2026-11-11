//! Minimal TGA decoder (types 1, 2, 3, 9, 10, 11; 8/15/16/24/32 bpp).

use crate::error::{invalid, Error};
use zr_assets::RgbaImage;

const MAX_DIM: usize = 8192;

pub fn decode(d: &[u8]) -> Result<RgbaImage, Error> {
    if d.len() < 18 {
        return Err(invalid("tga: truncated header"));
    }
    let id_len = d[0] as usize;
    let cmap_type = d[1];
    let img_type = d[2];
    let cmap_first = u16::from_le_bytes([d[3], d[4]]) as usize;
    let cmap_len = u16::from_le_bytes([d[5], d[6]]) as usize;
    let cmap_bits = d[7] as usize;
    let w = u16::from_le_bytes([d[12], d[13]]) as usize;
    let h = u16::from_le_bytes([d[14], d[15]]) as usize;
    let bpp = d[16] as usize;
    let desc = d[17];
    if w == 0 || h == 0 || w > MAX_DIM || h > MAX_DIM {
        return Err(invalid("tga: bad dimensions"));
    }
    let rle = matches!(img_type, 9..=11);
    let kind = if rle { img_type - 8 } else { img_type };
    if !matches!(kind, 1..=3) || !matches!(bpp, 8 | 15 | 16 | 24 | 32) {
        return Err(invalid(format!("tga: unsupported type {img_type} / {bpp} bpp")));
    }
    let mut pos = 18 + id_len;
    // Colour map.
    let mut cmap: Vec<[u8; 4]> = Vec::new();
    if cmap_type == 1 {
        let cb = cmap_bits.div_ceil(8);
        let total = cmap_len.checked_mul(cb).ok_or_else(|| invalid("tga: cmap"))?;
        let data = d.get(pos..pos + total).ok_or_else(|| invalid("tga: truncated cmap"))?;
        for i in 0..cmap_len {
            cmap.push(pixel(&data[i * cb..(i + 1) * cb], cmap_bits));
        }
        pos += total;
    }
    if kind == 1 && cmap.is_empty() {
        return Err(invalid("tga: missing colour map"));
    }
    let bytes_pp = bpp.div_ceil(8);
    let npix = w * h;
    let mut raw: Vec<u8> = Vec::with_capacity(npix * bytes_pp);
    let body = d.get(pos..).unwrap_or(&[]);
    if rle {
        let mut i = 0;
        while raw.len() < npix * bytes_pp {
            let hdr = *body.get(i).ok_or_else(|| invalid("tga: truncated rle"))?;
            i += 1;
            let n = (hdr & 0x7F) as usize + 1;
            if hdr & 0x80 != 0 {
                let px = body.get(i..i + bytes_pp).ok_or_else(|| invalid("tga: truncated rle"))?;
                i += bytes_pp;
                for _ in 0..n {
                    raw.extend_from_slice(px);
                }
            } else {
                let px = body.get(i..i + n * bytes_pp).ok_or_else(|| invalid("tga: truncated rle"))?;
                i += n * bytes_pp;
                raw.extend_from_slice(px);
            }
        }
        raw.truncate(npix * bytes_pp);
    } else {
        raw.extend_from_slice(body.get(..npix * bytes_pp).ok_or_else(|| invalid("tga: truncated pixels"))?);
    }
    let mut img = RgbaImage::new(w as u32, h as u32);
    let top_first = desc & 0x20 != 0;
    let right_first = desc & 0x10 != 0;
    for y in 0..h {
        let dy = if top_first { y } else { h - 1 - y };
        for x in 0..w {
            let dx = if right_first { w - 1 - x } else { x };
            let s = &raw[(y * w + x) * bytes_pp..][..bytes_pp];
            let px = match kind {
                1 => {
                    let idx = s[0] as usize;
                    cmap.get(idx.wrapping_sub(cmap_first)).copied().unwrap_or([0, 0, 0, 255])
                }
                3 => match bytes_pp {
                    1 => [s[0], s[0], s[0], 255],
                    _ => [s[0], s[0], s[0], s[1]],
                },
                _ => pixel(s, bpp),
            };
            img.set_pixel(dx as u32, dy as u32, px);
        }
    }
    Ok(img)
}

fn pixel(s: &[u8], bits: usize) -> [u8; 4] {
    match bits {
        32 => [s[2], s[1], s[0], s[3]],
        24 => [s[2], s[1], s[0], 255],
        15 | 16 => {
            let v = u16::from_le_bytes([s[0], s[1]]);
            let e = |c: u16| ((c as u32 * 255 + 15) / 31) as u8;
            let a = if bits == 16 && v & 0x8000 == 0 { 0 } else { 255 };
            [e((v >> 10) & 31), e((v >> 5) & 31), e(v & 31), a]
        }
        _ => [s[0], s[0], s[0], 255],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uncompressed_bottom_up_bgr() {
        // 2x2, 24bpp, bottom-left origin: rows given bottom first.
        let mut d = vec![0u8; 18];
        d[2] = 2;
        d[12] = 2;
        d[14] = 2;
        d[16] = 24;
        d.extend_from_slice(&[1, 2, 3, 4, 5, 6]); // bottom row (BGR)
        d.extend_from_slice(&[7, 8, 9, 10, 11, 12]); // top row
        let img = decode(&d).unwrap();
        assert_eq!(img.pixel(0, 0), [9, 8, 7, 255]);
        assert_eq!(img.pixel(1, 1), [6, 5, 4, 255]);
    }

    #[test]
    fn rle_and_truncation() {
        let mut d = vec![0u8; 18];
        d[2] = 10;
        d[12] = 4;
        d[14] = 1;
        d[16] = 32;
        d[17] = 0x28;
        d.extend_from_slice(&[0x83, 1, 2, 3, 4]);
        let img = decode(&d).unwrap();
        assert_eq!(img.pixel(3, 0), [3, 2, 1, 4]);
        assert!(decode(&d[..20]).is_err());
        assert!(decode(&[0; 5]).is_err());
    }
}
