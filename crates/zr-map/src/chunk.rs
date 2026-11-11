//! DataChunk container format.
//!
//! Source: `Generals*/Code/GameEngine/Source/Common/System/DataChunk.cpp`
//! (`DataChunkTableOfContents::read`, `DataChunkInput::openDataChunk`, `readDict`, ...).
//!
//! Layout (little endian):
//! - `"CkMp"`, `i32 count`, then `count` x (`u8 len`, name bytes, `u32 id`)
//! - chunks until EOF: `u32 id`, `u16 version`, `i32 size`, `size` bytes of payload.
//!
//! Whether a chunk contains sub-chunks is decided by its consumer (the C++ calls
//! `file.parse()` inside the callback), not by the container, so nesting is exposed
//! lazily via [`Chunk::children`] / [`Cursor::chunks`].

use std::collections::HashMap;
use std::fmt;

/// Size of a chunk header: id (4) + version (2) + data size (4).
pub const HEADER_BYTES: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChunkError {
    /// Missing `CkMp` tag (the C++ silently treats this as a legacy, unparseable file).
    NotChunky,
    /// A read ran past the end of the available data.
    Eof { wanted: usize, left: usize },
    /// Negative size or other structurally invalid value.
    Corrupt(&'static str),
    /// A chunk id has no entry in the name table.
    UnknownId(u32),
}

impl fmt::Display for ChunkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotChunky => write!(f, "missing CkMp tag"),
            Self::Eof { wanted, left } => write!(f, "read of {wanted} bytes with only {left} left"),
            Self::Corrupt(m) => write!(f, "corrupt data: {m}"),
            Self::UnknownId(id) => write!(f, "chunk id {id} not in name table"),
        }
    }
}
impl std::error::Error for ChunkError {}

pub type Result<T> = std::result::Result<T, ChunkError>;

/// Name table (`DataChunkTableOfContents`): id -> name.
#[derive(Debug, Clone, Default)]
pub struct Names(HashMap<u32, String>);

impl Names {
    /// Name for `id`; `None` if absent.
    pub fn get(&self, id: u32) -> Option<&str> {
        self.0.get(&id).map(String::as_str)
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    /// Builds a table from `(id, name)` pairs.
    pub fn from_pairs<'a>(pairs: impl IntoIterator<Item = (u32, &'a str)>) -> Self {
        Self(pairs.into_iter().map(|(i, n)| (i, n.to_owned())).collect())
    }
}

/// A chunk header plus its payload slice.
#[derive(Debug, Clone, Copy)]
pub struct Chunk<'a> {
    pub id: u32,
    pub name: &'a str,
    pub version: u16,
    pub data: &'a [u8],
}

