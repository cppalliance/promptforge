//! The `web_search` tool: proxy a search query through the gateway.
//!
//! This tool does not talk to a search provider directly. Instead it POSTs the
//! query to the gateway's `POST /v1/tools/web_search` endpoint with the shared
//! bearer token, so the vendor credential (the Brave API key) never leaves the
//! server. The gateway's JSON results are validated for shape and returned as
//! an untrusted string, ready to hand back to the model.

use std::fmt;
use std::time::Duration;

use promptforge_tools::{Tool, ToolError, ToolErrorKind, ToolId, ToolOutput};

use crate::endpoint::Endpoint;
use crate::secret::Token;

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
/// `https://gateway.example.com/v1`). The root is validated at construction,
/// which requires an `http`/`https` scheme and a host and rejects embedded
/// credentials, a query, or a fragment; any trailing slash is trimmed. Each
/// call composes `{root}/tools/web_search`.
///
/// # Token handling
/// The bearer token is stored redacted, so it never appears in `Debug` output
/// and is never printed. It rides the `Authorization` header on each request
/// and never appears in an argument body or an error message.
#[derive(Clone)]
#[non_exhaustive]
pub struct WebSearch {
    /// The HTTP client used for outbound requests (carries the deadline).
    http: reqwest::Client,
    /// The gateway base URL, with any trailing slash trimmed.
    base_url: String,
    /// The shared bearer token presented to the gateway, redacted in `Debug`.
    token: Token,
}

impl fmt::Debug for WebSearch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Manual `Debug` (no derive): the token is a secret, so redact it here
        // rather than relying on the token wrapper's own redaction transitively.
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
    /// The root is parsed and normalized at construction and an empty token is
    /// rejected, so an invalid endpoint or credential fails here rather than
    /// during a tool call. The HTTP client is built with a fixed request
    /// deadline so a stalled gateway cannot hang a call indefinitely.
    ///
    /// # Errors
    /// Returns a [`ToolError`] with [`ToolErrorKind::InvalidArguments`] when
    /// `base_url` is not a valid gateway API root or `token` is empty, or with
    /// [`ToolErrorKind::Transport`] when the HTTP client cannot be built.
    ///
    /// # Examples
    /// ```
    /// use promptforge_web_search::WebSearch;
    ///
    /// let tool = WebSearch::new("https://gateway.example.com/v1", "bearer-token")?;
    /// // The token is redacted, never printed.
    /// assert!(format!("{tool:?}").contains("<redacted>"));
    ///
    /// assert!(WebSearch::new("not-a-url", "bearer-token").is_err());
    /// assert!(WebSearch::new("https://gateway.example.com/v1", "").is_err());
    /// # Ok::<(), promptforge_tools::ToolError>(())
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
        let endpoint = Endpoint::new(base_url).map_err(|error| {
            ToolError::with_source(format!("web_search: invalid gateway URL: {error}"), error)
                .with_kind(ToolErrorKind::InvalidArguments)
        })?;
        let token = Token::new(token).map_err(|error| {
            ToolError::with_source(format!("web_search: gateway token {error}"), error)
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
mod tests;
