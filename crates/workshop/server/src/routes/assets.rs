//! Routes serving the embedded workshop UI assets.

use axum::Router;
use axum::extract::Path;
use axum::response::{IntoResponse, Response};
use axum::routing::get;

use crate::assets::{self, AssetServer, CachePolicy};
use crate::error::AppError;

/// The asset layer the shell wires into these routes: the embedded UI
/// bundle, or the no-op implementation under the `headless` feature,
/// which drops the UI build so server-only integration tests run without
/// the webview assets.
fn asset_server() -> &'static dyn AssetServer {
    #[cfg(not(feature = "headless"))]
    {
        &assets::EmbeddedAssets
    }
    #[cfg(feature = "headless")]
    {
        &assets::NoopAssets
    }
}

/// The UI asset routes: the index page, the bundled script and styles
/// (stable logical routes resolving through the build's manifest, plus
/// the content-hashed `bundle/` files the stamped index page loads
/// directly), the code-split chunks the bundle lazy-loads, the PCM
/// worklet, and the program icon at 1x and 2x. Stateless: every response
/// comes straight from the [`AssetServer`].
pub(crate) fn routes() -> Router {
    Router::new()
        .route("/", get(ui_index))
        .route("/app.js", get(ui_app_js))
        .route("/app.css", get(ui_app_css))
        .route("/style.css", get(ui_style_css))
        .route("/pcm-worklet.js", get(ui_pcm_worklet))
        .route("/bundle/{*name}", get(ui_bundle))
        .route("/chunks/{*name}", get(ui_chunk))
        .route("/icons/promptforge-icon.png", get(ui_program_icon))
        .route("/icons/promptforge-icon@2x.png", get(ui_program_icon_2x))
}

/// Serves the chat UI's `index.html`.
async fn ui_index() -> Response {
    assets::ui_asset(
        asset_server(),
        "index.html",
        "text/html; charset=utf-8",
        CachePolicy::Revalidate,
    )
}

/// Serves the bundled application script through the stable logical
/// route: the build's manifest maps `app.js` to the current
/// content-hashed file, whose bytes this answers with. The URL keeps its
/// meaning across builds, so the response must revalidate; the stamped
/// index page loads the hashed URL (immutable) directly.
async fn ui_app_js() -> Response {
    logical_asset("app.js", "text/javascript; charset=utf-8")
}

/// Serves the stylesheet esbuild extracts from the bundle's CSS imports
/// (the dockview styles and the workshop components' colocated CSS),
/// through the same manifest resolution as the script.
async fn ui_app_css() -> Response {
    logical_asset("app.css", "text/css; charset=utf-8")
}

/// Serves one logical bundle route by resolving the build manifest to
/// the current hashed file.
fn logical_asset(logical: &str, content_type: &'static str) -> Response {
    let server = asset_server();
    let Some(manifest) = server.manifest() else {
        return AppError::AssetMissing(logical.to_string()).into_response();
    };
    let Some(target) = manifest.resolve(logical) else {
        return AppError::AssetMissing(logical.to_string()).into_response();
    };
    assets::ui_asset(server, target, content_type, CachePolicy::Revalidate)
}

/// Serves the chat UI's own stylesheet.
async fn ui_style_css() -> Response {
    assets::ui_asset(
        asset_server(),
        "style.css",
        "text/css; charset=utf-8",
        CachePolicy::Revalidate,
    )
}

/// Serves the AudioWorklet PCM capture processor.
async fn ui_pcm_worklet() -> Response {
    assets::ui_asset(
        asset_server(),
        "pcm-worklet.js",
        "text/javascript; charset=utf-8",
        CachePolicy::Revalidate,
    )
}

/// The content type for one wildcard-served bundle file, or None when
/// the extension is not one esbuild emits.
fn bundle_content_type(name: &str) -> Option<&'static str> {
    let extension_is = |expected: &str| {
        std::path::Path::new(name)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case(expected))
    };
    if extension_is("js") {
        Some("text/javascript; charset=utf-8")
    } else if extension_is("css") {
        Some("text/css; charset=utf-8")
    } else {
        None
    }
}

/// Serves one content-hashed entry file. esbuild emits the entry (and
/// its extracted stylesheet) under `bundle/` with content-hashed names,
/// so the routes cannot name them individually; the wildcard stays inside
/// the embedded asset root through [`assets::ui_asset`], and only the two
/// kinds esbuild emits are served. The names change with the content, so
/// the responses are cached immutably.
async fn ui_bundle(Path(name): Path<String>) -> Response {
    let Some(content_type) = bundle_content_type(&name) else {
        return AppError::AssetMissing(format!("bundle/{name}")).into_response();
    };
    assets::ui_asset(
        asset_server(),
        &format!("bundle/{name}"),
        content_type,
        CachePolicy::Immutable,
    )
}

/// Serves one code-split bundle chunk. esbuild emits the lazily loaded
/// feature chunks (the agent session's Shiki/TipTap graph, the editor's
/// CodeMirror) under `chunks/` with content-hashed names, so the routes
/// cannot name them individually; the wildcard stays inside the embedded
/// asset root through [`assets::ui_asset`], and only the two kinds
/// esbuild emits are served. The names change with the content, so the
/// responses are cached immutably.
async fn ui_chunk(Path(name): Path<String>) -> Response {
    let Some(content_type) = bundle_content_type(&name) else {
        return AppError::AssetMissing(format!("chunks/{name}")).into_response();
    };
    assets::ui_asset(
        asset_server(),
        &format!("chunks/{name}"),
        content_type,
        CachePolicy::Immutable,
    )
}

/// Serves the program icon shown in the custom title bar at 1x (128 px),
/// the `src` of the title bar `<img>`.
async fn ui_program_icon() -> Response {
    assets::ui_asset(
        asset_server(),
        "icons/promptforge-icon.png",
        "image/png",
        CachePolicy::Revalidate,
    )
}

/// Serves the program icon at 2x (256 px), the title bar's `srcset`
/// entry for high-DPI displays.
async fn ui_program_icon_2x() -> Response {
    assets::ui_asset(
        asset_server(),
        "icons/promptforge-icon@2x.png",
        "image/png",
        CachePolicy::Revalidate,
    )
}

#[cfg(all(test, not(feature = "headless")))]
#[path = "assets-tests.rs"]
mod tests;

#[cfg(all(test, feature = "headless"))]
#[path = "assets-headless-tests.rs"]
mod headless_tests;
