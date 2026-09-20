//! The refresh route's await-and-answer contract: `POST
//! /admin/cloud-models/refresh` awaits the download it starts or joins
//! and answers 200 with the fresh sheet, or the download's 502 error
//! while the old sheet keeps serving; concurrent POSTs share one
//! download.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Duration;

use axum::http::Method;
use time::OffsetDateTime;

use super::*;
use crate::boot::CACHE_FILE_NAME;

type SequenceStubState = (
    Arc<AtomicUsize>,
    Arc<Mutex<VecDeque<(StatusCode, String)>>>,
    Option<Arc<Notify>>,
);

/// A loopback stub that answers each request with the next queued
/// (status, body) pair and counts requests; a gate, when present, parks
/// every response until released.
struct SequenceStub {
    url: String,
    requests: Arc<AtomicUsize>,
    gate: Option<Arc<Notify>>,
}

async fn sequence_stub_handler(
    State((requests, responses, gate)): State<SequenceStubState>,
) -> (StatusCode, String) {
    requests.fetch_add(1, Ordering::AcqRel);
    let answer = responses
        .lock()
        .expect("the response queue is not poisoned")
        .pop_front()
        .expect("the stub has a queued response for every request");
    if let Some(gate) = gate {
        gate.notified().await;
    }
    answer
}

async fn sequence_stub(responses: Vec<(StatusCode, String)>, gated: bool) -> SequenceStub {
    let requests = Arc::new(AtomicUsize::new(0));
    let gate = gated.then(|| Arc::new(Notify::new()));
    let state: SequenceStubState = (
        Arc::clone(&requests),
        Arc::new(Mutex::new(responses.into())),
        gate.clone(),
    );
    let app = axum::Router::new()
        .route("/sheet.json", axum::routing::get(sequence_stub_handler))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the stub binds");
    let addr = listener.local_addr().expect("the stub address");
    tokio::spawn(async move {
        let _ignored = axum::serve(listener, app).await;
    });
    SequenceStub {
        url: format!("http://{addr}/sheet.json"),
        requests,
        gate,
    }
}

/// One queued 200 answer carrying `sheet`.
fn ok_sheet(sheet: &Sheet) -> (StatusCode, String) {
    (
        StatusCode::OK,
        serde_json::to_string(sheet).expect("the sheet serializes"),
    )
}

/// The model id a JSON sheet body serves.
fn body_model_id(body: &serde_json::Value) -> &str {
    body["providers"]["test"]["models"][0]["id"]
        .as_str()
        .expect("the sheet body carries the model id")
}

/// Polls until the stub has received `expected` requests.
async fn wait_for_requests(stub: &SequenceStub, expected: usize) {
    for _ in 0..200 {
        if stub.requests.load(Ordering::Acquire) == expected {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the stub never saw {expected} requests");
}

#[tokio::test]
async fn the_refresh_route_answers_with_the_sheet_it_downloaded() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let cache = temp.path().join(CACHE_FILE_NAME);
    let launched = test_sheet(OffsetDateTime::now_utc(), "launched-model");
    let refreshed = test_sheet(OffsetDateTime::now_utc(), "refreshed-model");
    let stub = sequence_stub(vec![ok_sheet(&launched), ok_sheet(&refreshed)], false).await;
    let state = route_state();
    let download = state.cloud_models.launch(cache, stub.url.clone()).await;
    download
        .expect("a missing cache spawns a launch download")
        .await
        .expect("the launch download joins");
    let response = request(state.clone(), Method::POST, "/admin/cloud-models/refresh").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(
        body_model_id(&body),
        "refreshed-model",
        "the refresh POST answers 200 with the sheet it downloaded"
    );
    let response = request(state, Method::GET, "/admin/cloud-models").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(
        body_model_id(&body),
        "refreshed-model",
        "the GET then serves the refreshed sheet"
    );
    assert_eq!(stub.requests.load(Ordering::Acquire), 2);
}

#[tokio::test]
async fn the_refresh_route_answers_502_and_keeps_the_serving_sheet() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let cache = temp.path().join(CACHE_FILE_NAME);
    let launched = test_sheet(OffsetDateTime::now_utc(), "launched-model");
    let stub = sequence_stub(
        vec![
            ok_sheet(&launched),
            (StatusCode::INTERNAL_SERVER_ERROR, "boom".to_owned()),
        ],
        false,
    )
    .await;
    let state = route_state();
    let download = state.cloud_models.launch(cache, stub.url.clone()).await;
    download
        .expect("a missing cache spawns a launch download")
        .await
        .expect("the launch download joins");
    let response = request(state.clone(), Method::POST, "/admin/cloud-models/refresh").await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body = body_json(response).await;
    assert_eq!(
        body["error"]["code"], "cloud_models_unavailable",
        "the refresh POST answers the download's 502 error"
    );
    let response = request(state, Method::GET, "/admin/cloud-models").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(
        body_model_id(&body),
        "launched-model",
        "the GET still serves the pre-refresh sheet"
    );
}

