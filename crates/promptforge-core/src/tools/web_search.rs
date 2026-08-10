//! The `web_search` tool: proxy a search query through the gateway.
//!
//! This tool does not talk to a search provider directly. Instead it POSTs the
//! query to the gateway's `POST /v1/tools/web_search` endpoint with the shared
//! bearer token, so the vendor credential (the Brave API key) never leaves the
//! server. The gateway's JSON results are returned verbatim as a string, ready
//! to hand back to the model.

use crate::client::{GatewayEndpoint, SecretString};
use crate::tools::{Tool, ToolError, ToolErrorKind, ToolId, ToolOutput};

/// The largest error body kept for diagnostics, in characters.
const MAX_ERROR_BODY: usize = 2000;

/// The largest successful response body accepted from the gateway, in bytes.
///
/// Search results carry third-party web content, so the body is bounded to keep
/// a hostile or misbehaving upstream from returning an unbounded payload.
const MAX_RESPONSE_BODY: usize = 256 * 1024;

/// A tool that searches the web by proxying through the gateway.
///
/// The tool holds a reusable [`reqwest::Client`] plus the gateway base URL and
/// the shared bearer token. Each call POSTs the search arguments to the
/// gateway, which owns the search provider credential. The token is a
/// [`SecretString`], so it is redacted from `Debug` output.
#[derive(Debug, Clone)]
pub struct WebSearch {
    /// The HTTP client used for outbound requests.
    http: reqwest::Client,
    /// The gateway base URL, with any trailing slash trimmed.
    base_url: String,
    /// The shared bearer token presented to the gateway, redacted in `Debug`.
    token: SecretString,
}

impl WebSearch {
    /// Construct a `WebSearch` bound to a validated gateway base URL and a
    /// non-empty bearer token.
    ///
    /// The URL is parsed and normalized by [`GatewayEndpoint`] (rejecting an
    /// empty or non-`http(s)` URL), and an empty token is rejected, so an
    /// invalid endpoint or credential fails here rather than during a tool call.
    ///
    /// # Errors
    /// Returns a [`ToolError`] with `InvalidArguments` kind when `base_url` is
    /// not a valid gateway URL or `token` is empty.
    pub fn new(base_url: &str, token: impl Into<String>) -> Result<WebSearch, ToolError> {
        let endpoint = GatewayEndpoint::new(base_url).map_err(|error| {
            ToolError::message(format!("web_search: invalid gateway URL: {error}"))
                .with_kind(ToolErrorKind::InvalidArguments)
        })?;
        let token = token.into();
        if token.is_empty() {
            return Err(
                ToolError::message("web_search: gateway token must not be empty")
                    .with_kind(ToolErrorKind::InvalidArguments),
            );
        }
        Ok(WebSearch {
            http: reqwest::Client::new(),
            base_url: endpoint.url().to_owned(),
            token: SecretString::new(token),
        })
    }
}

