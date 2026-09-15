use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::header::AUTHORIZATION;
use axum::http::{Method, Request};
use gateway_config::Config;
use gateway_protocol::http_util::MAX_JSON_BODY;
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
async fn an_over_cap_download_records_the_cap_error_without_swapping_the_sheet() {
    let stale = test_sheet(
        OffsetDateTime::now_utc() - time::Duration::days(8),
        "stale-model",
    );
    let (_temp, cache) = cache_dir_with(&stale);
    let original = std::fs::read(&cache).expect("the cache reads");
    let over_cap = "x".repeat(MAX_JSON_BODY + 1);
    let stub = stub(StatusCode::OK, over_cap, false).await;
    let state = route_state();
    let download = state.cloud_models.launch(cache.clone(), stub.url.clone()).await;
    download
        .expect("a week-old cache spawns a download")
        .await
        .expect("the download task joins");
    assert_eq!(stub.requests.load(Ordering::Acquire), 1);
    let error = state
        .cloud_models
        .last_error()
        .expect("the over-cap download records an error");
    assert!(
        error.contains(MAX_JSON_BODY.to_string().as_str()),
        "the recorded error names the {MAX_JSON_BODY} byte cap: {error}"
    );
    let served = state.cloud_models.sheet().expect("the old sheet survives");
    assert_eq!(
        model_id(&served),
        "stale-model",
        "an over-cap body never swaps the sheet"
    );
    assert_eq!(
        std::fs::read(&cache).expect("the cache reads"),
        original,
        "an over-cap download never touches the cache file"
    );
    let response = request(state, Method::GET, "/admin/cloud-models").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(
        body["providers"]["test"]["models"][0]["id"],
        "stale-model",
        "the route keeps serving the pre-download sheet"
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
