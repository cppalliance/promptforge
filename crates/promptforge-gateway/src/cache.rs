//! The `/v1/cache` routes: bearer-authenticated on-demand blob downloads into
//! the operator cache, with sidecar-based listing and removal.
//!
//! The store is blocking filesystem plus a reqwest-blocking client, so every
//! store operation runs inside `tokio::task::spawn_blocking` and never blocks
//! the executor (Amendment D). A download reports progress over a bounded
//! channel that the SSE response drains; intermediate samples drop under
//! backpressure, while the terminal ready/error event is produced from the
//! download task's join result and is therefore never lost.

use std::convert::Infallible;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use axum::Json;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use futures_util::StreamExt as _;
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::error::GatewayError;
use crate::local::artifacts::{DownloadProgress, filename_from_url, parse_expected_digest};
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
/// usable filename segment. Anything else is a 400, never a download attempt.
fn validate_source(source: &str) -> Result<(), GatewayError> {
    let parsed = url::Url::parse(source).map_err(|_| {
        GatewayError::MalformedRequest(format!("cache source `{source}` is not a valid URL"))
    })?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(GatewayError::MalformedRequest(format!(
            "cache source `{source}` must be an http or https URL with a host"
        )));
    }
    filename_from_url(source).map_err(|error| GatewayError::MalformedRequest(error.to_string()))?;
    Ok(())
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
    validate_source(&request.source)?;
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

    let (tx, rx) = mpsc::channel::<CacheProgress>(64);
    let join = tokio::task::spawn_blocking(move || {
        let progress = ChannelProgress::new(tx);
        cache.download_to_cache(&source, expected.as_deref(), &progress)
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

/// One progress sample from a running download.
#[derive(Debug, Clone, Copy)]
struct CacheProgress {
    bytes: u64,
    total: Option<u64>,
}

/// [`DownloadProgress`] over a bounded channel toward the SSE response.
///
/// Intermediate samples are sent with `try_send` and dropped when the client
/// is not keeping up - progress is lossy by nature. The terminal event is not
/// a sample: it comes from the download task's join result, so backpressure
/// can never drop the ready/error outcome.
struct ChannelProgress {
    tx: mpsc::Sender<CacheProgress>,
    downloaded: AtomicU64,
    total: Mutex<Option<u64>>,
}

impl ChannelProgress {
    fn new(tx: mpsc::Sender<CacheProgress>) -> Self {
        Self {
            tx,
            downloaded: AtomicU64::new(0),
            total: Mutex::new(None),
        }
    }

    fn total(&self) -> Option<u64> {
        // The guarded value is plain data with no panic path; a poisoned lock
        // (only possible if a panic landed mid-store) recovers the value.
        *self.total.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn publish(&self, bytes: u64) {
        let _ = self.tx.try_send(CacheProgress {
            bytes,
            total: self.total(),
        });
    }
}

impl DownloadProgress for ChannelProgress {
    fn set_len(&self, total: Option<u64>) {
        *self.total.lock().unwrap_or_else(PoisonError::into_inner) = total;
        self.publish(self.downloaded.load(Ordering::Relaxed));
    }

    fn inc(&self, n: u64) {
        let downloaded = self.downloaded.fetch_add(n, Ordering::Relaxed) + n;
        self.publish(downloaded);
    }

    fn finish(&self) {}

    fn abandon(&self) {}
}

/// Builds the SSE response draining the progress channel, then appending the
/// terminal event from the download task's join result.
///
/// The channel closes when the download task drops its `ChannelProgress`, so
/// the progress stream ends before the terminal event is awaited. A client
/// disconnect drops the response body and the receiver; the blocking download
/// itself runs to completion (its staging cleanup still applies) and a later
/// POST for the same source then hits the cache.
fn sse_response(
    mut rx: mpsc::Receiver<CacheProgress>,
    join: JoinHandle<Result<CachedBlob, LocalError>>,
) -> Response {
    let progress = futures_util::stream::poll_fn(move |cx| rx.poll_recv(cx)).map(|sample| {
        Ok::<_, Infallible>(format!(
            "data: {}\n\n",
            serde_json::json!({
                "status": "downloading",
                "bytes": sample.bytes,
                "total": sample.total,
            })
        ))
    });
    let terminal = futures_util::stream::once(async move {
        let payload = match join.await {
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
        Ok::<_, Infallible>(format!("data: {payload}\n\n"))
    });
    let mut response = Response::new(Body::from_stream(progress.chain(terminal)));
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response
}
