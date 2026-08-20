//! The `web_search` tool: proxy a search query through the gateway.
//!
//! This tool does not talk to a search provider directly. Instead it POSTs the
//! query to the gateway's `POST /v1/tools/web_search` endpoint with the shared
//! bearer token, so the vendor credential (the Brave API key) never leaves the
//! server. The gateway's JSON results are validated for shape and returned as
//! an untrusted string, ready to hand back to the model.

use std::fmt;
use std::time::Duration;

use crate::client::{GatewayEndpoint, SecretString};
use crate::tools::{Tool, ToolError, ToolErrorKind, ToolId, ToolOutput};

/// The largest error body kept for diagnostics, in characters.
const MAX_ERROR_BODY: usize = 2000;

/// The largest successful response body accepted from the gateway, in bytes.
///
/// Search results carry third-party web content, so the body is bounded to keep
/// a hostile or misbehaving upstream from returning an unbounded payload. A body
/// past this cap is rejected rather than silently truncated, since a truncated
/// JSON document is not a valid result set.
const MAX_RESPONSE_BODY: usize = 256 * 1024;

/// The deadline applied to the HTTP client and every outbound request, so a
/// stalled gateway cannot hang a tool call (and thus a run) indefinitely.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// The largest accepted `query` string, in characters (Brave's documented cap).
const MAX_QUERY_LEN: usize = 400;
/// The inclusive upper bound on the requested result `count`.
const MAX_COUNT: u32 = 20;
/// The largest accepted free-form string argument (country, language, domain).
const MAX_STRING_LEN: usize = 128;
/// The largest number of hostnames accepted in a domain include/exclude list.
const MAX_DOMAINS: usize = 20;

/// A tool that searches the web by proxying through the gateway.
///
/// The tool holds a reusable [`reqwest::Client`] (with a request deadline) plus
/// the gateway base URL and the shared bearer token. Each call validates its
/// arguments, POSTs them to the gateway (which owns the search provider
/// credential), and returns the validated results as untrusted output.
///
/// # Accepted API root
/// [`WebSearch::new`] takes the gateway's OpenAI-shaped API root (for example
/// `https://gateway.example.com/v1`). The root is validated by
/// [`GatewayEndpoint`], which requires an `http`/`https` scheme and a host and
/// rejects embedded credentials, a query, or a fragment; any trailing slash is
/// trimmed. Each call composes `{root}/tools/web_search`.
///
/// # Token handling
/// The bearer token is stored as a [`SecretString`], so it is redacted from
/// `Debug` output and never printed. It rides the `Authorization` header on each
/// request and never appears in an argument body or an error message.
#[derive(Clone)]
#[non_exhaustive]
pub struct WebSearch {
    /// The HTTP client used for outbound requests (carries the deadline).
    http: reqwest::Client,
    /// The gateway base URL, with any trailing slash trimmed.
    base_url: String,
    /// The shared bearer token presented to the gateway, redacted in `Debug`.
    token: SecretString,
}

