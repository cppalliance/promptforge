//! The error types shared by every VFS layer: [`VfsError`], the one
//! error every operation returns, and [`PathReason`], why a path or
//! glob pattern was refused before any backend saw it.

use std::fmt;

/// Why a path or glob pattern was rejected before any backend saw it.
///
/// Every [`VfsError::InvalidPath`] carries one. Store operations validate
/// each path and report the first nine reasons. Only glob and rename
/// operations report the last two: [`PathReason::Wildcard`] for a glob
/// pattern whose wildcard grammar is invalid, and
/// [`PathReason::IntoDescendant`] for a rename into the source's own
/// descendant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PathReason {
    /// The path was empty or contained only separators.
    Empty,
    /// The path began with `/`, so it addressed outside the run's namespace.
    Absolute,
    /// The path contained a `.` (current directory) or `..` (parent
    /// directory) segment.
    Traversal,
    /// The path contained a control character (below `0x20`, or `0x7f`).
    Control,
    /// The path contained an empty segment (a `//` run, or a trailing `/`).
    EmptySegment,
    /// The path contained a backslash, which is ambiguous across backends (a
    /// literal byte to one, a separator to another).
    Backslash,
    /// A segment was a platform-reserved device name (for example `CON`,
    /// `NUL`, `COM1`), which some backends treat as the device itself.
    ReservedName,
    /// A segment ended in a byte some backends silently strip (a trailing `.`
    /// or space), so the stored name would differ from the name supplied.
    UnsafeSuffix,
    /// The path exceeded the maximum supported length in bytes.
    TooLong,
    /// The wildcard grammar of a glob pattern is invalid: a run of three or
    /// more `*`, or a `**` that shares its path segment with other characters.
    Wildcard,
    /// A rename named a destination inside the source's own subtree.
    IntoDescendant,
}

impl PathReason {
    /// Returns the short tag for this reason, such as `empty` or `too_long`.
    ///
    /// A store error value holds this tag in its `rule` field.
    /// [`PathReason::from_tag`] parses the same word back, so the two
    /// round-trip.
    #[must_use]
    pub fn tag(self) -> &'static str {
        match self {
            PathReason::Empty => "empty",
            PathReason::Absolute => "absolute",
            PathReason::Traversal => "traversal",
            PathReason::Control => "control",
            PathReason::EmptySegment => "empty_segment",
            PathReason::Backslash => "backslash",
            PathReason::ReservedName => "reserved_name",
            PathReason::UnsafeSuffix => "unsafe_suffix",
            PathReason::TooLong => "too_long",
            PathReason::Wildcard => "wildcard",
            PathReason::IntoDescendant => "into_descendant",
        }
    }

    /// Parses a tag from [`PathReason::tag`] back into its reason.
    ///
    /// Returns `None` for any other string.
    #[must_use]
    pub fn from_tag(tag: &str) -> Option<PathReason> {
        match tag {
            "empty" => Some(PathReason::Empty),
            "absolute" => Some(PathReason::Absolute),
            "traversal" => Some(PathReason::Traversal),
            "control" => Some(PathReason::Control),
            "empty_segment" => Some(PathReason::EmptySegment),
            "backslash" => Some(PathReason::Backslash),
            "reserved_name" => Some(PathReason::ReservedName),
            "unsafe_suffix" => Some(PathReason::UnsafeSuffix),
            "too_long" => Some(PathReason::TooLong),
            "wildcard" => Some(PathReason::Wildcard),
            "into_descendant" => Some(PathReason::IntoDescendant),
            _ => None,
        }
    }
}

