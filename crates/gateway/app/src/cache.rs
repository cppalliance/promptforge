//! The `/v1/cache` routes: bearer-authenticated on-demand blob downloads into
//! the operator cache, with sidecar-based listing and removal.
//!
//! The store is blocking filesystem plus a reqwest-blocking client, so every
//! store operation runs inside `tokio::task::spawn_blocking` and never blocks
//! the executor (Amendment D). Each download begins an activity on the
//! process hub, whose text carries `"Downloading {name} {pct}%"` for the
//! status consumers, and keeps its own byte counts on a `watch` channel the
//! SSE response reads: intermediate samples coalesce under backpressure,
//! while the terminal ready/error event is produced from the download task's
//! join result and is therefore never lost.

use std::convert::Infallible;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::HeaderValue;
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use axum::{Json, Router};
use gateway_progress::Activity;
use serde::Deserialize;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::AppState;
use crate::auth::AuthedCaller;
use crate::error::{GatewayError, WireJson, blocking};
use crate::local::artifacts::{
    DownloadProgress, PercentText, filename_from_url, parse_expected_digest,
};
use crate::local::cache::{BlobCache, CacheEntry, CachedBlob};
use crate::local::{LocalError, resolve_cache_root};

/// The blob-cache routes.
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/cache", get(list_cache).post(post_cache))
        .route("/v1/cache/{sha256}", delete(delete_cache))
}

/// Opens the blob cache at the active profile's resolved cache root.
fn open_cache(cache_dir: Option<&str>) -> Result<BlobCache, LocalError> {
    BlobCache::new(resolve_cache_root(cache_dir)?)
}

/// The cache dir configured on the live profile (`[local].cache_dir`).
async fn live_cache_dir(state: &AppState) -> Option<String> {
    state.cache_dir().await
}

/// `GET /v1/cache`: the sidecar-backed listing of cached blobs.
///
/// Reads `<file>.meta.json` sidecars only, so listing never re-hashes a blob
/// (Amendment C); blobs without sidecars are not cache entries and do not
/// appear.
pub(crate) async fn list_cache(
    State(state): State<AppState>,
    _caller: AuthedCaller,
) -> Result<Json<Vec<CacheEntry>>, GatewayError> {
    let cache_dir = live_cache_dir(&state).await;
    let entries = blocking(move || open_cache(cache_dir.as_deref())?.list())
        .await?
        .map_err(GatewayError::cache)?;
    Ok(Json(entries))
}

/// The `POST /v1/cache` request body.
#[derive(Debug, Deserialize)]
pub(crate) struct CacheRequest {
    /// The http(s) URL to download.
    source: String,
    /// Optional SHA-256 pin, verified against the downloaded bytes.
    sha256: Option<String>,
}

/// Validates the network-facing `source`: an http(s) URL with a host and a
/// usable filename segment, which is returned for the download's leaf label.
/// Anything else is a 400, never a download attempt.
fn validate_source(source: &str) -> Result<String, GatewayError> {
    let parsed = url::Url::parse(source).map_err(|cause| {
        GatewayError::MalformedRequest(format!(
            "cache source `{source}` is not a valid URL: {cause}"
        ))
    })?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(GatewayError::MalformedRequest(format!(
            "cache source `{source}` must be an http or https URL with a host"
        )));
    }
    filename_from_url(source).map_err(|error| GatewayError::MalformedRequest(error.to_string()))
}