impl fmt::Debug for WebSearch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Manual `Debug` (no derive): the token is a secret, so redact it here
        // rather than relying on `SecretString`'s own redaction transitively.
        formatter
            .debug_struct("WebSearch")
            .field("base_url", &self.base_url)
            .field("token", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl WebSearch {
    /// Construct a `WebSearch` bound to a validated gateway API root and a
    /// non-empty bearer token.
    ///
    /// The root is parsed and normalized by [`GatewayEndpoint`] and an empty
    /// token is rejected, so an invalid endpoint or credential fails here rather
    /// than during a tool call. The HTTP client is built with a fixed request
    /// deadline so a stalled gateway cannot hang a call indefinitely.
    ///
    /// # Errors
    /// Returns a [`ToolError`] with [`ToolErrorKind::InvalidArguments`] when
    /// `base_url` is not a valid gateway API root or `token` is empty, or with
    /// [`ToolErrorKind::Transport`] when the HTTP client cannot be built.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::tools::WebSearch;
    ///
    /// let tool = WebSearch::new("https://gateway.example.com/v1", "bearer-token")?;
    /// // The token is redacted, never printed.
    /// assert!(format!("{tool:?}").contains("<redacted>"));
    ///
    /// assert!(WebSearch::new("not-a-url", "bearer-token").is_err());
    /// assert!(WebSearch::new("https://gateway.example.com/v1", "").is_err());
    /// # Ok::<(), promptforge_core::tools::ToolError>(())
    /// ```
    pub fn new(base_url: &str, token: impl Into<String>) -> Result<WebSearch, ToolError> {
        Self::with_timeout(base_url, token, REQUEST_TIMEOUT)
    }

    /// Construct a `WebSearch` with an explicit request deadline.
    ///
    /// Shared by [`WebSearch::new`] (default deadline) and tests (short deadline
    /// against a stalling mock), so the timeout is always injected rather than
    /// implicit.
    fn with_timeout(
        base_url: &str,
        token: impl Into<String>,
        timeout: Duration,
    ) -> Result<WebSearch, ToolError> {
        let endpoint = GatewayEndpoint::new(base_url).map_err(|error| {
            ToolError::message(format!("web_search: invalid gateway URL: {error}"))
                .with_kind(ToolErrorKind::InvalidArguments)
        })?;
        let token = SecretString::new(token).map_err(|error| {
            ToolError::message(format!("web_search: gateway token {error}"))
                .with_kind(ToolErrorKind::InvalidArguments)
        })?;
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|error| {
                ToolError::with_source("web_search: could not build HTTP client", error)
                    .with_kind(ToolErrorKind::Transport)
            })?;
        Ok(WebSearch {
            http,
            base_url: endpoint.url().to_owned(),
            token,
        })
    }
}

/// The freshness filter, deserialized as a closed enum so an unknown token is
/// rejected as an invalid argument rather than forwarded.
#[derive(Debug, Clone, Copy, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
enum Freshness {
    /// Past day.
    Pd,
    /// Past week.
    Pw,
    /// Past month.
    Pm,
    /// Past year.
    Py,
}

/// The SafeSearch level, deserialized as a closed enum.
#[derive(Debug, Clone, Copy, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
enum SafeSearch {
    /// No filtering.
    Off,
    /// Moderate filtering.
    Moderate,
    /// Strict filtering.
    Strict,
}

/// The validated search request forwarded to the gateway.
///
/// `deny_unknown_fields` means an argument the tool does not model is rejected
/// (rather than silently forwarded), and the typed optional fields reject a
/// wrong JSON type at deserialization. [`SearchRequest::validate`] then enforces
/// the string, count, and domain bounds. Only this validated value is
/// serialized onto the wire.
#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct SearchRequest {
    /// The search query.
    query: String,
    /// Maximum number of results.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    count: Option<u32>,
    /// Freshness filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    freshness: Option<Freshness>,
    /// Country code for the search.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    country: Option<String>,
    /// Search language code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    search_lang: Option<String>,
    /// SafeSearch level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    safesearch: Option<SafeSearch>,
    /// Only keep results from these hostnames.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    include_domains: Option<Vec<String>>,
    /// Drop results from these hostnames.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    exclude_domains: Option<Vec<String>>,
}

impl SearchRequest {
    /// Deserializes and validates the raw call arguments.
    fn from_args(args: serde_json::Value) -> Result<SearchRequest, ToolError> {
        let request: SearchRequest = serde_json::from_value(args).map_err(|error| {
            ToolError::with_source("web_search: invalid arguments", error)
                .with_kind(ToolErrorKind::InvalidArguments)
        })?;
        request.validate()?;
        Ok(request)
    }

