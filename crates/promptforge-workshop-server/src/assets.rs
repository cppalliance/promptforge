//! The embedded workshop UI assets and the file-serving helper; the routes
//! that expose them live in [`crate::routes::assets`].

use axum::http::header;
use axum::response::{IntoResponse, Response};

use crate::error::AppError;

/// The workshop UI assets under `$OUT_DIR/ui-dist/`, written by the crate's
/// build script (the esbuild bundle plus copies of the static files). Debug
/// builds read the files from disk at request time, so UI edits need no Rust
/// recompile; release builds embed them into the binary.
#[derive(rust_embed::Embed)]
#[folder = "$OUT_DIR/ui-dist/"]
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

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;

    use super::*;

    /// Asserts an asset name answers 404 rather than file contents.
    fn assert_asset_misses(path: &str) {
        let response = ui_asset(path, "text/plain; charset=utf-8");
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "{path:?} must not escape the asset root"
        );
    }

    // These pin traversal parity between the two build profiles: release
    // misses the embed map by construction, while a debug build reads
    // `$OUT_DIR/ui-dist/` from disk at request time and must refuse names
    // resolving outside it. That guarantee covers request-supplied names
    // only: rust-embed 8.12.0 deliberately still serves an out-of-root
    // symlink planted inside the asset root, a bypass outside the parity
    // pinned here. The absolute target names this crate's own manifest - a
    // file that exists on disk - so it can only fail on containment; the
    // relative targets may also fail on absence.

    #[test]
    fn relative_traversal_answers_not_found() {
        assert_asset_misses("../../Cargo.toml");
    }

    #[test]
    fn backslash_traversal_answers_not_found() {
        assert_asset_misses(r"..\..\Cargo.toml");
    }

    #[test]
    fn absolute_path_answers_not_found() {
        assert_asset_misses(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"));
    }
}