/// `POST /v1/cache`: ensure the blob for `source` is cached.
///
/// A cache hit (blob + sidecar present, pin matching when named) answers
/// immediately with JSON `{"path", "status": "ready"}`. A miss answers with
/// `text/event-stream`: `{"status": "downloading", "bytes", "total"}`
/// progress events (`total` is null when the server sent no Content-Length),
/// terminated by `{"status": "ready", "path"}` or, on failure,
/// `{"status": "error", "message"}`.
pub(crate) async fn post_cache(
    State(state): State<AppState>,
    _caller: AuthedCaller,
    WireJson(request): WireJson<CacheRequest>,
) -> Result<Response, GatewayError> {
    let label = validate_source(&request.source)?;
    let expected = request
        .sha256
        .as_deref()
        .map(parse_expected_digest)
        .transpose()
        .map_err(|error| GatewayError::MalformedRequest(error.to_string()))?;
    let cache_dir = live_cache_dir(&state).await;
    let source = request.source;

    // The hit check runs on the blocking pool; on a miss the opened store is
    // handed to the download task so the root is enforced exactly once.
    let lookup_source = source.clone();
    let lookup_pin = expected.clone();
    let (cache, hit) = blocking(move || {
        let cache = open_cache(cache_dir.as_deref())?;
        let hit = cache.lookup(&lookup_source, lookup_pin.as_deref())?;
        Ok::<_, LocalError>((cache, hit))
    })
    .await?
    .map_err(GatewayError::cache)?;

    if let Some(blob) = hit {
        return Ok(Json(serde_json::json!({
            "path": blob.path,
            "status": "ready",
        }))
        .into_response());
    }

    // The download's activity is owned by the reporter, which the blocking
    // task holds until the download ends: its drop ends the activity.
    let progress = Arc::new(ChannelProgress::new(state.hub.begin(&label), &label));
    let rx = progress.subscribe();
    tracing::info!(source = %source, "cache download started");
    let join = tokio::task::spawn_blocking(move || {
        let result = cache.download_to_cache(&source, expected.as_deref(), progress.as_ref());
        match &result {
            Ok(blob) => tracing::info!(path = %blob.path.display(), "cache download finished"),
            Err(error) => tracing::warn!(%error, "cache download failed"),
        }
        drop(progress);
        result
    });
    Ok(sse_response(rx, join))
}

/// `DELETE /v1/cache/{sha256}`: removes the blob and sidecar for a digest.
///
/// Answers 200 with `{"status": "deleted", "sha256"}` when an entry was
/// removed, 404 `cache_entry_not_found` when no sidecar records the digest,
/// and 400 when the path parameter is not a 64-character hex digest.
pub(crate) async fn delete_cache(
    State(state): State<AppState>,
    _caller: AuthedCaller,
    Path(sha256): Path<String>,
) -> Result<Json<serde_json::Value>, GatewayError> {
    let wanted = parse_expected_digest(&sha256)
        .map_err(|error| GatewayError::MalformedRequest(error.to_string()))?;
    let cache_dir = live_cache_dir(&state).await;
    let lookup = wanted.clone();
    let removed = blocking(move || open_cache(cache_dir.as_deref())?.remove(&lookup))
        .await?
        .map_err(GatewayError::cache)?;
    if !removed {
        return Err(GatewayError::CacheEntryNotFound(wanted));
    }
    Ok(Json(serde_json::json!({
        "status": "deleted",
        "sha256": wanted,
    })))
}

/// The byte counts of one cache download, as the SSE payload carries them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Sample {
    downloaded: u64,
    total: Option<u64>,
}

/// [`DownloadProgress`] for one cache download: publishes the raw byte
/// counts on a `watch` channel for the SSE response and formats
/// `"Downloading {name} {pct}%"` into the download's activity for the
/// status consumers. Owns the activity, so its drop ends it.
struct ChannelProgress {
    activity: Activity,
    text: PercentText,
    samples: watch::Sender<Sample>,
}

impl ChannelProgress {
    fn new(activity: Activity, name: &str) -> Self {
        let text = PercentText::new("Downloading", name);
        activity.set_text(text.label());
        let (samples, _rx) = watch::channel(Sample::default());
        Self {
            activity,
            text,
            samples,
        }
    }

    /// A receiver over the byte samples. `watch::Sender::subscribe` marks
    /// the current sample seen, so the SSE stream never opens with the
    /// zero-byte sample the channel was created with.
    fn subscribe(&self) -> watch::Receiver<Sample> {
        self.samples.subscribe()
    }

    /// The current `(downloaded, total)` counts, as the SSE payload reads
    /// them off the receiver.
    #[cfg(test)]
    fn sample(&self) -> (u64, Option<u64>) {
        let sample = *self.samples.borrow();
        (sample.downloaded, sample.total)
    }
}

impl DownloadProgress for ChannelProgress {
    fn set_len(&self, total: Option<u64>) {
        self.samples.send_modify(|sample| sample.total = total);
    }

    fn inc(&self, n: u64) {
        let mut published = Sample::default();
        self.samples.send_modify(|sample| {
            sample.downloaded = sample.downloaded.saturating_add(n);
            published = *sample;
        });
        if let Some(total) = published.total
            && total > 0
        {
            self.text
                .report(&self.activity, published.downloaded, total);
        }
    }
}

