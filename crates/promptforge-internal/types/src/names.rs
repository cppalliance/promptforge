//! The one global naming grammar.
//!
//! Kind is encoded by arity: capabilities are `namespace/pack` (2 segments)
//! and tools are `namespace/pack/name` (3 segments), so a reader can tell the
//! kind of any name by counting segments. A namespace is reverse-DNS
//! (`org.rustalliance`) or the reserved first-party prefix `promptforge`.
//! Segments are lowercase ASCII alphanumeric plus `-`, `_`, `.`, and
//! comparison is case-sensitive. v1 is unversioned: a `@` is a parse error.

use std::fmt;

#[cfg(test)]
#[path = "names-tests.rs"]
mod tests;

/// A validated global name for a capability or a tool.
///
/// A name has two or three segments separated by `/`. Two segments name a
/// capability (`namespace/pack`). Three segments name a tool
/// (`namespace/pack/name`).
///
/// Every value comes from [`GlobalName::parse`], so each segment always
/// consists of one or more lowercase ASCII letters, digits, `-`, `_`, and `.`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GlobalName {
    /// The `/`-separated segments: exactly 2 (capability) or 3 (tool).
    segments: Vec<String>,
}

impl GlobalName {
    /// Parses a string into a global name.
    ///
    /// The string must have 2 or 3 segments separated by `/`. Each segment must
    /// consist of one or more lowercase ASCII letters, digits, `-`, `_`, and
    /// `.`.
    ///
    /// # Errors
    ///
    /// Returns [`GlobalNameError`] when:
    ///
    /// - the name has fewer than 2 or more than 3 segments
    ///   ([`GlobalNameErrorKind::SegmentCount`]);
    /// - a segment is empty ([`GlobalNameErrorKind::Empty`]);
    /// - a segment contains any other character
    ///   ([`GlobalNameErrorKind::Control`]).
    pub fn parse(s: &str) -> Result<GlobalName, GlobalNameError> {
        let segments: Vec<&str> = s.split('/').collect();
        if !(2..=3).contains(&segments.len()) {
            return Err(GlobalNameError {
                kind: GlobalNameErrorKind::SegmentCount,
                reason: "must have exactly 2 segments (namespace/plugin) or 3 (namespace/plugin/name)",
            });
        }
        for segment in &segments {
            validate_segment(segment)?;
        }
        Ok(GlobalName {
            segments: segments.iter().map(|s| (*s).to_owned()).collect(),
        })
    }

    /// Returns the namespace, which is the first segment.
    ///
    /// By convention, a namespace is a reverse-DNS name such as
    /// `org.rustalliance`, or `promptforge` for first-party names.
    /// `GlobalName::parse` accepts any valid segment as the namespace.
    #[must_use]
    pub fn namespace(&self) -> &str {
        &self.segments[0]
    }

    /// Returns the pack, which is the second segment.
    #[must_use]
    pub fn plugin(&self) -> &str {
        &self.segments[1]
    }

    /// Returns the segments (exactly 2 or 3 by construction).
    ///
    /// Crate-internal: the id newtypes in [`crate::tools`] index segments.
    pub(crate) fn segments(&self) -> &[String] {
        &self.segments
    }

    /// Returns the 2-segment capability prefix of a 3-segment (tool) name.
    ///
    /// Crate-internal: backs [`crate::tools::ToolId::plugin`].
    pub(crate) fn plugin_prefix(&self) -> GlobalName {
        GlobalName {
            segments: self.segments[..2].to_vec(),
        }
    }
}

impl fmt::Display for GlobalName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.segments.join("/"))
    }
}

/// Validates one segment against the charset rule.
///
/// A segment must be non-empty and contain only lowercase ASCII
/// alphanumeric characters plus `-`, `_`, `.`. Anything else - including
/// uppercase (comparison is case-sensitive), `@` (v1 is unversioned),
/// control characters, and non-ASCII - is rejected.
fn validate_segment(segment: &str) -> Result<(), GlobalNameError> {
    if segment.is_empty() {
        return Err(GlobalNameError {
            kind: GlobalNameErrorKind::Empty,
            reason: "segments must not be empty",
        });
    }
    for byte in segment.bytes() {
        if byte < 0x20 || byte == 0x7f {
            return Err(GlobalNameError {
                kind: GlobalNameErrorKind::Control,
                reason: "segments must not contain a control character",
            });
        }
        if !matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.') {
            return Err(GlobalNameError {
                kind: GlobalNameErrorKind::Control,
                reason: "segments may contain only lowercase ASCII letters, digits, '-', '_', '.'",
            });
        }
    }
    Ok(())
}

/// A stable classification of a [`GlobalNameError`].
///
/// `GlobalNameError::kind` returns it. Match on it to branch on why a name
/// was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum GlobalNameErrorKind {
    /// The name had fewer than 2 or more than 3 segments.
    SegmentCount,
    /// A segment was empty.
    Empty,
    /// A segment contained a character other than a lowercase ASCII letter, a
    /// digit, `-`, `_`, or `.`.
    ///
    /// This covers control characters, uppercase letters, `@`, and non-ASCII
    /// characters.
    Control,
}

/// The error returned when a [`GlobalName`] fails to parse.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid global name: {reason}")]
#[non_exhaustive]
pub struct GlobalNameError {
    /// A stable classification of why the name was rejected.
    kind: GlobalNameErrorKind,
    /// A human-readable reason.
    reason: &'static str,
}

impl GlobalNameError {
    /// Returns the stable classification of this error.
    #[must_use]
    pub fn kind(&self) -> GlobalNameErrorKind {
        self.kind
    }
}
