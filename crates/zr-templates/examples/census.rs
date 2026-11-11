//! Module-coverage census: every `ModuleData::Unparsed` instance grouped by module class, split
//! into logic vs client-draw/client-update by its `MODULEINTERFACE_*` mask.
//!
//! Usage: `cargo run --release -p zr-templates --example census`

use std::collections::BTreeMap;

use zr_assets::{Installs, Vfs};
use zr_templates::{iface, ModuleData, Templates};

fn main() {
    let Some(install) = Installs::detect().zero_hour else {
        eprintln!("Zero Hour install not found (set ZR_ZEROHOUR_DIR)");
        return;
    };
    let vfs = Vfs::zero_hour(&install).expect("mounting Zero Hour");
    let loaded = zr_ini::load_zero_hour(&vfs);
    let (t, warnings) = Templates::from_ini(&loaded.db);

    let total: usize = t.things().iter().map(|x| x.modules.len()).sum();
    let mut by_type: BTreeMap<&str, (usize, usize, usize)> = BTreeMap::new();
    for m in t.things().iter().flat_map(|x| &x.modules) {
        if let ModuleData::Unparsed { module_type, .. } = &m.data {
            let draw = m.interface_mask & (iface::DRAW | iface::CLIENT_UPDATE) != 0
                && m.interface_mask & !(iface::DRAW | iface::CLIENT_UPDATE) == 0;
            let e = by_type.entry(module_type.as_str()).or_default();
            if draw {
                e.1 += 1;
            } else {
                e.0 += 1;
            }
            e.2 += 1;
        }
    }
    let unparsed: usize = by_type.values().map(|v| v.2).sum();
    let logic: usize = by_type.values().map(|v| v.0).sum();
    let client: usize = by_type.values().map(|v| v.1).sum();
    println!(
        "{total} module instances, {} typed, {unparsed} unparsed ({logic} logic, {client} client-draw/update), {} distinct unparsed types, {} warnings",
        total - unparsed,
        by_type.len(),
        warnings.len()
    );
    let mut top: Vec<(&str, (usize, usize, usize))> = by_type.into_iter().collect();
    top.sort_by(|a, b| b.1.2.cmp(&a.1.2).then_with(|| a.0.cmp(b.0)));
    println!("\n  total  logic  client  module");
    for (ty, (lo, cl, n)) in top {
        println!("  {n:>5}  {lo:>5}  {cl:>6}  {ty}");
    }
}
