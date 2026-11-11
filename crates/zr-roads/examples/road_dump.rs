//! `cargo run -p zr-roads --example road_dump -- "alpine assault"`: road / bridge summary of a map.
use zr_assets::{compression::decompress_if_needed, Installs, Vfs};

fn main() -> anyhow::Result<()> {
    let name = std::env::args().nth(1).unwrap_or_else(|| "alpine assault".into()).to_ascii_lowercase();
    let install = Installs::detect().zero_hour.ok_or_else(|| anyhow::anyhow!("no Zero Hour install"))?;
    let vfs = Vfs::zero_hour(&install)?;
    let ini = zr_ini::load_zero_hour(&vfs).db;
    let raw = vfs.read(&format!(r"maps\{name}\{name}.map")).ok_or_else(|| anyhow::anyhow!("map not found"))?;
    let map = zr_map::MapFile::parse(&decompress_if_needed(&raw)?).map_err(|e| anyhow::anyhow!("{e}"))?;
    for o in map.objects.iter().filter(|o| o.flags & zr_roads::SEGMENT_POINT_FLAGS != 0) {
        println!("{:<24} flags {:#06x} at ({:.1}, {:.1})", o.template, o.flags, o.pos[0], o.pos[1]);
    }
    let hm = map.height_map.as_ref().unwrap();
    let g = |x: f32, y: f32| {
        let b = hm.border_size as f32;
        let (ix, iy) = (((x / 10.0) + b) as i32, ((y / 10.0) + b) as i32);
        f32::from(hm.data[(iy * hm.width + ix) as usize]) * 10.0 / 16.0
    };
    let roads = zr_roads::build_roads(&map, &vfs, &ini)?;
    for r in &roads {
        println!("road {} order {} verts {} tris {} tex {}x{}", r.name, r.order, r.positions.len(), r.indices.len() / 3, r.texture.width, r.texture.height);
    }
    for b in zr_roads::build_bridges(&map, &vfs, &ini, &g)? {
        println!("bridge {} {:?} -> {:?}: {} pieces", b.template, b.from, b.to, b.pieces.len());
    }
    if let Some(bn) = std::env::args().nth(2) {
        let m = zr_w3d::load_model(&vfs, &bn)?;
        for me in &m.meshes {
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for p in &me.positions {
                for k in 0..3 {
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
            }
            println!("mesh {:?} tex {:?} hidden {} bounds {lo:?} {hi:?}", me.name, me.texture, me.hidden);
        }
    }
    Ok(())
}
