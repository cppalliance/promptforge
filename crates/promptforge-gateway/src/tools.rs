//! Built-in tool endpoints the gateway exposes.
//!
//! The gateway holds the search provider credential, so the executor above it
//! never sees it. This module implements the `web_search` tool: a bearer-authed
//! `POST /v1/tools/web_search` that proxies a query to the Brave Search API and
//! returns a trimmed result set.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::check_auth;
use crate::config::{Secret, WebSearchConfig};
use crate::error::GatewayError;
use crate::web_search_process::post_process_results;


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
            default_count: cfg.default_count,
            max_count: cfg.max_count,
            max_per_host: cfg.max_per_host,
            default_freshness: cfg.default_freshness.clone(),
            default_safesearch: cfg.default_safesearch.clone(),
            strip_tracking: cfg.strip_tracking,
        }
    }
}

/// The web-search runtime state: the provider credential, the base URL, and a
/// shared HTTP client.
#[derive(Debug)]
pub(crate) struct WebSearchState {
    /// The credential sent to the search provider.
    api_key: Secret,
    /// The search API base URL.
    base_url: String,
    /// Cloneable tool settings derived from config.
    pub settings: WebSearchSettings,
    /// The shared HTTP client used for provider calls.
    http: reqwest::Client,
}

impl WebSearchState {
    /// Build web-search state from its configuration.
    #[must_use]
    pub(crate) fn new(cfg: &WebSearchConfig) -> WebSearchState {
        // v0 supports only the Brave provider; the query path below is
        // Brave-shaped. Reading the provider keeps the selection explicit.
        let crate::config::SearchProvider::Brave = cfg.provider;
        WebSearchState {
            api_key: cfg.api_key.clone(),
            base_url: cfg.base_url.trim_end_matches('/').to_string(),
            settings: WebSearchSettings::from_config(cfg),
            http: crate::http_util::bounded_client(),
        }
    }
}

/// The request body for `POST /v1/tools/web_search`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WebSearchRequest {
    /// The search query.
    pub query: String,
    /// The desired number of results. Defaults to
    /// [`WebSearchSettings::default_count`] and is clamped to
    /// [`WebSearchSettings::max_count`].
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
#[derive(Debug, Serialize)]
pub(crate) struct WebSearchResponse {
    /// The trimmed request query that produced these results.
    pub query: String,
    /// The trimmed search results.
    pub results: Vec<SearchResult>,
}

/// One search result, trimmed to the fields the executor needs.
#[derive(Debug, Serialize)]
pub(crate) struct SearchResult {
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

/// The Brave response envelope, narrowed to the `web.results` array.
#[derive(Deserialize)]
struct BraveResponse {
    web: Option<BraveWeb>,
}

/// The `web` object of a Brave response.
#[derive(Deserialize)]
struct BraveWeb {
    #[serde(default)]
    results: Vec<BraveResult>,
}

/// One Brave result, narrowed to the fields the executor needs.
#[derive(Deserialize)]
struct BraveResult {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    age: Option<String>,
    #[serde(default)]
    extra_snippets: Vec<String>,
}

/// Parameters for a Brave Search API request.
#[derive(Debug, Clone)]
struct BraveSearchParams<'a> {
    /// The trimmed search query (`q`).
    query: &'a str,
    /// Over-fetched result count sent to Brave (`count`).
    count: u8,
    /// Optional freshness filter.
    freshness: Option<&'a str>,
    /// Optional country code.
    country: Option<&'a str>,
    /// Optional search language.
    search_lang: Option<&'a str>,
    /// Optional SafeSearch level.
    safesearch: Option<&'a str>,
}

/// Trim ASCII whitespace from `query` and reject empty values.
///
/// # Errors
/// Returns [`GatewayError::MalformedRequest`] with
/// `"web_search: empty query"` when the trimmed query is empty.
fn trim_web_search_query(query: &str) -> Result<String, GatewayError> {
    let trimmed = query
        .trim_matches(|c: char| c.is_ascii_whitespace())
        .to_string();
    if trimmed.is_empty() {
        return Err(GatewayError::MalformedRequest(
            "web_search: empty query".to_string(),
        ));
    }
    Ok(trimmed)
}

/// Clamp the requested count into `1..=max_count`.
#[must_use]
fn clamp_count(requested: u8, max_count: u8) -> u8 {
    let max_count = max_count.max(1);
    requested.clamp(1, max_count)
}

