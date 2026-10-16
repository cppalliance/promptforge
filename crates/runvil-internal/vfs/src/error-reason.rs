//! [`PathReason`], why a path or glob pattern was refused before any
//! backend saw it, and the error its tag parse returns.

use std::fmt;
use std::str::FromStr;

/// Why a path or glob pattern was rejected before any backend saw it.
///
/// Every [`VfsError::InvalidPath`](crate::VfsError::InvalidPath) carries
/// one. Store operations validate each path and report the first nine
/// reasons. Only glob and rename operations report the last two:
/// [`PathReason::Wildcard`] for a glob pattern whose wildcard grammar is
/// invalid, and [`PathReason::IntoDescendant`] for a rename into the
/// source's own descendant.
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
    /// `NUL`, `COM1`) or held a `:`, which some backends treat as a device
    /// or a stream rather than a file.
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
    /// Every reason, in declaration order.
    pub(crate) const ALL: &'static [PathReason] = &[
        PathReason::Empty,
        PathReason::Absolute,
        PathReason::Traversal,
        PathReason::Control,
        PathReason::EmptySegment,
        PathReason::Backslash,
        PathReason::ReservedName,
        PathReason::UnsafeSuffix,
        PathReason::TooLong,
        PathReason::Wildcard,
        PathReason::IntoDescendant,
    ];

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
            PathReason::ReservedName => "path contains a reserved device name or a colon",
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

impl FromStr for PathReason {
    type Err = ParsePathReasonError;

    /// Parses a reason from its exact tag, as [`PathReason::from_tag`]
    /// does.
    fn from_str(tag: &str) -> Result<PathReason, ParsePathReasonError> {
        PathReason::from_tag(tag).ok_or_else(|| ParsePathReasonError {
            input: tag.to_owned(),
        })
    }
}

/// The error returned when text is not a [`PathReason`] tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsePathReasonError {
    /// The rejected text.
    input: String,
}

impl ParsePathReasonError {
    /// Returns the rejected text.
    #[must_use]
    pub fn input(&self) -> &str {
        &self.input
    }
}

impl fmt::Display for ParsePathReasonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid path reason `{}`: required one of ", self.input)?;
        for (position, reason) in PathReason::ALL.iter().enumerate() {
            if position > 0 {
                f.write_str(", ")?;
            }
            write!(f, "`{}`", reason.tag())?;
        }
        Ok(())
    }
}

impl std::error::Error for ParsePathReasonError {}
