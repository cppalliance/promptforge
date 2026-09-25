//! The JSON state-bucket validator shared by the workshop's two persisted
//! ui-state buckets (the account-scoped user-state and the workspace
//! file): one allow-list check on the key, one size cap, and one JSON
//! parse, returning a support-level refusal the caller maps onto its own
//! wire error so the wire code and message stay the caller's.

use serde_json::Value;

/// A state-bucket refusal: one variant per way a put is refused. The
/// bucket crates map each onto their own wire error, so the wire code
/// and message stay the bucket's.
#[derive(Debug, thiserror::Error)]
pub enum StateBucketError {
    /// The key is outside the allow-list.
    #[error("state-bucket key {0:?} is not allowed")]
    Key(String),

    /// The value's JSON text exceeds the cap.
    #[error("state-bucket value is {actual} bytes; at most {cap} bytes are allowed")]
    TooLarge {
        /// The size of the refused value.
        actual: usize,
        /// The cap it exceeded.
        cap: usize,
    },

    /// The value does not parse as JSON.
    #[error("state-bucket value is not JSON")]
    NotJson {
        /// The parse refusal.
        #[source]
        source: serde_json::Error,
    },
}

/// Resolves `key` to its allow-list entry.
///
/// # Errors
/// Returns [`StateBucketError::Key`] when `key` is not in `allowed`.
pub fn resolve_bucket_key<'a>(key: &str, allowed: &[&'a str]) -> Result<&'a str, StateBucketError> {
    allowed
        .iter()
        .copied()
        .find(|allowed| *allowed == key)
        .ok_or_else(|| StateBucketError::Key(key.to_owned()))
}

/// Checks that a value whose JSON text is `actual` bytes fits under `cap`.
///
/// # Errors
/// Returns [`StateBucketError::TooLarge`] past `cap`.
pub fn check_bucket_cap(actual: usize, cap: usize) -> Result<(), StateBucketError> {
    if actual > cap {
        return Err(StateBucketError::TooLarge { actual, cap });
    }
    Ok(())
}

/// Checks that `text` parses as JSON, without building a value; the
/// file-backed bucket stores text verbatim and needs only the validity
/// check.
///
/// # Errors
/// Returns [`StateBucketError::NotJson`] for text that does not parse.
pub fn check_bucket_text(text: &str) -> Result<(), StateBucketError> {
    serde_json::from_str::<serde::de::IgnoredAny>(text)
        .map(|_| ())
        .map_err(|source| StateBucketError::NotJson { source })
}

/// Validates one state-bucket put, in cheapest-refusal-first order: the
/// key against `allowed`, the body's size against `cap`, then the body's
/// shape. Returns the parsed value.
///
/// # Errors
/// Returns [`StateBucketError::Key`], [`StateBucketError::TooLarge`], or
/// [`StateBucketError::NotJson`] for a refused input.
pub fn validate_bucket_body(
    key: &str,
    allowed: &[&str],
    body: &[u8],
    cap: usize,
) -> Result<Value, StateBucketError> {
    resolve_bucket_key(key, allowed)?;
    check_bucket_cap(body.len(), cap)?;
    serde_json::from_slice(body).map_err(|source| StateBucketError::NotJson { source })
}

#[cfg(test)]
#[path = "state_bucket-tests.rs"]
mod tests;
