//! [`WebFetch`], the `web_fetch` tool, and its [`Tool`] implementation.
//!
//! `WebFetch` composes the URL-admission policy, the guarded DNS resolver, the
//! per-hop redirect policy, and the bounded body reads into one safe fetch. The
//! client is built with no ambient proxy, no automatic `Referer`, no cookie
//! store, and no default credentials, so no request carries an ambient identity
//! on any hop.

use std::sync::Arc;

use reqwest::header::CONTENT_TYPE;

use promptforge_tools::{Tool, ToolError, ToolErrorKind, ToolId, ToolOutput};

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
/// use promptforge_webfetch::WebFetch;
///
/// let tool = WebFetch::new();
/// let shared: Arc<dyn promptforge_tools::Tool> = Arc::new(tool);
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
    /// use promptforge_webfetch::WebFetch;
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
    /// use promptforge_webfetch::{FetchConfig, WebFetch};
    ///
    /// let policy = FetchConfig::builder().max_chars(10_000).build()?;
    /// let tool = WebFetch::try_with_config(policy)?;
    /// # let _ = tool;
    /// # Ok::<(), promptforge_webfetch::ConfigError>(())
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
/// A refusal produced by the resolver or redirect policy is carried as a
/// [`FetchError`] in the error source chain; its [`Disposition`] decides the
/// outcome. A bare transport failure with no such source is soft.
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
    #[expect(
        clippy::expect_used,
        reason = "the id components are compile-time constants that satisfy ToolId's validation"
    )]
    fn id(&self) -> ToolId {
        ToolId::new("promptforge", "web_fetch")
            .expect("`promptforge`/`web_fetch` is a valid tool id")
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
mod tests {
    use std::io::Write;
    use std::net::{IpAddr, SocketAddr};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use axum::Router;
    use axum::body::Body;
    use axum::extract::State;
    use axum::http::HeaderMap;
    use axum::http::header::{CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE};
    use axum::response::{Html, IntoResponse, Redirect, Response};
    use axum::routing::get;
    use flate2::Compression;
    use flate2::write::GzEncoder;

    use super::WebFetch;
    use crate::config::{FetchConfig, FetchConfigBuilder};
    use crate::resolver::{Lookup, LookupFuture};
    use promptforge_tools::{Tool, ToolErrorKind, ToolId};

    /// An article page long enough for readability extraction to fire.
    const ARTICLE_HTML: &str = r"
        <html><body>
          <article>
            <h1>Loopback Test Article</h1>
            <p>This is the first substantial paragraph of a loopback test page,
               deliberately long enough to be treated as real article content.</p>
            <p>A second paragraph continues the prose so the extractor keeps the
               body and the character count stays comfortably above threshold.</p>
          </article>
        </body></html>
    ";

    /// An article whose prose is full of multibyte characters.
    const UNICODE_HTML: &str = r"
        <html><body>
          <article>
            <h1>Café Résumé Naïve</h1>
            <p>Café résumé naïve façade jalapeño piñata. Café résumé naïve façade
               jalapeño piñata. Café résumé naïve façade jalapeño piñata.</p>
            <p>Café résumé naïve façade jalapeño piñata. Café résumé naïve façade
               jalapeño piñata. Café résumé naïve façade jalapeño piñata.</p>
          </article>
        </body></html>
    ";

    /// An HTML table page that readability would discard.
    const TABLE_HTML: &str = r"
        <html><body>
          <p>Prices.</p>
          <table>
            <tr><th>Item</th><th>Cost</th></tr>
            <tr><td>WIDGETROW</td><td>4.20</td></tr>
            <tr><td>GADGETROW</td><td>6.90</td></tr>
          </table>
        </body></html>
    ";

    /// A JSON document served with an `application/json` type.
    const JSON_BODY: &str = r#"{"key":"value","numbers":[1,2,3],"nested":{"ok":true}}"#;

    /// A [`Lookup`] that maps host names to fixed addresses for injected tests.
    struct MapLookup {
        entries: Vec<(String, IpAddr)>,
    }

    impl Lookup for MapLookup {
        fn lookup(&self, host: String) -> LookupFuture {
            let addrs: Vec<SocketAddr> = self
                .entries
                .iter()
                .filter(|(h, _)| *h == host)
                .map(|(_, ip)| SocketAddr::new(*ip, 0))
                .collect();
            Box::pin(async move { Ok(addrs) })
        }
    }

    #[test]
    fn descriptor_is_stable_and_faithful() {
        let tool = WebFetch::new();

        assert_eq!(
            tool.id(),
            ToolId::new("promptforge", "web_fetch").expect("valid id")
        );
        assert_eq!(tool.wire_name(), "web_fetch");
        assert_eq!(
            tool.description(),
            "Fetch a web page and return its main content as markdown."
        );
        let schema = tool.parameters_schema();
        assert_eq!(schema["properties"]["max_chars"]["maximum"], 40_000);
        assert_eq!(schema["required"], serde_json::json!(["url"]));
        assert_eq!(schema["properties"]["url"]["type"], "string");
    }

    #[derive(Clone)]
    struct AppState {
        port: u16,
        hits: Arc<AtomicUsize>,
    }

    async fn root() -> Html<&'static str> {
        Html(ARTICLE_HTML)
    }

    async fn redir(State(state): State<AppState>) -> Redirect {
        Redirect::temporary(&format!("http://127.0.0.1:{}/target", state.port))
    }

    async fn target(State(state): State<AppState>) -> &'static str {
        state.hits.fetch_add(1, Ordering::SeqCst);
        "reached the internal target"
    }

    async fn unicode() -> Html<&'static str> {
        Html(UNICODE_HTML)
    }

    async fn large() -> Html<String> {
        let filler = "x".repeat(200_000);
        Html(format!("<html><body><p>{filler}</p></body></html>"))
    }

    async fn gzip_bomb() -> impl IntoResponse {
        let raw = "A".repeat(200_000);
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(raw.as_bytes())
            .expect("writing to an in-memory gzip encoder must succeed");
        let compressed = encoder
            .finish()
            .expect("finishing an in-memory gzip encoder must succeed");
        (
            [(CONTENT_ENCODING, "gzip"), (CONTENT_TYPE, "text/html")],
            compressed,
        )
    }

    /// Declares a `Content-Length` far over any cap while its body never ends,
    /// so `send` resolves on the headers and the precheck refuses on the
    /// declared length alone. The tail never completes, so no timing crutch is
    /// needed for the precheck to fire first.
    async fn liar_content_length() -> Response {
        use futures_util::StreamExt as _;

        let head = futures_util::stream::once(async {
            Ok::<_, std::io::Error>("<html><body><p>x</p></body></html>")
        });
        let tail = futures_util::stream::once(async {
            std::future::pending::<()>().await;
            Ok::<_, std::io::Error>("")
        });
        Response::builder()
            .header(CONTENT_LENGTH, "1000000")
            .header(CONTENT_TYPE, "text/html")
            .body(Body::from_stream(head.chain(tail)))
            .expect("building the oversized-content-length response must succeed")
    }

    async fn table() -> Response {
        Response::builder()
            .header(CONTENT_TYPE, "text/html; charset=utf-8")
            .body(Body::from(TABLE_HTML))
            .expect("building the table html response must succeed")
    }

    async fn json_route() -> Response {
        Response::builder()
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(JSON_BODY))
            .expect("building the json response must succeed")
    }

    async fn jsonbig_route() -> Response {
        let filler = "x".repeat(200_000);
        let body = format!(r#"{{"filler":"{filler}"}}"#);
        Response::builder()
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .expect("building the large json response must succeed")
    }

    async fn badcharset_route() -> Response {
        Response::builder()
            .header(CONTENT_TYPE, "text/plain; charset=not-a-charset")
            .body(Body::from("some plain body text"))
            .expect("building the bad-charset response must succeed")
    }

    async fn pdf_route() -> Response {
        Response::builder()
            .header(CONTENT_TYPE, "application/pdf")
            .body(Body::from(&b"%PDF-1.4 not a real pdf"[..]))
            .expect("building the pdf response must succeed")
    }

    async fn octet_route() -> Response {
        Response::builder()
            .header(CONTENT_TYPE, "application/octet-stream")
            .body(Body::from(vec![0u8, 1, 2, 3, 4, 5]))
            .expect("building the octet-stream response must succeed")
    }

    async fn notype_route() -> Response {
        Response::builder()
            .body(Body::from("a body with no declared content type"))
            .expect("building the no-content-type response must succeed")
    }

    async fn not_found_route() -> Response {
        Response::builder()
            .status(axum::http::StatusCode::NOT_FOUND)
            .header(CONTENT_TYPE, "text/html")
            .body(Body::from("<html><body>Not Found</body></html>"))
            .expect("building the 404 response must succeed")
    }

    async fn internal_error_route() -> Response {
        Response::builder()
            .status(axum::http::StatusCode::INTERNAL_SERVER_ERROR)
            .header(CONTENT_TYPE, "text/html")
            .body(Body::from("<html><body>Server Error</body></html>"))
            .expect("building the 500 response must succeed")
    }

    async fn latin1_route() -> Response {
        let body = vec![b'C', b'a', b'f', 0xE9];
        Response::builder()
            .header(CONTENT_TYPE, "text/plain; charset=ISO-8859-1")
            .body(Body::from(body))
            .expect("building the latin-1 response must succeed")
    }

    async fn plainbig_route() -> Response {
        Response::builder()
            .header(CONTENT_TYPE, "text/plain; charset=utf-8")
            .body(Body::from("y".repeat(200_000)))
            .expect("building the large text response must succeed")
    }

    /// Serves a `text/plain` body that fails mid-stream: one chunk, then an I/O
    /// error, so the client's body read fails deterministically without any
    /// timing crutch.
    async fn plain_broken_route() -> Response {
        use futures_util::StreamExt as _;

        let head = futures_util::stream::once(async { Ok::<_, std::io::Error>("partial body ") });
        let boom = futures_util::stream::once(async {
            Err::<&'static str, std::io::Error>(std::io::Error::other("mid-stream failure"))
        });
        Response::builder()
            .header(CONTENT_TYPE, "text/plain; charset=utf-8")
            .body(Body::from_stream(head.chain(boom)))
            .expect("building the broken plain response must succeed")
    }

    async fn spawn_server() -> (u16, Arc<AtomicUsize>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("binding a loopback listener must succeed");
        let port = listener
            .local_addr()
            .expect("the listener must have a local address")
            .port();
        let hits = Arc::new(AtomicUsize::new(0));
        let state = AppState {
            port,
            hits: Arc::clone(&hits),
        };
        let app = Router::new()
            .route("/", get(root))
            .route("/redir", get(redir))
            .route("/target", get(target))
            .route("/unicode", get(unicode))
            .route("/large", get(large))
            .route("/gzip", get(gzip_bomb))
            .route("/liar", get(liar_content_length))
            .route("/table", get(table))
            .route("/json", get(json_route))
            .route("/jsonbig", get(jsonbig_route))
            .route("/badcharset", get(badcharset_route))
            .route("/pdf", get(pdf_route))
            .route("/octet", get(octet_route))
            .route("/notype", get(notype_route))
            .route("/notfound", get(not_found_route))
            .route("/error500", get(internal_error_route))
            .route("/latin1", get(latin1_route))
            .route("/plainbig", get(plainbig_route))
            .route("/plainbroken", get(plain_broken_route))
            .with_state(state);
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("the loopback server must serve");
        });
        (port, hits)
    }

    /// A builder that can reach the loopback server: http allowed, its port on
    /// the allowlist, and `localhost` pinned to `127.0.0.1`.
    fn loopback_builder(port: u16) -> FetchConfigBuilder {
        let loopback: IpAddr = "127.0.0.1".parse().expect("loopback literal parses");
        FetchConfig::builder()
            .allow_http(true)
            .allow_ports([80, 443, port])
            .allow_host_address("localhost", loopback)
    }

    /// The built loopback policy.
    fn loopback_config(port: u16) -> FetchConfig {
        loopback_builder(port)
            .build()
            .expect("loopback config is valid")
    }

    /// Builds a `WebFetch` over the loopback policy.
    fn loopback_tool(port: u16) -> WebFetch {
        WebFetch::try_with_config(loopback_config(port)).expect("the loopback client builds")
    }

    #[derive(Clone)]
    struct RecordingState {
        port: u16,
        recorded: Arc<Mutex<Vec<HeaderMap>>>,
    }

    async fn record_headers(
        State(state): State<RecordingState>,
        headers: HeaderMap,
    ) -> Html<&'static str> {
        state
            .recorded
            .lock()
            .expect("the recorded-headers mutex must not be poisoned")
            .push(headers);
        Html(ARTICLE_HTML)
    }

    async fn redirect_to_record(State(state): State<RecordingState>) -> Redirect {
        Redirect::temporary(&format!("http://localhost:{}/record", state.port))
    }

    /// Never responds, so a short total timeout aborts the request.
    async fn hang() -> Html<&'static str> {
        std::future::pending::<()>().await;
        Html(ARTICLE_HTML)
    }

    async fn spawn_recording_server() -> (u16, Arc<Mutex<Vec<HeaderMap>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("binding a loopback listener must succeed");
        let port = listener
            .local_addr()
            .expect("the listener must have a local address")
            .port();
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let state = RecordingState {
            port,
            recorded: Arc::clone(&recorded),
        };
        let app = Router::new()
            .route("/record", get(record_headers))
            .route("/redir-record", get(redirect_to_record))
            .route("/slow", get(hang))
            .with_state(state);
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("the loopback recording server must serve");
        });
        (port, recorded)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn slow_server_past_total_timeout_yields_timeout() {
        let (port, _recorded) = spawn_recording_server().await;
        let config = loopback_builder(port)
            .timeout(std::time::Duration::from_millis(200))
            .build()
            .expect("valid config");
        let tool = WebFetch::try_with_config(config).expect("client builds");

        let url = format!("http://localhost:{port}/slow");
        let result = tool
            .call(serde_json::json!({ "url": url }))
            .await
            .expect("a timeout is a soft (recoverable) return")
            .text()
            .to_owned();

        assert!(
            result.contains("timed out"),
            "expected a timeout message, got: {result}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn request_carries_no_cookie_or_credential() {
        let (port, recorded) = spawn_recording_server().await;
        let tool = loopback_tool(port);

        let url = format!("http://localhost:{port}/record");
        tool.call(serde_json::json!({ "url": url }))
            .await
            .expect("a loopback fetch through allow_exact must succeed");

        let recorded = recorded
            .lock()
            .expect("the recorded-headers mutex must not be poisoned");
        assert_eq!(recorded.len(), 1);
        let headers = &recorded[0];
        assert!(!headers.contains_key(axum::http::header::COOKIE));
        assert!(!headers.contains_key(axum::http::header::AUTHORIZATION));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn no_credential_or_referer_survives_a_redirect() {
        let (port, recorded) = spawn_recording_server().await;
        let tool = loopback_tool(port);

        // A query-bearing source URL redirecting to a distinct allowed path: the
        // target must receive no Cookie, Authorization, or Referer.
        let url = format!("http://localhost:{port}/redir-record?secret=leak-me");
        tool.call(serde_json::json!({ "url": url }))
            .await
            .expect("a redirect between loopback paths must succeed");

        let recorded = recorded
            .lock()
            .expect("the recorded-headers mutex must not be poisoned");
        assert_eq!(recorded.len(), 1);
        let headers = &recorded[0];
        assert!(!headers.contains_key(axum::http::header::COOKIE));
        assert!(!headers.contains_key(axum::http::header::AUTHORIZATION));
        assert!(
            !headers.contains_key(axum::http::header::REFERER),
            "no Referer may survive a redirect, got: {headers:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn fetch_returns_provenance_line_then_content() {
        let (port, _hits) = spawn_server().await;
        let tool = loopback_tool(port);

        let url = format!("http://localhost:{port}/");
        let out = tool
            .call(serde_json::json!({ "url": url }))
            .await
            .expect("a loopback fetch through allow_exact must succeed")
            .text()
            .to_owned();

        let expected = format!("url: http://localhost:{port}/");
        assert!(out.starts_with(&expected), "got: {out}");
        assert!(out.contains("substantial paragraph"), "got: {out}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn redirect_to_internal_is_refused_and_target_untouched() {
        let (port, hits) = spawn_server().await;
        let tool = loopback_tool(port);

        let url = format!("http://localhost:{port}/redir");
        let result = tool
            .call(serde_json::json!({ "url": url }))
            .await
            .expect("a redirect-target policy refusal is a soft (recoverable) return")
            .text()
            .to_owned();

        assert!(
            result.contains("refused") && result.contains("127.0.0.1"),
            "got: {result}"
        );
        assert_eq!(hits.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn redirect_to_internal_via_injected_lookup_never_contacts_target() {
        let (port, hits) = spawn_server().await;
        let loopback: IpAddr = "127.0.0.1".parse().expect("loopback parses");
        let blocked: IpAddr = "10.0.0.1".parse().expect("private parses");
        // allowed.test reaches the loopback server; internal.test resolves only
        // to a blocked address and carries no exact exception.
        let lookup = MapLookup {
            entries: vec![
                ("allowed.test".to_string(), loopback),
                ("internal.test".to_string(), blocked),
            ],
        };
        // The server redirects to internal.test, which resolves only to a
        // blocked address, so the redirected target is never contacted.
        let redir_state = AppState {
            port,
            hits: Arc::clone(&hits),
        };
        let redir_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("binding a second loopback listener must succeed");
        let redir_port = redir_listener.local_addr().expect("local addr").port();
        let app = Router::new()
            .route(
                "/go",
                get(move || async move {
                    Redirect::temporary(&format!("http://internal.test:{port}/target"))
                }),
            )
            .with_state(redir_state);
        tokio::spawn(async move {
            axum::serve(redir_listener, app)
                .await
                .expect("the redirect server must serve");
        });
        // Reach the redirect server via allowed.test on its own port.
        let config = FetchConfig::builder()
            .allow_http(true)
            .allow_ports([redir_port, port])
            .allow_host_address("allowed.test", loopback)
            .build()
            .expect("valid config");
        let tool = WebFetch::with_lookup(config, lookup);

        let url = format!("http://allowed.test:{redir_port}/go");
        // The outcome may be a hard error (the redirect address is blocked) or a
        // soft return, but it must never carry the internal target's body.
        if let Ok(output) = tool.call(serde_json::json!({ "url": url })).await {
            assert!(
                !output.text().contains("reached the internal target"),
                "the internal target body must never be returned"
            );
        }

        assert_eq!(
            hits.load(Ordering::SeqCst),
            0,
            "the internal redirect target must never be contacted"
        );
    }

    #[tokio::test]
    async fn call_rejects_bad_urls_before_network() {
        let tool = WebFetch::new();

        let hard_cases = [
            (
                "https://user:pass@example.com/",
                "url must not contain userinfo",
            ),
            ("https://example.com:8080/", "port not allowed: 8080"),
            ("https://0177.0.0.1/", "ip literal host not allowed"),
            ("https://2130706433/", "ip literal host not allowed"),
            ("https://[::1]/", "ip literal host not allowed"),
            ("https://127.1/", "ip literal host not allowed"),
        ];

        for (raw, reason) in hard_cases {
            let err = tool
                .call(serde_json::json!({ "url": raw }))
                .await
                .expect_err(&format!("expected {raw} to be refused before any network"));
            assert!(
                err.kind() == ToolErrorKind::InvalidArguments,
                "expected a policy rejection for {raw}, got: {err:?}"
            );
            assert!(
                err.to_string().contains(reason),
                "expected policy reason {reason:?} for {raw}, got: {err}"
            );
        }

        let soft = tool
            .call(serde_json::json!({ "url": "http://example.com/" }))
            .await
            .expect("blocked http scheme must be soft tool text")
            .text()
            .to_owned();
        assert!(
            soft.contains("scheme not allowed: http"),
            "expected soft scheme refusal, got: {soft}"
        );
    }

    fn split_header(out: &str) -> (&str, &str) {
        out.split_once("\n\n")
            .expect("the return must carry a header and a blank-line separator")
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn oversized_html_is_refused() {
        let (port, _hits) = spawn_server().await;
        let config = loopback_builder(port)
            .max_bytes(4096)
            .build()
            .expect("valid");
        let tool = WebFetch::try_with_config(config).expect("client builds");

        let url = format!("http://localhost:{port}/large");
        let result = tool
            .call(serde_json::json!({ "url": url }))
            .await
            .expect("an oversized HTML body is a soft return")
            .text()
            .to_owned();

        assert!(
            result.contains("exceeds") && result.contains("4096"),
            "got: {result}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn declared_content_length_over_cap_is_refused_before_read() {
        let (port, _hits) = spawn_server().await;
        let config = loopback_builder(port)
            .max_bytes(4096)
            .build()
            .expect("valid");
        let tool = WebFetch::try_with_config(config).expect("client builds");

        let url = format!("http://localhost:{port}/liar");
        let result = tool
            .call(serde_json::json!({ "url": url }))
            .await
            .expect("a declared Content-Length over the cap is a soft return")
            .text()
            .to_owned();

        assert!(
            result.contains("exceeds") && result.contains("4096"),
            "got: {result}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn gzip_bomb_refused_on_decompressed_count() {
        let (port, _hits) = spawn_server().await;
        let config = loopback_builder(port)
            .max_bytes(4096)
            .build()
            .expect("valid");
        let tool = WebFetch::try_with_config(config).expect("client builds");

        let url = format!("http://localhost:{port}/gzip");
        let result = tool
            .call(serde_json::json!({ "url": url }))
            .await
            .expect("a gzip body that decompresses past the cap is a soft return")
            .text()
            .to_owned();

        assert!(result.contains("exceeds"), "got: {result}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn text_over_max_chars_is_truncated_on_char_boundary() {
        let (port, _hits) = spawn_server().await;
        let tool = loopback_tool(port);

        let max_chars = 25usize;
        let url = format!("http://localhost:{port}/unicode");
        let out = tool
            .call(serde_json::json!({ "url": url, "max_chars": max_chars }))
            .await
            .expect("a unicode fetch through allow_exact must succeed")
            .text()
            .to_owned();

        let (header, body) = split_header(&out);
        assert!(header.contains("truncated: true"), "got: {header}");
        assert_eq!(body.chars().count(), max_chars, "got: {body:?}");
        assert!(
            body.contains('é') || body.contains('ï') || body.contains('ç'),
            "got: {body:?}"
        );
        assert!(!body.contains('\u{FFFD}'), "got: {body:?}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn per_call_max_chars_is_clamped_to_the_configured_ceiling() {
        let (port, _hits) = spawn_server().await;
        // A tiny ceiling: a huge per-call request must be clamped to it.
        let config = loopback_builder(port).max_chars(10).build().expect("valid");
        let tool = WebFetch::try_with_config(config).expect("client builds");

        let url = format!("http://localhost:{port}/plainbig");
        let out = tool
            .call(serde_json::json!({ "url": url, "max_chars": 1_000_000 }))
            .await
            .expect("a plain fetch must succeed")
            .text()
            .to_owned();

        let (header, body) = split_header(&out);
        assert!(header.contains("truncated: true"), "got: {header}");
        assert_eq!(
            body.chars().count(),
            10,
            "the per-call max_chars must be clamped to the ceiling, got {} chars",
            body.chars().count()
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn body_one_byte_under_cap_succeeds_untruncated() {
        let (port, _hits) = spawn_server().await;
        let config = loopback_builder(port)
            .max_bytes(ARTICLE_HTML.len() + 1)
            .build()
            .expect("valid");
        let tool = WebFetch::try_with_config(config).expect("client builds");

        let url = format!("http://localhost:{port}/");
        let out = tool
            .call(serde_json::json!({ "url": url }))
            .await
            .expect("a body one byte under the cap must be accepted")
            .text()
            .to_owned();

        let (header, body) = split_header(&out);
        assert!(header.contains("truncated: false"), "got: {header}");
        assert!(body.contains("substantial paragraph"), "got: {body}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn html_is_extracted_and_reports_readability() {
        let (port, _hits) = spawn_server().await;
        let tool = loopback_tool(port);

        let url = format!("http://localhost:{port}/");
        let out = tool
            .call(serde_json::json!({ "url": url }))
            .await
            .expect("a loopback html fetch must succeed")
            .text()
            .to_owned();

        let (header, body) = split_header(&out);
        assert!(header.contains("extraction: readability"), "got: {header}");
        assert!(body.contains("substantial paragraph"), "got: {body}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn raw_forces_whole_page_render_keeping_table() {
        let (port, _hits) = spawn_server().await;
        let tool = loopback_tool(port);

        let url = format!("http://localhost:{port}/table");
        let out = tool
            .call(serde_json::json!({ "url": url, "raw": true }))
            .await
            .expect("a raw table fetch must succeed")
            .text()
            .to_owned();

        let (header, body) = split_header(&out);
        assert!(header.contains("extraction: raw-html"), "got: {header}");
        assert!(
            body.contains("WIDGETROW") && body.contains("GADGETROW"),
            "got: {body}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn json_is_returned_verbatim_as_plain() {
        let (port, _hits) = spawn_server().await;
        let tool = loopback_tool(port);

        let url = format!("http://localhost:{port}/json");
        let out = tool
            .call(serde_json::json!({ "url": url }))
            .await
            .expect("a json fetch must succeed")
            .text()
            .to_owned();

        let (header, body) = split_header(&out);
        assert!(header.contains("extraction: plain"), "got: {header}");
        assert_eq!(body, JSON_BODY, "got: {body}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn oversized_json_is_hard_refused_not_truncated() {
        let (port, _hits) = spawn_server().await;
        let config = loopback_builder(port)
            .max_bytes(4096)
            .build()
            .expect("valid");
        let tool = WebFetch::try_with_config(config).expect("client builds");

        let url = format!("http://localhost:{port}/jsonbig");
        let result = tool
            .call(serde_json::json!({ "url": url }))
            .await
            .expect("an oversized json body is a soft return")
            .text()
            .to_owned();

        assert!(
            result.contains("exceeds") && result.contains("4096"),
            "got: {result}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn flat_text_body_read_failure_is_soft() {
        let (port, _hits) = spawn_server().await;
        let tool = loopback_tool(port);

        let url = format!("http://localhost:{port}/plainbroken");
        // The mid-stream failure must be a soft (recoverable) return, never a
        // hard error: identical to the HTML and structured routes. A `text()`
        // return proves the outcome was soft untrusted output.
        let outcome = tool
            .call(serde_json::json!({ "url": url }))
            .await
            .expect("a mid-stream flat-text failure must be a soft return, not a hard error");
        assert_eq!(
            outcome.trust(),
            promptforge_tools::OutputTrust::Untrusted,
            "a soft body-read failure must be untrusted output"
        );
        let result = outcome.text().to_owned();
        assert!(
            result.contains("failed to read the response body") || result.contains("network error"),
            "got: {result}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unrecognized_charset_is_refused_naming_the_label() {
        let (port, _hits) = spawn_server().await;
        let tool = loopback_tool(port);

        let url = format!("http://localhost:{port}/badcharset");
        let result = tool
            .call(serde_json::json!({ "url": url }))
            .await
            .expect("an unrecognized charset is a soft return")
            .text()
            .to_owned();

        assert!(result.contains("not-a-charset"), "got: {result}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pdf_is_refused_naming_the_type() {
        let (port, _hits) = spawn_server().await;
        let tool = loopback_tool(port);

        let url = format!("http://localhost:{port}/pdf");
        let result = tool
            .call(serde_json::json!({ "url": url }))
            .await
            .expect("a pdf response is a soft return")
            .text()
            .to_owned();

        assert!(result.contains("application/pdf"), "got: {result}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn octet_stream_is_refused() {
        let (port, _hits) = spawn_server().await;
        let tool = loopback_tool(port);

        let url = format!("http://localhost:{port}/octet");
        let result = tool
            .call(serde_json::json!({ "url": url }))
            .await
            .expect("an octet-stream response is a soft return")
            .text()
            .to_owned();

        assert!(result.contains("application/octet-stream"), "got: {result}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn absent_content_type_is_refused() {
        let (port, _hits) = spawn_server().await;
        let tool = loopback_tool(port);

        let url = format!("http://localhost:{port}/notype");
        let result = tool
            .call(serde_json::json!({ "url": url }))
            .await
            .expect("an absent content type is a soft return")
            .text()
            .to_owned();

        assert!(result.contains("no content type"), "got: {result}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn latin1_page_decodes_with_declared_charset() {
        let (port, _hits) = spawn_server().await;
        let tool = loopback_tool(port);

        let url = format!("http://localhost:{port}/latin1");
        let out = tool
            .call(serde_json::json!({ "url": url }))
            .await
            .expect("a latin-1 fetch must succeed")
            .text()
            .to_owned();

        let (header, body) = split_header(&out);
        assert!(header.contains("extraction: plain"), "got: {header}");
        assert!(body.contains('é'), "got: {body:?}");
        assert!(!body.contains('\u{FFFD}'), "got: {body:?}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn plain_text_over_cap_is_truncated_not_refused() {
        let (port, _hits) = spawn_server().await;
        let config = loopback_builder(port)
            .max_bytes(4096)
            .build()
            .expect("valid");
        let tool = WebFetch::try_with_config(config).expect("client builds");

        let url = format!("http://localhost:{port}/plainbig");
        let out = tool
            .call(serde_json::json!({ "url": url }))
            .await
            .expect("an oversized flat-text body must be truncated, not refused")
            .text()
            .to_owned();

        let (header, body) = split_header(&out);
        assert!(header.contains("truncated: true"), "got: {header}");
        assert!(header.contains("extraction: plain"), "got: {header}");
        assert_eq!(body.len(), 4096, "got {} bytes", body.len());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn soft_return_on_404() {
        let (port, _hits) = spawn_server().await;
        let tool = loopback_tool(port);

        let url = format!("http://localhost:{port}/notfound");
        let result = tool
            .call(serde_json::json!({ "url": url }))
            .await
            .expect("a 404 must be a soft return")
            .text()
            .to_owned();

        assert!(result.contains("404"), "got: {result}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn soft_return_on_500() {
        let (port, _hits) = spawn_server().await;
        let tool = loopback_tool(port);

        let url = format!("http://localhost:{port}/error500");
        let result = tool
            .call(serde_json::json!({ "url": url }))
            .await
            .expect("a 500 must be a soft return")
            .text()
            .to_owned();

        assert!(result.contains("500"), "got: {result}");
    }

    #[tokio::test]
    async fn blocked_url_still_hard_fails() {
        let tool = WebFetch::new();

        let err = tool
            .call(serde_json::json!({ "url": "https://1.2.3.4/secret" }))
            .await
            .expect_err("a bare IP literal URL must still be a hard error");

        assert!(
            err.kind() == ToolErrorKind::InvalidArguments && err.to_string().contains("ip literal"),
            "got: {err:?}"
        );
    }
}
