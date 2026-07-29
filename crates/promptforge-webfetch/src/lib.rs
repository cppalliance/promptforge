//! The `web_fetch` tool: fetch a URL and return its main content as markdown.
//!
//! This tool runs locally in-process. It performs a plain HTTP GET, extracts
//! the page's main article content with [`readabilityrs`], and renders it to
//! markdown. Pages that are not article-shaped fall back to a whole-page
//! HTML-to-markdown conversion with [`htmd`], so the tool always returns
//! something useful when the fetch itself succeeds.

use std::sync::Arc;

use readabilityrs::{Readability, ReadabilityOptions};

use promptforge_core::tools::Tool;
use promptforge_core::{Error, Result};

pub mod address;
pub mod config;
pub mod error;
pub mod redirect;
pub mod resolver;
pub mod url_policy;

pub use address::{BLOCKED_CIDRS, addr_allowed, addr_allowed_for_host};
pub use config::FetchConfig;
pub use error::FetchError;
pub use redirect::{check_redirect, redirect_policy};
pub use resolver::{GuardedResolver, Lookup, SystemLookup};
pub use url_policy::check_url;

/// The request timeout applied to each fetch.
const FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// The largest error body kept for diagnostics, in characters.
const MAX_ERROR_BODY: usize = 2000;

/// The minimum extracted length below which the whole-page fallback fires.
const MIN_CONTENT_LEN: usize = 100;

/// A tool that fetches a web page and returns its main content as markdown.
///
/// The tool holds a reusable [`reqwest::Client`] so repeated calls share a
/// connection pool.
#[derive(Debug, Clone)]
pub struct WebFetch {
    /// The HTTP client used for outbound requests.
    http: reqwest::Client,
    /// The security policy applied to each fetch.
    config: FetchConfig,
}

impl WebFetch {
    /// Construct a `WebFetch` with a fresh HTTP client and the default policy.
    #[must_use]
    pub fn new() -> WebFetch {
        WebFetch::with_config(FetchConfig::default())
    }

    /// Construct a `WebFetch` with a fresh HTTP client and the given policy.
    ///
    /// The client installs a [`GuardedResolver`], so every connection (on the
    /// first hop and after each redirect) is made only to an address the policy
    /// allows, and a [`redirect_policy`] that re-checks each hop's URL and
    /// refuses an `https` to `http` downgrade.
    ///
    /// # Panics
    /// Panics if the underlying HTTP client cannot be built, which for this
    /// static, valid configuration means the TLS backend failed to initialize:
    /// a defect in the environment, not a condition a caller can act on.
    #[must_use]
    pub fn with_config(config: FetchConfig) -> WebFetch {
        let resolver = Arc::new(GuardedResolver::system(config.clone()));
        let http = reqwest::Client::builder()
            .dns_resolver(resolver)
            .redirect(redirect_policy(config.clone()))
            .build()
            // The builder is fed a static, valid configuration; a failure here
            // means the TLS backend could not initialize, which is a defect, not
            // a condition a caller can act on.
            .expect(
                "building the web_fetch HTTP client cannot fail with a valid static configuration",
            );
        WebFetch { http, config }
    }
}

/// Recovers a [`FetchError`] carried as a source of a reqwest error.
///
/// The guarded resolver and the redirect policy report refusals by boxing a
/// [`FetchError`] into the error reqwest ultimately returns. Walking the source
/// chain recovers it so the model sees the policy reason ([`Error::Parse`])
/// rather than an opaque transport error. Anything else stays an [`Error::Http`].
fn map_send_error(err: reqwest::Error) -> Error {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(&err);
    while let Some(current) = source {
        if let Some(fetch_err) = current.downcast_ref::<FetchError>() {
            return Error::Parse(fetch_err.model_facing());
        }
        source = current.source();
    }
    Error::Http(Box::new(err))
}

impl Default for WebFetch {
    fn default() -> WebFetch {
        WebFetch::new()
    }
}

