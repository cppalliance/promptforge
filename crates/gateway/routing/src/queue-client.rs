//! The bounded client identity that keys fair scheduling.

/// A bounded scheduling identity parsed from the client header.
///
/// Callers name themselves via `X-PromptForge-Client` for fair queueing. The
/// value is parsed at the boundary into a bounded id (max length, restricted
/// charset); anything missing, empty, oversized, or containing other characters
/// maps to the single documented `default` bucket so an authenticated caller
/// cannot mint unbounded, attacker-chosen scheduler identities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientId(String);

impl ClientId {
    /// Maximum accepted client-id length, in bytes.
    pub const MAX_LEN: usize = 64;
    /// The fallback bucket for absent or invalid ids.
    pub const DEFAULT: &'static str = "default";

    /// Parses an optional header string into a bounded [`ClientId`].
    pub fn from_header(value: Option<&str>) -> ClientId {
        value.map_or_else(|| ClientId(Self::DEFAULT.to_owned()), Self::parse)
    }

    /// Parses a raw string into a bounded [`ClientId`], falling back to `default`.
    #[must_use]
    pub fn parse(raw: &str) -> ClientId {
        let trimmed = raw.trim();
        let valid = !trimmed.is_empty()
            && trimmed.len() <= Self::MAX_LEN
            && trimmed
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'));
        if valid {
            ClientId(trimmed.to_owned())
        } else {
            ClientId(Self::DEFAULT.to_owned())
        }
    }

    /// The validated id as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
