//! Game string tables (`TheGameText`).
//!
//! Ported from `Core/GameEngine/Source/GameClient/GameText.cpp`. The engine
//! loads either the source text file `Data\Generals.str` (if present) or the
//! compiled `Data\<language>\Generals.csf`, builds a table sorted by label, and
//! answers `fetch(label)` with a binary search using `stricmp`, i.e. labels are
//! ASCII case-insensitive. A per-map `map.str` (see `zr_ini::load_map_ini`) is
//! a second table consulted when the main one misses.
//!
//! * [`StringTable::parse_csf`]: compiled format, see [`csf`].
//! * [`StringTable::parse_str`]: source format, see [`str_format`].
//! * [`GameText`]: main table + map table, with `fetch`-style lookups.
//! * [`load`]: discovers the file in a [`zr_assets::Vfs`].
//!
//! All parsers take untrusted bytes: every read is bounds-checked, allocations
//! are limited by the input size, and malformed input is an [`Error`], never a
//! panic.

pub mod csf;
pub mod str_format;

use std::borrow::Cow;
use std::fmt;

use zr_assets::{normalize_path, Vfs};

/// One labelled string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub label: String,
    /// The first string stored for the label, with the engine's whitespace
    /// clean-up (`stripSpaces`) applied. Empty if the label has no string.
    pub text: String,
    /// Voice-over file name (the `STRW` wave name in `.csf`, the name after the
    /// closing quote in `.str`), if any.
    pub speech: Option<String>,
}

/// A parse failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The file is too short for its header (`GameTextManager::parseCSF` requires a full `CSFHeader`).
    Truncated { offset: usize },
    /// The `.csf` header does not start with the `CSF_ID` tag.
    BadMagic,
    /// A label block does not start with the `CSF_LABEL` tag (`parseCSF` aborts the whole load).
    BadLabelTag { offset: usize },
    /// A string block is neither `CSF_STRING` nor `CSF_STRINGWITHWAVE`.
    BadStringTag { offset: usize },
    /// A negative or impossible length field.
    BadLength { offset: usize },
    /// `.str`: the file ended between a label and its `END`.
    UnexpectedEnd { label: String },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Truncated { offset } => write!(f, "truncated at byte {offset}"),
            Error::BadMagic => write!(f, "not a compiled string file (bad header tag)"),
            Error::BadLabelTag { offset } => write!(f, "expected a label block at byte {offset}"),
            Error::BadStringTag { offset } => write!(f, "expected a string block at byte {offset}"),
            Error::BadLength { offset } => write!(f, "invalid length at byte {offset}"),
            Error::UnexpectedEnd { label } => write!(f, "string file ended inside label `{label}`"),
        }
    }
}

impl std::error::Error for Error {}

/// A table of labelled strings with case-insensitive lookup.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StringTable {
    entries: Vec<Entry>,
    /// Indices into `entries`, sorted by lowercase label (stable, so the first
    /// duplicate in file order comes first).
    sorted: Vec<u32>,
    /// `.csf` language id (`LanguageID`), if the format carries one.
    pub language_id: Option<u32>,
}

/// `stricmp` ordering of two labels.
fn cmp_nocase(a: &str, b: &str) -> std::cmp::Ordering {
    a.bytes().map(|c| c.to_ascii_lowercase()).cmp(b.bytes().map(|c| c.to_ascii_lowercase()))
}

impl StringTable {
    /// Builds the lookup index. Called by the parsers.
    pub(crate) fn from_entries(entries: Vec<Entry>, language_id: Option<u32>) -> Self {
        let mut sorted: Vec<u32> = (0..entries.len() as u32).collect();
        sorted.sort_by(|&a, &b| cmp_nocase(&entries[a as usize].label, &entries[b as usize].label));
        Self { entries, sorted, language_id }
    }

    /// Parses a compiled `.csf` file. See [`csf`].
    pub fn parse_csf(bytes: &[u8]) -> Result<Self, Error> {
        csf::parse(bytes)
    }

    /// Parses a source `.str` file. See [`str_format`].
    pub fn parse_str(bytes: &[u8]) -> Result<Self, Error> {
        str_format::parse(bytes)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Entries in file order.
    pub fn iter(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter()
    }

    /// The entry for `label` (ASCII case-insensitive). For duplicate labels the
    /// engine's `bsearch` result is unspecified; the first in file order is returned.
    pub fn entry(&self, label: &str) -> Option<&Entry> {
        let i = self.sorted.partition_point(|&i| cmp_nocase(&self.entries[i as usize].label, label).is_lt());
        let e = &self.entries[*self.sorted.get(i)? as usize];
        cmp_nocase(&e.label, label).is_eq().then_some(e)
    }

    pub fn get(&self, label: &str) -> Option<&str> {
        self.entry(label).map(|e| e.text.as_str())
    }

    /// Labels that start with `prefix` (case-sensitive, like the engine's
    /// `strstr(...) == label`), in sorted-label order
    /// (`GameTextManager::getStringsWithLabelPrefix`).
    pub fn labels_with_prefix<'a>(&'a self, prefix: &'a str) -> impl Iterator<Item = &'a str> {
        self.sorted
            .iter()
            .map(|&i| self.entries[i as usize].label.as_str())
            .filter(move |l| l.starts_with(prefix))
    }
}

/// `TheGameText`: the main table, the current map's `map.str` table, and the
/// install's `Data\Patch.str` (see [`load`]).
#[derive(Debug, Clone, Default)]
pub struct GameText {
    pub main: StringTable,
    pub map: Option<StringTable>,
    /// `Data\Patch.str` (in `PatchData.big`): labels the main table lacks, such as
    /// `GUI:CustomMission` (the start screen's Custom Mission button).
    pub patch: Option<StringTable>,
}

impl GameText {
    pub fn new(main: StringTable) -> Self {
        Self { main, map: None, patch: None }
    }

