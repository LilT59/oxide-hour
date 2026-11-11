//! `zr-probe summary`             — archives, file counts, extension histogram
//! `zr-probe parse`               — run every .w3d / .map through the parsers, report failures
//! `zr-probe extract <game> <prefix> <out-dir>` — dump files whose path starts with prefix
//!
//! `<game>` is `zh` or `renegade`. Install paths are auto-detected (Steam) or taken
//! from `ZR_ZEROHOUR_DIR` / `ZR_RENEGADE_DIR`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use cnc_formats::w3d::W3dFile;
use zr_map::MapFile;
use zr_assets::{Installs, Vfs};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let installs = Installs::detect();
    let mut games = Vec::new();
    if let Some(i) = &installs.zero_hour {
        games.push(("zh", Vfs::zero_hour(i).context("mounting Zero Hour")?));
    }
    if let Some(i) = &installs.renegade {
        games.push(("renegade", Vfs::renegade(i).context("mounting Renegade")?));
    }
    if games.is_empty() {
        bail!("no installs found; set ZR_ZEROHOUR_DIR / ZR_RENEGADE_DIR");
    }

    match args.first().map(String::as_str) {
        None | Some("summary") => games.iter().for_each(|(name, vfs)| summary(name, vfs)),
        Some("parse") => games.iter().for_each(|(name, vfs)| parse(name, vfs)),
        Some("extract") => {
            let [_, game, prefix, out] = &args[..] else { bail!("usage: extract <zh|renegade> <prefix> <out-dir>") };
            let (_, vfs) = games.iter().find(|(n, _)| n == game).context("unknown or missing game")?;
            extract(vfs, &prefix.to_ascii_lowercase().replace('/', "\\"), &PathBuf::from(out))?;
        }
        Some(other) => bail!("unknown command {other}"),
    }
    Ok(())
}

fn summary(game: &str, vfs: &Vfs) {
    println!("== {game}: {} archives", vfs.archives().len());
    for a in vfs.archives() {
        println!("  {:?} {:>6} entries  {}", a.kind(), a.entries().len(), a.path().display());
    }
    let mut exts: BTreeMap<String, usize> = BTreeMap::new();
    let mut total = 0;
    for (path, _) in vfs.files() {
        total += 1;
        let ext = path.rsplit_once('.').map_or("", |(_, e)| e);
        *exts.entry(ext.to_owned()).or_default() += 1;
    }
    println!("  {total} unique files");
    let mut by_count: Vec<_> = exts.into_iter().collect();
    by_count.sort_by_key(|&(_, n)| std::cmp::Reverse(n));
    for (ext, n) in by_count.iter().take(15) {
        println!("    .{ext:<6} {n}");
    }
}

fn parse(game: &str, vfs: &Vfs) {
    let mut stats: BTreeMap<&str, (usize, Vec<String>)> = BTreeMap::new();
    let mut meshes = 0usize;
    let mut map_stats: Vec<(String, MapFile)> = Vec::new();
    for (path, _) in vfs.files() {
        let Some(data) = vfs.read_archived(path) else { continue };
        let (kind, result) = if path.ends_with(".w3d") {
            ("w3d", W3dFile::parse(data).map(|f| meshes += f.meshes().len()).map_err(|e| e.to_string()))
        } else if path.ends_with(".map") && game == "zh" {
            ("map", zr_assets::compression::decompress_if_needed(data).map_err(|e| e.to_string()).and_then(|d| MapFile::parse(&d).map_err(|e| e.to_string())).map(|m| map_stats.push((path.to_owned(), m))))
        } else {
            continue;
        };
        let entry = stats.entry(kind).or_default();
        entry.0 += 1;
        if let Err(e) = result {
            entry.1.push(format!("{path}: {e}"));
        }
    }
    println!("== {game}");
    for (kind, (n, failures)) in &stats {
        println!("  .{kind}: {}/{n} parsed", n - failures.len());
        for f in failures.iter().take(10) {
            println!("    FAIL {f}");
        }
    }
    println!("  meshes found in .w3d files: {meshes}");
    if game == "zh" {
        ini_and_text_report(vfs);
    }
    if !map_stats.is_empty() {
        let objects: usize = map_stats.iter().map(|(_, m)| m.objects.len()).sum();
        println!("  maps via zr-map: {} parsed, {objects} objects total", map_stats.len());
        map_report(&map_stats);
        for (path, m) in map_stats.iter().filter(|(p, _)| p.contains("alpine") || p.contains("tournamentisland")).take(2).chain(map_stats.first()) {
            let hm = m.height_map.as_ref();
            println!("    {path}: {}x{} border {}, {} objects, {} sides, chunks: {}", hm.map_or(0, |h| h.width), hm.map_or(0, |h| h.height), hm.map_or(0, |h| h.border_size), m.objects.len(), m.sides.as_ref().map_or(0, |s| s.sides.len()), m.chunk_names().collect::<Vec<_>>().join(","));
        }
    }
}

