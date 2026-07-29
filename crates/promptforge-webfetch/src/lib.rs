//! The `web_fetch` tool: fetch a URL and return its main content as markdown.
//!
//! This tool runs locally in-process. It performs a plain HTTP GET, extracts
//! the page's main article content with [`readabilityrs`], and renders it to
//! markdown. Pages that are not article-shaped fall back to a whole-page
//! HTML-to-markdown conversion with [`htmd`], so the tool always returns
//! something useful when the fetch itself succeeds.

use readabilityrs::{Readability, ReadabilityOptions};

use promptforge_core::tools::Tool;
use promptforge_core::{Error, Result};

pub mod config;
pub mod error;
pub mod url_policy;

pub use config::FetchConfig;
pub use error::FetchError;
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
    #[must_use]
    pub fn with_config(config: FetchConfig) -> WebFetch {
        WebFetch {
            http: reqwest::Client::new(),
            config,
        }
    }
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

        // Enforce the URL-admission policy before any network access.
        let url = check_url(url, &self.config)?;

        let response = self
            .http
            .get(url.clone())
            .timeout(FETCH_TIMEOUT)
            .send()
            .await
            .map_err(|source| Error::Http(Box::new(source)))?;

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

        let html = response
            .text()
            .await
            .map_err(|source| Error::Http(Box::new(source)))?;
        Ok(extract_markdown(&html, Some(url.as_str())))
    }
}

#[cfg(test)]
mod tests {
    use super::{WebFetch, extract_markdown};
    use promptforge_core::Error;
    use promptforge_core::tools::Tool;

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
