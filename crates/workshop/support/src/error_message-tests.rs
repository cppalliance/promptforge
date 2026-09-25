//! Tests for the shared error-message rendering: the production message
//! stays at the error's own `Display` text, the debug message appends the
//! source chain, and the leak flag tracks debug assertions.

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
