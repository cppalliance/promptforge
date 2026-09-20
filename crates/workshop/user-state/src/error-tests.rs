//! User-state error tests: the JSON cause behind a refused put stays
//! reachable as the shared wrapper, which transparent delegation would
//! otherwise hide.

use std::error::Error as _;

use super::*;

#[test]
fn the_not_json_variant_reaches_the_serde_error_through_the_shared_wrapper() {
    let Err(json) = serde_json::from_str::<serde_json::Value>("nope") else {
        panic!("`nope` must not parse as JSON");
    };
    let error = UserStateError::NotJson {
        source: json.into(),
    };
    let Some(cause) = error.source() else {
        panic!("the not-json variant carries its serde cause as source()");
    };
    let Some(wrapper) = cause.downcast_ref::<JsonSource>() else {
        panic!("the serde cause is the shared JsonSource");
    };
    assert!(wrapper.as_inner().is_syntax());
}
