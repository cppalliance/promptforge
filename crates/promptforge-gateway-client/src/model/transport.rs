//! Catalog transport: fetching and decoding gateway `GET /v1/models`, plus
//! the `GET /admin/progress` event-stream subscription.

use std::num::NonZeroU32;

use futures_util::Stream;
use promptforge_progress::ProgressEvent;
use serde::Deserialize;

use super::{CompletionError, ModelCatalog, ModelDescriptor, ModelId, ThinkingMode};
use crate::Error;

/// Wire shape of one entry from gateway `GET /v1/models`.
#[derive(Debug, Deserialize)]
struct ModelsListEntry {
    id: String,
    description: String,
    context: u32,
    thinking: ThinkingMode,
}

/// Wire shape of gateway `GET /v1/models`.
#[derive(Debug, Deserialize)]
struct ModelsListResponse {
    data: Vec<ModelsListEntry>,
}

/// The largest gateway error body kept for a catalog-fetch diagnostic, in bytes.
pub(crate) const MAX_CATALOG_ERROR_BODY: usize = 2000;

/// The largest success-path model-catalog body accepted before decoding, in
/// bytes. A gateway that returns more than this is refused rather than buffered
/// unbounded, mirroring the bound the error path already applies. Sized well
/// above any realistic model list (16 MiB) so legitimate catalogs are unaffected.
pub(crate) const MAX_CATALOG_BODY: u64 = 16 * 1024 * 1024;

