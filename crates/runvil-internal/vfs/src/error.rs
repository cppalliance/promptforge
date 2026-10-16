//! The error types shared by every VFS layer: [`VfsError`], the one
//! error every operation returns, and [`PathReason`], why a path or
//! glob pattern was refused before any backend saw it.

use std::fmt;

#[path = "error-reason.rs"]
mod reason;

pub use reason::{ParsePathReasonError, PathReason};

/// The single error type returned by every virtual filesystem operation.
///
/// Each variant is one kind of failure and carries its details in public
/// fields. A caller matches on the variant and reads the fields directly.
/// A custom backend builds the variants directly as struct literals.
///
/// The enum is `#[non_exhaustive]` so that downstream crates keep
/// compiling when new variants are added. A `match` on it outside this
/// crate needs a wildcard arm.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum VfsError {
    /// The path is absent from the backend that serves it.
    NotFound {
        /// The canonical path that failed to resolve.
        path: String,
    },
    /// The path already exists where creation required absence.
    AlreadyExists {
        /// The canonical path that was already present.
        path: String,
    },
    /// A directory operation named a non-directory.
    NotADirectory {
        /// The canonical path that is not a directory.
        path: String,
    },
    /// A file operation named a directory.
    IsADirectory {
        /// The canonical path that is a directory.
        path: String,
    },
    /// A directory removal with `recursive` set to `false` named a directory
    /// that still has entries.
    DirectoryNotEmpty {
        /// The canonical path of the directory that still has entries.
        path: String,
    },
    /// Bytes that are invalid UTF-8 appeared where UTF-8 text was required:
    /// in a file's contents, or in the name of a real file or directory.
    NotUtf8 {
        /// The file whose contents are invalid UTF-8, or the directory
        /// holding a name that is.
        path: String,
        /// The length in bytes of the longest valid UTF-8 prefix of the
        /// contents or the name.
        valid_up_to: usize,
    },
    /// The path or glob pattern is malformed or escapes the namespace root.
    InvalidPath {
        /// The rejected path or pattern, exactly as supplied.
        path: String,
        /// The validation rule the path or pattern broke.
        reason: PathReason,
    },
    /// A line range was rejected.
    InvalidRange {
        /// The path the read targeted.
        path: String,
        /// A short human-readable reason the range was rejected.
        reason: &'static str,
    },
    /// A `str_replace` edit failed because its anchor text was empty, absent
    /// from the file, or found more than once.
    Anchor {
        /// The path the edit targeted.
        path: String,
        /// The anchor text. Empty means the anchor was itself invalid and
        /// was refused before any search.
        anchor: String,
        /// The number of times the anchor matched: `0` when it was absent,
        /// and `2` or more when the edit would be ambiguous.
        count: usize,
    },
    /// The handle's policy denied the operation.
    PermissionDenied {
        /// The canonical path the operation targeted.
        path: String,
        /// The policy's verdict text, naming the rule that fired.
        reason: String,
    },
    /// The handle's policy asked for the user's approval, which this layer
    /// never grants, so the operation was refused.
    ApprovalRequired {
        /// The canonical path the operation targeted.
        path: String,
        /// The policy's approval text: what is asked and which rule fired.
        reason: String,
    },
    /// The run or the tool call that owned the access has ended, so the
    /// access refuses every later operation.
    ScopeEnded {
        /// The canonical path the operation targeted.
        path: String,
    },
    /// The mount or backend that serves the path is read-only.
    ReadOnly {
        /// The canonical path the mutation targeted.
        path: String,
    },
    /// A rooted backend refused a path that resolves outside its root
    /// directory or passes through a dangling link.
    OutsideRoot {
        /// The canonical path the operation targeted.
        path: String,
        /// How the path left the root.
        detail: String,
    },
    /// The operation would remove or rename the mounted root, or rename
    /// a path onto it.
    RootProtected {
        /// The root the operation would have replaced or removed.
        path: String,
    },
    /// A store view was requested from a handle that declares no store.
    NoStore,
    /// A two-path operation named paths that different mounts serve.
    CrossMount {
        /// The canonical source path.
        path: String,
        /// The operation and both paths.
        detail: String,
    },
    /// An operating-system I/O call failed for a reason outside the
    /// other variants.
    Io {
        /// The canonical path the operation targeted.
        path: String,
        /// The kind of the I/O failure.
        kind: std::io::ErrorKind,
        /// The operating system's error code, when it reported one.
        raw_os_error: Option<i32>,
        /// The operating system's description of the failure.
        message: String,
    },
    /// The backend that serves the path lacks an implementation of the
    /// operation.
    Unsupported {
        /// The canonical path the operation targeted.
        path: String,
        /// What the backend lacks and why.
        detail: String,
    },
    /// The operation conflicts with another access's claim on the same
    /// path or pattern.
    ///
    /// Two overlapping claims conflict when at least one of them is a write
    /// and the two are concurrent. Two claims are concurrent when the other
    /// claim was made by an identity in another live scope, or in the same
    /// scope at an epoch later than the last one the operation's clock has
    /// seen.
    Conflict {
        /// The canonical path or pattern both accesses claimed.
        path: String,
        /// A description of the conflict that names both identities and
        /// both claim kinds.
        detail: String,
    },
    /// The backend failed for a reason outside the other variants and
    /// outside any I/O call.
    Backend {
        /// The backend's own diagnosis.
        message: String,
    },
}

