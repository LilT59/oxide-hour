//! A read-only virtual file system matching each engine's lookup rules.

use std::borrow::Cow;
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use crate::archive::Archive;
use crate::install::GameInstall;
use crate::normalize_path;

pub struct Vfs {
    /// Loose-file root; loose files shadow archived ones in both engines.
    loose_root: Option<PathBuf>,
    archives: Vec<Archive>,
    /// normalized path -> (archive index, entry index); first insert wins.
    index: HashMap<String, (usize, usize)>,
}

impl Vfs {
    pub fn empty() -> Self {
        Self { loose_root: None, archives: Vec::new(), index: HashMap::new() }
    }

    /// Mirrors `Win32BIGFileSystem::init`: every `*.big` under the install
    /// (recursively, case-insensitive alphabetical order), then the base
    /// Generals archives. The first archive to provide a path wins.
    pub fn zero_hour(install: &GameInstall) -> io::Result<Self> {
        let mut vfs = Self::empty();
        vfs.loose_root = Some(install.root.clone());
        let mut bigs = find_files(&install.root, "big")?;
        // TheSuperHackers fix: some SKUs ship a duplicate Data\INI\INIZH.big.
        bigs.retain(|rel| !rel.eq_ignore_ascii_case(r"data\ini\inizh.big"));
        for rel in bigs {
            vfs.mount(&install.root.join(rel))?;
        }
        if let Some(base) = install.generals_base_dir() {
            if !base.starts_with(&install.root) {
                for rel in find_files(&base, "big")? {
                    vfs.mount(&base.join(rel))?;
                }
            }
        }
        Ok(vfs)
    }

    /// Renegade: loose files in `Data\`, then `always.dat`, `always2.dat`
    /// and the per-map `.mix` files.
    pub fn renegade(install: &GameInstall) -> io::Result<Self> {
        let data = install.root.join("Data");
        let mut vfs = Self::empty();
        vfs.loose_root = Some(data.clone());
        let mut archives = find_files(&data, "dat")?;
        archives.extend(find_files(&data, "mix")?);
        // always*.dat hold the shared content; mount those before map packs.
        archives.sort_by_key(|rel| (!rel.starts_with("always"), rel.clone()));
        for rel in archives {
            vfs.mount(&data.join(rel))?;
        }
        Ok(vfs)
    }

    pub fn mount(&mut self, path: &Path) -> io::Result<()> {
        let archive = Archive::open(path)?;
        let ai = self.archives.len();
        for (ei, e) in archive.entries().iter().enumerate() {
            self.index.entry(e.normalized_name()).or_insert((ai, ei));
        }
        self.archives.push(archive);
        Ok(())
    }

    pub fn archives(&self) -> &[Archive] {
        &self.archives
    }

    /// Every archived path, normalized, with its providing archive.
    pub fn files(&self) -> impl Iterator<Item = (&str, &Archive)> {
        self.index.iter().map(|(k, &(ai, _))| (k.as_str(), &self.archives[ai]))
    }

    pub fn read(&self, path: &str) -> Option<Cow<'_, [u8]>> {
        if let Some(root) = &self.loose_root {
            let loose = root.join(path.replace('\\', "/"));
            if let Ok(bytes) = std::fs::read(loose) {
                return Some(Cow::Owned(bytes));
            }
        }
        self.read_archived(path).map(Cow::Borrowed)
    }

    pub fn read_archived(&self, path: &str) -> Option<&[u8]> {
        let &(ai, ei) = self.index.get(&normalize_path(path))?;
        Some(self.archives[ai].data(ei))
    }
}

/// Relative, lowercased paths of files with `ext`, sorted the way the engine's
/// case-insensitive `std::set` orders them.
fn find_files(root: &Path, ext: &str) -> io::Result<Vec<String>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let path = entry?.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext)) {
                let rel = path.strip_prefix(root).unwrap().to_string_lossy();
                out.push(normalize_path(&rel));
            }
        }
    }
    out.sort();
    Ok(out)
}
