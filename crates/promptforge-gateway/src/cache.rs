//! The `/v1/cache` routes: bearer-authenticated on-demand blob downloads into
//! the operator cache, with sidecar-based listing and removal.
//!
//! The store is blocking filesystem plus a reqwest-blocking client, so every
//! store operation runs inside `tokio::task::spawn_blocking` and never blocks
//! the executor (Amendment D). Each download attaches a small operation tree
//! to the process progress hub and reports bytes into its leaf; the SSE
//! response is a filtered view of the hub's events for that operation, with
//! intermediate samples lossy under backpressure, while the terminal
//! ready/error event is produced from the download task's join result and is
//! therefore never lost.

use std::convert::Infallible;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use axum::Json;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use tokio::task::JoinHandle;

use promptforge_progress::{EventState, OperationId, ProgressEvent, ProgressHandle};

use crate::error::GatewayError;
use crate::local::artifacts::{
    DownloadProgress, TreeProgress, filename_from_url, parse_expected_digest,
};
use crate::local::cache::{BlobCache, CacheEntry, CachedBlob};
use crate::local::{LocalError, resolve_cache_root};
use crate::{AppState, check_auth};

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
    headers: HeaderMap,
) -> Result<Json<Vec<CacheEntry>>, GatewayError> {
    check_auth(&state, &headers).await?;
    let cache_dir = live_cache_dir(&state).await;
    let entries = tokio::task::spawn_blocking(move || open_cache(cache_dir.as_deref())?.list())
        .await
        .map_err(GatewayError::cache)?
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
    let parsed = url::Url::parse(source).map_err(|_| {
        GatewayError::MalformedRequest(format!("cache source `{source}` is not a valid URL"))
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
    headers: HeaderMap,
    Json(request): Json<CacheRequest>,
) -> Result<Response, GatewayError> {
    check_auth(&state, &headers).await?;
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
    let (cache, hit) = tokio::task::spawn_blocking(move || {
        let cache = open_cache(cache_dir.as_deref())?;
        let hit = cache.lookup(&lookup_source, lookup_pin.as_deref())?;
        Ok::<_, LocalError>((cache, hit))
    })
    .await
    .map_err(GatewayError::cache)?
    .map_err(GatewayError::cache)?;

    if let Some(blob) = hit {
        return Ok(Json(serde_json::json!({
            "path": blob.path,
            "status": "ready",
        }))
        .into_response());
    }

    // Subscribe before the tree attaches so no event of this download is
    // missed; the response filters the hub stream to this download's
    // operation.
    let rx = state.hub.subscribe();
    let tree = state.hub.operation();
    let operation = tree.operation();
    let progress = Arc::new(ChannelProgress::new(tree.register(&label, 1.0)));
    let reporter = Arc::clone(&progress);
    let join = tokio::task::spawn_blocking(move || {
        let result = cache.download_to_cache(&source, expected.as_deref(), reporter.as_ref());
        // The tree's Drop detaches it from the hub: the download's operation
        // leaves snapshots and the event stream when the download ends.
        drop(tree);
        result
    });
    Ok(sse_response(rx, operation, progress, join))
}

/// `DELETE /v1/cache/{sha256}`: removes the blob and sidecar for a digest.
///
/// Answers 200 with `{"status": "deleted", "sha256"}` when an entry was
/// removed, 404 `cache_entry_not_found` when no sidecar records the digest,
/// and 400 when the path parameter is not a 64-character hex digest.
pub(crate) async fn delete_cache(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(sha256): Path<String>,
) -> Result<Json<serde_json::Value>, GatewayError> {
    check_auth(&state, &headers).await?;
    let wanted = parse_expected_digest(&sha256)
        .map_err(|error| GatewayError::MalformedRequest(error.to_string()))?;
    let cache_dir = live_cache_dir(&state).await;
    let lookup = wanted.clone();
    let removed =
        tokio::task::spawn_blocking(move || open_cache(cache_dir.as_deref())?.remove(&lookup))
            .await
            .map_err(GatewayError::cache)?
            .map_err(GatewayError::cache)?;
    if !removed {
        return Err(GatewayError::CacheEntryNotFound(wanted));
    }
    Ok(Json(serde_json::json!({
        "status": "deleted",
        "sha256": wanted,
    })))
}

/// [`DownloadProgress`] for one cache download: reports byte counts into the
/// download's tree leaf and keeps the raw counts alongside, because the
/// tree's events carry fractions while the SSE payload carries bytes.
struct ChannelProgress {
    leaf: TreeProgress,
    downloaded: AtomicU64,
    total: Mutex<Option<u64>>,
}

impl ChannelProgress {
    fn new(handle: ProgressHandle) -> Self {
        Self {
            leaf: TreeProgress::new(handle),
            downloaded: AtomicU64::new(0),
            total: Mutex::new(None),
        }
    }

    /// The current `(downloaded, total)` counts for the SSE payload.
    fn sample(&self) -> (u64, Option<u64>) {
        // The guarded value is plain data with no panic path; a poisoned lock
        // (only possible if a panic landed mid-store) recovers the value.
        (
            self.downloaded.load(Ordering::Relaxed),
            *self.total.lock().unwrap_or_else(PoisonError::into_inner),
        )
    }
}

impl DownloadProgress for ChannelProgress {
    fn set_len(&self, total: Option<u64>) {
        *self.total.lock().unwrap_or_else(PoisonError::into_inner) = total;
        self.leaf.set_len(total);
    }

    fn inc(&self, n: u64) {
        self.downloaded.fetch_add(n, Ordering::Relaxed);
        self.leaf.inc(n);
    }

    fn finish(&self) {
        self.leaf.finish();
    }

    fn abandon(&self) {
        self.leaf.abandon();
    }
}

/// Builds the SSE response: the hub's event stream filtered to this
/// download's operation, each `Updated` re-emitted as the
/// `{"status": "downloading", ...}` event the route has always carried, then
/// the terminal event from the download task's join result, so the outcome
/// can never be lost to broadcast lag.
///
/// The download task emits every event before it returns, so once the join
/// handle resolves the remaining events are already queued on the receiver
/// and the latest sample is drained ahead of the terminal event. A client
/// disconnect drops the response body and the receiver; the blocking download
/// itself runs to
/// completion (its staging cleanup still applies) and a later POST for the
/// same source then hits the cache.
fn sse_response(
    rx: tokio::sync::broadcast::Receiver<ProgressEvent>,
    operation: OperationId,
    progress: Arc<ChannelProgress>,
    join: JoinHandle<Result<CachedBlob, LocalError>>,
) -> Response {
    let stream = futures_util::stream::unfold(
        (rx, join, std::collections::VecDeque::new(), false),
        move |(mut rx, mut join, mut pending, mut done)| {
            let progress = Arc::clone(&progress);
            async move {
                loop {
                    if let Some(line) = pending.pop_front() {
                        return Some((Ok::<_, Infallible>(line), (rx, join, pending, done)));
                    }
                    if done {
                        return None;
                    }
                    let result = loop {
                        tokio::select! {
                            received = rx.recv() => match received {
                                Ok(event) => {
                                    if event.operation == operation
                                        && matches!(event.state, EventState::Updated { .. })
                                    {
                                        return Some((
                                            Ok(downloading_line(&progress)),
                                            (rx, join, pending, done),
                                        ));
                                    }
                                }
                                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                                    tracing::debug!(skipped, "cache progress subscriber lagged; events dropped");
                                }
                                // The hub lives in `AppState` for the process
                                // lifetime, so its sender never closes first; the
                                // join result still carries the outcome if it did.
                                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                    break (&mut join).await;
                                }
                            },
                            result = &mut join => break result,
                        }
                    };
                    // Samples are lossy, so only the latest queued one is
                    // worth emitting ahead of the terminal event.
                    let mut updated = false;
                    while let Ok(event) = rx.try_recv() {
                        if event.operation == operation
                            && matches!(event.state, EventState::Updated { .. })
                        {
                            updated = true;
                        }
                    }
                    if updated {
                        pending.push_back(downloading_line(&progress));
                    }
                    done = true;
                    pending.push_back(terminal_line(result));
                }
            }
        },
    );
    let mut response = Response::new(Body::from_stream(stream));
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response
}

