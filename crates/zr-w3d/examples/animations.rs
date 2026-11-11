//! Lists the condition states of two templates, decodes every referenced
//! animation (with a pose sanity-check), then decodes every animation file in
//! the install and reports failures.
//!
//! Usage: `cargo run --release -p zr-w3d --example animations`

use std::collections::BTreeSet;
use zr_assets::{Installs, Vfs};
use zr_w3d::{load_animation, load_hierarchy, parse_animation, states_for_template, Animation};

fn main() {
    let install = Installs::detect().zero_hour.expect("Zero Hour install not found");
    let vfs = Vfs::zero_hour(&install).expect("mounting Zero Hour");
    let ini = zr_ini::load_zero_hour(&vfs).db;

    let mut referenced: BTreeSet<String> = BTreeSet::new();
    for template in ["AmericaInfantryRanger", "AmericaTankCrusader"] {
        let Some(states) = states_for_template(&ini, template) else {
            println!("{template}: no W3D draw module");
            continue;
        };
        println!("\n=== {template} ({} states, module {})", states.states.len(), states.module);
        if let Some(d) = states.default_state() {
            println!("  default model: {:?}", d.model);
        }
        for (i, st) in states.states.iter().enumerate() {
            let cond: Vec<String> = st.conditions.iter().map(|c| {
                let n: Vec<&str> = c.names().collect();
                if n.is_empty() { "NONE".to_string() } else { n.join("|") }
            }).collect();
            let mode = format!("{:?}", st.animation_mode);
            let mut seen = BTreeSet::new();
            let anims: Vec<&str> = st.animations.iter().filter_map(|a| seen.insert(a.name.as_str()).then_some(a.name.as_str())).collect();
            let hide: Vec<String> = st.hide_show.iter().map(|(n, h)| format!("{n}:{}", if *h { "hide" } else { "show" })).collect();
            println!(
                "  [{i}] {}{} model={:?} mode={mode} anims={anims:?}{}{}",
                cond.join(" / "),
                if Some(i) == states.default_state { " (default)" } else { "" },
                st.model,
                if hide.is_empty() { String::new() } else { format!(" hide/show={hide:?}") },
                if st.flags.is_empty() { String::new() } else { format!(" flags={:?}", st.flags) },
            );
            referenced.extend(st.animations.iter().map(|a| a.name.clone()));
        }
        println!("  transitions: {}", states.transitions.len());
    }

    println!("\n=== referenced animations");
    let mut decode_fail = 0;
    for name in &referenced {
        match load_animation(&vfs, name) {
            Ok(a) => {
                println!(
                    "  {name}: {} frames @ {} fps, flavor {:?}, {} pivots",
                    a.frame_count, a.frame_rate, a.flavor, a.pivot_count()
                );
                pose_check(&vfs, &a);
            }
            Err(e) => {
                decode_fail += 1;
                println!("  DECODE FAIL {name}: {e}");
            }
        }
    }
    println!("referenced: {} ({} failed)", referenced.len(), decode_fail);

    scan_all(&vfs);
}

/// Loads the animation's skeleton and samples a couple of frames to make sure
/// the pose evaluator produces finite transforms.
fn pose_check(vfs: &Vfs, a: &Animation) {
    let Ok(h) = load_hierarchy(vfs, &a.hierarchy) else {
        println!("    (no hierarchy for {})", a.hierarchy);
        return;
    };
    if h.pivot_count() < 2 {
        return;
    }
    let frames = [0.0, a.frame_count as f32 * 0.5];
    let mut poses = Vec::new();
    for f in frames {
        poses.push(h.pose(a, f));
    }
    let mut finite = true;
    for pose in &poses {
        for m in pose {
            for row in m {
                for v in row {
                    finite &= v.is_finite();
                }
            }
        }
    }
    let (mut max_delta, mut max_pivot) = (0.0f32, 0usize);
    for (i, (p0, p1)) in poses[0].iter().zip(poses[1].iter()).enumerate().skip(1) {
        let a = zr_w3d::xform_point(p0, [0.0, 0.0, 0.0]);
        let b = zr_w3d::xform_point(p1, [0.0, 0.0, 0.0]);
        let d = ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt();
        if d > max_delta {
            max_delta = d;
            max_pivot = i;
        }
    }
    println!(
        "    pose {} pivots, max joint movement frame0..mid = {:.4} at bone {max_pivot} finite={finite}",
        h.pivot_count(),
        max_delta
    );
}

fn scan_all(vfs: &Vfs) {
    println!("\n=== all art\\w3d animation files");
    let mut files: Vec<String> = vfs
        .files()
        .filter_map(|(p, _)| {
            let r = p.strip_prefix("art\\w3d\\")?;
            (!r.contains('\\') && r.ends_with(".w3d")).then(|| p.to_string())
        })
        .collect();
    files.sort();
    let (mut total, mut ok, mut fail) = (0usize, 0usize, 0usize);
    let mut frames = 0u64;
    let mut per_flavor = [0usize; 3];
    for path in &files {
        let Some(bytes) = vfs.read(path) else { continue };
        // A file is an animation when a top-level chunk is ANIMATION or
        // COMPRESSED_ANIMATION (some files carry the hierarchy first).
        let is_anim = match cnc_formats::w3d::W3dFile::parse(&bytes) {
            Ok(w) => w.chunks.iter().any(|c| c.chunk_type == 0x0200 || c.chunk_type == 0x0280),
            Err(_) => {
                let head = if bytes.len() >= 4 {
                    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
                } else {
                    0
                };
                head == 0x0200 || head == 0x0280
            }
        };
        if !is_anim {
            continue;
        }
        total += 1;
        match parse_animation(&bytes, path) {
            Ok(a) => {
                ok += 1;
                frames += a.frame_count as u64;
                per_flavor[a.flavor as usize] += 1;
            }
            Err(e) => {
                fail += 1;
                if fail <= 10 {
                    println!("  FAIL {path}: {e}");
                }
            }
        }
    }
    println!(
        "animation files: {total}, decoded: {ok}, failed: {fail} (raw {}, timecoded {}, adaptive {})",
        per_flavor[0], per_flavor[1], per_flavor[2]
    );
    println!("total decoded frames: {frames}");
}
