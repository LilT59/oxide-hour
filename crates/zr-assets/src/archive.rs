//! Memory-mapped `.big` (SAGE) and `MIX1` (Renegade) archives.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

use memmap2::Mmap;

use crate::normalize_path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveKind {
    /// EA `BIGF`/`BIG4` archive used by Generals / Zero Hour.
    Big,
    /// Renegade `MIX1` archive (`.mix`, `.dat`). Unrelated to the classic C&C MIX format.
    Mix1,
}

#[derive(Debug, Clone)]
pub struct Entry {
    /// Path as stored in the archive.
    pub name: String,
    pub offset: u64,
    pub size: u64,
}

pub struct Archive {
    path: PathBuf,
    kind: ArchiveKind,
    map: Mmap,
    entries: Vec<Entry>,
}

impl Archive {
    /// Opens an archive, detecting its format from the magic bytes.
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = File::open(path)?;
        // SAFETY: the game files are treated as read-only; if another process
        // truncates them while mapped we may fault, which is acceptable for a
        // game data directory.
        let map = unsafe { Mmap::map(&file)? };
        let (kind, entries) = match map.get(..4) {
            Some(b"BIGF") | Some(b"BIG4") => (ArchiveKind::Big, parse_big(&map)?),
            Some(b"MIX1") => (ArchiveKind::Mix1, parse_mix1(&map)?),
            _ => return Err(invalid(format!("{}: unknown archive magic", path.display()))),
        };
        for e in &entries {
            if e.offset.checked_add(e.size).is_none_or(|end| end > map.len() as u64) {
                return Err(invalid(format!("{}: entry {} out of bounds", path.display(), e.name)));
            }
        }
        Ok(Self { path: path.to_owned(), kind, map, entries })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn kind(&self) -> ArchiveKind {
        self.kind
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn data(&self, index: usize) -> &[u8] {
        let e = &self.entries[index];
        &self.map[e.offset as usize..(e.offset + e.size) as usize]
    }
}

fn invalid(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

fn parse_big(data: &[u8]) -> io::Result<Vec<Entry>> {
    let big = cnc_formats::big::BigArchive::parse(data).map_err(|e| invalid(e.to_string()))?;
    Ok(big
        .entries()
        .iter()
        .map(|e| Entry { name: e.name.clone(), offset: e.offset, size: e.size })
        .collect())
}

/// Layout (little-endian), from Renegade's `wwlib/mixfile.cpp`:
/// `"MIX1"`, `u32 header_offset`, `u32 names_offset`, `u32 reserved`;
/// at `header_offset`: `u32 count`, then `count × {crc, offset, size}` sorted by CRC;
/// at `names_offset`: `u32 count`, then `count × {u8 len, name[len]}` (NUL-terminated,
/// same order as the info table).
fn parse_mix1(data: &[u8]) -> io::Result<Vec<Entry>> {
    let mut r = Reader { data, pos: 4 };
    let header_offset = r.u32()? as usize;
    let names_offset = r.u32()? as usize;

    r.pos = header_offset;
    let count = r.u32()? as usize;
    let mut infos = Vec::with_capacity(count.min(1 << 20));
    for _ in 0..count {
        let _crc = r.u32()?;
        let offset = r.u32()? as u64;
        let size = r.u32()? as u64;
        infos.push((offset, size));
    }

    r.pos = names_offset;
    let name_count = r.u32()? as usize;
    if name_count != count {
        return Err(invalid(format!("MIX1 name count {name_count} != file count {count}")));
    }
    let mut entries = Vec::with_capacity(count);
    for (offset, size) in infos {
        let len = r.u8()? as usize;
        let raw = r.bytes(len)?;
        let raw = raw.strip_suffix(&[0]).unwrap_or(raw);
        let name = String::from_utf8_lossy(raw).into_owned();
        entries.push(Entry { name, offset, size });
    }
    Ok(entries)
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn bytes(&mut self, n: usize) -> io::Result<&'a [u8]> {
        let out = self
            .data
            .get(self.pos..self.pos + n)
            .ok_or_else(|| invalid("MIX1: unexpected end of file".into()))?;
        self.pos += n;
        Ok(out)
    }

    fn u8(&mut self) -> io::Result<u8> {
        Ok(self.bytes(1)?[0])
    }

    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }
}

impl Entry {
    pub fn normalized_name(&self) -> String {
        normalize_path(&self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_mix1(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = b"MIX1".to_vec();
        out.extend_from_slice(&[0; 12]);
        let mut infos = Vec::new();
        for (name, body) in files {
            infos.push((name, out.len() as u32, body.len() as u32));
            out.extend_from_slice(body);
        }
        let header_offset = out.len() as u32;
        out.extend_from_slice(&(files.len() as u32).to_le_bytes());
        for (_, off, size) in &infos {
            out.extend_from_slice(&0u32.to_le_bytes());
            out.extend_from_slice(&off.to_le_bytes());
            out.extend_from_slice(&size.to_le_bytes());
        }
        let names_offset = out.len() as u32;
        out.extend_from_slice(&(files.len() as u32).to_le_bytes());
        for (name, _, _) in &infos {
            out.push(name.len() as u8 + 1);
            out.extend_from_slice(name.as_bytes());
            out.push(0);
        }
        out[4..8].copy_from_slice(&header_offset.to_le_bytes());
        out[8..12].copy_from_slice(&names_offset.to_le_bytes());
        out
    }

    #[test]
    fn mix1_roundtrip() {
        let data = build_mix1(&[("a.w3d", b"hello"), ("b.tga", b"xy")]);
        let entries = parse_mix1(&data).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "a.w3d");
        let e = &entries[1];
        assert_eq!(&data[e.offset as usize..(e.offset + e.size) as usize], b"xy");
    }

    #[test]
    fn mix1_truncated_is_error() {
        let data = build_mix1(&[("a.w3d", b"hello")]);
        assert!(parse_mix1(&data[..data.len() - 3]).is_err());
    }
}
