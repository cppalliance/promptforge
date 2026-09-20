//! The open admin tier: bearer-authed routes reachable from any peer the
//! listener admits. Nothing here reads a secret in plaintext, writes a
//! file, or launches a process; a route that would belongs in
//! [`super::walled`].

pub(crate) mod profiles;
pub(crate) mod progress;
pub(crate) mod queue;
pub(crate) mod status;

use axum::Router;

use crate::AppState;

/// The open admin routes, merged into the root router without a wall.
pub(crate) fn routes() -> Router<AppState> {
    Router::new()
        .merge(profiles::routes())
        .merge(status::routes())
        .merge(progress::routes())
        .merge(queue::routes())
}
