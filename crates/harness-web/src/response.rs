//! Response handling: content-type routing, bounded body reads, decoding, and
//! HTML-to-markdown extraction.
//!
//! The route is decided from the response `Content-Type` before the body is
//! read, so a binary or absent type is refused without downloading it. HTML and
//! structured text are read all-or-nothing under the byte cap; genuinely flat
//! text is truncated at the cap. Decoding honors the declared charset, and HTML
//! is rendered to markdown with readability plus a whole-page fallback.

use readabilityrs::{Readability, ReadabilityOptions};

use crate::error::{FetchError, SafeUrl};

/// The minimum extracted length below which the whole-page fallback fires.
const MIN_CONTENT_LEN: usize = 100;

/// How a response body was turned into the returned text.
///
/// The mode is reported on the provenance header's `extraction:` line so the
/// model knows whether it is holding an extracted article, a whole-page
/// rendering, or decoded plain text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Extraction {
    /// An HTML page's main article, isolated by [`readabilityrs`].
    Readability,
    /// A whole HTML document rendered to markdown, with no article extraction.
    RawHtml,
    /// A non-HTML text body decoded and returned verbatim, with no extraction.
    Plain,
}

impl Extraction {
    /// The label written on the provenance header's `extraction:` line.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Extraction::Readability => "readability",
            Extraction::RawHtml => "raw-html",
            Extraction::Plain => "plain",
        }
    }
}

/// How a response's `Content-Type` routes through the fetch pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Route {
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
pub(crate) fn classify(mime: &mime::Mime) -> Option<Route> {
    let type_ = mime.type_();
    let subtype = mime.subtype();
    let suffix = mime.suffix();

    let is_html = (type_ == mime::TEXT && subtype == mime::HTML)
        || (type_ == mime::APPLICATION && subtype == "xhtml" && suffix == Some(mime::XML));
    if is_html {
        return Some(Route::Html);
    }

    let is_json_or_xml = subtype == mime::JSON
        || subtype == mime::XML
        || suffix == Some(mime::JSON)
        || suffix == Some(mime::XML);

    if type_ == mime::APPLICATION && is_json_or_xml {
        return Some(Route::Plain { structured: true });
    }

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
pub(crate) fn decode_body(
    bytes: &[u8],
    charset: Option<&str>,
    url: &str,
) -> Result<String, FetchError> {
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
                    url: SafeUrl::new(url),
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
pub(crate) fn extract_html(html: &str, base_url: Option<&str>, raw: bool) -> (String, Extraction) {
    if raw {
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

    (
        htmd::convert(html).unwrap_or(article_markdown),
        Extraction::RawHtml,
    )
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
pub(crate) async fn read_body_truncating(
    response: reqwest::Response,
    max_bytes: usize,
) -> Result<(Vec<u8>, bool), FetchError> {
    let url = response.url().to_string();
    let mut response = response;
    let mut body: Vec<u8> = Vec::new();
    let mut truncated = false;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|source| FetchError::BodyRead {
            url: SafeUrl::new(&url),
            source,
        })?
    {
        let remaining = max_bytes - body.len();
        if chunk.len() > remaining {
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
pub(crate) async fn read_body_capped(
    mut response: reqwest::Response,
    url: &str,
    max_bytes: usize,
) -> Result<Vec<u8>, FetchError> {
    let too_large = || FetchError::TooLarge {
        url: SafeUrl::new(url),
        limit: max_bytes,
    };

    if let Some(len) = response.content_length()
        && len > max_bytes as u64
    {
        return Err(too_large());
    }

    let mut body: Vec<u8> = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|source| FetchError::BodyRead {
            url: SafeUrl::new(url),
            source,
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
pub(crate) fn truncate_to_chars(text: &str, max_chars: usize) -> (&str, bool) {
    match text.char_indices().nth(max_chars) {
        Some((idx, _)) => (&text[..idx], true),
        None => (text, false),
    }
}

#[cfg(test)]
mod tests {
    use super::extract_html;

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
}
