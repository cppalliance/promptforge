use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
use axum::http::{HeaderMap, StatusCode, Uri};
use gateway_config::{Config, Secret};

use super::HfProxy;
use crate::test_support::serve_with_hf;

/// A minimal profile: the hub proxy needs nothing beyond `[server]`.
fn hf_config() -> Config {
    Config::from_toml_str(
        r#"
config-version = 0

[server]
bind = "127.0.0.1:0"
api_key = "test-token"
# Strict bearer auth: the tests below pin that a missing key is refused.
trust_loopback = false
"#,
    )
    .expect("the fixture profile parses")
}

/// One request the stub hub observed.
#[derive(Debug, Clone)]
struct Seen {
    path: String,
    query: String,
    authorization: Option<String>,
}

/// Spawns a stub hub answering every request with `status` and `body`,
/// recording each request it sees.
async fn spawn_stub(status: StatusCode, body: &'static str) -> (String, Arc<Mutex<Vec<Seen>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&seen);
    let app = axum::Router::new().fallback(move |uri: Uri, headers: HeaderMap| {
        let recorded = Arc::clone(&recorded);
        async move {
            recorded.lock().expect("the stub log lock").push(Seen {
                path: uri.path().to_owned(),
                query: uri.query().unwrap_or("").to_owned(),
                authorization: headers
                    .get(AUTHORIZATION)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned),
            });
            (status, [(CONTENT_TYPE, "application/json")], body)
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the stub listener binds");
    let addr = listener.local_addr().expect("the stub bound address");
    tokio::spawn(async move {
        let _ignored = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), seen)
}

/// Serves the gateway with its hub proxy aimed at a fresh stub.
async fn serve_against_stub(
    status: StatusCode,
    body: &'static str,
    token: Option<&str>,
) -> (SocketAddr, Arc<Mutex<Vec<Seen>>>) {
    let (base_url, seen) = spawn_stub(status, body).await;
    let proxy = HfProxy::new(base_url, token.map(|token| Secret::new(token.to_owned())));
    let addr = serve_with_hf(hf_config(), proxy).await;
    (addr, seen)
}

/// GETs `path` on the gateway with the given bearer token.
async fn get(addr: SocketAddr, path: &str, token: &str) -> reqwest::Response {
    reqwest::Client::new()
        .get(format!("http://{addr}{path}"))
        .bearer_auth(token)
        .send()
        .await
        .expect("the request sends")
}

#[tokio::test]
async fn admin_hf_search_forwards_params_and_body() {
    let stub_body = r#"[{"id":"unsloth/Qwen3-8B-GGUF","downloads":123}]"#;
    let (addr, seen) = serve_against_stub(StatusCode::OK, stub_body, None).await;

    let response = get(
        addr,
        "/admin/hf/search?q=qwen&filter=gguf&pipeline_tag=text-generation\
         &sort=downloads&direction=-1&limit=30&full=true",
        "test-token",
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(response.text().await.expect("a body"), stub_body);

    let seen = seen.lock().expect("the stub log lock");
    let [request] = seen.as_slice() else {
        panic!("expected exactly one upstream request, saw {seen:?}");
    };
    assert_eq!(request.path, "/api/models");
    for pair in [
        "search=qwen",
        "filter=gguf",
        "pipeline_tag=text-generation",
        "sort=downloads",
        "direction=-1",
        "limit=30",
        "full=true",
    ] {
        assert!(
            request.query.contains(pair),
            "`{pair}` missing from forwarded query `{}`",
            request.query
        );
    }
}

#[tokio::test]
async fn admin_hf_model_targets_the_owner_name_path() {
    let stub_body =
        r#"{"id":"unsloth/Qwen3-8B-GGUF","siblings":[{"rfilename":"q4.gguf","size":4900000000}]}"#;
    let (addr, seen) = serve_against_stub(StatusCode::OK, stub_body, None).await;

    let response = get(addr, "/admin/hf/model/unsloth/Qwen3-8B-GGUF", "test-token").await;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(response.text().await.expect("a body"), stub_body);

    let seen = seen.lock().expect("the stub log lock");
    let [request] = seen.as_slice() else {
        panic!("expected exactly one upstream request, saw {seen:?}");
    };
    assert_eq!(request.path, "/api/models/unsloth/Qwen3-8B-GGUF");
    assert!(
        request.query.contains("blobs=true"),
        "`blobs=true` missing from `{}`: the quant picker needs sibling sizes",
        request.query
    );
}

#[tokio::test]
async fn admin_hf_sends_the_token_only_when_configured() {
    let (with_token, seen_with) = serve_against_stub(StatusCode::OK, "[]", Some("hf_secret")).await;
    let response = get(with_token, "/admin/hf/search?q=x", "test-token").await;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(
        seen_with.lock().expect("the stub log lock")[0]
            .authorization
            .as_deref(),
        Some("Bearer hf_secret"),
        "a configured HF_TOKEN must reach the hub"
    );

    let (without_token, seen_without) = serve_against_stub(StatusCode::OK, "[]", None).await;
    let response = get(without_token, "/admin/hf/search?q=x", "test-token").await;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(
        seen_without.lock().expect("the stub log lock")[0].authorization,
        None,
        "an anonymous proxy must not invent an Authorization header"
    );
}

#[tokio::test]
async fn admin_hf_forwards_upstream_client_errors() {
    for (upstream, expected) in [
        (StatusCode::UNAUTHORIZED, reqwest::StatusCode::UNAUTHORIZED),
        (StatusCode::NOT_FOUND, reqwest::StatusCode::NOT_FOUND),
    ] {
        let (addr, _seen) = serve_against_stub(upstream, r#"{"error":"denied"}"#, None).await;
        let response = get(addr, "/admin/hf/model/owner/name", "test-token").await;
        assert_eq!(
            response.status(),
            expected,
            "hub {upstream} must pass through"
        );
        let body: serde_json::Value = response.json().await.expect("a JSON error envelope");
        assert_eq!(body["error"]["code"], "upstream_client_error");
    }
}

/// GETs `path` over a raw socket, bypassing reqwest's client-side URL
/// normalization (which collapses `%2E%2E` dot-segments before they
/// ever leave a well-behaved client).
async fn raw_get(addr: SocketAddr, path: &str, token: &str) -> (u16, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(addr)
        .await
        .expect("the raw client connects");
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .await
        .expect("the raw request writes");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .await
        .expect("the raw response reads");
    let status = response
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .expect("a status line");
    (status, response)
}

#[tokio::test]
async fn admin_hf_search_maps_a_rejected_query_into_the_error_envelope() {
    let (addr, seen) = serve_against_stub(StatusCode::OK, "[]", None).await;
    for query in [
        "q=a&q=b",
        "pipeline_tag=not-a-workload",
        "pipeline_tag=text-generation&pipeline_tag=automatic-speech-recognition",
        "filter=safetensors",
        "sort=created",
        "direction=1",
        "full=false",
        "limit=0",
        "limit=101",
        "limit=many",
    ] {
        let response = get(addr, &format!("/admin/hf/search?{query}"), "test-token").await;
        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
        let body: serde_json::Value = response.json().await.expect("a JSON error envelope");
        assert_eq!(body["error"]["code"], "malformed_request");
    }
    assert!(
        seen.lock().expect("the stub log lock").is_empty(),
        "a rejected query must never produce an upstream request"
    );
}

#[tokio::test]
async fn admin_hf_model_maps_a_rejected_path_into_the_error_envelope() {
    let (addr, seen) = serve_against_stub(StatusCode::OK, "{}", None).await;
    // `%FF` percent-decodes to invalid UTF-8, so `Path<String>` rejects;
    // the rejection must land in the JSON envelope, after auth.
    let response = get(addr, "/admin/hf/model/%FF%FF/name", "test-token").await;
    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
    let body: serde_json::Value = response.json().await.expect("a JSON error envelope");
    assert_eq!(body["error"]["code"], "malformed_request");

    let unauthenticated = reqwest::Client::new()
        .get(format!("http://{addr}/admin/hf/model/%FF%FF/name"))
        .send()
        .await
        .expect("the request sends");
    assert_eq!(
        unauthenticated.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "auth must win over a malformed path"
    );
    assert!(
        seen.lock().expect("the stub log lock").is_empty(),
        "a rejected path must never produce an upstream request"
    );
}

#[tokio::test]
async fn admin_hf_model_rejects_malformed_repos_without_calling_upstream() {
    let (addr, seen) = serve_against_stub(StatusCode::OK, "{}", None).await;
    // With `{owner}/{name}` segments, repos with spaces or control
    // characters match the route but fail `validate_repo`.
    for repo in ["owner/na%20me", ".../name", "owner/..."] {
        let response = get(addr, &format!("/admin/hf/model/{repo}"), "test-token").await;
        assert_eq!(
            response.status(),
            reqwest::StatusCode::BAD_REQUEST,
            "repo `{repo}` must be refused at the boundary"
        );
        let body: serde_json::Value = response.json().await.expect("a JSON error envelope");
        assert_eq!(body["error"]["code"], "malformed_request");
    }
    // Encoded dot-segments match the route; validate_repo rejects them.
    for repo in ["%2E%2E/name", "owner/%2E%2E"] {
        let (status, response) =
            raw_get(addr, &format!("/admin/hf/model/{repo}"), "test-token").await;
        assert_eq!(status, 400, "repo `{repo}` must be refused at the boundary");
        assert!(
            response.contains("malformed_request"),
            "repo `{repo}` must map to the JSON error envelope, got: {response}"
        );
    }
    assert!(
        seen.lock().expect("the stub log lock").is_empty(),
        "a rejected repo must never produce an upstream request"
    );
}

#[tokio::test]
async fn admin_hf_routes_require_bearer_auth() {
    let (addr, seen) = serve_against_stub(StatusCode::OK, "[]", None).await;
    for path in [
        "/admin/hf/search?q=x",
        "/admin/hf/model/owner/name",
        "/admin/hf/model/owner/name/readme",
    ] {
        let unauthenticated = reqwest::Client::new()
            .get(format!("http://{addr}{path}"))
            .send()
            .await
            .expect("the request sends");
        assert_eq!(
            unauthenticated.status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "`{path}` without a bearer token is refused"
        );

        let wrong_key = get(addr, path, "wrong-token").await;
        assert_eq!(
            wrong_key.status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "`{path}` with the wrong bearer token is refused"
        );
    }
    assert!(
        seen.lock().expect("the stub log lock").is_empty(),
        "an unauthenticated caller must never reach the hub"
    );
}

/// Spawns a stub hub that serves README and model-detail paths
/// differently, recording each request it sees.
async fn spawn_readme_stub(
    readme_status: StatusCode,
    readme_body: &'static str,
) -> (String, Arc<Mutex<Vec<Seen>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&seen);
    let app = axum::Router::new().fallback(move |uri: Uri, headers: HeaderMap| {
        let recorded = Arc::clone(&recorded);
        async move {
            recorded.lock().expect("the stub log lock").push(Seen {
                path: uri.path().to_owned(),
                query: uri.query().unwrap_or("").to_owned(),
                authorization: headers
                    .get(AUTHORIZATION)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned),
            });
            if uri.path().ends_with("/README.md") {
                (
                    readme_status,
                    [(CONTENT_TYPE, "text/markdown")],
                    readme_body,
                )
            } else {
                (StatusCode::OK, [(CONTENT_TYPE, "application/json")], "{}")
            }
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the stub listener binds");
    let addr = listener.local_addr().expect("the stub bound address");
    tokio::spawn(async move {
        let _ignored = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), seen)
}

async fn serve_readme_stub(
    readme_status: StatusCode,
    readme_body: &'static str,
    token: Option<&str>,
) -> (SocketAddr, Arc<Mutex<Vec<Seen>>>) {
    let (base_url, seen) = spawn_readme_stub(readme_status, readme_body).await;
    let proxy = HfProxy::new(base_url, token.map(|t| Secret::new(t.to_owned())));
    let addr = serve_with_hf(hf_config(), proxy).await;
    (addr, seen)
}

#[tokio::test]
async fn admin_hf_readme_proxies_to_the_raw_readme_path() {
    let (addr, seen) = serve_readme_stub(StatusCode::OK, "# Model Card\nHello", None).await;
    let response = get(
        addr,
        "/admin/hf/model/unsloth/Qwen3-8B-GGUF/readme",
        "test-token",
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("text/markdown; charset=utf-8"),
    );
    assert_eq!(
        response.text().await.expect("a body"),
        "# Model Card\nHello"
    );

    let seen = seen.lock().expect("the stub log lock");
    let [request] = seen.as_slice() else {
        panic!("expected one upstream request, saw {seen:?}");
    };
    assert_eq!(
        request.path, "/unsloth/Qwen3-8B-GGUF/raw/main/README.md",
        "the proxy must hit the hub's raw README path"
    );
}

#[tokio::test]
async fn admin_hf_readme_returns_404_for_a_missing_readme() {
    let (addr, _seen) = serve_readme_stub(StatusCode::NOT_FOUND, "", None).await;
    let response = get(addr, "/admin/hf/model/owner/name/readme", "test-token").await;
    assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn admin_hf_readme_validates_the_repo() {
    let (addr, seen) = serve_readme_stub(StatusCode::OK, "# hello", None).await;
    let response = get(addr, "/admin/hf/model/.../name/readme", "test-token").await;
    assert_eq!(
        response.status(),
        reqwest::StatusCode::BAD_REQUEST,
        "a dot-only owner must be refused"
    );
    let body: serde_json::Value = response.json().await.expect("a JSON error envelope");
    assert_eq!(body["error"]["code"], "malformed_request");
    assert!(
        seen.lock().expect("the stub log lock").is_empty(),
        "a rejected repo must never produce an upstream request"
    );
}

#[tokio::test]
async fn admin_hf_readme_caps_the_body_at_one_mib() {
    let seen = Arc::new(Mutex::new(Vec::<Seen>::new()));
    let recorded = Arc::clone(&seen);
    let app = axum::Router::new().fallback(move |uri: Uri, headers: HeaderMap| {
        let recorded = Arc::clone(&recorded);
        async move {
            recorded.lock().expect("the stub log lock").push(Seen {
                path: uri.path().to_owned(),
                query: uri.query().unwrap_or("").to_owned(),
                authorization: headers
                    .get(AUTHORIZATION)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned),
            });
            let big = "x".repeat(2 * 1024 * 1024);
            (StatusCode::OK, [(CONTENT_TYPE, "text/markdown")], big)
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the stub listener binds");
    let addr = listener.local_addr().expect("the stub bound address");
    tokio::spawn(async move {
        let _ignored = axum::serve(listener, app).await;
    });
    let proxy = HfProxy::new(format!("http://{addr}"), None);
    let gw = serve_with_hf(hf_config(), proxy).await;
    let response = get(gw, "/admin/hf/model/owner/name/readme", "test-token").await;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body = response.bytes().await.expect("a body");
    assert!(
        body.len() <= 1024 * 1024,
        "the body must be capped at 1 MiB, got {} bytes",
        body.len()
    );
}
