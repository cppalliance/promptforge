use axum::body::Body;
use axum::http::{Request, Response, StatusCode, header};
use tower::ServiceExt;

use crate::app::fixtures::{body_bytes, state_for};
use crate::app::router;

/// Performs one GET against the router and returns the response.
async fn get(uri: &str) -> Response<Body> {
    let (state, _state_dir) = state_for("http://127.0.0.1:1");
    let request = Request::builder()
        .uri(uri)
        .body(Body::empty())
        .unwrap_or_else(|_| panic!("static request parts are valid"));
    router(state)
        .oneshot(request)
        .await
        .unwrap_or_else(|_| panic!("the router is infallible"))
}

/// The response's Cache-Control value.
fn cache_control(response: &Response<Body>) -> &header::HeaderValue {
    response
        .headers()
        .get(header::CACHE_CONTROL)
        .unwrap_or_else(|| panic!("the response sets cache-control"))
}

/// The build's asset manifest: the logical-to-hashed name map the UI
/// build emits next to the bundle.
fn asset_manifest() -> serde_json::Value {
    let asset = crate::assets::UiAssets::get("manifest.json")
        .unwrap_or_else(|| panic!("the UI build emits manifest.json"));
    serde_json::from_slice(&asset.data)
        .unwrap_or_else(|error| panic!("manifest.json is valid JSON: {error}"))
}

/// The manifest's hashed bundle target for one logical name.
fn manifest_target(logical: &str) -> String {
    asset_manifest()[logical]
        .as_str()
        .unwrap_or_else(|| panic!("the manifest maps {logical}"))
        .to_string()
}

/// Asserts a static UI route answers 200 with the expected content type
/// and a non-empty body.
async fn assert_ui_asset(uri: &str, expected_content_type: &str) {
    let (state, _state_dir) = state_for("http://127.0.0.1:1");
    let request = Request::builder()
        .uri(uri)
        .body(Body::empty())
        .unwrap_or_else(|_| panic!("static request parts are valid"));
    let response = router(state)
        .oneshot(request)
        .await
        .unwrap_or_else(|_| panic!("the router is infallible"));
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

/// The code-split chunks have content-hashed names, so the test discovers
/// one through the embedded asset listing rather than naming it.
fn first_chunk_name(extension: &str) -> String {
    crate::assets::UiAssets::iter()
        .map(std::borrow::Cow::into_owned)
        .find(|name| name.starts_with("chunks/") && name.ends_with(extension))
        .unwrap_or_else(|| panic!("the UI build emits a chunks/*{extension} chunk"))
}

#[tokio::test]
async fn a_code_split_chunk_is_served_as_javascript() {
    let name = first_chunk_name(".js");
    assert_ui_asset(&format!("/{name}"), "text/javascript; charset=utf-8").await;
}

#[tokio::test]
async fn a_chunk_without_a_bundle_extension_is_not_found() {
    let (state, _state_dir) = state_for("http://127.0.0.1:1");
    let request = Request::builder()
        .uri("/chunks/app.toml")
        .body(Body::empty())
        .expect("static request parts are valid");
    let response = router(state).oneshot(request).await.expect("infallible");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_chunk_name_cannot_escape_the_asset_root() {
    // The wildcard captures the remainder raw; ui_asset's own tests pin
    // traversal refusal for the names it is handed. Here the route must
    // not 500 on a hostile name.
    let (state, _state_dir) = state_for("http://127.0.0.1:1");
    let request = Request::builder()
        .uri("/chunks/..%2F..%2FCargo.toml")
        .body(Body::empty())
        .expect("static request parts are valid");
    let response = router(state).oneshot(request).await.expect("infallible");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_missing_chunk_is_not_found() {
    let (state, _state_dir) = state_for("http://127.0.0.1:1");
    let request = Request::builder()
        .uri("/chunks/chunk-DEADBEEF00.js")
        .body(Body::empty())
        .expect("static request parts are valid");
    let response = router(state).oneshot(request).await.expect("infallible");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_manifest_maps_logical_names_to_hashed_bundle_files() {
    let manifest = asset_manifest();
    for (logical, extension) in [("app.js", ".js"), ("app.css", ".css")] {
        let target = manifest[logical]
            .as_str()
            .unwrap_or_else(|| panic!("the manifest maps {logical}"));
        assert!(
            target.starts_with("bundle/app-") && target.ends_with(extension),
            "{logical} maps to a content-hashed bundle file, got {target:?}"
        );
        assert!(
            crate::assets::UiAssets::get(target).is_some(),
            "the manifest target {target:?} exists in the bundle"
        );
    }
}

#[tokio::test]
async fn the_index_page_references_the_hashed_bundle() {
    let response = get("/").await;
    assert_eq!(response.status(), StatusCode::OK);
    let html = String::from_utf8(body_bytes(response).await.to_vec()).expect("index.html is UTF-8");
    let script = manifest_target("app.js");
    let styles = manifest_target("app.css");
    assert!(
        html.contains(&format!("src=\"/{script}\"")),
        "index.html loads the hashed script {script:?}"
    );
    assert!(
        html.contains(&format!("href=\"/{styles}\"")),
        "index.html loads the hashed styles {styles:?}"
    );
    assert!(
        !html.contains("src=\"/app.js\""),
        "index.html never references the unversioned script"
    );
}

#[tokio::test]
async fn the_logical_app_routes_serve_the_hashed_bytes() {
    for (uri, logical) in [("/app.js", "app.js"), ("/app.css", "app.css")] {
        let response = get(uri).await;
        assert_eq!(response.status(), StatusCode::OK, "{uri} serves");
        assert_eq!(
            cache_control(&response),
            "no-cache",
            "{uri} is a stable URL and must revalidate"
        );
        let expected = crate::assets::UiAssets::get(&manifest_target(logical))
            .expect("the manifest target exists");
        assert_eq!(
            &body_bytes(response).await[..],
            &expected.data[..],
            "{uri} serves the manifest target's bytes"
        );
    }
}

#[tokio::test]
async fn hashed_bundle_assets_are_cached_immutably() {
    for logical in ["app.js", "app.css"] {
        let target = manifest_target(logical);
        let response = get(&format!("/{target}")).await;
        assert_eq!(response.status(), StatusCode::OK, "/{target} serves");
        assert_eq!(
            cache_control(&response),
            "public, max-age=31536000, immutable",
            "/{target} is content-hashed, so its URL never changes meaning"
        );
    }
}

#[tokio::test]
async fn a_code_split_chunk_is_cached_immutably() {
    let name = first_chunk_name(".js");
    let response = get(&format!("/{name}")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        cache_control(&response),
        "public, max-age=31536000, immutable",
        "chunk names are content-hashed, so their URLs never change meaning"
    );
}

#[tokio::test]
async fn a_bundle_name_without_a_bundle_extension_is_not_found() {
    let response = get("/bundle/app.toml").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_bundle_name_cannot_escape_the_asset_root() {
    let response = get("/bundle/..%2F..%2FCargo.toml").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_missing_bundle_file_is_not_found() {
    let response = get("/bundle/app-DEADBEEF00.js").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
