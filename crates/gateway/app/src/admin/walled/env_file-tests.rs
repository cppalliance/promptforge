use gateway_config::{Config, ProfileSelection, profile_state_path, shadow_path};

use super::render_value;
use crate::test_support::{AdminPaths, serve_with_paths};

const CONFIG: &str = r#"
config-version = 0

[server]
bind = "127.0.0.1:0"
api_key = "test-token"

[[profile]]
name = "main"
models = []
"#;

fn fixture() -> (tempfile::TempDir, Config, AdminPaths) {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let config_path = temp.path().join("gateway.toml");
    std::fs::write(&config_path, CONFIG).expect("write config");
    std::fs::write(
        profile_state_path(&config_path),
        "active_profile = \"main\"\n",
    )
    .expect("write state");
    std::fs::write(config_path.with_extension("env"), "GLOBAL=one\n").expect("write env");
    let config = Config::load(&config_path, &ProfileSelection::default()).expect("load config");
    let paths = AdminPaths {
        fixture_dir: temp.path().to_path_buf(),
        active: "main".to_owned(),
        config_path,
    };
    (temp, config, paths)
}

#[tokio::test]
async fn env_routes_expose_only_the_single_global_file() {
    let (_temp, config, paths) = fixture();
    let env_path = paths.config_path.with_extension("env");
    let addr = serve_with_paths(config, paths).await;
    let http = reqwest::Client::new();

    let get: serde_json::Value = http
        .get(format!("http://{addr}/admin/env"))
        .bearer_auth("test-token")
        .send()
        .await
        .expect("get sends")
        .json()
        .await
        .expect("env json");
    assert_eq!(get["boot"]["vars"], serde_json::json!({"GLOBAL": "one"}));
    assert!(get["profile"].is_null());

    let put = http
        .put(format!("http://{addr}/admin/env?scope=global"))
        .bearer_auth("test-token")
        .json(&serde_json::json!({"GLOBAL": "two"}))
        .send()
        .await
        .expect("put sends");
    assert_eq!(put.status(), reqwest::StatusCode::OK);
    assert!(shadow_path(&env_path).is_file());

    let profile = http
        .put(format!("http://{addr}/admin/env?scope=profile"))
        .bearer_auth("test-token")
        .json(&serde_json::json!({}))
        .send()
        .await
        .expect("profile put sends");
    assert_eq!(profile.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);
}

#[test]
fn render_value_round_trips_supported_forms() {
    assert_eq!(render_value("abc-123"), Some("abc-123".to_owned()));
    assert_eq!(render_value("two words"), Some("'two words'".to_owned()));
    assert_eq!(render_value("it's"), Some("\"it's\"".to_owned()));
    assert_eq!(render_value("a\nb"), None);
}
