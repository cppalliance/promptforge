//! Store error types and their stable classifiers.

/// The backend lock was poisoned by a panicked holder.
///
/// Kept private and surfaced only as an opaque [`StoreError::Backend`] source
/// (STORE-004), so a poisoned lock is a visible backend failure rather than a
/// silent recovery of state this handle cannot vouch for.
#[derive(Debug, thiserror::Error)]
#[error("store backend lock was poisoned by a panicked holder")]
pub(crate) struct StorePoisoned;

/// Why a logical store path was rejected before any backend saw it.
///
/// `StoreRef` validates every caller-supplied path into one canonical form
/// before dispatch; this names the rule the path broke.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PathReason {
    /// The path was empty or contained only separators.
    Empty,
    /// The path began with `/`, so it addressed outside the run's namespace.
    Absolute,
    /// The path contained a `.` or `..` segment (parent or current traversal).
    Traversal,
    /// The path contained a control character (below `0x20`, or `0x7f`).
    Control,
    /// The path contained an empty segment (a `//` run, or a trailing `/`).
    EmptySegment,
    /// The path contained a backslash, which is ambiguous across backends (a
    /// literal byte to one, a separator to another).
    Backslash,
    /// A segment was a platform-reserved device name (for example `CON`,
    /// `NUL`, `COM1`), which some backends cannot represent as a plain file.
    ReservedName,
    /// A segment ended in a byte some backends silently strip (a trailing `.`
    /// or space), so the stored name would not round-trip.
    UnsafeSuffix,
    /// The path exceeded the maximum supported length in bytes.
    TooLong,
}

impl std::fmt::Display for PathReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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
        };
        formatter.write_str(text)
    }
}

/// A stable, matchable classification of a [`StoreError`].
///
/// A caller matches on this instead of the private error representation, so new
/// error causes can be added without breaking a `match`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum StoreErrorKind {
    /// No file exists at the requested path.
    NotFound,
    /// A `str_replace` anchor did not occur, or occurred more than once.
    Anchor,
    /// A `str_replace` anchor was itself invalid (for example empty), so it was
    /// refused before any backend search rather than being reported as merely
    /// "not found".
    InvalidAnchor,
    /// A caller-supplied path failed validation.
    InvalidPath,
    /// A caller-supplied glob pattern failed validation.
    InvalidPattern,
    /// A caller-supplied line range failed validation.
    InvalidRange,
    /// Two arms of one fanout wrote the same path.
    WriteRace,
    /// The backend itself failed.
    Backend,
}

