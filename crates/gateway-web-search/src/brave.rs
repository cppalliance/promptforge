//! The Brave Search provider: request shaping, HTTP call, and response mapping.
//!
//! Isolated from the `web_search` route so the provider wire format and the
//! gateway's tool endpoint evolve independently. The route builds a
//! [`BraveSearchParams`] and calls [`brave_search`]; everything Brave-specific
//! (query pairs, over-fetch policy, JSON shape, error prefixing) lives here.

use serde::Deserialize;
use shared_protocol::ProtocolError;
use shared_protocol::http_util;

use crate::service::SearchResult;

/// Byte ceiling for a successful Brave response body (TOOLS-010).
const SUCCESS_BODY_CAP: usize = http_util::MAX_JSON_BODY;

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
pub(crate) fn prefix_web_search_upstream(err: ProtocolError) -> ProtocolError {
    prefix_protocol(err)
}

/// Prefix the protocol-level Brave upstream errors with `web_search: `.
fn prefix_protocol(err: ProtocolError) -> ProtocolError {
    match err {
        ProtocolError::UpstreamStatus { status, body, .. } => ProtocolError::upstream_status(
            status,
            if body.starts_with("web_search: ") {
                body
            } else {
                format!("web_search: {body}")
            },
        ),
        ProtocolError::UpstreamTransport(source, ..) => {
            ProtocolError::transport(WebSearchUpstream { source })
        }
        ProtocolError::UpstreamConnect(source, ..) => {
            ProtocolError::connect(WebSearchUpstream { source })
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
/// Returns [`ProtocolError::UpstreamConnect`] when the connection itself fails,
/// [`ProtocolError::UpstreamTransport`] on a mid-flight transport failure, and
/// [`ProtocolError::UpstreamStatus`] on a non-success provider status. All are
/// prefixed with `web_search: ` on the body or source message.
pub(crate) async fn brave_search(
    http: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    params: &BraveSearchParams<'_>,
) -> Result<Vec<SearchResult>, ProtocolError> {
    let query = brave_search_query(params);

    let response = http
        .get(format!("{base_url}/web/search"))
        .query(&query)
        .header("X-Subscription-Token", api_key)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| prefix_web_search_upstream(ProtocolError::upstream_transport(e)))?;

    let status = response.status();
    if !status.is_success() {
        // Error body: preserve a read failure instead of masquerading it as an
        // empty/short body (TOOLS-009/010).
        let body = match http_util::read_bytes_capped(response, http_util::MAX_ERROR_BODY).await {
            Ok(bytes) => String::from_utf8_lossy(&bytes).chars().take(2000).collect(),
            Err(error) => format!("<error body unreadable: {error}>"),
        };
        return Err(prefix_web_search_upstream(ProtocolError::upstream_status(
            status.as_u16(),
            body,
        )));
    }

    // Bounded success body read that *detects* oversize (TOOLS-010): read one
    // byte past the ceiling and reject a larger body rather than decoding a
    // truncated prefix. A transport failure mid-body is surfaced explicitly.
    let bytes = http_util::read_bytes_capped(response, SUCCESS_BODY_CAP + 1)
        .await
        .map_err(|e| prefix_web_search_upstream(ProtocolError::upstream_transport(e)))?;
    if bytes.len() > SUCCESS_BODY_CAP {
        return Err(prefix_web_search_upstream(ProtocolError::upstream_status(
            502,
            format!("response body exceeded {SUCCESS_BODY_CAP} bytes"),
        )));
    }
    let parsed: BraveResponse = serde_json::from_slice(&bytes)
        .map_err(|e| prefix_web_search_upstream(ProtocolError::upstream_protocol(e)))?;

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

#[cfg(test)]
mod provider_tests {
    use std::io::{Read as _, Write as _};
    use std::net::TcpListener;
    use std::thread::{self, JoinHandle};

    use super::*;

    /// A one-shot fake HTTP server: serves a single canned `(status, body)` and
    /// returns its base URL plus a join handle whose `io::Result` surfaces any
    /// serve-side transport failure (no ignored write errors).
    fn serve_once(status_line: &str, body: &str) -> (String, JoinHandle<std::io::Result<()>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake brave server");
        let addr = listener.local_addr().expect("addr");
        let response = format!(
            "HTTP/1.1 {status_line}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let handle = thread::spawn(move || -> std::io::Result<()> {
            let (mut stream, _) = listener.accept()?;
            let mut buf = [0_u8; 2048];
            // Read the request head (best effort; we serve a fixed response).
            let _read = stream.read(&mut buf)?;
            stream.write_all(response.as_bytes())?;
            stream.flush()?;
            Ok(())
        });
        (format!("http://{addr}"), handle)
    }

    fn params() -> BraveSearchParams<'static> {
        BraveSearchParams {
            query: "q",
            count: 5,
            freshness: None,
            country: None,
            search_lang: None,
            safesearch: None,
        }
    }

    #[tokio::test]
    async fn maps_a_successful_provider_response() {
        // TOOLS-011: deterministic success path against a local mock server.
        let (base, handle) = serve_once(
            "200 OK",
            r#"{"web":{"results":[{"title":"T","url":"https://a.com/1","description":"d"}]}}"#,
        );
        let results = brave_search(&reqwest::Client::new(), &base, "key", &params())
            .await
            .expect("brave search ok");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].url, "https://a.com/1");
        handle.join().expect("thread").expect("serve ok");
    }

    #[tokio::test]
    async fn non_success_status_is_a_prefixed_upstream_status() {
        // TOOLS-011: a provider error status surfaces as a prefixed UpstreamStatus.
        let (base, handle) = serve_once("429 Too Many Requests", "rate limited");
        let err = brave_search(&reqwest::Client::new(), &base, "key", &params())
            .await
            .expect_err("should fail");
        match err {
            ProtocolError::UpstreamStatus { status, body, .. } => {
                assert_eq!(status, 429);
                assert!(body.starts_with("web_search: "), "body was {body:?}");
            }
            other => panic!("expected UpstreamStatus, got {other:?}"),
        }
        handle.join().expect("thread").expect("serve ok");
    }

    #[tokio::test]
    async fn malformed_success_body_is_a_protocol_error() {
        // TOOLS-011: a 200 with a non-JSON body is a decode/protocol failure,
        // not a transport failure.
        let (base, handle) = serve_once("200 OK", "not json at all");
        let err = brave_search(&reqwest::Client::new(), &base, "key", &params())
            .await
            .expect_err("should fail");
        assert!(
            matches!(err, ProtocolError::UpstreamProtocol(..)),
            "expected UpstreamProtocol, got {err:?}"
        );
        handle.join().expect("thread").expect("serve ok");
    }
}
