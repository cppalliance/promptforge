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

/// A validated global name of two or three segments.
///
/// Two segments name a capability (`namespace/pack`); three segments name a
/// tool (`namespace/pack/name`). Construct only through
/// [`GlobalName::parse`]; the segment list is private so the arity and
/// charset invariants hold by construction.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GlobalName {
    /// The `/`-separated segments: exactly 2 (capability) or 3 (tool).
    segments: Vec<String>,
}

impl GlobalName {
    /// Parses a global name, enforcing the arity and charset rules.
    ///
    /// # Errors
    ///
    /// Returns [`GlobalNameError`] when the segment count is not 2 or 3
    /// ([`GlobalNameErrorKind::SegmentCount`]), a segment is empty
    /// ([`GlobalNameErrorKind::Empty`]), or a segment contains a character
    /// outside the allowed set ([`GlobalNameErrorKind::Control`]).
    ///
    /// # Examples
    ///
    /// ```
    /// use promptforge::capabilities::GlobalName;
    ///
    /// let name = GlobalName::parse("promptforge/web/fetch")?;
    /// assert_eq!(name.namespace(), "promptforge");
    /// assert_eq!(name.pack(), "web");
    /// assert_eq!(name.to_string(), "promptforge/web/fetch");
    /// # Ok::<(), promptforge::capabilities::GlobalNameError>(())
    /// ```
    pub fn parse(s: &str) -> Result<GlobalName, GlobalNameError> {
        let segments: Vec<&str> = s.split('/').collect();
        if !(2..=3).contains(&segments.len()) {
            return Err(GlobalNameError {
                kind: GlobalNameErrorKind::SegmentCount,
                reason: "must have exactly 2 segments (namespace/pack) or 3 (namespace/pack/name)",
            });
        }
        for segment in &segments {
            validate_segment(segment)?;
        }
        Ok(GlobalName {
            segments: segments.iter().map(|s| (*s).to_owned()).collect(),
        })
    }

    /// Returns the namespace segment (reverse-DNS or `promptforge`).
    #[must_use]
    pub fn namespace(&self) -> &str {
        &self.segments[0]
    }

    /// Returns the pack segment.
    #[must_use]
    pub fn pack(&self) -> &str {
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
    /// Crate-internal: backs [`crate::tools::ToolId::capability`].
    pub(crate) fn capability_prefix(&self) -> GlobalName {
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

/// A stable, matchable classification of a [`GlobalNameError`].
///
/// Every public error exposes a `kind()` classifier so callers can branch on
/// the failure without matching a private representation (DESIGN-5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum GlobalNameErrorKind {
    /// The name did not have exactly 2 or 3 segments.
    SegmentCount,
    /// A segment was empty.
    Empty,
    /// A segment contained a character outside the allowed set.
    Control,
}

/// The reason a [`GlobalName`] could not be parsed.
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
    /// Returns the stable classification of this error (DESIGN-5).
    #[must_use]
    pub fn kind(&self) -> GlobalNameErrorKind {
        self.kind
    }
}