    /// Enforces the bounds the type alone cannot express.
    fn validate(&self) -> Result<(), ToolError> {
        let invalid = |message: String| {
            ToolError::message(message).with_kind(ToolErrorKind::InvalidArguments)
        };
        if self.query.trim().is_empty() {
            return Err(invalid("web_search: query must not be empty".to_owned()));
        }
        if self.query.chars().count() > MAX_QUERY_LEN {
            return Err(invalid(format!(
                "web_search: query exceeds {MAX_QUERY_LEN} characters"
            )));
        }
        if let Some(count) = self.count
            && !(1..=MAX_COUNT).contains(&count)
        {
            return Err(invalid(format!(
                "web_search: count must be between 1 and {MAX_COUNT}"
            )));
        }
        for (field, value) in [
            ("country", &self.country),
            ("search_lang", &self.search_lang),
        ] {
            if let Some(value) = value
                && (value.trim().is_empty() || value.chars().count() > MAX_STRING_LEN)
            {
                return Err(invalid(format!(
                    "web_search: {field} must be 1..={MAX_STRING_LEN} characters"
                )));
            }
        }
        for (field, domains) in [
            ("include_domains", &self.include_domains),
            ("exclude_domains", &self.exclude_domains),
        ] {
            if let Some(domains) = domains {
                if domains.len() > MAX_DOMAINS {
                    return Err(invalid(format!(
                        "web_search: {field} may list at most {MAX_DOMAINS} hostnames"
                    )));
                }
                for domain in domains {
                    let bad = domain.trim().is_empty()
                        || domain.chars().count() > MAX_STRING_LEN
                        || domain.contains('/')
                        || domain.chars().any(|c| c.is_whitespace() || c.is_control());
                    if bad {
                        return Err(invalid(format!(
                            "web_search: {field} contains an invalid hostname"
                        )));
                    }
                }
            }
        }
        Ok(())
    }
}

/// The validated shape of a successful gateway response: an array of results,
/// each carrying at least a string `url`. Unknown fields are ignored so the
/// upstream can evolve, but a response missing `results` or a result missing a
/// non-empty `url` is rejected as malformed.
#[derive(serde::Deserialize)]
struct GatewayResults {
    /// The result rows.
    results: Vec<GatewayResult>,
}

