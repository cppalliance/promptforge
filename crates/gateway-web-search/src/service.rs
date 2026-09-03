//! The `web_search` service: request validation, the Brave provider call, and
//! result assembly.
//!
//! The gateway holds the search provider credential, so the executor above it
//! never sees it. This module implements the query path behind
//! `POST /v1/tools/web_search`: it proxies a query to the Brave Search API and
//! returns a trimmed result set. The gateway owns the route, the bearer-auth
//! check, and the profile-switch reload; this crate owns everything past auth.

use serde::{Deserialize, Serialize};

use gateway_config::{Secret, WebSearchConfig};

use crate::brave::{BraveSearchParams, brave_overfetch_count, brave_search};
use crate::error::WebSearchError;
use crate::process::post_process_results;

/// Cloneable runtime settings for `web_search`, filled from [`WebSearchConfig`].
#[derive(Debug, Clone)]
pub(crate) struct WebSearchSettings {
    /// Used when the request omits `count`.
    pub default_count: u8,
    /// Clamp and over-fetch ceiling for result counts.
    pub max_count: u8,
    /// Diversity cap per hostname group.
    pub max_per_host: u8,
    /// Applied when the request omits `freshness` and this is non-empty.
    pub default_freshness: String,
    /// Applied when the request omits `safesearch` and this is non-empty.
    pub default_safesearch: String,
    /// When true, scrub known tracking query params from result URLs.
    pub strip_tracking: bool,
}

impl WebSearchSettings {
    /// Build settings from the tool configuration.
    #[must_use]
    pub(crate) fn from_config(cfg: &WebSearchConfig) -> WebSearchSettings {
        WebSearchSettings {
            default_count: cfg.default_count(),
            max_count: cfg.max_count(),
            max_per_host: cfg.max_per_host(),
            default_freshness: cfg.default_freshness().to_owned(),
            default_safesearch: cfg.default_safesearch().to_owned(),
            strip_tracking: cfg.strip_tracking(),
        }
    }
}

/// The web-search runtime state: the provider credential, the base URL, and a
/// shared HTTP client.
#[derive(Debug)]
pub struct WebSearchState {
    /// The credential sent to the search provider.
    api_key: Secret,
    /// The search API base URL.
    base_url: String,
    /// Cloneable tool settings derived from config.
    settings: WebSearchSettings,
    /// The shared HTTP client used for provider calls.
    http: reqwest::Client,
}

impl WebSearchState {
    /// Build web-search state from its configuration.
    #[must_use]
    pub fn new(cfg: &WebSearchConfig) -> WebSearchState {
        // v0 supports only the Brave provider; the query path below is
        // Brave-shaped. Reading the provider keeps the selection explicit.
        let gateway_config::SearchProvider::Brave = cfg.provider() else {
            unreachable!("SearchProvider is non_exhaustive; wire up new providers here")
        };
        WebSearchState {
            api_key: cfg.api_key().clone(),
            base_url: cfg.base_url().trim_end_matches('/').to_string(),
            settings: WebSearchSettings::from_config(cfg),
            http: gateway_protocol::http_util::bounded_client(),
        }
    }
}

/// The request body for `POST /v1/tools/web_search`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebSearchRequest {
    /// The search query.
    pub query: String,
    /// The desired number of results. Defaults to
    /// [`WebSearchConfig::default_count`] and is clamped to
    /// [`WebSearchConfig::max_count`].
    #[serde(default)]
    pub count: Option<u8>,
    /// Freshness filter; empty or absent means omit from the provider query.
    #[serde(default)]
    pub freshness: Option<String>,
    /// Country code; empty or absent means omit from the provider query.
    #[serde(default)]
    pub country: Option<String>,
    /// Search language; empty or absent means omit from the provider query.
    #[serde(default)]
    pub search_lang: Option<String>,
    /// SafeSearch level; falls back to settings when omitted or empty.
    #[serde(default)]
    pub safesearch: Option<String>,
    /// Hostname include filter; empty means no include filter.
    #[serde(default)]
    pub include_domains: Vec<String>,
    /// Hostname exclude filter; empty means no exclude filter.
    #[serde(default)]
    pub exclude_domains: Vec<String>,
}

/// The response body for `POST /v1/tools/web_search`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WebSearchResponse {
    /// The trimmed request query that produced these results.
    pub query: String,
    /// The trimmed search results.
    pub results: Vec<SearchResult>,
}

/// One search result, trimmed to the fields the executor needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SearchResult {
    /// The result's title.
    pub title: String,
    /// The result's URL.
    pub url: String,
    /// A short description or snippet.
    pub description: String,
    /// The result's age, when the provider reports one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub age: Option<String>,
    /// Hostname derived from `url`, when parsing succeeds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site_name: Option<String>,
    /// Extra snippets from the provider, when present.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub extra_snippets: Vec<String>,
}

/// Maximum query length kept, in Unicode scalar values (TOOLS-004).
const MAX_QUERY_CHARS: usize = 512;