/// INI load order, per-map `map.ini` overlay and string table (`zr-ini`, `zr-text`).
fn ini_and_text_report(vfs: &Vfs) {
    let loaded = zr_ini::load_zero_hour(vfs);
    println!(
        "  ini: {} files merged in engine order ({}), {} blocks; {} parse failures, {} unreferenced + {} legacy files",
        loaded.order.len(),
        loaded.language,
        loaded.db.len(),
        loaded.failures.len(),
        loaded.unreferenced.len(),
        loaded.legacy.len(),
    );
    audio_report(vfs, &loaded.db, &loaded.language);
    ui_report(vfs, &loaded.db, &loaded.language);
    let mut db = loaded.db;
    let map_inis: Vec<&str> = vfs.files().map(|(p, _)| p).filter(|p| p.starts_with("maps") && p.ends_with("map.ini")).collect();
    let (mut overlaid, mut failed) = (0, 0);
    for ini in &map_inis {
        let map = format!("{}.map", &ini[..ini.len() - "map.ini".len()]);
        let m = zr_ini::load_map_ini(&mut db, vfs, &map);
        overlaid += db.len() - m.checkpoint;
        failed += m.failures.len();
        db.rollback(m.checkpoint);
    }
    println!("  map.ini: {} maps with one, {overlaid} override blocks overlaid and rolled back, {failed} failures", map_inis.len());

    let lang = zr_text::detect_language(vfs);
    match zr_text::load(vfs, &lang) {
        Ok(text) => {
            println!("  text ({lang}): {} labels, csf language id {:?}", text.main.len(), text.main.language_id);
            let (mut named, mut missing) = (0, Vec::new());
            for obj in db.of_kind("Object") {
                let Some(label) = obj.field("DisplayName").and_then(|f| f.tokens().into_iter().next()) else { continue };
                named += 1;
                if text.get(label).is_none() {
                    missing.push(label.to_owned());
                }
            }
            println!("  DisplayName labels on objects: {named}, unresolved: {}{}", missing.len(), if missing.is_empty() { String::new() } else { format!(" (e.g. {:?})", &missing[..missing.len().min(3)]) });
            for name in ["AmericaTankCrusader", "ChinaTankOverlord", "AmericaInfantryRanger"] {
                let label = db.get("Object", name).and_then(|b| b.value("DisplayName")).and_then(|v| v.split_whitespace().next());
                println!("    {name}: DisplayName {label:?} -> {:?}", label.map(|l| text.fetch(l)));
            }
        }
        Err(e) => println!("  text ({lang}): {e}"),
    }
    templates_report(&db);
}

/// Typed templates (`zr-templates`): counts and the module-coverage split.
fn templates_report(db: &zr_ini::IniDb) {
    let (t, warnings) = zr_templates::Templates::from_ini(db);
    let modules = || t.things().iter().flat_map(|x| &x.modules);
    let total: usize = modules().count();
    let unparsed: usize = modules()
        .filter(|m| matches!(m.data, zr_templates::ModuleData::Unparsed { .. }))
        .count();
    let logic_unparsed: usize = modules()
        .filter(|m| {
            matches!(m.data, zr_templates::ModuleData::Unparsed { .. })
                && m.interface_mask & !(zr_templates::iface::DRAW | zr_templates::iface::CLIENT_UPDATE) != 0
        })
        .count();
    println!(
        "  templates: {} objects, {} typed / {unparsed} unparsed module instances ({} logic, {} client-draw/update), {} warnings",
        t.things().len(),
        total - unparsed,
        logic_unparsed,
        unparsed - logic_unparsed,
        warnings.len(),
    );
}

/// Audio tables (`zr-audio`): event counts, reference resolution and WAV headers.
fn audio_report(vfs: &Vfs, db: &zr_ini::IniDb, language: &str) {
    let audio = zr_audio::AudioDatabase::from_ini(db, language);
    let report = zr_audio::make_report(&audio, vfs);
    println!("  audio: {}", report.summary());
    for u in report.unresolved.iter().take(3) {
        println!("    UNRESOLVED {} [{}.{}] -> {} ({})", u.event, u.source, u.raw, u.expected, u.reason);
    }
}

/// RTS-UI groundwork (`zr-ui`): `.wnd` layouts, mapped images and resolution.
fn ui_report(vfs: &Vfs, db: &zr_ini::IniDb, language: &str) {
    let report = zr_ui::make_report(vfs, db, language);
    println!("  ui: {}", report.summary());
    for (name, reason) in report.images.missing.iter().take(3) {
        println!("    UNRESOLVED {name}: {reason}");
    }
}

