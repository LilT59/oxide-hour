//! Warnings and the raw (unparsed) INI tree kept for modules the sim does not need yet.

use std::fmt;

use zr_ini::{Block, Field, Item};

/// What went wrong while building a template.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WarningKind {
    /// A field the engine's parse table for this block does not know (`DEBUG_CRASH` "Unknown field"
    /// in `INI::initFromINIMulti`, silently ignored in release builds).
    UnknownField,
    /// A field whose value the engine's parse procedure rejects (the engine throws and aborts the load).
    BadValue,
    /// A reference to a weapon, armor, locomotor, ... that does not exist.
    UnknownReference,
    /// A structural problem (bad module edit, locomotor without AI module, ...).
    Structure,
    /// A later definition the engine ignores (duplicate `Weapon`).
    DuplicateIgnored,
}

/// One problem found while converting INI blocks into templates. Never silently dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    pub kind: WarningKind,
    /// What was being parsed, e.g. `Object AmericaTankCrusader > Behavior ModuleTag_03`.
    pub scope: String,
    /// The offending key (empty for block-level problems).
    pub field: String,
    /// 1-based source line within the INI file (0 if unknown).
    pub line: u32,
    pub message: String,
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{:?}] {}", self.kind, self.scope)?;
        if !self.field.is_empty() {
            write!(f, ": {}", self.field)?;
        }
        write!(f, " (line {}): {}", self.line, self.message)
    }
}

/// Collects warnings together with the scope they are raised in.
#[derive(Debug, Default)]
pub struct Diag {
    pub warnings: Vec<Warning>,
    scope: Vec<String>,
}

impl Diag {
    pub fn push_scope(&mut self, s: impl Into<String>) {
        self.scope.push(s.into());
    }
    pub fn pop_scope(&mut self) {
        self.scope.pop();
    }
    pub fn scope_string(&self) -> String {
        self.scope.join(" > ")
    }
    pub fn warn(&mut self, kind: WarningKind, field: &str, line: u32, message: impl Into<String>) {
        let w = Warning { kind, scope: self.scope_string(), field: field.to_string(), line, message: message.into() };
        self.warnings.push(w);
    }
    pub fn unknown_field(&mut self, f: &Field) {
        self.warn(WarningKind::UnknownField, &f.key, f.line, format!("unknown field `{} = {}`", f.key, f.value));
    }
    pub fn bad_value(&mut self, f: &Field, why: impl Into<String>) {
        self.warn(WarningKind::BadValue, &f.key, f.line, format!("`{}`: {}", f.value, why.into()));
    }
}

/// A raw `Key = Value` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawField {
    pub key: String,
    /// Everything after the key, as `zr_ini` produced it.
    pub value: String,
    pub line: u32,
}

/// A raw nested block (`Key = header...` ... `End`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawBlock {
    pub kind: String,
    pub header: Vec<String>,
    pub items: Vec<RawItem>,
    pub line: u32,
}

/// A field or nested block, in file order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawItem {
    Field(RawField),
    Block(RawBlock),
}

impl RawItem {
    pub fn from_block_items(items: &[Item]) -> Vec<RawItem> {
        items
            .iter()
            .map(|i| match i {
                Item::Field(f) => RawItem::Field(RawField { key: f.key.clone(), value: f.value.clone(), line: f.line }),
                Item::Block(b) => RawItem::Block(RawBlock::from(b)),
            })
            .collect()
    }
}

impl From<&Block> for RawBlock {
    fn from(b: &Block) -> Self {
        RawBlock {
            kind: b.kind.clone(),
            header: b.header.clone(),
            items: RawItem::from_block_items(&b.items),
            line: b.line,
        }
    }
}

/// Helpers for looking into raw item lists.
pub fn raw_value<'a>(items: &'a [RawItem], key: &str) -> Option<&'a str> {
    items
        .iter()
        .filter_map(|i| match i {
            RawItem::Field(f) if f.key == key => Some(f.value.as_str()),
            _ => None,
        })
        .next_back()
}
