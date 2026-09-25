//! The `/user/state` route handlers: the opaque account-scoped ui-state
//! bucket. `GET` answers every allow-listed key with its value or
//! `null`; `PUT /{key}` stores one value and answers `saved: true`.
//!
//! The body is taken raw rather than through the `Json` extractor so
//! that every refusal - an unknown key, a body over the cap, a body that
//! is not JSON - reaches the wire as the crate's own [`UserStateError`]
//! envelope instead of axum's plain-text rejection. The key is judged
//! first, then the body's size, then its shape, so the client is told
//! about the cheapest mistake. The raw body still passes through axum's
//! default body limit (2 MiB) before it reaches the handler; that hard
//! stop answers axum's own 413.

use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use serde_json::Value;

use workshop_support::{DEFAULT_DEADLINE, with_deadline};

use crate::error::UserStateError;
use crate::store::{UserStateStore, check_text_cap, user_state_key};

/// The user-state routes, narrowed to the [`UserStateStore`] - the only
/// state their handlers use - under the default deadline tier. The
/// subsystem registers this constructor into the registry; the server
/// merges its result into the API router.
pub fn routes(store: Arc<UserStateStore>) -> axum::Router {
    with_deadline(
        axum::Router::new()
            .route("/user/state", get(get_state))
            .route("/user/state/{key}", put(put_state))
            .with_state(store),
        DEFAULT_DEADLINE,
    )
}

/// Reports every user-state value the store holds, keyed by its
/// allow-listed name, `null` where nothing has been put.
pub(crate) async fn get_state(State(store): State<Arc<UserStateStore>>) -> Response {
    let document: serde_json::Map<String, Value> = store
        .all()
        .await
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value.unwrap_or(Value::Null)))
        .collect();
    respond(Ok(Value::Object(document)))
}

/// Stores the JSON body under `key`. A refused key or body answers the
/// envelope and changes nothing; a failed write answers the server-error
/// envelope while the value stands in memory.
pub(crate) async fn put_state(
    State(store): State<Arc<UserStateStore>>,
    Path(key): Path<String>,
    body: Bytes,
) -> Response {
    respond(store_value(&store, &key, &body).await)
}

/// Validates the key, then the body's size, then its shape, and only
/// then hands the value to the store.
async fn store_value(
    store: &UserStateStore,
    key: &str,
    body: &[u8],
) -> Result<Value, UserStateError> {
    let key = user_state_key(key)?;
    check_text_cap(body.len())?;
    let value: Value = serde_json::from_slice(body).map_err(|source| UserStateError::NotJson {
        source: source.into(),
    })?;
    store.put(key, value).await?;
    Ok(serde_json::json!({ "saved": true }))
}

/// Renders a user-state result as JSON, routing failures through the
/// [`UserStateError`] wire envelope.
fn respond(result: Result<Value, UserStateError>) -> Response {
    match result {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(error) => error.into_response(),
    }
}

#[cfg(test)]
#[path = "handlers-tests.rs"]
mod tests;
