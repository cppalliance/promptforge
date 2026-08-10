//! The `web_fetch` tool: fetch a URL and return its content as text, markdown
//! for an HTML page.
//!
//! This tool runs locally in-process. It performs a GET, routes the response on
//! its `Content-Type`, and refuses a type it cannot render. An HTML page has its
//! main article content extracted with [`readabilityrs`] and rendered to
//! markdown; a page that is not article-shaped falls back to a whole-page
//! HTML-to-markdown conversion with [`htmd`], so an HTML fetch that succeeds
//! always returns something useful. A non-HTML text body (JSON, XML, plain
//! text) is returned decoded, with no extraction.

use std::sync::Arc;

use readabilityrs::{Readability, ReadabilityOptions};
use reqwest::header::CONTENT_TYPE;

use promptforge_core::tools::{Tool, ToolError, ToolErrorKind, ToolId, ToolOutput};

/// The result the [`Tool::call`] boundary returns: untrusted page text on
/// success, a narrow [`ToolError`] on a hard failure.
type CallResult = std::result::Result<ToolOutput, ToolError>;

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
    /// refuses an `https` to `http` downgrade. It also applies the configured
    /// connect and total timeouts and a bounded pool idle timeout, sets the
    /// configured `User-Agent`, and carries no cookie store and no credential or
    /// `Authorization` header, so no request sends an ambient identity on any
    /// hop, including after a redirect.
    ///
    /// # Panics
    /// Panics if the underlying HTTP client cannot be built, which for this
    /// static, valid configuration means the TLS backend failed to initialize:
    /// a defect in the environment, not a condition a caller can act on.
    #[must_use]
    pub fn with_config(config: FetchConfig) -> WebFetch {
        let resolver = Arc::new(GuardedResolver::system(config.clone()));
        #[expect(
            clippy::expect_used,
            reason = "the builder is fed a static, valid configuration; a failure means the TLS backend could not initialize, a defect, not a caller-actionable condition"
        )]
        let http = reqwest::Client::builder()
            .dns_resolver(resolver)
            .redirect(redirect_policy(config.clone()))
            // Bound each hop's connect time, the whole request's time, and how
            // long an idle pooled socket lives (which bounds the DNS-rebinding
            // window). No cookie store is enabled and no default headers are
            // set, so the client sends no Cookie or Authorization on any hop.
            .connect_timeout(config.connect_timeout)
            .timeout(config.timeout)
            .pool_idle_timeout(config.pool_idle_timeout)
            .user_agent(config.user_agent.clone())
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

fn soft_or_hard(err: FetchError) -> CallResult {
    if err.is_recoverable() {
        Ok(ToolOutput::untrusted(err.model_facing()))
    } else {
        Err(err.into())
    }
}

/// Maps a body-read [`FetchError`] into a soft tool result.
///
/// A body-read failure is always a size cap ([`FetchError::TooLarge`]) or a
/// mid-stream transport failure ([`FetchError::BodyRead`]); both are recoverable
/// and returned as model-facing tool text so the model can try a different URL.
fn body_read_outcome(err: FetchError) -> CallResult {
    Ok(ToolOutput::untrusted(err.model_facing()))
}

/// Maps a reqwest send error into either a soft tool result (recoverable) or
/// a hard `Err` (policy/admission).
fn map_send_error_to_outcome(err: &reqwest::Error, url: &str) -> CallResult {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(current) = source {
        if let Some(fetch_err) = current.downcast_ref::<FetchError>() {
            if fetch_err.is_recoverable() {
                return Ok(ToolOutput::untrusted(fetch_err.model_facing()));
            }
            return Err(ToolError::message(fetch_err.model_facing())
                .with_kind(ToolErrorKind::InvalidArguments));
        }
        source = current.source();
    }
    if err.is_timeout() {
        return Ok(ToolOutput::untrusted(
            FetchError::Timeout {
                url: url.to_string(),
            }
            .model_facing(),
        ));
    }
    Ok(ToolOutput::untrusted(format!(
        "fetch failed for {url}: network error; try a different URL"
    )))
}

impl Default for WebFetch {
    fn default() -> WebFetch {
        WebFetch::new()
    }
}

/// How a response body was turned into the returned text.
///
/// The mode is reported on the provenance header's `extraction:` line so the
/// model knows whether it is holding an extracted article, a whole-page
/// rendering, or decoded plain text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Extraction {
    /// An HTML page's main article, isolated by [`readabilityrs`].
    Readability,
    /// A whole HTML document rendered to markdown, with no article extraction.
    RawHtml,
    /// A non-HTML text body decoded and returned verbatim, with no extraction.
    Plain,
}

