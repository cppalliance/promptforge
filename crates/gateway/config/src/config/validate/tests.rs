//! Tests for the URL gate and the web-search knob vocabularies.

use super::{is_iso_date, is_valid_freshness, is_valid_safesearch, validate_http_url};

#[test]
fn http_url_accepts_http_and_https_with_host() {
    assert!(validate_http_url("ctx", "http://127.0.0.1:9").is_ok());
    assert!(validate_http_url("ctx", "https://api.example.com/res/v1").is_ok());
}

#[test]
fn http_url_rejects_missing_scheme_and_bad_scheme() {
    assert!(validate_http_url("ctx", "not-a-url").is_err());
    assert!(validate_http_url("ctx", "ftp://example.com").is_err());
    assert!(validate_http_url("ctx", "127.0.0.1:9").is_err());
}

#[test]
fn freshness_vocabulary() {
    for ok in ["", "pd", "pw", "pm", "py", "2024-01-01to2024-12-31"] {
        assert!(is_valid_freshness(ok), "expected {ok:?} to be valid");
    }
    for bad in [
        "daily",
        "p1",
        "2024/01/01to2024/12/31",
        "2024-1-1to2024-12-31",
    ] {
        assert!(!is_valid_freshness(bad), "expected {bad:?} to be invalid");
    }
}

#[test]
fn safesearch_vocabulary() {
    for ok in ["", "off", "moderate", "strict"] {
        assert!(is_valid_safesearch(ok));
    }
    for bad in ["on", "medium", "safe"] {
        assert!(!is_valid_safesearch(bad));
    }
}

#[test]
fn iso_date_shape() {
    assert!(is_iso_date("2024-01-01"));
    assert!(!is_iso_date("2024-1-01"));
    assert!(!is_iso_date("2024-01-01T"));
}