/// Builds the SSE response: each byte-count change re-emitted as the
/// `{"status": "downloading", ...}` event the route has always carried, then
/// the terminal event from the download task's join result, so the outcome
/// can never be lost.
///
/// The download task publishes every sample before it returns, so once the
/// join handle resolves the latest sample is already on the receiver and is
/// drained ahead of the terminal event. A client disconnect drops the
/// response body and the receiver; the blocking download itself runs to
/// completion (its staging cleanup still applies) and a later POST for the
/// same source then hits the cache.
fn sse_response(
    rx: watch::Receiver<Sample>,
    join: JoinHandle<Result<CachedBlob, LocalError>>,
) -> Response {
    let stream = futures_util::stream::unfold(
        (rx, join, std::collections::VecDeque::new(), false, None),
        |(mut rx, mut join, mut pending, mut done, mut emitted)| async move {
            loop {
                if let Some(line) = pending.pop_front() {
                    return Some((
                        Ok::<_, Infallible>(line),
                        (rx, join, pending, done, emitted),
                    ));
                }
                if done {
                    return None;
                }
                let result = tokio::select! {
                    changed = rx.changed() => match changed {
                        Ok(()) => {
                            let sample = *rx.borrow_and_update();
                            emitted = Some(sample);
                            return Some((
                                Ok(downloading_line(sample)),
                                (rx, join, pending, done, emitted),
                            ));
                        }
                        // The reporter dropped with the download task;
                        // the join result carries the outcome.
                        Err(_) => (&mut join).await,
                    },
                    result = &mut join => result,
                };
                // Samples coalesce, so only the latest one, when it was not
                // yet emitted, is worth sending ahead of the terminal event.
                let latest = *rx.borrow_and_update();
                if emitted != Some(latest) && latest != Sample::default() {
                    pending.push_back(downloading_line(latest));
                }
                done = true;
                pending.push_back(terminal_line(result));
            }
        },
    );
    let mut response = Response::new(Body::from_stream(stream));
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response
}

/// Maps a byte sample to the route's downloading event.
fn downloading_line(sample: Sample) -> String {
    let Sample {
        downloaded: bytes,
        total,
    } = sample;
    format!(
        "data: {}\n\n",
        serde_json::json!({
            "status": "downloading",
            "bytes": bytes,
            "total": total,
        })
    )
}

/// Maps the download task's join result to the stream's terminal event.
fn terminal_line(result: Result<Result<CachedBlob, LocalError>, tokio::task::JoinError>) -> String {
    let payload = match result {
        Ok(Ok(blob)) => serde_json::json!({
            "status": "ready",
            "path": blob.path,
        }),
        Ok(Err(error)) => serde_json::json!({
            "status": "error",
            "message": error.to_string(),
        }),
        Err(join_error) => serde_json::json!({
            "status": "error",
            "message": format!("download task failed: {join_error}"),
        }),
    };
    format!("data: {payload}\n\n")
}

#[cfg(test)]
mod tests {
    use futures_util::StreamExt as _;
    use gateway_progress::ProgressHub;

    use super::*;

