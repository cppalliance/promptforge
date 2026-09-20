//! The cloud provider model sheet cache and its admin routes.
//!
//! The published provider sheet (the gateway-api-types [`Sheet`]) is a
//! release artifact of the promptforge-cloud-providers repository. At
//! launch, after the async boot completes and off the serving path, the
//! gateway loads `<profile>/cloud-provider-models.json` from disk when
//! present, parseable, and of an accepted schema version (an
//! unparseable or version-mismatched cache is logged, treated as
//! absent, and overwritten by the next successful download), and spawns
//! one bounded background download when the cache is missing,
//! unusable, or its envelope `generated_at` is older than one week.
//! The age check is a launch-time timestamp comparison, not a timer
//! loop: a gateway that runs for weeks re-checks at its next launch.
//!
//! A successful download lands on disk by temp-file-plus-rename before
//! the in-memory copy swaps, so a crash mid-write never leaves a
//! truncated cache and a failed download keeps the old one. `GET
//! /admin/cloud-models` serves the in-memory sheet, a 503 loading
//! indication while none has arrived, or the last download error; `POST
//! /admin/cloud-models/refresh` forces a re-download regardless of age
//! and answers with the fresh sheet once the download it started or
//! joined lands, or with the download's error. Both routes sit behind
//! the shared loopback wall with the rest of the admin config surface.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use axum::Json;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use gateway_api_types::{ACCEPTED_SHEET_SCHEMA_VERSION, Sheet};
use gateway_protocol::http_util::{MAX_JSON_BODY, bounded_client, read_bytes_capped};
use time::OffsetDateTime;

use crate::AppState;
use crate::auth::AuthedCaller;
use crate::error::{GatewayError, blocking};

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

#[derive(Debug)]
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
    /// The generation of the latest download, bumped per spawn so a
    /// refresh awaits exactly the download it started or joined.
    generation: u64,
    /// The latest completed download's generation and outcome; the
    /// refresh route's answer.
    last_outcome: Option<(u64, Result<(), String>)>,
    /// Announces each completed download's generation to awaiting
    /// refreshes.
    completion: tokio::sync::watch::Sender<u64>,
}

