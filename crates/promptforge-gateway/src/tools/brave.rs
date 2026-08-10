//! The Brave Search provider: request shaping, HTTP call, and response mapping.
//!
//! Isolated from the `web_search` route so the provider wire format and the
//! gateway's tool endpoint evolve independently. The route builds a
//! [`BraveSearchParams`] and calls [`brave_search`]; everything Brave-specific
//! (query pairs, over-fetch policy, JSON shape, error prefixing) lives here.

use serde::Deserialize;

use super::SearchResult;
use crate::error::GatewayError;

/// The Brave `/web/search` response envelope.
#[derive(Deserialize)]
pub(super) struct BraveResponse {
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
pub(crate) struct BraveSearchParams<'a> {
    /// The trimmed search query (`q`).
    pub(crate) query: &'a str,
    /// Over-fetched result count sent to Brave (`count`).
    pub(crate) count: u8,
    /// Optional freshness filter.
    pub(crate) freshness: Option<&'a str>,
    /// Optional country code.
    pub(crate) country: Option<&'a str>,
    /// Optional search language.
    pub(crate) search_lang: Option<&'a str>,
    /// Optional SafeSearch level.
    pub(crate) safesearch: Option<&'a str>,
}

/// Compute the Brave over-fetch count from a clamped requested count.
///
/// `brave_count = min(max_count, requested_count.saturating_mul(3).max(requested_count))`
#[must_use]
pub(crate) fn brave_overfetch_count(requested_count: u8, max_count: u8) -> u8 {
    let max_count = max_count.max(1);
    let over = requested_count.saturating_mul(3).max(requested_count);
    over.min(max_count)
}

/// Prefix Brave upstream errors with `web_search: `.
pub(crate) fn prefix_web_search_upstream(err: GatewayError) -> GatewayError {
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
pub(crate) fn brave_search_query(params: &BraveSearchParams<'_>) -> Vec<(&'static str, String)> {
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
pub(crate) async fn brave_search(
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
