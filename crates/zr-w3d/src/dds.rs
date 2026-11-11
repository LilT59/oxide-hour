//! DDS decoder: DXT1/3/5 (BC1/2/3) and uncompressed formats, top level plus the authored mip chain.
//!
//! Layout per DDSURFACEDESC2 as read by the engine's `ddsfile.cpp`.

use crate::error::{invalid, Error};
use zr_assets::RgbaImage;

const MAX_DIM: usize = 8192;

fn u32_at(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

/// Decodes the top level and the file's authored mip chain (`RgbaImage::mips`). The engine loads a
/// compressed texture's own mip levels (`textureloader.cpp` `Load_Compressed_Texture`: one surface per
/// `Get_Mip_Level_Count`), so the viewer must not regenerate them. A truncated or unsupported lower level
/// ends the chain instead of failing the texture.
pub fn decode(d: &[u8]) -> Result<RgbaImage, Error> {
    let mut top = decode_top(d)?;
    let pf_flags = u32_at(d, 80);
    let fourcc = &d[84..88];
    let bits = u32_at(d, 88);
    // Bytes of one level: DXT blocks are 4x4 (`div_ceil`), uncompressed levels are packed rows.
    let level_bytes = |w: usize, h: usize| -> Option<usize> {
        if pf_flags & 0x4 != 0 {
            let block = match fourcc {
                b"DXT1" => 8,
                b"DXT2" | b"DXT3" | b"DXT4" | b"DXT5" => 16,
                _ => return None,
            };
            Some(w.div_ceil(4) * h.div_ceil(4) * block)
        } else {
            let bytes = (bits as usize).div_ceil(8);
            (1..=4).contains(&bytes).then_some(w * h * bytes)
        }
    };
    let count = if u32_at(d, 8) & 0x20000 != 0 { u32_at(d, 28) as usize } else { 1 };
    let full = 1 + (top.width.max(top.height) as usize).ilog2() as usize;
    let mut mips = Vec::new();
    let (mut w, mut h) = (top.width as usize, top.height as usize);
    let Some(first) = level_bytes(w, h) else { return Ok(top) };
    let mut off = 128 + first;
    for _ in 1..count.min(full) {
        (w, h) = ((w / 2).max(1), (h / 2).max(1));
        let Some(n) = level_bytes(w, h) else { break };
        let Some(src) = d.get(off..off + n) else { break };
        // A one-level DDS with this level's size in its header, decoded by the top-level path.
        let mut hdr = d[..128].to_vec();
        hdr[12..16].copy_from_slice(&(h as u32).to_le_bytes());
        hdr[16..20].copy_from_slice(&(w as u32).to_le_bytes());
        hdr.extend_from_slice(src);
        match decode_top(&hdr) {
            Ok(m) => mips.push(m),
            Err(_) => break,
        }
        off += n;
    }
    // `DDSFileClass::DDSFileClass` (ddsfile.cpp:98-109): the engine keeps `MipLevels - 2` levels (one level
    // when there are two or fewer) and drops the two lowest. A 1x1 or 2x2 level therefore never reaches the
    // sampler, so a far alpha-tested surface samples its 8x8 level, not a fully opaque 1x1.
    let kept = if count > 2 { count - 2 } else { 1 };
    mips.truncate(kept - 1);
    top.mips = mips;
    Ok(top)
}

fn decode_top(d: &[u8]) -> Result<RgbaImage, Error> {
    if d.len() < 128 || &d[0..4] != b"DDS " {
        return Err(invalid("dds: bad header"));
    }
    let h = u32_at(d, 12) as usize;
    let w = u32_at(d, 16) as usize;
    if w == 0 || h == 0 || w > MAX_DIM || h > MAX_DIM {
        return Err(invalid("dds: bad dimensions"));
    }
    let pf_flags = u32_at(d, 80);
    let fourcc = &d[84..88];
    let bits = u32_at(d, 88);
    let masks = [u32_at(d, 92), u32_at(d, 96), u32_at(d, 100), u32_at(d, 104)];
    let data = &d[128..];
    let mut img = RgbaImage::new(w as u32, h as u32);

    if pf_flags & 0x4 != 0 {
        let (block_bytes, kind) = match fourcc {
            b"DXT1" => (8, 1),
            b"DXT2" | b"DXT3" => (16, 3),
            b"DXT4" | b"DXT5" => (16, 5),
            _ => return Err(invalid(format!("dds: unsupported fourcc {:?}", String::from_utf8_lossy(fourcc)))),
        };
        let bw = w.div_ceil(4);
        let bh = h.div_ceil(4);
        let need = bw * bh * block_bytes;
        let data = data.get(..need).ok_or_else(|| invalid("dds: truncated data"))?;
        for by in 0..bh {
            for bx in 0..bw {
                let b = &data[(by * bw + bx) * block_bytes..][..block_bytes];
                let mut px = [[0u8; 4]; 16];
                match kind {
                    1 => bc1(&b[0..8], &mut px, true),
                    3 => {
                        bc1(&b[8..16], &mut px, false);
                        for (i, p) in px.iter_mut().enumerate() {
                            let n = (b[i / 2] >> ((i & 1) * 4)) & 0xF;
                            p[3] = n * 17;
                        }
                    }
                    _ => {
                        bc1(&b[8..16], &mut px, false);
                        bc4(&b[0..8], &mut px);
                    }
                }
                for (i, p) in px.iter().enumerate() {
                    let (x, y) = (bx * 4 + (i & 3), by * 4 + (i >> 2));
                    if x < w && y < h {
                        img.set_pixel(x as u32, y as u32, *p);
                    }
                }
            }
        }
        return Ok(img);
    }

    // Uncompressed: DDPF_RGB (0x40), optional alpha (0x1 / 0x2), luminance (0x20000).
    let bytes = (bits as usize).div_ceil(8);
    if !(1..=4).contains(&bytes) {
        return Err(invalid(format!("dds: unsupported bit count {bits}")));
    }
    let data = data.get(..w * h * bytes).ok_or_else(|| invalid("dds: truncated data"))?;
    let luminance = pf_flags & 0x20000 != 0;
    let has_alpha = pf_flags & 0x3 != 0 && masks[3] != 0;
    let chan = |v: u32, m: u32| -> u8 {
        if m == 0 {
            return 255;
        }
        let shift = m.trailing_zeros();
        let max = m >> shift;
        (((v & m) >> shift) as u64 * 255 / max as u64) as u8
    };
    for i in 0..w * h {
        let mut v = 0u32;
        for k in 0..bytes {
            v |= (data[i * bytes + k] as u32) << (8 * k);
        }
        let a = if has_alpha { chan(v, masks[3]) } else { 255 };
        let px = if luminance {
            let l = chan(v, masks[0]);
            [l, l, l, a]
        } else {
            [chan(v, masks[0]), chan(v, masks[1]), chan(v, masks[2]), a]
        };
        img.set_pixel((i % w) as u32, (i / w) as u32, px);
    }
    Ok(img)
}

fn rgb565(c: u16) -> [u8; 3] {
    let r = ((c >> 11) & 31) as u32;
    let g = ((c >> 5) & 63) as u32;
    let b = (c & 31) as u32;
    [((r * 255 + 15) / 31) as u8, ((g * 255 + 31) / 63) as u8, ((b * 255 + 15) / 31) as u8]
}

/// BC1 colour block; `punch` enables the 1-bit alpha mode when c0 <= c1.
fn bc1(b: &[u8], out: &mut [[u8; 4]; 16], punch: bool) {
    let c0 = u16::from_le_bytes([b[0], b[1]]);
    let c1 = u16::from_le_bytes([b[2], b[3]]);
    let (p0, p1) = (rgb565(c0), rgb565(c1));
    let mix = |a: u8, b: u8, wa: u32, wb: u32, div: u32| ((a as u32 * wa + b as u32 * wb) / div) as u8;
    let mut pal = [[0u8; 4]; 4];
    pal[0] = [p0[0], p0[1], p0[2], 255];
    pal[1] = [p1[0], p1[1], p1[2], 255];
    if c0 > c1 || !punch {
        for k in 0..3 {
            pal[2][k] = mix(p0[k], p1[k], 2, 1, 3);
            pal[3][k] = mix(p0[k], p1[k], 1, 2, 3);
        }
        pal[2][3] = 255;
        pal[3][3] = 255;
    } else {
        for k in 0..3 {
            pal[2][k] = mix(p0[k], p1[k], 1, 1, 2);
        }
        pal[2][3] = 255;
        pal[3] = [0, 0, 0, 0];
    }
    let bits = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
    for (i, o) in out.iter_mut().enumerate() {
        *o = pal[((bits >> (2 * i)) & 3) as usize];
    }
}

/// BC3 alpha block into `out[i][3]`.
fn bc4(b: &[u8], out: &mut [[u8; 4]; 16]) {
    let (a0, a1) = (b[0] as u32, b[1] as u32);
    let mut pal = [0u8; 8];
    pal[0] = a0 as u8;
    pal[1] = a1 as u8;
    if a0 > a1 {
        for i in 1..7 {
            pal[i + 1] = (((7 - i as u32) * a0 + i as u32 * a1) / 7) as u8;
        }
    } else {
        for i in 1..5 {
            pal[i + 1] = (((5 - i as u32) * a0 + i as u32 * a1) / 5) as u8;
        }
        pal[6] = 0;
        pal[7] = 255;
    }
    let mut bits = 0u64;
    for (k, byte) in b[2..8].iter().enumerate() {
        bits |= (*byte as u64) << (8 * k);
    }
    for (i, o) in out.iter_mut().enumerate() {
        o[3] = pal[((bits >> (3 * i)) & 7) as usize];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(w: u32, h: u32, flags: u32, fourcc: &[u8; 4], bits: u32, masks: [u32; 4]) -> Vec<u8> {
        let mut d = vec![0u8; 128];
        d[0..4].copy_from_slice(b"DDS ");
        d[4..8].copy_from_slice(&124u32.to_le_bytes());
        d[12..16].copy_from_slice(&h.to_le_bytes());
        d[16..20].copy_from_slice(&w.to_le_bytes());
        d[76..80].copy_from_slice(&32u32.to_le_bytes());
        d[80..84].copy_from_slice(&flags.to_le_bytes());
        d[84..88].copy_from_slice(fourcc);
        d[88..92].copy_from_slice(&bits.to_le_bytes());
        for (i, m) in masks.iter().enumerate() {
            d[92 + i * 4..96 + i * 4].copy_from_slice(&m.to_le_bytes());
        }
        d
    }

    #[test]
    fn dxt1_solid_red() {
        let mut d = header(4, 4, 4, b"DXT1", 0, [0; 4]);
        d.extend_from_slice(&[0x00, 0xF8, 0x00, 0xF8, 0, 0, 0, 0]);
        let img = decode(&d).unwrap();
        assert_eq!(img.pixel(2, 3), [255, 0, 0, 255]);
    }

    #[test]
    fn dxt5_alpha() {
        let mut d = header(4, 4, 4, b"DXT5", 0, [0; 4]);
        // alpha endpoints 255/0, all indices 1 -> 0
        d.extend_from_slice(&[255, 0, 0x49, 0x92, 0x24, 0x49, 0x92, 0x24]);
        d.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0]);
        let img = decode(&d).unwrap();
        assert_eq!(img.pixel(0, 0), [255, 255, 255, 0]);
    }

    /// An 8x8 DXT1 file declaring `count` levels: one red 8-byte block per level's 4x4 region.
    fn chain(count: u32) -> Vec<u8> {
        let red = [0x00, 0xF8, 0x00, 0xF8, 0, 0, 0, 0];
        let mut d = header(8, 8, 4, b"DXT1", 0, [0; 4]);
        d[8..12].copy_from_slice(&0x20007u32.to_le_bytes()); // DDSD_MIPMAPCOUNT set
        d[28..32].copy_from_slice(&count.to_le_bytes());
        for _ in 0..4 + 1 + 1 + 1 {
            d.extend_from_slice(&red);
        }
        d
    }

    #[test]
    fn authored_mip_chain_is_kept() {
        // 8x8 DXT1 with 4 authored levels (8, 4, 2, 1). The engine keeps two of them (`MipLevels - 2`,
        // ddsfile.cpp:107-109), so the top level and the 4x4 level remain.
        let d = chain(4);
        let img = decode(&d).unwrap();
        assert_eq!(img.mips.len(), 1);
        assert_eq!((img.mips[0].width, img.mips[0].height), (4, 4));
        assert_eq!(img.mips[0].pixel(0, 0), [255, 0, 0, 255]);
        // A chain cut short by the file's end still gets the drop: count 5 keeps three levels (two mips), and with
        // the 1x1 block cut off the 4x4 and 2x2 levels that remain are the same two mips.
        let full5 = chain(5);
        assert_eq!(decode(&full5).unwrap().mips.len(), 2, "count 5 keeps three levels");
        let short5 = &full5[..full5.len() - 8];
        assert_eq!(decode(short5).unwrap().mips.len(), 2, "cut short: the drop still applies");
        // Without the MIPMAPCOUNT flag the header's count is ignored.
        let mut one = d.clone();
        one[8..12].copy_from_slice(&0x1007u32.to_le_bytes());
        assert!(decode(&one).unwrap().mips.is_empty());
    }

    #[test]
    fn the_two_lowest_levels_are_dropped() {
        // `MipLevels > 2` keeps `MipLevels - 2`; two or fewer keep one (ddsfile.cpp:107-109).
        assert_eq!(decode(&chain(4)).unwrap().mips.len(), 1);
        assert_eq!(decode(&chain(3)).unwrap().mips.len(), 0);
        assert_eq!(decode(&chain(2)).unwrap().mips.len(), 0);
        // The top level is always the file's top, whatever the count.
        let top = decode(&chain(4)).unwrap();
        assert_eq!((top.width, top.height), (8, 8));
    }

    /// A DXT1 file of `w x h` declaring `count` levels, with one red block per level's 4x4 region (`blocks` blocks).
    fn sized_chain(w: u32, h: u32, count: u32, blocks: usize) -> Vec<u8> {
        let red = [0x00, 0xF8, 0x00, 0xF8, 0, 0, 0, 0];
        let mut d = header(w, h, 4, b"DXT1", 0, [0; 4]);
        d[8..12].copy_from_slice(&0x20007u32.to_le_bytes());
        d[28..32].copy_from_slice(&count.to_le_bytes());
        for _ in 0..blocks {
            d.extend_from_slice(&red);
        }
        d
    }

    #[test]
    fn non_square_and_large_chains_drop_the_two_lowest() {
        // 8x4 with four levels (8x4, 4x2, 2x1, 1x1): 2 + 1 + 1 + 1 blocks. Two levels are kept: one mip (4x2).
        let img = decode(&sized_chain(8, 4, 4, 5)).unwrap();
        assert_eq!(img.mips.len(), 1);
        assert_eq!((img.mips[0].width, img.mips[0].height), (4, 2));
        // 16x16 with five real levels (16..1): 16 + 4 + 1 + 1 + 1 blocks. Count 6 keeps four levels: mips 8, 4, 2.
        let six = decode(&sized_chain(16, 16, 6, 23)).unwrap();
        assert_eq!(six.mips.len(), 3);
        assert_eq!((six.mips[2].width, six.mips[2].height), (2, 2));
        // Count 7 keeps five levels, which is all five the file has: four mips.
        assert_eq!(decode(&sized_chain(16, 16, 7, 23)).unwrap().mips.len(), 4);
    }

    #[test]
    fn rgba32_and_errors() {
        let mut d = header(1, 1, 0x41, b"\0\0\0\0", 32, [0xFF0000, 0xFF00, 0xFF, 0xFF000000]);
        d.extend_from_slice(&[10, 20, 30, 40]); // BGRA
        assert_eq!(decode(&d).unwrap().pixel(0, 0), [30, 20, 10, 40]);
        assert!(decode(&d[..100]).is_err());
        let mut t = header(4, 4, 4, b"DXT1", 0, [0; 4]);
        t.extend_from_slice(&[0; 4]);
        assert!(decode(&t).is_err());
    }
}
