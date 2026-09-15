//! The cloud provider model sheet cache and its admin routes.
//!
//! The published provider sheet (the shared-gateway-api [`Sheet`]) is a
//! release artifact of the promptforge-cloud-providers repository. At
//! launch, after the async boot completes and off the serving path, the
//! gateway loads `<profile>/cloud-provider-models.json` from disk when
//! present and parseable (an unparseable cache is logged, treated as
//! absent, and overwritten by the next successful download), and spawns
//! one bounded background download when the cache is missing,
//! unparseable, or its envelope `generated_at` is older than one week.
//! The age check is a launch-time timestamp comparison, not a timer
//! loop: a gateway that runs for weeks re-checks at its next launch.
//!
//! A successful download lands on disk by temp-file-plus-rename before
//! the in-memory copy swaps, so a crash mid-write never leaves a
//! truncated cache and a failed download keeps the old one. `GET
//! /admin/cloud-models` serves the in-memory sheet, a 503 loading
//! indication while none has arrived, or the last download error; `POST
//! /admin/cloud-models/refresh` forces a background re-download
//! regardless of age. Both routes sit behind the shared loopback wall
//! with the rest of the admin config surface.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use gateway_protocol::http_util::{MAX_JSON_BODY, bounded_client, read_bytes_capped};
use shared_gateway_api::Sheet;
use time::OffsetDateTime;

use crate::auth::Caller;
use crate::error::GatewayError;
use crate::{AppState, check_auth};

/// The release artifact the sheet downloads from.
pub(crate) const DEFAULT_SHEET_URL: &str = "https://github.com/cppalliance/promptforge-cloud-providers/releases/download/models/cloud-provider-models.json";

/// The environment override for the sheet URL, matching the repo's
/// `PROMPTFORGE_*` convention; there is no config-schema knob.
pub(crate) const SHEET_URL_ENV: &str = "PROMPTFORGE_MODELS_SHEET_URL";

/// The cache file name inside the profile directory.
pub(crate) const CACHE_FILE_NAME: &str = "cloud-provider-models.json";

/// The cache age past which a launch re-downloads: one week, compared
/// against the envelope's `generated_at`, which survives file copies and
/// so needs no sidecar metadata.
const MAX_CACHE_AGE: time::Duration = time::Duration::days(7);

/// The process-lifetime cloud sheet slot behind `GET
/// /admin/cloud-models`: the in-memory sheet, the last download error,
/// and the one-download-at-a-time guard, all behind one lock.
#[derive(Debug, Clone, Default)]
pub(crate) struct CloudModels {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Debug, Default)]
struct Inner {
    /// The launched cache location; `None` until [`CloudModels::launch`].
    cache_path: Option<PathBuf>,
    /// The download URL resolved at launch (env override or default).
    url: Option<String>,
    /// The in-memory sheet, swapped only after a successful cache write.
    sheet: Option<Arc<Sheet>>,
    /// The last download or cache-write failure, cleared by the next
    /// success; the route's error answer when no sheet has arrived.
    last_error: Option<String>,
    /// One download at a time, across launch and every refresh.
    download_in_flight: bool,
}

/// The outcome of asking for a background download.
#[derive(Debug)]
pub(crate) enum Download {
    /// A new download task; the handle is the test rendezvous.
    Started(tokio::task::JoinHandle<()>),
    /// A download is already running; it is never duplicated.
    InFlight,
    /// The module was never launched, so there is nowhere to cache to.
    Unavailable,
}

impl Download {
    /// The task handle when this call spawned the download.
    fn started(self) -> Option<tokio::task::JoinHandle<()>> {
        match self {
            Download::Started(handle) => Some(handle),
            Download::InFlight | Download::Unavailable => None,
        }
    }
}

/// What reading the cache file found.
#[derive(Debug)]
enum CacheRead {
    /// No cache file.
    Missing,
    /// A cache file existed but would not read or parse; treated as
    /// absent and overwritten by the next successful download.
    Unparseable,
    /// A parseable sheet; freshness is the caller's decision.
    Loaded(Sheet),
}

impl CloudModels {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The launch sequence: install the cache path and URL, load a
    /// parseable cache into memory, and spawn the background download
    /// when the cache is missing, unparseable, or older than a week.
    ///
    /// Returns the download task's handle, or `None` when a fresh cache
    /// made the download unnecessary; the handle is the test rendezvous.
    pub(crate) async fn launch(
        &self,
        cache_path: PathBuf,
        url: String,
    ) -> Option<tokio::task::JoinHandle<()>> {
        {
            let mut inner = self.lock();
            inner.cache_path = Some(cache_path.clone());
            inner.url = Some(url);
        }
        let read = tokio::task::spawn_blocking({
            let cache_path = cache_path.clone();
            move || read_cache(&cache_path)
        })
        .await;
        let fresh = match read {
            Ok(CacheRead::Loaded(sheet)) => {
                let fresh = OffsetDateTime::now_utc() - sheet.generated_at <= MAX_CACHE_AGE;
                self.lock().sheet = Some(Arc::new(sheet));
                fresh
            }
            Ok(CacheRead::Missing) => false,
            Ok(CacheRead::Unparseable) => {
                tracing::warn!(
                    path = %cache_path.display(),
                    "cloud provider sheet cache is unparseable; treating it as absent"
                );
                false
            }
            Err(join) => {
                tracing::warn!(
                    path = %cache_path.display(),
                    "cloud provider sheet cache read failed: {join}"
                );
                false
            }
        };
        if fresh {
            None
        } else {
            self.spawn_download().started()
        }
    }
    /// Force a background re-download regardless of cache age.
    pub(crate) fn refresh(&self) -> Download {
        self.spawn_download()
    }