/// One result row's shape-relevant field.
#[derive(serde::Deserialize)]
struct GatewayResult {
    /// The result URL; required and validated non-empty.
    url: String,
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

/// Reads at most `limit` bytes of a diagnostic body, stopping early once the cap
/// is reached. Used for the error path, where a truncated, lossy rendering is an
/// acceptable diagnostic.
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

/// Reads a success body, rejecting it once it would exceed `limit` bytes rather
/// than truncating (a truncated JSON document is not a valid result set), and
/// requiring valid UTF-8.
async fn read_capped(mut response: reqwest::Response, limit: usize) -> Result<String, ToolError> {
    let mut buffer: Vec<u8> = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|source| {
        ToolError::with_source("web_search: reading response failed", source)
            .with_kind(ToolErrorKind::Transport)
    })? {
        if buffer.len() + chunk.len() > limit {
            return Err(ToolError::message(format!(
                "web_search: response body exceeded {limit} bytes"
            ))
            .with_kind(ToolErrorKind::Backend));
        }
        buffer.extend_from_slice(&chunk);
    }
    String::from_utf8(buffer).map_err(|source| {
        ToolError::with_source("web_search: response body was not valid UTF-8", source)
            .with_kind(ToolErrorKind::Backend)
    })
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
            "additionalProperties": false,
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query.",
                    "minLength": 1,
                    "maxLength": MAX_QUERY_LEN
                },
                "count": {
                    "type": "integer",
                    "description": "Max number of results.",
                    "minimum": 1,
                    "maximum": MAX_COUNT
                },
                "freshness": {
                    "type": "string",
                    "description": "Freshness filter.",
                    "enum": ["pd", "pw", "pm", "py"]
                },
                "country": {
                    "type": "string",
                    "description": "Country code for the search.",
                    "maxLength": MAX_STRING_LEN
                },
                "search_lang": {
                    "type": "string",
                    "description": "Search language code.",
                    "maxLength": MAX_STRING_LEN
                },
                "safesearch": {
                    "type": "string",
                    "description": "SafeSearch level.",
                    "enum": ["off", "moderate", "strict"]
                },
                "include_domains": {
                    "type": "array",
                    "items": { "type": "string" },
                    "maxItems": MAX_DOMAINS,
                    "description": "Only keep results from these hostnames."
                },
                "exclude_domains": {
                    "type": "array",
                    "items": { "type": "string" },
                    "maxItems": MAX_DOMAINS,
                    "description": "Drop results from these hostnames."
                }
            },
            "required": ["query"]
        })
    }

    async fn call(&self, args: serde_json::Value) -> Result<ToolOutput, ToolError> {
        // Validate and normalize arguments before spending a network round-trip;
        // only the validated request is serialized onto the wire.
        let request = SearchRequest::from_args(args)?;

        let response = self
            .http
            .post(format!("{}/tools/web_search", self.base_url))
            .bearer_auth(self.token.expose())
            .json(&request)
            .send()
            .await
            .map_err(|source| {
                ToolError::with_source("web_search: request failed", source)
                    .with_kind(ToolErrorKind::Transport)
            })?;

        let status = response.status();
        if !status.is_success() {
            let code = status.as_u16();
            // The error body is external gateway content: bound the read and
            // sanitize control characters. If the body itself cannot be read,
            // keep the read failure as the returned error's `source()`.
            match read_bounded(response, MAX_ERROR_BODY).await {
                Ok(body) => {
                    let body = if body.is_empty() {
                        "(empty body)".to_owned()
                    } else {
                        sanitize_diagnostic(&body)
                    };
                    return Err(ToolError::message(format!(
                        "web_search: backend returned {code}: {body}"
                    ))
                    .with_kind(ToolErrorKind::Backend));
                }
                Err(source) => {
                    return Err(ToolError::with_source(
                        format!(
                            "web_search: backend returned {code}, and its error body could not be read"
                        ),
                        source,
                    )
                    .with_kind(ToolErrorKind::Backend));
                }
            }
        }

        // Success bodies carry third-party content: bound them (rejecting cap
        // overflow), then validate the promised JSON shape before returning it.
        let body = read_capped(response, MAX_RESPONSE_BODY).await?;
        let parsed: GatewayResults = serde_json::from_str(&body).map_err(|source| {
            ToolError::with_source("web_search: malformed search response", source)
                .with_kind(ToolErrorKind::Backend)
        })?;
        if let Some(index) = parsed.results.iter().position(|r| r.url.trim().is_empty()) {
            return Err(ToolError::message(format!(
                "web_search: malformed search response: result {index} has an empty url"
            ))
            .with_kind(ToolErrorKind::Backend));
        }

        // The validated results embed third-party titles, URLs, and
        // descriptions, so the body is marked untrusted: it is nonce-wrapped
        // before it can reach model input.
        Ok(ToolOutput::untrusted(body))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_COUNT, MAX_DOMAINS, MAX_ERROR_BODY, MAX_QUERY_LEN, MAX_RESPONSE_BODY, MAX_STRING_LEN,
        WebSearch,
    };
    use crate::tools::{OutputTrust, Tool, ToolErrorKind, ToolId};

    use std::net::SocketAddr;
    use std::time::Duration;

    use axum::Json;
    use axum::Router;
    use axum::http::HeaderMap;
    use axum::routing::post;
    use serde_json::Value;

    /// A mock gateway whose task is owned by the test: dropping it aborts the
    /// server task deterministically instead of leaking a detached task.
    struct MockServer {
        addr: SocketAddr,
        handle: tokio::task::JoinHandle<()>,
    }

    impl MockServer {
        /// Binds an ephemeral port, serves `router`, and returns the address.
        async fn spawn(router: Router) -> MockServer {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let handle = tokio::spawn(async move {
                let _ = axum::serve(listener, router).await;
            });
            MockServer { addr, handle }
        }

        fn url(&self) -> String {
            format!("http://{}", self.addr)
        }
    }

    impl Drop for MockServer {
        fn drop(&mut self) {
            self.handle.abort();
        }
    }

    /// A router serving the canned success result at the tool's endpoint.
    fn success_router() -> Router {
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
                "expected the validated query to be forwarded in the body"
            );
            Json(serde_json::json!({
                "results": [
                    { "title": "T", "url": "https://e.com", "description": "D" }
                ]
            }))
        }
        Router::new().route("/tools/web_search", post(web_search))
    }

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
                "additionalProperties": false,
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The search query.",
                        "minLength": 1,
                        "maxLength": MAX_QUERY_LEN
                    },
                    "count": {
                        "type": "integer",
                        "description": "Max number of results.",
                        "minimum": 1,
                        "maximum": MAX_COUNT
                    },
                    "freshness": {
                        "type": "string",
                        "description": "Freshness filter.",
                        "enum": ["pd", "pw", "pm", "py"]
                    },
                    "country": {
                        "type": "string",
                        "description": "Country code for the search.",
                        "maxLength": MAX_STRING_LEN
                    },
                    "search_lang": {
                        "type": "string",
                        "description": "Search language code.",
                        "maxLength": MAX_STRING_LEN
                    },
                    "safesearch": {
                        "type": "string",
                        "description": "SafeSearch level.",
                        "enum": ["off", "moderate", "strict"]
                    },
                    "include_domains": {
                        "type": "array",
                        "items": { "type": "string" },
                        "maxItems": MAX_DOMAINS,
                        "description": "Only keep results from these hostnames."
                    },
                    "exclude_domains": {
                        "type": "array",
                        "items": { "type": "string" },
                        "maxItems": MAX_DOMAINS,
                        "description": "Drop results from these hostnames."
                    }
                },
                "required": ["query"]
            })
        );
    }

    #[tokio::test]
    async fn forwards_query_and_returns_untrusted_results() {
        let mock = MockServer::spawn(success_router()).await;
        let tool = WebSearch::new(&mock.url(), "tok").expect("valid web search configuration");

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

    #[tokio::test]
    async fn forwards_validated_optional_fields() {
        async fn web_search(Json(body): Json<Value>) -> Json<Value> {
            assert_eq!(body.get("count").and_then(Value::as_u64), Some(5));
            assert_eq!(body.get("freshness").and_then(Value::as_str), Some("pw"));
            assert_eq!(
                body.get("safesearch").and_then(Value::as_str),
                Some("strict")
            );
            assert_eq!(
                body.get("include_domains"),
                Some(&serde_json::json!(["example.com"]))
            );
            Json(serde_json::json!({ "results": [{ "url": "https://e.com" }] }))
        }
        let mock =
            MockServer::spawn(Router::new().route("/tools/web_search", post(web_search))).await;
        let tool = WebSearch::new(&mock.url(), "tok").expect("valid web search configuration");

        tool.call(serde_json::json!({
            "query": "hi",
            "count": 5,
            "freshness": "pw",
            "safesearch": "strict",
            "include_domains": ["example.com"]
        }))
        .await
        .expect("a fully-specified valid request should succeed");
    }

    #[tokio::test]
    async fn rejects_missing_query() {
        let tool =
            WebSearch::new("http://127.0.0.1:0", "tok").expect("valid web search configuration");
        let err = tool
            .call(serde_json::json!({ "count": 3 }))
            .await
            .expect_err("missing query should be rejected before any network call");
        assert_eq!(err.kind(), ToolErrorKind::InvalidArguments);
    }

    #[tokio::test]
    async fn rejects_empty_and_oversized_query() {
        let tool =
            WebSearch::new("http://127.0.0.1:0", "tok").expect("valid web search configuration");
        assert_eq!(
            tool.call(serde_json::json!({ "query": "   " }))
                .await
                .expect_err("blank query")
                .kind(),
            ToolErrorKind::InvalidArguments
        );
        let long = "x".repeat(MAX_QUERY_LEN + 1);
        assert_eq!(
            tool.call(serde_json::json!({ "query": long }))
                .await
                .expect_err("oversized query")
                .kind(),
            ToolErrorKind::InvalidArguments
        );
    }

    #[tokio::test]
    async fn rejects_unknown_fields_and_bad_optional_types() {
        let tool =
            WebSearch::new("http://127.0.0.1:0", "tok").expect("valid web search configuration");
        // Unknown field.
        let err = tool
            .call(serde_json::json!({ "query": "hi", "nonsense": 1 }))
            .await
            .expect_err("unknown field must be rejected");
        assert_eq!(err.kind(), ToolErrorKind::InvalidArguments);
        assert!(
            std::error::Error::source(&err).is_some(),
            "a deserialization failure must preserve its serde source"
        );
        // Wrong type for count.
        assert_eq!(
            tool.call(serde_json::json!({ "query": "hi", "count": "five" }))
                .await
                .expect_err("count must be an integer")
                .kind(),
            ToolErrorKind::InvalidArguments
        );
        // Out-of-range count.
        assert_eq!(
            tool.call(serde_json::json!({ "query": "hi", "count": MAX_COUNT + 1 }))
                .await
                .expect_err("count above the cap")
                .kind(),
            ToolErrorKind::InvalidArguments
        );
        assert_eq!(
            tool.call(serde_json::json!({ "query": "hi", "count": 0 }))
                .await
                .expect_err("zero count")
                .kind(),
            ToolErrorKind::InvalidArguments
        );
        // Unknown enum values.
        assert_eq!(
            tool.call(serde_json::json!({ "query": "hi", "freshness": "yesterday" }))
                .await
                .expect_err("unknown freshness")
                .kind(),
            ToolErrorKind::InvalidArguments
        );
        assert_eq!(
            tool.call(serde_json::json!({ "query": "hi", "safesearch": "maybe" }))
                .await
                .expect_err("unknown safesearch")
                .kind(),
            ToolErrorKind::InvalidArguments
        );
    }

    #[tokio::test]
    async fn rejects_invalid_domain_lists() {
        let tool =
            WebSearch::new("http://127.0.0.1:0", "tok").expect("valid web search configuration");
        assert_eq!(
            tool.call(
                serde_json::json!({ "query": "hi", "include_domains": ["ok.com", "bad/host"] })
            )
            .await
            .expect_err("a hostname with a separator must be rejected")
            .kind(),
            ToolErrorKind::InvalidArguments
        );
        let many: Vec<String> = (0..30).map(|i| format!("h{i}.com")).collect();
        assert_eq!(
            tool.call(serde_json::json!({ "query": "hi", "exclude_domains": many }))
                .await
                .expect_err("too many hostnames must be rejected")
                .kind(),
            ToolErrorKind::InvalidArguments
        );
    }

    #[test]
    fn constructor_rejects_bad_urls_credentials_query_and_empty_token() {
        assert!(WebSearch::new("not-a-url", "tok").is_err(), "invalid URL");
        assert!(WebSearch::new("", "tok").is_err(), "empty URL");
        assert!(
            WebSearch::new("ftp://host/v1", "tok").is_err(),
            "non-http scheme"
        );
        assert!(
            WebSearch::new("http://user:pass@host/v1", "tok").is_err(),
            "embedded credentials must be rejected"
        );
        assert!(
            WebSearch::new("http://host/v1?q=1", "tok").is_err(),
            "a query component must be rejected"
        );
        assert!(
            WebSearch::new("http://host/v1#frag", "tok").is_err(),
            "a fragment must be rejected"
        );
        assert!(
            WebSearch::new("http://localhost", "").is_err(),
            "empty token must be rejected"
        );
        assert!(WebSearch::new("http://localhost", "tok").is_ok());
    }

    #[tokio::test]
    async fn transport_failure_is_transport_kind() {
        // Bind then drop the listener so the port is closed and the connection
        // is refused deterministically.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let tool = WebSearch::new(&format!("http://{addr}"), "tok")
            .expect("valid web search configuration");

        let err = tool
            .call(serde_json::json!({ "query": "hi" }))
            .await
            .expect_err("a refused connection must surface as an error");
        assert_eq!(err.kind(), ToolErrorKind::Transport);
        assert!(std::error::Error::source(&err).is_some());
    }

    #[tokio::test]
    async fn stalling_gateway_times_out_as_transport() {
        async fn web_search() -> Json<Value> {
            tokio::time::sleep(Duration::from_secs(30)).await;
            Json(serde_json::json!({ "results": [] }))
        }
        let mock =
            MockServer::spawn(Router::new().route("/tools/web_search", post(web_search))).await;
        let tool = WebSearch::with_timeout(&mock.url(), "tok", Duration::from_millis(200))
            .expect("valid web search configuration");

        let err = tool
            .call(serde_json::json!({ "query": "hi" }))
            .await
            .expect_err("a stalled gateway must surface as an error");
        assert_eq!(err.kind(), ToolErrorKind::Transport);
        assert!(
            std::error::Error::source(&err).is_some(),
            "the timeout must be preserved as the error's transport source"
        );
    }

    #[tokio::test]
    async fn malformed_success_json_is_backend_error_with_source() {
        async fn web_search() -> Json<Value> {
            // Missing the required `results` array: valid JSON, wrong shape.
            Json(serde_json::json!({ "unexpected": true }))
        }
        let mock =
            MockServer::spawn(Router::new().route("/tools/web_search", post(web_search))).await;
        let tool = WebSearch::new(&mock.url(), "tok").expect("valid web search configuration");

        let err = tool
            .call(serde_json::json!({ "query": "hi" }))
            .await
            .expect_err("a wrong-shaped success body must be rejected");
        assert_eq!(err.kind(), ToolErrorKind::Backend);
        assert!(
            std::error::Error::source(&err).is_some(),
            "a malformed response must preserve its parse source"
        );
    }

    #[tokio::test]
    async fn success_body_with_empty_url_is_rejected() {
        async fn web_search() -> Json<Value> {
            Json(serde_json::json!({ "results": [{ "url": "" }] }))
        }
        let mock =
            MockServer::spawn(Router::new().route("/tools/web_search", post(web_search))).await;
        let tool = WebSearch::new(&mock.url(), "tok").expect("valid web search configuration");

        let err = tool
            .call(serde_json::json!({ "query": "hi" }))
            .await
            .expect_err("an empty result url must be rejected");
        assert_eq!(err.kind(), ToolErrorKind::Backend);
    }

    #[tokio::test]
    async fn oversized_success_body_is_rejected() {
        async fn web_search() -> String {
            "x".repeat(MAX_RESPONSE_BODY + 4096)
        }
        let mock =
            MockServer::spawn(Router::new().route("/tools/web_search", post(web_search))).await;
        let tool = WebSearch::new(&mock.url(), "tok").expect("valid web search configuration");

        let err = tool
            .call(serde_json::json!({ "query": "hi" }))
            .await
            .expect_err("an oversized success body must be rejected, not truncated");
        assert_eq!(err.kind(), ToolErrorKind::Backend);
        assert!(
            err.to_string().contains("exceeded"),
            "the error must name the cap overflow: {err}"
        );
    }

    #[tokio::test]
    async fn oversized_error_body_is_bounded_and_sanitized() {
        async fn web_search() -> (axum::http::StatusCode, String) {
            // Oversized and control-laden so both bounding and sanitization run.
            let mut body = "line-one\nline-two\ttab".to_owned();
            body.push_str(&"e".repeat(MAX_ERROR_BODY * 4));
            (axum::http::StatusCode::INTERNAL_SERVER_ERROR, body)
        }
        let mock =
            MockServer::spawn(Router::new().route("/tools/web_search", post(web_search))).await;
        let tool = WebSearch::new(&mock.url(), "tok").expect("valid web search configuration");

        let err = tool
            .call(serde_json::json!({ "query": "hi" }))
            .await
            .expect_err("a 500 response must surface as an error");
        let message = err.to_string();
        assert!(
            message.contains("backend returned 500"),
            "error must name the status: {message}"
        );
        assert!(
            !message.contains('\n') && !message.contains('\t'),
            "control characters must be escaped, got: {message}"
        );
        assert!(
            message.len() < MAX_ERROR_BODY + 128,
            "the error-path body must be bounded, got {} bytes",
            message.len()
        );
    }

    /// A raw TCP mock that promises a large body via `Content-Length`, sends a
    /// few bytes, then drops the connection so the error-body read fails partway.
    #[tokio::test]
    async fn error_body_read_failure_is_preserved_as_source() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = socket.read(&mut buf).await;
                let header = "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 100000\r\n\r\n";
                let _ = socket.write_all(header.as_bytes()).await;
                let _ = socket.write_all(b"partial").await;
                let _ = socket.flush().await;
            }
        });
        let tool = WebSearch::new(&format!("http://{addr}"), "tok")
            .expect("valid web search configuration");

        let err = tool
            .call(serde_json::json!({ "query": "hi" }))
            .await
            .expect_err("a truncated 500 body must surface as an error");
        assert_eq!(err.kind(), ToolErrorKind::Backend);
        assert!(
            std::error::Error::source(&err).is_some(),
            "the body-read failure must be preserved as the error's source, got: {err}"
        );
        handle.abort();
    }
}
