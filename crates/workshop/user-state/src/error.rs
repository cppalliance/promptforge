//! The user-state operation failure type and its wire mapping.
//!
//! [`UserStateError`] is the boundary between the store's refusals
//! and its caller: a put names the key it refused or the size
//! it exceeded, and a failed write surfaces the I/O cause. At the route
//! boundary each variant maps to exactly one status code and one
//! machine-readable envelope code, rendered through `workshop-protocol`'s
//! [`ErrorEnvelope`]. Internal failure detail (the source chain) reaches
//! the response body in debug builds only; production bodies stay at
//! each variant's own message.

use std::io;

use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};

// The JSON cause behind `UserStateError::NotJson`. A caller that needs
// the parse error itself names `shared_error_source` directly; this
// crate does not re-export the wrapper, so there is one name for the
// cause across the workspace rather than one per crate.
use shared_error_source::JsonSource;
use workshop_protocol::ErrorEnvelope;
use workshop_support::{LEAK_DETAIL, StateBucketError, render_message};

use crate::store::USER_STATE_KEYS;

/// A user-state operation failure.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum UserStateError {
    /// A put named a key outside the allow-list. The message lists the
    /// allow-list itself so it cannot drift from the keys.
    #[non_exhaustive]
    #[error(
        "user-state key {0:?} is not allowed; one of {allowed} is required",
        allowed = USER_STATE_KEYS.join(", ")
    )]
    Key(String),

    /// A value's JSON text exceeds the size cap.
    #[non_exhaustive]
    #[error("user-state value is {actual} bytes; at most {cap} bytes are allowed")]
    TooLarge {
        /// The size of the refused value.
        actual: usize,
        /// The cap it exceeded.
        cap: usize,
    },

    /// A put body does not parse as JSON.
    #[non_exhaustive]
    #[error("user-state value is not JSON")]
    NotJson {
        /// The parse refusal.
        #[source]
        source: JsonSource,
    },

    /// The state file could not be written.
    #[non_exhaustive]
    #[error("user-state file cannot be written")]
    Io(#[source] io::Error),
}

impl UserStateError {
    /// The one HTTP status this failure answers with.
    pub(crate) fn status(&self) -> StatusCode {
        match self {
            Self::Key(_) | Self::NotJson { .. } => StatusCode::BAD_REQUEST,
            Self::TooLarge { .. } => StatusCode::PAYLOAD_TOO_LARGE,
            Self::Io(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// The machine-readable code of the JSON error envelope.
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::Key(_) => "user_state_key",
            Self::TooLarge { .. } => "user_state_too_large",
            Self::NotJson { .. } => "user_state_not_json",
            Self::Io(_) => "user_state_io",
        }
    }
}

impl IntoResponse for UserStateError {
    fn into_response(self) -> Response {
        let status = self.status();
        let envelope = ErrorEnvelope::new(render_message(&self, LEAK_DETAIL), self.code());
        // Serializing the envelope cannot fail: two strings only. A body
        // that somehow cannot serialize degrades to the status line's
        // own text.
        let body = serde_json::to_string(&envelope)
            .unwrap_or_else(|_| status.canonical_reason().unwrap_or("error").to_string());
        (status, [(header::CONTENT_TYPE, "application/json")], body).into_response()
    }
}

impl From<StateBucketError> for UserStateError {
    /// Maps a shared state-bucket refusal onto this crate's wire error,
    /// preserving each refusal's own message.
    fn from(error: StateBucketError) -> Self {
        match error {
            StateBucketError::Key(key) => Self::Key(key),
            StateBucketError::TooLarge { actual, cap } => Self::TooLarge { actual, cap },
            StateBucketError::NotJson { source } => Self::NotJson {
                source: source.into(),
            },
        }
    }
}

#[cfg(test)]
#[path = "error-tests.rs"]
mod tests;
