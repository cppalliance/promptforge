//! Tests for the shared state-bucket validator: each refusal, an accepted
//! body, and the cheapest-refusal-first order the route boundary promises.

use super::*;

/// A two-key allow-list for the tests.
const KEYS: [&str; 2] = ["alpha", "beta"];
/// A small cap so the over-cap fixtures stay short.
const CAP: usize = 16;

#[test]
fn a_known_key_resolves_to_its_allow_list_entry() {
    assert_eq!(
        resolve_bucket_key("beta", &KEYS).expect("beta is allowed"),
        "beta"
    );
}

#[test]
fn an_unknown_key_is_refused() {
    assert!(matches!(
        resolve_bucket_key("gamma", &KEYS),
        Err(StateBucketError::Key(ref key)) if key == "gamma"
    ));
}

#[test]
fn a_body_at_the_cap_is_accepted() {
    assert!(check_bucket_cap(CAP, CAP).is_ok());
}

#[test]
fn a_body_past_the_cap_is_refused() {
    assert!(matches!(
        check_bucket_cap(CAP + 1, CAP),
        Err(StateBucketError::TooLarge { actual, cap })
            if actual == CAP + 1 && cap == CAP
    ));
}

#[test]
fn text_that_parses_is_accepted() {
    assert!(check_bucket_text(r#"{"n":1}"#).is_ok());
}

#[test]
fn text_that_does_not_parse_is_refused() {
    assert!(matches!(
        check_bucket_text("{ not json"),
        Err(StateBucketError::NotJson { .. })
    ));
}

#[test]
fn a_valid_body_parses_to_its_value() {
    let value = validate_bucket_body("alpha", &KEYS, br#"{"n":1}"#, CAP).expect("a valid body");
    assert_eq!(value, serde_json::json!({ "n": 1 }));
}

#[test]
fn a_non_json_body_is_refused() {
    assert!(matches!(
        validate_bucket_body("alpha", &KEYS, b"{", CAP),
        Err(StateBucketError::NotJson { .. })
    ));
}

#[test]
fn the_key_is_judged_before_the_body() {
    // A foreign key wins over a body that is also invalid.
    assert!(matches!(
        validate_bucket_body("gamma", &KEYS, b"{", CAP),
        Err(StateBucketError::Key(_))
    ));
}

#[test]
fn the_size_is_judged_before_the_shape() {
    // A body past the cap is refused as too-large even though it also
    // fails to parse.
    let oversized = vec![b'x'; CAP + 1];
    assert!(matches!(
        validate_bucket_body("alpha", &KEYS, &oversized, CAP),
        Err(StateBucketError::TooLarge { .. })
    ));
}
