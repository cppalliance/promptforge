//! The redacting `Secret` string and its serde field helpers.

use std::fmt;

use serde::Deserialize;

/// A secret string (an API key or the shared token) that never serializes and
/// redacts in both `Debug` and `Display`.
///
/// The type has no public `Deserialize` or `From<String>` impl: configuration
/// deserialization constructs it through a private field deserializer, so a
/// redacting secret can never be round-tripped from a downstream consumer.
/// `expose` is the single read accessor.
#[derive(Clone)]
#[non_exhaustive]
pub struct Secret(String);

impl Secret {
    /// Wraps a plaintext secret.
    ///
    /// Used by config deserialization and by the gateway's adapters that mint
    /// an ephemeral loopback credential.
    #[must_use]
    pub fn new(value: String) -> Secret {
        Secret(value)
    }

    /// The secret's bytes. The one place a secret is read, when building auth.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Whether the secret is empty (an intentionally credential-free endpoint).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Deserializes a [`Secret`] field from a bare TOML string without exposing a
/// public `Deserialize` impl on the redacting type.
pub(super) fn de_secret<'de, D>(deserializer: D) -> Result<Secret, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = String::deserialize(deserializer)?;
    Ok(Secret::new(raw))
}

/// Serializes a [`Secret`] field as `"***"`: a serialized configuration never
/// contains credential material, and a reader treats the marker as "keep the
/// existing value" on write.
pub(super) fn ser_redacted<S>(_: &Secret, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str("***")
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(redacted)")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("redacted")
    }
}
