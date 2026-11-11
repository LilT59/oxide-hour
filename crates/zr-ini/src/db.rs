//! A multi-file collection of blocks with last-definition-wins lookup.

use crate::{Block, Item};
use std::collections::HashMap;

/// Blocks collected from many INI files, in load order.
///
/// # Duplicate semantics (from the C++)
///
/// The engine keeps one instance per (type, name) and every later definition
/// is parsed *on top of* it: `ThingFactory::parseObjectDefinition`,
/// `parseLocomotorTemplateDefinition`, etc. find the existing instance and
/// run `initFromINI` again, so fields it mentions overwrite and fields it
/// omits survive. Only `Weapon` (`WeaponStore::parseWeaponTemplateDefinition`)
/// differs: outside override loads a duplicate is rejected. `CREATE_OVERRIDES`
/// loads (map-local `map.ini`) stack a new override instance, which also
/// resolves to "latest wins" per field.
///
/// This type therefore keeps *all* definitions: [`IniDb::get`] returns the
/// last whole block, [`IniDb::definitions`] every one in order, and
/// [`IniDb::resolved`] overlays them into a single block whose repeated
/// scalar keys resolve with [`Block::field`] (last wins).
///
/// `Object` and `ObjectReskin` share one namespace (both create a
/// `ThingTemplate`); names are case-sensitive (`NameKeyGenerator::nameToKey`
/// uses `strcmp`).
#[derive(Debug, Default)]
pub struct IniDb {
    blocks: Vec<Block>,
    index: HashMap<(String, String), Vec<usize>>,
}

/// Namespace under which a block kind is stored.
fn namespace(kind: &str) -> &str {
    if kind == "ObjectReskin" {
        "Object"
    } else {
        kind
    }
}

impl IniDb {
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends blocks (e.g. one parsed file) in load order.
    pub fn extend(&mut self, blocks: impl IntoIterator<Item = Block>) {
        for block in blocks {
            let key = (
                namespace(&block.kind).to_owned(),
                block.name().unwrap_or("").to_owned(),
            );
            self.index.entry(key).or_default().push(self.blocks.len());
            self.blocks.push(block);
        }
    }

    /// Marks the current end of the database for [`IniDb::rollback`].
    pub fn checkpoint(&self) -> usize {
        self.blocks.len()
    }

    /// Drops every block added after `checkpoint` (e.g. a map's `map.ini`
    /// overrides, which the engine discards when the map ends).
    pub fn rollback(&mut self, checkpoint: usize) {
        if checkpoint >= self.blocks.len() {
            return;
        }
        self.blocks.truncate(checkpoint);
        for v in self.index.values_mut() {
            v.retain(|&i| i < checkpoint);
        }
        self.index.retain(|_, v| !v.is_empty());
    }

    /// Every block in load order.
    pub fn iter(&self) -> impl Iterator<Item = &Block> {
        self.blocks.iter()
    }

    /// Number of blocks (counting every definition).
    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// Number of distinct names among blocks whose type is exactly `kind`.
    pub fn count_unique(&self, kind: &str) -> usize {
        let names: std::collections::HashSet<_> = self
            .blocks
            .iter()
            .filter(|b| b.kind == kind)
            .map(|b| b.name())
            .collect();
        names.len()
    }

    /// Latest definition of `kind name`. Unnamed blocks (`GameData`) use `""`.
    pub fn get(&self, kind: &str, name: &str) -> Option<&Block> {
        self.definitions(kind, name).last().copied()
    }

    /// All definitions of `kind name`, oldest first.
    pub fn definitions(&self, kind: &str, name: &str) -> Vec<&Block> {
        let key = (namespace(kind).to_owned(), name.to_owned());
        self.index
            .get(&key)
            .map_or_else(Vec::new, |v| v.iter().map(|&i| &self.blocks[i]).collect())
    }

    /// Like [`IniDb::definitions`], with each block's position in load order (comparable with a
    /// [`IniDb::checkpoint`], so a caller can tell base definitions from map overrides).
    pub fn definitions_indexed(&self, kind: &str, name: &str) -> Vec<(usize, &Block)> {
        let key = (namespace(kind).to_owned(), name.to_owned());
        self.index
            .get(&key)
            .map_or_else(Vec::new, |v| v.iter().map(|&i| (i, &self.blocks[i])).collect())
    }

    /// Latest definitions of every block of `kind`, in first-definition order.
    pub fn of_kind<'a>(&'a self, kind: &'a str) -> impl Iterator<Item = &'a Block> {
        let ns = namespace(kind);
        let mut seen = std::collections::HashSet::new();
        self.blocks
            .iter()
            .filter(move |b| namespace(&b.kind) == ns)
            .filter_map(move |b| {
                let name = b.name().unwrap_or("");
                seen.insert(name).then(|| self.get(kind, name)).flatten()
            })
    }

    /// All definitions of `kind name` overlaid in order into one block: the
    /// header and line come from the first, items are concatenated.
    pub fn resolved(&self, kind: &str, name: &str) -> Option<Block> {
        let defs = self.definitions(kind, name);
        let (first, rest) = defs.split_first()?;
        let mut merged = (*first).clone();
        for d in rest {
            merged.kind.clone_from(&d.kind);
            merged.items.extend(d.items.iter().cloned());
        }
        Some(merged)
    }

    /// Counts of blocks per type (all definitions), sorted by type name.
    pub fn counts(&self) -> Vec<(String, usize)> {
        let mut m: HashMap<&str, usize> = HashMap::new();
        for b in &self.blocks {
            *m.entry(b.kind.as_str()).or_default() += 1;
        }
        let mut v: Vec<_> = m.into_iter().map(|(k, n)| (k.to_owned(), n)).collect();
        v.sort();
        v
    }
}

impl Block {
    /// Total number of nested blocks below this one.
    pub fn descendant_count(&self) -> usize {
        self.items
            .iter()
            .map(|i| match i {
                Item::Block(b) => 1 + b.descendant_count(),
                Item::Field(_) => 0,
            })
            .sum()
    }
}
