//! Builds Alpine Assault, prints stats and writes `alpine_atlas.png` and `alpine_topdown.png`
//! to a temp directory outside the repo.
//!
//! `cargo run --release -p zr-terrain --example alpine [map path in the VFS]`

use std::io::Write;
use std::time::Instant;

use anyhow::{Context, Result};
use zr_assets::{Installs, RgbaImage, Vfs};

fn main() -> Result<()> {
    let map_path = std::env::args().nth(1).unwrap_or_else(|| "maps/alpine assault/alpine assault.map".into());
    let install = Installs::detect().zero_hour.context("no Zero Hour install")?;
    let vfs = Vfs::zero_hour(&install)?;
    let ini = zr_ini::load_zero_hour(&vfs).db;
    if let Some(b) = ini.resolved("GameData", "") {
        println!("GameData Use3WayTerrainBlends {:?} AdjustCliffTextures {:?}", b.value("Use3WayTerrainBlends"), b.value("AdjustCliffTextures"));
    } else {
        println!("no GameData block");
    }
    let raw = vfs.read(&map_path).context("map not found")?;
    let data = zr_assets::compression::decompress_if_needed(&raw)?;
    let map = zr_map::MapFile::parse(&data)?;

    if let Some(bt) = &map.blend_tiles {
        let classes: Vec<_> = bt.texture_classes.iter().map(|c| (c.first_tile, c.num_tiles, c.width)).collect();
        println!("texture classes {} {:?}", bt.texture_classes.len(), classes);
    }
    let t = Instant::now();
    let mesh = zr_terrain::build_terrain(&map, &vfs, &ini)?;
    let dt = t.elapsed();
    let s = &mesh.stats;
    println!("grid {}x{}  border {}  playable {:?}..{:?}", mesh.grid.0, mesh.grid.1, mesh.border_size, mesh.playable_min, mesh.playable_max);
    println!("cells {}  vertices {}  triangles {}", s.cells, mesh.positions.len(), mesh.indices.len() / 3);
    println!(
        "texture classes {}  unplaced {}  blended cells {}  extra-blend cells {}  extra quads {}",
        s.texture_classes,
        s.unplaced_classes,
        s.blended_cells,
        s.extra_blended_cells,
        mesh.extra.as_ref().map_or(0, |e| e.indices.len() / 6)
    );
    println!("atlas {}x{}", mesh.atlas.width, mesh.atlas.height);
    println!("missing texture classes: {:?}", s.missing_classes);
    println!("build time {dt:.2?}");

    let dir = std::env::temp_dir().join("zr-terrain");
    std::fs::create_dir_all(&dir)?;
    write_png(&dir.join("alpine_atlas.png"), &mesh.atlas)?;
    let top = top_down(&mesh, 4);
    write_png(&dir.join("alpine_topdown.png"), &top)?;
    println!("wrote {}", dir.display());
    Ok(())
}

