use std::fmt;

/// Failure to load a model or texture.
#[derive(Debug)]
pub enum Error {
    /// The file is not in the VFS (carries the name or path tried).
    NotFound(String),
    /// The file is present but malformed or unsupported.
    Invalid(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotFound(s) => write!(f, "not found: {s}"),
            Error::Invalid(s) => write!(f, "invalid: {s}"),
        }
    }
}

impl std::error::Error for Error {}

pub(crate) fn invalid(s: impl Into<String>) -> Error {
    Error::Invalid(s.into())
}
