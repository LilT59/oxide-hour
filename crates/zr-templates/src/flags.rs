//! Index-list enums and the bit-flag masks built from them.
//!
//! The engine parses most enumerations with `INI::scanIndexList` (case-insensitive, `stricmp`) and
//! flag sets with `BitFlags::parse` / `INI::parseBitString32`. Both are mirrored here.

use std::fmt;
use std::marker::PhantomData;

/// An enum parsed from an engine name table: the discriminant is the table index.
pub trait NameEnum: Copy + 'static {
    const NAMES: &'static [&'static str];
    fn index(self) -> usize;
    fn from_index(i: usize) -> Self;

    fn name(self) -> &'static str {
        Self::NAMES[self.index()]
    }

    /// `INI::scanIndexList`: case-insensitive lookup.
    fn from_name(s: &str) -> Option<Self> {
        Self::NAMES
            .iter()
            .position(|n| n.eq_ignore_ascii_case(s))
            .map(Self::from_index)
    }

    fn count() -> usize {
        Self::NAMES.len()
    }
}

macro_rules! name_enum {
    ($(#[$m:meta])* pub enum $E:ident { $($V:ident = $s:literal,)+ }) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[repr(u8)]
        pub enum $E { $($V),+ }

        impl $E {
            /// Every value, in table order.
            pub const ALL: &'static [$E] = &[$($E::$V),+];
        }

        impl $crate::flags::NameEnum for $E {
            const NAMES: &'static [&'static str] = &[$($s),+];
            fn index(self) -> usize { self as usize }
            fn from_index(i: usize) -> Self { Self::ALL[i] }
        }
    };
}
pub(crate) use name_enum;

/// A set of `E` stored as bits (index = bit), like the engine's `BitFlags<N>`.
pub struct Mask<E>(u128, PhantomData<E>);

impl<E> Clone for Mask<E> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<E> Copy for Mask<E> {}
impl<E> Default for Mask<E> {
    fn default() -> Self {
        Mask(0, PhantomData)
    }
}
impl<E> PartialEq for Mask<E> {
    fn eq(&self, o: &Self) -> bool {
        self.0 == o.0
    }
}
impl<E> Eq for Mask<E> {}
impl<E> std::hash::Hash for Mask<E> {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        self.0.hash(h)
    }
}
impl<E> PartialOrd for Mask<E> {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl<E> Ord for Mask<E> {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        self.0.cmp(&o.0)
    }
}

impl<E: NameEnum> Mask<E> {
    pub const EMPTY: Self = Mask(0, PhantomData);

    pub fn from_bits(bits: u128) -> Self {
        Mask(bits, PhantomData)
    }
    pub fn bits(self) -> u128 {
        self.0
    }
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
    pub fn contains(self, e: E) -> bool {
        self.0 >> e.index() & 1 != 0
    }
    pub fn insert(&mut self, e: E) {
        self.0 |= 1 << e.index();
    }
    pub fn remove(&mut self, e: E) {
        self.0 &= !(1 << e.index());
    }
    pub fn with(mut self, e: E) -> Self {
        self.insert(e);
        self
    }
    /// `anyIntersectionWith`.
    pub fn intersects(self, o: Self) -> bool {
        self.0 & o.0 != 0
    }
    pub fn iter(self) -> impl Iterator<Item = E> {
        (0..E::count()).filter(move |&i| self.0 >> i & 1 != 0).map(E::from_index)
    }

    /// `BitFlags::parse` / `INI::parseBitString32`: `NONE` clears and stops, `+X`/`-X` edit the
    /// current value, bare names replace it; mixing the two forms is an error. The slice is
    /// everything after the key.
    pub fn parse(self, toks: &[&str]) -> Result<Self, String> {
        let mut bits = self.0;
        let (mut normal, mut addsub) = (false, false);
        for t in toks {
            if t.eq_ignore_ascii_case("NONE") {
                if normal || addsub {
                    return Err("may not mix normal and +- ops in bitstring lists".into());
                }
                bits = 0;
                break;
            }
            let lookup = |n: &str| E::from_name(n).ok_or_else(|| format!("`{n}` is not a valid member of the index list"));
            if let Some(n) = t.strip_prefix('+') {
                if normal {
                    return Err("may not mix normal and +- ops in bitstring lists".into());
                }
                bits |= 1 << lookup(n)?.index();
                addsub = true;
            } else if let Some(n) = t.strip_prefix('-') {
                if normal {
                    return Err("may not mix normal and +- ops in bitstring lists".into());
                }
                bits &= !(1 << lookup(n)?.index());
                addsub = true;
            } else {
                if addsub {
                    return Err("may not mix normal and +- ops in bitstring lists".into());
                }
                if !normal {
                    bits = 0;
                }
                bits |= 1 << lookup(t)?.index();
                normal = true;
            }
        }
        Ok(Mask(bits, PhantomData))
    }
}

impl<E: NameEnum> fmt::Debug for Mask<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.iter().map(NameEnum::name)).finish()
    }
}

impl<E: NameEnum> FromIterator<E> for Mask<E> {
    fn from_iter<I: IntoIterator<Item = E>>(it: I) -> Self {
        let mut m = Mask::EMPTY;
        for e in it {
            m.insert(e);
        }
        m
    }
}

pub type KindOfMask = Mask<crate::names::KindOf>;
pub type WeaponSetFlags = Mask<crate::names::WeaponSetType>;
pub type ArmorSetFlags = Mask<crate::names::ArmorSetType>;
