//! Asset access for the original game installs.
//!
//! Nothing from the games is redistributed: every byte is read from the
//! player's own Zero Hour and Renegade installs at runtime.

pub mod archive;
pub mod compression;
pub mod image;
pub mod install;
pub mod vfs;

pub use archive::{Archive, ArchiveKind};
pub use image::RgbaImage;
pub use install::{GameInstall, Installs};
pub use vfs::Vfs;

/// Normalizes an in-game path the way both engines compare them:
/// case-insensitive, with `\` as the separator.
pub fn normalize_path(path: &str) -> String {
    path.replace('/', "\\").to_ascii_lowercase()
}