impl Extraction {
    /// The label written on the provenance header's `extraction:` line.
    fn label(self) -> &'static str {
        match self {
            Extraction::Readability => "readability",
            Extraction::RawHtml => "raw-html",
            Extraction::Plain => "plain",
        }
    }
}

/// How a response's `Content-Type` routes through the fetch pipeline.
///
/// The route is decided from the response header before the body is read, so a
/// binary or absent type is refused without downloading it, and the body's
/// size discipline (all-or-nothing versus truncating) is chosen before any
/// bytes arrive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    /// `text/html` and `application/xhtml+xml`: readability plus htmd.
    Html,
    /// A non-HTML text body returned as decoded plain text, with no extraction.
    ///
    /// `structured` distinguishes JSON and XML (where a truncated prefix is
    /// invalid, so the body is read all-or-nothing on the byte cap) from
    /// genuinely flat text (where a prefix is a legitimate result, so an
    /// oversized body is truncated and flagged rather than refused).
    Plain {
        /// Whether the body is a structured format (JSON or XML) that must be
        /// read all-or-nothing rather than truncated to a prefix.
        structured: bool,
    },
}

/// Classifies a parsed content type into its fetch [`Route`].
///
/// `text/html` and `application/xhtml+xml` route to [`Route::Html`].
/// `application/json`, `application/xml`, `text/xml`, and any `+json`/`+xml`
/// suffix route to a structured [`Route::Plain`] (all-or-nothing on the byte
/// cap). Every other `text/*` routes to a flat [`Route::Plain`] (truncated on
/// oversize). Everything else (PDF, octet-stream, images, audio, video,
/// archives) returns [`None`], meaning refuse.
fn classify(mime: &mime::Mime) -> Option<Route> {
    let type_ = mime.type_();
    let subtype = mime.subtype();
    let suffix = mime.suffix();

    let is_html = (type_ == mime::TEXT && subtype == mime::HTML)
        || (type_ == mime::APPLICATION && subtype == "xhtml" && suffix == Some(mime::XML));
    if is_html {
        return Some(Route::Html);
    }

    // JSON and XML, by exact subtype or by a `+json`/`+xml` suffix. A prefix of
    // either is invalid, so these are structured and read all-or-nothing.
    let is_json_or_xml = subtype == mime::JSON
        || subtype == mime::XML
        || suffix == Some(mime::JSON)
        || suffix == Some(mime::XML);

    // `application/*` is accepted only when it is JSON or XML; anything else
    // (PDF, octet-stream, ...) is refused. Those accepted are structured.
    if type_ == mime::APPLICATION && is_json_or_xml {
        return Some(Route::Plain { structured: true });
    }

    // Every `text/*` is accepted. `text/xml` (and any textual `+json`/`+xml`)
    // is structured; all other flat text may be truncated to a prefix.
    if type_ == mime::TEXT {
        return Some(Route::Plain {
            structured: is_json_or_xml,
        });
    }

    None
}

/// Decodes response bytes to text using the declared charset.
///
/// An absent charset, or UTF-8, decodes as UTF-8 with lossy replacement of
/// invalid sequences. A declared non-UTF-8 charset is decoded through
/// [`encoding_rs`]. The header charset is authoritative: no embedded `meta`
/// charset is consulted.
///
/// # Errors
/// Returns [`FetchError::Undecodable`] if `charset` names a label
/// [`encoding_rs`] does not recognize.
fn decode_body(
    bytes: &[u8],
    charset: Option<&str>,
    url: &str,
) -> std::result::Result<String, FetchError> {
    match charset {
        None => Ok(String::from_utf8_lossy(bytes).into_owned()),
        Some(label)
            if label.eq_ignore_ascii_case("utf-8") || label.eq_ignore_ascii_case("utf8") =>
        {
            Ok(String::from_utf8_lossy(bytes).into_owned())
        }
        Some(label) => {
            let encoding = encoding_rs::Encoding::for_label(label.as_bytes()).ok_or_else(|| {
                FetchError::Undecodable {
                    url: url.to_string(),
                    charset: label.to_string(),
                }
            })?;
            Ok(encoding.decode(bytes).0.into_owned())
        }
    }
}

/// Renders an HTML page to markdown, reporting how it was produced.
///
/// When `raw` is true the whole document is converted with [`htmd::convert`] and
/// the mode is [`Extraction::RawHtml`], skipping article extraction entirely.
/// Otherwise [`readabilityrs`] isolates the main article; if that yields fewer
/// than [`MIN_CONTENT_LEN`] characters, the whole document is converted instead
/// and the mode is [`Extraction::RawHtml`]. Rendering never fails: the worst
/// case is an empty string when even the whole-page conversion produces no text.
fn extract_html(html: &str, base_url: Option<&str>, raw: bool) -> (String, Extraction) {
    if raw {
        // Forced whole-page rendering: skip readability so a page that is mostly
        // a table or list keeps its content.
        return (htmd::convert(html).unwrap_or_default(), Extraction::RawHtml);
    }

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
        return (article_markdown, Extraction::Readability);
    }

    // Fallback: convert the whole document. If even this fails, return
    // whatever the article extraction produced (possibly empty).
    (
        htmd::convert(html).unwrap_or(article_markdown),
        Extraction::RawHtml,
    )
}