impl<'a> Chunk<'a> {
    /// Cursor over the payload.
    pub fn cursor(&self, names: &'a Names) -> Cursor<'a> {
        Cursor::new(self.data, names)
    }
    /// Treats the whole payload as a sequence of sub-chunks (e.g. `ObjectsList`).
    pub fn children(&self, names: &'a Names) -> ChunkIter<'a> {
        self.cursor(names).chunks()
    }
}

/// Parses the header table; returns the name table and the bytes after it
/// (the top-level chunk stream).
pub fn open(data: &[u8]) -> Result<(Names, &[u8])> {
    let none = Names::default();
    let mut c = Cursor::new(data, &none);
    if c.bytes(4).map_err(|_| ChunkError::NotChunky)? != b"CkMp" {
        return Err(ChunkError::NotChunky);
    }
    let count = c.i32()?;
    if count < 0 {
        return Err(ChunkError::Corrupt("negative name count"));
    }
    let mut map = HashMap::new();
    for _ in 0..count {
        let len = c.u8()? as usize;
        let name = latin1(c.bytes(len)?);
        let id = c.u32()?;
        map.insert(id, name);
    }
    let left = c.left();
    Ok((Names(map), &data[data.len() - left..]))
}

fn latin1(b: &[u8]) -> String {
    b.iter().map(|&c| c as char).collect()
}

/// Bounds-checked reader over a chunk payload (the `read*` family of `DataChunkInput`).
#[derive(Debug, Clone)]
pub struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
    names: &'a Names,
}

impl<'a> Cursor<'a> {
    pub fn new(data: &'a [u8], names: &'a Names) -> Self {
        Self { data, pos: 0, names }
    }
    pub fn left(&self) -> usize {
        self.data.len() - self.pos
    }
    pub fn at_end(&self) -> bool {
        self.left() == 0
    }
    /// Unread remainder.
    pub fn rest(&self) -> &'a [u8] {
        &self.data[self.pos..]
    }
    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.left() {
            return Err(ChunkError::Eof { wanted: n, left: self.left() });
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let mut a = [0; N];
        a.copy_from_slice(self.bytes(N)?);
        Ok(a)
    }
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.array::<1>()?[0])
    }
    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.array()?))
    }
    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.array()?))
    }
    /// `readInt`.
    pub fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.array()?))
    }
    /// `readReal`.
    pub fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_le_bytes(self.array()?))
    }
    /// `readByte` interpreted as a flag.
    pub fn bool(&mut self) -> Result<bool> {
        Ok(self.u8()? != 0)
    }
    /// `readAsciiString`: `u16 len`, then `len` bytes.
    pub fn ascii(&mut self) -> Result<String> {
        let len = self.u16()? as usize;
        Ok(latin1(self.bytes(len)?))
    }
    /// `readUnicodeString`: `u16 len` in UTF-16 code units.
    pub fn unicode(&mut self) -> Result<String> {
        let len = self.u16()? as usize;
        let raw = self.bytes(len * 2)?;
        let units = raw.chunks_exact(2).map(|p| u16::from_le_bytes([p[0], p[1]]));
        Ok(char::decode_utf16(units).map(|r| r.unwrap_or('\u{FFFD}')).collect())
    }
    /// `readDict`: `u16 n`, then `n` x (`u32 keyAndType`, value). Key id = `>> 8`, type = low byte
    /// (0 bool(u8), 1 int, 2 real, 3 ascii, 4 unicode; see `Dict.h`). Unknown key ids yield `""`
    /// like `DataChunkTableOfContents::getName`.
    pub fn dict(&mut self) -> Result<Dict> {
        let n = self.u16()?;
        let mut d = Dict::default();
        for _ in 0..n {
            let kt = self.u32()?;
            let key = self.names.get(kt >> 8).unwrap_or("").to_owned();
            let v = match kt & 0xff {
                0 => DictValue::Bool(self.bool()?),
                1 => DictValue::Int(self.i32()?),
                2 => DictValue::Real(self.f32()?),
                3 => DictValue::Ascii(self.ascii()?),
                4 => DictValue::Unicode(self.unicode()?),
                _ => return Err(ChunkError::Corrupt("unknown dict value type")),
            };
            d.0.push((key, v));
        }
        Ok(d)
    }
    /// `readNameKey`: an `i32` whose bits `>> 8` index the name table (low byte is the dict
    /// type tag, always ascii). `None` when the id is not in the table.
    pub fn name_key(&mut self) -> Result<Option<String>> {
        let kt = self.u32()?;
        Ok(self.names.get(kt >> 8).map(str::to_owned))
    }
    /// Sub-chunks in the remaining bytes.
    pub fn chunks(&self) -> ChunkIter<'a> {
        ChunkIter { cur: Cursor { data: self.rest(), pos: 0, names: self.names } }
    }
}

/// `Dict` value types (`Dict::DataType`).
#[derive(Debug, Clone, PartialEq)]
pub enum DictValue {
    Bool(bool),
    Int(i32),
    Real(f32),
    Ascii(String),
    Unicode(String),
}

/// Ordered key/value list as stored in the file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Dict(pub Vec<(String, DictValue)>);

impl Dict {
    /// Case-insensitive lookup (the engine's name keys are case-insensitive).
    pub fn get(&self, key: &str) -> Option<&DictValue> {
        self.0.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v)
    }
    pub fn int(&self, key: &str) -> Option<i32> {
        match self.get(key)? {
            DictValue::Int(i) => Some(*i),
            _ => None,
        }
    }
    pub fn str(&self, key: &str) -> Option<&str> {
        match self.get(key)? {
            DictValue::Ascii(s) | DictValue::Unicode(s) => Some(s),
            _ => None,
        }
    }
    /// Sets `key` to an ASCII value, replacing it or appending a new entry (case-insensitive).
    pub fn set_ascii(&mut self, key: &str, value: &str) {
        if let Some((_, v)) = self.0.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(key)) {
            *v = DictValue::Ascii(value.to_owned());
        } else {
            self.0.push((key.to_owned(), DictValue::Ascii(value.to_owned())));
        }
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Iterator over consecutive chunks; yields `Err` once on malformed data, then stops.
///
/// Mirrors `DataChunkInput::parse`: stops cleanly when fewer bytes than a header remain.
pub struct ChunkIter<'a> {
    cur: Cursor<'a>,
}

impl<'a> ChunkIter<'a> {
    /// Chunk stream over `data` with the given name table.
    pub fn new(data: &'a [u8], names: &'a Names) -> Self {
        Self { cur: Cursor::new(data, names) }
    }
}

impl<'a> Iterator for ChunkIter<'a> {
    type Item = Result<Chunk<'a>>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.cur.left() < HEADER_BYTES {
            return None;
        }
        let r = (|| {
            let id = self.cur.u32()?;
            let version = self.cur.u16()?;
            let size = usize::try_from(self.cur.i32()?).map_err(|_| ChunkError::Corrupt("negative chunk size"))?;
            let data = self.cur.bytes(size)?;
            let name = self.cur.names.get(id).ok_or(ChunkError::UnknownId(id))?;
            Ok(Chunk { id, name, version, data })
        })();
        if r.is_err() {
            self.cur.pos = self.cur.data.len();
        }
        Some(r)
    }
}

