//! Routes serving the embedded workshop UI assets.

use axum::Router;
use axum::response::Response;
use axum::routing::get;

use crate::assets;

/// The UI asset routes: the index page, the bundled script and styles,
/// the PCM worklet, and the program icon at 1x and 2x. Stateless: every
/// response comes straight from [`crate::assets::UiAssets`].
pub(crate) fn routes() -> Router {
    Router::new()
        .route("/", get(ui_index))
        .route("/app.js", get(ui_app_js))
        .route("/app.css", get(ui_app_css))
        .route("/style.css", get(ui_style_css))
        .route("/pcm-worklet.js", get(ui_pcm_worklet))
        .route("/icons/promptforge-icon.png", get(ui_program_icon))
        .route("/icons/promptforge-icon@2x.png", get(ui_program_icon_2x))
}

/// Serves the chat UI's `index.html`.
async fn ui_index() -> Response {
    assets::ui_asset("index.html", "text/html; charset=utf-8")
}

/// Serves the chat UI's bundled application script.
async fn ui_app_js() -> Response {
    assets::ui_asset("app.js", "text/javascript; charset=utf-8")
}

/// Serves the stylesheet esbuild extracts from the bundle's CSS imports
/// (the dockview styles and the workshop components' colocated CSS).
async fn ui_app_css() -> Response {
    assets::ui_asset("app.css", "text/css; charset=utf-8")
}

/// Serves the chat UI's own stylesheet.
async fn ui_style_css() -> Response {
    assets::ui_asset("style.css", "text/css; charset=utf-8")
}

/// Serves the AudioWorklet PCM capture processor.
async fn ui_pcm_worklet() -> Response {
    assets::ui_asset("pcm-worklet.js", "text/javascript; charset=utf-8")
}

/// Serves the program icon shown in the custom title bar at 1x (128 px),
/// the `src` of the title bar `<img>`.
async fn ui_program_icon() -> Response {
    assets::ui_asset("icons/promptforge-icon.png", "image/png")
}

/// Serves the program icon at 2x (256 px), the title bar's `srcset`
/// entry for high-DPI displays.
async fn ui_program_icon_2x() -> Response {
    assets::ui_asset("icons/promptforge-icon@2x.png", "image/png")
}

#[cfg(test)]
mod tests;