impl Default for Inner {
    fn default() -> Self {
        Self {
            cache_path: None,
            url: None,
            sheet: None,
            last_error: None,
            download_in_flight: false,
            generation: 0,
            last_outcome: None,
            completion: tokio::sync::watch::channel(0).0,
        }
    }
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
    /// A cache file parsed but declared a schema version this gateway
    /// does not accept; treated as absent like [`CacheRead::Unparseable`]
    /// and overwritten by the next successful download.
    UnsupportedVersion(u32),
    /// A parseable sheet of an accepted schema version; freshness is the
    /// caller's decision.
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
            Ok(CacheRead::UnsupportedVersion(found)) => {
                tracing::warn!(
                    path = %cache_path.display(),
                    found,
                    accepted = ACCEPTED_SHEET_SCHEMA_VERSION,
                    "cloud provider sheet cache schema version is not accepted; treating it as absent"
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
    /// Forces a re-download regardless of cache age and awaits its
    /// outcome: the fresh sheet on success, the download's error on
    /// failure. A refresh asked during an in-flight download joins it
    /// and awaits the same result instead of starting a second one.
    pub(crate) async fn refresh(&self) -> Result<Arc<Sheet>, GatewayError> {
        let (generation, mut completion) = {
            let mut inner = self.lock();
            match self.spawn_download_locked(&mut inner) {
                Download::Started(_) | Download::InFlight => {
                    (inner.generation, inner.completion.subscribe())
                }
                Download::Unavailable => return Err(GatewayError::CloudModelsLoading),
            }
        };
        let waited = completion.wait_for(|done| *done >= generation).await;
        let inner = self.lock();
        match (waited, &inner.last_outcome) {
            (Ok(_), Some((done, Ok(())))) if *done >= generation => match &inner.sheet {
                Some(sheet) => Ok(Arc::clone(sheet)),
                None => Err(GatewayError::CloudModelsUnavailable(
                    "cloud provider sheet download finished without installing a sheet".to_owned(),
                )),
            },
            (Ok(_), Some((done, Err(error)))) if *done >= generation => {
                Err(GatewayError::CloudModelsUnavailable(error.clone()))
            }
            _ => Err(GatewayError::CloudModelsUnavailable(
                "cloud provider sheet download outcome was lost".to_owned(),
            )),
        }
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

    /// Spawns the one background download, or reports why none started.
    fn spawn_download(&self) -> Download {
        let mut inner = self.lock();
        self.spawn_download_locked(&mut inner)
    }

    /// The spawn half of [`CloudModels::spawn_download`] with the lock
    /// already held, so a refresh spawns or joins and subscribes to the
    /// completion signal atomically.
    fn spawn_download_locked(&self, inner: &mut Inner) -> Download {
        if inner.download_in_flight {
            return Download::InFlight;
        }
        let (Some(cache_path), Some(url)) = (inner.cache_path.clone(), inner.url.clone()) else {
            return Download::Unavailable;
        };
        inner.download_in_flight = true;
        inner.generation += 1;
        let generation = inner.generation;
        let this = self.clone();
        Download::Started(tokio::spawn(async move {
            let outcome = download_once(&cache_path, &url).await;
            let mut inner = this.lock();
            let result = match outcome {
                Ok(sheet) => {
                    inner.sheet = Some(Arc::new(sheet));
                    inner.last_error = None;
                    Ok(())
                }
                Err(error) => {
                    tracing::warn!("{error}");
                    let message = error.to_string();
                    inner.last_error = Some(message.clone());
                    Err(message)
                }
            };
            inner.download_in_flight = false;
            inner.last_outcome = Some((generation, result));
            let _ignored = inner.completion.send(generation);
        }))
    }
}

/// Fetches the sheet and persists it, returning the sheet only after the
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
    if sheet.schema_version != ACCEPTED_SHEET_SCHEMA_VERSION {
        return Err(GatewayError::CloudModelsSchemaVersion {
            found: sheet.schema_version,
            accepted: ACCEPTED_SHEET_SCHEMA_VERSION,
        });
    }
    let bytes = serde_json::to_vec(&sheet).map_err(|error| {
        GatewayError::CloudModelsUnavailable(format!(
            "cloud provider sheet serialization failed: {error}"
        ))
    })?;
    let display = cache_path.display().to_string();
    let path = cache_path.to_path_buf();
    blocking(move || write_cache_atomic(&path, &bytes))
        .await?
        .map_err(|error| {
            GatewayError::CloudModelsUnavailable(format!(
                "cloud provider sheet cache write to {display} failed: {error}"
            ))
        })?;
    Ok(sheet)
}

/// Reads and parses the cache file, gating on the accepted schema version.
fn read_cache(path: &Path) -> CacheRead {
    match std::fs::read(path) {
        Ok(bytes) => match serde_json::from_slice::<Sheet>(&bytes) {
            Ok(sheet) if sheet.schema_version == ACCEPTED_SHEET_SCHEMA_VERSION => {
                CacheRead::Loaded(sheet)
            }
            Ok(sheet) => CacheRead::UnsupportedVersion(sheet.schema_version),
            Err(_) => CacheRead::Unparseable,
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => CacheRead::Missing,
        Err(_) => CacheRead::Unparseable,
    }
}

/// Writes `bytes` to `path` by temp-file-plus-rename, so a crash mid-write
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
    _caller: AuthedCaller,
) -> Result<Response, GatewayError> {
    if let Some(sheet) = state.cloud_models.sheet() {
        return Ok(Json(Sheet::clone(&sheet)).into_response());
    }
    Err(match state.cloud_models.last_error() {
        Some(error) => GatewayError::CloudModelsUnavailable(error),
        None => GatewayError::CloudModelsLoading,
    })
}

/// The `POST /admin/cloud-models/refresh` route: bearer-authed and
/// loopback-walled, forces a re-download regardless of cache age and
/// answers 200 with the fresh sheet once the download it started or
/// joined lands, or the download's 502 error; a concurrent refresh
/// awaits the same download rather than starting a second one.
pub(crate) async fn admin_cloud_models_refresh(
    State(state): State<AppState>,
    _caller: AuthedCaller,
) -> Result<Json<Sheet>, GatewayError> {
    let sheet = state.cloud_models.refresh().await?;
    Ok(Json(Sheet::clone(&sheet)))
}

#[cfg(test)]
mod tests;