/// Trim Unicode whitespace from `query`, reject empty values, and cap length.
///
/// # Errors
/// Returns [`WebSearchError::MalformedRequest`] with
/// `"web_search: empty query"` when the trimmed query is empty.
fn trim_web_search_query(query: &str) -> Result<String, WebSearchError> {
    // Unicode-aware trim (TOOLS-004), not just ASCII whitespace.
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Err(WebSearchError::MalformedRequest(
            "web_search: empty query".to_string(),
        ));
    }
    // Cap by scalar count so an oversized query cannot bloat the provider call.
    Ok(trimmed.chars().take(MAX_QUERY_CHARS).collect())
}

/// Validate and canonicalize caller-supplied domain filters (WSP-006).
///
/// Each entry must be a bare hostname/domain, not a URL: non-empty, ASCII, no
/// scheme, path, port, or whitespace, and standard label syntax. A malformed
/// entry rejects the whole request with a clear error rather than being
/// silently dropped or leniently parsed and forwarded to the provider.
///
/// `field` is `"include"`/`"exclude"` for the error message. Returned domains
/// are lowercased for case-insensitive matching.
///
/// # Errors
/// Returns [`WebSearchError::MalformedRequest`] for any malformed entry.
fn validate_domain_filters(field: &str, domains: &[String]) -> Result<Vec<String>, WebSearchError> {
    domains
        .iter()
        .map(|raw| validate_domain_filter(field, raw))
        .collect()
}

/// Validate one caller-supplied domain filter entry (WSP-006).
fn validate_domain_filter(field: &str, raw: &str) -> Result<String, WebSearchError> {
    let domain = raw.trim();
    let malformed =
        || WebSearchError::MalformedRequest(format!("web_search: invalid {field} domain {raw:?}"));
    if domain.is_empty()
        || domain.len() > 253
        || domain.contains("://")
        || domain.contains('/')
        || domain.contains(':')
        || domain.chars().any(char::is_whitespace)
    {
        return Err(malformed());
    }
    let lower = domain.to_ascii_lowercase();
    if !is_valid_domain_syntax(&lower) {
        return Err(malformed());
    }
    Ok(lower)
}

/// Whether `domain` is dot-separated labels of `[a-z0-9-]`, each `1..=63` chars
/// and not starting or ending with `-` (at least one label).
fn is_valid_domain_syntax(domain: &str) -> bool {
    let mut labels = 0_usize;
    for label in domain.split('.') {
        if label.is_empty()
            || label.len() > 63
            || label.starts_with('-')
            || label.ends_with('-')
            || !label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return false;
        }
        labels += 1;
    }
    labels >= 1
}

/// Clamp the requested count into `1..=max_count`.
#[must_use]
fn clamp_count(requested: u8, max_count: u8) -> u8 {
    let max_count = max_count.max(1);
    requested.clamp(1, max_count)
}

/// Reject malformed request-supplied provider knobs at the boundary (TOOLS-004).
///
/// Empty/absent knobs are omitted downstream and need no validation; the config
/// defaults are already validated at load. This validates only caller-supplied,
/// non-empty values so an arbitrary string is never forwarded to the provider.
///
/// # Errors
/// Returns [`WebSearchError::MalformedRequest`] for an out-of-vocabulary
/// `freshness`/`safesearch` or a malformed `country`/`search_lang` code.
fn validate_request_knobs(request: &WebSearchRequest) -> Result<(), WebSearchError> {
    if let Some(freshness) = non_empty_opt(request.freshness.as_deref())
        && !is_valid_freshness(freshness)
    {
        return Err(WebSearchError::MalformedRequest(format!(
            "web_search: invalid freshness {freshness:?}"
        )));
    }
    if let Some(safesearch) = non_empty_opt(request.safesearch.as_deref())
        && !matches!(safesearch, "off" | "moderate" | "strict")
    {
        return Err(WebSearchError::MalformedRequest(format!(
            "web_search: invalid safesearch {safesearch:?}"
        )));
    }
    if let Some(country) = non_empty_opt(request.country.as_deref())
        && !is_alpha_code(country, 2, 2)
    {
        return Err(WebSearchError::MalformedRequest(format!(
            "web_search: invalid country {country:?}"
        )));
    }
    if let Some(lang) = non_empty_opt(request.search_lang.as_deref())
        && !is_alpha_code(lang, 2, 3)
    {
        return Err(WebSearchError::MalformedRequest(format!(
            "web_search: invalid search_lang {lang:?}"
        )));
    }
    Ok(())
}

/// Whether `value` is an accepted Brave freshness knob: one of `pd`/`pw`/`pm`/
/// `py`, or a `YYYY-MM-DDtoYYYY-MM-DD` date range.
fn is_valid_freshness(value: &str) -> bool {
    if matches!(value, "pd" | "pw" | "pm" | "py") {
        return true;
    }
    value
        .split_once("to")
        .is_some_and(|(from, to)| is_iso_date(from) && is_iso_date(to))
}

/// Whether `value` is `YYYY-MM-DD`.
fn is_iso_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| index == 4 || index == 7 || byte.is_ascii_digit())
}

