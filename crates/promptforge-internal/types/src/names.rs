//! The one global naming grammar.
//!
//! A name is one or more segments separated by `/`. The id types built on
//! it add their own segment count: a [`PluginId`](crate::plugins::PluginId)
//! is one segment, the local name a Plugin is installed under (`web`),
//! and a [`ToolId`](crate::tools::ToolId) is two or more, its Plugin's
//! local name first (`web/fetch`). Segments are lowercase ASCII
//! alphanumeric plus `-`, `_`, `.`, and comparison is case-sensitive. A
//! name carries no version, so a `@` is a parse error.

use std::fmt;

#[cfg(test)]
#[path = "names-tests.rs"]
mod tests;

/// A validated slash-separated name, such as `web` or `web/fetch`.
///
/// A name has one or more segments separated by `/`. [`PluginId`] and
/// [`ToolId`] are built on it and each adds its own segment count.
///
/// Every value comes from [`GlobalName::parse`], so each segment always
/// consists of one or more lowercase ASCII letters, digits, `-`, `_`, and `.`.
///
/// [`PluginId`]: crate::plugins::PluginId
/// [`ToolId`]: crate::tools::ToolId
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GlobalName {
    /// The `/`-separated segments: one or more.
    segments: Vec<String>,
}

impl GlobalName {
    /// Parses a string into a global name.
    ///
    /// The string must have one or more segments separated by `/`. Each
    /// segment must consist of one or more lowercase ASCII letters, digits,
    /// `-`, `_`, and `.`.
    ///
    /// # Errors
    ///
    /// Returns [`GlobalNameError`] when:
    ///
    /// - a segment is empty, including the whole string being empty
    ///   ([`GlobalNameErrorKind::Empty`]);
    /// - a segment contains any other character
    ///   ([`GlobalNameErrorKind::Control`]).
    pub fn parse(s: &str) -> Result<GlobalName, GlobalNameError> {
        let segments: Vec<&str> = s.split('/').collect();
        for segment in &segments {
            validate_segment(segment)?;
        }
        Ok(GlobalName {
            segments: segments.iter().map(|s| (*s).to_owned()).collect(),
        })
    }

    /// Returns the segments (one or more by construction).
    ///
    /// Crate-internal: the id newtypes count and index segments.
    pub(crate) fn segments(&self) -> &[String] {
        &self.segments
    }

    /// Returns the first segment as a one-segment name.
    ///
    /// Crate-internal: backs [`crate::tools::ToolId::plugin`].
    pub(crate) fn first(&self) -> GlobalName {
        GlobalName {
            segments: self.segments[..1].to_vec(),
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
/// uppercase (comparison is case-sensitive), `@`, control characters, and
/// non-ASCII - is rejected.
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