#[async_trait::async_trait]
impl Tool for WebFetch {
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

        // A per-call `max_chars` overrides the configured default for this
        // fetch; absent, the config default applies.
        let max_chars = parse_max_chars(&args, self.config.max_chars)?;

        // `raw` forces whole-page rendering of an HTML response.
        let raw = parse_raw(&args)?;

        // Enforce the URL-admission policy before any network access. Scheme
        // refusals (plain http when disallowed) are soft so a fanout arm can
        // retry https; SSRF/userinfo/port/literal refusals stay hard.
        let url = match check_url(url, &self.config) {
            Ok(u) => u,
            Err(err) => return soft_or_hard(err),
        };

        let response = match self.http.get(url.clone()).send().await {
            Ok(resp) => resp,
            Err(err) => return map_send_error_to_outcome(&err, url.as_str()),
        };

        // The final URL after any redirects; this is the provenance the model
        // cites, since the bytes came from here, not from where it aimed.
        let final_url = response.url().clone();

        let status = response.status();
        if !status.is_success() {
            let err = FetchError::HttpStatus {
                url: final_url.to_string(),
                status: status.as_u16(),
            };
            return Ok(ToolOutput::untrusted(err.model_facing()));
        }

        // Route on the response Content-Type, read before the body: a binary or
        // absent type is refused without downloading it, and a flat-text body is
        // read under a truncating cap rather than an all-or-nothing one. The
        // charset parameter (if any) drives decoding.
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);

        let Some(content_type) = content_type else {
            return soft_or_hard(FetchError::NoContentType {
                url: final_url.to_string(),
            });
        };

        let parsed_mime: mime::Mime = match content_type.parse() {
            Ok(m) => m,
            Err(_) => {
                return soft_or_hard(FetchError::UnsupportedContentType {
                    url: final_url.to_string(),
                    content_type: content_type.clone(),
                });
            }
        };

        let Some(route) = classify(&parsed_mime) else {
            return soft_or_hard(FetchError::UnsupportedContentType {
                url: final_url.to_string(),
                content_type: content_type.clone(),
            });
        };

        let charset = parsed_mime
            .get_param(mime::CHARSET)
            .map(|name| name.as_str().to_owned());

        let (decoded, extraction, size_truncated) = match route {
            Route::Html => {
                let body =
                    match read_body_capped(response, final_url.as_str(), self.config.max_bytes)
                        .await
                    {
                        Ok(b) => b,
                        Err(e) => return body_read_outcome(e),
                    };
                let decoded = match decode_body(&body, charset.as_deref(), final_url.as_str()) {
                    Ok(d) => d,
                    Err(e) => return soft_or_hard(e),
                };
                let (markdown, extraction) = extract_html(&decoded, Some(final_url.as_str()), raw);
                (markdown, extraction, false)
            }
            Route::Plain { structured: true } => {
                let body =
                    match read_body_capped(response, final_url.as_str(), self.config.max_bytes)
                        .await
                    {
                        Ok(b) => b,
                        Err(e) => return body_read_outcome(e),
                    };
                let decoded = match decode_body(&body, charset.as_deref(), final_url.as_str()) {
                    Ok(d) => d,
                    Err(e) => return soft_or_hard(e),
                };
                (decoded, Extraction::Plain, false)
            }
            Route::Plain { structured: false } => {
                let (body, size_truncated) = read_body_truncating(response, self.config.max_bytes)
                    .await
                    .map_err(|e| {
                        ToolError::message(e.to_string()).with_kind(ToolErrorKind::Transport)
                    })?;
                let decoded = match decode_body(&body, charset.as_deref(), final_url.as_str()) {
                    Ok(d) => d,
                    Err(e) => return soft_or_hard(e),
                };
                (decoded, Extraction::Plain, size_truncated)
            }
        };

        // Cap the returned text at `max_chars`, cutting on a character boundary
        // so a multibyte character is never split. On the plain path this
        // stacks with any byte-level truncation already applied.
        let (text, char_truncated) = truncate_to_chars(&decoded, max_chars);
        let truncated = size_truncated || char_truncated;

        // Provenance header: a `url:` line naming the final URL, a `truncated:`
        // line, and an `extraction:` line naming how the text was produced, then
        // a blank line, then the content.
        Ok(ToolOutput::untrusted(format!(
            "url: {final_url}\ntruncated: {truncated}\nextraction: {}\n\n{text}",
            extraction.label()
        )))
    }
}

