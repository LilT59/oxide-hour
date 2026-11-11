//! The source `.str` format (`Generals.str`, per-map `map.str`).
//!
//! Ported from `GameTextManager::parseStringFile` and its helpers
//! (`readToEndOfQuote`, `translateCopy`, `stripSpaces`) in
//! `Core/GameEngine/Source/GameClient/GameText.cpp`:
//!
//! ```text
//! // comment lines start with //
//! LABEL:Name
//! "Text, possibly spanning \"escaped\" quotes\nand lines" wave_name
//! END
//! ```
//!
//! Blank lines and `//` lines between entries are skipped. After the label,
//! lines are scanned until `END` (case-insensitive); the first line starting
//! with `"` opens the string, which runs to the next unescaped `"` (physical
//! newlines inside become spaces; `\n`, `\t`, `\\`, `\"`, `\'`, `\?` are
//! unescaped afterwards). The rest of the closing line is scanned for a voice
//! file name (alphanumerics and `_`, with `e` appended when it ends in a digit).
//! Further quoted strings under the same label are ignored. The bytes are
//! treated as Latin-1.

use crate::csf::{strip_spaces, utf16};
use crate::{Entry, Error, StringTable};

/// `MAX_UITEXT_LENGTH`: the engine stops collecting a quoted string here.
const MAX_QUOTED: usize = 10 * 1024;

/// `iswspace` on a `char`.
fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | 0x0B | 0x0C | b'\r')
}

fn trim(line: &[u8]) -> &[u8] {
    let start = line.iter().position(|&b| !is_space(b)).unwrap_or(line.len());
    let end = line.iter().rposition(|&b| !is_space(b)).map_or(start, |p| p + 1);
    &line[start..end]
}

/// Byte-wise line source (`readLine`: split on `\n`, keep everything else).
struct Lines<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Lines<'a> {
    fn next_line(&mut self) -> Option<&'a [u8]> {
        if self.pos >= self.data.len() {
            return None;
        }
        let rest = &self.data[self.pos..];
        let n = rest.iter().position(|&b| b == b'\n');
        let line = &rest[..n.unwrap_or(rest.len())];
        self.pos += n.map_or(rest.len(), |n| n + 1);
        Some(line)
    }

    fn next_byte(&mut self) -> Option<u8> {
        let b = *self.data.get(self.pos)?;
        self.pos += 1;
        Some(b)
    }
}

/// `readToEndOfQuote`: `first` is the rest of the opening line after the `"`
/// (plus the `\n` the engine appends). Returns the raw quoted text (escapes
/// intact, newlines as spaces) and the wave name.
fn read_to_end_of_quote(src: &mut Lines, first: &[u8]) -> (Vec<u8>, String) {
    let mut inbuf: Option<std::vec::IntoIter<u8>> = Some([first, b"\n"].concat().into_iter());
    let mut next = |src: &mut Lines| -> Option<u8> {
        if let Some(it) = inbuf.as_mut() {
            if let Some(b) = it.next() {
                return Some(b);
            }
            inbuf = None;
        }
        src.next_byte()
    };

    let mut out = Vec::new();
    let mut slash = false;
    let mut max = MAX_QUOTED;
    while max > 0 {
        let Some(mut ch) = next(src) else { return (out, String::new()) };
        if ch == b'\n' {
            slash = false;
            ch = b' ';
        } else if ch == b'\\' && !slash {
            slash = true;
        } else if ch == b'\\' && slash {
            slash = false;
        } else if ch == b'"' && !slash {
            break;
        } else {
            slash = false;
        }
        if is_space(ch) {
            ch = b' ';
        }
        out.push(ch);
        max -= 1;
    }

    // Remainder of the line: extract the wave file name.
    let mut wave = String::new();
    let mut state = 0;
    while let Some(ch) = next(src) {
        if ch == b'\n' {
            break;
        }
        if state == 0 {
            if is_space(ch) || ch == b'=' {
                continue;
            }
            state = 1;
        }
        if state == 1 {
            if ch.is_ascii_alphanumeric() || ch == b'_' {
                wave.push(ch as char);
                continue;
            }
            state = 2;
        }
    }
    if wave.ends_with(|c: char| c.is_ascii_digit()) {
        wave.push('e');
    }
    (out, wave)
}

/// `translateCopy`: unescape and widen (Latin-1) the raw quoted bytes.
fn translate_copy(raw: &[u8]) -> Vec<u16> {
    let mut out = Vec::with_capacity(raw.len());
    let mut slash = false;
    for &b in raw {
        if slash {
            slash = false;
            out.push(match b {
                b't' => u16::from(b'\t'),
                b'n' => u16::from(b'\n'),
                other => u16::from(other),
            });
        } else if b == b'\\' {
            slash = true;
        } else {
            out.push(u16::from(b));
        }
    }
    out
}

fn latin1(b: &[u8]) -> String {
    b.iter().map(|&c| c as char).collect()
}

/// Parses a `.str` file.
pub fn parse(data: &[u8]) -> Result<StringTable, Error> {
    let mut src = Lines { data, pos: 0 };
    let mut entries = Vec::new();
    while let Some(line) = src.next_line() {
        let label = trim(line);
        if label.is_empty() || label.starts_with(b"//") {
            continue;
        }
        let label = latin1(label);
        let mut entry = Entry { label: label.clone(), text: String::new(), speech: None };
        let mut have_string = false;
        loop {
            let Some(line) = src.next_line() else { return Err(Error::UnexpectedEnd { label }) };
            let line = trim(line);
            if line.first() == Some(&b'"') {
                let (raw, wave) = read_to_end_of_quote(&mut src, &line[1..]);
                if !have_string {
                    entry.text = utf16(&strip_spaces(&translate_copy(&raw)));
                    entry.speech = (!wave.is_empty()).then_some(wave);
                    have_string = true;
                }
            } else if line.eq_ignore_ascii_case(b"END") {
                break;
            }
        }
        entries.push(entry);
    }
    Ok(StringTable::from_entries(entries, None))
}