/// Extract a page's main content as markdown, with a whole-page fallback.
///
/// First tries [`readabilityrs`] with markdown output enabled. If article
/// extraction yields nothing usable (no article, or fewer than
/// [`MIN_CONTENT_LEN`] characters), the whole page is converted with
/// [`htmd::convert`] instead. Extraction never fails: the worst case is an
/// empty string when even the fallback conversion produces no text.
fn extract_markdown(html: &str, base_url: Option<&str>) -> String {
    let options = ReadabilityOptions {
        output_markdown: true,
        ..ReadabilityOptions::default()
    };

    let article_markdown = Readability::new(html, base_url, Some(options))
        .ok()
        .and_then(Readability::parse)
        .and_then(|article| {
            // Prefer readability's own markdown; otherwise convert the clean
            // article HTML it isolated.
            article.markdown_content.or_else(|| {
                article
                    .content
                    .and_then(|content| htmd::convert(&content).ok())
            })
        })
        .unwrap_or_default();

    if article_markdown.trim().len() >= MIN_CONTENT_LEN {
        return article_markdown;
    }

    // Fallback: convert the whole document. If even this fails, return
    // whatever the article extraction produced (possibly empty).
    htmd::convert(html).unwrap_or(article_markdown)
}

#[async_trait::async_trait]
impl Tool for WebFetch {
    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Tool trait fixes this return type to &str, so the &'static str suggestion cannot be applied"
    )]
    fn name(&self) -> &str {
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
                    "description": "Maximum number of characters of text to return for this call, overriding the configured default. Longer text is truncated on a character boundary and the result is flagged as truncated."
                }
            },
            "required": ["url"]
        })
    }

    async fn call(&self, args: serde_json::Value) -> Result<String> {
        let url = args
            .get("url")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| Error::Parse("web_fetch: missing url argument".into()))?;

        // A per-call `max_chars` overrides the configured default for this
        // fetch; absent, the config default applies.
        let max_chars = parse_max_chars(&args, self.config.max_chars)?;

        // Enforce the URL-admission policy before any network access.
        let url = check_url(url, &self.config)?;

        let response = self
            .http
            .get(url.clone())
            .timeout(FETCH_TIMEOUT)
            .send()
            .await
            .map_err(map_send_error)?;

        // The final URL after any redirects; this is the provenance the model
        // cites, since the bytes came from here, not from where it aimed.
        let final_url = response.url().clone();

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

        // Read the body under the byte cap. A declared Content-Length over the
        // cap is refused before any body is read; otherwise the decompressed
        // stream is counted as it arrives and aborted the moment it exceeds the
        // cap. For the HTML/structured path this is an all-or-nothing refusal.
        let body = read_body_capped(response, final_url.as_str(), self.config.max_bytes).await?;
        let html = String::from_utf8_lossy(&body);
        let markdown = extract_markdown(&html, Some(final_url.as_str()));

        // Cap the returned text at `max_chars`, cutting on a character boundary
        // so a multibyte character is never split.
        let (text, truncated) = truncate_to_chars(&markdown, max_chars);

        // Provenance header: a `url:` line naming the final URL, then a
        // `truncated:` line, then a blank line, then the content. Later steps
        // add the extraction mode to this header.
        Ok(format!(
            "url: {final_url}\ntruncated: {truncated}\n\n{text}"
        ))
    }
}

/// Parses the optional `max_chars` argument, falling back to `default`.
///
/// # Errors
/// Returns [`Error::Parse`] if `max_chars` is present but is not a positive
/// integer that fits in `usize`.
fn parse_max_chars(args: &serde_json::Value, default: usize) -> Result<usize> {
    let Some(value) = args.get("max_chars") else {
        return Ok(default);
    };
    if value.is_null() {
        return Ok(default);
    }
    let n = value
        .as_u64()
        .filter(|n| *n >= 1)
        .ok_or_else(|| Error::Parse("web_fetch: max_chars must be a positive integer".into()))?;
    Ok(usize::try_from(n).unwrap_or(usize::MAX))
}

