//! Tests for the embeddings request and response bodies.

use super::*;

fn embedding_request(model: &str, input: EmbeddingInput) -> EmbeddingRequest {
    EmbeddingRequest {
        model: model.to_owned(),
        input,
        encoding_format: None,
        rest: Map::new(),
    }
}

#[test]
fn embedding_request_round_trips_with_string_input() {
    let json = serde_json::json!({
        "model": "m",
        "input": "embed me",
        "encoding_format": "base64",
        "dimensions": 512,
    });
    let req: EmbeddingRequest = serde_json::from_value(json).expect("parse request");
    assert_eq!(req.input, EmbeddingInput::One("embed me".to_owned()));
    assert_eq!(req.encoding_format.as_deref(), Some("base64"));
    // Unnamed fields land in `rest`, not on named fields.
    assert!(req.rest.contains_key("dimensions"));
    assert!(!req.rest.contains_key("model"));
    assert!(!req.rest.contains_key("input"));
    assert!(!req.rest.contains_key("encoding_format"));
    let reparsed: EmbeddingRequest =
        serde_json::from_value(serde_json::to_value(&req).expect("serialize")).expect("reparse");
    assert_eq!(req, reparsed);
}

#[test]
fn embedding_request_round_trips_with_array_input() {
    let json = serde_json::json!({
        "model": "m",
        "input": ["one", "two"],
    });
    let req: EmbeddingRequest = serde_json::from_value(json).expect("parse request");
    assert_eq!(
        req.input,
        EmbeddingInput::Many(vec!["one".to_owned(), "two".to_owned()])
    );
    // An absent encoding_format neither errors nor serializes as null.
    assert_eq!(req.encoding_format, None);
    assert!(
        !serde_json::to_value(&req)
            .expect("serialize")
            .as_object()
            .expect("object")
            .contains_key("encoding_format")
    );
    let reparsed: EmbeddingRequest =
        serde_json::from_value(serde_json::to_value(&req).expect("serialize")).expect("reparse");
    assert_eq!(req, reparsed);
}

#[test]
fn embedding_request_rejects_empty_model_and_empty_batch() {
    assert!(
        embedding_request("  ", EmbeddingInput::One("x".to_owned()))
            .validate()
            .is_err()
    );
    assert!(
        embedding_request("m", EmbeddingInput::Many(vec![]))
            .validate()
            .is_err()
    );
    assert!(
        embedding_request("m", EmbeddingInput::One("x".to_owned()))
            .validate()
            .is_ok()
    );
}

#[test]
fn embedding_request_rejects_reserved_keys_in_rest() {
    let mut req = embedding_request("m", EmbeddingInput::One("x".to_owned()));
    req.rest.insert("input".to_owned(), serde_json::json!("y"));
    assert!(req.validate().is_err());
}

#[test]
fn embedding_response_round_trips_and_preserves_usage() {
    let json = serde_json::json!({
        "object": "list",
        "model": "backend",
        "data": [{ "object": "embedding", "index": 0, "embedding": [0.1, 0.2] }],
        "usage": { "prompt_tokens": 3, "total_tokens": 3 },
    });
    let resp: EmbeddingResponse = serde_json::from_value(json).expect("parse response");
    assert!(resp.validate().is_ok());
    assert!(resp.rest.contains_key("usage"));
    let reparsed: EmbeddingResponse =
        serde_json::from_value(serde_json::to_value(&resp).expect("serialize")).expect("reparse");
    assert_eq!(resp, reparsed);
}

#[test]
fn embedding_response_rejects_malformed_entries() {
    // WIRE-002: a structurally broken entry is an upstream-protocol failure.
    let response = |entry: Value| EmbeddingResponse {
        model: "m".to_owned(),
        data: vec![entry],
        rest: Map::new(),
    };
    assert!(response(serde_json::json!(42)).validate().is_err());
    assert!(
        response(serde_json::json!({ "index": 0 }))
            .validate()
            .is_err()
    );
    assert!(
        response(serde_json::json!({ "embedding": [0.1] }))
            .validate()
            .is_err()
    );
    assert!(
        response(serde_json::json!({ "index": 0, "embedding": [0.1] }))
            .validate()
            .is_ok()
    );
}

#[test]
fn embedding_response_rejects_reserved_keys_in_rest() {
    let mut resp = EmbeddingResponse {
        model: "m".to_owned(),
        data: vec![],
        rest: Map::new(),
    };
    resp.rest.insert("data".to_owned(), serde_json::json!([]));
    assert!(resp.validate().is_err());
}
