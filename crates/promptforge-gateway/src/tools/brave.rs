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

impl From<BraveResult> for SearchResult {
    fn from(result: BraveResult) -> SearchResult {
        SearchResult {
            title: result.title,
            url: result.url,
            description: result.description,
            age: result.age,
            site_name: None,
            extra_snippets: result.extra_snippets,
        }
    }
}

/// Maps a decoded Brave envelope to the executor-facing result list.
///
/// Pure and deterministic, so the mapping is unit-tested without a live call.
fn map_brave_response(parsed: BraveResponse) -> Vec<SearchResult> {
    parsed
        .web
        .map(|web| web.results)
        .unwrap_or_default()
        .into_iter()
        .map(SearchResult::from)
        .collect()
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
        GatewayError::UpstreamTransport(source) => {
            GatewayError::UpstreamTransport(Box::new(WebSearchUpstream { source }))
        }
        other => other,
    }
}

/// Transport error context wrapper that preserves the underlying cause via
/// `source()` (TOOLS-008) rather than flattening it into a string.
#[derive(Debug)]
struct WebSearchUpstream {
    source: Box<dyn std::error::Error + Send + Sync>,
}

impl std::fmt::Display for WebSearchUpstream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("web_search upstream request failed")
    }
}

impl std::error::Error for WebSearchUpstream {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

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

    // Bounded success body read with explicit result handling (TOOLS-009): a
    // transport failure mid-body is surfaced, then the capped bytes are decoded.
    let bytes = crate::http_util::read_bytes_capped(response, crate::http_util::MAX_JSON_BODY)
        .await
        .map_err(|e| prefix_web_search_upstream(GatewayError::upstream_transport(e)))?;
    let parsed: BraveResponse = serde_json::from_slice(&bytes)
        .map_err(|e| prefix_web_search_upstream(GatewayError::UpstreamTransport(Box::new(e))))?;

    Ok(map_brave_response(parsed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_brave_response_maps_web_results() {
        let json = serde_json::json!({
            "web": { "results": [
                {
                    "title": "T",
                    "url": "https://a.com/1",
                    "description": "d",
                    "age": "1 day ago",
                    "extra_snippets": ["s"]
                }
            ]}
        });
        let parsed: BraveResponse = serde_json::from_value(json).expect("parse");
        let mapped = map_brave_response(parsed);
        assert_eq!(mapped.len(), 1);
        assert_eq!(mapped[0].title, "T");
        assert_eq!(mapped[0].url, "https://a.com/1");
        assert_eq!(mapped[0].age.as_deref(), Some("1 day ago"));
        assert!(mapped[0].site_name.is_none());
        assert_eq!(mapped[0].extra_snippets, vec!["s".to_owned()]);
    }

    #[test]
    fn map_brave_response_is_empty_without_web_object() {
        let parsed: BraveResponse =
            serde_json::from_value(serde_json::json!({})).expect("parse empty");
        assert!(map_brave_response(parsed).is_empty());
    }
}
