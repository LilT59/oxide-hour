//! Generic parser for the Generals / Zero Hour INI dialect.
//!
//! The dialect is *not* the classic `[Section] Key=Value` format. A file is a
//! sequence of top-level blocks, each opened by a type token from the engine's
//! block table and closed by `End`; blocks nest, and the same key may repeat.
//! The tokenizer, comment and `End` rules follow `INI::readLine`,
//! `INI::load` and `INI::initFromINIMulti` in
//! `Core/GameEngine/Source/Common/INI/INI.cpp` of the reference sources.
//!
//! This crate produces a *syntax tree only* (see [`Block`]); interpreting
//! field values is left to the consumers that own each block type.
//!
//! # Block nesting
//!
//! Whether `Key = Value` opens a nested block is decided by the field parser
//! in C++, not by syntax, so [`blocks::opens_block`] holds an explicit
//! table, derived from the C++ field tables and verified against every
//! shipped INI file (`examples/parse_all.rs`). See that module for details.
//!
//! # Extensions
//!
//! The engine has no preprocessor and treats only `;` as a comment. This
//! parser additionally honours `#define`, `#include` and `//` comments for
//! modding convenience; none of them occur in the shipped data.

pub mod blocks;
mod db;
mod load;
mod preprocess;
mod tree;

pub use db::IniDb;
pub use load::{
    detect_language, engine_load_order, load_map_ini, load_zero_hour, load_zero_hour_lang, map_dir, Loaded,
    MapIni, Step, LOAD_ORDER,
};
pub use tree::{tokens, Block, Field, Item};

use std::fmt;

/// A parse failure with the file and 1-based line where it was detected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub file: String,
    pub line: u32,
    pub kind: ErrorKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    /// A line at top level does not start with a known block type
    /// (`INI::load` throws `INI_UNKNOWN_TOKEN`).
    UnknownBlock(String),
    /// End of file inside a block (`INI_MISSING_END_TOKEN`); carries the
    /// block kind and the line it was opened on.
    MissingEnd { kind: String, opened_at: u32 },
    /// `#include` of a file the loader could not provide, or nested too deep.
    Include(String),
    /// Malformed `#define` / `#include` directive.
    Directive(String),
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: ", self.file, self.line)?;
        match &self.kind {
            ErrorKind::UnknownBlock(t) => write!(f, "unknown block type `{t}`"),
            ErrorKind::MissingEnd { kind, opened_at } => {
                write!(f, "missing `End` for `{kind}` opened at line {opened_at}")
            }
            ErrorKind::Include(m) => write!(f, "include failed: {m}"),
            ErrorKind::Directive(m) => write!(f, "bad directive: {m}"),
        }
    }
}

impl std::error::Error for ParseError {}

/// Parses `text` (named `file` for diagnostics). `#include "path"` is resolved
/// through `loader`, which receives the path exactly as written.
pub fn parse_with(
    file: &str,
    text: &str,
    loader: &mut dyn FnMut(&str) -> Option<String>,
) -> Result<Vec<Block>, ParseError> {
    let lines = preprocess::expand(file, text, loader)?;
    tree::parse_lines(&lines)
}

/// Parses `text` with `#include` disabled.
pub fn parse(file: &str, text: &str) -> Result<Vec<Block>, ParseError> {
    parse_with(file, text, &mut |_| None)
}

#[cfg(test)]
mod tests;