    /// The in-memory sheet, when one has loaded or downloaded.
    pub(crate) fn sheet(&self) -> Option<Arc<Sheet>> {
        self.lock().sheet.clone()
    }

    /// The last download or cache-write error, cleared by the next
    /// success.
    pub(crate) fn last_error(&self) -> Option<String> {
        self.lock().last_error.clone()
    }

    /// Spawn the one background download, or report why none started.
    fn spawn_download(&self) -> Download {
        let (cache_path, url) = {
            let mut inner = self.lock();
            if inner.download_in_flight {
                return Download::InFlight;
            }
            let (Some(cache_path), Some(url)) = (inner.cache_path.clone(), inner.url.clone())
            else {
                return Download::Unavailable;
            };
            inner.download_in_flight = true;
            (cache_path, url)
        };
        let this = self.clone();
        Download::Started(tokio::spawn(async move {
            let outcome = download_once(&cache_path, &url).await;
            let mut inner = this.lock();
            match outcome {
                Ok(sheet) => {
                    inner.sheet = Some(Arc::new(sheet));
                    inner.last_error = None;
                }
                Err(error) => {
                    tracing::warn!("{error}");
                    inner.last_error = Some(error.to_string());
                }
            }
            inner.download_in_flight = false;
        }))
    }
}

/// Fetch the sheet and persist it, returning the sheet only after the
/// cache write lands: the in-memory copy never runs ahead of the disk.
///
/// The body read is capped at [`MAX_JSON_BODY`] like every other gateway
/// outbound read; a response announcing an over-cap `Content-Length` is
/// refused before the read so the failure names the cap rather than a
/// truncated parse.
async fn download_once(cache_path: &Path, url: &str) -> Result<Sheet, GatewayError> {
    let response = bounded_client()
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| {
            GatewayError::CloudModelsUnavailable(format!(
                "cloud provider sheet download from {url} failed: {error}"
            ))
        })?;
    if let Some(announced) = response.content_length()
        && announced > MAX_JSON_BODY as u64
    {
        return Err(GatewayError::CloudModelsBodyTooLarge {
            announced,
            cap: MAX_JSON_BODY,
        });
    }
    let bytes = read_bytes_capped(response, MAX_JSON_BODY)
        .await
        .map_err(|error| {
            GatewayError::CloudModelsUnavailable(format!(
                "cloud provider sheet download from {url} failed: {error}"
            ))
        })?;
    let sheet: Sheet = serde_json::from_slice(&bytes).map_err(|error| {
        GatewayError::CloudModelsUnavailable(format!(
            "cloud provider sheet from {url} failed to parse: {error}"
        ))
    })?;
    let bytes = serde_json::to_vec(&sheet).map_err(|error| {
        GatewayError::CloudModelsUnavailable(format!(
            "cloud provider sheet serialization failed: {error}"
        ))
    })?;
    let display = cache_path.display().to_string();
    let path = cache_path.to_path_buf();
    tokio::task::spawn_blocking(move || write_cache_atomic(&path, &bytes))
        .await
        .map_err(|join| {
            GatewayError::CloudModelsUnavailable(format!(
                "cloud provider sheet cache write to {display} failed: {join}"
            ))
        })?
        .map_err(|error| {
            GatewayError::CloudModelsUnavailable(format!(
                "cloud provider sheet cache write to {display} failed: {error}"
            ))
        })?;
    Ok(sheet)
}

/// Read and parse the cache file.
fn read_cache(path: &Path) -> CacheRead {
    match std::fs::read(path) {
        Ok(bytes) => match serde_json::from_slice::<Sheet>(&bytes) {
            Ok(sheet) => CacheRead::Loaded(sheet),
            Err(_) => CacheRead::Unparseable,
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => CacheRead::Missing,
        Err(_) => CacheRead::Unparseable,
    }
}

/// Write `bytes` to `path` by temp-file-plus-rename, so a crash mid-write
/// never leaves a truncated cache behind.
fn write_cache_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // The temp file is a sibling, so the rename stays on one filesystem
    // and the replace is atomic.
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, bytes)?;
    std::fs::rename(&temp, path)
}

/// The `GET /admin/cloud-models` route: bearer-authed and loopback-walled,
/// serves the in-memory sheet, a 503 loading indication while none has
/// arrived, or the last download error.
pub(crate) async fn admin_cloud_models(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Response, GatewayError> {
    check_auth(&state, &caller).await?;
    if let Some(sheet) = state.cloud_models.sheet() {
        return Ok(Json(Sheet::clone(&sheet)).into_response());
    }
    Err(match state.cloud_models.last_error() {
        Some(error) => GatewayError::CloudModelsUnavailable(error),
        None => GatewayError::CloudModelsLoading,
    })
}

/// The `POST /admin/cloud-models/refresh` route: bearer-authed and
/// loopback-walled, forces a background re-download regardless of cache
/// age; 202 whether this call spawned the download or one was already
/// running.
pub(crate) async fn admin_cloud_models_refresh(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<(StatusCode, Json<serde_json::Value>), GatewayError> {
    check_auth(&state, &caller).await?;
    match state.cloud_models.refresh() {
        Download::Started(_) | Download::InFlight => Ok((
            StatusCode::ACCEPTED,
            Json(serde_json::json!({ "status": "downloading" })),
        )),
        Download::Unavailable => Err(GatewayError::CloudModelsLoading),
    }
}

#[cfg(test)]
mod tests;
