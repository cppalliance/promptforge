//! The `web_search` tool: proxy a search query through the gateway.
//!
//! This tool does not talk to a search provider directly. Instead it POSTs the
//! query to the gateway's `POST /v1/tools/web_search` endpoint with the shared
//! bearer token, so the vendor credential (the Brave API key) never leaves the
//! server. The gateway's JSON results are returned verbatim as a string, ready
//! to hand back to the model.

use crate::tools::{Tool, ToolId};
use crate::{Error, Result};

/// The largest error body kept for diagnostics, in characters.
const MAX_ERROR_BODY: usize = 2000;

/// A tool that searches the web by proxying through the gateway.
///
/// The tool holds a reusable [`reqwest::Client`] plus the gateway base URL and
/// the shared bearer token. Each call POSTs the search arguments to the
/// gateway, which owns the search provider credential.
#[derive(Debug, Clone)]
pub struct WebSearch {
    /// The HTTP client used for outbound requests.
    http: reqwest::Client,
    /// The gateway base URL, with any trailing slash trimmed.
    base_url: String,
    /// The shared bearer token presented to the gateway.
    token: String,
}

impl WebSearch {
    /// Construct a `WebSearch` bound to a gateway base URL and bearer token.
    ///
    /// A trailing slash on `base_url` is trimmed, matching
    /// [`crate::client::GatewayClient::new`].
    #[must_use]
    pub fn new(base_url: &str, token: impl Into<String>) -> WebSearch {
        WebSearch {
            http: reqwest::Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
            token: token.into(),
        }
    }
}

#[async_trait::async_trait]
impl Tool for WebSearch {
    fn id(&self) -> ToolId {
        ToolId::new("promptforge", "web_search")
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
        "Search the web and return a list of results (title, url, description, age, site_name, extra_snippets)."
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

    async fn call(&self, args: serde_json::Value) -> Result<String> {
        // Validate the query argument before spending a network round-trip.
        if args
            .get("query")
            .and_then(serde_json::Value::as_str)
            .is_none()
        {
            return Err(Error::Parse("web_search: missing query argument".into()));
        }

        let response = self
            .http
            .post(format!("{}/tools/web_search", self.base_url))
            .bearer_auth(&self.token)
            .json(&args)
            .send()
            .await
            .map_err(Error::http)?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            let body: String = body.chars().take(MAX_ERROR_BODY).collect();
            let body = if body.is_empty() {
                "(empty body)".to_string()
            } else {
                body
            };
            return Err(Error::Backend {
                status: status.as_u16(),
                body,
            });
        }

        response.text().await.map_err(Error::http)
    }
}

#[cfg(test)]
mod tests {
    use super::WebSearch;
    use crate::tools::{Tool, ToolId};

    use std::net::SocketAddr;

    use axum::Json;
    use axum::Router;
    use axum::http::HeaderMap;
    use axum::routing::post;
    use serde_json::Value;

    #[test]
    fn descriptor_is_stable_and_faithful() {
        let tool = WebSearch::new("http://localhost", "test");

        assert_eq!(tool.id(), ToolId::new("promptforge", "web_search"));
        assert_eq!(tool.wire_name(), "web_search");
        assert_eq!(
            tool.description(),
            "Search the web and return a list of results (title, url, description, age, site_name, extra_snippets)."
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
    async fn forwards_query_and_returns_results() {
        let addr = spawn_mock().await;
        let tool = WebSearch::new(&format!("http://{addr}"), "tok");

        let raw = tool
            .call(serde_json::json!({ "query": "hi" }))
            .await
            .expect("call should succeed");

        let parsed: Value = serde_json::from_str(&raw).expect("response should be valid JSON");
        assert_eq!(
            parsed["results"][0]["title"].as_str(),
            Some("T"),
            "expected the canned result title to survive the round-trip"
        );
    }

    #[tokio::test]
    async fn rejects_missing_query() {
        let tool = WebSearch::new("http://127.0.0.1:0", "tok");
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
