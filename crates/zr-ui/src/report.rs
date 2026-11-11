//! Real-data report over the `Window\*.wnd` files, `MappedImage`s and their
//! texture / text references. Shared by the crate example and `zr-probe parse`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use zr_assets::Vfs;
use zr_ini::IniDb;

use crate::command::{CommandDb, COMMAND_SET_SLOTS};
use crate::{parse as parse_wnd, ImageDb, ImageResolution};

/// `.wnd` parse totals.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WndStats {
    pub files: usize,
    pub parsed: usize,
    pub windows: usize,
    pub by_type: BTreeMap<String, usize>,
    pub failures: Vec<(String, String)>,
}

/// Image reference resolution totals.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImageStats {
    pub mapped_images: usize,
    pub referenced: usize,
    pub resolved: usize,
    /// `(image name, reason)`; reasons include the first `.wnd` that uses it.
    pub missing: Vec<(String, String)>,
}

/// Text label resolution totals (only filled with the `text` feature).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextStats {
    pub labels: usize,
    pub resolved: usize,
    pub missing_samples: Vec<String>,
}

/// `CommandButton` / `CommandSet` totals.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandStats {
    pub buttons: usize,
    pub sets: usize,
    /// `(button type name, count)`.
    pub by_kind: BTreeMap<String, usize>,
    /// Command-set slot entries that reference a command button.
    pub slot_refs: usize,
    /// Slot entries whose button exists / is missing.
    pub slots_filled: usize,
    pub slots_missing: usize,
    /// Buttons that name a `ButtonImage`, and how many of those images resolve.
    pub button_images: usize,
    pub button_images_resolved: usize,
    pub missing_samples: Vec<String>,
}

/// Everything the example and the probe print.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UiReport {
    pub wnd: WndStats,
    pub images: ImageStats,
    pub text: TextStats,
    pub commands: CommandStats,
}

/// Scans every `Window\*.wnd` in the VFS and resolves its image/text references.
pub fn make_report(vfs: &Vfs, db: &IniDb, language: &str) -> UiReport {
    let images = ImageDb::from_ini(db);
    let mut r = UiReport { images: ImageStats { mapped_images: images.len(), ..Default::default() }, ..Default::default() };

    // image name -> first .wnd that references it.
    let mut refs: BTreeMap<String, String> = BTreeMap::new();
    // text labels (TEXT / TOOLTIPTEXT).
    let mut labels: BTreeSet<String> = BTreeSet::new();

    for (path, _) in vfs.files() {
        if !(path.starts_with("window\\") && path.ends_with(".wnd")) {
            continue;
        }
        r.wnd.files += 1;
        let Some(bytes) = vfs.read_archived(path) else {
            r.wnd.failures.push((path.to_owned(), "unreadable".to_owned()));
            continue;
        };
        match parse_wnd(bytes) {
            Ok(f) => {
                r.wnd.parsed += 1;
                for w in f.windows.iter().flat_map(|root| root.walk()) {
                    r.wnd.windows += 1;
                    *r.wnd.by_type.entry(w.window_type.clone()).or_default() += 1;
                    for name in w.draw_data.image_names() {
                        refs.entry(name.to_owned()).or_insert_with(|| path.to_owned());
                    }
                    for label in [&w.text, &w.tooltip_text].into_iter().flatten() {
                        if !label.is_empty() && !label.eq_ignore_ascii_case("[none]") {
                            labels.insert(label.clone());
                        }
                    }
                }
            }
            Err(e) => r.wnd.failures.push((path.to_owned(), e.to_string())),
        }
    }

    let exists = |p: &str| vfs.read_archived(p).is_some();
    for (name, user) in &refs {
        r.images.referenced += 1;
        match images.resolve(name, language, &exists) {
            ImageResolution::Found { .. } => r.images.resolved += 1,
            ImageResolution::MissingImage { .. } => {
                r.images.missing.push((name.clone(), format!("no MappedImage block (used by {user})")));
            }
            ImageResolution::MissingTexture { texture, candidates, .. } => {
                r.images.missing.push((
                    name.clone(),
                    format!("texture `{texture}` not in VFS, tried {} (used by {user})", candidates.join(", ")),
                ));
            }
            ImageResolution::NoImage => {}
        }
    }

    // CommandButton / CommandSet.
    let commands = CommandDb::from_ini(db);
    r.commands.buttons = commands.buttons().len();
    r.commands.sets = commands.sets().len();
    for b in commands.buttons() {
        *r.commands.by_kind.entry(b.command.as_str().to_owned()).or_default() += 1;
        if let Some(img) = &b.button_image {
            r.commands.button_images += 1;
            if images.get(img).is_some() && images.resolve(img, language, &exists).resolve_ok() {
                r.commands.button_images_resolved += 1;
            } else {
                crate::report::push_sample(&mut r.commands.missing_samples, format!("button image `{img}` (from {})", b.name));
            }
        }
    }
    for set in commands.sets() {
        for i in 0..COMMAND_SET_SLOTS {
            let Some(name) = set.slot(i) else { continue };
            r.commands.slot_refs += 1;
            if commands.button(name).is_some() {
                r.commands.slots_filled += 1;
            } else {
                r.commands.slots_missing += 1;
                crate::report::push_sample(&mut r.commands.missing_samples, format!("{}.{} -> `{name}`", set.name, i + 1));
            }
        }
    }

    r.text.labels = labels.len();
    #[cfg(feature = "text")]
    {
        if let Ok(text) = zr_text::load(vfs, language) {
            for label in &labels {
                if text.get(label).is_some() {
                    r.text.resolved += 1;
                } else {
                    crate::report::push_sample(&mut r.text.missing_samples, label.clone());
                }
            }
        }
    }

    r
}

