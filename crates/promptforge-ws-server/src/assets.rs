//! The embedded workshop UI assets and the file-serving helper; the routes
//! that expose them live in [`crate::routes::assets`].

use axum::http::header;
use axum::response::{IntoResponse, Response};

use crate::error::AppError;

/// The workshop UI assets under `ui/dist/`, written by the crate's build
/// script (the esbuild bundle plus copies of the static files). Debug builds
/// read the files from disk at request time, so UI edits need no Rust
/// recompile; release builds embed them into the binary.
#[derive(rust_embed::Embed)]
#[folder = "ui/dist/"]
pub(crate) struct UiAssets;

/// Serves one UI asset from [`UiAssets`] with the given content type.
pub(crate) fn ui_asset(path: &str, content_type: &'static str) -> Response {
    match UiAssets::get(path) {
        Some(asset) => (
            [(header::CONTENT_TYPE, content_type)],
            asset.data.into_owned(),
        )
            .into_response(),
        None => AppError::AssetMissing(path.to_string()).into_response(),
    }
}
