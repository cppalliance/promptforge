//! Tests for the shared state-bucket validator: each refusal, an accepted
//! put, and the cheapest-refusal-first order the route boundary promises.

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
fn a_valid_body_parses_to_its_value() {
    let put = StateBucketValue::new("alpha", &KEYS, br#"{"n":1}"#).expect("a valid body");
    assert_eq!(put.value(), &serde_json::json!({ "n": 1 }));
}

#[test]
fn a_valid_put_holds_its_allow_list_key_and_compact_text() {
    let put = StateBucketValue::new("beta", &KEYS, br#"{ "n" : [1, 2] }"#).expect("a valid body");
    assert_eq!(put.key(), "beta");
    assert_eq!(
        put.text(),
        r#"{"n":[1,2]}"#,
        "the text is the value's compact serialization, not the body"
    );
    assert_eq!(put.into_value(), serde_json::json!({ "n": [1, 2] }));
}

#[test]
fn a_non_json_body_is_refused() {
    assert!(matches!(
        StateBucketValue::new("alpha", &KEYS, b"{"),
        Err(StateBucketError::NotJson { .. })
    ));
}

#[test]
fn the_key_is_judged_before_the_body() {
    // A foreign key wins over a body that is also invalid.
    assert!(matches!(
        StateBucketValue::new("gamma", &KEYS, b"{"),
        Err(StateBucketError::Key(_))
    ));
}

#[test]
fn the_size_is_judged_before_the_shape() {
    // A body past the cap is refused as too-large even though it also
    // fails to parse.
    let oversized = vec![b'x'; STATE_BUCKET_VALUE_CAP + 1];
    assert!(matches!(
        StateBucketValue::new("alpha", &KEYS, &oversized),
        Err(StateBucketError::TooLarge { .. })
    ));
}

#[test]
fn a_body_whose_compact_text_passes_the_cap_is_refused() {
    // `1e15` is four bytes of body but eighteen of compact text
    // (`1000000000000000.0`), so this body fits while its text does not.
    let body = format!(
        "[{}]",
        vec!["1e15"; STATE_BUCKET_VALUE_CAP / 18 + 1].join(",")
    );
    assert!(body.len() <= STATE_BUCKET_VALUE_CAP, "the body fits");
    assert!(matches!(
        StateBucketValue::new("alpha", &KEYS, body.as_bytes()),
        Err(StateBucketError::TooLarge { actual, cap })
            if actual > STATE_BUCKET_VALUE_CAP && cap == STATE_BUCKET_VALUE_CAP
    ));
}
