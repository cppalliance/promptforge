//! Tests for the shared error-message rendering: the production message
//! stays at the error's own `Display` text, the debug message appends the
//! source chain, the leak flag tracks debug assertions, and an envelope
//! that cannot serialize answers the status line's text.

use std::io;

use super::*;

/// A nested test error, so the source-chain walk has a cause to append.
#[derive(Debug, thiserror::Error)]
#[error("read failed")]
struct TestError {
    /// The injected cause.
    #[source]
    source: io::Error,
}

/// An envelope whose serialization always fails, so the response takes
/// its fallback body.
struct Unserializable;

impl serde::Serialize for Unserializable {
    fn serialize<S: serde::Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
        Err(serde::ser::Error::custom("refused"))
    }
}

#[test]
fn production_messages_stay_at_the_variant_text() {
    let error = TestError {
        source: io::Error::other("disk gone"),
    };
    assert_eq!(render_message(&error, false), "read failed");
}

#[test]
fn debug_messages_append_the_source_chain() {
    let error = TestError {
        source: io::Error::other("disk gone"),
    };
    assert_eq!(render_message(&error, true), "read failed: disk gone");
}

#[test]
fn the_leak_flag_tracks_debug_assertions() {
    assert_eq!(LEAK_DETAIL, cfg!(debug_assertions));
}

#[tokio::test]
async fn an_envelope_that_cannot_serialize_answers_the_status_text() {
    let response = envelope_response(StatusCode::BAD_GATEWAY, &Unserializable);
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("the body is in memory already");
    assert_eq!(&body[..], b"Bad Gateway");
}
