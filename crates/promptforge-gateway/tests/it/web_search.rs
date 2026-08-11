//! Web-search tool route: result shaping, per-host caps, auth, and errors.

use serde_json::Value;

use crate::support::{fake_backend, fake_brave, gateway_for, gateway_with_web_search};

#[tokio::test]
async fn web_search_returns_results() {
    let brave = fake_brave().await;
    let gateway = gateway_with_web_search(brave).await;

    let response = reqwest::Client::new()
        .post(format!("http://{}/v1/tools/web_search", gateway.addr))
        .bearer_auth("test-token")
        .json(&serde_json::json!({ "query": "hi" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);

    let body: Value = response.json().await.unwrap();
    assert_eq!(body.get("query").and_then(Value::as_str), Some("hi"));
    let results = body.get("results").and_then(Value::as_array).unwrap();
    // Default max_per_host=2: keep A1,A2,B1,B2; drop A3.
    assert_eq!(results.len(), 4);
    assert_eq!(results[0].get("title").and_then(Value::as_str), Some("A1"));
    assert_eq!(
        results[0].get("url").and_then(Value::as_str),
        Some("https://a.com/1")
    );
    assert_eq!(
        results[0].get("site_name").and_then(Value::as_str),
        Some("a.com")
    );
    assert_eq!(
        results[0]
            .get("extra_snippets")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1)
    );

    let mut host_counts = std::collections::HashMap::<String, usize>::new();
    for hit in results {
        let site = hit
            .get("site_name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        *host_counts.entry(site).or_default() += 1;
    }
    for count in host_counts.values() {
        assert!(*count <= 2, "default max_per_host is 2");
    }
}

#[tokio::test]
async fn web_search_empty_query_is_400() {
    let brave = fake_brave().await;
    let gateway = gateway_with_web_search(brave).await;

    let response = reqwest::Client::new()
        .post(format!("http://{}/v1/tools/web_search", gateway.addr))
        .bearer_auth("test-token")
        .json(&serde_json::json!({ "query": "   " }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 400);
}

#[tokio::test]
async fn web_search_wrong_token_is_401() {
    let brave = fake_brave().await;
    let gateway = gateway_with_web_search(brave).await;

    let response = reqwest::Client::new()
        .post(format!("http://{}/v1/tools/web_search", gateway.addr))
        .bearer_auth("wrong-token")
        .json(&serde_json::json!({ "query": "hi" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 401);
}

#[tokio::test]
async fn web_search_not_configured_is_404() {
    let backend = fake_backend().await;
    let gateway = gateway_for(backend).await;

    let response = reqwest::Client::new()
        .post(format!("http://{}/v1/tools/web_search", gateway.addr))
        .bearer_auth("test-token")
        .json(&serde_json::json!({ "query": "hi" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 404);
}
