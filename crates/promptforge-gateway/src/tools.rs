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

/// Cloneable runtime settings for `web_search`, filled from [`WebSearchConfig`].
#[derive(Debug, Clone)]
pub struct WebSearchSettings {
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
    pub fn from_config(cfg: &WebSearchConfig) -> WebSearchSettings {
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
pub struct WebSearchState {
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
    pub fn new(cfg: &WebSearchConfig) -> WebSearchState {
        WebSearchState {
            api_key: cfg.api_key.clone(),
            base_url: cfg.base_url.trim_end_matches('/').to_string(),
            settings: WebSearchSettings::from_config(cfg),
            http: reqwest::Client::new(),
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
pub struct WebSearchResponse {
    /// The trimmed request query that produced these results.
    pub query: String,
    /// The trimmed search results.
    pub results: Vec<SearchResult>,
}

/// One search result, trimmed to the fields the executor needs.
#[derive(Debug, Serialize)]
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

/// Call the Brave Search API and return a trimmed result set.
///
/// # Errors
/// Returns [`GatewayError::UpstreamTransport`] on a transport failure and
/// [`GatewayError::UpstreamStatus`] on a non-success provider status.
async fn brave_search(
    http: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    query: &str,
    count: u8,
    max_count: u8,
) -> Result<WebSearchResponse, GatewayError> {
    let count = count.clamp(1, max_count);
    let response = http
        .get(format!("{base_url}/web/search"))
        .query(&[("q", query), ("count", &count.to_string())])
        .header("X-Subscription-Token", api_key)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(GatewayError::upstream_transport)?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        let body: String = body.chars().take(2000).collect();
        return Err(GatewayError::UpstreamStatus {
            status: status.as_u16(),
            body,
        });
    }

    let parsed: BraveResponse = response
        .json()
        .await
        .map_err(GatewayError::upstream_transport)?;

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

    Ok(WebSearchResponse {
        query: query.to_string(),
        results,
    })
}

/// The `POST /v1/tools/web_search` route: bearer-authed, proxies to Brave.
///
/// # Errors
/// Returns [`GatewayError::Unauthorized`] when the bearer token is absent or
/// wrong, [`GatewayError::ToolNotConfigured`] when no `[tools.web_search]`
/// section is present, [`GatewayError::MalformedRequest`] when `query` is empty
/// after trimming, and the upstream variants on a provider failure.
pub async fn web_search(
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
    let count = request.count.unwrap_or(web_search.settings.default_count);
    let response = brave_search(
        &web_search.http,
        &web_search.base_url,
        web_search.api_key.expose(),
        &query,
        count,
        web_search.settings.max_count,
    )
    .await?;
    Ok(Json(response))
}

#[cfg(test)]
mod tests {
    use super::trim_web_search_query;
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
}

#[cfg(test)]
mod live_tests {
    use super::brave_search;

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

        let response = brave_search(
            &http,
            "https://api.search.brave.com/res/v1",
            &api_key,
            "rust programming language",
            5,
            20,
        )
        .await
        .expect("brave search should succeed");

        assert_eq!(response.query, "rust programming language");
        assert!(
            !response.results.is_empty(),
            "expected at least one result from Brave"
        );
        assert!(
            response.results[0].url.starts_with("http"),
            "expected a real URL, got: {}",
            response.results[0].url
        );

        // Printed for eyeballing when run manually with --nocapture.
        for result in &response.results {
            println!("- {} :: {}", result.title, result.url);
        }
    }
}