impl fmt::Display for PathReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            PathReason::Empty => "path is empty",
            PathReason::Absolute => "path is absolute",
            PathReason::Traversal => "path contains a traversal segment",
            PathReason::Control => "path contains a control character",
            PathReason::EmptySegment => "path contains an empty segment",
            PathReason::Backslash => "path contains a backslash",
            PathReason::ReservedName => "path contains a reserved device name",
            PathReason::UnsafeSuffix => "path segment ends in an unsafe character",
            PathReason::TooLong => "path is too long",
            PathReason::Wildcard => "pattern contains invalid wildcard grammar",
            PathReason::IntoDescendant => {
                "a rename cannot move a directory into its own descendant"
            }
        };
        formatter.write_str(text)
    }
}

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
    /// Bytes that are invalid UTF-8 appeared where UTF-8 text was required.
    NotUtf8 {
        /// The path of the file whose contents are invalid UTF-8.
        path: String,
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
    /// The operation was refused because a mount is read-only or a policy
    /// denied it.
    PermissionDenied {
        /// The canonical path the operation targeted.
        path: String,
        /// Why the operation was refused: the policy's verdict text or the
        /// mount's refusal, naming the rule that fired.
        reason: String,
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
    /// The backend failed for a reason outside the other variants.
    Backend {
        /// The backend's own diagnosis.
        message: String,
    },
}

impl fmt::Display for VfsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { path } => write!(f, "not found: {path}"),
            Self::AlreadyExists { path } => write!(f, "already exists: {path}"),
            Self::NotADirectory { path } => write!(f, "not a directory: {path}"),
            Self::IsADirectory { path } => write!(f, "is a directory: {path}"),
            Self::DirectoryNotEmpty { path } => write!(f, "directory not empty: {path}"),
            Self::NotUtf8 { path } => write!(f, "not UTF-8: {path}"),
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
            Self::Unsupported { detail, .. } => write!(f, "unsupported operation: {detail}"),
            Self::Conflict { detail, .. } => write!(f, "conflicting claim: {detail}"),
            Self::Backend { message } => write!(f, "backend failure: {message}"),
        }
    }
}

impl std::error::Error for VfsError {}

#[cfg(test)]
mod tests {
    use super::{PathReason, VfsError};

    #[test]
    fn the_derives_hold_as_before() {
        // Clone, PartialEq, Eq, and Debug stay available on the struct
        // variants: backends compare and clone errors.
        let error = VfsError::NotFound {
            path: "/x".to_owned(),
        };
        let clone = error.clone();
        assert_eq!(error, clone);
        assert!(format!("{error:?}").contains("NotFound"));
        assert_eq!(
            VfsError::InvalidPath {
                path: String::new(),
                reason: PathReason::Empty,
            },
            VfsError::InvalidPath {
                path: String::new(),
                reason: PathReason::Empty,
            }
        );
    }

    #[test]
    fn every_variant_displays_with_its_own_prefix() {
        let cases = [
            (
                VfsError::NotFound {
                    path: "/x".to_owned(),
                },
                "not found: /x",
            ),
            (
                VfsError::AlreadyExists {
                    path: "/x".to_owned(),
                },
                "already exists: /x",
            ),
            (
                VfsError::NotADirectory {
                    path: "/x".to_owned(),
                },
                "not a directory: /x",
            ),
            (
                VfsError::IsADirectory {
                    path: "/x".to_owned(),
                },
                "is a directory: /x",
            ),
            (
                VfsError::DirectoryNotEmpty {
                    path: "/x".to_owned(),
                },
                "directory not empty: /x",
            ),
            (
                VfsError::NotUtf8 {
                    path: "/x".to_owned(),
                },
                "not UTF-8: /x",
            ),
            (
                VfsError::InvalidPath {
                    path: String::new(),
                    reason: PathReason::Empty,
                },
                "invalid path \"\": path is empty",
            ),
            (
                VfsError::InvalidRange {
                    path: "/x".to_owned(),
                    reason: "start is below 1",
                },
                "invalid line range for /x: start is below 1",
            ),
            (
                VfsError::Anchor {
                    path: "/x".to_owned(),
                    anchor: "TODO".to_owned(),
                    count: 2,
                },
                "anchor \"TODO\" occurs 2 times in /x, expected exactly one",
            ),
            (
                VfsError::PermissionDenied {
                    path: "/x".to_owned(),
                    reason: "denied".to_owned(),
                },
                "permission denied: denied",
            ),
            (
                VfsError::Unsupported {
                    path: "/x".to_owned(),
                    detail: "no".to_owned(),
                },
                "unsupported operation: no",
            ),
            (
                VfsError::Conflict {
                    path: "/x".to_owned(),
                    detail: "d".to_owned(),
                },
                "conflicting claim: d",
            ),
            (
                VfsError::Backend {
                    message: "m".to_owned(),
                },
                "backend failure: m",
            ),
        ];
        for (error, text) in cases {
            assert_eq!(error.to_string(), text);
        }
    }
}
