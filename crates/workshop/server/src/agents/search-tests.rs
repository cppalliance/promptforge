//! Search provider tests: a mock Gateway's reply and error status map into
//! the provider's results and errors, a replaced gateway serves the next
//! search, and a missing or down gateway fails as `request failed`.

use std::sync::{Arc, Mutex};

use axum::Json;
use axum::Router;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use harness_web::{
    Freshness, SafeSearch, SearchError, SearchErrorKind, SearchProvider, SearchQuery, SearchResult,
};
use serde_json::{Value, json};
use workshop_gateway::{GatewayBinding, GatewayHandles, GatewayHealth};
use workshop_registry::{Registration, Registry};

use super::GatewaySearchProvider;
use crate::app::test_helpers::spawn_gateway;

/// A registry holding gateway handles bound to `base_url` under the
/// fixture bearer, with the binding, the health flag, and the guard that
/// keeps the registration alive.
fn registry_with_gateway(
    base_url: &str,
) -> (Registry, GatewayBinding, GatewayHealth, Registration) {
    let registry = Registry::new();
    let binding = GatewayBinding::new(base_url, "test-key").expect("the binding builds");
    let health = GatewayHealth::new();
    let guard = workshop_gateway::register(
        &registry,
        GatewayHandles::new(binding.clone(), health.clone()),
    );
    (registry, binding, health, guard)
}

/// A query for `text` with every option left out.
fn query(text: &str) -> SearchQuery {
    SearchQuery {
        query: text.to_owned(),
        ..SearchQuery::default()
    }
}

/// The Authorization header and body of each search a mock received.
type Seen = Arc<Mutex<Vec<(Option<String>, Value)>>>;

/// A mock Gateway that records each search and answers `reply`.
fn recording_gateway(seen: Seen, reply: Value) -> Router {
    Router::new().route(
        "/v1/tools/web_search",
        post(move |headers: HeaderMap, Json(body): Json<Value>| {
            let seen = Arc::clone(&seen);
            let reply = reply.clone();
            async move {
                let auth = headers
                    .get("authorization")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned);
                seen.lock()
                    .expect("the capture lock is healthy")
                    .push((auth, body));
                Json(reply)
            }
        }),
    )
}

#[tokio::test]
async fn a_gateway_reply_maps_into_results_and_the_query_into_its_request() {
    let seen = Seen::default();
    let reply = json!({
        "query": "hi",
        "results": [
            {
                "title": "T", "url": "https://e.com", "description": "D",
                "age": "2 days ago", "site_name": "e.com", "extra_snippets": ["more"]
            },
            { "url": "https://f.com" }
        ]
    });
    let base = spawn_gateway(recording_gateway(Arc::clone(&seen), reply)).await;
    let (registry, _binding, _health, _guard) = registry_with_gateway(&base);
    let provider = GatewaySearchProvider::new(registry);

    let results = provider
        .search(SearchQuery {
            count: Some(3),
            freshness: Some(Freshness::Pw),
            safesearch: Some(SafeSearch::Strict),
            include_domains: vec!["e.com".to_owned()],
            ..query("hi")
        })
        .await
        .expect("the search succeeds");

    assert_eq!(results.query, "hi");
    assert_eq!(
        results.results,
        [
            SearchResult {
                title: "T".to_owned(),
                url: "https://e.com".to_owned(),
                description: "D".to_owned(),
                age: Some("2 days ago".to_owned()),
                site_name: Some("e.com".to_owned()),
                extra_snippets: vec!["more".to_owned()],
            },
            SearchResult {
                url: "https://f.com".to_owned(),
                ..SearchResult::default()
            },
        ]
    );
    let seen = seen.lock().expect("the capture lock is healthy");
    assert_eq!(seen.len(), 1, "one search reached the Gateway");
    assert_eq!(seen[0].0.as_deref(), Some("Bearer test-key"));
    assert_eq!(
        seen[0].1,
        json!({
            "query": "hi", "count": 3, "freshness": "pw", "safesearch": "strict",
            "include_domains": ["e.com"]
        })
    );
}

#[tokio::test]
async fn a_gateway_error_status_maps_as_backend_with_the_gateway_error_as_source() {
    let base = spawn_gateway(Router::new().route(
        "/v1/tools/web_search",
        post(|| async { (StatusCode::BAD_GATEWAY, "upstream down") }),
    ))
    .await;
    let (registry, _binding, _health, _guard) = registry_with_gateway(&base);
    let provider = GatewaySearchProvider::new(registry);

    let error = provider
        .search(query("hi"))
        .await
        .expect_err("a 502 fails the search");
    assert_eq!(error.kind(), SearchErrorKind::Backend);
    assert_eq!(error.to_string(), "backend returned 502: upstream down");
    let source = std::error::Error::source(&error).expect("the failure keeps a cause");
    assert!(
        source.is::<harness_gateway_client::GatewaySearchError>(),
        "the Gateway's error is the cause: {source}"
    );
}

#[tokio::test]
async fn a_replaced_gateway_serves_the_next_search() {
    let first = spawn_gateway(recording_gateway(
        Seen::default(),
        json!({ "query": "hi", "results": [{ "url": "https://first.example" }] }),
    ))
    .await;
    let second_seen = Seen::default();
    let second = spawn_gateway(recording_gateway(
        Arc::clone(&second_seen),
        json!({ "query": "hi", "results": [{ "url": "https://second.example" }] }),
    ))
    .await;
    let (registry, binding, _health, _guard) = registry_with_gateway(&first);
    let provider = GatewaySearchProvider::new(registry);

    let before = provider
        .search(query("hi"))
        .await
        .expect("the first search succeeds");
    assert_eq!(before.results[0].url, "https://first.example");

    binding
        .replace(&second, "replacement-key")
        .expect("the replacement publishes");
    let after = provider
        .search(query("hi"))
        .await
        .expect("the second search succeeds");
    assert_eq!(
        after.results[0].url, "https://second.example",
        "the new generation's gateway answers"
    );
    assert_eq!(
        second_seen.lock().expect("the capture lock is healthy")[0]
            .0
            .as_deref(),
        Some("Bearer replacement-key"),
        "the new generation's key is presented"
    );
}

/// Asserts `error` is the bare transport failure that names no gateway
/// detail.
fn assert_no_gateway(error: &SearchError) {
    assert_eq!(error.kind(), SearchErrorKind::Transport);
    assert_eq!(error.to_string(), "request failed");
    assert!(
        std::error::Error::source(error).is_none(),
        "no endpoint or key detail rides along"
    );
}

#[tokio::test]
async fn without_a_gateway_registration_a_search_fails_as_request_failed() {
    let provider = GatewaySearchProvider::new(Registry::new());
    let error = provider
        .search(query("hi"))
        .await
        .expect_err("no gateway, no search");
    assert_no_gateway(&error);
}

#[tokio::test]
async fn a_gateway_the_heartbeat_reports_down_fails_as_request_failed() {
    let seen = Seen::default();
    let base = spawn_gateway(recording_gateway(
        Arc::clone(&seen),
        json!({ "query": "hi", "results": [] }),
    ))
    .await;
    let (registry, _binding, health, _guard) = registry_with_gateway(&base);
    health.publish(false);
    let provider = GatewaySearchProvider::new(registry);

    let error = provider
        .search(query("hi"))
        .await
        .expect_err("a down gateway is not searched");
    assert_no_gateway(&error);
    assert!(
        seen.lock().expect("the capture lock is healthy").is_empty(),
        "nothing is sent to a gateway reported down"
    );
}
