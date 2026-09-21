use gateway_config::{Config, ProfileSelection, profile_state_path, shadow_path};

use crate::test_support::{AdminPaths, serve_with_paths};

const CONFIG: &str = r#"
config-version = 0

[server]
bind = "127.0.0.1:0"
api_key = "test-token"

[[endpoint]]
id = "fake"
protocol = "openai"
base_url = "http://127.0.0.1:9"
api_key = ""

[[model]]
name = "alpha-model"
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
"#;

fn fixture() -> (tempfile::TempDir, Config, AdminPaths) {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let config_path = temp.path().join("gateway.toml");
    std::fs::write(&config_path, CONFIG).expect("write config");
    std::fs::write(
        profile_state_path(&config_path),
        "active_profile = \"alpha\"\n",
    )
    .expect("write state");
    let config = Config::load(&config_path, &ProfileSelection::default()).expect("load config");
    let paths = AdminPaths {
        fixture_dir: temp.path().to_path_buf(),
        active: "alpha".to_owned(),
        config_path,
    };
    (temp, config, paths)
}

/// The live config as a save body: `GET /admin/config` also reports the
/// running `active_profile`, which is not a configuration key and never
/// goes back in a save.
async fn save_body(addr: std::net::SocketAddr) -> serde_json::Value {
    let mut body: serde_json::Value = reqwest::Client::new()
        .get(format!("http://{addr}/admin/config"))
        .bearer_auth("test-token")
        .send()
        .await
        .expect("get sends")
        .json()
        .await
        .expect("config json");
    body.as_object_mut()
        .expect("the config is an object")
        .remove("active_profile");
    body
}

async fn put_config(addr: std::net::SocketAddr, body: &serde_json::Value) -> reqwest::Response {
    reqwest::Client::new()
        .put(format!("http://{addr}/admin/config"))
        .bearer_auth("test-token")
        .json(body)
        .send()
        .await
        .expect("put sends")
}

#[tokio::test]
async fn a_save_setting_active_profile_is_rejected_and_stages_nothing() {
    let (_temp, config, paths) = fixture();
    let config_path = paths.config_path.clone();
    let addr = serve_with_paths(config, paths).await;
    let mut body = save_body(addr).await;
    body["active_profile"] = serde_json::json!("beta");

    let response = put_config(addr, &body).await;

    assert_eq!(response.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);
    let error: serde_json::Value = response.json().await.expect("error envelope");
    assert_eq!(error["error"]["code"], "config_write_rejected");
    assert!(
        error["error"]["message"].as_str().is_some_and(|message| {
            message.contains("active_profile is not a configuration key")
                && message.contains("POST /admin/switch-profile")
        }),
        "the message names the switch route: {error}"
    );
    assert!(!shadow_path(&config_path).exists());
    assert!(!shadow_path(&profile_state_path(&config_path)).exists());
}

#[tokio::test]
async fn a_save_replies_with_the_config_shadow_alone() {
    let (_temp, config, paths) = fixture();
    let config_path = paths.config_path.clone();
    let addr = serve_with_paths(config, paths).await;
    let mut body = save_body(addr).await;
    body["model"][0]["description"] = serde_json::json!("edited");

    let response = put_config(addr, &body).await;

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let reply: serde_json::Value = response.json().await.expect("save reply");
    assert_eq!(
        reply,
        serde_json::json!({ "shadow": shadow_path(&config_path).display().to_string() }),
        "the reply names only the config shadow"
    );
    assert!(shadow_path(&config_path).is_file());
    assert!(!shadow_path(&profile_state_path(&config_path)).exists());
}
