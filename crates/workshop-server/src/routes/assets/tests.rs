use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use tower::ServiceExt;

use crate::app::fixtures::{body_bytes, state_for};
use crate::app::router;

/// Asserts a static UI route answers 200 with the expected content type
/// and a non-empty body.
async fn assert_ui_asset(uri: &str, expected_content_type: &str) {
    let (state, _state_dir) = state_for("http://127.0.0.1:1");
    let request = Request::builder()
        .uri(uri)
        .body(Body::empty())
        .expect("static request parts are valid");
    let response = router(state)
        .oneshot(request)
        .await
        .expect("the router is infallible");
    assert_eq!(response.status(), StatusCode::OK, "{uri} serves");
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .unwrap_or_else(|| panic!("{uri} sets content-type"));
    assert_eq!(content_type, expected_content_type, "{uri} content type");
    assert!(
        !body_bytes(response).await.is_empty(),
        "{uri} body is non-empty"
    );
}

/// Every asset must force revalidation: the bundle is unversioned, so
/// a heuristic cache with no validator serves a stale script against a
/// newer server.
#[tokio::test]
async fn every_asset_forces_revalidation() {
    for uri in [
        "/",
        "/app.js",
        "/style.css",
        "/app.css",
        "/pcm-worklet.js",
        "/icons/promptforge-icon.png",
        "/icons/promptforge-icon@2x.png",
    ] {
        let (state, _state_dir) = state_for("http://127.0.0.1:1");
        let request = Request::builder()
            .uri(uri)
            .body(Body::empty())
            .expect("static request parts are valid");
        let response = router(state).oneshot(request).await.expect("infallible");
        let cache_control = response
            .headers()
            .get(header::CACHE_CONTROL)
            .unwrap_or_else(|| panic!("{uri} sets cache-control"));
        assert_eq!(cache_control, "no-cache", "{uri} cache-control");
    }
}

#[tokio::test]
async fn index_is_served_at_the_root() {
    assert_ui_asset("/", "text/html; charset=utf-8").await;
}

#[tokio::test]
async fn app_js_is_served_as_javascript() {
    assert_ui_asset("/app.js", "text/javascript; charset=utf-8").await;
}

#[tokio::test]
async fn style_css_is_served_as_css() {
    assert_ui_asset("/style.css", "text/css; charset=utf-8").await;
}

#[tokio::test]
async fn bundled_app_css_is_served_as_css() {
    assert_ui_asset("/app.css", "text/css; charset=utf-8").await;
}

#[tokio::test]
async fn pcm_worklet_is_served_as_javascript() {
    assert_ui_asset("/pcm-worklet.js", "text/javascript; charset=utf-8").await;
}

#[tokio::test]
async fn program_icon_is_served_as_png() {
    assert_ui_asset("/icons/promptforge-icon.png", "image/png").await;
}

#[tokio::test]
async fn program_icon_2x_is_served_as_png() {
    assert_ui_asset("/icons/promptforge-icon@2x.png", "image/png").await;
}
