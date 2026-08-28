//! Routes for voice capture: the `/voice` WebSocket session and the GPU
//! capability probe.

use axum::Router;
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::get;

use crate::app::AppState;
use crate::deadline::{DEFAULT_DEADLINE, with_deadline};
use crate::voice;

/// The voice routes. They take the whole [`AppState`]: the session reaches
/// the engine slot, the status bus, and the tape. The capability probe is
/// local and instant, so it carries the default deadline; `/voice` is
/// added after the layer and carries none - the upgrade answers
/// immediately and the session then outlives any deadline.
pub(crate) fn routes(state: AppState) -> Router {
    with_deadline(
        Router::new().route("/voice/capability", get(voice_capability)),
        DEFAULT_DEADLINE,
    )
    .route("/voice", get(voice::upgrade))
    .with_state(state)
}

/// Reports whether voice transcription can run on the GPU, so the UI can
/// hide the mic rather than offer a take that stalls on a CPU pass.
async fn voice_capability() -> impl IntoResponse {
    let gpu = crate::transcribe::gpu_transcription_available();
    (
        [(header::CONTENT_TYPE, "application/json")],
        format!(r#"{{"gpu":{gpu}}}"#),
    )
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use crate::app::fixtures::{body_bytes, state_for};
    use crate::app::router;

    /// A plain GET to `/voice` without upgrade headers is rejected with 400,
    /// which proves the route is mounted; the full WebSocket session flow is
    /// covered by the `voice` module's own tests over a live socket.
    #[tokio::test]
    async fn voice_route_rejects_a_non_upgrade_get() {
        let (state, _tape_dir) = state_for("http://127.0.0.1:1");
        let request = Request::builder()
            .uri("/voice")
            .body(Body::empty())
            .expect("static request parts are valid");
        let response = router(state)
            .oneshot(request)
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn voice_capability_reports_the_build() {
        let (state, _tape_dir) = state_for("http://127.0.0.1:1");
        let request = Request::builder()
            .uri("/voice/capability")
            .body(Body::empty())
            .expect("request builds");
        let response = router(state)
            .oneshot(request)
            .await
            .expect("the route answers");
        assert_eq!(response.status(), StatusCode::OK);
        let expected = crate::transcribe::gpu_transcription_available();
        assert_eq!(
            &body_bytes(response).await[..],
            format!(r#"{{"gpu":{expected}}}"#).as_bytes()
        );
    }
}
