//! Test support: the JSON shape check and the text completion fixture.

use serde::Serialize;

use crate::client::{Completion, CompletionResult};

/// Asserts that `value` serializes to the JSON value `expected`.
#[track_caller]
pub(crate) fn assert_json<T: Serialize>(value: &T, expected: &serde_json::Value) {
    assert_eq!(&serde_json::to_value(value).expect("serializes"), expected);
}

/// A `pong` text reply from `model`, built by the validating constructor.
#[track_caller]
pub(crate) fn pong(model: &str) -> Completion {
    Completion::from_result(CompletionResult::Text("pong".to_owned()), model)
        .expect("a text result is accepted")
}