/// Whether `value` is `min..=max` ASCII alphabetic characters (a locale code).
fn is_alpha_code(value: &str, min: usize, max: usize) -> bool {
    let len = value.chars().count();
    len >= min && len <= max && value.chars().all(|c| c.is_ascii_alphabetic())
}

/// Resolve an optional string knob: `Some` and non-empty after trim, else `None`.
fn non_empty_opt(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|s| !s.is_empty())
}

/// Resolve freshness: request value, else non-empty settings default, else omit.
fn resolve_freshness<'a>(request: Option<&'a str>, default_freshness: &'a str) -> Option<&'a str> {
    non_empty_opt(request).or_else(|| non_empty_opt(Some(default_freshness)))
}

/// Resolve safesearch: request value, else non-empty settings default, else omit.
fn resolve_safesearch<'a>(
    request: Option<&'a str>,
    default_safesearch: &'a str,
) -> Option<&'a str> {
    non_empty_opt(request).or_else(|| non_empty_opt(Some(default_safesearch)))
}

impl WebSearchState {
    /// Run a web search against the configured provider and post-process the
    /// results.
    ///
    /// The query is trimmed and capped, the closed-vocabulary knobs are
    /// checked, and malformed domain filters are rejected before any provider
    /// call (TOOLS-004, WSP-006).
    ///
    /// # Errors
    /// Returns [`WebSearchError::MalformedRequest`] when `query` is empty
    /// after trimming or an `include`/`exclude` domain filter is malformed,
    /// and [`WebSearchError::Protocol`] on a provider failure.
    pub async fn search(
        &self,
        request: &WebSearchRequest,
    ) -> Result<WebSearchResponse, WebSearchError> {
        let query = trim_web_search_query(&request.query)?;
        validate_request_knobs(request)?;
        // Reject malformed domain filters at the boundary before any provider
        // call (WSP-006).
        let include_domains = validate_domain_filters("include", &request.include_domains)?;
        let exclude_domains = validate_domain_filters("exclude", &request.exclude_domains)?;
        let count = clamp_count(
            request.count.unwrap_or(self.settings.default_count),
            self.settings.max_count,
        );
        let fetch_count = brave_overfetch_count(count, self.settings.max_count);
        let params = BraveSearchParams {
            query: &query,
            count: fetch_count,
            freshness: resolve_freshness(
                request.freshness.as_deref(),
                &self.settings.default_freshness,
            ),
            country: non_empty_opt(request.country.as_deref()),
            search_lang: non_empty_opt(request.search_lang.as_deref()),
            safesearch: resolve_safesearch(
                request.safesearch.as_deref(),
                &self.settings.default_safesearch,
            ),
        };
        let mapped =
            brave_search(&self.http, &self.base_url, self.api_key.expose(), &params).await?;
        let results = post_process_results(
            mapped,
            self.settings.strip_tracking,
            &include_domains,
            &exclude_domains,
            self.settings.max_per_host,
            count,
        );
        Ok(WebSearchResponse { query, results })
    }
}

#[cfg(test)]
mod tests {
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

    fn knob_request(
        freshness: &str,
        safesearch: &str,
        country: &str,
        lang: &str,
    ) -> WebSearchRequest {
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
            validate_request_knobs(&knob_request("2024-01-01to2024-12-31", "off", "GB", "eng"))
                .is_ok()
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
        let err = prefix_web_search_upstream(
            gateway_protocol::ProtocolError::upstream_status(
                429,
                "rate limited".to_string(),
            ),
        );
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
}

#[cfg(test)]
mod live_tests {
    use crate::brave::{BraveSearchParams, brave_search};

    /// Hits the real Brave Search API to validate the request shape and the
    /// `web.results` parsing against Brave's actual JSON.
    ///
    /// Ignored by default so the normal test run needs no credential. Run it
    /// manually with `BRAVE_API_KEY` set in the environment:
    /// `cargo test -p gateway-web-search -- --ignored live_brave_search --nocapture`
    #[tokio::test]
    #[ignore = "hits the real Brave API; requires BRAVE_API_KEY, run with --ignored"]
    async fn live_brave_search() {
        let api_key =
            std::env::var("BRAVE_API_KEY").expect("set BRAVE_API_KEY to run this live test");
        let http = reqwest::Client::new();
        let params = BraveSearchParams {
            query: "rust programming language",
            count: 5,
            freshness: None,
            country: None,
            search_lang: None,
            safesearch: None,
        };

        let results = brave_search(
            &http,
            "https://api.search.brave.com/res/v1",
            &api_key,
            &params,
        )
        .await
        .expect("brave search should succeed");

        assert!(
            !results.is_empty(),
            "expected at least one result from Brave"
        );
        assert!(
            results[0].url.starts_with("http"),
            "expected a real URL, got: {}",
            results[0].url
        );

        // Printed for eyeballing when run manually with --nocapture.
        for result in &results {
            println!("- {} :: {}", result.title, result.url);
        }
    }
}