impl VfsError {
    /// Returns the stable tag for this variant, such as `not_found` or
    /// `read_only`.
    ///
    /// A store error value holds this tag in its `reason` field, so prompt
    /// code branches on it.
    #[must_use]
    pub fn tag(&self) -> &'static str {
        match self {
            Self::NotFound { .. } => "not_found",
            Self::AlreadyExists { .. } => "already_exists",
            Self::NotADirectory { .. } => "not_a_directory",
            Self::IsADirectory { .. } => "is_a_directory",
            Self::DirectoryNotEmpty { .. } => "directory_not_empty",
            Self::NotUtf8 { .. } => "not_utf8",
            Self::InvalidPath { .. } => "invalid_path",
            Self::InvalidRange { .. } => "invalid_range",
            Self::Anchor { .. } => "anchor",
            Self::PermissionDenied { .. } => "permission_denied",
            Self::ApprovalRequired { .. } => "approval_required",
            Self::ScopeEnded { .. } => "scope_ended",
            Self::ReadOnly { .. } => "read_only",
            Self::OutsideRoot { .. } => "outside_root",
            Self::RootProtected { .. } => "root_protected",
            Self::NoStore => "no_store",
            Self::CrossMount { .. } => "cross_mount",
            Self::Io { .. } => "io",
            Self::Unsupported { .. } => "unsupported",
            Self::Conflict { .. } => "conflict",
            Self::Backend { .. } => "backend",
        }
    }
}

impl fmt::Display for VfsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { path } => write!(f, "not found: {path}"),
            Self::AlreadyExists { path } => write!(f, "already exists: {path}"),
            Self::NotADirectory { path } => write!(f, "not a directory: {path}"),
            Self::IsADirectory { path } => write!(f, "is a directory: {path}"),
            Self::DirectoryNotEmpty { path } => write!(f, "directory not empty: {path}"),
            Self::NotUtf8 { path, valid_up_to } => {
                write!(f, "not UTF-8: {path}, valid up to byte {valid_up_to}")
            }
            Self::InvalidPath { path, reason } => write!(f, "invalid path {path:?}: {reason}"),
            Self::InvalidRange { path, reason } => {
                write!(f, "invalid line range for {path}: {reason}")
            }
            Self::Anchor {
                path,
                anchor,
                count,
            } => {
                if anchor.is_empty() {
                    write!(f, "str_replace requires a non-empty anchor: {path}")
                } else if *count == 0 {
                    write!(
                        f,
                        "anchor {anchor:?} was not found in {path}, expected exactly one"
                    )
                } else {
                    write!(
                        f,
                        "anchor {anchor:?} occurs {count} times in {path}, expected exactly one"
                    )
                }
            }
            Self::PermissionDenied { reason, .. } => write!(f, "permission denied: {reason}"),
            Self::ApprovalRequired { reason, .. } => write!(f, "approval required: {reason}"),
            Self::ScopeEnded { path } => write!(
                f,
                "access ended: the run or tool call that owned this access has ended, \
                 so {path} can no longer be touched through it"
            ),
            Self::ReadOnly { path } => {
                write!(f, "read-only: {path} is served read-only and cannot change")
            }
            Self::OutsideRoot { detail, .. } => write!(f, "outside the mounted root: {detail}"),
            Self::RootProtected { path } => write!(
                f,
                "root protected: the mounted root {path} cannot be removed, renamed, or replaced"
            ),
            Self::NoStore => f.write_str("no store: the handle declares no store"),
            Self::CrossMount { detail, .. } => write!(f, "across mounts: {detail}"),
            Self::Io { path, message, .. } => write!(f, "I/O failure on {path}: {message}"),
            Self::Unsupported { detail, .. } => write!(f, "unsupported operation: {detail}"),
            Self::Conflict { detail, .. } => write!(f, "conflicting claim: {detail}"),
            Self::Backend { message } => write!(f, "backend failure: {message}"),
        }
    }
}

impl std::error::Error for VfsError {}

#[cfg(test)]
#[path = "error-tests.rs"]
mod tests;
