//! Decode `Data\Scripts\SkirmishScripts.scb` / `MultiplayerScripts.scb` and report the condition
//! and action types they use, by invocation count (for the script-engine coverage report).
//!
//! `cargo run --release -p zr-game --example scb_stats` (needs a Zero Hour install).

use std::collections::BTreeMap;
use zr_assets::{Installs, Vfs};
use zr_map::parse_scripts_file;

fn main() {
    let installs = Installs::detect();
    let Some(zh) = &installs.zero_hour else {
        eprintln!("no Zero Hour install; set ZR_ZEROHOUR_DIR");
        return;
    };
    let vfs = Vfs::zero_hour(zh).expect("mount");
    for path in ["Data\\Scripts\\SkirmishScripts.scb", "Data\\Scripts\\MultiplayerScripts.scb"] {
        let Some(bytes) = vfs.read(path) else {
            println!("{path}: not found");
            continue;
        };
        match parse_scripts_file(&bytes) {
            Ok(f) => {
                let st = f.scripts.as_ref().map(|s| s.stats()).unwrap_or_default();
                println!(
                    "== {path}: {} bytes, {} players, {} teams, {} lists, {} groups, {} scripts, {} conds, {} acts",
                    bytes.len(),
                    f.players.len(),
                    f.teams.len(),
                    st.lists,
                    st.groups,
                    st.scripts,
                    st.conditions,
                    st.actions
                );
                let mut conds: BTreeMap<&str, usize> = BTreeMap::new();
                let mut acts: BTreeMap<&str, usize> = BTreeMap::new();
                for sc in f.all_scripts() {
                    for oc in &sc.or_conditions {
                        for c in &oc.conditions {
                            *conds.entry(c.name().unwrap_or("?")).or_default() += 1;
                        }
                    }
                    for a in sc.actions.iter().chain(&sc.actions_false) {
                        *acts.entry(a.name().unwrap_or("?")).or_default() += 1;
                    }
                }
                print_sorted("conditions", conds);
                print_sorted("actions", acts);
            }
            Err(e) => println!("{path}: parse error {e:?}"),
        }
    }
}

fn print_sorted(title: &str, map: BTreeMap<&str, usize>) {
    let mut v: Vec<_> = map.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    println!("  {title} by type:");
    for (n, c) in v {
        println!("    {c:>5}  {n}");
    }
}
