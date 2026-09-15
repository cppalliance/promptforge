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
                    inner.last_error = Some(error);
                }
            }
            inner.download_in_flight = false;
        }))
    }
}

/// Fetch the sheet and persist it, returning the sheet only after the
/// cache write lands: the in-memory copy never runs ahead of the disk.
async fn download_once(cache_path: &Path, url: &str) -> Result<Sheet, String> {
    let sheet =
        shared_cloud_providers::fetch_sheet(&gateway_protocol::http_util::bounded_client(), url)
            .await
            .map_err(|error| format!("cloud provider sheet download from {url} failed: {error}"))?;
    let bytes = serde_json::to_vec(&sheet)
        .map_err(|error| format!("cloud provider sheet serialization failed: {error}"))?;
    let display = cache_path.display().to_string();
    let path = cache_path.to_path_buf();
    tokio::task::spawn_blocking(move || write_cache_atomic(&path, &bytes))
        .await
        .map_err(|join| format!("cloud provider sheet cache write to {display} failed: {join}"))?
        .map_err(|error| {
            format!("cloud provider sheet cache write to {display} failed: {error}")
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
mod tests {
    use std::collections::BTreeMap;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::body::Body;
    use axum::extract::ConnectInfo;
    use axum::http::header::AUTHORIZATION;
    use axum::http::{Method, Request};
    use gateway_config::Config;
    use shared_gateway_api::{ModelEntry, ModelKind, ProviderSlice, SliceStatus, Thinking, Tier};
    use tokio::sync::Notify;
    use tower::ServiceExt as _;

    use super::*;

    /// A one-provider sheet stamped `generated_at`, carrying one model
    /// whose id distinguishes one test sheet from another.
    fn test_sheet(generated_at: OffsetDateTime, model_id: &str) -> Sheet {
        Sheet {
            schema_version: 1,
            generated_at,
            providers: BTreeMap::from([(
                "test".to_owned(),
                ProviderSlice {
                    display_name: "Test".to_owned(),
                    tier: Tier::Prime,
                    status: SliceStatus::Static,
                    fetched_at: None,
                    openai_base_url: Some("https://api.test.example/v1".to_owned()),
                    env_vars: vec![],
                    models: vec![ModelEntry {
                        id: model_id.to_owned(),
                        display_name: model_id.to_owned(),
                        family: "test-family".to_owned(),
                        variant_of: None,
                        variant: None,
                        languages: vec![],
                        kind: ModelKind::Chat,
                        released_at: None,
                        context_window: None,
                        max_output: None,
                        images: false,
                        pdf_input: false,
                        video_input: false,
                        audio_input: false,
                        batch: false,
                        citations: false,
                        code_execution: false,
                        structured_outputs: false,
                        tool_calling: false,
                        thinking: Thinking::default(),
                        effort_levels: vec![],
                        default_effort: None,
                        pricing: None,
                        deprecation: None,
                    }],
                },
            )]),
        }
    }

    /// The model id of the one entry in a sheet built by [`test_sheet`].
    fn model_id(sheet: &Sheet) -> &str {
        &sheet.providers["test"].models[0].id
    }

    type StubState = (
        Arc<AtomicUsize>,
        Option<Arc<Notify>>,
        Arc<Notify>,
        StatusCode,
        String,
    );

    /// A loopback stub for the release URL: counts requests, parks each
    /// response on the gate when gated, signals `answered` per response,
    /// and answers with a fixed status and body.
    struct Stub {
        url: String,
        requests: Arc<AtomicUsize>,
        gate: Option<Arc<Notify>>,
        answered: Arc<Notify>,
    }

    async fn stub_handler(
        State((requests, gate, answered, status, body)): State<StubState>,
    ) -> (StatusCode, String) {
        requests.fetch_add(1, Ordering::AcqRel);
        if let Some(gate) = gate {
            gate.notified().await;
        }
        answered.notify_one();
        (status, body)
    }

    async fn stub(status: StatusCode, body: String, gated: bool) -> Stub {
        let requests = Arc::new(AtomicUsize::new(0));
        let gate = gated.then(|| Arc::new(Notify::new()));
        let answered = Arc::new(Notify::new());
        let state: StubState = (
            Arc::clone(&requests),
            gate.clone(),
            Arc::clone(&answered),
            status,
            body,
        );
        let app = axum::Router::new()
            .route("/sheet.json", axum::routing::get(stub_handler))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("the stub binds");
        let addr = listener.local_addr().expect("the stub address");
        tokio::spawn(async move {
            let _ignored = axum::serve(listener, app).await;
        });
        Stub {
            url: format!("http://{addr}/sheet.json"),
            requests,
            gate,
            answered,
        }
    }

    /// Writes `sheet` as the cache file under a fresh tempdir, returning
    /// both so the tempdir outlives the test.
    fn cache_dir_with(sheet: &Sheet) -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let cache = temp.path().join(CACHE_FILE_NAME);
        std::fs::write(
            &cache,
            serde_json::to_vec(sheet).expect("the sheet serializes"),
        )
        .expect("the cache writes");
        (temp, cache)
    }

    /// A bearer-authed state with no filesystem context, for route tests.
    fn route_state() -> AppState {
        let config = Config::from_toml_str(
            "config-version = 2\n\
             [server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n",
        )
        .expect("the config parses");
        crate::test_support::app_state(config, None)
    }

    /// Sends one empty-bodied request through `build_router` with the
    /// valid bearer key and a loopback peer planted as the `ConnectInfo`.
    async fn request(state: AppState, method: Method, path: &str) -> Response {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header(AUTHORIZATION, "Bearer test-token")
            .body(Body::empty())
            .expect("static request parts are valid");
        let peer: SocketAddr = "127.0.0.1:50000".parse().expect("a socket address");
        request.extensions_mut().insert(ConnectInfo(peer));
        crate::build_router(state, None)
            .oneshot(request)
            .await
            .expect("the router is infallible")
    }

    /// Reads a JSON response body to a value.
    async fn body_json(response: Response) -> serde_json::Value {
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("the response body reads");
        serde_json::from_slice(&body).expect("the response body is JSON")
    }

    #[tokio::test]
    async fn a_fresh_cache_loads_without_a_download() {
        let sheet = test_sheet(OffsetDateTime::now_utc(), "cached-model");
        let (_temp, cache) = cache_dir_with(&sheet);
        let stub = stub(StatusCode::OK, "unreachable".to_owned(), false).await;
        let cloud = CloudModels::default();
        let download = cloud.launch(cache, stub.url.clone()).await;
        assert!(cloud.sheet().is_some(), "the fresh cache loads into memory");
        assert!(download.is_none(), "a fresh cache spawns no download");
        let served = cloud.sheet().expect("the cached sheet is in memory");
        assert_eq!(model_id(&served), "cached-model");
        // No task was spawned, so no request can ever have arrived.
        assert_eq!(stub.requests.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn a_week_old_cache_triggers_exactly_one_download() {
        let stale = test_sheet(
            OffsetDateTime::now_utc() - time::Duration::days(8),
            "stale-model",
        );
        let (_temp, cache) = cache_dir_with(&stale);
        let fresh = test_sheet(OffsetDateTime::now_utc(), "fresh-model");
        let stub = stub(
            StatusCode::OK,
            serde_json::to_string(&fresh).expect("the sheet serializes"),
            false,
        )
        .await;
        let cloud = CloudModels::default();
        let download = cloud.launch(cache.clone(), stub.url.clone()).await;
        assert_eq!(
            model_id(&cloud.sheet().expect("the stale sheet is in memory")),
            "stale-model"
        );
        let download = download.expect("a week-old cache spawns a download");
        download.await.expect("the download task joins");
        assert_eq!(stub.requests.load(Ordering::Acquire), 1);
        let served = cloud.sheet().expect("the downloaded sheet swaps in");
        assert_eq!(model_id(&served), "fresh-model");
        let on_disk: Sheet =
            serde_json::from_slice(&std::fs::read(&cache).expect("the cache reads"))
                .expect("the rewritten cache parses");
        assert_eq!(model_id(&on_disk), "fresh-model");
    }

    #[tokio::test]
    async fn an_unparseable_cache_is_treated_as_absent() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let cache = temp.path().join(CACHE_FILE_NAME);
        std::fs::write(&cache, b"not a sheet").expect("the cache writes");
        let sheet = test_sheet(OffsetDateTime::now_utc(), "downloaded-model");
        let stub = stub(
            StatusCode::OK,
            serde_json::to_string(&sheet).expect("the sheet serializes"),
            false,
        )
        .await;
        let cloud = CloudModels::default();
        let download = cloud.launch(cache.clone(), stub.url.clone()).await;
        assert!(
            cloud.sheet().is_none(),
            "an unparseable cache loads nothing"
        );
        let download = download.expect("an unparseable cache spawns a download");
        download.await.expect("the download task joins");
        let served = cloud.sheet().expect("the downloaded sheet is in memory");
        assert_eq!(model_id(&served), "downloaded-model");
        let on_disk: Sheet =
            serde_json::from_slice(&std::fs::read(&cache).expect("the cache reads"))
                .expect("the overwritten cache parses");
        assert_eq!(model_id(&on_disk), "downloaded-model");
    }

    #[tokio::test]
    async fn a_failed_download_keeps_the_old_cache() {
        let stale = test_sheet(
            OffsetDateTime::now_utc() - time::Duration::days(8),
            "stale-model",
        );
        let (_temp, cache) = cache_dir_with(&stale);
        let original = std::fs::read(&cache).expect("the cache reads");
        let stub = stub(StatusCode::INTERNAL_SERVER_ERROR, "boom".to_owned(), false).await;
        let cloud = CloudModels::default();
        let download = cloud.launch(cache.clone(), stub.url.clone()).await;
        assert!(
            cloud.sheet().is_some(),
            "the stale cache still loads into memory"
        );
        let download = download.expect("a week-old cache spawns a download");
        download.await.expect("the download task joins");
        assert_eq!(stub.requests.load(Ordering::Acquire), 1);
        let served = cloud.sheet().expect("the old sheet survives");
        assert_eq!(model_id(&served), "stale-model");
        assert!(
            cloud.last_error().is_some(),
            "the failure is recorded for the route's error answer"
        );
        assert_eq!(
            std::fs::read(&cache).expect("the cache reads"),
            original,
            "a failed download never touches the cache file"
        );
    }

    #[tokio::test]
    async fn concurrent_refreshes_never_start_a_second_download() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let cache = temp.path().join(CACHE_FILE_NAME);
        let sheet = test_sheet(OffsetDateTime::now_utc(), "downloaded-model");
        let stub = stub(
            StatusCode::OK,
            serde_json::to_string(&sheet).expect("the sheet serializes"),
            true,
        )
        .await;
        let cloud = CloudModels::default();
        let download = cloud.launch(cache, stub.url.clone()).await;
        assert!(cloud.sheet().is_none(), "a missing cache loads nothing");
        let download = download.expect("a missing cache spawns a download");
        // The in-flight guard is set before the task starts, so both
        // refreshes observe it regardless of where the download sits.
        assert!(
            matches!(cloud.refresh(), Download::InFlight),
            "a refresh during a download never starts a second one"
        );
        assert!(
            matches!(cloud.refresh(), Download::InFlight),
            "every concurrent refresh shares the one download"
        );
        stub.gate.as_ref().expect("the stub is gated").notify_one();
        download.await.expect("the download task joins");
        assert_eq!(stub.requests.load(Ordering::Acquire), 1);
    }

    #[tokio::test]
    async fn the_cache_write_replaces_the_old_file_and_leaves_no_temp() {
        let stale = test_sheet(
            OffsetDateTime::now_utc() - time::Duration::days(8),
            "stale-model",
        );
        let (_temp, cache) = cache_dir_with(&stale);
        let fresh = test_sheet(OffsetDateTime::now_utc(), "fresh-model");
        let stub = stub(
            StatusCode::OK,
            serde_json::to_string(&fresh).expect("the sheet serializes"),
            false,
        )
        .await;
        let cloud = CloudModels::default();
        let download = cloud.launch(cache.clone(), stub.url.clone()).await;
        download
            .expect("a week-old cache spawns a download")
            .await
            .expect("the download task joins");
        let on_disk: Sheet =
            serde_json::from_slice(&std::fs::read(&cache).expect("the cache reads"))
                .expect("the replaced cache parses");
        assert_eq!(
            model_id(&on_disk),
            "fresh-model",
            "the rename replaced the old cache wholesale"
        );
        assert!(
            !cache.with_extension("tmp").exists(),
            "the temp file is gone after the atomic replace"
        );
    }

    #[tokio::test]
    async fn the_route_serves_the_cached_sheet() {
        let sheet = test_sheet(OffsetDateTime::now_utc(), "cached-model");
        let (_temp, cache) = cache_dir_with(&sheet);
        let stub = stub(StatusCode::OK, "unreachable".to_owned(), false).await;
        let state = route_state();
        let download = state.cloud_models.launch(cache, stub.url.clone()).await;
        assert!(download.is_none(), "a fresh cache spawns no download");
        assert!(state.cloud_models.sheet().is_some());
        let response = request(state, Method::GET, "/admin/cloud-models").await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["providers"]["test"]["models"][0]["id"], "cached-model");
    }

    #[tokio::test]
    async fn the_route_reports_loading_before_the_sheet_arrives() {
        let state = route_state();
        let response = request(state.clone(), Method::GET, "/admin/cloud-models").await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = body_json(response).await;
        assert_eq!(body["error"]["code"], "cloud_models_loading");
        let response = request(state, Method::POST, "/admin/cloud-models/refresh").await;
        assert_eq!(
            response.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "an unlaunched module cannot refresh: there is nowhere to cache to"
        );
    }

    #[tokio::test]
    async fn the_route_reports_the_download_error() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let cache = temp.path().join(CACHE_FILE_NAME);
        let stub = stub(StatusCode::INTERNAL_SERVER_ERROR, "boom".to_owned(), false).await;
        let state = route_state();
        let download = state.cloud_models.launch(cache, stub.url.clone()).await;
        download
            .expect("a missing cache spawns a download")
            .await
            .expect("the download task joins");
        let response = request(state, Method::GET, "/admin/cloud-models").await;
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let body = body_json(response).await;
        assert_eq!(body["error"]["code"], "cloud_models_unavailable");
    }

    #[tokio::test]
    async fn the_refresh_route_forces_a_redownload_regardless_of_age() {
        let sheet = test_sheet(OffsetDateTime::now_utc(), "cached-model");
        let (_temp, cache) = cache_dir_with(&sheet);
        let fresh = test_sheet(OffsetDateTime::now_utc(), "refreshed-model");
        let stub = stub(
            StatusCode::OK,
            serde_json::to_string(&fresh).expect("the sheet serializes"),
            false,
        )
        .await;
        let state = route_state();
        let download = state.cloud_models.launch(cache, stub.url.clone()).await;
        assert!(
            download.is_none(),
            "a fresh cache spawns no launch download"
        );
        let response = request(state, Method::POST, "/admin/cloud-models/refresh").await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        tokio::time::timeout(std::time::Duration::from_secs(10), stub.answered.notified())
            .await
            .expect("the forced download answers");
        assert_eq!(
            stub.requests.load(Ordering::Acquire),
            1,
            "refresh downloads even though the cache is fresh"
        );
    }
}