fn push_sample(v: &mut Vec<String>, s: String) {
    const MAX: usize = 8;
    if v.len() < MAX {
        v.push(s);
    }
}

impl UiReport {
    /// One-line summary for the `zr-probe parse` block.
    pub fn summary(&self) -> String {
        let missing_images = self.images.missing.iter().filter(|(_, r)| r.starts_with("no MappedImage")).count();
        let missing_textures = self.images.missing.len() - missing_images;
        format!(
            "{} .wnd ({} parsed, {} windows); {} MappedImages; image refs {}/{} resolve ({} no image, {} missing texture); text {}/{} labels resolve; {} CommandButtons + {} CommandSets, {} slot refs ({} missing), {} button images ({}/{} resolve)",
            self.wnd.files,
            self.wnd.parsed,
            self.wnd.windows,
            self.images.mapped_images,
            self.images.resolved,
            self.images.referenced,
            missing_images,
            missing_textures,
            self.text.resolved,
            self.text.labels,
            self.commands.buttons,
            self.commands.sets,
            self.commands.slot_refs,
            self.commands.slots_missing,
            self.commands.button_images,
            self.commands.button_images_resolved,
            self.commands.button_images,
        )
    }

    /// `(type, count)` pairs, most common first.
    pub fn by_type_sorted(&self) -> Vec<(&str, usize)> {
        let mut v: Vec<(&str, usize)> = self.wnd.by_type.iter().map(|(k, n)| (k.as_str(), *n)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        v
    }
}

impl fmt::Display for UiReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "windows: {} .wnd files ({} parse), {} windows total",
            self.wnd.files, self.wnd.parsed, self.wnd.windows
        )?;
        for (kind, n) in self.by_type_sorted() {
            writeln!(f, "  {kind:<16} {n}")?;
        }
        for (path, e) in &self.wnd.failures {
            writeln!(f, "  FAIL {path}: {e}")?;
        }
        writeln!(f, "mapped images: {}", self.images.mapped_images)?;
        writeln!(
            f,
            "image references: {} unique, {} resolve, {} unresolved",
            self.images.referenced,
            self.images.resolved,
            self.images.missing.len()
        )?;
        for (name, reason) in self.images.missing.iter().take(20) {
            writeln!(f, "  MISSING {name}: {reason}")?;
        }
        writeln!(f, "text: {}/{} labels resolve", self.text.resolved, self.text.labels)?;
        for label in &self.text.missing_samples {
            writeln!(f, "  literal {label}")?;
        }
        writeln!(
            f,
            "commands: {} buttons, {} sets, {} slot refs ({} unresolved), {} button images ({} resolve)",
            self.commands.buttons,
            self.commands.sets,
            self.commands.slot_refs,
            self.commands.slots_missing,
            self.commands.button_images,
            self.commands.button_images_resolved
        )?;
        let mut kinds: Vec<_> = self.commands.by_kind.iter().collect();
        kinds.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        for (kind, n) in kinds.iter().take(12) {
            writeln!(f, "  {kind:<24} {n}")?;
        }
        for s in &self.commands.missing_samples {
            writeln!(f, "  UNRESOLVED {s}")?;
        }
        Ok(())
    }
}