/// Parses the optional `max_chars` argument, falling back to `default`.
///
/// # Errors
/// Returns an invalid-arguments [`ToolError`] if `max_chars` is present but is
/// not a positive integer that fits in `usize`.
fn parse_max_chars(
    args: &serde_json::Value,
    default: usize,
) -> std::result::Result<usize, ToolError> {
    let Some(value) = args.get("max_chars") else {
        return Ok(default);
    };
    if value.is_null() {
        return Ok(default);
    }
    let n = value.as_u64().filter(|n| *n >= 1).ok_or_else(|| {
        ToolError::message("web_fetch: max_chars must be a positive integer")
            .with_kind(ToolErrorKind::InvalidArguments)
    })?;
    Ok(usize::try_from(n).unwrap_or(usize::MAX))
}

/// Parses the optional `raw` argument, defaulting to `false`.
///
/// # Errors
/// Returns an invalid-arguments [`ToolError`] if `raw` is present and is neither
/// null nor a boolean.
fn parse_raw(args: &serde_json::Value) -> std::result::Result<bool, ToolError> {
    match args.get("raw") {
        None => Ok(false),
        Some(value) if value.is_null() => Ok(false),
        Some(value) => value.as_bool().ok_or_else(|| {
            ToolError::message("web_fetch: raw must be a boolean")
                .with_kind(ToolErrorKind::InvalidArguments)
        }),
    }
}

