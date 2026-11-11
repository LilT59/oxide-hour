//! Syntax tree and the recursive-descent block parser.

use crate::blocks::{is_block_type, is_line_block, opens_block};
use crate::preprocess::Line;
use crate::{ErrorKind, ParseError};
use std::fmt;

/// Token separators of `INI::getSeps()`: space, `\n`, `\r`, `\t` and `=`.
const SEPS: [char; 4] = [' ', '\t', '\r', '='];

/// Splits `s` into tokens the way the engine's `strtok(.., getSeps())` does.
pub fn tokens(s: &str) -> impl Iterator<Item = &str> {
    s.split(SEPS).filter(|t| !t.is_empty())
}

/// A `Key = Value` line. Keys repeat freely, so fields are kept in file order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub key: String,
    /// Everything after the key, comment-free, with leading separators and
    /// trailing whitespace trimmed. Quotes and `R:1 G:2` style values are left
    /// for the consumer, as the engine does per field type.
    pub value: String,
    pub line: u32,
}

impl Field {
    /// The value split into engine tokens (separators: space, tab, `=`).
    pub fn tokens(&self) -> Vec<&str> {
        tokens(&self.value).collect()
    }
}

/// One entry of a block body, preserving the interleaving of fields and
/// nested blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Field(Field),
    Block(Block),
}

/// A block: either top-level (`kind` is the type, e.g. `Object`, `header[0]`
/// the name) or nested (`kind` is the key, e.g. `Draw`, `header` the value
/// tokens such as `["W3DModelDraw", "ModuleTag_01"]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub kind: String,
    pub header: Vec<String>,
    pub items: Vec<Item>,
    /// Line of the opening token.
    pub line: u32,
}

impl Block {
    /// The block name: first header token (`Object NAME`, `Locomotor NAME`).
    pub fn name(&self) -> Option<&str> {
        self.header.first().map(String::as_str)
    }

    pub fn fields(&self) -> impl Iterator<Item = &Field> {
        self.items.iter().filter_map(|i| match i {
            Item::Field(f) => Some(f),
            Item::Block(_) => None,
        })
    }

    pub fn children(&self) -> impl Iterator<Item = &Block> {
        self.items.iter().filter_map(|i| match i {
            Item::Block(b) => Some(b),
            Item::Field(_) => None,
        })
    }

    /// All fields named `key` (exact, case-sensitive like the engine's tables).
    pub fn fields_named<'a>(&'a self, key: &'a str) -> impl Iterator<Item = &'a Field> {
        self.fields().filter(move |f| f.key == key)
    }

    /// The last field named `key`: the one that wins for a scalar field.
    pub fn field(&self, key: &str) -> Option<&Field> {
        self.fields().filter(|f| f.key == key).last()
    }

    /// Shorthand for the raw value of [`Block::field`].
    pub fn value(&self, key: &str) -> Option<&str> {
        self.field(key).map(|f| f.value.as_str())
    }

    /// Nested blocks whose key is `kind`.
    pub fn children_named<'a>(&'a self, kind: &'a str) -> impl Iterator<Item = &'a Block> {
        self.children().filter(move |b| b.kind == kind)
    }

    pub fn child(&self, kind: &str) -> Option<&Block> {
        self.children().find(|b| b.kind == kind)
    }

    fn write(&self, f: &mut fmt::Formatter<'_>, depth: usize) -> fmt::Result {
        let pad = "  ".repeat(depth);
        write!(f, "{pad}{}", self.kind)?;
        if !self.header.is_empty() {
            // Nested blocks use the data's `Key = Value` spelling.
            write!(
                f,
                "{} {}",
                if depth > 0 { " =" } else { "" },
                self.header.join(" ")
            )?;
        }
        writeln!(f)?;
        for item in &self.items {
            match item {
                Item::Field(x) if x.value.is_empty() => writeln!(f, "{pad}  {}", x.key)?,
                Item::Field(x) => writeln!(f, "{pad}  {} = {}", x.key, x.value)?,
                Item::Block(b) => b.write(f, depth + 1)?,
            }
        }
        writeln!(f, "{pad}End")
    }
}

/// Canonical, re-parseable text form (comments and spacing normalised).
impl fmt::Display for Block {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.write(f, 0)
    }
}

struct Parser<'a> {
    lines: &'a [Line],
    pos: usize,
}

pub(crate) fn parse_lines(lines: &[Line]) -> Result<Vec<Block>, ParseError> {
    let mut p = Parser { lines, pos: 0 };
    let mut out = Vec::new();
    while let Some(line) = p.lines.get(p.pos) {
        let mut toks = tokens(&line.text);
        let Some(kind) = toks.next().filter(|k| is_block_type(k)) else {
            let first = tokens(&line.text).next().unwrap_or_default();
            return Err(p.error(line, ErrorKind::UnknownBlock(first.to_owned())));
        };
        let header = toks.map(str::to_owned).collect();
        p.pos += 1;
        if is_line_block(kind) {
            out.push(Block {
                kind: kind.to_owned(),
                header,
                items: Vec::new(),
                line: line.no,
            });
        } else {
            out.push(p.block(kind.to_owned(), header, line)?);
        }
    }
    Ok(out)
}

impl Parser<'_> {
    fn error(&self, line: &Line, kind: ErrorKind) -> ParseError {
        ParseError {
            file: line.file.to_string(),
            line: line.no,
            kind,
        }
    }

    /// Parses a block body up to and including its `End`
    /// (`INI::initFromINIMulti`: a line whose first token is `End`, any case).
    fn block(
        &mut self,
        kind: String,
        header: Vec<String>,
        open: &Line,
    ) -> Result<Block, ParseError> {
        let mut block = Block {
            kind,
            header,
            items: Vec::new(),
            line: open.no,
        };
        loop {
            let Some(line) = self.lines.get(self.pos) else {
                let last = self.lines.last().unwrap_or(open);
                return Err(self.error(
                    last,
                    ErrorKind::MissingEnd {
                        kind: block.kind,
                        opened_at: open.no,
                    },
                ));
            };
            self.pos += 1;
            let text = line.text.as_str();
            let key = tokens(text).next().unwrap_or_default();
            if key.eq_ignore_ascii_case("end") {
                return Ok(block);
            }
            // The value is whatever follows the key once separators are skipped.
            let value = text[text.find(key).unwrap_or(0) + key.len()..]
                .trim_start_matches(SEPS)
                .trim_end();
            if opens_block(&block.kind, &block.header, key) {
                let header = tokens(value).map(str::to_owned).collect();
                block
                    .items
                    .push(Item::Block(self.block(key.to_owned(), header, line)?));
            } else {
                block.items.push(Item::Field(Field {
                    key: key.to_owned(),
                    value: value.to_owned(),
                    line: line.no,
                }));
            }
        }
    }
}
