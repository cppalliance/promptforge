//! Routes for the gateway relay: the model catalog passthrough and the
//! `/ws` workshop socket. The buffered `POST /chat` completion is gone -
//! chat runs through agent sessions on `/agents/ws` - so `/chat` answers
//! 404 like any unknown API path.

use axum::Router;
use axum::routing::get;

use crate::app::AppState;
use crate::deadline::{RELAY_DEADLINE, with_deadline};
use crate::{relay, session};

/// The relay routes. They take the whole [`AppState`]: the handlers reach
/// the gateway client, the health flag, and the status and catalog buses.
/// The buffered relay route waits on a gateway call, so it carries the
/// relay deadline; `/ws` is added after the layer and carries none - the
/// upgrade answers immediately and the session then outlives any deadline.
pub(crate) fn routes(state: AppState) -> Router {
    with_deadline(
        Router::new().route("/v1/models", get(relay::models)),
        RELAY_DEADLINE,
    )
    .route("/ws", get(session::upgrade))
    .with_state(state)
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use tower::ServiceExt;

    use crate::app::fixtures::state_for;
    use crate::app::router;

    /// A plain GET to `/ws` without upgrade headers is rejected with 400,
    /// which proves the route is mounted; the WebSocket flow is covered
    /// by the integration binary's `session` modules over a live socket.
    #[tokio::test]
    async fn ws_route_rejects_a_non_upgrade_get() {
        let (state, _state_dir) = state_for("http://127.0.0.1:1");
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

    /// The excised buffered chat endpoint is gone from the router: a
    /// `POST /chat` answers 404, not a relay response.
    #[tokio::test]
    async fn post_chat_is_absent_and_answers_not_found() {
        let (state, _state_dir) = state_for("http://127.0.0.1:1");
        let request = Request::builder()
            .method("POST")
            .uri("/chat")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{"model":"test-model","messages":[{"role":"user","content":"ping"}]}"#,
            ))
            .expect("static request parts are valid");
        let response = router(state)
            .oneshot(request)
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
