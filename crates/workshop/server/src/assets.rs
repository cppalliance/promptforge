//! The embedded workshop UI assets, the narrow [`AssetServer`] interface
//! the shell wires into the asset routes, and the file-serving helper; the
//! routes that expose them live in [`crate::routes::assets`].

use axum::http::header;
use axum::response::{IntoResponse, Response};

use crate::error::AppError;

/// The workshop UI assets under `$OUT_DIR/ui-dist/`, written by the crate's
/// build script (the esbuild bundle plus copies of the static files). Debug
/// builds read the files from disk at request time, so UI edits need no Rust
/// recompile; release builds embed them into the binary. Absent under the
/// `headless` feature, which drops the UI build entirely.
#[cfg(not(feature = "headless"))]
#[derive(rust_embed::Embed)]
#[folder = "$OUT_DIR/ui-dist/"]
pub(crate) struct UiAssets;

/// How long a cache may hold one asset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CachePolicy {
    /// A stable URL whose content changes between builds (`index.html`,
    /// the logical `app.js` route): force revalidation on every load, so
    /// a heuristic cache never serves a stale script against a newer
    /// server.
    Revalidate,
    /// A content-hashed URL (`bundle/app-<hash>.js`, the chunks): the
    /// name changes when the content does, so the response may be cached
    /// forever.
    Immutable,
}

impl CachePolicy {
    /// The Cache-Control header value for the policy.
    fn header_value(self) -> &'static str {
        match self {
            Self::Revalidate => "no-cache",
            Self::Immutable => "public, max-age=31536000, immutable",
        }
    }
}

/// The build's logical-to-hashed name map, parsed from the UI build's
/// `manifest.json`. The server resolves the stable routes (`/app.js`,
/// `/app.css`) through it to the current hashed files.
#[derive(Clone, Debug)]
pub(crate) struct AssetManifest {
    /// The hashed bundle path serving the logical `app.js`.
    app_js: String,
    /// The hashed bundle path serving the logical `app.css`.
    app_css: String,
}

impl AssetManifest {
    /// The hashed target for one logical name, or None when the manifest
    /// does not map it.
    pub(crate) fn resolve(&self, logical: &str) -> Option<&str> {
        match logical {
            "app.js" => Some(&self.app_js),
            "app.css" => Some(&self.app_css),
            _ => None,
        }
    }
}

/// The narrow asset-serving interface of the server's webview asset
/// layer. The shell wires exactly one implementation into the asset
/// routes: [`EmbeddedAssets`] in a normal build, [`NoopAssets`] under the
/// `headless` feature, which drops the UI build so server-only
/// integration tests run without the webview bundle.
pub(crate) trait AssetServer {
    /// Returns one asset's bytes, or None when the name is absent.
    fn get(&self, path: &str) -> Option<Vec<u8>>;

    /// The build's asset manifest, or None when the build emitted none.
    fn manifest(&self) -> Option<AssetManifest>;
}

/// The real asset layer: the embedded UI bundle.
#[cfg(not(feature = "headless"))]
pub(crate) struct EmbeddedAssets;

#[cfg(not(feature = "headless"))]
impl AssetServer for EmbeddedAssets {
    fn get(&self, path: &str) -> Option<Vec<u8>> {
        UiAssets::get(path).map(|asset| asset.data.into_owned())
    }

    fn manifest(&self) -> Option<AssetManifest> {
        let asset = UiAssets::get("manifest.json")?;
        // A parse failure is safe to ignore: the build writes the manifest
        // itself and embeds it beside the assets, so malformed JSON means a
        // corrupt build output rather than user input. Treating it as "no
        // manifest" degrades the logical routes to the same AssetMissing
        // 404 a missing manifest produces, instead of panicking the server.
        let value: serde_json::Value = serde_json::from_slice(&asset.data).ok()?;
        let target = |logical: &str| value.get(logical)?.as_str().map(str::to_string);
        Some(AssetManifest {
            app_js: target("app.js")?,
            app_css: target("app.css")?,
        })
    }
}

/// The no-op asset layer of a `headless` build: every name misses, so the
/// asset routes answer 404 while the API surface keeps working.
#[cfg(any(feature = "headless", test))]
pub(crate) struct NoopAssets;

#[cfg(any(feature = "headless", test))]
impl AssetServer for NoopAssets {
    fn get(&self, _path: &str) -> Option<Vec<u8>> {
        None
    }

    fn manifest(&self) -> Option<AssetManifest> {
        None
    }
}

/// Serves one UI asset through `server` with the given content type and
/// cache policy.
pub(crate) fn ui_asset(
    server: &dyn AssetServer,
    path: &str,
    content_type: &'static str,
    cache: CachePolicy,
) -> Response {
    match server.get(path) {
        Some(data) => (
            [
                (header::CONTENT_TYPE, content_type),
                (header::CACHE_CONTROL, cache.header_value()),
            ],
            data,
        )
            .into_response(),
        None => AppError::AssetMissing(path.to_string()).into_response(),
    }
}

#[cfg(all(test, not(feature = "headless")))]
mod tests {
    use axum::http::StatusCode;

    use super::*;

    /// Asserts an asset name answers 404 rather than file contents.
    fn assert_asset_misses(path: &str) {
        let response = ui_asset(
            &EmbeddedAssets,
            path,
            "text/plain; charset=utf-8",
            CachePolicy::Revalidate,
        );
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

    #[test]
    fn the_noop_asset_layer_serves_nothing() {
        let server = NoopAssets;
        assert!(server.get("index.html").is_none());
        assert!(server.manifest().is_none());
        let response = ui_asset(
            &server,
            "index.html",
            "text/html; charset=utf-8",
            CachePolicy::Revalidate,
        );
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn the_cache_policies_render_their_header_values() {
        assert_eq!(CachePolicy::Revalidate.header_value(), "no-cache");
        assert_eq!(
            CachePolicy::Immutable.header_value(),
            "public, max-age=31536000, immutable"
        );
    }
}
