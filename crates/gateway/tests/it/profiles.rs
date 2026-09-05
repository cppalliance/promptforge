//! Single-file profile listing, switching, persistence, and request draining.

use std::fs;
use std::time::Duration;

use futures_util::StreamExt as _;
use gateway::{Config, Gateway, ProfileName, ProfilesContext};
use gateway_config::{shadow_path, write_shadow};
use serde_json::Value;

use crate::support::{
    TestServer, catalog_ids, join_within, json_within, next_arrival, parse_sse, send_within,
    slow_fake_backend, spawn_chat, text_within,
};

fn catalog(backend: std::net::SocketAddr) -> String {
    format!(
        r#"
config-version = 2

[server]
bind = "127.0.0.1:0"
api_key = "test-token"

[[endpoint]]
id = "fake"
protocol = "openai"
base_url = "http://{backend}"
api_key = ""

[[model]]
name = "test-model"
description = "alpha"
context = 1024
upstream = "alpha"
endpoints = ["fake"]

[[model]]
name = "beta-model"
description = "beta"
context = 1024
upstream = "beta"
endpoints = ["fake"]

[[profile]]
name = "alpha"
models = ["test-model"]

[[profile]]
name = "beta"
models = ["beta-model"]
"#
    )
}

async fn profile_server(backend: std::net::SocketAddr) -> (tempfile::TempDir, TestServer) {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let path = temp.path().join("gateway.toml");
    fs::write(&path, catalog(backend)).expect("write config");
    fs::write(
        gateway_config::profile_state_path(&path),
        "active_profile = \"alpha\"\n",
    )
    .expect("write state");
    let alpha = ProfileName::parse("alpha").expect("name");
    let config = Config::from_toml_str(&catalog(backend))
        .expect("catalog parses")
        .select_profile(&alpha)
        .expect("alpha selects");
    let context = ProfilesContext::new(Some(path), Some(alpha));
    let server =
        TestServer::start(Gateway::from_config(&config, context).expect("gateway builds")).await;
    (temp, server)
}

async fn switch_events(
    http: &reqwest::Client,
    addr: std::net::SocketAddr,
    name: &str,
) -> Vec<Value> {
    let response = send_within(
        http.post(format!("http://{addr}/admin/switch-profile"))
            .bearer_auth("test-token")
            .json(&serde_json::json!({ "name": name })),
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    parse_sse(&text_within(response).await)
}

#[tokio::test]
async fn switch_uses_loaded_catalog_without_disk_reload() {
    let (backend, mut arrivals) = slow_fake_backend().await;
    let (temp, server) = profile_server(backend).await;
    let http = reqwest::Client::new();
    fs::write(temp.path().join("gateway.toml"), "not valid TOML").expect("replace disk config");

    let events = switch_events(&http, server.addr, "beta").await;

    assert_eq!(
        events.last(),
        Some(&serde_json::json!({"status": "ready", "profile": "beta"}))
    );
    assert_eq!(catalog_ids(&http, server.addr).await, ["beta-model"]);
    assert!(
        arrivals.try_recv().is_err(),
        "switching performs no inference request"
    );
    server.shutdown().await;
}

#[tokio::test]
async fn unknown_profile_is_refused_from_the_loaded_catalog() {
    let (backend, _arrivals) = slow_fake_backend().await;
    let (_temp, server) = profile_server(backend).await;
    let http = reqwest::Client::new();

    let events = switch_events(&http, server.addr, "ghost").await;

    assert_eq!(
        events,
        [
            serde_json::json!({"stage": "loading-profile"}),
            serde_json::json!({
                "status": "error",
                "message": "profile not found: ghost",
            }),
        ]
    );
    assert_eq!(catalog_ids(&http, server.addr).await, ["test-model"]);
    server.shutdown().await;
}

/// A remote-only switch from a remote-only profile has nothing to download
/// and nothing to stop, so it registers neither the `downloading-models`
/// nor the `stopping-models` stage; the stages it does emit keep their
/// names and order, and the success persists.
#[cfg(feature = "local")]
#[tokio::test]
async fn switch_preserves_stage_names_and_persists_success() {
    let (backend, _arrivals) = slow_fake_backend().await;
    let (temp, server) = profile_server(backend).await;
    let http = reqwest::Client::new();

    let events = switch_events(&http, server.addr, "beta").await;

    assert_eq!(
        events,
        [
            serde_json::json!({"stage": "loading-profile"}),
            serde_json::json!({"stage": "starting-models"}),
            serde_json::json!({"status": "ready", "profile": "beta"}),
        ]
    );
    let state =
        fs::read_to_string(temp.path().join("gateway.state.toml")).expect("read persisted state");
    assert_eq!(state, "active_profile = \"beta\"\n");
    server.shutdown().await;
}

#[tokio::test]
async fn immediate_switch_preserves_a_pending_profile_selection() {
    let (backend, _arrivals) = slow_fake_backend().await;
    let (temp, server) = profile_server(backend).await;
    let state_path = temp.path().join("gateway.state.toml");
    write_shadow(&state_path, "active_profile = \"alpha\"\n").expect("stage pending selection");
    let http = reqwest::Client::new();

    let events = switch_events(&http, server.addr, "beta").await;

    assert_eq!(
        events.last(),
        Some(&serde_json::json!({"status": "ready", "profile": "beta"}))
    );
    assert_eq!(
        fs::read_to_string(&state_path).expect("read persisted state"),
        "active_profile = \"beta\"\n"
    );
    assert_eq!(
        fs::read_to_string(shadow_path(&state_path)).expect("read pending state"),
        "active_profile = \"alpha\"\n"
    );
    server.shutdown().await;
}

#[tokio::test]
async fn switch_waits_for_an_in_flight_request() {
    let (backend, mut arrivals) = slow_fake_backend().await;
    let (_temp, server) = profile_server(backend).await;
    let http = reqwest::Client::new();
    let chat = spawn_chat(
        &http,
        &format!("http://{}/v1/chat/completions", server.addr),
    );
    let release = next_arrival(&mut arrivals).await;
    let addr = server.addr;
    let mut switch =
        tokio::spawn(async move { switch_events(&reqwest::Client::new(), addr, "beta").await });

    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut switch)
            .await
            .is_err(),
        "the switch must remain blocked while the request is in flight"
    );
    release.send(()).expect("release backend");
    let response = join_within(chat).await.expect("chat request sends");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let events = join_within(switch).await;
    assert_eq!(
        events.last(),
        Some(&serde_json::json!({"status": "ready", "profile": "beta"}))
    );
    server.shutdown().await;
}

