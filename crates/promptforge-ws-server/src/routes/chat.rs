//! Routes for the chat relay: model and profile passthroughs, the buffered
//! `/chat` completion, and the `/ws` streaming chat session.

use axum::Router;
use axum::routing::{get, post};

use crate::app::AppState;
use crate::{chat_ws, relay};

/// The chat relay routes. They take the whole [`AppState`]: the handlers
/// reach the gateway client, the tape, the health flag, and the status and
/// catalog buses.
pub(crate) fn routes(state: AppState) -> Router {
    Router::new()
        .route("/v1/models", get(relay::models))
        .route("/profiles", get(relay::profiles))
        .route("/profiles/switch", post(relay::switch_profile))
        .route("/chat", post(relay::chat))
        .route("/ws", get(chat_ws::upgrade))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use crate::app::fixtures::state_for;
    use crate::app::router;

    /// A plain GET to `/ws` without upgrade headers is rejected with 400,
    /// which proves the route is mounted; the WebSocket chat flow is covered
    /// by the `chat_ws` module's own tests over a live socket.
    #[tokio::test]
    async fn ws_route_rejects_a_non_upgrade_get() {
        let (state, _tape_dir) = state_for("http://127.0.0.1:1");
        let request = Request::builder()
            .uri("/ws")
            .body(Body::empty())
            .expect("static request parts are valid");
        let response = router(state)
            .oneshot(request)
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
