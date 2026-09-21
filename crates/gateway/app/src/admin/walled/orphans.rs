//! The `GET /admin/orphans` route: files in the artifact cache's `models/`
//! tree that no `[[local_model]]` or `[[stt_model]]` declared in the catalog
//! references, so an operator can adopt or delete leftovers.
//!
//! The scan is blocking filesystem work, so it goes through
//! [`crate::error::blocking`] like every store operation (Amendment D).
//! The diff itself sits in the local crate beside the blob cache, which owns
//! the slot layout and the sidecar records.

use axum::extract::State;
use axum::http::Method;
use axum::routing::get;
use axum::{Json, Router};
use gateway_config::SttModelConfig;
use serde::Serialize;

use crate::AppState;
use crate::auth::LoopbackCaller;
use crate::error::{GatewayError, blocking};
use crate::local::cache::{OrphanEntry, orphans};
use crate::local::resolve_cache_root;
use crate::registry::RouteInfo;

/// The `GET /admin/orphans` reply.
#[derive(Debug, Serialize)]
pub(crate) struct OrphansReply {
    /// Every cache file no catalog entry references.
    orphans: Vec<OrphanEntry>,
}

const ORPHANS: RouteInfo = RouteInfo::walled("/admin/orphans", &[Method::GET]);

/// The orphan-scan route, as the registry sees it.
pub(crate) const ROUTES: &[RouteInfo] = &[ORPHANS];

/// The orphan-scan route.
pub(crate) fn routes() -> Router<AppState> {
    Router::new().route(ORPHANS.path, get(admin_orphans))
}

/// The `GET /admin/orphans` route: bearer-authed, scans `<cache_dir>/models/`
/// and reports every file no `[[local_model]]` or `[[stt_model]]` declared in
/// the catalog references as `{"orphans": [{"path", "size_bytes", "sha256"}]}`.
///
/// `path` is relative to the resolved cache root (`/`-separated on every
/// platform). `sha256` comes from the blob's cache sidecar and is null for
/// files the cache API never downloaded: blobs are multi-gigabyte, so their
/// bytes are never re-hashed here. A missing cache or `models/` directory
/// reports an empty list.
pub(crate) async fn admin_orphans(
    State(state): State<AppState>,
    _caller: LoopbackCaller,
) -> Result<Json<OrphansReply>, GatewayError> {
    // The retained running config holds both the `[local].cache_dir` the
    // scan resolves and the catalog it diffs against: every `[[local_model]]`
    // and `[[stt_model]]` the document declares, whether or not the running
    // profile selects it. The catalog does not move on an apply, which
    // republishes the document with no profile selected.
    let config = state.config().await;
    let entries = blocking(move || {
        let root = resolve_cache_root(config.local().cache_dir())?;
        let stt_sources: Vec<&str> = config
            .catalog_stt_models()
            .iter()
            .map(SttModelConfig::source)
            .collect();
        orphans(&root, config.catalog_local_models(), &stt_sources)
    })
    .await?
    .map_err(GatewayError::cache)?;
    Ok(Json(OrphansReply { orphans: entries }))
}

#[cfg(test)]
#[path = "orphans-tests.rs"]
mod tests;
