//! The `GET /admin/progress` route: the process activity hub as an SSE
//! stream of [`Progress`] snapshots with heartbeats.

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::http::{HeaderValue, Method};
use axum::response::Response;
use axum::routing::get;
use gateway_api_types::Progress;
use gateway_progress::ProgressHub;

use crate::AppState;
use crate::admin::walled::shutdown;
use crate::auth::AuthedCaller;
use crate::error::GatewayError;
use crate::registry::RouteInfo;

const PROGRESS: RouteInfo = RouteInfo::open("/admin/progress", &[Method::GET]);

/// The progress stream route, as the registry sees it.
pub(crate) const ROUTES: &[RouteInfo] = &[PROGRESS];

/// The progress stream route.
pub(crate) fn routes() -> Router<AppState> {
    Router::new().route(PROGRESS.path, get(admin_progress))
}

/// Heartbeat cadence for the progress stream: SSE comment lines keep an
/// idle connection alive through NAT and firewall timeouts.
pub(crate) const PROGRESS_HEARTBEAT: std::time::Duration = std::time::Duration::from_secs(15);

/// The `GET /admin/progress` route: bearer-authed, streams the process
/// activity hub as SSE.
///
/// The reply is `text/event-stream` and never terminates on its own: a
/// freshly connected subscriber first receives the current [`Progress`]
/// snapshot, so it can render state without waiting for the next change,
/// then one `data:` line per change, with heartbeat comment lines every
/// [`PROGRESS_HEARTBEAT`] while nothing changes. The hub keeps only the
/// latest snapshot, so a slow subscriber skips intermediate texts and never
/// falls behind. Client disconnect is Drop all the way down: the response
/// body owns the receiver. The one server-side end is the process shutdown
/// signal: an attached subscriber (the config SPA, the workshop) would
/// otherwise hold its connection open through the graceful drain and pin
/// the process.
pub(crate) async fn admin_progress(
    State(state): State<AppState>,
    _caller: AuthedCaller,
) -> Result<Response, GatewayError> {
    Ok(progress_sse_response(&state.hub, state.shutdown.clone()))
}

/// Builds the progress SSE response over `hub`: the current snapshot first,
/// then one line per change, heartbeats in the gaps, until `shutdown`
/// fires.
pub(crate) fn progress_sse_response(
    hub: &ProgressHub,
    shutdown: shutdown::ShutdownSignal,
) -> Response {
    // The receiver starts holding the current snapshot as unseen, so the
    // first `changed()` resolves at once with the opening line and nothing
    // between subscribe and first poll is lost.
    let mut rx = hub.subscribe();
    rx.mark_changed();
    let heartbeat_at = tokio::time::Instant::now() + PROGRESS_HEARTBEAT;
    let stream = futures_util::stream::unfold(
        (
            rx,
            tokio::time::interval_at(heartbeat_at, PROGRESS_HEARTBEAT),
            shutdown,
        ),
        |(mut rx, mut heartbeat, shutdown)| async move {
            loop {
                tokio::select! {
                    () = shutdown.fired() => return None,
                    _ = heartbeat.tick() => {
                        return Some((
                            Ok::<_, std::convert::Infallible>(": heartbeat\n\n".to_owned()),
                            (rx, heartbeat, shutdown),
                        ));
                    }
                    changed = rx.changed() => match changed {
                        Ok(()) => {
                            let line = snapshot_line(&rx.borrow_and_update());
                            if let Some(line) = line {
                                return Some((Ok(line), (rx, heartbeat, shutdown)));
                            }
                        }
                        // The hub lives in `AppState` for the process
                        // lifetime, so its sender never closes first.
                        Err(_) => return None,
                    },
                }
            }
        },
    );
    let mut response = Response::new(Body::from_stream(stream));
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response
}

/// Serializes one snapshot as an SSE `data:` line, or `None` (logged) when
/// serialization fails: the wire type is plain data, so a failure is a
/// schema bug, and one bad snapshot must not kill the stream.
fn snapshot_line(snapshot: &Progress) -> Option<String> {
    match serde_json::to_string(snapshot) {
        Ok(json) => Some(format!("data: {json}\n\n")),
        Err(error) => {
            tracing::warn!(%error, "progress snapshot failed to serialize; dropping it");
            None
        }
    }
}

#[cfg(test)]
#[path = "progress-tests.rs"]
mod tests;