/// An error from a virtual-file operation.
///
/// Marked `#[non_exhaustive]` so a future backend (real filesystem, network)
/// can add variants without a breaking change; each data-carrying variant is
/// likewise `#[non_exhaustive]`. `Display` messages are lowercase noun phrases
/// with no trailing period, so a caller supplies the surrounding context.
/// Match on [`StoreError::kind`] rather than the variants directly.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StoreError {
    /// No file exists at the requested path.
    #[error("file not found: {path}")]
    #[non_exhaustive]
    NotFound {
        /// The logical path that did not resolve to a file.
        path: String,
    },

    /// A `str_replace` anchor was itself invalid (for example empty) and was
    /// refused before any backend search.
    #[error("invalid anchor for {path}: {reason}")]
    #[non_exhaustive]
    InvalidAnchor {
        /// The logical path the edit targeted.
        path: String,
        /// A short human-readable reason the anchor was rejected.
        reason: &'static str,
    },

    /// The `str_replace` anchor did not occur in the file.
    #[error("anchor not found in {path}")]
    #[non_exhaustive]
    AnchorNotFound {
        /// The logical path that was searched.
        path: String,
        /// The anchor text that was not found.
        anchor: String,
    },

    /// The `str_replace` anchor occurred more than once, so the edit is
    /// ambiguous and is refused rather than applied to an arbitrary match.
    #[error("anchor occurs {count} times in {path}, expected exactly one")]
    #[non_exhaustive]
    AnchorAmbiguous {
        /// The logical path that was searched.
        path: String,
        /// The anchor text that matched more than once.
        anchor: String,
        /// The number of times the anchor matched.
        count: usize,
    },

    /// A caller-supplied path was rejected before any backend saw it.
    #[error("invalid path {path:?}: {reason}")]
    #[non_exhaustive]
    InvalidPath {
        /// The rejected path, exactly as supplied.
        path: String,
        /// The validation rule the path broke.
        reason: PathReason,
    },

    /// A caller-supplied glob pattern was rejected before matching.
    #[error("invalid glob pattern {pattern:?}: {reason}")]
    #[non_exhaustive]
    InvalidPattern {
        /// The rejected pattern, exactly as supplied.
        pattern: String,
        /// A short human-readable reason.
        reason: String,
    },

    /// A caller-supplied line range was rejected before any lines were
    /// sliced.
    #[error("invalid line range for {path}: {reason}")]
    #[non_exhaustive]
    InvalidRange {
        /// The logical path the read targeted.
        path: String,
        /// A short human-readable reason the range was rejected.
        reason: &'static str,
    },

    /// Two arms of one fanout wrote the same path: a write-write race. The
    /// losing write never reached the backend.
    #[error("write-write race on {path}: another arm of the same fanout already wrote it")]
    #[non_exhaustive]
    WriteRace {
        /// The logical path both arms wrote.
        path: String,
    },

    /// The backend failed for a reason of its own, kept as an opaque source.
    #[error("store backend failure")]
    #[non_exhaustive]
    Backend {
        /// The backend's own error, hidden behind `#[source]`.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

impl StoreError {
    /// Returns the stable classification of this error.
    ///
    /// # Examples
    /// ```
    /// use promptforge_store::{StoreErrorKind, StoreRef};
    ///
    /// let err = StoreRef::memory().read("missing.txt").unwrap_err();
    /// assert_eq!(err.kind(), StoreErrorKind::NotFound);
    /// ```
    #[must_use]
    pub fn kind(&self) -> StoreErrorKind {
        match self {
            StoreError::NotFound { .. } => StoreErrorKind::NotFound,
            StoreError::InvalidAnchor { .. } => StoreErrorKind::InvalidAnchor,
            StoreError::AnchorNotFound { .. } | StoreError::AnchorAmbiguous { .. } => {
                StoreErrorKind::Anchor
            }
            StoreError::InvalidPath { .. } => StoreErrorKind::InvalidPath,
            StoreError::InvalidPattern { .. } => StoreErrorKind::InvalidPattern,
            StoreError::InvalidRange { .. } => StoreErrorKind::InvalidRange,
            StoreError::WriteRace { .. } => StoreErrorKind::WriteRace,
            StoreError::Backend { .. } => StoreErrorKind::Backend,
        }
    }

    /// Returns whether this error means the addressed file was absent.
    ///
    /// # Examples
    /// ```
    /// use promptforge_store::StoreRef;
    ///
    /// let err = StoreRef::memory().read("missing.txt").unwrap_err();
    /// assert!(err.is_not_found());
    /// ```
    #[must_use]
    pub fn is_not_found(&self) -> bool {
        matches!(self, StoreError::NotFound { .. })
    }

    /// Returns the logical path this error concerns, when it names one.
    ///
    /// # Examples
    /// ```
    /// use promptforge_store::StoreRef;
    ///
    /// let err = StoreRef::memory().read("missing.txt").unwrap_err();
    /// assert_eq!(err.path(), Some("missing.txt"));
    /// ```
    #[must_use]
    pub fn path(&self) -> Option<&str> {
        match self {
            StoreError::NotFound { path }
            | StoreError::InvalidAnchor { path, .. }
            | StoreError::AnchorNotFound { path, .. }
            | StoreError::AnchorAmbiguous { path, .. }
            | StoreError::InvalidPath { path, .. }
            | StoreError::InvalidRange { path, .. }
            | StoreError::WriteRace { path } => Some(path),
            StoreError::InvalidPattern { .. } | StoreError::Backend { .. } => None,
        }
    }

    /// Wraps a backend's own error as an opaque [`StoreError::Backend`] source.
    ///
    /// A downstream [`Store`](crate::Store) implementation uses this so its concrete error
    /// type never leaks through this crate's public API.
    ///
    /// # Examples
    /// ```
    /// use promptforge_store::{StoreError, StoreErrorKind};
    ///
    /// let io = std::io::Error::other("disk gone");
    /// let err = StoreError::backend(io);
    /// assert_eq!(err.kind(), StoreErrorKind::Backend);
    /// ```
    #[must_use]
    pub fn backend(source: impl std::error::Error + Send + Sync + 'static) -> StoreError {
        StoreError::Backend {
            source: Box::new(source),
        }
    }

    /// Builds [`StoreError::NotFound`] for `path`.
    ///
    /// `#[doc(hidden)]`: a cross-crate seam for `promptforge-core` test
    /// doubles, which cannot construct the `#[non_exhaustive]` variant
    /// directly. Not host API.
    #[doc(hidden)]
    #[must_use]
    pub fn not_found(path: &str) -> StoreError {
        StoreError::NotFound {
            path: path.to_owned(),
        }
    }

    /// Builds [`StoreError::InvalidRange`] for `path` with `reason`.
    ///
    /// `#[doc(hidden)]`: a cross-crate seam for `promptforge-core`'s Lua
    /// host, which refuses an `end` without a `start` with the same
    /// `InvalidRange` a zero bound earns but cannot construct the
    /// `#[non_exhaustive]` variant directly. Not host API.
    #[doc(hidden)]
    #[must_use]
    pub fn invalid_range(path: &str, reason: &'static str) -> StoreError {
        StoreError::InvalidRange {
            path: path.to_owned(),
            reason,
        }
    }
}
