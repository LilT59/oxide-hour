//! Real-data report over the parsed tables and the VFS: reference resolution
//! counts, unresolved samples and a WAV format breakdown. Used by the crate
//! example and `zr-probe parse`.

use std::collections::BTreeMap;
use std::fmt;

use zr_assets::Vfs;

use crate::{parse_wav, AudioDatabase, AudioType, WavFormat};

/// Per-kind reference totals.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KindRefs {
    pub events: usize,
    pub references: usize,
    pub resolved: usize,
    pub unresolved: usize,
}

/// One sound file reference that is not in the VFS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unresolved {
    pub event: String,
    pub source: &'static str,
    pub raw: String,
    /// The base (non-localized) path the engine would try.
    pub expected: String,
    pub reason: String,
}

/// WAV header statistics over every `.wav` in the VFS.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WavBreakdown {
    pub total: usize,
    pub parsed: usize,
    pub failed: usize,
    pub by_format: BTreeMap<String, usize>,
    pub by_channels: BTreeMap<u16, usize>,
    pub by_rate: BTreeMap<u32, usize>,
    pub by_bits: BTreeMap<u16, usize>,
    pub failure_samples: Vec<String>,
}

/// Everything the example and the probe print.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AudioReport {
    pub language: String,
    pub sound_effects: KindRefs,
    pub music: KindRefs,
    pub dialog: KindRefs,
    pub unresolved: Vec<Unresolved>,
    pub wav: WavBreakdown,
    pub misc_entries: usize,
    /// `MiscAudio` names that exist as an event (`findAudioEventInfo`).
    pub misc_resolved: usize,
}

/// Builds a report by resolving every reference against `vfs`.
pub fn make_report(db: &AudioDatabase, vfs: &Vfs) -> AudioReport {
    let exists = |p: &str| vfs.read_archived(p).is_some();
    let mut r = AudioReport { language: db.language.clone(), ..Default::default() };
    for event in db.events() {
        let kind = event.sound_type;
        let bucket = match kind {
            AudioType::SoundEffect => &mut r.sound_effects,
            AudioType::Music => &mut r.music,
            AudioType::Streaming => &mut r.dialog,
        };
        bucket.events += 1;
        for reference in db.references(event) {
            bucket.references += 1;
            if db.resolve_reference(kind, &reference.raw, &exists).is_some() {
                bucket.resolved += 1;
            } else {
                bucket.unresolved += 1;
                let expected = db.settings.base_path(kind, &db.reference_filename(kind, &reference.raw));
                let reason = if reference.raw.eq_ignore_ascii_case("NoFilename") {
                    "placeholder (NoFilename)".to_owned()
                } else {
                    "file not in VFS".to_owned()
                };
                r.unresolved.push(Unresolved {
                    event: event.name.clone(),
                    source: reference.source,
                    raw: reference.raw,
                    expected,
                    reason,
                });
            }
        }
    }
    for (key, value) in &db.misc_audio {
        let _ = key;
        r.misc_entries += 1;
        if db.get(value).is_some() {
            r.misc_resolved += 1;
        }
    }
    r.wav = wav_breakdown(vfs);
    r
}

fn wav_breakdown(vfs: &Vfs) -> WavBreakdown {
    let mut w = WavBreakdown::default();
    for (path, _) in vfs.files() {
        if !path.ends_with(".wav") {
            continue;
        }
        w.total += 1;
        let Some(data) = vfs.read_archived(path) else {
            w.failed += 1;
            push_sample(&mut w.failure_samples, format!("{path}: unreadable"));
            continue;
        };
        match parse_wav(data) {
            Ok(info) => {
                w.parsed += 1;
                *w.by_format.entry(format_name(info.format)).or_default() += 1;
                *w.by_channels.entry(info.channels).or_default() += 1;
                *w.by_rate.entry(info.sample_rate).or_default() += 1;
                *w.by_bits.entry(info.bits_per_sample).or_default() += 1;
            }
            Err(e) => {
                w.failed += 1;
                push_sample(&mut w.failure_samples, format!("{path}: {e}"));
            }
        }
    }
    w
}

fn format_name(f: WavFormat) -> String {
    match f {
        WavFormat::Other(t) => format!("other (0x{t:04X})"),
        other => other.name().to_owned(),
    }
}

fn push_sample(v: &mut Vec<String>, s: String) {
    const MAX: usize = 10;
    if v.len() < MAX {
        v.push(s);
    }
}

impl AudioReport {
    /// One-line summary for the `zr-probe parse` block.
    pub fn summary(&self) -> String {
        format!(
            "{} audio events + {} music tracks + {} dialog events; refs {}/{} resolve (sfx {}/{}); {} .wav ({} decoded: {}); {} MiscAudio refs, {} resolve",
            self.sound_effects.events,
            self.music.events,
            self.dialog.events,
            self.total_resolved(),
            self.total_references(),
            self.sound_effects.resolved,
            self.sound_effects.references,
            self.wav.total,
            self.wav.parsed,
            format_map(&self.wav.by_format),
            self.misc_entries,
            self.misc_resolved,
        )
    }

    pub fn total_references(&self) -> usize {
        self.sound_effects.references + self.music.references + self.dialog.references
    }

    pub fn total_resolved(&self) -> usize {
        self.sound_effects.resolved + self.music.resolved + self.dialog.resolved
    }
}

fn format_map<T: fmt::Display>(m: &BTreeMap<T, usize>) -> String {
    let mut parts: Vec<String> = m.iter().map(|(k, v)| format!("{k}:{v}")).collect();
    if parts.is_empty() {
        parts.push("none".to_owned());
    }
    parts.join(", ")
}

impl fmt::Display for AudioReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "audio ({})", self.language)?;
        for (name, k) in [
            ("AudioEvent", &self.sound_effects),
            ("MusicTrack", &self.music),
            ("DialogEvent", &self.dialog),
        ] {
            writeln!(
                f,
                "  {name}: {} events, {} sound refs, {} resolve, {} missing",
                k.events, k.references, k.resolved, k.unresolved
            )?;
        }
        writeln!(
            f,
            "  total refs {}/{} resolve",
            self.total_resolved(),
            self.total_references()
        )?;
        writeln!(
            f,
            "  MiscAudio: {} refs, {} resolve to an event",
            self.misc_entries, self.misc_resolved
        )?;
        writeln!(
            f,
            "  .wav: {} files, {} decoded, {} failed",
            self.wav.total, self.wav.parsed, self.wav.failed
        )?;
        writeln!(f, "    formats:  {}", format_map(&self.wav.by_format))?;
        writeln!(f, "    channels: {}", format_map(&self.wav.by_channels))?;
        writeln!(f, "    rates:    {}", format_map(&self.wav.by_rate))?;
        writeln!(f, "    bits:     {}", format_map(&self.wav.by_bits))?;
        if !self.wav.failure_samples.is_empty() {
            writeln!(f, "    failures:")?;
            for s in &self.wav.failure_samples {
                writeln!(f, "      {s}")?;
            }
        }
        writeln!(f, "  unresolved references: {}", self.unresolved.len())?;
        for u in self.unresolved.iter().take(15) {
            writeln!(
                f,
                "    {} [{}.{}] -> {} ({})",
                u.event, u.source, u.raw, u.expected, u.reason
            )?;
        }
        Ok(())
    }
}
