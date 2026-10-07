//! The Gateway search as a Host's `SearchProvider`: a validated query
//! goes out as the Gateway's request body, the reply's rows come back
//! field by field, and a failed search keeps its kind with the Gateway's
//! error as the cause.

use super::*;

use std::sync::{Arc, Mutex};

use axum::http::StatusCode;
use plugin_web::{
    Freshness, SafeSearch, SearchErrorKind, SearchProvider, SearchQuery, SearchResult,
};

use crate::search::GatewaySearchError;

/// The Authorization header and body of each search a mock received.
type Seen = Arc<Mutex<Vec<(Option<String>, Value)>>>;

/// A mock Gateway that records each search and answers `reply`.
fn recording_gateway(seen: Seen, reply: Value) -> Router {
    Router::new().route(
        "/tools/web_search",
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

/// A provider query for `text` with every option left out.
fn provider_query(text: &str) -> SearchQuery {
    SearchQuery {
        query: text.to_owned(),
        ..SearchQuery::default()
    }
}

#[tokio::test]
async fn a_gateway_reply_maps_into_results_and_the_query_into_its_request() {
    let seen = Seen::default();
    let reply = serde_json::json!({
        "query": "hi",
        "results": [
            {
                "title": "T", "url": "https://e.com", "description": "D",
                "age": "2 days ago", "site_name": "e.com", "extra_snippets": ["more"]
            },
            { "url": "https://f.com" }
        ]
    });
    let mock = MockServer::spawn(recording_gateway(Arc::clone(&seen), reply)).await;

    let results = SearchProvider::search(
        &search_at(&mock.url()),
        SearchQuery {
            count: Some(3),
            freshness: Some(Freshness::Pw),
            country: Some("de".to_owned()),
            search_lang: Some("en".to_owned()),
            safesearch: Some(SafeSearch::Strict),
            include_domains: vec!["e.com".to_owned()],
            exclude_domains: vec!["g.com".to_owned()],
            ..provider_query("hi")
        },
    )
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
    assert_eq!(seen[0].0.as_deref(), Some("Bearer tok"));
    assert_eq!(
        seen[0].1,
        serde_json::json!({
            "query": "hi", "count": 3, "freshness": "pw", "country": "de",
            "search_lang": "en", "safesearch": "strict",
            "include_domains": ["e.com"], "exclude_domains": ["g.com"]
        })
    );
}

#[tokio::test]
async fn a_gateway_error_status_maps_as_backend_with_the_gateway_error_as_source() {
    async fn web_search() -> (StatusCode, &'static str) {
        (StatusCode::BAD_GATEWAY, "upstream down")
    }
    let mock = MockServer::spawn(Router::new().route("/tools/web_search", post(web_search))).await;

    let error = SearchProvider::search(&search_at(&mock.url()), provider_query("hi"))
        .await
        .expect_err("a 502 fails the search");
    assert_eq!(error.kind(), SearchErrorKind::Backend);
    assert_eq!(error.to_string(), "backend returned 502: upstream down");
    let source = std::error::Error::source(&error).expect("the failure keeps a cause");
    assert!(
        source.is::<GatewaySearchError>(),
        "the Gateway's error is the cause: {source}"
    );
}

#[tokio::test]
async fn a_refused_connection_maps_as_transport_with_the_gateway_error_as_source() {
    // Bind then drop the listener so the port is closed and the connection
    // is refused deterministically.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let error = SearchProvider::search(&search_at(&format!("http://{addr}")), provider_query("hi"))
        .await
        .expect_err("a refused connection fails the search");
    assert_eq!(error.kind(), SearchErrorKind::Transport);
    assert_eq!(error.to_string(), "request failed");
    let source = std::error::Error::source(&error).expect("the failure keeps a cause");
    assert!(
        source.is::<GatewaySearchError>(),
        "the Gateway's error is the cause: {source}"
    );
}
