//! [`WebFetch`], the `web_fetch` tool, and its [`Tool`] implementation.
//!
//! `WebFetch` composes the URL-admission policy, the guarded DNS resolver, the
//! per-hop redirect policy, and the bounded body reads into one safe fetch. The
//! client is built with no ambient proxy, no automatic `Referer`, no cookie
//! store, and no default credentials, so no request includes an ambient
//! identity on any hop.

use std::sync::Arc;

use reqwest::header::CONTENT_TYPE;

use harness_capabilities::Tool;
use promptforge::tools::{ToolError, ToolErrorKind, ToolId, ToolOutput};

use crate::config::{ConfigError, FetchConfig};
use crate::error::{Disposition, FetchError, SafeUrl};
use crate::redirect::redirect_policy;
use crate::resolver::GuardedResolver;
use crate::response::{
    Extraction, Route, classify, decode_body, extract_html, read_body_capped, read_body_truncating,
    truncate_to_chars,
};
use crate::url_policy::check_url;

/// The result the [`Tool::call`] boundary returns: untrusted page text on
/// success, a narrow [`ToolError`] on a hard failure.
type CallResult = Result<ToolOutput, ToolError>;

/// A tool that fetches a web page and returns its main content as markdown.
///
/// The tool holds a reusable [`reqwest::Client`] so repeated calls share a
/// connection pool, plus the validated [`FetchConfig`] policy it enforces.
///
/// # Examples
/// ```
/// use std::sync::Arc;
///
/// use harness_webfetch::WebFetch;
///
/// let tool = WebFetch::new();
/// let shared: Arc<dyn harness_capabilities::Tool> = Arc::new(tool);
/// assert_eq!(shared.wire_name(), "web_fetch");
/// ```
#[derive(Debug, Clone)]
pub struct WebFetch {
    /// The HTTP client used for outbound requests.
    http: reqwest::Client,
    /// The validated security policy applied to each fetch.
    config: Arc<FetchConfig>,
}

/// Builds the hardened HTTP client for `config`.
///
/// Installs the [`GuardedResolver`] so every connection is made only to an
/// address the policy allows, and the per-hop [`redirect_policy`]. Disables
/// ambient proxies (`no_proxy`) and automatic `Referer` (`referer(false)`), and
/// sets no cookie store and no default headers, so no request sends an ambient
/// identity on any hop, including after a redirect.
fn build_client(config: &Arc<FetchConfig>) -> Result<reqwest::Client, reqwest::Error> {
    let resolver = Arc::new(GuardedResolver::system(Arc::clone(config)));
    reqwest::Client::builder()
        .dns_resolver(resolver)
        .redirect(redirect_policy((**config).clone()))
        .no_proxy()
        .referer(false)
        .connect_timeout(config.connect_timeout())
        .timeout(config.timeout())
        .pool_idle_timeout(config.pool_idle_timeout())
        .user_agent(config.user_agent())
        .build()
}

impl WebFetch {
    /// Constructs a `WebFetch` with the built-in default policy.
    ///
    /// The default policy is a compile-time-valid constant, so no policy field
    /// can cause a failure.
    ///
    /// # Panics
    /// Panics only if the underlying HTTP client cannot be built for the default
    /// policy, which means the TLS backend failed to initialize: a defect in the
    /// environment, not a condition a caller can act on. Use
    /// [`WebFetch::try_with_config`] for a fallible constructor.
    ///
    /// # Examples
    /// ```
    /// use harness_webfetch::WebFetch;
    ///
    /// let tool = WebFetch::new();
    /// # let _ = tool;
    /// ```
    #[must_use]
    pub fn new() -> WebFetch {
        let config = Arc::new(FetchConfig::default());
        #[expect(
            clippy::expect_used,
            reason = "the default policy is a compile-time-valid constant; a build failure means the TLS backend could not initialize, a defect, not a caller-actionable condition"
        )]
        let http = build_client(&config)
            .expect("building the web_fetch client cannot fail with the default policy");
        WebFetch { http, config }
    }

    /// Constructs a `WebFetch` with a validated custom policy.
    ///
    /// # Errors
    /// Returns [`ConfigError`] if the HTTP client cannot be built for `config`
    /// (for example a TLS backend that fails to initialize). The policy itself
    /// is already validated by [`FetchConfig`] construction, so no policy field
    /// can trigger a failure here.
    ///
    /// # Examples
    /// ```
    /// use harness_webfetch::{FetchConfig, WebFetch};
    ///
    /// let policy = FetchConfig::builder().max_chars(10_000).build()?;
    /// let tool = WebFetch::try_with_config(policy)?;
    /// # let _ = tool;
    /// # Ok::<(), harness_webfetch::ConfigError>(())
    /// ```
    pub fn try_with_config(config: FetchConfig) -> Result<WebFetch, ConfigError> {
        let config = Arc::new(config);
        let http = build_client(&config).map_err(ConfigError::client_build)?;
        Ok(WebFetch { http, config })
    }

    /// Builds a `WebFetch` over an injected [`Lookup`] for tests.
    ///
    /// [`Lookup`]: crate::resolver::Lookup
    #[cfg(test)]
    pub(crate) fn with_lookup<L: crate::resolver::Lookup>(
        config: FetchConfig,
        lookup: L,
    ) -> WebFetch {
        let config = Arc::new(config);
        let resolver = Arc::new(GuardedResolver::new(lookup, Arc::clone(&config)));
        let http = reqwest::Client::builder()
            .dns_resolver(resolver)
            .redirect(redirect_policy((*config).clone()))
            .no_proxy()
            .referer(false)
            .connect_timeout(config.connect_timeout())
            .timeout(config.timeout())
            .pool_idle_timeout(config.pool_idle_timeout())
            .user_agent(config.user_agent())
            .build()
            .expect("the test client builds with a valid policy");
        WebFetch { http, config }
    }
}

