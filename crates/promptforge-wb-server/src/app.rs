//! The axum router, handlers, and serving loop for the workbench server.

use axum::Router;
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::get;

/// Address the server binds to when no override is given.
pub const DEFAULT_ADDR: &str = "127.0.0.1:7910";

/// Returns the workbench server router with every route mounted.
pub fn router() -> Router {
    Router::new().route("/health", get(health))
}

/// Binds to [`DEFAULT_ADDR`] and serves until the process is stopped.
///
/// # Errors
/// Returns `std::io::Error` if the bind fails or the server stops with an
/// error.
pub async fn run() -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(DEFAULT_ADDR).await?;
    axum::serve(listener, router()).await
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
    use super::*;

    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[test]
    fn default_bind_is_loopback_port_7910() {
        assert_eq!(DEFAULT_ADDR, "127.0.0.1:7910");
    }

    #[tokio::test]
    async fn health_returns_serving() {
        let request = Request::builder()
            .uri("/health")
            .body(Body::empty())
            .expect("static request parts are valid");
        let response = router()
            .oneshot(request)
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::OK);
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .expect("the health handler sets content-type");
        assert_eq!(content_type, "application/json");
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("the body is in memory already");
        assert_eq!(&body[..], br#"{"status":"serving"}"#);
    }
}
