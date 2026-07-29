//! End-to-end tests: a fake OpenAI backend behind the real gateway, driven by
//! the executor's real [`GatewayClient`]. This is the test that keeps the two
//! independent definitions of the wire shape honest.
#![expect(
    clippy::unwrap_used,
    reason = "test helpers panic on setup failure, which is the desired behavior"
)]

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::routing::post;
use promptforge_core::client::GatewayClient;
use promptforge_gateway::config::Config;
use promptforge_gateway::routing::Routing;
use promptforge_gateway::{AppState, build_router};
use serde_json::Value;

/// Spawn a server on an ephemeral port and return its address.
async fn spawn(router: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    addr
}

/// A fake OpenAI backend that echoes the model it was asked for and returns a
/// canned assistant message.
async fn fake_backend() -> SocketAddr {
    async fn completions(Json(body): Json<Value>) -> Json<Value> {
        let model = body
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        Json(serde_json::json!({
            "id": "cmpl-test",
            "object": "chat.completion",
            "model": model,
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "pong" },
                "finish_reason": "stop"
            }]
        }))
    }
    let router = Router::new().route("/chat/completions", post(completions));
    spawn(router).await
}

/// Start the gateway wired to the fake backend and return its address.
async fn gateway_for(backend: SocketAddr) -> SocketAddr {
    let toml = format!(
        r#"
[server]
bind = "127.0.0.1:0"
token = "test-token"

[[endpoint]]
id = "fake"
protocol = "openai"
base_url = "http://{backend}"
api_key = ""

[[model]]
name = "test-model"
upstream = "backend-model"
endpoints = ["fake"]
"#
    );
    let config = Config::from_toml_str(&toml).unwrap();
    let routing = Arc::new(Routing::from_config(&config).unwrap());
    let state = AppState::new(routing, config.server.token);
    spawn(build_router(state)).await
}

#[tokio::test]
async fn happy_path_through_the_real_client() {
    let backend = fake_backend().await;
    let gateway = gateway_for(backend).await;

    let client = GatewayClient::new(&format!("http://{gateway}/v1"), "test-token", "test-model");
    let reply = client
        .complete(&[promptforge_core::client::Message::user("ping")])
        .await
        .unwrap();
    assert_eq!(reply, "pong");
}

#[tokio::test]
async fn unknown_model_is_404() {
    let backend = fake_backend().await;
    let gateway = gateway_for(backend).await;

    let response = reqwest::Client::new()
        .post(format!("http://{gateway}/v1/chat/completions"))
        .bearer_auth("test-token")
        .json(&serde_json::json!({ "model": "nope", "messages": [] }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 404);
}

#[tokio::test]
async fn wrong_token_is_401() {
    let backend = fake_backend().await;
    let gateway = gateway_for(backend).await;

    let response = reqwest::Client::new()
        .post(format!("http://{gateway}/v1/chat/completions"))
        .bearer_auth("wrong-token")
        .json(&serde_json::json!({ "model": "test-model", "messages": [] }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 401);
}

#[tokio::test]
async fn health_needs_no_auth() {
    let backend = fake_backend().await;
    let gateway = gateway_for(backend).await;

    let response = reqwest::Client::new()
        .get(format!("http://{gateway}/health"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
}
