//! The `/v1/cache` routes: bearer-authenticated on-demand blob downloads into
//! the operator cache, with sidecar-based listing and removal.
//!
//! The store is blocking filesystem plus a reqwest-blocking client, so every
//! store operation runs inside `tokio::task::spawn_blocking` and never blocks
//! the executor (Amendment D).

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;

use crate::error::GatewayError;
use crate::local::cache::{BlobCache, CacheEntry};
use crate::local::{LocalError, resolve_cache_root};
use crate::{AppState, check_auth};

/// Opens the blob cache at the active profile's resolved cache root.
fn open_cache(cache_dir: Option<&str>) -> Result<BlobCache, LocalError> {
    BlobCache::new(resolve_cache_root(cache_dir)?)
}

/// The cache dir configured on the live profile (`[local].cache_dir`).
async fn live_cache_dir(state: &AppState) -> Option<String> {
    state.cache_dir().await
}

/// `GET /v1/cache`: the sidecar-backed listing of cached blobs.
///
/// Reads `<file>.meta.json` sidecars only, so listing never re-hashes a blob
/// (Amendment C); blobs without sidecars are not cache entries and do not
/// appear.
pub(crate) async fn list_cache(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<CacheEntry>>, GatewayError> {
    check_auth(&state, &headers).await?;
    let cache_dir = live_cache_dir(&state).await;
    let entries = tokio::task::spawn_blocking(move || open_cache(cache_dir.as_deref())?.list())
        .await
        .map_err(GatewayError::cache)?
        .map_err(GatewayError::cache)?;
    Ok(Json(entries))
}
