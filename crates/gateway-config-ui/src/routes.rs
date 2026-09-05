//! Routes serving the embedded config UI assets behind the loopback wall.

use axum::Router;
use axum::response::Response;
use axum::routing::get;

use crate::{assets, require_loopback};

/// The config UI asset routes - the index page, the bundled script and
/// stylesheet, and the program icon at 1x and 2x - with
/// [`require_loopback`] already applied, so every asset answers 403 to a
/// non-loopback peer. The gateway nests this router at `/config`; the
/// index references its assets by relative path, so the mount point
/// needs no configuration.
pub fn routes() -> Router {
    Router::new()
        .route("/", get(ui_index))
        .route("/app.js", get(ui_app_js))
        .route("/app.css", get(ui_app_css))
        .route("/icons/promptforge-icon.png", get(ui_program_icon))
        .route("/icons/promptforge-icon@2x.png", get(ui_program_icon_2x))
        .layer(axum::middleware::from_fn(require_loopback))
}

/// Serves the config UI's `index.html`.
async fn ui_index() -> Response {
    assets::ui_asset("index.html", "text/html; charset=utf-8")
}

/// Serves the config UI's bundled application script.
async fn ui_app_js() -> Response {
    assets::ui_asset("app.js", "text/javascript; charset=utf-8")
}

/// Serves the config UI's bundled stylesheet, which esbuild emits next
/// to the script from the CSS imports in `main.ts`.
async fn ui_app_css() -> Response {
    assets::ui_asset("app.css", "text/css; charset=utf-8")
}

/// Serves the program icon at 1x (128 px), the `src` of every `<img>`.
async fn ui_program_icon() -> Response {
    assets::ui_asset("icons/promptforge-icon.png", "image/png")
}

/// Serves the program icon at 2x (256 px), the `srcset` entry for
/// high-DPI displays.
async fn ui_program_icon_2x() -> Response {
    assets::ui_asset("icons/promptforge-icon@2x.png", "image/png")
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use axum::body::Body;
    use axum::extract::ConnectInfo;
    use axum::http::{Request, StatusCode, header};
    use tower::ServiceExt;

    use super::*;

    /// Sends one GET through [`routes`] with a loopback peer planted, as
    /// `into_make_service_with_connect_info` would at a real listener.
    async fn get_as_loopback(uri: &str) -> Response {
        let mut request = Request::builder()
            .uri(uri)
            .body(Body::empty())
            .expect("static request parts are valid");
        let peer: SocketAddr = "127.0.0.1:50000".parse().expect("a socket address");
        request.extensions_mut().insert(ConnectInfo(peer));
        routes()
            .oneshot(request)
            .await
            .expect("the router is infallible")
    }

    /// Asserts a static UI route answers 200 with the expected content
    /// type and a non-empty body. Debug test builds serve the bundle from
    /// disk, so this also pins that the build script produced the assets.
    async fn assert_ui_asset(uri: &str, expected_content_type: &str) {
        let response = get_as_loopback(uri).await;
        assert_eq!(response.status(), StatusCode::OK, "{uri} serves");
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .unwrap_or_else(|| panic!("{uri} sets content-type"));
        assert_eq!(content_type, expected_content_type, "{uri} content type");
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("the body reads to completion");
        assert!(!body.is_empty(), "{uri} body is non-empty");
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
    async fn app_css_is_served_as_css() {
        assert_ui_asset("/app.css", "text/css; charset=utf-8").await;
    }

    #[tokio::test]
    async fn program_icon_is_served_as_png() {
        assert_ui_asset("/icons/promptforge-icon.png", "image/png").await;
    }

    #[tokio::test]
    async fn program_icon_2x_is_served_as_png() {
        assert_ui_asset("/icons/promptforge-icon@2x.png", "image/png").await;
    }

    #[tokio::test]
    async fn a_lan_peer_is_refused_by_every_asset_route() {
        for uri in [
            "/",
            "/app.js",
            "/app.css",
            "/icons/promptforge-icon.png",
            "/icons/promptforge-icon@2x.png",
        ] {
            let mut request = Request::builder()
                .uri(uri)
                .body(Body::empty())
                .expect("static request parts are valid");
            let peer: SocketAddr = "198.51.100.7:44821".parse().expect("a socket address");
            request.extensions_mut().insert(ConnectInfo(peer));
            let response = routes()
                .oneshot(request)
                .await
                .expect("the router is infallible");
            assert_eq!(
                response.status(),
                StatusCode::FORBIDDEN,
                "{uri} must refuse a LAN peer"
            );
        }
    }
}
