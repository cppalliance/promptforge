//! Error-message rendering shared by the workshop crates: the
//! debug-only source-chain leak flag, the helper that renders an
//! error's envelope message (its own `Display` text with the
//! source chain appended as `: cause` segments in debug builds
//! only), and the helper that answers an envelope as the JSON
//! response.

use std::fmt::Write as _;

use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

/// Whether wire bodies include internal failure detail. Debug builds append
/// the source chain to the envelope message; production bodies stay at
/// the variant's own message.
pub const LEAK_DETAIL: bool = cfg!(debug_assertions);

/// Renders the envelope message for `error`: its own `Display` text, with
/// the source chain appended as `: cause` segments when `leak_detail` is
/// set.
#[must_use]
pub fn render_message<E>(error: &E, leak_detail: bool) -> String
where
    E: std::fmt::Display + std::error::Error,
{
    let mut message = error.to_string();
    if leak_detail {
        let mut source = std::error::Error::source(error);
        while let Some(cause) = source {
            // fmt::Write to a String cannot fail; the Result is a trait
            // artifact.
            let _ = write!(message, ": {cause}");
            source = cause.source();
        }
    }
    message
}

/// Answers `envelope` as the JSON body of a `status` response; an
/// envelope that cannot serialize degrades to the status line's own text.
/// Generic because this crate may not name `workshop-protocol`'s
/// `ErrorEnvelope`.
#[must_use]
pub fn envelope_response<T: Serialize>(status: StatusCode, envelope: &T) -> Response {
    let body = serde_json::to_string(envelope)
        .unwrap_or_else(|_| status.canonical_reason().unwrap_or("error").to_string());
    (status, [(header::CONTENT_TYPE, "application/json")], body).into_response()
}

#[cfg(test)]
#[path = "error_message-tests.rs"]
mod tests;
