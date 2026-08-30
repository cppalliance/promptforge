//! The `GET /admin/hf/*` routes: a thin bearer-authed proxy onto the
//! Hugging Face hub API, feeding the config UI's Discover view.
//!
//! The proxy forwards the hub's JSON bodies verbatim - the UI adapts the
//! shape - and attaches the boot-time `HF_TOKEN` when one is present, so
//! the browser never holds the token and public repos keep working
//! without one. Upstream 4xx statuses pass through in the gateway's error
//! envelope via [`ProtocolError::upstream_status`]; nothing is cached.

use std::time::Duration;

use axum::body::Body;
use axum::extract::rejection::{PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::header::CONTENT_TYPE;
use axum::http::{HeaderMap, HeaderValue};
use axum::response::Response;
use promptforge_gateway_config::Secret;
use promptforge_gateway_protocol::ProtocolError;
use promptforge_gateway_protocol::http_util::{self, MAX_ERROR_BODY, read_body_capped};
use serde::Deserialize;

use crate::error::GatewayError;
use crate::{AppState, check_auth};

/// Whole-request deadline for one hub call, applied per request; reqwest's
/// per-request timeout replaces the bounded client's wider default.
const HF_TIMEOUT: Duration = Duration::from_secs(30);

/// Shared Hugging Face hub client: one reqwest client, the hub base URL,
/// and the boot-time `HF_TOKEN` (absent for anonymous access).
#[derive(Debug)]
pub(crate) struct HfProxy {
    /// The shared bounded HTTP client; the hub deadline is applied per request.
    client: reqwest::Client,
    /// The hub origin, `https://huggingface.co` outside tests.
    base_url: String,
    /// The bearer token sent to the hub, when one was configured.
    token: Option<Secret>,
}

impl HfProxy {
    /// The production hub client: `https://huggingface.co`, with the token
    /// read once from the process `HF_TOKEN` (dotenvy has already folded
    /// the `.env` files into the process env at boot).
    pub(crate) fn from_env() -> HfProxy {
        let token = std::env::var("HF_TOKEN")
            .ok()
            .filter(|token| !token.is_empty())
            .map(Secret::new);
        HfProxy::new("https://huggingface.co".to_owned(), token)
    }

    /// A hub client aimed at `base_url` with an explicit token, so tests
    /// point the proxy at a local stub without touching the process env.
    pub(crate) fn new(base_url: String, token: Option<Secret>) -> HfProxy {
        HfProxy {
            client: http_util::bounded_client(),
            base_url,
            token,
        }
    }

    /// GETs `{base_url}{path}` with `query`, forwarding the hub's JSON body
    /// and status verbatim on success and mapping a non-success status or a
    /// transport failure into the gateway's error envelope.
    async fn forward(&self, path: &str, query: &[(&str, &str)]) -> Result<Response, GatewayError> {
        let mut request = self
            .client
            .get(format!("{}{path}", self.base_url))
            .query(query)
            .timeout(HF_TIMEOUT);
        if let Some(token) = &self.token {
            request = request.bearer_auth(token.expose());
        }
        let response = request
            .send()
            .await
            .map_err(ProtocolError::upstream_transport)?;
        let status = response.status();
        if !status.is_success() {
            let body = read_body_capped(response, MAX_ERROR_BODY).await;
            let body: String = body.chars().take(2000).collect();
            return Err(ProtocolError::upstream_status(status.as_u16(), body).into());
        }
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .cloned()
            .unwrap_or(HeaderValue::from_static("application/json"));
        // Streaming the body through keeps the gateway's memory use flat no
        // matter how large the hub's sibling list gets.
        Response::builder()
            .status(status)
            .header(CONTENT_TYPE, content_type)
            .body(Body::from_stream(response.bytes_stream()))
            .map_err(GatewayError::upstream_protocol)
    }
}

/// Query parameters accepted by `GET /admin/hf/search`; each present field
/// is forwarded to the hub's model-search API, and everything else is
/// dropped at this boundary.
#[derive(Debug, Deserialize)]
pub(crate) struct HfSearchQuery {
    /// Free-text search, forwarded as the hub's `search` parameter.
    q: Option<String>,
    /// Tag filter; the Discover view pins `gguf`.
    filter: Option<String>,
    /// Sort field: `downloads`, `trendingScore`, or `lastModified`.
    sort: Option<String>,
    /// Sort direction, `-1` for descending.
    direction: Option<String>,
    /// Result page size.
    limit: Option<String>,
    /// `full=true` asks the hub to include each result's sibling file list.
    full: Option<String>,
}

/// The `GET /admin/hf/search` route: bearer-authed, proxies the hub's
/// `GET /api/models` search and returns its JSON body verbatim.
pub(crate) async fn admin_hf_search(
    State(state): State<AppState>,
    query: Result<Query<HfSearchQuery>, QueryRejection>,
    headers: HeaderMap,
) -> Result<Response, GatewayError> {
    check_auth(&state, &headers).await?;
    // Deferring the extractor keeps auth first and puts the rejection in
    // the gateway's JSON error envelope instead of axum's plain-text 400.
    let Query(query) =
        query.map_err(|rejection| GatewayError::MalformedRequest(rejection.body_text()))?;
    let renames = [("search", &query.q)];
    let passthrough = [
        ("filter", &query.filter),
        ("sort", &query.sort),
        ("direction", &query.direction),
        ("limit", &query.limit),
        ("full", &query.full),
    ];
    let params: Vec<(&str, &str)> = renames
        .iter()
        .chain(passthrough.iter())
        .filter_map(|(name, value)| Some((*name, value.as_deref()?)))
        .collect();
    state.hf.forward("/api/models", &params).await
}

/// The `GET /admin/hf/model/{repo}` route: bearer-authed, proxies the hub's
/// model detail for an `owner/name` repo with `blobs=true`, so the sibling
/// list carries the exact file sizes the quant picker needs.
///
/// `repo` is caller input holding a slash, matched by a wildcard segment:
/// it must be exactly two non-empty hub-legal segments, so a caller can
/// never steer the upstream path (traversal, encoded slashes, empty
/// segments all map to 400 before any request leaves the gateway).
pub(crate) async fn admin_hf_model(
    State(state): State<AppState>,
    repo: Result<Path<String>, PathRejection>,
    headers: HeaderMap,
) -> Result<Response, GatewayError> {
    check_auth(&state, &headers).await?;
    // Deferring the extractor keeps auth first and puts the rejection in
    // the gateway's JSON error envelope instead of axum's plain-text 400.
    let Path(repo) =
        repo.map_err(|rejection| GatewayError::MalformedRequest(rejection.body_text()))?;
    validate_repo(&repo)?;
    state
        .hf
        .forward(&format!("/api/models/{repo}"), &[("blobs", "true")])
        .await
}

/// Checks that `repo` is exactly `owner/name`: two non-empty segments of
/// hub-legal characters (ASCII alphanumerics, `-`, `_`, `.`), neither made
/// only of dots.
fn validate_repo(repo: &str) -> Result<(), GatewayError> {
    let mut segments = repo.split('/');
    if let (Some(owner), Some(name), None) = (segments.next(), segments.next(), segments.next())
        && is_repo_segment(owner)
        && is_repo_segment(name)
    {
        return Ok(());
    }
    Err(GatewayError::MalformedRequest(format!(
        "repo `{repo}` is not an owner/name pair of path-safe segments"
    )))
}

/// Whether one repo segment is non-empty, hub-legal, and not a dot run
/// (`.` and `..` are path traversal, not names).
fn is_repo_segment(segment: &str) -> bool {
    !segment.is_empty()
        && !segment.bytes().all(|byte| byte == b'.')
        && segment
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::sync::{Arc, Mutex};

    use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
    use axum::http::{HeaderMap, StatusCode, Uri};
    use promptforge_gateway_config::{Config, Secret};

    use super::HfProxy;
    use crate::test_support::serve_with_hf;

    /// A minimal profile: the hub proxy needs nothing beyond `[server]`.
    fn hf_config() -> Config {
        Config::from_toml_str(
            r#"
[server]
bind = "127.0.0.1:0"
api_key = "test-token"
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
            "/admin/hf/search?q=qwen&filter=gguf&sort=downloads&direction=-1&limit=30&full=true",
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
        let stub_body = r#"{"id":"unsloth/Qwen3-8B-GGUF","siblings":[{"rfilename":"q4.gguf","size":4900000000}]}"#;
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
        let (with_token, seen_with) =
            serve_against_stub(StatusCode::OK, "[]", Some("hf_secret")).await;
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
        // A duplicate key fails `HfSearchQuery` deserialization, which must
        // surface in the JSON envelope, not axum's plain-text 400.
        let response = get(addr, "/admin/hf/search?q=a&q=b", "test-token").await;
        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
        let body: serde_json::Value = response.json().await.expect("a JSON error envelope");
        assert_eq!(body["error"]["code"], "malformed_request");
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
        for repo in ["noslash", "owner/name/extra", "owner/", "owner/na%20me"] {
            let response = get(addr, &format!("/admin/hf/model/{repo}"), "test-token").await;
            assert_eq!(
                response.status(),
                reqwest::StatusCode::BAD_REQUEST,
                "repo `{repo}` must be refused at the boundary"
            );
            let body: serde_json::Value = response.json().await.expect("a JSON error envelope");
            assert_eq!(body["error"]["code"], "malformed_request");
        }
        // A hostile client can put encoded dot-segments on the wire even
        // though a well-behaved URL parser collapses them client-side.
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
        for path in ["/admin/hf/search?q=x", "/admin/hf/model/owner/name"] {
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
}
