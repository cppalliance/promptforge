//! The `GET /admin/model-info` route: architecture, layer count, and
//! parameter count read from a GGUF header in the artifact cache, feeding
//! the UI's `gpu_layers` "N / total" slider readout.
//!
//! The header parse is blocking filesystem work, so it goes through
//! [`crate::error::blocking`] like every store operation (Amendment D).
//! The parser itself sits in the local crate beside the blob cache, which
//! owns GGUF domain knowledge.

use std::path::PathBuf;

use axum::extract::State;
use axum::http::Method;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;

use crate::AppState;
use crate::auth::LoopbackCaller;
use crate::error::{GatewayError, WireQuery, blocking};
use crate::local::{LocalError, gguf, resolve_cache_root};
use crate::registry::RouteInfo;

const MODEL_INFO: RouteInfo = RouteInfo::walled("/admin/model-info", &[Method::GET]);

/// The GGUF header readout route, as the registry sees it.
pub(crate) const ROUTES: &[RouteInfo] = &[MODEL_INFO];

/// The GGUF header readout route.
pub(crate) fn routes() -> Router<AppState> {
    Router::new().route(MODEL_INFO.path, get(admin_model_info))
}

/// Query parameters for `GET /admin/model-info`.
#[derive(Debug, Deserialize)]
pub(crate) struct ModelInfoQuery {
    /// Cache-relative path of the GGUF file to inspect.
    path: String,
}

/// The `GET /admin/model-info?path=` route: bearer-authed, parses the GGUF
/// header of the named cache file and reports
/// `{"architecture", "layer_count", "parameter_count", "chat_template"}`
/// (each nullable).
///
/// `path` is caller input and is confined to the artifact cache: only a
/// relative path that resolves under the resolved cache root without
/// crossing a link is accepted - the same `/`-separated form
/// `GET /admin/orphans` reports - so the endpoint can never read an
/// arbitrary file. A missing or escaping path maps to 400; a file that is
/// missing or not a well-formed GGUF header maps to 422. The UI treats any
/// failure as "layer count unknown" and falls back to a plain readout.
pub(crate) async fn admin_model_info(
    State(state): State<AppState>,
    _caller: LoopbackCaller,
    WireQuery(query): WireQuery<ModelInfoQuery>,
) -> Result<Json<gguf::ModelInfo>, GatewayError> {
    // The retained running config carries the `[local].cache_dir` the path
    // is confined to, so the boundary and the store agree on the root.
    let config = state.config().await;
    let info = blocking(move || {
        let root = resolve_cache_root(config.local().cache_dir())?;
        gguf::read_model_info(&root, &PathBuf::from(query.path))
    })
    .await?
    .map_err(|error| match error {
        // The rejected boundary check is the caller's fault, not the file's.
        LocalError::UnsafeCachePath { path } => GatewayError::MalformedRequest(format!(
            "path `{}` is not a relative path inside the artifact cache",
            path.display()
        )),
        other => GatewayError::model_info(other),
    })?;
    Ok(Json(info))
}

#[cfg(test)]
#[path = "model_info-tests.rs"]
mod tests;
