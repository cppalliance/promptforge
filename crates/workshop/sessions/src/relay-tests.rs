use super::*;

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, header};
use axum::routing::get;
use tower::ServiceExt as _;
use workshop_gateway::{GatewayBinding, GatewayHandles, GatewayHealth};
use workshop_registry::{Registration, Registry};

use crate::state::routes;

const CATALOG: &str = r#"{"object":"list","data":[{"id":"test-model","object":"model","created":1,"owned_by":"promptforge"}]}"#;
const UPSTREAM_ERROR: &str =
    r#"{"error":{"message":"model unloaded","code":"upstream_unavailable"}}"#;

/// Collects a response body already buffered in memory.
async fn body_bytes(response: Response) -> axum::body::Bytes {
    axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("the body is in memory already")
}

/// Binds `app` as a mock gateway on a free loopback port and returns its
/// base URL.
async fn spawn_gateway(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock gateway");
    let addr = listener.local_addr().expect("mock gateway address");
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("mock gateway serves");
    });
    format!("http://{addr}")
}

/// Builds the sessions route state against a stub gateway address: the
/// gateway handle set registered into the registry the state reads it
/// through, the push sinks unregistered (pushes are graceful no-ops),
/// the state directory a fresh tempdir returned alongside so it outlives
/// the test.
fn state_for(base_url: &str) -> (SessionsState, tempfile::TempDir, Registration) {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let registry = Registry::new();
    let gateway = GatewayBinding::new(base_url, "test-key").expect("the binding builds");
    let guard = workshop_gateway::register(
        &registry,
        GatewayHandles::new(gateway, GatewayHealth::new()),
    );
    let state = SessionsState::new(registry, |_| true);
    (state, dir, guard)
}

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
    let (state, _dir, _guard) = state_for(&base_url);
    let response = routes(state)
        .oneshot(models_request())
        .await
        .expect("the router is infallible");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(&body_bytes(response).await[..], CATALOG.as_bytes());
}

#[tokio::test]
async fn gateway_error_status_is_relayed_byte_for_byte() {
    let base_url = spawn_gateway(Router::new().route("/v1/models", get(mock_broken_models))).await;
    let (state, _dir, _guard) = state_for(&base_url);
    let response = routes(state)
        .oneshot(models_request())
        .await
        .expect("the router is infallible");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(&body_bytes(response).await[..], UPSTREAM_ERROR.as_bytes());
}

#[tokio::test]
async fn unreachable_gateway_becomes_bad_gateway() {
    // Port 1 is never listening, so the connect fails deterministically.
    let (state, _dir, _guard) = state_for("http://127.0.0.1:1");
    let response = routes(state)
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
    let (state, _dir, _guard) = state_for("http://127.0.0.1:1");
    state
        .health()
        .expect("the gateway handles are registered")
        .publish(false);
    let response = routes(state)
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
