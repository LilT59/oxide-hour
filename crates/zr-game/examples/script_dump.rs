//! Prints the shipped `Data\Scripts\SkirmishScripts.scb` scripts whose name contains a filter
//! (case-insensitive): active / one-shot / difficulty flags, delay, the OR-groups of conditions and
//! the true / false actions, with their parameters. Read-only diagnostic for AI build-script work.
//!
//! `cargo run --release -p zr-game --example script_dump -- "Black Market"` (needs a Zero Hour install).

use zr_assets::{Installs, Vfs};
use zr_map::{parse_scripts_file, Action, Condition, ParamValue, Script};

fn params(p: &[zr_map::Parameter]) -> String {
    p.iter()
        .map(|x| match &x.value {
            ParamValue::Coord3D(c) => format!("({:.0},{:.0})", c[0], c[1]),
            ParamValue::Plain { int, real, string } => {
                if !string.is_empty() {
                    format!("'{string}'")
                } else if *real != 0.0 {
                    format!("{real}")
                } else {
                    format!("{int}")
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn cond(c: &Condition) -> String {
    format!("{}[{}]", c.name().unwrap_or("?"), params(&c.params))
}

fn act(a: &Action) -> String {
    format!("{}[{}]", a.name().unwrap_or("?"), params(&a.params))
}

fn dump(list: usize, group: Option<&str>, s: &Script) {
    println!(
        "== list {list} {}script '{}' active={} one_shot={} easy={} normal={} hard={} subroutine={} delay={}s",
        group.map_or(String::new(), |g| format!("group '{g}' ")),
        s.name,
        s.active,
        s.one_shot,
        s.easy,
        s.normal,
        s.hard,
        s.subroutine,
        s.delay_seconds
    );
    for (i, oc) in s.or_conditions.iter().enumerate() {
        let parts: Vec<String> = oc.conditions.iter().map(cond).collect();
        println!("   or[{i}]: {}", parts.join(" AND "));
    }
    for a in &s.actions {
        println!("   do:    {}", act(a));
    }
    for a in &s.actions_false {
        println!("   else:  {}", act(a));
    }
}

fn main() {
    let filter = std::env::args().nth(1).unwrap_or_default().to_ascii_lowercase();
    let installs = Installs::detect();
    let Some(zh) = &installs.zero_hour else {
        eprintln!("no Zero Hour install; set ZR_ZEROHOUR_DIR");
        return;
    };
    let vfs = Vfs::zero_hour(zh).expect("mount");
    let Some(bytes) = vfs.read("Data\\Scripts\\SkirmishScripts.scb") else {
        eprintln!("SkirmishScripts.scb not found");
        return;
    };
    let f = parse_scripts_file(&bytes).expect("parse SkirmishScripts.scb");
    let Some(ps) = f.scripts else { return };
    for (li, list) in ps.lists.iter().enumerate() {
        for s in &list.scripts {
            if s.name.to_ascii_lowercase().contains(&filter) {
                dump(li, None, s);
            }
        }
        for g in &list.groups {
            for s in &g.scripts {
                if s.name.to_ascii_lowercase().contains(&filter) {
                    dump(li, Some(&g.name), s);
                }
            }
        }
    }
}