/// Reads a success-path response body, refusing it once it would exceed `cap`
/// bytes so a decode cannot buffer an unbounded body first.
///
/// The advertised `Content-Length` short-circuits an oversize body, and the
/// streamed chunks are bounded so a gateway that omits or lies about the length
/// still cannot force an unbounded allocation.
async fn read_catalog_body_capped(
    mut response: reqwest::Response,
    cap: u64,
) -> std::result::Result<Vec<u8>, CompletionError> {
    if let Some(len) = response.content_length()
        && len > cap
    {
        return Err(CompletionError::from(Error::MalformedResponse(format!(
            "model list body of {len} bytes exceeds the {cap}-byte limit"
        ))));
    }
    let mut body: Vec<u8> = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(Error::http)? {
        if body.len() as u64 + chunk.len() as u64 > cap {
            return Err(CompletionError::from(Error::MalformedResponse(format!(
                "model list body exceeds the {cap}-byte limit"
            ))));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Reads at most `limit` bytes of a non-success response body, stopping early so
/// an oversized error body cannot exhaust memory.
///
/// A read failure is returned as the concrete [`reqwest::Error`] (MODEL-010) so
/// the caller can retain it as an error-chain `#[source]`, rather than being
/// flattened into display text that severs the cause.
async fn read_error_body_bounded(
    mut response: reqwest::Response,
    limit: usize,
) -> std::result::Result<String, reqwest::Error> {
    let mut buffer: Vec<u8> = Vec::new();
    while buffer.len() < limit {
        match response.chunk().await? {
            Some(chunk) => {
                let take = (limit - buffer.len()).min(chunk.len());
                buffer.extend_from_slice(&chunk[..take]);
                if take < chunk.len() {
                    break;
                }
            }
            None => break,
        }
    }
    if buffer.is_empty() {
        return Ok("(empty body)".to_owned());
    }
    // F5: escape control characters so a hostile catalog error body cannot forge
    // log lines or smuggle terminal control sequences into a diagnostic.
    let lossy = String::from_utf8_lossy(&buffer);
    let mut escaped = String::with_capacity(lossy.len());
    for ch in lossy.chars() {
        if ch.is_control() {
            escaped.extend(ch.escape_default());
        } else {
            escaped.push(ch);
        }
    }
    Ok(escaped)
}

/// Returns the process-wide catalog HTTP client, building it once on first use.
///
/// A single reusable client (MODEL-018) lets catalog fetches share one
/// connection pool and transport configuration rather than each constructing a
/// throwaway client with its own pool. The returned handle is a cheap clone of
/// the shared client (its state is reference-counted internally).
fn catalog_client() -> reqwest::Client {
    static CATALOG_CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CATALOG_CLIENT.get_or_init(reqwest::Client::new).clone()
}

/// Sends a bearer-authed GET through the shared client (MODEL-018) and
/// returns the success response, classifying every failure the same way for
/// each gateway endpoint: `Transport` when the send fails, `Backend` with a
/// bounded, control-escaped body on a non-success status (MODEL-010: no
/// unbounded buffering), and `BackendBodyRead` when that error body cannot be
/// read, keeping the [`reqwest::Error`] as a typed source.
async fn get_authed(
    url: String,
    token: &str,
) -> std::result::Result<reqwest::Response, CompletionError> {
    let response = catalog_client()
        .get(url)
        .bearer_auth(token)
        .send()
        .await
        .map_err(Error::http)?;
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = match read_error_body_bounded(response, MAX_CATALOG_ERROR_BODY).await {
        Ok(body) => body,
        Err(source) => {
            return Err(CompletionError::from(Error::BackendBodyRead {
                status: status.as_u16(),
                source: Box::new(source),
            }));
        }
    };
    Err(CompletionError::from(Error::Backend {
        status: status.as_u16(),
        body,
    }))
}

/// Fetches a [`ModelCatalog`] from a bearer-authed gateway `/models` endpoint.
///
/// `base_url` is the OpenAI-shaped API root (for example `http://127.0.0.1:8081/v1`).
///
/// # Errors
/// Returns a [`CompletionError`] whose [`kind`](CompletionError::kind) is
/// `Transport` on transport failure, `Backend` on a non-success status, and
/// `MalformedResponse` when the body is not a model list.
///
/// # Examples
///
/// ```no_run
/// # async fn run() -> Result<(), promptforge_gateway_client::model::CompletionError> {
/// use promptforge_gateway_client::model::fetch_model_catalog;
///
/// let catalog = fetch_model_catalog("http://127.0.0.1:8081/v1", "secret-token").await?;
/// println!("gateway offers {} models", catalog.models().len());
/// # Ok(())
/// # }
/// ```
pub async fn fetch_model_catalog(
    base_url: &str,
    token: &str,
) -> std::result::Result<ModelCatalog, CompletionError> {
    let base = base_url.trim_end_matches('/');
    let response = get_authed(format!("{base}/models"), token).await?;
    // Bound the success body BEFORE decoding so an oversized (or unbounded)
    // model list cannot exhaust memory, matching the bound the error path applies.
    let body = read_catalog_body_capped(response, MAX_CATALOG_BODY).await?;
    // A body that does not decode as a model list is a malformed response, not a
    // transport failure - matching this function's documented error contract.
    let list: ModelsListResponse = serde_json::from_slice(&body).map_err(|error| {
        // MODEL-009: keep the decode error as a private `#[source]` cause instead
        // of flattening it into the message, while the classification stays
        // `MalformedResponse`.
        CompletionError::from(Error::MalformedResponseSource {
            message: "model list response was not valid JSON".to_owned(),
            source: Box::new(error),
        })
    })?;
    let mut descriptors = Vec::with_capacity(list.data.len());
    for entry in list.data {
        let id = ModelId::gateway(entry.id).map_err(|error| {
            CompletionError::from(Error::MalformedResponse(format!(
                "model catalog entry has an invalid id: {error}"
            )))
        })?;
        let context = NonZeroU32::new(entry.context).ok_or_else(|| {
            CompletionError::from(Error::MalformedResponse(format!(
                "model {} declares a zero-token context window",
                id.name()
            )))
        })?;
        descriptors.push(ModelDescriptor::new(
            id,
            entry.description,
            context,
            entry.thinking,
        ));
    }
    ModelCatalog::new(descriptors).map_err(|error| {
        CompletionError::from(Error::MalformedResponse(format!(
            "gateway returned an inconsistent model catalog: {error}"
        )))
    })
}

/// The largest single SSE event block buffered before the stream refuses it,
/// in bytes. A peer that never sends a blank-line terminator would otherwise
/// grow the reassembly buffer unbounded (MODEL-010); sized well above any
/// realistic progress event.
const MAX_EVENT_BLOCK: usize = 1024 * 1024;

/// One decoded item of a [`subscribe_progress`] stream.
type ProgressStreamItem = std::result::Result<ProgressEvent, CompletionError>;

/// Subscribes to a bearer-authed gateway `GET /admin/progress` event stream.
///
/// `base_url` is the gateway root (for example `http://127.0.0.1:8081`), not
/// the OpenAI-shaped `/v1` API root [`fetch_model_catalog`] takes.
///
/// The returned stream yields every [`ProgressEvent`] the gateway sends,
/// beginning with the snapshot replay of the operations live at connect time.
/// Heartbeat comment lines and other non-`data:` lines are skipped.
/// Intermediate events are lossy at the source, so the stream promises no
/// completeness; detect completion only from `Finished` events, never from a
/// fraction reaching 1.0. A `data:` line that does not decode is yielded as
/// one `Err` item without ending the stream; a read failure or an event block
/// oversized beyond one MiB is yielded as one `Err` item that ends the
/// stream. The stream ends when the gateway closes the body; whether to
/// resubscribe is the caller's decision.
///
/// # Errors
/// Returns a [`CompletionError`] whose [`kind`](CompletionError::kind) is
/// `Transport` on transport failure and `Backend` on a non-success status
/// (for example 401 on a rejected token). Decode failures surface as per-item
/// `Err` values instead.
///
/// # Examples
///
/// ```no_run
/// # async fn run() -> Result<(), promptforge_gateway_client::model::CompletionError> {
/// use futures_util::StreamExt;
/// use promptforge_gateway_client::model::subscribe_progress;
///
/// let events = subscribe_progress("http://127.0.0.1:8081", "secret-token").await?;
/// futures_util::pin_mut!(events);
/// while let Some(item) = events.next().await {
///     let event = item?;
///     println!("{}: {}", event.path, event.label);
/// }
/// # Ok(())
/// # }
/// ```
pub async fn subscribe_progress(
    base_url: &str,
    token: &str,
) -> std::result::Result<impl Stream<Item = ProgressStreamItem> + Send, CompletionError> {
    let base = base_url.trim_end_matches('/');
    let response = get_authed(format!("{base}/admin/progress"), token).await?;
    Ok(progress_event_stream(response))
}

/// Decodes an SSE body into an event stream: chunks are buffered until a
/// blank line terminates an event block, comment-only blocks (heartbeats) are
/// skipped, and an undecodable block becomes one `Err` item rather than
/// killing the stream. A mid-stream read failure likewise surfaces as one
/// `Err` item, after which the stream ends. A block that grows past
/// [`MAX_EVENT_BLOCK`] without a terminator is refused as one `Err` item,
/// after which the stream ends, so a peer cannot buffer the client unbounded.
fn progress_event_stream(
    response: reqwest::Response,
) -> impl Stream<Item = ProgressStreamItem> + Send {
    futures_util::stream::unfold(
        (response, Vec::new(), false),
        |(mut response, mut buffer, mut failed)| async move {
            loop {
                if let Some(item) = next_buffered_event(&mut buffer) {
                    return Some((item, (response, buffer, failed)));
                }
                if failed {
                    return None;
                }
                match response.chunk().await {
                    Ok(Some(chunk)) => {
                        if buffer.len() + chunk.len() > MAX_EVENT_BLOCK {
                            failed = true;
                            let item =
                                Err(CompletionError::from(Error::MalformedResponse(format!(
                                    "progress event block exceeds the {MAX_EVENT_BLOCK}-byte limit"
                                ))));
                            return Some((item, (response, buffer, failed)));
                        }
                        buffer.extend_from_slice(&chunk);
                    }
                    // An incomplete trailing block is discarded, matching the
                    // SSE rule that only blank-line-terminated blocks dispatch.
                    Ok(None) => return None,
                    Err(source) => {
                        failed = true;
                        let item = Err(CompletionError::from(Error::http(source)));
                        return Some((item, (response, buffer, failed)));
                    }
                }
            }
        },
    )
}

/// Pops the next decodable event out of `buffer`, or `None` when no complete
/// block is buffered yet. Comment-only blocks (heartbeats) are consumed and
/// skipped.
fn next_buffered_event(buffer: &mut Vec<u8>) -> Option<ProgressStreamItem> {
    loop {
        let end = buffer.windows(2).position(|pair| pair == b"\n\n")?;
        let block: Vec<u8> = buffer.drain(..end + 2).collect();
        if let Some(item) = parse_event_block(&block) {
            return Some(item);
        }
    }
}

/// Decodes one SSE event block: `data:` lines join into the payload, comment
/// lines and unrecognized fields are ignored, and a block with no payload
/// (a heartbeat) yields `None`.
fn parse_event_block(block: &[u8]) -> Option<ProgressStreamItem> {
    let mut data: Vec<u8> = Vec::new();
    for line in block.split(|byte| *byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if let Some(rest) = line.strip_prefix(b"data:") {
            if !data.is_empty() {
                data.push(b'\n');
            }
            data.extend_from_slice(rest.strip_prefix(b" ").unwrap_or(rest));
        }
    }
    if data.is_empty() {
        return None;
    }
    Some(serde_json::from_slice(&data).map_err(|source| {
        CompletionError::from(Error::MalformedResponseSource {
            message: "progress event was not valid JSON".to_owned(),
            source: Box::new(source),
        })
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::CompletionErrorKind;

    async fn spawn_models(app: axum::Router) -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        addr
    }

    #[tokio::test]
    async fn fetch_model_catalog_bounds_and_reports_non_success_body() {
        use axum::Router;
        use axum::routing::get;

        async fn models() -> (axum::http::StatusCode, String) {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "e".repeat(MAX_CATALOG_ERROR_BODY * 4),
            )
        }
        let app = Router::new().route("/models", get(models));
        let addr = spawn_models(app).await;

        let err = fetch_model_catalog(&format!("http://{addr}"), "tok")
            .await
            .expect_err("a 500 response must surface as an error");
        assert_eq!(err.kind(), CompletionErrorKind::Backend);
        let msg = err.to_string();
        assert!(
            msg.len() < MAX_CATALOG_ERROR_BODY + 128,
            "the error-path body must be bounded, got {} bytes",
            msg.len()
        );
    }

    #[tokio::test]
    async fn fetch_model_catalog_bounds_an_oversized_success_body() {
        use axum::Router;
        use axum::routing::get;

        // A 200 response whose body exceeds the success cap must be refused
        // BEFORE decoding, not buffered unbounded. The body is deliberately not
        // valid JSON: the bound must trip first, regardless of contents.
        async fn models() -> (axum::http::StatusCode, String) {
            let oversized = usize::try_from(MAX_CATALOG_BODY).unwrap() + 1;
            (axum::http::StatusCode::OK, "e".repeat(oversized))
        }
        let app = Router::new().route("/models", get(models));
        let addr = spawn_models(app).await;

        let err = fetch_model_catalog(&format!("http://{addr}"), "tok")
            .await
            .expect_err("an oversized success body must be refused");
        assert_eq!(err.kind(), CompletionErrorKind::MalformedResponse);
        assert!(
            err.to_string().contains("exceeds"),
            "the bound must report the size limit, got {err}"
        );
    }

    #[tokio::test]
    async fn fetch_model_catalog_preserves_the_json_decode_source() {
        use axum::Router;
        use axum::routing::get;

        // MODEL-009: a 200 body that is not a valid model list is classified as
        // MalformedResponse, and the underlying `serde_json::Error` survives as
        // the error-chain `#[source]` rather than being flattened into the text.
        async fn models() -> (axum::http::StatusCode, String) {
            (axum::http::StatusCode::OK, "{ this is not json".to_owned())
        }
        let app = Router::new().route("/models", get(models));
        let addr = spawn_models(app).await;

        let err = fetch_model_catalog(&format!("http://{addr}"), "tok")
            .await
            .expect_err("an undecodable body must surface as an error");
        assert_eq!(err.kind(), CompletionErrorKind::MalformedResponse);
        let source =
            std::error::Error::source(&err).expect("the decode error must be a preserved source");
        assert!(
            source.downcast_ref::<serde_json::Error>().is_some(),
            "the preserved source must be the JSON decode error, got {source}"
        );
    }

    #[tokio::test]
    async fn fetch_model_catalog_preserves_a_body_read_failure_source() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        // MODEL-010: a non-success response whose body cannot be fully read
        // (the server promises a large body then drops the connection) must
        // surface as a typed transport failure that keeps the `reqwest::Error`
        // as its `#[source]`, not display text.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf).await;
                let header = "HTTP/1.1 500 Internal Server Error\r\n\
                     Content-Length: 1000000\r\n\r\n";
                let _ = sock.write_all(header.as_bytes()).await;
                let _ = sock.write_all(b"abc").await;
                // Socket drops here: the promised body never completes.
            }
        });

        let err = fetch_model_catalog(&format!("http://{addr}"), "tok")
            .await
            .expect_err("a truncated error body must surface as an error");
        assert_eq!(err.kind(), CompletionErrorKind::Transport);
        assert_eq!(err.status(), Some(500));
        let source =
            std::error::Error::source(&err).expect("the read failure must be a preserved source");
        assert!(
            source.downcast_ref::<reqwest::Error>().is_some(),
            "the preserved source must be the reqwest read error, got {source}"
        );
    }

    use promptforge_progress::EventState;

    /// Serializes a wire-format progress event by hand, so the tests pin the
    /// JSON shape rather than the progress crate's constructors.
    fn event_json(state: &serde_json::Value) -> String {
        serde_json::json!({
            "operation": 7,
            "path": "local-models/ggml/download",
            "label": "Download",
            "state": state,
        })
        .to_string()
    }

    /// A mock `GET /admin/progress` that requires the bearer token and
    /// answers with `body` as the verbatim SSE payload.
    fn mock_progress(body: String) -> axum::Router {
        use axum::Router;
        use axum::response::IntoResponse;
        use axum::routing::get;

        Router::new().route(
            "/admin/progress",
            get(move |headers: axum::http::HeaderMap| {
                let body = body.clone();
                async move {
                    let auth = headers
                        .get(axum::http::header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok());
                    if auth != Some("Bearer tok") {
                        return (axum::http::StatusCode::UNAUTHORIZED, "bad token").into_response();
                    }
                    (
                        [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                        body,
                    )
                        .into_response()
                }
            }),
        )
    }

    #[tokio::test]
    async fn subscribe_progress_decodes_events_and_skips_heartbeat_comments() {
        use futures_util::StreamExt;

        let begun = event_json(&serde_json::json!({"Begun": {"weight": 2.0}}));
        let updated = event_json(&serde_json::json!({"Updated": {"fraction": 0.5}}));
        let finished = event_json(&serde_json::json!({"Finished": {"ok": true}}));
        let body = format!(
            ": heartbeat\n\ndata: {begun}\n\ndata: {updated}\n\n: heartbeat\n\ndata: {finished}\n\n"
        );
        let addr = spawn_models(mock_progress(body)).await;

        let events: Vec<_> = subscribe_progress(&format!("http://{addr}"), "tok")
            .await
            .expect("a well-formed stream subscribes")
            .collect()
            .await;

        let states: Vec<EventState> = events
            .iter()
            .map(|item| item.as_ref().expect("every item decodes").state)
            .collect();
        assert_eq!(
            states,
            vec![
                EventState::Begun { weight: 2.0 },
                EventState::Updated { fraction: 0.5 },
                EventState::Finished { ok: true },
            ]
        );
    }

    #[tokio::test]
    async fn subscribe_progress_classifies_a_non_success_status() {
        let addr = spawn_models(mock_progress(String::new())).await;

        let Err(err) = subscribe_progress(&format!("http://{addr}"), "wrong-token").await else {
            panic!("a 401 response must surface as an error");
        };
        assert_eq!(err.kind(), CompletionErrorKind::Backend);
        assert_eq!(err.status(), Some(401));
    }

    #[tokio::test]
    async fn subscribe_progress_yields_one_error_per_bad_event_and_continues() {
        use futures_util::StreamExt;

        let begun = event_json(&serde_json::json!({"Begun": {"weight": 1.0}}));
        let finished = event_json(&serde_json::json!({"Finished": {"ok": true}}));
        let body = format!("data: {begun}\n\ndata: {{not json\n\ndata: {finished}\n\n");
        let addr = spawn_models(mock_progress(body)).await;

        let events: Vec<_> = subscribe_progress(&format!("http://{addr}"), "tok")
            .await
            .expect("a well-formed stream subscribes")
            .collect()
            .await;

        assert_eq!(events.len(), 3, "one item per data block");
        assert!(events[0].is_ok(), "the leading event decodes");
        let err = events[1]
            .as_ref()
            .expect_err("the undecodable line is one error item");
        assert_eq!(err.kind(), CompletionErrorKind::MalformedResponse);
        assert!(
            events[2].is_ok(),
            "a bad line must not end the stream: the trailing event still arrives"
        );
    }

    #[tokio::test]
    async fn subscribe_progress_reassembles_an_event_split_across_chunks() {
        use futures_util::StreamExt;

        let begun = event_json(&serde_json::json!({"Begun": {"weight": 1.0}}));
        let wire = format!("data: {begun}\n\n");
        let (head, tail) = wire.split_at(wire.len() / 2);
        let (head, tail) = (head.to_owned(), tail.to_owned());
        let app = axum::Router::new().route(
            "/admin/progress",
            axum::routing::get(move || {
                let chunks = vec![
                    Ok::<_, std::convert::Infallible>(head.clone()),
                    Ok::<_, std::convert::Infallible>(tail.clone()),
                ];
                async move {
                    (
                        [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                        axum::body::Body::from_stream(futures_util::stream::iter(chunks)),
                    )
                }
            }),
        );
        let addr = spawn_models(app).await;

        let events: Vec<_> = subscribe_progress(&format!("http://{addr}"), "tok")
            .await
            .expect("a well-formed stream subscribes")
            .collect()
            .await;

        assert_eq!(events.len(), 1, "the split block decodes as one event");
        assert!(events[0].is_ok(), "the reassembled event decodes");
    }

    #[tokio::test]
    async fn subscribe_progress_yields_one_error_on_a_mid_stream_read_failure_then_ends() {
        use futures_util::StreamExt;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        // The server promises a large body, delivers one complete event, then
        // drops the connection: the read failure must surface as one `Err`
        // item that ends the stream, not as a hang or a silent close.
        let begun = event_json(&serde_json::json!({"Begun": {"weight": 1.0}}));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf).await;
                let header = "HTTP/1.1 200 OK\r\n\
                     Content-Type: text/event-stream\r\n\
                     Content-Length: 1000000\r\n\r\n";
                let _ = sock.write_all(header.as_bytes()).await;
                let _ = sock
                    .write_all(format!("data: {begun}\n\n").as_bytes())
                    .await;
                // Socket drops here: the promised body never completes.
            }
        });

        let events: Vec<_> = subscribe_progress(&format!("http://{addr}"), "tok")
            .await
            .expect("a well-formed stream subscribes")
            .collect()
            .await;

        assert_eq!(events.len(), 2, "the decoded event, then one error item");
        assert!(events[0].is_ok(), "the leading event decodes");
        let err = events[1]
            .as_ref()
            .expect_err("the truncated body is one error item");
        assert_eq!(err.kind(), CompletionErrorKind::Transport);
    }

    #[tokio::test]
    async fn subscribe_progress_bounds_an_event_block_that_never_terminates() {
        use futures_util::StreamExt;

        let body = "x".repeat(MAX_EVENT_BLOCK + 1);
        let addr = spawn_models(mock_progress(body)).await;

        let events: Vec<_> = subscribe_progress(&format!("http://{addr}"), "tok")
            .await
            .expect("a well-formed stream subscribes")
            .collect()
            .await;

        assert_eq!(events.len(), 1, "the oversized block is one error item");
        let err = events[0]
            .as_ref()
            .expect_err("an unterminated oversized block must be refused");
        assert_eq!(err.kind(), CompletionErrorKind::MalformedResponse);
        assert!(
            err.to_string().contains("exceeds"),
            "the bound must report the size limit, got {err}"
        );
    }
}
