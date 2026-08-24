//! The axum router, handlers, and serving loop for the workbench server.

use axum::Router;
use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};

use crate::config::Config;
use crate::gateway::{ChatRequest, GatewayClient, GatewayError, GatewayResponse};

/// Address the server binds to when no override is given.
pub const DEFAULT_ADDR: &str = "127.0.0.1:7910";

/// Shared handler state: the authenticated gateway client.
#[derive(Debug, Clone)]
pub struct AppState {
    gateway: GatewayClient,
}

impl AppState {
    /// Builds shared state from the loaded configuration.
    ///
    /// # Errors
    /// Returns [`GatewayError::Build`] if the HTTP client cannot be built.
    pub fn new(config: &Config) -> Result<Self, GatewayError> {
        let gateway = GatewayClient::new(&config.gateway.base_url, &config.gateway.api_key)?;
        Ok(Self { gateway })
    }
}

/// Returns the workbench server router with every route mounted.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/models", get(models))
        .route("/chat", post(chat))
        .with_state(state)
}

/// Binds to `bind` and serves until the process is stopped.
///
/// # Errors
/// Returns `std::io::Error` if the bind fails or the server stops with an
/// error.
pub async fn run(state: AppState, bind: &str) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(bind).await?;
    axum::serve(listener, router(state)).await
}

/// Answers the health probe with a static JSON body.
async fn health() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/json")],
        r#"{"status":"serving"}"#,
    )
}

/// Relays the gateway's model catalog to the caller verbatim.
async fn models(State(state): State<AppState>) -> Response {
    relay(state.gateway.list_models().await)
}

/// Forwards a chat completion to the gateway and relays the reply verbatim.
async fn chat(State(state): State<AppState>, body: String) -> Response {
    let request: ChatRequest = match serde_json::from_str(&body) {
        Ok(request) => request,
        Err(error) => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                [(header::CONTENT_TYPE, "application/json")],
                serde_json::json!({
                    "error": {
                        "message": format!("invalid chat request: {error}"),
                        "code": "bad_request",
                    }
                })
                .to_string(),
            )
                .into_response();
        }
    };
    relay(state.gateway.chat_completion(&request).await)
}

/// Turns a gateway call outcome into the workbench's HTTP response.
///
/// Success (any status) is relayed byte-for-byte; a transport failure
/// becomes `502 Bad Gateway` with a small JSON error envelope.
fn relay(result: Result<GatewayResponse, GatewayError>) -> Response {
    match result {
        Ok(upstream) => (
            upstream.status,
            [(header::CONTENT_TYPE, "application/json")],
            upstream.body,
        )
            .into_response(),
        Err(error) => (
            axum::http::StatusCode::BAD_GATEWAY,
            [(header::CONTENT_TYPE, "application/json")],
            serde_json::json!({
                "error": {
                    "message": error.to_string(),
                    "code": "gateway_unreachable",
                }
            })
            .to_string(),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::Json;
    use axum::body::{Body, to_bytes};
    use axum::http::{HeaderMap, Request, StatusCode};
    use tower::ServiceExt;

    use crate::config::{GatewayConfig, ServerConfig, TapeConfig};

    const CATALOG: &str = r#"{"object":"list","data":[{"id":"test-model","object":"model","created":1,"owned_by":"promptforge"}]}"#;
    const COMPLETION: &str = r#"{"id":"chatcmpl-1","object":"chat.completion","created":1,"model":"test-model","choices":[{"index":0,"message":{"role":"assistant","content":"pong"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}"#;
    const UPSTREAM_ERROR: &str = r#"{"error":{"message":"model unloaded","code":"upstream_unavailable"}}"#;

    fn config_for(base_url: &str) -> Config {
        Config {
            gateway: GatewayConfig {
                base_url: base_url.to_string(),
                api_key: "test-key".to_string(),
            },
            tape: TapeConfig::default(),
            server: ServerConfig::default(),
        }
    }

    fn state_for(base_url: &str) -> AppState {
        AppState::new(&config_for(base_url)).expect("client builds in tests")
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

    async fn mock_chat(headers: HeaderMap, Json(body): Json<serde_json::Value>) -> Response {
        if !authorized(&headers) {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        assert_eq!(body["model"], "test-model");
        assert!(body["messages"].is_array());
        ([(header::CONTENT_TYPE, "application/json")], COMPLETION).into_response()
    }

    async fn mock_broken_models() -> Response {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            [(header::CONTENT_TYPE, "application/json")],
            UPSTREAM_ERROR,
        )
            .into_response()
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

    async fn spawn_mock_gateway() -> String {
        spawn_gateway(
            Router::new()
                .route("/v1/models", get(mock_models))
                .route("/v1/chat/completions", post(mock_chat)),
        )
        .await
    }

    async fn spawn_broken_mock_gateway() -> String {
        spawn_gateway(Router::new().route("/v1/models", get(mock_broken_models))).await
    }

    async fn body_bytes(response: Response) -> axum::body::Bytes {
        to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("the body is in memory already")
    }

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
        let response = router(state_for("http://127.0.0.1:1"))
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

    #[tokio::test]
    async fn models_are_relayed_byte_for_byte() {
        let base_url = spawn_mock_gateway().await;
        let request = Request::builder()
            .uri("/v1/models")
            .body(Body::empty())
            .expect("static request parts are valid");
        let response = router(state_for(&base_url))
            .oneshot(request)
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(&body_bytes(response).await[..], CATALOG.as_bytes());
    }

    #[tokio::test]
    async fn chat_completions_are_relayed_byte_for_byte() {
        let base_url = spawn_mock_gateway().await;
        let request = Request::builder()
            .method("POST")
            .uri("/chat")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{"model":"test-model","messages":[{"role":"user","content":"ping"}]}"#,
            ))
            .expect("static request parts are valid");
        let response = router(state_for(&base_url))
            .oneshot(request)
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(&body_bytes(response).await[..], COMPLETION.as_bytes());
    }

    #[tokio::test]
    async fn gateway_error_status_is_relayed_byte_for_byte() {
        let base_url = spawn_broken_mock_gateway().await;
        let request = Request::builder()
            .uri("/v1/models")
            .body(Body::empty())
            .expect("static request parts are valid");
        let response = router(state_for(&base_url))
            .oneshot(request)
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(&body_bytes(response).await[..], UPSTREAM_ERROR.as_bytes());
    }

    #[tokio::test]
    async fn unreachable_gateway_becomes_bad_gateway() {
        // Port 1 is never listening, so the connect fails deterministically.
        let request = Request::builder()
            .uri("/v1/models")
            .body(Body::empty())
            .expect("static request parts are valid");
        let response = router(state_for("http://127.0.0.1:1"))
            .oneshot(request)
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let body = body_bytes(response).await;
        let json: serde_json::Value = serde_json::from_slice(&body).expect("error body is JSON");
        assert_eq!(json["error"]["code"], "gateway_unreachable");
    }

    #[tokio::test]
    async fn malformed_chat_body_is_a_bad_request() {
        let request = Request::builder()
            .method("POST")
            .uri("/chat")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"not_model":true}"#))
            .expect("static request parts are valid");
        let response = router(state_for("http://127.0.0.1:1"))
            .oneshot(request)
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