/// Reads a response body into memory under a decompressed-byte cap.
///
/// A declared `Content-Length` greater than `max_bytes` is refused before the
/// body is read. Otherwise the body is streamed and counted as it arrives
/// (reqwest decompresses in the stream, so the count is on decompressed bytes),
/// and the read is aborted the moment the running total exceeds `max_bytes`. A
/// body of exactly `max_bytes` is accepted.
///
/// # Errors
/// Returns [`FetchError::TooLarge`] (as [`Error::Parse`]) if the response
/// exceeds `max_bytes`, or [`Error::Http`] on a transport failure mid-stream.
async fn read_body_capped(
    mut response: reqwest::Response,
    url: &str,
    max_bytes: usize,
) -> Result<Vec<u8>> {
    let too_large = || -> Error {
        FetchError::TooLarge {
            url: url.to_string(),
            limit: max_bytes,
        }
        .into()
    };

    // Precheck: an honest Content-Length over the cap is refused before any
    // body is read. A compressed response reports no usable length here, so the
    // streamed counter below is what catches an expanding payload.
    if let Some(len) = response.content_length() {
        if len > max_bytes as u64 {
            return Err(too_large());
        }
    }

    let mut body: Vec<u8> = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|source| Error::Http(Box::new(source)))?
    {
        if body.len() + chunk.len() > max_bytes {
            return Err(too_large());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Truncates `text` to at most `max_chars` characters on a character boundary.
///
/// Returns the (possibly shortened) text and whether it was truncated. The cut
/// falls on a [`char`] boundary, so a multibyte character is never split. Text
/// of exactly `max_chars` characters is returned untruncated.
fn truncate_to_chars(text: &str, max_chars: usize) -> (&str, bool) {
    match text.char_indices().nth(max_chars) {
        Some((idx, _)) => (&text[..idx], true),
        None => (text, false),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::net::IpAddr;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::Router;
    use axum::body::Body;
    use axum::extract::State;
    use axum::http::header::{CONTENT_ENCODING, CONTENT_LENGTH};
    use axum::response::{Html, IntoResponse, Redirect, Response};
    use axum::routing::get;
    use flate2::Compression;
    use flate2::write::GzEncoder;

    use super::{WebFetch, extract_markdown};
    use crate::config::FetchConfig;
    use promptforge_core::Error;
    use promptforge_core::tools::Tool;

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

    /// An article whose prose is full of multibyte characters, so a truncation
    /// that split a character would produce invalid UTF-8.
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

    /// Shared state for the loopback server: its own port and a target hit count.
    #[derive(Clone)]
    struct AppState {
        /// The port the server bound to, embedded in the redirect target.
        port: u16,
        /// How many times `/target` was requested.
        hits: Arc<AtomicUsize>,
    }

    async fn root() -> Html<&'static str> {
        Html(ARTICLE_HTML)
    }

    async fn redir(State(state): State<AppState>) -> Redirect {
        // Redirect to an IP-literal internal target; the redirect policy must
        // refuse it before any connection is attempted.
        Redirect::temporary(&format!("http://127.0.0.1:{}/target", state.port))
    }

    async fn target(State(state): State<AppState>) -> &'static str {
        state.hits.fetch_add(1, Ordering::SeqCst);
        "reached the internal target"
    }

    async fn unicode() -> Html<&'static str> {
        Html(UNICODE_HTML)
    }

    /// A plain HTML page far larger than the small `max_bytes` used in tests.
    ///
    /// It carries an honest `Content-Length`, so the size cap can refuse it on
    /// the pre-read check.
    async fn large() -> Html<String> {
        let filler = "x".repeat(200_000);
        Html(format!("<html><body><p>{filler}</p></body></html>"))
    }

    /// A response declaring `Content-Encoding: gzip` whose compressed body is
    /// tiny on the wire but decompresses far past the small `max_bytes`.
    ///
    /// Highly compressible payload: 200_000 identical bytes shrink to a few
    /// hundred on the wire, so a wire-byte counter would accept it and only a
    /// decompressed-byte counter refuses it.
    async fn gzip_bomb() -> impl IntoResponse {
        let raw = "A".repeat(200_000);
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(raw.as_bytes())
            .expect("writing to an in-memory gzip encoder must succeed");
        let compressed = encoder
            .finish()
            .expect("finishing an in-memory gzip encoder must succeed");
        ([(CONTENT_ENCODING, "gzip")], compressed)
    }

    /// A response that lies about its size: it declares a `Content-Length`
    /// far over any test cap while its actual body is a handful of bytes.
    ///
    /// The declared length is what the size precheck reads, before the body is
    /// streamed. The tiny real body is well under the cap, so the streamed byte
    /// counter would accept it; only the precheck refuses this response. That is
    /// what makes the test fail if the precheck were removed.
    ///
    /// The body is built from a stream so it carries no known length of its own,
    /// which lets the manually set `Content-Length` header stand (a body with a
    /// known size makes hyper reject the mismatched header instead of sending
    /// it). The stream emits one small chunk, then holds briefly before ending
    /// short of the declared length. The hold lets the client's `send` resolve
    /// on the headers first, so the precheck refuses on the declared length
    /// alone; were the precheck gone, the short body would surface as a
    /// transport error rather than the expected `TooLarge`, so the assertion
    /// still fails.
    async fn liar_content_length() -> Response {
        use futures_util::StreamExt as _;

        let head = futures_util::stream::once(async {
            Ok::<_, std::io::Error>("<html><body><p>x</p></body></html>")
        });
        let tail = futures_util::stream::once(async {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            Ok::<_, std::io::Error>("")
        });
        Response::builder()
            .header(CONTENT_LENGTH, "1000000")
            .body(Body::from_stream(head.chain(tail)))
            .expect("building the oversized-content-length response must succeed")
    }

    /// Binds a loopback axum server on an ephemeral port and serves it.
    ///
    /// Returns the port and the `/target` hit counter.
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
            .with_state(state);
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("the loopback server must serve");
        });
        (port, hits)
    }

    /// A config that can reach the loopback server: http allowed, its port on
    /// the allowlist, and `localhost` pinned to `127.0.0.1` via `allow_exact`.
    fn loopback_config(port: u16) -> FetchConfig {
        let loopback: IpAddr = "127.0.0.1".parse().expect("loopback literal parses");
        FetchConfig {
            allow_http: true,
            allow_ports: vec![80, 443, port],
            allow_exact: vec![("localhost".to_string(), loopback)],
            ..FetchConfig::default()
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn fetch_returns_provenance_line_then_content() {
        let (port, _hits) = spawn_server().await;
        let tool = WebFetch::with_config(loopback_config(port));

        let url = format!("http://localhost:{port}/");
        let out = tool
            .call(serde_json::json!({ "url": url }))
            .await
            .expect("a loopback fetch through allow_exact must succeed");

        let expected = format!("url: http://localhost:{port}/");
        assert!(
            out.starts_with(&expected),
            "output must begin with the final-url provenance line, got: {out}"
        );
        assert!(
            out.contains("substantial paragraph"),
            "the extracted article content must follow the header, got: {out}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn redirect_to_internal_is_refused_and_target_untouched() {
        let (port, hits) = spawn_server().await;
        let tool = WebFetch::with_config(loopback_config(port));

        let url = format!("http://localhost:{port}/redir");
        let err = tool
            .call(serde_json::json!({ "url": url }))
            .await
            .expect_err("a redirect to an internal ip literal must be refused");

        assert!(
            matches!(&err, Error::Parse(msg) if msg.contains("refused")),
            "expected a redirect-refused policy error, got: {err:?}"
        );
        assert_eq!(
            hits.load(Ordering::SeqCst),
            0,
            "the internal redirect target must never be requested"
        );
    }

    /// Bad URLs must be refused by `WebFetch::call` before any network request.
    #[tokio::test]
    async fn call_rejects_bad_urls_before_network() {
        let tool = WebFetch::new();

        // Each case pairs a bad URL with the policy reason that must appear in
        // the rejection. Asserting the specific reason (and that the error is
        // Error::Parse, the policy-rejection channel, not Error::Http) proves
        // the URL was refused by policy before any network access: a network
        // timeout would surface as Error::Http and carry none of these strings.
        let cases = [
            (
                "https://user:pass@example.com/",
                "url must not contain userinfo",
            ),
            ("https://example.com:8080/", "port not allowed: 8080"),
            ("http://example.com/", "scheme not allowed: http"),
            ("https://0177.0.0.1/", "ip literal host not allowed"),
            ("https://2130706433/", "ip literal host not allowed"),
            ("https://[::1]/", "ip literal host not allowed"),
            ("https://127.1/", "ip literal host not allowed"),
        ];

        for (raw, reason) in cases {
            let err = tool
                .call(serde_json::json!({ "url": raw }))
                .await
                .expect_err(&format!("expected {raw} to be refused before any network"));
            assert!(
                matches!(err, Error::Parse(_)),
                "expected a policy rejection (Error::Parse) for {raw}, got: {err:?}"
            );
            assert!(
                err.to_string().contains(reason),
                "expected policy reason {reason:?} for {raw}, got: {err}"
            );
        }
    }

    /// Splits a `web_fetch` return into its provenance header and its body.
    ///
    /// The header is the lines before the first blank line; the body is the
    /// rest. Panics if the blank-line separator is missing.
    fn split_header(out: &str) -> (&str, &str) {
        out.split_once("\n\n")
            .expect("the return must carry a header and a blank-line separator")
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn oversized_html_is_refused() {
        let (port, _hits) = spawn_server().await;
        let config = FetchConfig {
            max_bytes: 4096,
            ..loopback_config(port)
        };
        let tool = WebFetch::with_config(config);

        let url = format!("http://localhost:{port}/large");
        let err = tool
            .call(serde_json::json!({ "url": url }))
            .await
            .expect_err("an oversized HTML body must be refused");

        assert!(
            matches!(&err, Error::Parse(msg) if msg.contains("exceeds") && msg.contains("4096")),
            "expected a TooLarge refusal naming the cap, got: {err:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn declared_content_length_over_cap_is_refused_before_read() {
        let (port, _hits) = spawn_server().await;
        // The cap sits far below the declared 1_000_000-byte Content-Length but
        // far above the tiny real body. A streamed byte counter would accept the
        // real body, so the only thing that can produce a refusal here is the
        // pre-read Content-Length check: deleting that precheck turns this fetch
        // into a success and breaks this test.
        let config = FetchConfig {
            max_bytes: 4096,
            ..loopback_config(port)
        };
        let tool = WebFetch::with_config(config);

        let url = format!("http://localhost:{port}/liar");
        let err = tool
            .call(serde_json::json!({ "url": url }))
            .await
            .expect_err("a declared Content-Length over the cap must be refused before the read");

        assert!(
            matches!(&err, Error::Parse(msg) if msg.contains("exceeds") && msg.contains("4096")),
            "expected a TooLarge refusal naming the cap from the precheck, got: {err:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn gzip_bomb_refused_on_decompressed_count() {
        let (port, _hits) = spawn_server().await;
        // The cap is far larger than the compressed wire size (a few hundred
        // bytes) but far smaller than the 200_000-byte decompressed body, so a
        // refusal can only come from counting decompressed bytes.
        let config = FetchConfig {
            max_bytes: 4096,
            ..loopback_config(port)
        };
        let tool = WebFetch::with_config(config);

        let url = format!("http://localhost:{port}/gzip");
        let err = tool
            .call(serde_json::json!({ "url": url }))
            .await
            .expect_err("a gzip body that decompresses past the cap must be refused");

        assert!(
            matches!(&err, Error::Parse(msg) if msg.contains("exceeds")),
            "expected a TooLarge refusal on the decompressed count, got: {err:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn text_over_max_chars_is_truncated_on_char_boundary() {
        let (port, _hits) = spawn_server().await;
        let tool = WebFetch::with_config(loopback_config(port));

        // A small per-call cap forces a cut; the body is full of two-byte
        // characters, so a byte-wise cut would land mid-character.
        let max_chars = 25usize;
        let url = format!("http://localhost:{port}/unicode");
        let out = tool
            .call(serde_json::json!({ "url": url, "max_chars": max_chars }))
            .await
            .expect("a unicode fetch through allow_exact must succeed");

        let (header, body) = split_header(&out);
        assert!(
            header.contains("truncated: true"),
            "the header must flag truncation, got: {header}"
        );
        assert_eq!(
            body.chars().count(),
            max_chars,
            "the body must be cut to exactly max_chars characters, got: {body:?}"
        );
        assert!(
            body.contains('é') || body.contains('ï') || body.contains('ç'),
            "the truncated body must retain multibyte characters, got: {body:?}"
        );
        assert!(
            !body.contains('\u{FFFD}'),
            "the cut must fall on a char boundary, never splitting a character, got: {body:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn body_one_byte_under_cap_succeeds_untruncated() {
        let (port, _hits) = spawn_server().await;
        // The article body is served verbatim, so a cap one byte above its
        // length admits it with room to spare for no byte.
        let config = FetchConfig {
            max_bytes: ARTICLE_HTML.len() + 1,
            ..loopback_config(port)
        };
        let tool = WebFetch::with_config(config);

        let url = format!("http://localhost:{port}/");
        let out = tool
            .call(serde_json::json!({ "url": url }))
            .await
            .expect("a body one byte under the cap must be accepted");

        let (header, body) = split_header(&out);
        assert!(
            header.contains("truncated: false"),
            "a short body must not be flagged truncated, got: {header}"
        );
        assert!(
            body.contains("substantial paragraph"),
            "the extracted article content must survive intact, got: {body}"
        );
    }

    #[test]
    fn extracts_article_body_and_drops_boilerplate() {
        let html = r#"
            <html>
              <body>
                <nav><a href="/home">Home</a><a href="/about">About Us Navigation</a></nav>
                <article>
                  <h1>The Title Of The Piece</h1>
                  <p>This is the first substantial paragraph of the article body,
                     long enough to be treated as real content by the extractor.</p>
                  <p>Here is a second paragraph that continues the discussion with
                     even more prose so the reader has plenty of material to read.</p>
                  <p>A third and final paragraph rounds out the article nicely and
                     keeps the character count comfortably above the threshold.</p>
                </article>
                <footer>Copyright boilerplate footer text here.</footer>
              </body>
            </html>
        "#;

        let markdown = extract_markdown(html, Some("https://example.com/article"));

        assert!(
            markdown.contains("first substantial paragraph"),
            "expected article body in output, got: {markdown}"
        );
        assert!(
            !markdown.contains("About Us Navigation"),
            "navigation boilerplate should be stripped, got: {markdown}"
        );
    }

    #[test]
    fn falls_back_for_non_article_html() {
        let html = "<div>short</div>";

        let markdown = extract_markdown(html, None);

        assert!(
            !markdown.trim().is_empty(),
            "fallback conversion should return non-empty markdown, got: {markdown:?}"
        );
        assert!(
            markdown.contains("short"),
            "fallback should preserve the page text, got: {markdown}"
        );
    }
}