/// A request arriving while the switch is parked in its cut-over drain
/// behind a held request does not register against the old routing; it
/// waits for the switch lock and lands on the new table. The in-process
/// test of the same name in the gateway crate pins that the wait is the
/// cut-over's, with the lock observed directly.
#[tokio::test]
async fn request_registration_waits_behind_the_switch_lock() {
    let (backend, mut arrivals) = slow_fake_backend().await;
    let (_temp, server) = profile_server(backend).await;
    let http = reqwest::Client::new();
    let alpha = spawn_chat(
        &http,
        &format!("http://{}/v1/chat/completions", server.addr),
    );
    let release_alpha = next_arrival(&mut arrivals).await;

    let response = send_within(
        http.post(format!("http://{}/admin/switch-profile", server.addr))
            .bearer_auth("test-token")
            .json(&serde_json::json!({ "name": "beta" })),
    )
    .await;
    let mut frames = response.bytes_stream();
    let mut switch_body = String::new();
    while !switch_body.contains("\"stage\":\"loading-profile\"") {
        let frame = tokio::time::timeout(crate::support::PHASE_TIMEOUT, frames.next())
            .await
            .expect("switch first stage timed out")
            .expect("switch stream ended")
            .expect("switch frame failed");
        switch_body.push_str(std::str::from_utf8(&frame).expect("switch SSE is UTF-8"));
    }

    let client = http.clone();
    let url = format!("http://{}/v1/chat/completions", server.addr);
    let beta = tokio::spawn(async move {
        client
            .post(url)
            .bearer_auth("test-token")
            .json(&serde_json::json!({
                "model": "beta-model",
                "messages": [{ "role": "user", "content": "ping" }]
            }))
            .send()
            .await
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(100), arrivals.recv())
            .await
            .is_err(),
        "a request arriving during drain must not register against old routing"
    );

    release_alpha.send(()).expect("release alpha request");
    assert_eq!(
        join_within(alpha)
            .await
            .expect("alpha request sends")
            .status(),
        reqwest::StatusCode::OK
    );
    while let Some(frame) = frames.next().await {
        let frame = frame.expect("switch frame failed");
        switch_body.push_str(std::str::from_utf8(&frame).expect("switch SSE is UTF-8"));
    }
    assert_eq!(
        parse_sse(&switch_body).last(),
        Some(&serde_json::json!({"status": "ready", "profile": "beta"}))
    );

    let release_beta = next_arrival(&mut arrivals).await;
    release_beta.send(()).expect("release beta request");
    assert_eq!(
        join_within(beta)
            .await
            .expect("beta request sends")
            .status(),
        reqwest::StatusCode::OK
    );
    server.shutdown().await;
}

#[tokio::test]
async fn profiles_list_comes_from_the_loaded_catalog() {
    let (backend, _arrivals) = slow_fake_backend().await;
    let (temp, server) = profile_server(backend).await;
    fs::write(temp.path().join("gateway.toml"), "not valid TOML").expect("replace disk config");

    let listed = json_within(
        send_within(
            reqwest::Client::new()
                .get(format!("http://{}/admin/profiles", server.addr))
                .bearer_auth("test-token"),
        )
        .await,
    )
    .await;

    assert_eq!(listed["profiles"], serde_json::json!(["alpha", "beta"]));
    server.shutdown().await;
}