#[cfg(test)]
pub(crate) mod test_util {
    /// Builds a chunk file: name table + raw body bytes.
    pub fn file(names: &[(&str, u32)], body: &[u8]) -> Vec<u8> {
        let mut v = b"CkMp".to_vec();
        v.extend((names.len() as i32).to_le_bytes());
        for (n, id) in names {
            v.push(n.len() as u8);
            v.extend(n.as_bytes());
            v.extend(id.to_le_bytes());
        }
        v.extend(body);
        v
    }
    pub fn chunk(id: u32, ver: u16, data: &[u8]) -> Vec<u8> {
        let mut v = id.to_le_bytes().to_vec();
        v.extend(ver.to_le_bytes());
        v.extend((data.len() as i32).to_le_bytes());
        v.extend(data);
        v
    }
    pub fn ascii(s: &str) -> Vec<u8> {
        let mut v = (s.len() as u16).to_le_bytes().to_vec();
        v.extend(s.as_bytes());
        v
    }
    pub fn i32s(v: &[i32]) -> Vec<u8> {
        v.iter().flat_map(|i| i.to_le_bytes()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::test_util::*;
    use super::*;

    type RawChunk = (u32, u16, Vec<u8>);

    fn chunks(f: &[u8]) -> (Names, Vec<Result<RawChunk>>) {
        let (names, rest) = open(f).unwrap();
        let v = ChunkIter::new(rest, &names).map(|c| c.map(|c| (c.id, c.version, c.data.to_vec()))).collect();
        (names, v)
    }

    #[test]
    fn table_and_chunks() {
        let mut body = chunk(2, 3, &[1, 2, 3]);
        body.extend(chunk(1, 1, &[]));
        let (names, cs) = chunks(&file(&[("A", 1), ("B", 2)], &body));
        assert_eq!(names.get(2), Some("B"));
        assert_eq!(cs[0], Ok((2, 3, vec![1, 2, 3])));
        assert_eq!(cs[1], Ok((1, 1, vec![])));
        assert_eq!(cs.len(), 2);
    }

    #[test]
    fn bad_input_errors() {
        assert_eq!(open(b"xxxx").unwrap_err(), ChunkError::NotChunky);
        assert_eq!(open(b"Ck").unwrap_err(), ChunkError::NotChunky);
        assert!(open(b"CkMp\x05\0\0\0").is_err());
        // chunk claims more data than present
        let (_, cs) = chunks(&file(&[("A", 1)], &[1, 0, 0, 0, 1, 0, 100, 0, 0, 0, 9]));
        assert!(matches!(cs[0], Err(ChunkError::Eof { .. })));
        // negative size
        let (_, cs) = chunks(&file(&[("A", 1)], &[1, 0, 0, 0, 1, 0, 0xff, 0xff, 0xff, 0xff]));
        assert!(matches!(cs[0], Err(ChunkError::Corrupt(_))));
        // unknown id
        let (_, cs) = chunks(&file(&[("A", 1)], &chunk(9, 1, &[])));
        assert_eq!(cs[0], Err(ChunkError::UnknownId(9)));
    }

    #[test]
    fn dict_and_strings() {
        let names = Names::from_pairs([(1, "a"), (2, "b"), (3, "c"), (4, "d"), (5, "e")]);
        let mut b = 5u16.to_le_bytes().to_vec();
        b.extend((1u32 << 8).to_le_bytes());
        b.push(1);
        b.extend((2u32 << 8 | 1).to_le_bytes());
        b.extend((-7i32).to_le_bytes());
        b.extend((3u32 << 8 | 2).to_le_bytes());
        b.extend(1.5f32.to_le_bytes());
        b.extend((4u32 << 8 | 3).to_le_bytes());
        b.extend(ascii("hi"));
        b.extend((5u32 << 8 | 4).to_le_bytes());
        b.extend(1u16.to_le_bytes());
        b.extend(0xE9u16.to_le_bytes());
        let d = Cursor::new(&b, &names).dict().unwrap();
        assert_eq!(d.get("a"), Some(&DictValue::Bool(true)));
        assert_eq!(d.int("B"), Some(-7));
        assert_eq!(d.get("c"), Some(&DictValue::Real(1.5)));
        assert_eq!(d.str("d"), Some("hi"));
        assert_eq!(d.str("e"), Some("\u{e9}"));
        // truncated and bad type
        assert!(Cursor::new(&b[..b.len() - 1], &names).dict().is_err());
        let mut bad = 1u16.to_le_bytes().to_vec();
        bad.extend(9u32.to_le_bytes());
        assert!(Cursor::new(&bad, &names).dict().is_err());
    }
}
