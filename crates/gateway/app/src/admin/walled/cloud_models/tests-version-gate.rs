//! The schema-version gate: a sheet that parses but declares a schema
//! version the gateway does not accept is refused on both the cache and
//! the download path.

use axum::http::Method;
use axum::http::StatusCode;
use gateway_api_types::ACCEPTED_SHEET_SCHEMA_VERSION;
use time::OffsetDateTime;

use super::*;

#[tokio::test]
async fn a_version_mismatched_cache_is_treated_as_absent() {
    let mut future = test_sheet(OffsetDateTime::now_utc(), "future-model");
    future.schema_version = ACCEPTED_SHEET_SCHEMA_VERSION + 1;
    let (_temp, cache) = cache_dir_with(&future);
    let sheet = test_sheet(OffsetDateTime::now_utc(), "downloaded-model");
    let stub = stub(
        StatusCode::OK,
        serde_json::to_string(&sheet).expect("the sheet serializes"),
        false,
    )
    .await;
    let cloud = CloudModels::default();
    let download = cloud.launch(cache, stub.url.clone()).await;
    assert!(
        cloud.sheet().is_none(),
        "a version-mismatched cache loads nothing"
    );
    let download = download.expect("a version-mismatched cache spawns a download");
    download.await.expect("the download task joins");
    let served = cloud.sheet().expect("the downloaded sheet is in memory");
    assert_eq!(model_id(&served), "downloaded-model");
}

#[tokio::test]
async fn a_version_mismatched_download_records_the_versions_in_last_error() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let cache = temp.path().join(CACHE_FILE_NAME);
    let mut future = test_sheet(OffsetDateTime::now_utc(), "future-model");
    future.schema_version = ACCEPTED_SHEET_SCHEMA_VERSION + 1;
    let stub = stub(
        StatusCode::OK,
        serde_json::to_string(&future).expect("the sheet serializes"),
        false,
    )
    .await;
    let state = route_state();
    let download = state.cloud_models.launch(cache, stub.url.clone()).await;
    download
        .expect("a missing cache spawns a download")
        .await
        .expect("the download task joins");
    let error = state
        .cloud_models
        .last_error()
        .expect("the version mismatch records an error");
    assert!(
        error.contains(&(ACCEPTED_SHEET_SCHEMA_VERSION + 1).to_string()),
        "the error names the found version: {error}"
    );
    assert!(
        error.contains(ACCEPTED_SHEET_SCHEMA_VERSION.to_string().as_str()),
        "the error names the accepted version: {error}"
    );
    assert!(
        state.cloud_models.sheet().is_none(),
        "a version-mismatched download never swaps the sheet in"
    );
    let response = request(state, Method::GET, "/admin/cloud-models").await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
}