impl Default for WebFetch {
    fn default() -> WebFetch {
        WebFetch::new()
    }
}

/// Maps a [`FetchError`] to a soft tool output or a hard `Err` by its
/// [`Disposition`].
fn soft_or_hard(err: &FetchError) -> CallResult {
    match err.classify() {
        Disposition::SoftOutput => Ok(ToolOutput::untrusted(err.model_facing())),
        Disposition::Hard(kind) => Err(ToolError::message(err.model_facing()).with_kind(kind)),
    }
}

/// Maps a body-read [`FetchError`] into soft, untrusted tool text.
///
/// A body-read failure is a size cap ([`FetchError::TooLarge`]) or a mid-stream
/// transport failure ([`FetchError::BodyRead`]); both are soft and returned as
/// model-facing tool text so the model can try a different URL.
fn body_read_outcome(err: &FetchError) -> ToolOutput {
    ToolOutput::untrusted(err.model_facing())
}

/// Maps a reqwest send error into either a soft tool result or a hard `Err`.
///
/// A refusal produced by the resolver or redirect policy appears as a
/// [`FetchError`] in the error source chain; its [`Disposition`] determines
/// the outcome. A bare transport failure with no such source is soft.
fn map_send_error_to_outcome(err: &reqwest::Error, url: &str) -> CallResult {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(current) = source {
        if let Some(fetch_err) = current.downcast_ref::<FetchError>() {
            return match fetch_err.classify() {
                Disposition::SoftOutput => Ok(ToolOutput::untrusted(fetch_err.model_facing())),
                Disposition::Hard(kind) => {
                    Err(ToolError::message(fetch_err.model_facing()).with_kind(kind))
                }
            };
        }
        source = current.source();
    }
    if err.is_timeout() {
        return Ok(ToolOutput::untrusted(
            FetchError::Timeout {
                url: SafeUrl::new(url),
            }
            .model_facing(),
        ));
    }
    Ok(ToolOutput::untrusted(format!(
        "fetch failed for {url}: network error; try a different URL"
    )))
}

