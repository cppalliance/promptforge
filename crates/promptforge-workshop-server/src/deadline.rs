//! Route deadlines: the wall-clock tiers every HTTP route group carries
//! and the middleware that enforces them. The composition root applies
//! the default tier; the gateway-relay routes carry the longer tier so
//! the gateway client's own timeout fires first.

use std::time::Duration;

use axum::Router;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::IntoResponse;

/// Deadline for ordinary HTTP routes: local, fast work that should never
/// run long. A response not produced in time answers 408.
pub(crate) const DEFAULT_DEADLINE: Duration = Duration::from_secs(10);

/// Deadline for routes that relay a buffered gateway call: longer than the
/// gateway client's own request timeout, so a stalled gateway surfaces as
/// the relay's 502 with its failure shape rather than a blunt 408 from the
/// route deadline.
pub(crate) const RELAY_DEADLINE: Duration = Duration::from_secs(35);

/// Bounds every route already in `router` on `limit`: a response not
/// produced by the deadline is abandoned and answered with 408 instead.
///
/// The WebSocket upgrade routes are deliberately left outside this layer
/// by their feature modules: an upgrade answers immediately and the
/// session then lives as long as the client stays connected.
pub(crate) fn with_deadline<S>(router: Router<S>, limit: Duration) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    router.layer(axum::middleware::from_fn(
        move |request: Request, next: Next| async move {
            let uri = request.uri().clone();
            match tokio::time::timeout(limit, next.run(request)).await {
                Ok(response) => response,
                Err(_elapsed) => {
                    tracing::warn!(%uri, ?limit, "request deadline elapsed");
                    StatusCode::REQUEST_TIMEOUT.into_response()
                }
            }
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::body::Body;
    use axum::routing::get;
    use tower::ServiceExt;

    use crate::app::fixtures::body_bytes;

    #[tokio::test(start_paused = true)]
    async fn a_stalled_route_answers_408_at_its_deadline() {
        // The handler stalls far past the deadline; the layer must answer
        // for it rather than let the caller hang. Time is paused, so the
        // stall and the deadline advance virtually and cost no wall clock;
        // the socketless oneshot does no real I/O that paused time would
        // freeze.
        let app = with_deadline(
            Router::new().route(
                "/stalled",
                get(|| async {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    "unreachable"
                }),
            ),
            Duration::from_millis(50),
        );
        let request = Request::builder()
            .uri("/stalled")
            .body(Body::empty())
            .expect("static request parts are valid");
        let response = app
            .oneshot(request)
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
    }

    #[tokio::test]
    async fn a_prompt_route_passes_through_its_deadline_untouched() {
        let app = with_deadline(
            Router::new().route("/quick", get(|| async { "ok" })),
            DEFAULT_DEADLINE,
        );
        let request = Request::builder()
            .uri("/quick")
            .body(Body::empty())
            .expect("static request parts are valid");
        let response = app
            .oneshot(request)
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(&body_bytes(response).await[..], b"ok");
    }

    #[test]
    fn the_relay_deadline_outlasts_the_gateway_request_timeout() {
        assert!(
            RELAY_DEADLINE > crate::gateway::REQUEST_TIMEOUT,
            "the route deadline must let the gateway client time out first, \
             so the caller sees the relay's 502 rather than a blunt 408"
        );
    }
}
