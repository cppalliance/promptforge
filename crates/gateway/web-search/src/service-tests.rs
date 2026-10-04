//! Tests for `web_search` request validation, knob resolution, and the Brave query shape.

use super::*;
use crate::brave::{brave_search_query, prefix_web_search_upstream};
use crate::error::WebSearchError;

#[test]
fn empty_query_is_malformed_request() {
    for query in ["", "   ", "\t\n"] {
        let err = trim_web_search_query(query).expect_err("empty query");
        match err {
            WebSearchError::MalformedRequest(message) => {
                assert_eq!(message, "web_search: empty query");
            }
            other => panic!("expected MalformedRequest, got {other:?}"),
        }
    }
}

fn knob_request(freshness: &str, safesearch: &str, country: &str, lang: &str) -> WebSearchRequest {
    WebSearchRequest {
        query: "q".to_string(),
        count: None,
        freshness: Some(freshness.to_string()),
        country: Some(country.to_string()),
        search_lang: Some(lang.to_string()),
        safesearch: Some(safesearch.to_string()),
        include_domains: Vec::new(),
        exclude_domains: Vec::new(),
    }
}

#[test]
fn validate_request_knobs_accepts_valid_and_empty() {
    // TOOLS-004: valid closed-vocab and locale codes pass; empty knobs are
    // omitted downstream and need no validation.
    assert!(validate_request_knobs(&knob_request("pd", "moderate", "us", "en")).is_ok());
    assert!(
        validate_request_knobs(&knob_request("2024-01-01to2024-12-31", "off", "GB", "eng")).is_ok()
    );
    assert!(validate_request_knobs(&knob_request("", "", "", "")).is_ok());
}

#[test]
fn validate_request_knobs_rejects_malformed() {
    // TOOLS-004: arbitrary strings are rejected at the boundary, not
    // forwarded to the provider.
    for req in [
        knob_request("daily", "", "", ""),
        knob_request("", "medium", "", ""),
        knob_request("", "", "usa", ""),
        knob_request("", "", "", "english"),
        knob_request("", "", "1", ""),
    ] {
        assert!(matches!(
            validate_request_knobs(&req),
            Err(WebSearchError::MalformedRequest(_))
        ));
    }
}

#[test]
fn validate_domain_filters_accepts_valid_and_rejects_malformed() {
    // WSP-006: bare valid domains pass (lowercased); malformed entries are
    // rejected with a clear MalformedRequest, not silently forwarded.
    assert_eq!(
        validate_domain_filters("include", &["Example.COM".into(), "sub.a-b.co".into()])
            .expect("valid domains"),
        vec!["example.com".to_string(), "sub.a-b.co".to_string()]
    );
    // An empty list means "no filter" and stays Ok.
    assert!(
        validate_domain_filters("exclude", &[])
            .expect("empty list")
            .is_empty()
    );
    for bad in [
        "",                    // empty
        "   ",                 // whitespace-only
        "https://example.com", // scheme
        "example.com/path",    // path
        "exa mple.com",        // embedded space
        "exa$mple.com",        // invalid character
        "example.com:8080",    // port
        "-bad.com",            // label starts with hyphen
        "bad-.com",            // label ends with hyphen
        "a..b.com",            // empty label
    ] {
        let err = validate_domain_filters("include", &[bad.to_string()])
            .expect_err(&format!("{bad:?} must be rejected"));
        assert!(
            matches!(err, WebSearchError::MalformedRequest(_)),
            "{err:?}"
        );
    }
}

#[test]
fn non_empty_query_is_trimmed() {
    assert_eq!(
        trim_web_search_query("  C++ Alliance  ").expect("ok"),
        "C++ Alliance"
    );
}

#[test]
fn brave_overfetch_uses_triple_capped_by_max() {
    assert_eq!(brave_overfetch_count(5, 20), 15);
    assert_eq!(brave_overfetch_count(10, 20), 20);
    assert_eq!(brave_overfetch_count(1, 20), 3);
    assert_eq!(brave_overfetch_count(20, 20), 20);
}

#[test]
fn clamp_count_bounds_to_one_through_max() {
    assert_eq!(clamp_count(0, 20), 1);
    assert_eq!(clamp_count(5, 20), 5);
    assert_eq!(clamp_count(50, 20), 20);
}

#[test]
fn resolve_knobs_prefer_request_then_defaults() {
    assert_eq!(resolve_freshness(Some("pd"), "pw"), Some("pd"));
    assert_eq!(resolve_freshness(Some(""), "pw"), Some("pw"));
    assert_eq!(resolve_freshness(None, ""), None);
    assert_eq!(resolve_safesearch(None, "moderate"), Some("moderate"));
    assert_eq!(non_empty_opt(Some("us")), Some("us"));
    assert_eq!(non_empty_opt(Some("  ")), None);
}

#[test]
fn prefix_web_search_upstream_prefixes_status_body() {
    let err = prefix_web_search_upstream(gateway_protocol::ProtocolError::upstream_status(
        429,
        "rate limited".to_string(),
    ));
    match err {
        gateway_protocol::ProtocolError::UpstreamStatus { body, .. } => {
            assert_eq!(body, "web_search: rate limited");
        }
        other => panic!("expected UpstreamStatus, got {other:?}"),
    }
}

#[test]
fn brave_search_query_always_sets_extra_snippets_and_optional_knobs() {
    let base = BraveSearchParams {
        query: "C++ Alliance",
        count: 15,
        freshness: None,
        country: None,
        search_lang: None,
        safesearch: None,
    };
    let pairs = brave_search_query(&base);
    assert_eq!(
        pairs,
        vec![
            ("q", "C++ Alliance".to_string()),
            ("count", "15".to_string()),
            ("extra_snippets", "true".to_string()),
        ]
    );

    let full = BraveSearchParams {
        query: "boost",
        count: 9,
        freshness: Some("pd"),
        country: Some("us"),
        search_lang: Some("en"),
        safesearch: Some("moderate"),
    };
    let pairs = brave_search_query(&full);
    assert_eq!(
        pairs,
        vec![
            ("q", "boost".to_string()),
            ("count", "9".to_string()),
            ("extra_snippets", "true".to_string()),
            ("freshness", "pd".to_string()),
            ("country", "us".to_string()),
            ("search_lang", "en".to_string()),
            ("safesearch", "moderate".to_string()),
        ]
    );
}
