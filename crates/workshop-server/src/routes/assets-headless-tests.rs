//! Asset-route behavior under the `headless` feature: the asset layer is
//! the no-op implementation, so every UI route answers 404 and the server
//! runs its API surface without the webview bundle.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use crate::app::fixtures::state_for;
use crate::app::router;

#[tokio::test]
async fn the_asset_routes_answer_not_found() {
    for uri in [
        "/",
        "/app.js",
        "/app.css",
        "/style.css",
        "/pcm-worklet.js",
        "/chunks/chunk-AAAAAAAA.js",
        "/bundle/app-AAAAAAAA.js",
        "/icons/promptforge-icon.png",
        "/icons/promptforge-icon@2x.png",
    ] {
        let (state, _state_dir) = state_for("http://127.0.0.1:1");
        let request = Request::builder()
            .uri(uri)
            .body(Body::empty())
            .expect("static request parts are valid");
        let response = router(state).oneshot(request).await.expect("infallible");
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "{uri} is a no-op under the headless feature"
        );
    }
}

#[tokio::test]
async fn the_api_surface_still_answers() {
    let (state, _state_dir) = state_for("http://127.0.0.1:1");
    let request = Request::builder()
        .uri("/health")
        .body(Body::empty())
        .expect("static request parts are valid");
    let response = router(state).oneshot(request).await.expect("infallible");
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "the headless server still serves its API"
    );
}
