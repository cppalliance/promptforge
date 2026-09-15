//! Profile listing and selection: `POST /admin/switch-profile` persists the
//! selection and reports whether a restart is needed, and a restart without
//! `--profile` boots the persisted profile.

use std::fs;

use gateway::{Config, Gateway, ProfileName, ProfilesContext};
use gateway_config::ProfileSelection;
use serde_json::Value;

use crate::support::{TestServer, catalog_ids, json_within, send_within, slow_fake_backend};

fn catalog(backend: std::net::SocketAddr) -> String {
    format!(
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
models = []

[[profile]]
name = "beta"
models = []
"#
    )
}

/// A gateway started as `--profile alpha` would be: the selection is
/// ephemeral, so no state file exists beside the config.
async fn profile_server(backend: std::net::SocketAddr) -> (tempfile::TempDir, TestServer) {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let path = temp.path().join("gateway.toml");
    fs::write(&path, catalog(backend)).expect("write config");
    let alpha = ProfileName::parse("alpha").expect("name");
    let config = Config::from_toml_str(&catalog(backend))
        .expect("catalog parses")
        .select_profile(Some(&alpha))
        .expect("alpha selects");
    let context = ProfilesContext::new(Some(path), Some(alpha));
    let server =
        TestServer::start(Gateway::from_config(&config, context).expect("gateway builds")).await;
    (temp, server)
}

/// A gateway restarted without `--profile`: the selection comes from the
/// state file, or is absent when the file is.
async fn restarted_server(temp: &tempfile::TempDir) -> TestServer {
    let path = temp.path().join("gateway.toml");
    let config =
        Config::load(&path, &ProfileSelection::default()).expect("a restart reads the config");
    let context = ProfilesContext::new(Some(path), None);
    TestServer::start(Gateway::from_config(&config, context).expect("gateway builds")).await
}

async fn switch(
    http: &reqwest::Client,
    addr: std::net::SocketAddr,
    body: Value,
) -> reqwest::Response {
    send_within(
        http.post(format!("http://{addr}/admin/switch-profile"))
            .bearer_auth("test-token")
            .json(&body),
    )
    .await
}

async fn running_profile(http: &reqwest::Client, addr: std::net::SocketAddr) -> Value {
    let mut status = json_within(
        send_within(
            http.get(format!("http://{addr}/admin/status"))
                .bearer_auth("test-token"),
        )
        .await,
    )
    .await;
    status["profile"].take()
}

#[tokio::test]
async fn switch_uses_loaded_catalog_without_disk_reload() {
    let (backend, mut arrivals) = slow_fake_backend().await;
    let (temp, server) = profile_server(backend).await;
    let http = reqwest::Client::new();
    fs::write(temp.path().join("gateway.toml"), "not valid TOML").expect("replace disk config");

    let response = switch(&http, server.addr, serde_json::json!({ "name": "beta" })).await;

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(
        json_within(response).await,
        serde_json::json!({ "profile": "beta", "restart_required": true })
    );
    assert_eq!(
        catalog_ids(&http, server.addr).await,
        ["test-model", "beta-model"],
        "every remote model is served under any profile"
    );
    assert!(
        arrivals.try_recv().is_err(),
        "switching performs no inference request"
    );
    server.shutdown().await;
}

#[tokio::test]
async fn unknown_profile_is_refused_from_the_loaded_catalog() {
    let (backend, _arrivals) = slow_fake_backend().await;
    let (temp, server) = profile_server(backend).await;
    let http = reqwest::Client::new();

    let response = switch(&http, server.addr, serde_json::json!({ "name": "ghost" })).await;

    assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
    let body = json_within(response).await;
    assert_eq!(body["error"]["code"], "profile_not_found");
    let message = body["error"]["message"]
        .as_str()
        .expect("the message is a string");
    assert!(
        message.contains("ghost"),
        "names the refused profile: {message}"
    );
    assert!(
        message.contains("alpha, beta"),
        "names the defined profiles: {message}"
    );
    assert!(
        !temp.path().join("gateway.state.toml").exists(),
        "a refused switch persists nothing"
    );
    assert_eq!(
        catalog_ids(&http, server.addr).await,
        ["test-model", "beta-model"]
    );
    server.shutdown().await;
}

/// The switch persists the selection and leaves the running process on its
/// boot profile; the next start, without `--profile`, boots the persisted
/// one.
#[tokio::test]
async fn a_switch_persists_and_a_restart_without_a_profile_flag_boots_it() {
    let (backend, _arrivals) = slow_fake_backend().await;
    let (temp, server) = profile_server(backend).await;
    let http = reqwest::Client::new();
    let state_path = temp.path().join("gateway.state.toml");
    assert!(!state_path.exists(), "--profile alone persists nothing");

    let response = switch(&http, server.addr, serde_json::json!({ "name": "beta" })).await;

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(
        json_within(response).await,
        serde_json::json!({ "profile": "beta", "restart_required": true })
    );
    assert_eq!(
        fs::read_to_string(&state_path).expect("read persisted state"),
        "active_profile = \"beta\"\n"
    );
    assert_eq!(
        running_profile(&http, server.addr).await,
        "alpha",
        "the running process stays on its boot profile until it restarts"
    );
    server.shutdown().await;

    let restarted = restarted_server(&temp).await;
    assert_eq!(
        running_profile(&http, restarted.addr).await,
        "beta",
        "a restart without --profile boots the persisted selection"
    );
    assert_eq!(
        catalog_ids(&http, restarted.addr).await,
        ["test-model", "beta-model"]
    );
    restarted.shutdown().await;
}

/// `{"name": null}` deletes the state file, the persisted form of "no
/// profile"; the next start serves the remote catalog with no profile.
#[tokio::test]
async fn selecting_no_profile_deletes_the_state_and_a_restart_boots_without_one() {
    let (backend, _arrivals) = slow_fake_backend().await;
    let (temp, server) = profile_server(backend).await;
    let http = reqwest::Client::new();
    let state_path = temp.path().join("gateway.state.toml");
    let response = switch(&http, server.addr, serde_json::json!({ "name": "beta" })).await;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert!(state_path.exists(), "the first switch persisted beta");

    let response = switch(&http, server.addr, serde_json::json!({ "name": null })).await;

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(
        json_within(response).await,
        serde_json::json!({ "profile": null, "restart_required": true })
    );
    assert!(!state_path.exists(), "no profile is persisted as no file");
    assert_eq!(running_profile(&http, server.addr).await, "alpha");
    server.shutdown().await;

    let restarted = restarted_server(&temp).await;
    assert!(
        running_profile(&http, restarted.addr).await.is_null(),
        "a restart with no state file boots no profile"
    );
    assert_eq!(
        catalog_ids(&http, restarted.addr).await,
        ["test-model", "beta-model"],
        "remote models serve with no profile selected"
    );
    let response = switch(&http, restarted.addr, serde_json::json!({ "name": null })).await;
    assert_eq!(
        json_within(response).await,
        serde_json::json!({ "profile": null, "restart_required": false }),
        "selecting no profile while none runs needs no restart"
    );
    restarted.shutdown().await;
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