#[tokio::test]
async fn two_concurrent_refresh_posts_share_one_download() {
    let sheet = test_sheet(OffsetDateTime::now_utc(), "cached-model");
    let (_temp, cache) = cache_dir_with(&sheet);
    let refreshed = test_sheet(OffsetDateTime::now_utc(), "refreshed-model");
    let stub = sequence_stub(vec![ok_sheet(&refreshed)], true).await;
    let state = route_state();
    let download = state.cloud_models.launch(cache, stub.url.clone()).await;
    assert!(
        download.is_none(),
        "a fresh cache spawns no launch download"
    );
    // join! polls both POSTs in one task: the first sets the in-flight
    // guard before its first await, so the second joins the same
    // download, and both park until the stub gate releases.
    let posts = tokio::spawn({
        let state = state.clone();
        async move {
            let first = state.clone();
            tokio::join!(
                request(first, Method::POST, "/admin/cloud-models/refresh"),
                request(state, Method::POST, "/admin/cloud-models/refresh"),
            )
        }
    });
    wait_for_requests(&stub, 1).await;
    stub.gate.as_ref().expect("the stub is gated").notify_one();
    let (first, second) = tokio::time::timeout(Duration::from_secs(10), posts)
        .await
        .expect("both refresh POSTs answer")
        .expect("the POST task joins");
    for response in [first, second] {
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(
            body_model_id(&body),
            "refreshed-model",
            "each concurrent POST answers with the shared download's sheet"
        );
    }
    assert_eq!(
        stub.requests.load(Ordering::Acquire),
        1,
        "two concurrent refresh POSTs produce one stub request"
    );
}

#[tokio::test]
async fn concurrent_refreshes_await_the_one_in_flight_download() {
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
    // The launch download holds the in-flight guard while parked on the
    // stub gate, so both refreshes join it instead of starting over.
    let refreshes = tokio::spawn({
        let cloud = cloud.clone();
        async move { tokio::join!(cloud.refresh(), cloud.refresh()) }
    });
    for _ in 0..200 {
        if stub.requests.load(Ordering::Acquire) == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(stub.requests.load(Ordering::Acquire), 1);
    stub.gate.as_ref().expect("the stub is gated").notify_one();
    let (first, second) = tokio::time::timeout(Duration::from_secs(10), refreshes)
        .await
        .expect("both refreshes answer")
        .expect("the refresh task joins");
    for outcome in [first, second] {
        let sheet = outcome.expect("each refresh awaits the shared download");
        assert_eq!(model_id(&sheet), "downloaded-model");
    }
    download.await.expect("the download task joins");
    assert_eq!(
        stub.requests.load(Ordering::Acquire),
        1,
        "every concurrent refresh shares the one download"
    );
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
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(
        body_model_id(&body),
        "refreshed-model",
        "the answered sheet is the forced download, not the fresh cache"
    );
    assert_eq!(
        stub.requests.load(Ordering::Acquire),
        1,
        "refresh downloads even though the cache is fresh"
    );
}