/// Counts for the chunks decoded beyond the basics: triggers, water, scripts, leftovers.
fn map_report(maps: &[(String, MapFile)]) {
    let with_triggers = maps.iter().filter(|(_, m)| m.triggers.as_ref().is_some_and(|t| !t.triggers.is_empty())).count();
    let trig = |f: &dyn Fn(&zr_map::PolygonTrigger) -> bool| -> usize { maps.iter().filter_map(|(_, m)| m.triggers.as_ref()).flat_map(|t| &t.triggers).filter(|t| f(t)).count() };
    let mut versions: BTreeMap<u16, usize> = BTreeMap::new();
    for t in maps.iter().filter_map(|(_, m)| m.triggers.as_ref()) {
        *versions.entry(t.version).or_default() += 1;
    }
    println!(
        "  triggers: {with_triggers} maps with triggers, {} total, {} water areas ({} maps), {} rivers, {} with < 2 points; chunk versions {versions:?}",
        trig(&|_| true),
        trig(&|t| t.is_water),
        maps.iter().filter(|(_, m)| m.triggers.as_ref().is_some_and(|t| t.water_areas().next().is_some())).count(),
        trig(&|t| t.is_river),
        trig(&|t| !t.is_valid()),
    );
    let mut tot = zr_map::ScriptStats::default();
    let mut with_scripts = 0;
    let mut unknown_ids = 0;
    let (mut named, mut mismatched) = (0usize, 0usize);
    for (_, m) in maps {
        let Some(s) = m.scripts() else { continue };
        let st = s.stats();
        with_scripts += usize::from(st.scripts > 0);
        for (a, b) in [(&mut tot.lists, st.lists), (&mut tot.groups, st.groups), (&mut tot.scripts, st.scripts), (&mut tot.or_conditions, st.or_conditions), (&mut tot.conditions, st.conditions), (&mut tot.actions, st.actions), (&mut tot.actions_false, st.actions_false)] {
            *a += b;
        }
        for sc in s.lists.iter().flat_map(|l| l.all_scripts()) {
            unknown_ids += sc.or_conditions.iter().flat_map(|o| &o.conditions).filter(|c| c.enum_name().is_none()).count();
            let names = sc.or_conditions.iter().flat_map(|o| &o.conditions).map(|c| (c.internal_name.as_deref(), c.enum_name())).chain(sc.actions.iter().chain(&sc.actions_false).map(|a| (a.internal_name.as_deref(), a.enum_name())));
            for (stored, table) in names.filter(|(s, _)| s.is_some()) {
                named += 1;
                mismatched += usize::from(stored != table);
            }
            unknown_ids += sc.actions.iter().chain(&sc.actions_false).filter(|a| a.enum_name().is_none()).count();
        }
    }
    println!(
        "  scripts: {with_scripts} maps with scripts, {} lists, {} groups, {} scripts, {} or-blocks, {} conditions, {} actions (+{} false-branch), {unknown_ids} out-of-range type ids, {mismatched}/{named} stored internal names differ from the header enum table",
        tot.lists, tot.groups, tot.scripts, tot.or_conditions, tot.conditions, tot.actions, tot.actions_false
    );
    let failures: Vec<_> = maps.iter().flat_map(|(p, m)| m.decode_errors.iter().map(move |e| format!("{p}: {} {}", e.chunk, e.error))).collect();
    println!("  optional-chunk decode failures: {}", failures.len());
    for f in failures.iter().take(10) {
        println!("    FAIL {f}");
    }
    // Top-level chunks that have no typed decoder, and the nested ones inside SidesList.
    const DECODED: &[&str] = &["HeightMapData", "BlendTileData", "WorldInfo", "ObjectsList", "SidesList", "GlobalLighting", "WaypointsList", "PolygonTriggers", "PlayerScriptsList"];
    let mut raw: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, m) in maps {
        for c in m.chunk_names().filter(|c| !DECODED.contains(c)) {
            *raw.entry(c).or_default() += 1;
        }
    }
    println!("  chunks still raw (maps containing): {raw:?}");
    let mut nested: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, m) in maps {
        for c in m.sides.iter().flat_map(|s| &s.nested).filter(|c| c.name != "PlayerScriptsList") {
            *nested.entry(c.name.as_str()).or_default() += 1;
        }
    }
    println!("  other chunks nested in SidesList: {nested:?}");
}

fn extract(vfs: &Vfs, prefix: &str, out: &Path) -> Result<()> {
    let mut n = 0;
    for (path, _) in vfs.files() {
        if !path.starts_with(prefix) {
            continue;
        }
        let dest = out.join(path.replace('\\', "/"));
        std::fs::create_dir_all(dest.parent().unwrap())?;
        std::fs::write(&dest, vfs.read_archived(path).unwrap())?;
        n += 1;
    }
    println!("extracted {n} files to {}", out.display());
    Ok(())
}
