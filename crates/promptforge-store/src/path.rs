//! Logical store-path validation and canonicalization.
//!
//! `StoreRef` parses every caller-supplied `&str` into a [`StorePath`] before
//! dispatch, so a backend never sees an empty, absolute, traversing,
//! control-bearing, backslash-bearing, platform-reserved, or over-long path
//! (STORE-003).

use super::{PathReason, StoreError};

/// The largest logical store path, in bytes, accepted before dispatch.
///
/// Bounds both the validated path itself and, transitively, the text the glob
/// matcher can be asked to scan (STORE-003/005), so neither is an unbounded
/// denial-of-service lever.
pub(crate) const MAX_STORE_PATH_BYTES: usize = 1024;

/// Returns whether `segment` is a platform-reserved device name.
///
/// Windows treats names like `CON`, `NUL`, `COM1`, and `LPT1` as devices even
/// with an extension (`con.txt`), so the base name before the first `.` is
/// checked case-insensitively (STORE-003).
fn is_reserved_device_name(segment: &str) -> bool {
    let base = segment.split('.').next().unwrap_or(segment);
    let upper = base.to_ascii_uppercase();
    matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || is_numbered_device(&upper, "COM")
        || is_numbered_device(&upper, "LPT")
}

/// Returns whether `name` is `<prefix>N` for a single digit `1..=9`.
fn is_numbered_device(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix)
        .and_then(|rest| rest.parse::<u8>().ok().filter(|_| rest.len() == 1))
        .is_some_and(|n| (1..=9).contains(&n))
}

/// A validated logical store path in one canonical form.
///
/// `StoreRef` parses every caller-supplied `&str` into this before dispatch, so
/// a backend never sees an empty, absolute, traversing, control-bearing, or
/// empty-segment path. The trait boundary keeps `&str`; this type is internal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StorePath(String);

impl StorePath {
    /// Validates `raw` into one canonical path, or reports why it was rejected.
    pub(crate) fn parse(raw: &str) -> Result<StorePath, StoreError> {
        let reject = |reason| {
            Err(StoreError::InvalidPath {
                path: raw.to_owned(),
                reason,
            })
        };
        if raw.is_empty() {
            return reject(PathReason::Empty);
        }
        if raw.len() > MAX_STORE_PATH_BYTES {
            return reject(PathReason::TooLong);
        }
        if raw.starts_with('/') {
            return reject(PathReason::Absolute);
        }
        if raw.bytes().any(|b| b < 0x20 || b == 0x7f) {
            return reject(PathReason::Control);
        }
        // A backslash is a separator on some backends and a literal on others;
        // refuse it so a canonical `/`-separated path cannot be reinterpreted
        // (STORE-003).
        if raw.contains('\\') {
            return reject(PathReason::Backslash);
        }
        let mut segments = 0usize;
        for segment in raw.split('/') {
            if segment.is_empty() {
                return reject(PathReason::EmptySegment);
            }
            if segment == "." || segment == ".." {
                return reject(PathReason::Traversal);
            }
            // Trailing `.`/space are stripped by some backends, so the stored
            // name would not round-trip (STORE-003).
            if segment.ends_with('.') || segment.ends_with(' ') {
                return reject(PathReason::UnsafeSuffix);
            }
            if is_reserved_device_name(segment) {
                return reject(PathReason::ReservedName);
            }
            segments += 1;
        }
        if segments == 0 {
            return reject(PathReason::Empty);
        }
        Ok(StorePath(raw.to_owned()))
    }

    /// Borrows the canonical path string for backend dispatch.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}
