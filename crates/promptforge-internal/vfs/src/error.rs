//! The error type shared by every VFS layer.

use std::fmt;

/// The one error type returned by every virtual filesystem operation.
///
/// `#[non_exhaustive]` so new kinds can ship without breaking match arms
/// in downstream crates; the public surface of this crate is load-bearing.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VfsError {
    /// The path does not exist in the serving backend.
    NotFound(String),
    /// The operation is not permitted: a read-only mount or a policy denial.
    PermissionDenied(String),
    /// The path already exists where creation required absence.
    AlreadyExists(String),
    /// The path is malformed or escapes the virtual namespace root.
    InvalidPath(String),
    /// A directory operation named a non-directory.
    NotADirectory(String),
    /// A file operation named a directory.
    IsADirectory(String),
    /// A directory removal without `recursive` named a non-empty directory.
    DirectoryNotEmpty(String),
    /// The serving backend does not implement the operation.
    Unsupported(String),
    /// The operation conflicts with another live identity's claim.
    Conflict(String),
    /// The serving backend failed for any other reason.
    Backend(String),
}

impl fmt::Display for VfsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(m) => write!(f, "not found: {m}"),
            Self::PermissionDenied(m) => write!(f, "permission denied: {m}"),
            Self::AlreadyExists(m) => write!(f, "already exists: {m}"),
            Self::InvalidPath(m) => write!(f, "invalid path: {m}"),
            Self::NotADirectory(m) => write!(f, "not a directory: {m}"),
            Self::IsADirectory(m) => write!(f, "is a directory: {m}"),
            Self::DirectoryNotEmpty(m) => write!(f, "directory not empty: {m}"),
            Self::Unsupported(m) => write!(f, "unsupported operation: {m}"),
            Self::Conflict(m) => write!(f, "conflicting claim: {m}"),
            Self::Backend(m) => write!(f, "backend failure: {m}"),
        }
    }
}

impl std::error::Error for VfsError {}
