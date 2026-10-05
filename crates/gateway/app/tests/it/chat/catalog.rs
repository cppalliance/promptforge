//! Models catalog route: configured entries, model kinds, capabilities, and auth.

use gateway::{Config, Gateway, ProfilesContext};
use serde_json::Value;

use crate::support::{TestServer, fake_backend, gateway_for, json_within, send_within};

#[tokio::test]
async fn models_catalog_returns_configured_entries() {
    let backend = fake_backend().await;
    let gateway = gateway_for(backend).await;

    let response = send_within(
        reqwest::Client::new()
            .get(format!("http://{}/v1/models", gateway.addr))
            .bearer_auth("test-token"),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);

    let body = json_within(response).await;
    assert_eq!(body.get("object").and_then(Value::as_str), Some("list"));
    let data = body.get("data").and_then(Value::as_array).unwrap();
    assert_eq!(data.len(), 1);
    assert_eq!(
        data[0].get("id").and_then(Value::as_str),
        Some("test-model")
    );
    assert_eq!(data[0].get("object").and_then(Value::as_str), Some("model"));
    assert_eq!(data[0].get("kind").and_then(Value::as_str), Some("chat"));
    assert_eq!(
        data[0].get("description").and_then(Value::as_str),
        Some("a test model for integration")
    );
    assert_eq!(data[0].get("context").and_then(Value::as_u64), Some(8192));
    assert_eq!(
        data[0].get("thinking").and_then(Value::as_str),
        Some("never")
    );
    gateway.shutdown().await;
}

#[tokio::test]
async fn models_catalog_reports_model_kinds() {
    let backend = fake_backend().await;
    let toml = format!(
        r#"
config-version = 0

[server]
bind = "127.0.0.1:0"
api_key = "test-token"

[[endpoint]]
id = "fake"
protocol = "openai"
base_url = "http://{backend}"
api_key = ""

[[model]]
name = "chat-model"
description = "a chat model"
context = 8192
upstream = "backend-model"
endpoints = ["fake"]

[[model]]
name = "embed-model"
kind = "embedding"
description = "an embedding model"
context = 8192
upstream = "backend-model"
endpoints = ["fake"]
"#
    );
    let config = Config::from_toml_str(&toml).unwrap();
    let gateway = Gateway::from_config(&config, ProfilesContext::default()).unwrap();
    let gateway = TestServer::start(gateway).await;

    let response = send_within(
        reqwest::Client::new()
            .get(format!("http://{}/v1/models", gateway.addr))
            .bearer_auth("test-token"),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);

    let body = json_within(response).await;
    let data = body.get("data").and_then(Value::as_array).unwrap();
    assert_eq!(data.len(), 2);
    assert_eq!(
        data[0].get("id").and_then(Value::as_str),
        Some("chat-model")
    );
    assert_eq!(data[0].get("kind").and_then(Value::as_str), Some("chat"));
    assert_eq!(
        data[1].get("id").and_then(Value::as_str),
        Some("embed-model")
    );
    assert_eq!(
        data[1].get("kind").and_then(Value::as_str),
        Some("embedding")
    );
    gateway.shutdown().await;
}

#[tokio::test]
async fn models_catalog_includes_capabilities() {
    let backend = fake_backend().await;
    let toml = format!(
        r#"
config-version = 0

[server]
bind = "127.0.0.1:0"
api_key = "test-token"

[[endpoint]]
id = "fake"
protocol = "openai"
base_url = "http://{backend}"
api_key = ""

[[model]]
name = "chat-model"
description = "a chat model"
context = 8192
thinking = "switchable"
upstream = "backend-model"
endpoints = ["fake"]
max_output = 4096
default_temperature = 0.7
images = true
parallel_tool_calls = true
effort_levels = ["low", "high"]
default_effort = "low"
adaptive_thinking = true
"#
    );
    let config = Config::from_toml_str(&toml).unwrap();
    let gateway = Gateway::from_config(&config, ProfilesContext::default()).unwrap();
    let gateway = TestServer::start(gateway).await;

    let response = send_within(
        reqwest::Client::new()
            .get(format!("http://{}/v1/models", gateway.addr))
            .bearer_auth("test-token"),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);

    let body = json_within(response).await;
    let data = body.get("data").and_then(Value::as_array).unwrap();
    assert_eq!(data.len(), 1);
    let entry = &data[0];
    assert_eq!(entry.get("max_output").and_then(Value::as_u64), Some(4096));
    assert_eq!(
        entry.get("default_temperature").and_then(Value::as_f64),
        Some(0.7)
    );
    assert_eq!(entry.get("images").and_then(Value::as_bool), Some(true));
    assert_eq!(
        entry.get("parallel_tool_calls").and_then(Value::as_bool),
        Some(true)
    );
    assert_eq!(
        entry.get("effort_levels").and_then(Value::as_array),
        Some(&vec![
            Value::String("low".to_owned()),
            Value::String("high".to_owned())
        ])
    );
    assert_eq!(
        entry.get("default_effort").and_then(Value::as_str),
        Some("low")
    );
    assert_eq!(
        entry.get("adaptive_thinking").and_then(Value::as_bool),
        Some(true)
    );
    gateway.shutdown().await;
}

#[tokio::test]
async fn models_catalog_omits_unset_optional_capabilities() {
    let backend = fake_backend().await;
    let gateway = gateway_for(backend).await;

    let response = send_within(
        reqwest::Client::new()
            .get(format!("http://{}/v1/models", gateway.addr))
            .bearer_auth("test-token"),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);

    let body = json_within(response).await;
    let data = body.get("data").and_then(Value::as_array).unwrap();
    let entry = data[0].as_object().unwrap();
    // Absent options never serialize as null; the flags default to false.
    assert!(!entry.contains_key("max_output"));
    assert!(!entry.contains_key("default_temperature"));
    assert!(!entry.contains_key("default_effort"));
    assert_eq!(entry.get("images").and_then(Value::as_bool), Some(false));
    assert_eq!(
        entry.get("parallel_tool_calls").and_then(Value::as_bool),
        Some(false)
    );
    assert_eq!(
        entry.get("adaptive_thinking").and_then(Value::as_bool),
        Some(false)
    );
    assert_eq!(
        entry.get("effort_levels").and_then(Value::as_array),
        Some(&vec![])
    );
    gateway.shutdown().await;
}

#[tokio::test]
async fn models_catalog_wrong_token_is_401() {
    let backend = fake_backend().await;
    let gateway = gateway_for(backend).await;

    let response = send_within(
        reqwest::Client::new()
            .get(format!("http://{}/v1/models", gateway.addr))
            .bearer_auth("wrong-token"),
    )
    .await;
    assert_eq!(response.status().as_u16(), 401);
    gateway.shutdown().await;
}