/// Orthographic top-down render through the same two-pass + extra-pass math as the viewer's
/// terrain material, lit by a fixed light.
fn top_down(m: &zr_terrain::TerrainMesh, px_per_cell: usize) -> RgbaImage {
    let (cw, ch) = (m.grid.0 as usize - 1, m.grid.1 as usize - 1);
    let mut img = RgbaImage::new((cw * px_per_cell) as u32, (ch * px_per_cell) as u32);
    let light = [-0.4f32, 0.5, 0.77];
    // The extra pass' quads, keyed by the cell they belong to (their SW vertex position).
    let mut extra_at: std::collections::HashMap<(i32, i32), usize> = std::collections::HashMap::new();
    let extra = m.extra.as_ref();
    if let Some(e) = extra {
        let border = m.border_size;
        for q in 0..e.positions.len() / 4 {
            let p = e.positions[q * 4];
            let cell = (((p[0] / 10.0) as i32) + border, ((p[1] / 10.0) as i32) + border);
            extra_at.insert(cell, q);
        }
        println!("topdown: {} extra quads, {} matched cells", e.positions.len() / 4, extra_at.len());
    }
    let weights = |s: f32, t: f32, flip: bool| -> [f32; 4] {
        if !flip {
            if t <= s { [1.0 - s, s - t, t, 0.0] } else { [1.0 - t, 0.0, s, t - s] }
        } else if s + t <= 1.0 {
            [1.0 - s - t, s, 0.0, t]
        } else {
            [0.0, 1.0 - t, s + t - 1.0, 1.0 - s]
        }
    };
    let sample = |uv: &[[f32; 2]], s: f32, t: f32| -> [u8; 4] {
        let u = uv[0][0] + (uv[1][0] - uv[0][0]) * s;
        let v = uv[0][1] + (uv[3][1] - uv[0][1]) * t;
        m.atlas.pixel((u * m.atlas.width as f32) as u32, (v * m.atlas.height as f32) as u32)
    };
    for cy in 0..ch {
        for cx in 0..cw {
            let cell = cy * cw + cx;
            let base_uv = &m.uvs[cell * 4..][..4];
            let blend_uv = &m.blend_uvs[cell * 4..][..4];
            let n = &m.normals[cell * 4..][..4];
            let flip = {
                let b = (cell * 4) as u32;
                m.indices[cell * 6 + 1] != b + 1
            };
            // The extra pass' quad replaces the two-layer sample for that cell (it is the
            // pre-composited base+blend+extra block, drawn opaque).
            let extra = extra_at.get(&(cx as i32, cy as i32)).map(|&q| {
                let e = extra.as_ref().unwrap();
                &e.uvs[q * 4..q * 4 + 4]
            });
            for py in 0..px_per_cell {
                for px in 0..px_per_cell {
                    let s = (px as f32 + 0.5) / px_per_cell as f32;
                    let t = 1.0 - (py as f32 + 0.5) / px_per_cell as f32; // row 0 = north
                    let mut c = [0u8; 3];
                    if let Some(uv) = extra {
                        let tex = sample(uv, s, t);
                        c = [tex[0], tex[1], tex[2]];
                    } else {
                        let w = weights(s, t, flip);
                        let a: f32 = (0..4).map(|k| w[k] * f32::from(m.blend_alpha[cell * 4 + k])).sum::<f32>() / 255.0;
                        let base = sample(base_uv, s, t);
                        let blend = sample(blend_uv, s, t);
                        let ba = a * f32::from(blend[3]) / 255.0;
                        for k in 0..3 {
                            c[k] = (f32::from(base[k]) * (1.0 - ba) + f32::from(blend[k]) * ba).round() as u8;
                        }
                    }
                    let nn = n[0];
                    let lam = (nn[0] * light[0] + nn[1] * light[1] + nn[2] * light[2]).max(0.0);
                    let shade = 0.45 + 0.75 * lam;
                    let (ox, oy) = (cx * px_per_cell + px, (ch - 1 - cy) * px_per_cell + py);
                    img.set_pixel(ox as u32, oy as u32, [
                        (f32::from(c[0]) * shade).min(255.0) as u8,
                        (f32::from(c[1]) * shade).min(255.0) as u8,
                        (f32::from(c[2]) * shade).min(255.0) as u8,
                        255,
                    ]);
                }
            }
        }
    }
    img
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

fn write_png(path: &std::path::Path, img: &RgbaImage) -> Result<()> {
    let mut raw = Vec::with_capacity(img.pixels.len() + img.height as usize);
    for row in img.pixels.chunks(img.width as usize * 4) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let z = miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6);
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut chunk = |kind: &[u8; 4], body: &[u8]| {
        out.extend((body.len() as u32).to_be_bytes());
        let mut c = kind.to_vec();
        c.extend(body);
        out.extend(&c);
        out.extend(crc32(&c).to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend(img.width.to_be_bytes());
    ihdr.extend(img.height.to_be_bytes());
    ihdr.extend([8, 6, 0, 0, 0]);
    chunk(b"IHDR", &ihdr);
    chunk(b"IDAT", &z);
    chunk(b"IEND", &[]);
    std::fs::File::create(path)?.write_all(&out)?;
    Ok(())
}
