//! The `/v1/cache` routes through the real gateway: listing, download with
//! SSE progress, and removal, against a tempdir cache root.

use std::fmt::Write as _;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::StatusCode;
use axum::http::header::CONTENT_TYPE;
use axum::response::Response;
use axum::routing::get;
use promptforge_gateway::{Config, Gateway, ProfilesContext};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use crate::support::{TestServer, json_within, parse_sse, send_within, spawn_backend, text_within};

/// Starts the gateway with `[local].cache_dir` rooted at `cache_dir`.
///
/// The TOML literal string (single quotes) keeps Windows backslashes verbatim.
async fn cache_gateway(cache_dir: &Path) -> TestServer {
    let toml = format!(
        r#"
config-version = 2

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

/// Shared state for the fake file server: the body to serve and a hit count.
struct FileServerState {
    body: Vec<u8>,
    requests: AtomicUsize,
}

/// Serves `body` at `/model.bin` with an accurate Content-Length.
async fn fake_file_server(body: &[u8]) -> (SocketAddr, Arc<FileServerState>) {
    async fn file(State(state): State<Arc<FileServerState>>) -> Response {
        state.requests.fetch_add(1, Ordering::AcqRel);
        Response::new(Body::from(state.body.clone()))
    }
    let state = Arc::new(FileServerState {
        body: body.to_owned(),
        requests: AtomicUsize::new(0),
    });
    let router = Router::new()
        .route("/model.bin", get(file))
        .with_state(Arc::clone(&state));
    (spawn_backend(router).await, state)
}

/// Every regular file under `root`, recursively.
fn all_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                stack.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files
}

#[tokio::test]
async fn post_cache_streams_progress_then_ready_and_caches_the_blob() {
    let body = b"sse-cache-fixture-bytes";
    let digest = hex_sha256(body);
    let (file_addr, file_server) = fake_file_server(body).await;
    let temp = TempDir::new().unwrap();
    let gateway = cache_gateway(temp.path()).await;
    let http = reqwest::Client::new();
    let source = format!("http://{file_addr}/model.bin");

    let response = send_within(
        http.post(format!("http://{}/v1/cache", gateway.addr))
            .bearer_auth("test-token")
            .json(&serde_json::json!({ "source": source, "sha256": digest })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(CONTENT_TYPE).unwrap(),
        "text/event-stream"
    );
    let text = text_within(response).await;
    let events = parse_sse(&text);
    assert!(
        events.len() >= 2,
        "expected progress plus ready: {events:?}"
    );

    let (terminal, progress) = events.split_last().unwrap();
    let mut last_bytes = 0;
    for event in progress {
        assert_eq!(event["status"], "downloading");
        let bytes = event["bytes"].as_u64().expect("bytes");
        assert!(bytes >= last_bytes, "bytes must not regress: {events:?}");
        last_bytes = bytes;
        assert_eq!(event["total"], body.len() as u64);
    }
    assert_eq!(last_bytes, body.len() as u64);

    assert_eq!(terminal["status"], "ready");
    let path = PathBuf::from(terminal["path"].as_str().expect("path"));
    assert_eq!(std::fs::read(&path).expect("read blob"), body);
    let sidecar: Value = serde_json::from_str(
        &std::fs::read_to_string(format!("{}.meta.json", path.display())).expect("sidecar"),
    )
    .expect("parse sidecar");
    assert_eq!(sidecar["source"], source);
    assert_eq!(sidecar["sha256"], digest);
    assert_eq!(sidecar["size_bytes"], body.len() as u64);
    assert_eq!(
        file_server.requests.load(Ordering::Acquire),
        1,
        "exactly one download"
    );

    // A second POST for the same source is an immediate JSON cache hit.
    let response = send_within(
        http.post(format!("http://{}/v1/cache", gateway.addr))
            .bearer_auth("test-token")
            .json(&serde_json::json!({ "source": source, "sha256": digest })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(CONTENT_TYPE).unwrap(),
        "application/json"
    );
    let hit = json_within(response).await;
    assert_eq!(hit["status"], "ready");
    assert_eq!(hit["path"], serde_json::json!(path));
    assert_eq!(
        file_server.requests.load(Ordering::Acquire),
        1,
        "a cache hit must not re-download"
    );
    gateway.shutdown().await;
}

#[tokio::test]
async fn post_cache_digest_mismatch_streams_an_error_event() {
    let body = b"real-bytes-wrong-pin";
    let (file_addr, _state) = fake_file_server(body).await;
    let temp = TempDir::new().unwrap();
    let gateway = cache_gateway(temp.path()).await;
    let source = format!("http://{file_addr}/model.bin");

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/cache", gateway.addr))
            .bearer_auth("test-token")
            .json(&serde_json::json!({ "source": source, "sha256": "0".repeat(64) })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(CONTENT_TYPE).unwrap(),
        "text/event-stream"
    );
    let text = text_within(response).await;
    let events = parse_sse(&text);
    let terminal = events.last().expect("a terminal event");
    assert_eq!(terminal["status"], "error");
    assert!(
        terminal["message"]
            .as_str()
            .expect("message")
            .contains("mismatch"),
        "terminal event: {terminal}"
    );

    // No blob, sidecar, or staging file survives a failed publication; only
    // the artifact lock file remains under the cache root.
    let left = all_files(temp.path());
    assert!(
        left.iter()
            .all(|path| path.parent().is_some_and(|dir| dir.ends_with(".locks"))),
        "only lock files may remain: {left:?}"
    );
    gateway.shutdown().await;
}

#[tokio::test]
async fn post_cache_validates_source_and_pin_before_downloading() {
    let body = b"never-served";
    let (file_addr, file_server) = fake_file_server(body).await;
    let temp = TempDir::new().unwrap();
    let gateway = cache_gateway(temp.path()).await;
    let http = reqwest::Client::new();
    let url = format!("http://{}/v1/cache", gateway.addr);

    for source in [
        "not a url",
        "ftp://example.com/f.bin",
        "https://example.com/",
    ] {
        let response = send_within(
            http.post(&url)
                .bearer_auth("test-token")
                .json(&serde_json::json!({ "source": source })),
        )
        .await;
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "source {source}"
        );
        let envelope = json_within(response).await;
        assert_eq!(envelope["error"]["code"], "malformed_request");
    }

    let response = send_within(http.post(&url).bearer_auth("test-token").json(
        &serde_json::json!({ "source": format!("http://{file_addr}/model.bin"), "sha256": "abc" }),
    ))
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let envelope = json_within(response).await;
    assert_eq!(envelope["error"]["code"], "malformed_request");

    assert_eq!(
        file_server.requests.load(Ordering::Acquire),
        0,
        "validation failures must not reach the network"
    );
    gateway.shutdown().await;
}

#[tokio::test]
async fn post_cache_requires_auth() {
    let temp = TempDir::new().unwrap();
    let gateway = cache_gateway(temp.path()).await;
    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/cache", gateway.addr))
            .json(&serde_json::json!({ "source": "http://127.0.0.1:9/model.bin" })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    gateway.shutdown().await;
}

#[tokio::test]
async fn delete_cache_removes_the_entry_and_then_404s() {
    let temp = TempDir::new().unwrap();
    let body = b"delete-route-fixture";
    let digest = hex_sha256(body);
    seed_blob(
        temp.path(),
        "dddddddddddddddd",
        "d.bin",
        "http://seeded.example/d.bin",
        body,
    );
    let gateway = cache_gateway(temp.path()).await;
    let http = reqwest::Client::new();

    let response = send_within(
        http.delete(format!("http://{}/v1/cache/{digest}", gateway.addr))
            .bearer_auth("test-token"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let deleted = json_within(response).await;
    assert_eq!(deleted["status"], "deleted");
    assert_eq!(deleted["sha256"], digest);

    // Blob and sidecar are gone from disk and from the listing.
    let blob = temp
        .path()
        .join("models")
        .join("dddddddddddddddd")
        .join("d.bin");
    assert!(!blob.exists(), "blob removed");
    assert!(
        !blob.with_file_name("d.bin.meta.json").exists(),
        "sidecar removed"
    );
    let listing = json_within(
        send_within(
            http.get(format!("http://{}/v1/cache", gateway.addr))
                .bearer_auth("test-token"),
        )
        .await,
    )
    .await;
    assert_eq!(listing.as_array().expect("array").len(), 0);

    // A second delete of the same digest is a 404.
    let response = send_within(
        http.delete(format!("http://{}/v1/cache/{digest}", gateway.addr))
            .bearer_auth("test-token"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let envelope = json_within(response).await;
    assert_eq!(envelope["error"]["code"], "cache_entry_not_found");

    // A malformed digest parameter is a 400.
    let response = send_within(
        http.delete(format!("http://{}/v1/cache/not-hex", gateway.addr))
            .bearer_auth("test-token"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let envelope = json_within(response).await;
    assert_eq!(envelope["error"]["code"], "malformed_request");
    gateway.shutdown().await;
}

#[tokio::test]
async fn delete_cache_requires_auth() {
    let temp = TempDir::new().unwrap();
    let gateway = cache_gateway(temp.path()).await;
    let response = send_within(reqwest::Client::new().delete(format!(
        "http://{}/v1/cache/{}",
        gateway.addr,
        "a".repeat(64)
    )))
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    gateway.shutdown().await;
}