    /// `initMapStringFile`: installs the map's table (replaces any previous one).
    pub fn set_map_strings(&mut self, table: StringTable) {
        self.map = Some(table);
    }

    /// `GameTextManager::reset`: drops the map table.
    pub fn clear_map_strings(&mut self) {
        self.map = None;
    }

    /// Looks `label` up in the main table, then the map table, then the patch table.
    pub fn get(&self, label: &str) -> Option<&str> {
        self.main
            .get(label)
            .or_else(|| self.map.as_ref()?.get(label))
            .or_else(|| self.patch.as_ref()?.get(label))
    }

    pub fn entry(&self, label: &str) -> Option<&Entry> {
        self.main
            .entry(label)
            .or_else(|| self.map.as_ref()?.entry(label))
            .or_else(|| self.patch.as_ref()?.entry(label))
    }

    /// `fetch`: the text, or the engine's `MISSING: 'label'` marker.
    pub fn fetch(&self, label: &str) -> Cow<'_, str> {
        match self.get(label) {
            Some(t) => Cow::Borrowed(t),
            None => Cow::Owned(format!("MISSING: '{label}'")),
        }
    }

    /// `fetchOrSubstitute`.
    pub fn fetch_or<'a>(&'a self, label: &str, substitute: &'a str) -> &'a str {
        self.get(label).unwrap_or(substitute)
    }

    /// `getStringsWithLabelPrefix`: main-table labels, then map-table labels, then patch labels.
    pub fn labels_with_prefix<'a>(&'a self, prefix: &'a str) -> impl Iterator<Item = &'a str> {
        self.main
            .labels_with_prefix(prefix)
            .chain(self.map.iter().flat_map(move |m| m.labels_with_prefix(prefix)))
            .chain(self.patch.iter().flat_map(move |p| p.labels_with_prefix(prefix)))
    }
}

/// Failure to load text from a [`Vfs`].
#[derive(Debug)]
pub enum LoadError {
    /// Neither `Data\Generals.str` nor `Data\<language>\Generals.csf` exists.
    NotFound { language: String },
    Parse { file: String, error: Error },
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::NotFound { language } => write!(f, "no string file for language `{language}`"),
            LoadError::Parse { file, error } => write!(f, "{file}: {error}"),
        }
    }
}

impl std::error::Error for LoadError {}

/// The language directory with a `Generals.csf`: `english` (the engine's
/// `GetRegistryLanguage` default) if present, else the first alphabetically.
pub fn detect_language(vfs: &Vfs) -> String {
    let mut langs: Vec<&str> = vfs
        .files()
        .filter_map(|(p, _)| p.strip_prefix(r"data\")?.strip_suffix(r"\generals.csf"))
        .filter(|l| !l.contains('\\'))
        .collect();
    langs.sort_unstable();
    if langs.is_empty() || langs.contains(&"english") {
        "english".into()
    } else {
        langs[0].into()
    }
}

/// `Data\Patch.str`, which `PatchData.big` ships on top of the language CSF. The retail
/// `EnglishZH.big` CSF has no `GUI:CustomMission`, yet the retail start screen shows
/// "CUSTOM MISSION", so the patch table is layered under the main one (it fills labels
/// the main table lacks; the C++ sources in `references/` never name this file).
pub const PATCH_STR_PATH: &str = r"data\patch.str";

/// `GameTextManager::init`: use `Data\Generals.str` if it exists, otherwise
/// `Data\<language>\Generals.csf` (`g_strFile` / `g_csfFile` in `Main/WinMain.cpp`). Then
/// layers [`PATCH_STR_PATH`] on top when present. A patch file that fails to parse is ignored.
pub fn load(vfs: &Vfs, language: &str) -> Result<GameText, LoadError> {
    let str_path = r"data\generals.str";
    let csf_path = format!(r"data\{}\generals.csf", normalize_path(language));
    type Parser = fn(&[u8]) -> Result<StringTable, Error>;
    let (file, parse): (&str, Parser) = if vfs.read(str_path).is_some() {
        (str_path, StringTable::parse_str)
    } else if vfs.read(&csf_path).is_some() {
        (&csf_path, StringTable::parse_csf)
    } else {
        return Err(LoadError::NotFound { language: language.to_owned() });
    };
    let bytes = vfs.read(file).ok_or_else(|| LoadError::NotFound { language: language.to_owned() })?;
    let mut text = parse(&bytes)
        .map(GameText::new)
        .map_err(|error| LoadError::Parse { file: file.to_owned(), error })?;
    text.patch = vfs.read(PATCH_STR_PATH).and_then(|b| StringTable::parse_str(&b).ok());
    Ok(text)
}

/// Loads a map's string file (the path returned in `zr_ini::MapIni::map_str`).
pub fn load_map_strings(vfs: &Vfs, path: &str) -> Option<Result<StringTable, LoadError>> {
    let bytes = vfs.read(path)?;
    Some(StringTable::parse_str(&bytes).map_err(|error| LoadError::Parse { file: path.to_owned(), error }))
}

#[cfg(test)]
mod tests;
