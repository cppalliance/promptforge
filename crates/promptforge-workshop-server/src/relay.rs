//! The buffered gateway relay: the `/v1/models` catalog passthrough and
//! the helpers that shape gateway responses for the wire.

use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};

use crate::app::AppState;
use crate::error::AppError;
use crate::gateway::{GatewayError, GatewayResponse};
use crate::protocol::Activity;
use crate::push::Push;

/// Relays the gateway's model catalog to the caller verbatim.
///
/// While the heartbeat reports the gateway down, the catalog is not
/// attempted: the route answers 502 with a user-visible message instead.
pub(crate) async fn models(State(state): State<AppState>) -> Response {
    if !state.health().is_reachable() {
        return AppError::GatewayUnreachable.into_response();
    }
    let push = state.push();
    push.push_status_update(
        "Loading models...",
        "fetching the gateway model catalog",
        Activity::General,
    );
    let result = state.gateway.list_models().await;
    report_gateway_outcome(&push, &result, "GET /v1/models");
    relay(result)
}

/// Reports a gateway call's outcome on the status bus: back to idle on
/// success, otherwise the error label matching the failure shape.
fn report_gateway_outcome(
    push: &Push,
    result: &Result<GatewayResponse, GatewayError>,
    route: &str,
) {
    match result {
        Ok(upstream) if upstream.status.is_success() => push.push_idle(),
        Ok(upstream) => push.push_failure(
            format!("Gateway error: {}", upstream.status),
            format!("{route} answered a non-success status"),
            Activity::General,
        ),
        Err(error) => push.push_failure("Connection lost", error.to_string(), Activity::General),
    }
}

/// Parses a gateway body as JSON, falling back to a plain string.
pub(crate) fn value_from_bytes(body: &[u8]) -> serde_json::Value {
    serde_json::from_slice(body)
        .unwrap_or_else(|_| serde_json::Value::String(String::from_utf8_lossy(body).into_owned()))
}

/// Turns a gateway call outcome into the workshop's HTTP response.
///
/// Success (any status) is relayed byte-for-byte; a transport failure
/// becomes `502 Bad Gateway` through the [`AppError`] wire envelope.
pub(crate) fn relay(result: Result<GatewayResponse, GatewayError>) -> Response {
    match result {
        Ok(upstream) => (
            upstream.status,
            [(header::CONTENT_TYPE, "application/json")],
            upstream.body,
        )
            .into_response(),
        Err(error) => AppError::Gateway(error).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use axum::Router;
    use axum::body::Body;
    use axum::http::{HeaderMap, Request, StatusCode, header};
    use axum::response::{IntoResponse, Response};
    use axum::routing::get;
    use tower::ServiceExt;

    use crate::app::fixtures::{body_bytes, spawn_gateway, state_for};
    use crate::app::router;

    const CATALOG: &str = r#"{"object":"list","data":[{"id":"test-model","object":"model","created":1,"owned_by":"promptforge"}]}"#;
    const UPSTREAM_ERROR: &str =
        r#"{"error":{"message":"model unloaded","code":"upstream_unavailable"}}"#;

    fn authorized(headers: &HeaderMap) -> bool {
        headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            == Some("Bearer test-key")
    }

    async fn mock_models(headers: HeaderMap) -> Response {
        if !authorized(&headers) {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        ([(header::CONTENT_TYPE, "application/json")], CATALOG).into_response()
    }

    async fn mock_broken_models() -> Response {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            [(header::CONTENT_TYPE, "application/json")],
            UPSTREAM_ERROR,
        )
            .into_response()
    }

    fn models_request() -> Request<Body> {
        Request::builder()
            .uri("/v1/models")
            .body(Body::empty())
            .expect("static request parts are valid")
    }

    #[tokio::test]
    async fn models_are_relayed_byte_for_byte() {
        let base_url = spawn_gateway(Router::new().route("/v1/models", get(mock_models))).await;
        let (state, _state_dir) = state_for(&base_url);
        let response = router(state)
            .oneshot(models_request())
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(&body_bytes(response).await[..], CATALOG.as_bytes());
    }

    #[tokio::test]
    async fn gateway_error_status_is_relayed_byte_for_byte() {
        let base_url =
            spawn_gateway(Router::new().route("/v1/models", get(mock_broken_models))).await;
        let (state, _state_dir) = state_for(&base_url);
        let response = router(state)
            .oneshot(models_request())
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(&body_bytes(response).await[..], UPSTREAM_ERROR.as_bytes());
    }

    #[tokio::test]
    async fn unreachable_gateway_becomes_bad_gateway() {
        // Port 1 is never listening, so the connect fails deterministically.
        let (state, _state_dir) = state_for("http://127.0.0.1:1");
        let response = router(state)
            .oneshot(models_request())
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let body = body_bytes(response).await;
        let json: serde_json::Value = serde_json::from_slice(&body).expect("error body is JSON");
        assert_eq!(json["error"]["code"], "gateway_unreachable");
    }

    #[tokio::test]
    async fn a_gateway_known_down_short_circuits_the_catalog_with_bad_gateway() {
        let (state, _state_dir) = state_for("http://127.0.0.1:1");
        state.health().publish(false);
        let response = router(state)
            .oneshot(models_request())
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let body = body_bytes(response).await;
        let json: serde_json::Value = serde_json::from_slice(&body).expect("error body is JSON");
        assert_eq!(json["error"]["code"], "gateway_unreachable");
        assert_eq!(
            json["error"]["message"], "Gateway unreachable",
            "the short-circuit message is user-visible"
        );
    }
}