#[async_trait::async_trait]
impl Tool for WebFetch {
    fn id(&self) -> ToolId {
        ToolId::from_validated("promptforge/web/fetch")
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Tool trait fixes this return type to &str, so the &'static str suggestion cannot be applied"
    )]
    fn wire_name(&self) -> &str {
        "web_fetch"
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Tool trait fixes this return type to &str, so the &'static str suggestion cannot be applied"
    )]
    fn description(&self) -> &str {
        "Fetch a web page and return its main content as markdown."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        let ceiling = self.config.max_chars();
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to fetch."
                },
                "max_chars": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": ceiling,
                    "description": "Maximum number of characters of text to return for this call. Clamped to the configured ceiling. Longer text is truncated on a character boundary and the result is flagged as truncated."
                },
                "raw": {
                    "type": "boolean",
                    "description": "Skip article extraction and render the whole HTML document. Use for a page that is mostly a table or list, where extraction would discard the content. Ignored for non-HTML responses. Defaults to false."
                }
            },
            "required": ["url"]
        })
    }

    async fn call(&self, args: serde_json::Value) -> CallResult {
        let url = args
            .get("url")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                ToolError::message("web_fetch: missing url argument")
                    .with_kind(ToolErrorKind::InvalidArguments)
            })?;

        // A per-call `max_chars` is clamped to the configured ceiling; absent,
        // the ceiling itself applies.
        let max_chars = parse_max_chars(&args, self.config.max_chars())?;

        let raw = parse_raw(&args)?;

        let url = match check_url(url, &self.config) {
            Ok(u) => u,
            Err(err) => return soft_or_hard(&err),
        };

        let response = match self.http.get(url.clone()).send().await {
            Ok(resp) => resp,
            Err(err) => return map_send_error_to_outcome(&err, url.as_str()),
        };

        let final_url = response.url().clone();

        let status = response.status();
        if !status.is_success() {
            let err = FetchError::HttpStatus {
                url: SafeUrl::new(final_url.as_str()),
                status: status.as_u16(),
            };
            return Ok(ToolOutput::untrusted(err.model_facing()));
        }

        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);

        let Some(content_type) = content_type else {
            return soft_or_hard(&FetchError::NoContentType {
                url: SafeUrl::new(final_url.as_str()),
            });
        };

        let parsed_mime: mime::Mime = match content_type.parse() {
            Ok(m) => m,
            Err(_) => {
                return soft_or_hard(&FetchError::UnsupportedContentType {
                    url: SafeUrl::new(final_url.as_str()),
                    content_type: content_type.clone(),
                });
            }
        };

        let Some(route) = classify(&parsed_mime) else {
            return soft_or_hard(&FetchError::UnsupportedContentType {
                url: SafeUrl::new(final_url.as_str()),
                content_type: content_type.clone(),
            });
        };

        let charset = parsed_mime
            .get_param(mime::CHARSET)
            .map(|name| name.as_str().to_owned());

        let max_bytes = self.config.max_bytes();
        let (decoded, extraction, size_truncated) = match route {
            Route::Html => {
                let body = match read_body_capped(response, final_url.as_str(), max_bytes).await {
                    Ok(b) => b,
                    Err(e) => return Ok(body_read_outcome(&e)),
                };
                let decoded = match decode_body(&body, charset.as_deref(), final_url.as_str()) {
                    Ok(d) => d,
                    Err(e) => return soft_or_hard(&e),
                };
                let (markdown, extraction) = extract_html(&decoded, Some(final_url.as_str()), raw);
                (markdown, extraction, false)
            }
            Route::Plain { structured: true } => {
                let body = match read_body_capped(response, final_url.as_str(), max_bytes).await {
                    Ok(b) => b,
                    Err(e) => return Ok(body_read_outcome(&e)),
                };
                let decoded = match decode_body(&body, charset.as_deref(), final_url.as_str()) {
                    Ok(d) => d,
                    Err(e) => return soft_or_hard(&e),
                };
                (decoded, Extraction::Plain, false)
            }
            Route::Plain { structured: false } => {
                let (body, size_truncated) = match read_body_truncating(response, max_bytes).await {
                    Ok(v) => v,
                    Err(e) => return Ok(body_read_outcome(&e)),
                };
                let decoded = match decode_body(&body, charset.as_deref(), final_url.as_str()) {
                    Ok(d) => d,
                    Err(e) => return soft_or_hard(&e),
                };
                (decoded, Extraction::Plain, size_truncated)
            }
        };

        let (text, char_truncated) = truncate_to_chars(&decoded, max_chars);
        let truncated = size_truncated || char_truncated;

        Ok(ToolOutput::untrusted(format!(
            "url: {final_url}\ntruncated: {truncated}\nextraction: {}\n\n{text}",
            extraction.label()
        )))
    }
}

/// Parses the optional `max_chars` argument, clamped to `ceiling`.
///
/// # Errors
/// Returns an invalid-arguments [`ToolError`] if `max_chars` is present but is
/// not a positive integer.
fn parse_max_chars(args: &serde_json::Value, ceiling: usize) -> Result<usize, ToolError> {
    let Some(value) = args.get("max_chars") else {
        return Ok(ceiling);
    };
    if value.is_null() {
        return Ok(ceiling);
    }
    let n = value.as_u64().filter(|n| *n >= 1).ok_or_else(|| {
        ToolError::message("web_fetch: max_chars must be a positive integer")
            .with_kind(ToolErrorKind::InvalidArguments)
    })?;
    let requested = usize::try_from(n).unwrap_or(usize::MAX);
    Ok(requested.min(ceiling))
}

/// Parses the optional `raw` argument, defaulting to `false`.
///
/// # Errors
/// Returns an invalid-arguments [`ToolError`] if `raw` is present and is neither
/// null nor a boolean.
fn parse_raw(args: &serde_json::Value) -> Result<bool, ToolError> {
    match args.get("raw") {
        None => Ok(false),
        Some(value) if value.is_null() => Ok(false),
        Some(value) => value.as_bool().ok_or_else(|| {
            ToolError::message("web_fetch: raw must be a boolean")
                .with_kind(ToolErrorKind::InvalidArguments)
        }),
    }
}

#[cfg(test)]
#[path = "tool-tests.rs"]
mod tests;
