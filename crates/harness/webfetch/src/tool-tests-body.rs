//! Body tests for [`WebFetch`]: the byte cap refuses an oversized HTML or
//! structured body (declared, streamed, or decompressed) and truncates a
//! flat-text body, `max_chars` truncates on a character boundary and is
//! clamped to the ceiling, HTML routes through extraction or raw render,
//! JSON is returned verbatim, an unsupported or absent content type is
//! refused naming it, and a declared charset decodes the body.

use super::*;

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
        promptforge_api_types::tools::OutputTrust::Untrusted,
        "a soft body-read failure must be untrusted output"
    );
    let result = outcome.text().to_owned();
    assert!(
        result.contains("could not be read") || result.contains("network error"),
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
