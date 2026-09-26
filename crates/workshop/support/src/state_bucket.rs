//! The JSON state-bucket validator shared by the workshop's two persisted
//! ui-state buckets (the account-scoped user-state and the workspace
//! file): one allow-list check on the key, one size cap, and one JSON
//! parse, run once at the route to build the [`StateBucketValue`] the
//! buckets' lower layers accept without checking again. A refusal is a
//! support-level error the caller maps onto its own wire error, so the
//! wire code and message stay the caller's.

use serde_json::Value;

/// The largest value either state bucket accepts, in bytes of JSON text.
pub const STATE_BUCKET_VALUE_CAP: usize = 1 << 20;

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

/// One validated state-bucket put: an allow-listed key and a JSON value
/// whose compact text fits under [`STATE_BUCKET_VALUE_CAP`].
#[derive(Debug)]
pub struct StateBucketValue {
    /// The allow-list entry the key resolved to.
    key: &'static str,
    /// The parsed value.
    value: Value,
    /// The value's compact JSON text, the form the buckets store.
    text: String,
}

impl StateBucketValue {
    /// Validates one put, in cheapest-refusal-first order: `key` against
    /// `allowed`, the body's size against the cap, then the body's shape.
    /// The compact text is held to the same cap, because a body under it
    /// can re-serialize past it (`1e15` becomes `1000000000000000.0`).
    ///
    /// # Errors
    /// Returns [`StateBucketError::Key`], [`StateBucketError::TooLarge`], or
    /// [`StateBucketError::NotJson`] for a refused input.
    pub fn new(key: &str, allowed: &[&'static str], body: &[u8]) -> Result<Self, StateBucketError> {
        let key = resolve_bucket_key(key, allowed)?;
        check_bucket_cap(body.len(), STATE_BUCKET_VALUE_CAP)?;
        let value: Value =
            serde_json::from_slice(body).map_err(|source| StateBucketError::NotJson { source })?;
        let text = value.to_string();
        check_bucket_cap(text.len(), STATE_BUCKET_VALUE_CAP)?;
        Ok(Self { key, value, text })
    }

    /// The allow-list entry the key resolved to.
    #[must_use]
    pub fn key(&self) -> &'static str {
        self.key
    }

    /// The parsed value.
    #[must_use]
    pub fn value(&self) -> &Value {
        &self.value
    }

    /// The value's compact JSON text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The parsed value, without the key and text.
    #[must_use]
    pub fn into_value(self) -> Value {
        self.value
    }
}

/// Resolves `key` to its allow-list entry.
///
/// # Errors
/// Returns [`StateBucketError::Key`] when `key` is not in `allowed`.
fn resolve_bucket_key<'a>(key: &str, allowed: &[&'a str]) -> Result<&'a str, StateBucketError> {
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
fn check_bucket_cap(actual: usize, cap: usize) -> Result<(), StateBucketError> {
    if actual > cap {
        return Err(StateBucketError::TooLarge { actual, cap });
    }
    Ok(())
}

#[cfg(test)]
#[path = "state_bucket-tests.rs"]
mod tests;
