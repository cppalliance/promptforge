//! The `/v1/cache` routes through the real gateway: listing, download with
//! SSE progress, and removal, against a tempdir cache root.

use std::fmt::Write as _;
use std::path::Path;

use axum::http::StatusCode;
use promptforge_gateway::{Config, Gateway, ProfilesContext};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use crate::support::{TestServer, json_within, send_within};

/// Starts the gateway with `[local].cache_dir` rooted at `cache_dir`.
///
/// The TOML literal string (single quotes) keeps Windows backslashes verbatim.
async fn cache_gateway(cache_dir: &Path) -> TestServer {
    let toml = format!(
        r#"
[server]
bind = "127.0.0.1:0"
api_key = "test-token"

[local]
cache_dir = '{cache_dir}'

[[endpoint]]
id = "fake"
protocol = "openai"
base_url = "http://127.0.0.1:9"
api_key = ""

[[model]]
name = "test-model"
description = "a test model for integration"
context = 8192
upstream = "backend-model"
endpoints = ["fake"]
"#,
        cache_dir = cache_dir.display()
    );
    let config = Config::from_toml_str(&toml).unwrap();
    let gateway = Gateway::from_config(&config, ProfilesContext::default()).unwrap();
    TestServer::start(gateway).await
}

/// Lowercase hex SHA-256 of `bytes`.
fn hex_sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut output, "{byte:02x}").expect("writing to a String cannot fail");
    }
    output
}

/// Seeds a cache slot: the blob plus its `<file>.meta.json` sidecar.
fn seed_blob(cache_dir: &Path, key: &str, name: &str, source: &str, body: &[u8]) {
    let slot = cache_dir.join("models").join(key);
    std::fs::create_dir_all(&slot).unwrap();
    std::fs::write(slot.join(name), body).unwrap();
    let sidecar = serde_json::json!({
        "source": source,
        "sha256": hex_sha256(body),
        "size_bytes": body.len(),
    });
    std::fs::write(slot.join(format!("{name}.meta.json")), sidecar.to_string()).unwrap();
}

#[tokio::test]
async fn get_cache_lists_sidecar_bearing_blobs_only() {
    let temp = TempDir::new().unwrap();
    let body_a = b"route-list-fixture-a";
    let body_b = b"route-list-fixture-bb";
    seed_blob(
        temp.path(),
        "aaaaaaaaaaaaaaaa",
        "a.bin",
        "http://seeded.example/a.bin",
        body_a,
    );
    seed_blob(
        temp.path(),
        "bbbbbbbbbbbbbbbb",
        "b.bin",
        "http://seeded.example/b.bin",
        body_b,
    );
    // A bare blob without a sidecar is not a cache entry and is not listed.
    let bare = temp.path().join("models").join("cccccccccccccccc");
    std::fs::create_dir_all(&bare).unwrap();
    std::fs::write(bare.join("bare.bin"), b"bare").unwrap();

    let gateway = cache_gateway(temp.path()).await;
    let http = reqwest::Client::new();
    let response = send_within(
        http.get(format!("http://{}/v1/cache", gateway.addr))
            .bearer_auth("test-token"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let listing = json_within(response).await;
    let entries = listing.as_array().expect("listing is an array");
    assert_eq!(entries.len(), 2, "listing: {listing}");

    // The store sorts by source for a stable response.
    let expected_a = temp
        .path()
        .join("models")
        .join("aaaaaaaaaaaaaaaa")
        .join("a.bin");
    assert_eq!(entries[0]["source"], "http://seeded.example/a.bin");
    assert_eq!(entries[0]["path"], serde_json::json!(expected_a));
    assert_eq!(entries[0]["sha256"], hex_sha256(body_a));
    assert_eq!(entries[0]["size_bytes"], body_a.len() as u64);
    let expected_b = temp
        .path()
        .join("models")
        .join("bbbbbbbbbbbbbbbb")
        .join("b.bin");
    assert_eq!(entries[1]["source"], "http://seeded.example/b.bin");
    assert_eq!(entries[1]["path"], serde_json::json!(expected_b));
    assert_eq!(entries[1]["sha256"], hex_sha256(body_b));
    assert_eq!(entries[1]["size_bytes"], body_b.len() as u64);
    gateway.shutdown().await;
}

#[tokio::test]
async fn get_cache_requires_auth() {
    let temp = TempDir::new().unwrap();
    let gateway = cache_gateway(temp.path()).await;
    let response =
        send_within(reqwest::Client::new().get(format!("http://{}/v1/cache", gateway.addr))).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    gateway.shutdown().await;
}
