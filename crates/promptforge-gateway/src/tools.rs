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

/// The default result count when the request omits one.
const DEFAULT_COUNT: u8 = 10;

/// The largest result count the gateway will request from the provider.
const MAX_COUNT: u8 = 20;

/// The web-search runtime state: the provider credential, the base URL, and a
/// shared HTTP client.
#[derive(Debug)]
pub struct WebSearchState {
    /// The credential sent to the search provider.
    api_key: Secret,
    /// The search API base URL.
    base_url: String,
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
    /// The desired number of results. Defaults to 10 and is clamped to 20.
    #[serde(default)]
    pub count: Option<u8>,
}

/// The response body for `POST /v1/tools/web_search`.
#[derive(Debug, Serialize)]
pub struct WebSearchResponse {
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
) -> Result<WebSearchResponse, GatewayError> {
    let count = count.clamp(1, MAX_COUNT);
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
        })
        .collect();

    Ok(WebSearchResponse { results })
}

/// The `POST /v1/tools/web_search` route: bearer-authed, proxies to Brave.
///
/// # Errors
/// Returns [`GatewayError::Unauthorized`] when the bearer token is absent or
/// wrong, [`GatewayError::ToolNotConfigured`] when no `[tools.web_search]`
/// section is present, and the upstream variants on a provider failure.
pub async fn web_search(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<WebSearchRequest>,
) -> Result<Json<WebSearchResponse>, GatewayError> {
    check_auth(&state, &headers)?;
    let web_search = state
        .web_search()
        .ok_or(GatewayError::ToolNotConfigured("web_search"))?;
    let count = request.count.unwrap_or(DEFAULT_COUNT);
    let response = brave_search(
        &web_search.http,
        &web_search.base_url,
        web_search.api_key.expose(),
        &request.query,
        count,
    )
    .await?;
    Ok(Json(response))
}
