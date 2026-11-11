//! Lists maps that contain bridge point objects.
use zr_assets::{compression::decompress_if_needed, Installs, Vfs};
fn main() {
    let install = Installs::detect().zero_hour.unwrap();
    let vfs = Vfs::zero_hour(&install).unwrap();
    let names: Vec<String> = vfs.files().map(|(p, _)| p.to_string()).filter(|p| p.starts_with("maps\\") && p.ends_with(".map")).collect();
    for p in names {
        let Some(raw) = vfs.read(&p) else { continue };
        let Ok(data) = decompress_if_needed(&raw) else { continue };
        let Ok(map) = zr_map::MapFile::parse(&data) else { continue };
        let n = map.objects.iter().filter(|o| o.flags & 0x10 != 0).count();
        if n > 0 {
            let t: std::collections::BTreeSet<_> = map.objects.iter().filter(|o| o.flags & 0x10 != 0).map(|o| o.template.clone()).collect();
            println!("{p}: {n} bridges {t:?}");
        }
    }
}
