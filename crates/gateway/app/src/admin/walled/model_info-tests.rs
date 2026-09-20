use std::net::SocketAddr;
use std::path::Path;

use gateway_config::Config;

use crate::test_support::serve;

/// A profile rooting the artifact cache at `cache_dir`.
fn cache_config(cache_dir: &Path) -> Config {
    Config::from_toml_str(&format!(
        r#"
config-version = 0

[server]
bind = "127.0.0.1:0"
api_key = "test-token"
# Strict bearer auth: the tests below pin that a missing key is refused.
trust_loopback = false

[local]
cache_dir = '{cache_dir}'
"#,
        cache_dir = cache_dir.display(),
    ))
    .expect("the fixture profile parses")
}

/// Appends a GGUF string (u64 LE length + bytes) to `out`.
fn push_string(out: &mut Vec<u8>, value: &str) {
    out.extend_from_slice(&(value.len() as u64).to_le_bytes());
    out.extend_from_slice(value.as_bytes());
}

/// A minimal GGUF header with a known architecture, block count, and
/// declared parameter count.
fn synthetic_gguf() -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"GGUF");
    out.extend_from_slice(&3u32.to_le_bytes());
    out.extend_from_slice(&0u64.to_le_bytes());
    out.extend_from_slice(&3u64.to_le_bytes());
    push_string(&mut out, "general.architecture");
    out.extend_from_slice(&8u32.to_le_bytes());
    push_string(&mut out, "llama");
    push_string(&mut out, "llama.block_count");
    out.extend_from_slice(&4u32.to_le_bytes());
    out.extend_from_slice(&32u32.to_le_bytes());
    push_string(&mut out, "general.parameter_count");
    out.extend_from_slice(&10u32.to_le_bytes());
    out.extend_from_slice(&8_030_000_000u64.to_le_bytes());
    out
}

/// GETs `/admin/model-info` for `path` with the given bearer token.
async fn get_model_info(addr: SocketAddr, path: &str, token: &str) -> reqwest::Response {
    reqwest::Client::new()
        .get(format!("http://{addr}/admin/model-info"))
        .query(&[("path", path)])
        .bearer_auth(token)
        .send()
        .await
        .expect("the request sends")
}

#[tokio::test]
async fn admin_model_info_reports_header_facts() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let models = temp.path().join("models");
    std::fs::create_dir_all(&models).expect("mkdir models");
    std::fs::write(models.join("tiny.gguf"), synthetic_gguf()).expect("write fixture");

    let addr = serve(cache_config(temp.path())).await;
    let response = get_model_info(addr, "models/tiny.gguf", "test-token").await;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = response.json().await.expect("a JSON body");
    assert_eq!(
        body,
        serde_json::json!({
            "architecture": "llama",
            "layer_count": 32,
            "parameter_count": 8_030_000_000u64,
            "chat_template": null,
        })
    );
}

#[tokio::test]
async fn admin_model_info_rejects_a_malformed_file_cleanly() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let models = temp.path().join("models");
    std::fs::create_dir_all(&models).expect("mkdir models");
    std::fs::write(models.join("junk.gguf"), b"not a gguf at all").expect("write junk");

    let addr = serve(cache_config(temp.path())).await;
    let response = get_model_info(addr, "models/junk.gguf", "test-token").await;
    assert_eq!(response.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);
    let body: serde_json::Value = response.json().await.expect("a JSON error envelope");
    assert_eq!(body["error"]["code"], "model_info_error");
}

#[tokio::test]
async fn admin_model_info_rejects_paths_escaping_the_cache() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    std::fs::create_dir_all(temp.path().join("models")).expect("mkdir models");
    let outside = temp.path().join("..").join("outside.gguf");

    let addr = serve(cache_config(temp.path())).await;
    for escape in ["../outside.gguf", &outside.display().to_string()] {
        let response = get_model_info(addr, escape, "test-token").await;
        assert_eq!(
            response.status(),
            reqwest::StatusCode::BAD_REQUEST,
            "path `{escape}` must be refused at the boundary"
        );
        let body: serde_json::Value = response.json().await.expect("a JSON error envelope");
        assert_eq!(body["error"]["code"], "malformed_request");
    }
}

#[tokio::test]
async fn admin_model_info_rejects_a_missing_path_in_the_error_envelope() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let addr = serve(cache_config(temp.path())).await;

    let response = reqwest::Client::new()
        .get(format!("http://{addr}/admin/model-info"))
        .bearer_auth("test-token")
        .send()
        .await
        .expect("the request sends");
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let body: serde_json::Value = response.json().await.expect("a JSON error envelope");
    assert_eq!(body["error"]["code"], "malformed_request");
}

#[tokio::test]
async fn admin_model_info_requires_bearer_auth() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let addr = serve(cache_config(temp.path())).await;

    let unauthenticated = reqwest::Client::new()
        .get(format!("http://{addr}/admin/model-info"))
        .query(&[("path", "models/tiny.gguf")])
        .send()
        .await
        .expect("the request sends");
    assert_eq!(
        unauthenticated.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "a request without a bearer token is refused"
    );

    let wrong_key = get_model_info(addr, "models/tiny.gguf", "wrong-token").await;
    assert_eq!(
        wrong_key.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "a request with the wrong bearer token is refused"
    );
}
