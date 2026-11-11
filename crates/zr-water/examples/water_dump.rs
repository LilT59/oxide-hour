//! `cargo run -p zr-water --example water_dump -- <map>`: prints the water look and mesh sizes.
use zr_assets::{compression::decompress_if_needed, Installs, Vfs};
use zr_map::MapFile;

fn main() -> anyhow::Result<()> {
    let name = std::env::args().nth(1).unwrap_or_else(|| "alpine assault".into()).to_ascii_lowercase();
    let vfs = Vfs::zero_hour(&Installs::detect().zero_hour.ok_or_else(|| anyhow::anyhow!("no install"))?)?;
    let ini = zr_ini::load_zero_hour(&vfs).db;
    let raw = vfs.read(&format!(r"maps\{name}\{name}.map")).ok_or_else(|| anyhow::anyhow!("map not found"))?;
    let map = MapFile::parse(&decompress_if_needed(&raw)?).map_err(|e| anyhow::anyhow!("{e}"))?;
    let lighting = map.lighting.clone().ok_or_else(|| anyhow::anyhow!("no lighting"))?;
    for b in ini.of_kind("WaterSet").chain(ini.of_kind("WaterTransparency")) {
        println!("{} {:?}: {:?}", b.kind, b.name(), b.fields().map(|f| format!("{}={}", f.key, f.value)).collect::<Vec<_>>());
    }
    let w = zr_water::build_water(&map, &vfs, &ini, &lighting, &|_, _| 0.0)?;
    match w {
        None => println!("no water"),
        Some(w) => {
            let l = &w.look;
            println!("look: tex {}x{} shade {:?} alpha {} depth {} minop {} additive {}", l.texture.width, l.texture.height, l.shade, l.diffuse_alpha, l.transparent_depth, l.min_opacity, l.additive);
            for m in &w.meshes {
                println!("{:?}: {} verts, {} tris", m.kind, m.positions.len(), m.indices.len() / 3);
            }
        }
    }
    Ok(())
}
