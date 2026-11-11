//! Loads every model placed on Alpine Assault with its textures and reports the
//! results, then rasterizes one model (default: AmericaTankCrusader) from a 3/4
//! view into a PNG under the system temp dir (never inside the repo).
//!
//! Usage: `cargo run --release -p zr-w3d --example alpine_models [-- Template [out.png]]`

use std::collections::{BTreeMap, BTreeSet, HashMap};
use zr_assets::{compression::decompress_if_needed, Installs, RgbaImage, Vfs};
use zr_w3d::{load_model, load_texture, model_for_template, Model};

fn main() {
    let install = Installs::detect().zero_hour.expect("Zero Hour install not found");
    let vfs = Vfs::zero_hour(&install).expect("mounting Zero Hour");
    let loaded = zr_ini::load_zero_hour(&vfs);
    let ini = &loaded.db;

    let raw = vfs.read("maps\\alpine assault\\alpine assault.map").expect("alpine assault.map");
    let data = decompress_if_needed(&raw).expect("decompress map");
    let map = zr_map::MapFile::parse(&data).expect("parse map");

    let templates: BTreeSet<&str> = map.objects.iter().map(|o| o.template.as_str()).collect();
    println!("placed objects: {}, unique templates: {}", map.objects.len(), templates.len());

    let mut no_model: Vec<&str> = Vec::new();
    let mut models: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for t in &templates {
        match model_for_template(ini, t) {
            Some(m) => models.entry(m.to_ascii_lowercase()).or_default().push(t),
            None => no_model.push(t),
        }
    }
    println!("templates resolving to a model: {} ({} unique models)", templates.len() - no_model.len(), models.len());
    println!("templates without a model: {}", no_model.len());
    for t in &no_model {
        println!("  no model: {t}");
    }

    let mut ok = 0;
    let mut failed = 0;
    let mut tex_ok = BTreeSet::new();
    let mut tex_fail: BTreeMap<String, String> = BTreeMap::new();
    let mut tex_less = 0;
    let (mut meshes, mut tris, mut hidden) = (0usize, 0usize, 0usize);
    for (m, users) in &models {
        match load_model(&vfs, m) {
            Ok(model) => {
                ok += 1;
                for mesh in &model.meshes {
                    meshes += 1;
                    tris += mesh.indices.len() / 3;
                    hidden += mesh.hidden as usize;
                    match &mesh.texture {
                        None => tex_less += 1,
                        Some(t) => {
                            if tex_ok.contains(t) || tex_fail.contains_key(t) {
                                continue;
                            }
                            match load_texture(&vfs, t) {
                                Ok(_) => {
                                    tex_ok.insert(t.clone());
                                }
                                Err(e) => {
                                    tex_fail.insert(t.clone(), e.to_string());
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => {
                failed += 1;
                println!("  MODEL FAIL {m} (used by {}): {e}", users.join(", "));
            }
        }
    }
    println!("models loaded OK: {ok}, failed: {failed}");
    println!("meshes: {meshes} ({hidden} hidden, {tex_less} untextured), triangles: {tris}");
    println!("unique textures OK: {}, failed: {}", tex_ok.len(), tex_fail.len());
    for (t, e) in &tex_fail {
        println!("  TEXTURE FAIL {t}: {e}");
    }

    // Sanity render.
    let mut args = std::env::args().skip(1);
    let template = args.next().unwrap_or_else(|| "AmericaTankCrusader".into());
    let out = args
        .next()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("zr_w3d_sanity.png"));
    let Some(mname) = model_for_template(ini, &template) else {
        println!("sanity render: {template} has no model");
        return;
    };
    let model = load_model(&vfs, &mname).expect("sanity model");
    println!("sanity render: {template} -> model {mname}, {} meshes", model.meshes.len());
    for m in &model.meshes {
        println!("  mesh {} tris={} tex={:?} blend={} hidden={}", m.name, m.indices.len() / 3, m.texture, m.alpha_blend, m.hidden);
    }
    let img = render(&vfs, &model);
    std::fs::write(&out, png(&img)).expect("write png");
    println!("wrote {}", out.display());
}

type V3 = [f32; 3];
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// Orthographic 3/4 view (Z up), z-buffered, textured, no culling.
fn render(vfs: &Vfs, model: &Model) -> RgbaImage {
    const W: usize = 640;
    const H: usize = 480;
    let az = 35f32.to_radians();
    let el = 28f32.to_radians();
    // Camera looks along `fwd` from the +X/-Y side.
    let fwd: V3 = [-(el.cos() * az.cos()), el.cos() * az.sin(), -el.sin()];
    let right = {
        let r = cross(fwd, [0.0, 0.0, 1.0]);
        let l = dot(r, r).sqrt();
        [r[0] / l, r[1] / l, r[2] / l]
    };
    let up = cross(right, fwd);
    let light: V3 = {
        let l = [-0.4, 0.5, 0.8f32];
        let n = dot(l, l).sqrt();
        [l[0] / n, l[1] / n, l[2] / n]
    };

    let mut textures: HashMap<String, Option<RgbaImage>> = HashMap::new();
    let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
    for m in model.meshes.iter().filter(|m| !m.hidden) {
        for p in &m.positions {
            for (k, ax) in [right, up].iter().enumerate() {
                lo[k] = lo[k].min(dot(*p, *ax));
                hi[k] = hi[k].max(dot(*p, *ax));
            }
        }
    }
    let scale = ((W as f32 - 40.0) / (hi[0] - lo[0]).max(1e-3)).min((H as f32 - 40.0) / (hi[1] - lo[1]).max(1e-3));
    let proj = |p: V3| -> [f32; 3] {
        [
            20.0 + (dot(p, right) - lo[0]) * scale,
            H as f32 - 20.0 - (dot(p, up) - lo[1]) * scale,
            dot(p, fwd),
        ]
    };

    let mut color = vec![[40u8, 44, 52, 255]; W * H];
    let mut zbuf = vec![f32::MAX; W * H];
    let (mut agree, mut total) = (0usize, 0usize);
    for m in model.meshes.iter().filter(|m| !m.hidden) {
        let tex = m.texture.as_ref().and_then(|t| {
            textures.entry(t.clone()).or_insert_with(|| load_texture(vfs, t).ok()).as_ref()
        });
        for t in m.indices.chunks_exact(3) {
            let ids = [t[0] as usize, t[1] as usize, t[2] as usize];
            let wp = ids.map(|i| m.positions[i]);
            // Winding check: geometric normal vs. averaged vertex normal.
            let gn = cross(sub(wp[1], wp[0]), sub(wp[2], wp[0]));
            let vn = ids.map(|i| m.normals[i]);
            let avg = [vn[0][0] + vn[1][0] + vn[2][0], vn[0][1] + vn[1][1] + vn[2][1], vn[0][2] + vn[1][2] + vn[2][2]];
            total += 1;
            agree += (dot(gn, avg) > 0.0) as usize;

            let p = wp.map(proj);
            let area = (p[1][0] - p[0][0]) * (p[2][1] - p[0][1]) - (p[2][0] - p[0][0]) * (p[1][1] - p[0][1]);
            if area.abs() < 1e-6 {
                continue;
            }
            let (minx, maxx) = (p.iter().map(|v| v[0]).fold(f32::MAX, f32::min), p.iter().map(|v| v[0]).fold(f32::MIN, f32::max));
            let (miny, maxy) = (p.iter().map(|v| v[1]).fold(f32::MAX, f32::min), p.iter().map(|v| v[1]).fold(f32::MIN, f32::max));
            for y in (miny.floor().max(0.0) as usize)..=(maxy.ceil().min(H as f32 - 1.0) as usize) {
                for x in (minx.floor().max(0.0) as usize)..=(maxx.ceil().min(W as f32 - 1.0) as usize) {
                    let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                    let w0 = ((p[1][0] - fx) * (p[2][1] - fy) - (p[2][0] - fx) * (p[1][1] - fy)) / area;
                    let w1 = ((p[2][0] - fx) * (p[0][1] - fy) - (p[0][0] - fx) * (p[2][1] - fy)) / area;
                    let w2 = 1.0 - w0 - w1;
                    if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                        continue;
                    }
                    let z = w0 * p[0][2] + w1 * p[1][2] + w2 * p[2][2];
                    if z >= zbuf[y * W + x] {
                        continue;
                    }
                    let mut rgb = [200u8, 200, 200];
                    if let Some(img) = tex {
                        let u = w0 * m.uvs[ids[0]][0] + w1 * m.uvs[ids[1]][0] + w2 * m.uvs[ids[2]][0];
                        let v = w0 * m.uvs[ids[0]][1] + w1 * m.uvs[ids[1]][1] + w2 * m.uvs[ids[2]][1];
                        let tx = (u.rem_euclid(1.0) * img.width as f32) as u32 % img.width;
                        let ty = (v.rem_euclid(1.0) * img.height as f32) as u32 % img.height;
                        let px = img.pixel(tx, ty);
                        if m.alpha_blend && px[3] < 128 {
                            continue;
                        }
                        rgb = [px[0], px[1], px[2]];
                    }
                    let n = [
                        w0 * vn[0][0] + w1 * vn[1][0] + w2 * vn[2][0],
                        w0 * vn[0][1] + w1 * vn[1][1] + w2 * vn[2][1],
                        w0 * vn[0][2] + w1 * vn[1][2] + w2 * vn[2][2],
                    ];
                    let l = dot(n, n).sqrt().max(1e-6);
                    let shade = 0.45 + 0.55 * (dot(n, light) / l).abs();
                    zbuf[y * W + x] = z;
                    color[y * W + x] = [
                        (rgb[0] as f32 * shade).min(255.0) as u8,
                        (rgb[1] as f32 * shade).min(255.0) as u8,
                        (rgb[2] as f32 * shade).min(255.0) as u8,
                        255,
                    ];
                }
            }
        }
    }
    println!("winding: {agree}/{total} triangles have CCW geometric normal agreeing with vertex normals");
    let mut img = RgbaImage::new(W as u32, H as u32);
    for (i, c) in color.iter().enumerate() {
        img.set_pixel((i % W) as u32, (i / W) as u32, *c);
    }
    img
}

/// Minimal PNG writer (stored deflate blocks) so the example needs no image crate.
fn png(img: &RgbaImage) -> Vec<u8> {
    fn crc32(data: &[u8]) -> u32 {
        let mut c = !0u32;
        for &b in data {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
        }
        !c
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        let mut c = kind.to_vec();
        c.extend_from_slice(body);
        out.extend_from_slice(&c);
        out.extend_from_slice(&crc32(&c).to_be_bytes());
    }
    let mut raw = Vec::new();
    for y in 0..img.height as usize {
        raw.push(0);
        raw.extend_from_slice(&img.pixels[y * img.width as usize * 4..(y + 1) * img.width as usize * 4]);
    }
    let mut z = vec![0x78, 0x01];
    let mut chunks = raw.chunks(65535).peekable();
    while let Some(c) = chunks.next() {
        z.push(chunks.peek().is_none() as u8);
        z.extend_from_slice(&(c.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(c.len() as u16)).to_le_bytes());
        z.extend_from_slice(c);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&img.width.to_be_bytes());
    ihdr.extend_from_slice(&img.height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}
