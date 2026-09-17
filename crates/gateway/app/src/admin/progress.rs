//! The `GET /admin/progress` route: the process progress hub as an SSE
//! stream with heartbeats.

use axum::body::Body;
use axum::extract::State;
use axum::http::HeaderValue;
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::response::Response;

use crate::AppState;
use crate::auth::{Caller, check_auth};
use crate::error::GatewayError;
use crate::shutdown;
use shared_progress::{EventState, ProgressEvent, ProgressHub};

/// Heartbeat cadence for the progress stream: SSE comment lines keep an
/// idle connection alive through NAT and firewall timeouts.
pub(crate) const PROGRESS_HEARTBEAT: std::time::Duration = std::time::Duration::from_secs(15);

/// The `GET /admin/progress` route: bearer-authed, streams the process
/// progress hub as SSE.
///
/// The reply is `text/event-stream` and never terminates on its own: a
/// freshly connected subscriber first receives the live operations replayed
/// as synthetic `Begun`/`Updated` events, plus a `Finished` for each leaf
/// that already reached its terminal state, so it can render current state
/// without waiting for the next event, and then every broadcast
/// [`ProgressEvent`], including one operation-level terminal event when a
/// tree detaches, with heartbeat comment lines every
/// [`PROGRESS_HEARTBEAT`] while the hub is idle. Intermediate events are
/// lossy - a lagging subscriber drops them - and terminal events are never
/// coalesced at the source. Client disconnect is Drop all the way down, as
/// with the switch stream: the response body owns the receiver. The one
/// server-side end is the process shutdown signal: an attached subscriber
/// (the config SPA, the workshop) would otherwise hold its connection open
/// through the graceful drain and pin the process.
pub(crate) async fn admin_progress(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Response, GatewayError> {
    check_auth(&state, &caller).await?;
    Ok(progress_sse_response(&state.hub, state.shutdown.clone()))
}

/// Builds the progress SSE response over `hub`: a snapshot of the live
/// operations first, then the broadcast stream, heartbeats in the gaps,
/// until `shutdown` fires.
pub(crate) fn progress_sse_response(
    hub: &ProgressHub,
    shutdown: shutdown::ShutdownSignal,
) -> Response {
    // Subscribe before snapshotting so no event between the two is lost; a
    // `Begun` replayed from the snapshot is idempotent for remote import.
    let rx = hub.subscribe();
    let mut pending = std::collections::VecDeque::new();
    for operation in hub.snapshot() {
        for node in &operation.nodes {
            pending.extend(event_line(&ProgressEvent::new(
                operation.operation,
                node.path.clone(),
                node.label.clone(),
                EventState::Begun {
                    weight: node.weight,
                },
            )));
            if node.fraction > 0.0 {
                pending.extend(event_line(&ProgressEvent::new(
                    operation.operation,
                    node.path.clone(),
                    node.label.clone(),
                    EventState::Updated {
                        fraction: node.fraction,
                    },
                )));
            }
            // A leaf that finished before the subscriber connected replays
            // its terminal event too, or the subscriber would hold it as
            // unfinished until the tree detaches.
            if node.finished {
                pending.extend(event_line(&ProgressEvent::new(
                    operation.operation,
                    node.path.clone(),
                    node.label.clone(),
                    EventState::Finished { ok: node.ok },
                )));
            }
        }
    }
    let heartbeat_at = tokio::time::Instant::now() + PROGRESS_HEARTBEAT;
    let stream = futures_util::stream::unfold(
        (
            pending,
            rx,
            tokio::time::interval_at(heartbeat_at, PROGRESS_HEARTBEAT),
            shutdown,
        ),
        |(mut pending, mut rx, mut heartbeat, shutdown)| async move {
            if let Some(line) = pending.pop_front() {
                return Some((
                    Ok::<_, std::convert::Infallible>(line),
                    (pending, rx, heartbeat, shutdown),
                ));
            }
            loop {
                tokio::select! {
                    () = shutdown.fired() => return None,
                    _ = heartbeat.tick() => {
                        return Some((Ok(": heartbeat\n\n".to_owned()), (pending, rx, heartbeat, shutdown)));
                    }
                    received = rx.recv() => match received {
                        Ok(event) => {
                            if let Some(line) = event_line(&event) {
                                return Some((Ok(line), (pending, rx, heartbeat, shutdown)));
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                            tracing::debug!(skipped, "progress subscriber lagged; events dropped");
                        }
                        // The hub lives in `AppState` for the process
                        // lifetime, so its sender never closes first.
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
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

/// Serializes one event as an SSE `data:` line, or `None` (logged) when
/// serialization fails: the wire types are plain data, so a failure is a
/// schema bug, and one bad event must not kill the stream.
fn event_line(event: &ProgressEvent) -> Option<String> {
    match serde_json::to_string(event) {
        Ok(json) => Some(format!("data: {json}\n\n")),
        Err(error) => {
            tracing::warn!(%error, "progress event failed to serialize; dropping it");
            None
        }
    }
}

#[cfg(test)]
#[path = "progress-tests.rs"]
mod tests;
