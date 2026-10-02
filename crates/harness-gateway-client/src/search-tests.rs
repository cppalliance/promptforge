//! The Gateway search client against an axum mock gateway: the bearer and
//! the request body on the wire, the parsed reply, and the redacted key.

use super::{GatewaySearch, GatewaySearchRequest, GatewaySearchResponse, GatewaySearchResult};
use crate::config::{GatewayEndpoint, SecretString};

use std::net::SocketAddr;

use axum::Json;
use axum::Router;
use axum::http::HeaderMap;
use axum::routing::post;
use serde_json::Value;

/// A mock gateway whose task is owned by the test: dropping it aborts the
/// server task deterministically instead of leaking a detached task.
struct MockServer {
    addr: SocketAddr,
    handle: tokio::task::JoinHandle<()>,
}

impl MockServer {
    /// Binds an ephemeral port, serves `router`, and returns the address.
    async fn spawn(router: Router) -> MockServer {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        MockServer { addr, handle }
    }

    fn url(&self) -> String {
        format!("http://{}", self.addr)
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

/// A search client keyed with `tok` and pointed at the API root `base`.
fn search_at(base: &str) -> GatewaySearch {
    GatewaySearch::new(
        GatewayEndpoint::new(base).expect("valid test endpoint"),
        SecretString::new("tok").expect("non-empty test key"),
    )
}

/// A request carrying only `query`.
fn query(text: &str) -> GatewaySearchRequest {
    GatewaySearchRequest {
        query: text.to_owned(),
        ..GatewaySearchRequest::default()
    }
}

/// A router serving the canned success result at the search endpoint.
fn success_router() -> Router {
    async fn web_search(headers: HeaderMap, Json(body): Json<Value>) -> Json<Value> {
        let auth = headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        assert_eq!(
            auth, "Bearer tok",
            "expected the bearer token to be forwarded"
        );
        assert_eq!(
            body.get("query").and_then(Value::as_str),
            Some("hi"),
            "expected the query to be forwarded in the body"
        );
        Json(serde_json::json!({
            "results": [
                { "title": "T", "url": "https://e.com", "description": "D" }
            ]
        }))
    }
    Router::new().route("/tools/web_search", post(web_search))
}

#[test]
fn debug_never_leaks_the_bearer_token() {
    let search = GatewaySearch::new(
        GatewayEndpoint::new("http://localhost").expect("valid test endpoint"),
        SecretString::new("super-secret-token").expect("non-empty test key"),
    );
    let rendered = format!("{search:?}");
    assert!(
        !rendered.contains("super-secret-token"),
        "the bearer token must never appear in Debug output, got: {rendered}"
    );
    assert!(
        rendered.contains("<redacted>"),
        "the token field must be redacted, got: {rendered}"
    );
}

#[tokio::test]
async fn forwards_query_and_returns_parsed_results() {
    let mock = MockServer::spawn(success_router()).await;

    let response = search_at(&mock.url())
        .search(&query("hi"))
        .await
        .expect("search should succeed");

    assert_eq!(
        response.results[0].title, "T",
        "expected the canned result title to survive the round-trip"
    );
}

#[tokio::test]
async fn forwards_validated_optional_fields() {
    async fn web_search(Json(body): Json<Value>) -> Json<Value> {
        assert_eq!(body.get("count").and_then(Value::as_u64), Some(5));
        assert_eq!(body.get("freshness").and_then(Value::as_str), Some("pw"));
        assert_eq!(
            body.get("safesearch").and_then(Value::as_str),
            Some("strict")
        );
        assert_eq!(
            body.get("include_domains"),
            Some(&serde_json::json!(["example.com"]))
        );
        Json(serde_json::json!({ "results": [{ "url": "https://e.com" }] }))
    }
    let mock = MockServer::spawn(Router::new().route("/tools/web_search", post(web_search))).await;
    let request = GatewaySearchRequest {
        query: "hi".to_owned(),
        count: Some(5),
        freshness: Some("pw".to_owned()),
        safesearch: Some("strict".to_owned()),
        include_domains: vec!["example.com".to_owned()],
        ..GatewaySearchRequest::default()
    };

    search_at(&mock.url())
        .search(&request)
        .await
        .expect("a fully-specified valid request should succeed");
}

#[tokio::test]
async fn parses_every_gateway_result_field_and_defaults_the_optional_ones() {
    async fn web_search() -> Json<Value> {
        Json(serde_json::json!({
            "query": "boost",
            "results": [
                {
                    "title": "T",
                    "url": "https://e.com",
                    "description": "D",
                    "age": "2 days ago",
                    "site_name": "e.com",
                    "extra_snippets": ["S1", "S2"]
                },
                { "url": "https://f.com" }
            ]
        }))
    }
    let mock = MockServer::spawn(Router::new().route("/tools/web_search", post(web_search))).await;

    let response = search_at(&mock.url())
        .search(&query("boost"))
        .await
        .expect("search should succeed");

    assert_eq!(
        response,
        GatewaySearchResponse {
            query: "boost".to_owned(),
            results: vec![
                GatewaySearchResult {
                    title: "T".to_owned(),
                    url: "https://e.com".to_owned(),
                    description: "D".to_owned(),
                    age: Some("2 days ago".to_owned()),
                    site_name: Some("e.com".to_owned()),
                    extra_snippets: vec!["S1".to_owned(), "S2".to_owned()],
                },
                GatewaySearchResult {
                    title: String::new(),
                    url: "https://f.com".to_owned(),
                    description: String::new(),
                    age: None,
                    site_name: None,
                    extra_snippets: Vec::new(),
                },
            ],
        }
    );
}

#[path = "search-tests-responses.rs"]
mod responses;