    /// Serves `body` at `/model.bin` with an accurate Content-Length and
    /// returns its URL.
    async fn fake_file_server(body: &'static [u8]) -> String {
        let app = axum::Router::new().route(
            "/model.bin",
            axum::routing::get(move || async move {
                axum::response::Response::new(axum::body::Body::from(body))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("the fake server binds");
        let address = listener.local_addr().expect("the bound address");
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("the fake server serves");
        });
        format!("http://{address}/model.bin")
    }

    /// Collects a response body's full text.
    async fn body_text(response: Response) -> String {
        let mut frames = response.into_body().into_data_stream();
        let mut text = String::new();
        while let Some(frame) = frames.next().await {
            let frame = frame.expect("the stream errored");
            text.push_str(std::str::from_utf8(&frame).expect("SSE frames are UTF-8"));
        }
        text
    }

    #[tokio::test]
    async fn a_download_drives_the_hub_text_and_ends_its_activity_at_completion() {
        let body = b"activity-visibility-fixture";
        let url = fake_file_server(body).await;
        let temp = tempfile::TempDir::new().expect("tempdir");
        let root = temp.path().to_path_buf();
        let hub = Arc::new(ProgressHub::new());

        let progress = Arc::new(ChannelProgress::new(hub.begin("model.bin"), "model.bin"));
        assert_eq!(
            hub.current(),
            gateway_api_types::Progress {
                busy: true,
                text: "Downloading model.bin".to_owned(),
            },
            "the reporter names the download before any byte arrives"
        );

        // The blocking reqwest client inside `BlobCache` cannot be built or
        // dropped in async context, so the whole store lifecycle runs on the
        // blocking pool, as it does in the route.
        let reporter = Arc::clone(&progress);
        tokio::task::spawn_blocking(move || {
            let cache = BlobCache::new(root).expect("the cache opens");
            cache.download_to_cache(&url, None, reporter.as_ref())
        })
        .await
        .expect("the download task joins")
        .expect("the download succeeds");
        assert_eq!(
            hub.current().text,
            "Downloading model.bin 100%",
            "the last byte drives the text to its final whole percent"
        );
        assert_eq!(
            progress.sample(),
            (body.len() as u64, Some(body.len() as u64)),
            "the byte counts stay alongside the text for the SSE payload"
        );

        drop(progress);
        assert!(
            !hub.current().busy,
            "the reporter's drop ends the download's activity"
        );
    }

    #[tokio::test]
    async fn the_sse_stream_derives_from_byte_samples_and_ends_with_the_join_result() {
        let body = b"sse-sample-derived-fixture";
        let url = fake_file_server(body).await;
        let temp = tempfile::TempDir::new().expect("tempdir");
        let root = temp.path().to_path_buf();
        let hub = Arc::new(ProgressHub::new());

        let progress = Arc::new(ChannelProgress::new(hub.begin("model.bin"), "model.bin"));
        let rx = progress.subscribe();
        let join = tokio::task::spawn_blocking(move || {
            let cache = BlobCache::new(root).expect("the cache opens");
            let result = cache.download_to_cache(&url, None, progress.as_ref());
            drop(progress);
            result
        });

        let text = body_text(sse_response(rx, join)).await;
        let events: Vec<serde_json::Value> = text
            .split("\n\n")
            .filter(|block| !block.trim().is_empty())
            .map(|block| {
                let data = block.trim().strip_prefix("data: ").expect("a data line");
                serde_json::from_str(data).expect("a data line is JSON")
            })
            .collect();
        let (terminal, progress) = events.split_last().expect("the stream has events");
        assert_eq!(terminal["status"], "ready");
        let path = std::path::PathBuf::from(terminal["path"].as_str().expect("path"));
        assert_eq!(
            std::fs::read(&path).expect("read blob"),
            body,
            "the terminal event names the cached blob"
        );
        assert!(
            !progress.is_empty(),
            "progress events precede the terminal event: {text}"
        );
        let last = progress.last().expect("a progress event");
        assert_eq!(last["status"], "downloading");
        assert_eq!(
            last["bytes"],
            body.len() as u64,
            "the final sample carries every byte: {text}"
        );
        assert_eq!(last["total"], body.len() as u64);
        assert!(
            !hub.current().busy,
            "the finished download released its activity"
        );
    }

    #[tokio::test]
    async fn the_sse_stream_emits_the_latest_sample_when_the_reporter_dropped_before_the_first_poll()
     {
        // The download completes (and the reporter drops) before the
        // stream is polled at all: the watch's closed channel must not
        // hide the final byte sample, which precedes the terminal event.
        let body = b"sse-late-poll-fixture";
        let url = fake_file_server(body).await;
        let temp = tempfile::TempDir::new().expect("tempdir");
        let root = temp.path().to_path_buf();
        let hub = Arc::new(ProgressHub::new());

        let progress = Arc::new(ChannelProgress::new(hub.begin("model.bin"), "model.bin"));
        let rx = progress.subscribe();
        let result = tokio::task::spawn_blocking(move || {
            let cache = BlobCache::new(root).expect("the cache opens");
            let result = cache.download_to_cache(&url, None, progress.as_ref());
            drop(progress);
            result
        })
        .await
        .expect("the download task joins");
        let join = tokio::task::spawn_blocking(move || result);

        let text = body_text(sse_response(rx, join)).await;
        let blocks: Vec<&str> = text
            .split("\n\n")
            .filter(|block| !block.trim().is_empty())
            .collect();
        assert_eq!(
            blocks.len(),
            2,
            "one final sample, then the terminal: {text}"
        );
        assert!(
            blocks[0].contains("\"status\":\"downloading\"")
                && blocks[0].contains(&format!("\"bytes\":{}", body.len())),
            "the latest sample is emitted even though the channel closed: {text}"
        );
        assert!(
            blocks[1].contains("\"status\":\"ready\""),
            "the stream still ends with the join result's terminal event: {text}"
        );
    }
}