/// Compute the Brave over-fetch count from a clamped requested count.
///
/// `brave_count = min(max_count, requested_count.saturating_mul(3).max(requested_count))`
#[must_use]
fn brave_overfetch_count(requested_count: u8, max_count: u8) -> u8 {
    let max_count = max_count.max(1);
    let over = requested_count.saturating_mul(3).max(requested_count);
    over.min(max_count)
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

/// Prefix Brave upstream errors with `web_search: `.
fn prefix_web_search_upstream(err: GatewayError) -> GatewayError {
    match err {
        GatewayError::UpstreamStatus { status, body } => GatewayError::UpstreamStatus {
            status,
            body: if body.starts_with("web_search: ") {
                body
            } else {
                format!("web_search: {body}")
            },
        },
        GatewayError::UpstreamTransport(source) => GatewayError::UpstreamTransport(Box::new(
            WebSearchUpstream(format!("web_search: {source}")),
        )),
        other => other,
    }
}

/// Transport error wrapper so the source chain carries the `web_search: ` prefix.
#[derive(Debug)]
struct WebSearchUpstream(String);

impl std::fmt::Display for WebSearchUpstream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for WebSearchUpstream {}

/// Build Brave `/web/search` query pairs from [`BraveSearchParams`].
///
/// Always includes `extra_snippets=true`. Optional knobs are omitted when `None`.
#[must_use]
fn brave_search_query(params: &BraveSearchParams<'_>) -> Vec<(&'static str, String)> {
    let mut query = vec![
        ("q", params.query.to_string()),
        ("count", params.count.to_string()),
        ("extra_snippets", "true".to_string()),
    ];
    if let Some(freshness) = params.freshness {
        query.push(("freshness", freshness.to_string()));
    }
    if let Some(country) = params.country {
        query.push(("country", country.to_string()));
    }
    if let Some(search_lang) = params.search_lang {
        query.push(("search_lang", search_lang.to_string()));
    }
    if let Some(safesearch) = params.safesearch {
        query.push(("safesearch", safesearch.to_string()));
    }
    query
}

/// Call the Brave Search API and map `web.results` to [`SearchResult`] values.
///
/// Always sends `extra_snippets=true`. Optional knobs are omitted when `None`.
///
/// # Errors
/// Returns [`GatewayError::UpstreamTransport`] on a transport failure and
/// [`GatewayError::UpstreamStatus`] on a non-success provider status. Both are
/// prefixed with `web_search: ` on the body or source message.
async fn brave_search(
    http: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    params: &BraveSearchParams<'_>,
) -> Result<Vec<SearchResult>, GatewayError> {
    let query = brave_search_query(params);

    let response = http
        .get(format!("{base_url}/web/search"))
        .query(&query)
        .header("X-Subscription-Token", api_key)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| prefix_web_search_upstream(GatewayError::upstream_transport(e)))?;

    let status = response.status();
    if !status.is_success() {
        let body =
            crate::http_util::read_body_capped(response, crate::http_util::MAX_ERROR_BODY).await;
        let body: String = body.chars().take(2000).collect();
        return Err(prefix_web_search_upstream(GatewayError::UpstreamStatus {
            status: status.as_u16(),
            body,
        }));
    }

    let parsed: BraveResponse = response
        .json()
        .await
        .map_err(|e| prefix_web_search_upstream(GatewayError::upstream_transport(e)))?;

    let results = parsed
        .web
        .map(|web| web.results)
        .unwrap_or_default()
        .into_iter()
        .map(|r| SearchResult {
            title: r.title,
            url: r.url,
            description: r.description,
            age: r.age,
            site_name: None,
            extra_snippets: r.extra_snippets,
        })
        .collect();

    Ok(results)
}

/// The `POST /v1/tools/web_search` route: bearer-authed, proxies to Brave.
///
/// # Errors
/// Returns [`GatewayError::Unauthorized`] when the bearer token is absent or
/// wrong, [`GatewayError::ToolNotConfigured`] when no `[tools.web_search]`
/// section is present, [`GatewayError::MalformedRequest`] when `query` is empty
/// after trimming, and the upstream variants on a provider failure.
pub(crate) async fn web_search(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<WebSearchRequest>,
) -> Result<Json<WebSearchResponse>, GatewayError> {
    check_auth(&state, &headers).await?;
    let web_search = state
        .web_search()
        .await
        .ok_or(GatewayError::ToolNotConfigured("web_search"))?;
    let query = trim_web_search_query(&request.query)?;
    let count = clamp_count(
        request.count.unwrap_or(web_search.settings.default_count),
        web_search.settings.max_count,
    );
    let brave_count = brave_overfetch_count(count, web_search.settings.max_count);
    let params = BraveSearchParams {
        query: &query,
        count: brave_count,
        freshness: resolve_freshness(
            request.freshness.as_deref(),
            &web_search.settings.default_freshness,
        ),
        country: non_empty_opt(request.country.as_deref()),
        search_lang: non_empty_opt(request.search_lang.as_deref()),
        safesearch: resolve_safesearch(
            request.safesearch.as_deref(),
            &web_search.settings.default_safesearch,
        ),
    };
    let mapped = brave_search(
        &web_search.http,
        &web_search.base_url,
        web_search.api_key.expose(),
        &params,
    )
    .await?;
    let results = post_process_results(
        mapped,
        web_search.settings.strip_tracking,
        &request.include_domains,
        &request.exclude_domains,
        web_search.settings.max_per_host,
        count,
    );
    Ok(Json(WebSearchResponse { query, results }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::GatewayError;

    #[test]
    fn empty_query_is_malformed_request() {
        for query in ["", "   ", "\t\n"] {
            let err = trim_web_search_query(query).expect_err("empty query");
            match err {
                GatewayError::MalformedRequest(message) => {
                    assert_eq!(message, "web_search: empty query");
                }
                other => panic!("expected MalformedRequest, got {other:?}"),
            }
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
        let err = prefix_web_search_upstream(GatewayError::UpstreamStatus {
            status: 429,
            body: "rate limited".to_string(),
        });
        match err {
            GatewayError::UpstreamStatus { body, .. } => {
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
    use super::{BraveSearchParams, brave_search};

    /// Hits the real Brave Search API to validate the request shape and the
    /// `web.results` parsing against Brave's actual JSON.
    ///
    /// Ignored by default so the normal test run needs no credential. Run it
    /// manually with `BRAVE_API_KEY` set in the environment:
    /// `cargo test -p promptforge-gateway -- --ignored live_brave_search --nocapture`
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
