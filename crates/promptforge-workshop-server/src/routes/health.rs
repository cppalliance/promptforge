//! The liveness probe route.

use axum::Router;
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::get;

/// The health probe route, stateless by design so it answers even while
/// every backing service is degraded.
pub(crate) fn routes() -> Router {
    Router::new().route("/health", get(health))
}

/// Answers the health probe with a static JSON body.
async fn health() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/json")],
        r#"{"status":"serving"}"#,
    )
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use tower::ServiceExt;

    use crate::app::fixtures::{body_bytes, state_for};
    use crate::app::router;

    #[tokio::test]
    async fn health_returns_serving() {
        let (state, _tape_dir) = state_for("http://127.0.0.1:1");
        let request = Request::builder()
            .uri("/health")
            .body(Body::empty())
            .expect("static request parts are valid");
        let response = router(state)
            .oneshot(request)
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::OK);
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .expect("the health handler sets content-type");
        assert_eq!(content_type, "application/json");
        assert_eq!(&body_bytes(response).await[..], br#"{"status":"serving"}"#);
    }
}
