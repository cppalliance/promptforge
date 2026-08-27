//! The embedded workshop UI assets and the handlers that serve them.

use axum::http::header;
use axum::response::{IntoResponse, Response};

/// The workshop UI assets under `ui/dist/`, written by the crate's build
/// script (the esbuild bundle plus copies of the static files). Debug builds
/// read the files from disk at request time, so UI edits need no Rust
/// recompile; release builds embed them into the binary.
#[derive(rust_embed::Embed)]
#[folder = "ui/dist/"]
pub(crate) struct UiAssets;

/// Serves one UI asset from [`UiAssets`] with the given content type.
fn ui_asset(path: &str, content_type: &'static str) -> Response {
    match UiAssets::get(path) {
        Some(asset) => (
            [(header::CONTENT_TYPE, content_type)],
            asset.data.into_owned(),
        )
            .into_response(),
        None => (
            axum::http::StatusCode::NOT_FOUND,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            format!("ui asset not found: {path}"),
        )
            .into_response(),
    }
}

/// Serves the chat UI's `index.html`.
pub(crate) async fn ui_index() -> Response {
    ui_asset("index.html", "text/html; charset=utf-8")
}

/// Serves the chat UI's bundled application script.
pub(crate) async fn ui_app_js() -> Response {
    ui_asset("app.js", "text/javascript; charset=utf-8")
}

/// Serves the stylesheet esbuild extracts from the bundle's CSS imports
/// (the vendored murm-ui and dockview styles).
pub(crate) async fn ui_app_css() -> Response {
    ui_asset("app.css", "text/css; charset=utf-8")
}

/// Serves the chat UI's own stylesheet.
pub(crate) async fn ui_style_css() -> Response {
    ui_asset("style.css", "text/css; charset=utf-8")
}

/// Serves the AudioWorklet PCM capture processor.
pub(crate) async fn ui_pcm_worklet() -> Response {
    ui_asset("pcm-worklet.js", "text/javascript; charset=utf-8")
}

/// Serves the program icon shown in the custom title bar (the cold
/// medallion frame; the heat stages are reserved for a future activity
/// animation).
pub(crate) async fn ui_program_icon() -> Response {
    ui_asset("icons/promptforge-icon-1.png", "image/png")
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use crate::app::fixtures::{body_bytes, state_for};
    use crate::app::router;

    /// Asserts a static UI route answers 200 with the expected content type
    /// and a non-empty body.
    async fn assert_ui_asset(uri: &str, expected_content_type: &str) {
        let (state, _tape_dir) = state_for("http://127.0.0.1:1");
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
        assert_ui_asset("/icons/promptforge-icon-1.png", "image/png").await;
    }
}