/// Reads a response body into memory, truncating at a decompressed-byte cap.
///
/// Unlike [`read_body_capped`], a body over `max_bytes` is not refused: the read
/// stops at `max_bytes` and the returned flag is `true`, because a flat-text
/// prefix is still useful. The count runs over the decompressed stream, so a
/// compressed payload is measured on its expanded size.
///
/// # Errors
/// Returns [`FetchError::BodyRead`] on a transport failure mid-stream.
async fn read_body_truncating(
    response: reqwest::Response,
    max_bytes: usize,
) -> std::result::Result<(Vec<u8>, bool), FetchError> {
    let url = response.url().to_string();
    let mut response = response;
    let mut body: Vec<u8> = Vec::new();
    let mut truncated = false;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_source| FetchError::BodyRead { url: url.clone() })?
    {
        let remaining = max_bytes - body.len();
        if chunk.len() > remaining {
            // This chunk crosses the cap: keep its prefix and stop.
            body.extend_from_slice(&chunk[..remaining]);
            truncated = true;
            break;
        }
        body.extend_from_slice(&chunk);
    }
    Ok((body, truncated))
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
/// Returns [`FetchError::TooLarge`] if the response exceeds `max_bytes`, or
/// [`FetchError::BodyRead`] on a transport failure mid-stream.
async fn read_body_capped(
    mut response: reqwest::Response,
    url: &str,
    max_bytes: usize,
) -> std::result::Result<Vec<u8>, FetchError> {
    let too_large = || FetchError::TooLarge {
        url: url.to_string(),
        limit: max_bytes,
    };

    // Precheck: an honest Content-Length over the cap is refused before any
    // body is read. A compressed response reports no usable length here, so the
    // streamed counter below is what catches an expanding payload.
    if let Some(len) = response.content_length()
        && len > max_bytes as u64
    {
        return Err(too_large());
    }

    let mut body: Vec<u8> = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_source| FetchError::BodyRead {
            url: url.to_string(),
        })?
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

    use super::{WebFetch, extract_html};
    use crate::config::FetchConfig;
    use promptforge_core::Error;
    use promptforge_core::tools::{Tool, ToolId};

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

    #[test]
    fn descriptor_is_stable_and_faithful() {
        let tool = WebFetch::new();

        assert_eq!(tool.id(), ToolId::new("promptforge", "web_fetch"));
        assert_eq!(tool.wire_name(), "web_fetch");
        assert_eq!(
            tool.description(),
            "Fetch a web page and return its main content as markdown."
        );
        assert_eq!(
            tool.parameters_schema(),
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
                    },
                    "raw": {
                        "type": "boolean",
                        "description": "Skip article extraction and render the whole HTML document. Use for a page that is mostly a table or list, where extraction would discard the content. Ignored for non-HTML responses. Defaults to false."
                    }
                },
                "required": ["url"]
            })
        );
    }

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
        // A text/html type routes this through the all-or-nothing HTML path,
        // where the streamed cap counts decompressed bytes.
        (
            [(CONTENT_ENCODING, "gzip"), (CONTENT_TYPE, "text/html")],
            compressed,
        )
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
            // A text/html type routes this through the HTML path, where the
            // Content-Length precheck refuses the declared oversize.
            .header(CONTENT_TYPE, "text/html")
            .body(Body::from_stream(head.chain(tail)))
            .expect("building the oversized-content-length response must succeed")
    }

    /// An HTML page whose payload is a table readability would discard, so
    /// only whole-page rendering (`raw = true`) keeps the cell text.
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

    /// Serves an HTML table page for the `raw` whole-page-render test.
    async fn table() -> Response {
        Response::builder()
            .header(CONTENT_TYPE, "text/html; charset=utf-8")
            .body(Body::from(TABLE_HTML))
            .expect("building the table html response must succeed")
    }

    /// Serves a JSON document verbatim under `application/json`.
    async fn json_route() -> Response {
        Response::builder()
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(JSON_BODY))
            .expect("building the json response must succeed")
    }

    /// Serves an `application/json` document far larger than the test cap.
    ///
    /// It carries an honest `Content-Length`, so the size cap refuses it. A
    /// truncated prefix of this JSON would be invalid, which is why the
    /// structured route must hard-refuse rather than truncate.
    async fn jsonbig_route() -> Response {
        let filler = "x".repeat(200_000);
        let body = format!(r#"{{"filler":"{filler}"}}"#);
        Response::builder()
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .expect("building the large json response must succeed")
    }

    /// Serves a body under a `Content-Type` naming a charset the decoder does
    /// not recognize, which the tool must refuse as [`FetchError::Undecodable`].
    async fn badcharset_route() -> Response {
        Response::builder()
            .header(CONTENT_TYPE, "text/plain; charset=not-a-charset")
            .body(Body::from("some plain body text"))
            .expect("building the bad-charset response must succeed")
    }

    /// Serves a small PDF-typed body, which the tool must refuse.
    async fn pdf_route() -> Response {
        Response::builder()
            .header(CONTENT_TYPE, "application/pdf")
            .body(Body::from(&b"%PDF-1.4 not a real pdf"[..]))
            .expect("building the pdf response must succeed")
    }

    /// Serves an octet-stream body, which the tool must refuse.
    async fn octet_route() -> Response {
        Response::builder()
            .header(CONTENT_TYPE, "application/octet-stream")
            .body(Body::from(vec![0u8, 1, 2, 3, 4, 5]))
            .expect("building the octet-stream response must succeed")
    }

    /// Serves a body with no `Content-Type` header, which the tool must refuse.
    async fn notype_route() -> Response {
        Response::builder()
            .body(Body::from("a body with no declared content type"))
            .expect("building the no-content-type response must succeed")
    }

    /// Serves a 404 Not Found response.
    async fn not_found_route() -> Response {
        Response::builder()
            .status(axum::http::StatusCode::NOT_FOUND)
            .header(CONTENT_TYPE, "text/html")
            .body(Body::from("<html><body>Not Found</body></html>"))
            .expect("building the 404 response must succeed")
    }

    /// Serves a 500 Internal Server Error response.
    async fn internal_error_route() -> Response {
        Response::builder()
            .status(axum::http::StatusCode::INTERNAL_SERVER_ERROR)
            .header(CONTENT_TYPE, "text/html")
            .body(Body::from("<html><body>Server Error</body></html>"))
            .expect("building the 500 response must succeed")
    }

    /// Serves `Café` encoded in ISO-8859-1 under a declared latin-1 charset.
    ///
    /// The `é` is byte `0xE9`, which is invalid standalone UTF-8, so a correct
    /// decode requires honoring the declared charset.
    async fn latin1_route() -> Response {
        let body = vec![b'C', b'a', b'f', 0xE9];
        Response::builder()
            .header(CONTENT_TYPE, "text/plain; charset=ISO-8859-1")
            .body(Body::from(body))
            .expect("building the latin-1 response must succeed")
    }

    /// Serves a large `text/plain` body used to exercise flat-text truncation.
    async fn plainbig_route() -> Response {
        Response::builder()
            .header(CONTENT_TYPE, "text/plain; charset=utf-8")
            .body(Body::from("y".repeat(200_000)))
            .expect("building the large text response must succeed")
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

    /// Shared state for the recording server: its own port and the header maps
    /// of every request that reached a recording route.
    #[derive(Clone)]
    struct RecordingState {
        /// The port the server bound to, embedded in the redirect target.
        port: u16,
        /// The headers of each request that hit a recording route, in order.
        recorded: Arc<Mutex<Vec<HeaderMap>>>,
    }

    /// Records the incoming request headers, then serves the article page.
    ///
    /// Used to assert that the request `web_fetch` makes carries no `Cookie` or
    /// `Authorization` header.
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

    /// Redirects once to the recording route on the same loopback host.
    ///
    /// The target is an allowed `localhost` path, so the hop is followed and the
    /// final request's headers are recorded, letting a test assert no credential
    /// survives the redirect.
    async fn redirect_to_record(State(state): State<RecordingState>) -> Redirect {
        Redirect::temporary(&format!("http://localhost:{}/record", state.port))
    }

    /// Sleeps well past any small test timeout, then responds.
    ///
    /// A fetch with a short total timeout aborts before this returns, surfacing
    /// as [`FetchError::Timeout`].
    async fn slow() -> Html<&'static str> {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        Html(ARTICLE_HTML)
    }

    /// Binds a loopback axum server exposing the recording and slow routes.
    ///
    /// Returns the port and the shared vector of recorded request headers.
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
            .route("/slow", get(slow))
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
        let config = FetchConfig {
            timeout: std::time::Duration::from_millis(200),
            ..loopback_config(port)
        };
        let tool = WebFetch::with_config(config);

        let url = format!("http://localhost:{port}/slow");
        let result = tool
            .call(serde_json::json!({ "url": url }))
            .await
            .expect("a timeout is now a soft (recoverable) return");

        assert!(
            result.contains("timed out"),
            "expected a timeout message, got: {result}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn request_carries_no_cookie_or_credential() {
        let (port, recorded) = spawn_recording_server().await;
        let tool = WebFetch::with_config(loopback_config(port));

        let url = format!("http://localhost:{port}/record");
        tool.call(serde_json::json!({ "url": url }))
            .await
            .expect("a loopback fetch through allow_exact must succeed");

        let recorded = recorded
            .lock()
            .expect("the recorded-headers mutex must not be poisoned");
        assert_eq!(
            recorded.len(),
            1,
            "the recording route must have been hit exactly once"
        );
        let headers = &recorded[0];
        assert!(
            !headers.contains_key(axum::http::header::COOKIE),
            "the request must carry no Cookie header, got: {headers:?}"
        );
        assert!(
            !headers.contains_key(axum::http::header::AUTHORIZATION),
            "the request must carry no Authorization header, got: {headers:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn no_credential_survives_a_redirect() {
        let (port, recorded) = spawn_recording_server().await;
        let tool = WebFetch::with_config(loopback_config(port));

        // A redirect between two loopback paths: the first hop redirects to the
        // recording route, and the final request's headers must still carry no
        // Cookie or Authorization.
        let url = format!("http://localhost:{port}/redir-record");
        tool.call(serde_json::json!({ "url": url }))
            .await
            .expect("a redirect between loopback paths must succeed");

        let recorded = recorded
            .lock()
            .expect("the recorded-headers mutex must not be poisoned");
        assert_eq!(
            recorded.len(),
            1,
            "the final recording route must have been reached exactly once"
        );
        let headers = &recorded[0];
        assert!(
            !headers.contains_key(axum::http::header::COOKIE),
            "no Cookie header may survive a redirect, got: {headers:?}"
        );
        assert!(
            !headers.contains_key(axum::http::header::AUTHORIZATION),
            "no Authorization header may survive a redirect, got: {headers:?}"
        );
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
        let result = tool
            .call(serde_json::json!({ "url": url }))
            .await
            .expect("a redirect-target policy refusal is a soft (recoverable) return");

        assert!(
            result.contains("refused") && result.contains("127.0.0.1"),
            "the soft return must name the refused redirect, got: {result}"
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
                matches!(err, Error::Parse(_)),
                "expected a policy rejection (Error::Parse) for {raw}, got: {err:?}"
            );
            assert!(
                err.to_string().contains(reason),
                "expected policy reason {reason:?} for {raw}, got: {err}"
            );
        }

        // Plain http is refused without a network hop, but as soft tool text so
        // the model can retry https without aborting a fanout arm.
        let soft = tool
            .call(serde_json::json!({ "url": "http://example.com/" }))
            .await
            .expect("blocked http scheme must be soft tool text");
        assert!(
            soft.contains("scheme not allowed: http"),
            "expected soft scheme refusal, got: {soft}"
        );
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
        let result = tool
            .call(serde_json::json!({ "url": url }))
            .await
            .expect("an oversized HTML body is now a soft (recoverable) return");

        assert!(
            result.contains("exceeds") && result.contains("4096"),
            "expected a TooLarge message naming the cap, got: {result}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn declared_content_length_over_cap_is_refused_before_read() {
        let (port, _hits) = spawn_server().await;
        let config = FetchConfig {
            max_bytes: 4096,
            ..loopback_config(port)
        };
        let tool = WebFetch::with_config(config);

        let url = format!("http://localhost:{port}/liar");
        let result = tool
            .call(serde_json::json!({ "url": url }))
            .await
            .expect("a declared Content-Length over the cap is now a soft return");

        assert!(
            result.contains("exceeds") && result.contains("4096"),
            "expected a TooLarge message naming the cap from the precheck, got: {result}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn gzip_bomb_refused_on_decompressed_count() {
        let (port, _hits) = spawn_server().await;
        let config = FetchConfig {
            max_bytes: 4096,
            ..loopback_config(port)
        };
        let tool = WebFetch::with_config(config);

        let url = format!("http://localhost:{port}/gzip");
        let result = tool
            .call(serde_json::json!({ "url": url }))
            .await
            .expect("a gzip body that decompresses past the cap is now a soft return");

        assert!(
            result.contains("exceeds"),
            "expected a TooLarge message on the decompressed count, got: {result}"
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn html_is_extracted_and_reports_readability() {
        let (port, _hits) = spawn_server().await;
        let tool = WebFetch::with_config(loopback_config(port));

        let url = format!("http://localhost:{port}/");
        let out = tool
            .call(serde_json::json!({ "url": url }))
            .await
            .expect("a loopback html fetch must succeed");

        let (header, body) = split_header(&out);
        assert!(
            header.contains("extraction: readability"),
            "an article page must report readability extraction, got: {header}"
        );
        assert!(
            body.contains("substantial paragraph"),
            "the extracted article content must appear, got: {body}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn raw_forces_whole_page_render_keeping_table() {
        let (port, _hits) = spawn_server().await;
        let tool = WebFetch::with_config(loopback_config(port));

        let url = format!("http://localhost:{port}/table");
        let out = tool
            .call(serde_json::json!({ "url": url, "raw": true }))
            .await
            .expect("a raw table fetch must succeed");

        let (header, body) = split_header(&out);
        assert!(
            header.contains("extraction: raw-html"),
            "raw must force whole-page rendering, got: {header}"
        );
        assert!(
            body.contains("WIDGETROW") && body.contains("GADGETROW"),
            "raw whole-page rendering must retain the table cells, got: {body}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn json_is_returned_verbatim_as_plain() {
        let (port, _hits) = spawn_server().await;
        let tool = WebFetch::with_config(loopback_config(port));

        let url = format!("http://localhost:{port}/json");
        let out = tool
            .call(serde_json::json!({ "url": url }))
            .await
            .expect("a json fetch must succeed");

        let (header, body) = split_header(&out);
        assert!(
            header.contains("extraction: plain"),
            "json must be returned as plain, got: {header}"
        );
        assert_eq!(
            body, JSON_BODY,
            "json must be returned verbatim with no extraction, got: {body}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn oversized_json_is_hard_refused_not_truncated() {
        let (port, _hits) = spawn_server().await;
        let config = FetchConfig {
            max_bytes: 4096,
            ..loopback_config(port)
        };
        let tool = WebFetch::with_config(config);

        let url = format!("http://localhost:{port}/jsonbig");
        let result = tool
            .call(serde_json::json!({ "url": url }))
            .await
            .expect("an oversized json body is now a soft (recoverable) return");

        assert!(
            result.contains("exceeds") && result.contains("4096"),
            "expected a TooLarge message for oversized json, got: {result}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unrecognized_charset_is_refused_naming_the_label() {
        let (port, _hits) = spawn_server().await;
        let tool = WebFetch::with_config(loopback_config(port));

        let url = format!("http://localhost:{port}/badcharset");
        let result = tool
            .call(serde_json::json!({ "url": url }))
            .await
            .expect("an unrecognized charset is now a soft (recoverable) return");

        assert!(
            result.contains("not-a-charset"),
            "expected an Undecodable message naming the charset label, got: {result}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pdf_is_refused_naming_the_type() {
        let (port, _hits) = spawn_server().await;
        let tool = WebFetch::with_config(loopback_config(port));

        let url = format!("http://localhost:{port}/pdf");
        let result = tool
            .call(serde_json::json!({ "url": url }))
            .await
            .expect("a pdf response is now a soft (recoverable) return");

        assert!(
            result.contains("application/pdf"),
            "expected an unsupported-type message naming the pdf type, got: {result}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn octet_stream_is_refused() {
        let (port, _hits) = spawn_server().await;
        let tool = WebFetch::with_config(loopback_config(port));

        let url = format!("http://localhost:{port}/octet");
        let result = tool
            .call(serde_json::json!({ "url": url }))
            .await
            .expect("an octet-stream response is now a soft (recoverable) return");

        assert!(
            result.contains("application/octet-stream"),
            "expected an unsupported-type message naming the octet-stream type, got: {result}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn absent_content_type_is_refused() {
        let (port, _hits) = spawn_server().await;
        let tool = WebFetch::with_config(loopback_config(port));

        let url = format!("http://localhost:{port}/notype");
        let result = tool
            .call(serde_json::json!({ "url": url }))
            .await
            .expect("an absent content type is now a soft (recoverable) return");

        assert!(
            result.contains("no content type"),
            "expected a no-content-type message, got: {result}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn latin1_page_decodes_with_declared_charset() {
        let (port, _hits) = spawn_server().await;
        let tool = WebFetch::with_config(loopback_config(port));

        let url = format!("http://localhost:{port}/latin1");
        let out = tool
            .call(serde_json::json!({ "url": url }))
            .await
            .expect("a latin-1 fetch must succeed");

        let (header, body) = split_header(&out);
        assert!(
            header.contains("extraction: plain"),
            "a text/plain body must be returned as plain, got: {header}"
        );
        assert!(
            body.contains('é'),
            "the latin-1 byte 0xE9 must decode to 'é', got: {body:?}"
        );
        assert!(
            !body.contains('\u{FFFD}'),
            "a correct charset decode must not produce replacement chars, got: {body:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn plain_text_over_cap_is_truncated_not_refused() {
        let (port, _hits) = spawn_server().await;
        let config = FetchConfig {
            max_bytes: 4096,
            ..loopback_config(port)
        };
        let tool = WebFetch::with_config(config);

        let url = format!("http://localhost:{port}/plainbig");
        let out = tool
            .call(serde_json::json!({ "url": url }))
            .await
            .expect("an oversized flat-text body must be truncated, not refused");

        let (header, body) = split_header(&out);
        assert!(
            header.contains("truncated: true"),
            "an oversized flat-text body must be flagged truncated, got: {header}"
        );
        assert!(
            header.contains("extraction: plain"),
            "flat text must be returned as plain, got: {header}"
        );
        assert_eq!(
            body.len(),
            4096,
            "the flat-text prefix must be cut to the byte cap, got {} bytes",
            body.len()
        );
    }

    #[test]
    fn recoverable_failure_is_returned_as_untrusted_output() {
        use promptforge_core::tools::OutputTrust;

        use crate::error::FetchError;

        // Fetched page text is attacker-controllable, so a recoverable failure
        // that the model reads as tool text must carry `Untrusted` trust.
        let outcome = super::soft_or_hard(FetchError::Timeout {
            url: "https://example.test/".to_string(),
        })
        .expect("a recoverable failure is a soft, successful tool result");
        assert_eq!(outcome.trust(), OutputTrust::Untrusted);
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

        let (markdown, _mode) = extract_html(html, Some("https://example.com/article"), false);

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

        let (markdown, _mode) = extract_html(html, None, false);

        assert!(
            !markdown.trim().is_empty(),
            "fallback conversion should return non-empty markdown, got: {markdown:?}"
        );
        assert!(
            markdown.contains("short"),
            "fallback should preserve the page text, got: {markdown}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn soft_return_on_404() {
        let (port, _hits) = spawn_server().await;
        let tool = WebFetch::with_config(loopback_config(port));

        let url = format!("http://localhost:{port}/notfound");
        let result = tool
            .call(serde_json::json!({ "url": url }))
            .await
            .expect("a 404 must be a soft (recoverable) return, not a hard error");

        assert!(
            result.contains("404"),
            "the soft return must mention the status code 404, got: {result}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn soft_return_on_500() {
        let (port, _hits) = spawn_server().await;
        let tool = WebFetch::with_config(loopback_config(port));

        let url = format!("http://localhost:{port}/error500");
        let result = tool
            .call(serde_json::json!({ "url": url }))
            .await
            .expect("a 500 must be a soft (recoverable) return, not a hard error");

        assert!(
            result.contains("500"),
            "the soft return must mention the status code 500, got: {result}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn soft_return_on_pdf_content_type() {
        let (port, _hits) = spawn_server().await;
        let tool = WebFetch::with_config(loopback_config(port));

        let url = format!("http://localhost:{port}/pdf");
        let result = tool
            .call(serde_json::json!({ "url": url }))
            .await
            .expect("a PDF content type must be a soft return, not a hard error");

        assert!(
            result.contains("application/pdf"),
            "the soft return must mention the content type, got: {result}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn blocked_url_still_hard_fails() {
        let tool = WebFetch::new();

        let err = tool
            .call(serde_json::json!({ "url": "https://1.2.3.4/secret" }))
            .await
            .expect_err("a bare IP literal URL must still be a hard error");

        assert!(
            matches!(&err, Error::Parse(msg) if msg.contains("ip literal")),
            "expected a policy rejection for a bare IP literal, got: {err:?}"
        );
    }
}
