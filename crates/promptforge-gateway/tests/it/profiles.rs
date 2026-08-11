//! Admin profile routes: switch, list, status, and failure isolation.

use std::fs;

use promptforge_gateway::{Config, Gateway, ProfileName, ProfilesContext};
use serde_json::Value;

use crate::support::{TestServer, catalog_ids, fake_backend};

/// Switch-profile rebuilds the catalog from a remote-only profile (no llama spawn).
#[tokio::test]
async fn switch_profile_updates_models_catalog() {
    let backend = fake_backend().await;
    let profiles = tempfile::tempdir().unwrap();

    let profile_toml = |model: &str, context: u32| {
        format!(
            r#"
[server]
bind = "127.0.0.1:0"
key = "test-token"

[[endpoint]]
id = "fake"
protocol = "openai"
base_url = "http://{backend}"
api_key = ""

[[model]]
name = "{model}"
description = "{model} catalog entry"
context = {context}
upstream = "backend-model"
endpoints = ["fake"]
"#
        )
    };
    fs::write(
        profiles.path().join("alpha.toml"),
        profile_toml("alpha-model", 8192),
    )
    .unwrap();
    fs::write(
        profiles.path().join("beta.toml"),
        profile_toml("beta-model", 4096),
    )
    .unwrap();

    let alpha = ProfileName::parse("alpha").unwrap();
    let config = Config::load_profile(profiles.path(), &alpha).unwrap();
    let context = ProfilesContext::new(Some(profiles.path().to_path_buf()), Some(alpha));
    let gateway = Gateway::from_config(&config, context).unwrap();
    let server = TestServer::start(gateway).await;
    let http = reqwest::Client::new();

    let ids = catalog_ids(&http, server.addr).await;
    assert_eq!(ids, vec!["alpha-model"]);

    let listed = http
        .get(format!("http://{}/admin/profiles", server.addr))
        .bearer_auth("test-token")
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(listed["profiles"], serde_json::json!(["alpha", "beta"]));

    let switched = http
        .post(format!("http://{}/admin/switch-profile", server.addr))
        .bearer_auth("test-token")
        .json(&serde_json::json!({ "name": "beta" }))
        .send()
        .await
        .unwrap();
    assert_eq!(switched.status().as_u16(), 200);

    let ids = catalog_ids(&http, server.addr).await;
    assert_eq!(ids, vec!["beta-model"]);

    let status = http
        .get(format!("http://{}/admin/status", server.addr))
        .bearer_auth("test-token")
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(status["profile"], "beta");
    assert_eq!(status["models"], serde_json::json!(["beta-model"]));
}

/// A failed switch (missing profile) leaves the live profile fully intact:
/// same catalog, same working bearer key (LIB-009 stable credential), and a
/// stable machine code on the 404 (IT-008).
#[tokio::test]
async fn failed_switch_leaves_live_profile_intact() {
    let backend = fake_backend().await;
    let profiles = tempfile::tempdir().unwrap();
    let alpha_toml = format!(
        r#"
[server]
bind = "127.0.0.1:0"
key = "test-token"

[[endpoint]]
id = "fake"
protocol = "openai"
base_url = "http://{backend}"
api_key = ""

[[model]]
name = "alpha-model"
description = "alpha catalog entry"
context = 8192
upstream = "backend-model"
endpoints = ["fake"]
"#
    );
    fs::write(profiles.path().join("alpha.toml"), alpha_toml).unwrap();

    let alpha = ProfileName::parse("alpha").unwrap();
    let config = Config::load_profile(profiles.path(), &alpha).unwrap();
    let context = ProfilesContext::new(Some(profiles.path().to_path_buf()), Some(alpha));
    let server = TestServer::start(Gateway::from_config(&config, context).unwrap()).await;
    let http = reqwest::Client::new();

    // Switch to a profile that does not exist: expect 404, no state change.
    let missing = http
        .post(format!("http://{}/admin/switch-profile", server.addr))
        .bearer_auth("test-token")
        .json(&serde_json::json!({ "name": "ghost" }))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status().as_u16(), 404);
    let body: Value = missing.json().await.unwrap();
    assert_eq!(
        body.pointer("/error/code").and_then(Value::as_str),
        Some("profile_not_found")
    );

    // The original catalog and bearer key still work unchanged.
    assert_eq!(catalog_ids(&http, server.addr).await, vec!["alpha-model"]);
}

/// A traversal profile name is rejected before touching the filesystem.
#[tokio::test]
async fn switch_profile_rejects_traversal_name() {
    let backend = fake_backend().await;
    let profiles = tempfile::tempdir().unwrap();
    std::fs::write(
        profiles.path().join("alpha.toml"),
        format!(
            "[server]\nbind = \"127.0.0.1:0\"\nkey = \"test-token\"\n\n[[endpoint]]\nid = \"fake\"\nprotocol = \"openai\"\nbase_url = \"http://{backend}\"\napi_key = \"\"\n\n[[model]]\nname = \"alpha-model\"\ndescription = \"alpha\"\ncontext = 8192\nupstream = \"backend-model\"\nendpoints = [\"fake\"]\n"
        ),
    )
    .unwrap();

    let alpha = ProfileName::parse("alpha").unwrap();
    let config = Config::load_profile(profiles.path(), &alpha).unwrap();
    let context = ProfilesContext::new(Some(profiles.path().to_path_buf()), Some(alpha));
    let server = TestServer::start(Gateway::from_config(&config, context).unwrap()).await;

    let response = reqwest::Client::new()
        .post(format!("http://{}/admin/switch-profile", server.addr))
        .bearer_auth("test-token")
        .json(&serde_json::json!({ "name": "../escape" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 400);
}
