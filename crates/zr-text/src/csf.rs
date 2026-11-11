//! The compiled `.csf` format.
//!
//! Ported from `GameTextManager::getCSFInfo` / `parseCSF` in
//! `Core/GameEngine/Source/GameClient/GameText.cpp`. All integers are
//! little-endian `i32`. The engine writes its four-character tags as
//! multi-character constants, so on disk they appear byte-reversed:
//! `CSF_ID` = `" FSC"`, `CSF_LABEL` = `" LBL"`, `CSF_STRING` = `" RTS"`,
//! `CSF_STRINGWITHWAVE` = `"WRTS"`.
//!
//! ```text
//! header   id, version, num_labels, num_strings, skip, langid     (6 x i32)
//! label    CSF_LABEL, num_strings, len, label[len]                (ASCII)
//!   string CSF_STRING|CSF_STRINGWITHWAVE, len, wchar[len]         (UTF-16LE, every unit bitwise NOT)
//!          [STRINGWITHWAVE only: wave_len, wave[wave_len]]
//! ```
//!
//! The engine keeps only the first string of each label (extra values are
//! parsed and skipped) and runs it through `stripSpaces`. `langid` exists from
//! version 2 on. The engine stops at the first block it cannot read and fails
//! the whole file; so do we.

use crate::{Entry, Error, StringTable};

const CSF_ID: &[u8; 4] = b" FSC";
const CSF_LABEL: &[u8; 4] = b" LBL";
const CSF_STRING: &[u8; 4] = b" RTS";
const CSF_STRINGWITHWAVE: &[u8; 4] = b"WRTS";
const HEADER_LEN: usize = 24;

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn bytes(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.data.len());
        let end = end.ok_or(Error::Truncated { offset: self.pos })?;
        let s = &self.data[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn tag(&mut self) -> Result<[u8; 4], Error> {
        Ok(self.bytes(4)?.try_into().expect("4 bytes"))
    }

    /// A non-negative `i32`.
    fn len(&mut self) -> Result<usize, Error> {
        let at = self.pos;
        let v = i32::from_le_bytes(self.bytes(4)?.try_into().expect("4 bytes"));
        usize::try_from(v).map_err(|_| Error::BadLength { offset: at })
    }

    fn at_end(&self) -> bool {
        self.pos >= self.data.len()
    }
}

/// ASCII bytes as Latin-1 characters (`AsciiString` holds raw bytes).
fn latin1(b: &[u8]) -> String {
    b.iter().map(|&c| c as char).collect()
}

/// `GameTextManager::stripSpaces`: drops leading spaces and collapses runs of
/// spaces, also after a tab or newline; removes a space before a tab/newline
/// and a trailing space.
pub(crate) fn strip_spaces(s: &[u16]) -> Vec<u16> {
    let mut out: Vec<u16> = Vec::with_capacity(s.len());
    let mut last = 0u16;
    let mut skipall = true;
    for &ch in s {
        if ch == 0 {
            break;
        }
        if ch == u16::from(b' ') && (last == u16::from(b' ') || skipall) {
            continue;
        }
        if ch == u16::from(b'\n') || ch == u16::from(b'\t') {
            if last == u16::from(b' ') {
                out.pop();
            }
            skipall = true;
            out.push(ch);
            last = ch;
            continue;
        }
        out.push(ch);
        last = ch;
        skipall = false;
    }
    if last == u16::from(b' ') {
        out.pop();
    }
    out
}

pub(crate) fn utf16(units: &[u16]) -> String {
    String::from_utf16_lossy(units)
}

/// Parses a `.csf` file.
pub fn parse(data: &[u8]) -> Result<StringTable, Error> {
    let mut r = Reader { data, pos: 0 };
    let header = r.bytes(HEADER_LEN)?;
    if &header[..4] != CSF_ID {
        return Err(Error::BadMagic);
    }
    let version = i32::from_le_bytes(header[4..8].try_into().expect("4 bytes"));
    let num_labels = i32::from_le_bytes(header[8..12].try_into().expect("4 bytes"));
    let langid = i32::from_le_bytes(header[20..24].try_into().expect("4 bytes"));
    let language_id = (version >= 2).then_some(langid as u32);

    // `num_labels` is only a capacity hint; every label needs at least 12 bytes.
    let cap = usize::try_from(num_labels).unwrap_or(0).min(data.len() / 12);
    let mut entries = Vec::with_capacity(cap);
    // The engine reads labels until it runs out of data.
    while data.len() - r.pos >= 4 {
        let at = r.pos;
        if &r.tag()? != CSF_LABEL {
            return Err(Error::BadLabelTag { offset: at });
        }
        let num_strings = r.len()?;
        let label_len = r.len()?;
        let label = latin1(r.bytes(label_len)?);

        let mut text = String::new();
        let mut speech = None;
        for n in 0..num_strings {
            let at = r.pos;
            let tag = r.tag()?;
            if &tag != CSF_STRING && &tag != CSF_STRINGWITHWAVE {
                return Err(Error::BadStringTag { offset: at });
            }
            let units = r.len()?;
            let raw = r.bytes(units.checked_mul(2).ok_or(Error::BadLength { offset: at })?)?;
            if n == 0 {
                // The engine negates up to the first zero unit, which also ends the string.
                let wide: Vec<u16> = raw
                    .chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .take_while(|&u| u != 0)
                    .map(|u| !u)
                    .collect();
                text = utf16(&strip_spaces(&wide));
            }
            if &tag == CSF_STRINGWITHWAVE {
                let wave_len = r.len()?;
                let wave = r.bytes(wave_len)?;
                if n == 0 && wave_len > 0 {
                    let w = wave.split(|&b| b == 0).next().unwrap_or(&[]);
                    speech = Some(latin1(w));
                }
            }
        }
        // The engine's AsciiString ends at the first NUL.
        let label = label.split('\0').next().unwrap_or_default().to_owned();
        entries.push(Entry { label, text, speech });
    }
    debug_assert!(r.at_end() || data.len() - r.pos < 4);
    Ok(StringTable::from_entries(entries, language_id))
}