/// Maps the reporter's counters to the route's downloading event.
fn downloading_line(progress: &ChannelProgress) -> String {
    let (bytes, total) = progress.sample();
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
    // Fractions are fixed-point millionths, so equality comparisons are exact.
    #![expect(clippy::float_cmp, reason = "fixed-point fractions compare exactly")]

    use futures_util::StreamExt as _;
    use promptforge_progress::ProgressHub;

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
    async fn a_downloads_tree_appears_in_hub_snapshots_and_detaches_at_completion() {
        let body = b"tree-visibility-fixture";
        let url = fake_file_server(body).await;
        let temp = tempfile::TempDir::new().expect("tempdir");
        let root = temp.path().to_path_buf();
        let hub = Arc::new(ProgressHub::new());

        let tree = hub.operation();
        let progress = Arc::new(ChannelProgress::new(tree.register("model.bin", 1.0)));
        let snapshot = hub.snapshot();
        assert_eq!(snapshot.len(), 1, "the download's tree is attached");
        assert_eq!(snapshot[0].nodes[0].label, "model.bin");
        assert_eq!(snapshot[0].nodes[0].fraction, 0.0);

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
        let snapshot = hub.snapshot();
        assert_eq!(
            snapshot[0].nodes[0].fraction, 1.0,
            "finish drives the leaf to 1.0"
        );
        assert_eq!(
            progress.sample(),
            (body.len() as u64, Some(body.len() as u64))
        );

        drop(progress);
        drop(tree);
        assert!(
            hub.snapshot().is_empty(),
            "the tree detaches from the hub at completion"
        );
    }

    #[tokio::test]
    async fn the_sse_stream_derives_from_tree_events_and_ends_with_the_join_result() {
        let body = b"sse-tree-derived-fixture";
        let url = fake_file_server(body).await;
        let temp = tempfile::TempDir::new().expect("tempdir");
        let root = temp.path().to_path_buf();
        let hub = Arc::new(ProgressHub::new());

        let rx = hub.subscribe();
        let tree = hub.operation();
        let operation = tree.operation();
        let progress = Arc::new(ChannelProgress::new(tree.register("model.bin", 1.0)));
        let reporter = Arc::clone(&progress);
        let join = tokio::task::spawn_blocking(move || {
            let cache = BlobCache::new(root).expect("the cache opens");
            let result = cache.download_to_cache(&url, None, reporter.as_ref());
            drop(tree);
            result
        });

        let text = body_text(sse_response(rx, operation, progress, join)).await;
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
    }
}