/// Escapes control characters in an external diagnostic body so a hostile
/// gateway cannot inject terminal/log control sequences or forge multiline
/// records through an error `Display`.
fn sanitize_diagnostic(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    for c in body.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                use std::fmt::Write as _;
                let _ = write!(out, "\\u{{{:04x}}}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out
}

/// Reads at most `limit` bytes of a response body, stopping early once the cap
/// is reached so an oversized payload cannot exhaust memory.
async fn read_bounded(mut response: reqwest::Response, limit: usize) -> Result<String, ToolError> {
    let mut buffer: Vec<u8> = Vec::new();
    while buffer.len() < limit {
        let chunk = response.chunk().await.map_err(|source| {
            ToolError::with_source("web_search: reading response failed", source)
                .with_kind(ToolErrorKind::Transport)
        })?;
        let Some(chunk) = chunk else { break };
        let take = (limit - buffer.len()).min(chunk.len());
        buffer.extend_from_slice(&chunk[..take]);
        if take < chunk.len() {
            break;
        }
    }
    Ok(String::from_utf8_lossy(&buffer).into_owned())
}

#[async_trait::async_trait]
impl Tool for WebSearch {
    fn id(&self) -> ToolId {
        ToolId::from_validated("promptforge", "web_search")
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Tool trait fixes this return type to &str, so the &'static str suggestion cannot be applied"
    )]
    fn wire_name(&self) -> &str {
        "web_search"
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Tool trait fixes this return type to &str, so the &'static str suggestion cannot be applied"
    )]
    fn description(&self) -> &str {
        // Keep this sentence aligned with shipped prompts/picker fixtures; knobs
        // live in parameters_schema so capability bind stays stable.
        "Search the web and return a list of results (title, url, description)."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query."
                },
                "count": {
                    "type": "integer",
                    "description": "Max number of results."
                },
                "freshness": {
                    "type": "string",
                    "description": "Freshness filter (for example pd, pw, pm, py)."
                },
                "country": {
                    "type": "string",
                    "description": "Country code for the search."
                },
                "search_lang": {
                    "type": "string",
                    "description": "Search language code."
                },
                "safesearch": {
                    "type": "string",
                    "description": "SafeSearch level (for example off, moderate, strict)."
                },
                "include_domains": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Only keep results from these hostnames."
                },
                "exclude_domains": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Drop results from these hostnames."
                }
            },
            "required": ["query"]
        })
    }

    async fn call(&self, args: serde_json::Value) -> Result<ToolOutput, ToolError> {
        // Validate the query argument before spending a network round-trip.
        if args
            .get("query")
            .and_then(serde_json::Value::as_str)
            .is_none()
        {
            return Err(ToolError::message("web_search: missing query argument")
                .with_kind(ToolErrorKind::InvalidArguments));
        }

        let response = self
            .http
            .post(format!("{}/tools/web_search", self.base_url))
            .bearer_auth(self.token.expose())
            .json(&args)
            .send()
            .await
            .map_err(|source| {
                ToolError::with_source("web_search: request failed", source)
                    .with_kind(ToolErrorKind::Transport)
            })?;

        let status = response.status();
        if !status.is_success() {
            let code = status.as_u16();
            // The error body is external gateway content: bound the read (same
            // streaming cap as the success path), sanitize control characters,
            // and preserve a read failure instead of masking it as empty.
            let body = match read_bounded(response, MAX_ERROR_BODY).await {
                Ok(body) if body.is_empty() => "(empty body)".to_owned(),
                Ok(body) => sanitize_diagnostic(&body),
                Err(_) => "(backend response body could not be read)".to_owned(),
            };
            return Err(
                ToolError::message(format!("web_search: backend returned {code}: {body}"))
                    .with_kind(ToolErrorKind::Backend),
            );
        }

        // Search results embed third-party titles, URLs, and descriptions, so
        // the body is bounded and marked untrusted: it is nonce-wrapped before it
        // can reach model input.
        let text = read_bounded(response, MAX_RESPONSE_BODY).await?;
        Ok(ToolOutput::untrusted(text))
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_ERROR_BODY, MAX_RESPONSE_BODY, WebSearch};
    use crate::tools::{OutputTrust, Tool, ToolId};

    use std::net::SocketAddr;

    use axum::Json;
    use axum::Router;
    use axum::http::HeaderMap;
    use axum::routing::post;
    use serde_json::Value;

    #[test]
    fn debug_never_leaks_the_bearer_token() {
        let tool = WebSearch::new("http://localhost", "super-secret-token")
            .expect("valid web search configuration");
        let rendered = format!("{tool:?}");
        assert!(
            !rendered.contains("super-secret-token"),
            "the bearer token must never appear in Debug output, got: {rendered}"
        );
        assert!(
            rendered.contains("<redacted>"),
            "the token field must be redacted, got: {rendered}"
        );
    }

    #[test]
    fn descriptor_is_stable_and_faithful() {
        let tool =
            WebSearch::new("http://localhost", "test").expect("valid web search configuration");

        assert_eq!(
            tool.id(),
            ToolId::new("promptforge", "web_search").expect("valid id")
        );
        assert_eq!(tool.wire_name(), "web_search");
        assert_eq!(
            tool.description(),
            "Search the web and return a list of results (title, url, description)."
        );
        assert_eq!(
            tool.parameters_schema(),
            serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The search query."
                    },
                    "count": {
                        "type": "integer",
                        "description": "Max number of results."
                    },
                    "freshness": {
                        "type": "string",
                        "description": "Freshness filter (for example pd, pw, pm, py)."
                    },
                    "country": {
                        "type": "string",
                        "description": "Country code for the search."
                    },
                    "search_lang": {
                        "type": "string",
                        "description": "Search language code."
                    },
                    "safesearch": {
                        "type": "string",
                        "description": "SafeSearch level (for example off, moderate, strict)."
                    },
                    "include_domains": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Only keep results from these hostnames."
                    },
                    "exclude_domains": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Drop results from these hostnames."
                    }
                },
                "required": ["query"]
            })
        );
    }

    /// Spawn the mock gateway on an ephemeral port and return its address.
    async fn spawn_mock() -> SocketAddr {
        async fn web_search(headers: HeaderMap, Json(body): Json<Value>) -> Json<Value> {
            let auth = headers
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default();
            assert_eq!(
                auth, "Bearer tok",
                "expected the bearer token to be forwarded"
            );
            assert_eq!(
                body.get("query").and_then(Value::as_str),
                Some("hi"),
                "expected the query to be forwarded in the body"
            );

            Json(serde_json::json!({
                "results": [
                    { "title": "T", "url": "https://e.com", "description": "D" }
                ]
            }))
        }

        let router = Router::new().route("/tools/web_search", post(web_search));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        addr
    }

    #[tokio::test]
    async fn forwards_query_and_returns_untrusted_results() {
        let addr = spawn_mock().await;
        let tool = WebSearch::new(&format!("http://{addr}"), "tok")
            .expect("valid web search configuration");

        let raw = tool
            .call(serde_json::json!({ "query": "hi" }))
            .await
            .expect("call should succeed");

        assert_eq!(
            raw.trust(),
            OutputTrust::Untrusted,
            "external search content must be marked untrusted"
        );
        let parsed: Value =
            serde_json::from_str(raw.text()).expect("response should be valid JSON");
        assert_eq!(
            parsed["results"][0]["title"].as_str(),
            Some("T"),
            "expected the canned result title to survive the round-trip"
        );
    }

    /// Spawn a mock gateway whose successful body exceeds the response cap.
    async fn spawn_oversized_mock() -> SocketAddr {
        async fn web_search() -> String {
            "x".repeat(MAX_RESPONSE_BODY + 4096)
        }

        let router = Router::new().route("/tools/web_search", post(web_search));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        addr
    }

    /// Spawn a mock gateway that fails with an oversized error body.
    async fn spawn_oversized_error_mock() -> SocketAddr {
        async fn web_search() -> (axum::http::StatusCode, String) {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "e".repeat(MAX_ERROR_BODY * 4),
            )
        }

        let router = Router::new().route("/tools/web_search", post(web_search));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        addr
    }

    #[tokio::test]
    async fn oversized_error_body_is_bounded() {
        let addr = spawn_oversized_error_mock().await;
        let tool = WebSearch::new(&format!("http://{addr}"), "tok")
            .expect("valid web search configuration");

        let err = tool
            .call(serde_json::json!({ "query": "hi" }))
            .await
            .expect_err("a 500 response must surface as an error");
        let message = err.to_string();
        assert!(
            message.contains("backend returned 500"),
            "error must name the status: {message}"
        );
        // The bounded error body plus the fixed prefix must stay small; the raw
        // body was MAX_ERROR_BODY * 4 bytes.
        assert!(
            message.len() < MAX_ERROR_BODY + 128,
            "the error-path body must be bounded, got {} bytes",
            message.len()
        );
    }

    #[test]
    fn constructor_rejects_invalid_url_and_empty_token() {
        assert!(WebSearch::new("not-a-url", "tok").is_err(), "invalid URL");
        assert!(WebSearch::new("", "tok").is_err(), "empty URL");
        assert!(
            WebSearch::new("http://localhost", "").is_err(),
            "empty token must be rejected"
        );
        assert!(WebSearch::new("http://localhost", "tok").is_ok());
    }

    #[tokio::test]
    async fn oversized_response_body_is_bounded_and_untrusted() {
        let addr = spawn_oversized_mock().await;
        let tool = WebSearch::new(&format!("http://{addr}"), "tok")
            .expect("valid web search configuration");

        let raw = tool
            .call(serde_json::json!({ "query": "hi" }))
            .await
            .expect("call should succeed");

        assert_eq!(
            raw.text().len(),
            MAX_RESPONSE_BODY,
            "an oversized response body must be truncated to the cap"
        );
        assert_eq!(
            raw.trust(),
            OutputTrust::Untrusted,
            "external search content must be marked untrusted"
        );
    }

    #[tokio::test]
    async fn rejects_missing_query() {
        let tool =
            WebSearch::new("http://127.0.0.1:0", "tok").expect("valid web search configuration");
        let err = tool
            .call(serde_json::json!({ "count": 3 }))
            .await
            .expect_err("missing query should be rejected before any network call");
        assert!(
            err.to_string().contains("missing query"),
            "expected a missing-query parse error, got: {err}"
        );
    }
}
