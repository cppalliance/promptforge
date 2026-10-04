//! Tests for the rerank request and response bodies.

use super::*;

fn rerank_request(model: &str) -> RerankRequest {
    RerankRequest {
        model: model.to_owned(),
        query: "what is rust".to_owned(),
        documents: vec!["a systems language".to_owned(), "a card game".to_owned()],
        top_n: None,
        rest: Map::new(),
    }
}

#[test]
fn rerank_request_round_trips_with_top_n() {
    let json = serde_json::json!({
        "model": "m",
        "query": "what is rust",
        "documents": ["a systems language", "a card game"],
        "top_n": 1,
        "truncate": true,
    });
    let req: RerankRequest = serde_json::from_value(json).expect("parse request");
    assert_eq!(req.top_n, Some(1));
    // Unnamed fields land in `rest`, not on named fields.
    assert!(req.rest.contains_key("truncate"));
    assert!(!req.rest.contains_key("model"));
    assert!(!req.rest.contains_key("query"));
    assert!(!req.rest.contains_key("documents"));
    assert!(!req.rest.contains_key("top_n"));
    let reparsed: RerankRequest =
        serde_json::from_value(serde_json::to_value(&req).expect("serialize")).expect("reparse");
    assert_eq!(req, reparsed);
}

#[test]
fn rerank_request_omits_an_absent_top_n() {
    let req = rerank_request("m");
    assert_eq!(req.top_n, None);
    // An absent top_n neither errors nor serializes as null.
    assert!(
        !serde_json::to_value(&req)
            .expect("serialize")
            .as_object()
            .expect("object")
            .contains_key("top_n")
    );
    let reparsed: RerankRequest =
        serde_json::from_value(serde_json::to_value(&req).expect("serialize")).expect("reparse");
    assert_eq!(req, reparsed);
}

#[test]
fn rerank_request_rejects_empty_model_query_and_documents() {
    assert!(rerank_request("  ").validate().is_err());
    let empty_query = RerankRequest {
        query: "  ".to_owned(),
        ..rerank_request("m")
    };
    assert!(empty_query.validate().is_err());
    let no_documents = RerankRequest {
        documents: vec![],
        ..rerank_request("m")
    };
    assert!(no_documents.validate().is_err());
    assert!(rerank_request("m").validate().is_ok());
}

#[test]
fn rerank_request_rejects_reserved_keys_in_rest() {
    let mut req = rerank_request("m");
    req.rest
        .insert("documents".to_owned(), serde_json::json!(["x"]));
    assert!(req.validate().is_err());
}

#[test]
fn rerank_response_round_trips_and_preserves_usage() {
    let json = serde_json::json!({
        "model": "backend",
        "results": [
            { "index": 0, "relevance_score": 0.9, "document": { "text": "a systems language" } },
            { "index": 1, "relevance_score": 0.1 }
        ],
        "usage": { "total_tokens": 12 },
    });
    let resp: RerankResponse = serde_json::from_value(json).expect("parse response");
    assert!(resp.validate().is_ok());
    assert!(resp.rest.contains_key("usage"));
    let reparsed: RerankResponse =
        serde_json::from_value(serde_json::to_value(&resp).expect("serialize")).expect("reparse");
    assert_eq!(resp, reparsed);
}

#[test]
fn rerank_response_rejects_malformed_results() {
    // WIRE-002: a structurally broken result is an upstream-protocol failure.
    let response = |result: Value| RerankResponse {
        model: "m".to_owned(),
        results: vec![result],
        rest: Map::new(),
    };
    assert!(response(serde_json::json!(42)).validate().is_err());
    assert!(
        response(serde_json::json!({ "index": 0 }))
            .validate()
            .is_err()
    );
    assert!(
        response(serde_json::json!({ "relevance_score": 0.9 }))
            .validate()
            .is_err()
    );
    assert!(
        response(serde_json::json!({ "index": 0, "relevance_score": 0.9 }))
            .validate()
            .is_ok()
    );
}

#[test]
fn rerank_response_rejects_reserved_keys_in_rest() {
    let mut resp = RerankResponse {
        model: "m".to_owned(),
        results: vec![],
        rest: Map::new(),
    };
    resp.rest
        .insert("results".to_owned(), serde_json::json!([]));
    assert!(resp.validate().is_err());
}
