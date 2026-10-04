//! Tests for the chat request, response, and streaming chunk bodies.

use super::*;

fn request(model: &str, messages: Vec<Value>) -> ChatRequest {
    ChatRequest {
        model: model.to_owned(),
        messages,
        stream: false,
        rest: Map::new(),
    }
}

#[test]
fn accepts_object_messages() {
    let req = request(
        "m",
        vec![serde_json::json!({ "role": "user", "content": "hi" })],
    );
    assert!(req.validate().is_ok());
}

#[test]
fn rejects_empty_model_and_non_object_messages() {
    assert!(request("  ", vec![]).validate().is_err());
    assert!(
        request("m", vec![serde_json::json!("not-an-object")])
            .validate()
            .is_err()
    );
}

#[test]
fn request_rejects_reserved_keys_in_rest() {
    let mut req = request(
        "m",
        vec![serde_json::json!({ "role": "user", "content": "hi" })],
    );
    req.rest
        .insert("messages".to_owned(), serde_json::json!(["x"]));
    assert!(req.validate().is_err());
}

#[test]
fn rejects_empty_messages_array() {
    // WIRE-001: an empty conversation is not a valid chat request.
    assert!(request("m", vec![]).validate().is_err());
}

#[test]
fn rejects_message_without_role_or_content() {
    // WIRE-001: a message object still needs a supported role and a payload.
    assert!(
        request("m", vec![serde_json::json!({ "content": "hi" })])
            .validate()
            .is_err()
    );
    assert!(
        request("m", vec![serde_json::json!({ "role": "user" })])
            .validate()
            .is_err()
    );
    assert!(
        request(
            "m",
            vec![serde_json::json!({ "role": "spork", "content": "x" })]
        )
        .validate()
        .is_err()
    );
}

#[test]
fn accepts_assistant_tool_call_without_content() {
    // WIRE-001: an assistant tool-call message legitimately omits content.
    let req = request(
        "m",
        vec![serde_json::json!({
            "role": "assistant",
            "tool_calls": [{ "id": "1", "type": "function" }]
        })],
    );
    assert!(req.validate().is_ok());
}

#[test]
fn response_rejects_choice_missing_index_or_payload() {
    // WIRE-002: a structurally broken choice is an upstream-protocol failure.
    let missing_index = ChatResponse {
        model: "m".to_owned(),
        choices: vec![serde_json::json!({ "message": { "role": "assistant" } })],
        rest: Map::new(),
    };
    assert!(missing_index.validate().is_err());
    let missing_payload = ChatResponse {
        model: "m".to_owned(),
        choices: vec![serde_json::json!({ "index": 0 })],
        rest: Map::new(),
    };
    assert!(missing_payload.validate().is_err());
}

#[test]
fn response_accepts_minimally_shaped_choice() {
    let response = ChatResponse {
        model: "m".to_owned(),
        choices: vec![serde_json::json!({
            "index": 0,
            "message": { "role": "assistant", "content": "hi" },
            "finish_reason": "stop"
        })],
        rest: Map::new(),
    };
    assert!(response.validate().is_ok());
}

#[test]
fn response_rejects_reserved_keys_in_rest() {
    let mut response = ChatResponse {
        model: "m".to_owned(),
        choices: vec![],
        rest: Map::new(),
    };
    response
        .rest
        .insert("choices".to_owned(), serde_json::json!([]));
    assert!(response.validate().is_err());
}

#[test]
fn response_rejects_non_object_choice() {
    let response = ChatResponse {
        model: "m".to_owned(),
        choices: vec![serde_json::json!(42)],
        rest: Map::new(),
    };
    assert!(response.validate().is_err());
}

#[test]
fn request_round_trips_through_json() {
    let json = serde_json::json!({
        "model": "m",
        "messages": [{ "role": "user", "content": "hi" }],
        "temperature": 0.5,
        "stream": false,
    });
    let req: ChatRequest = serde_json::from_value(json.clone()).expect("parse request");
    // Unnamed fields land in `rest`, not on named fields.
    assert!(req.rest.contains_key("temperature"));
    assert!(!req.stream);
    assert!(!req.rest.contains_key("stream"));
    assert!(!req.rest.contains_key("model"));
    assert!(!req.rest.contains_key("messages"));
    // Serialize back and re-parse: the value is stable.
    let reparsed: ChatRequest =
        serde_json::from_value(serde_json::to_value(&req).expect("serialize")).expect("reparse");
    assert_eq!(req, reparsed);
}

#[test]
fn request_stream_flag_round_trips_and_omits_when_absent() {
    let json = serde_json::json!({
        "model": "m",
        "messages": [{ "role": "user", "content": "hi" }],
        "stream": true,
    });
    let req: ChatRequest = serde_json::from_value(json).expect("parse request");
    assert!(req.stream);
    assert_eq!(
        serde_json::to_value(&req).expect("serialize").get("stream"),
        Some(&serde_json::json!(true))
    );
    // An absent stream flag neither errors nor serializes as false.
    let req = request(
        "m",
        vec![serde_json::json!({ "role": "user", "content": "hi" })],
    );
    assert!(!req.stream);
    assert!(
        !serde_json::to_value(&req)
            .expect("serialize")
            .as_object()
            .expect("object")
            .contains_key("stream")
    );
}

#[test]
fn response_round_trips_and_preserves_unknown_fields() {
    let json = serde_json::json!({
        "model": "backend",
        "choices": [{ "index": 0 }],
        "usage": { "total_tokens": 7 },
    });
    let resp: ChatResponse = serde_json::from_value(json).expect("parse response");
    assert!(resp.rest.contains_key("usage"));
    let reparsed: ChatResponse =
        serde_json::from_value(serde_json::to_value(&resp).expect("serialize")).expect("reparse");
    assert_eq!(resp, reparsed);
}

#[test]
fn chat_chunk_round_trips_and_preserves_unknown_fields() {
    let json = serde_json::json!({
        "id": "chatcmpl-1",
        "object": "chat.completion.chunk",
        "model": "backend",
        "choices": [
            { "index": 0, "delta": { "role": "assistant", "content": "Hel" }, "finish_reason": null }
        ],
    });
    let chunk: ChatChunk = serde_json::from_value(json).expect("parse chunk");
    assert_eq!(chunk.model, "backend");
    assert_eq!(chunk.choices.len(), 1);
    assert_eq!(chunk.choices[0].index, 0);
    assert_eq!(
        chunk.choices[0]
            .delta
            .get("content")
            .and_then(Value::as_str),
        Some("Hel")
    );
    // Unnamed fields land in `rest`, not on named fields.
    assert!(chunk.rest.contains_key("id"));
    assert!(chunk.choices[0].rest.contains_key("finish_reason"));
    assert!(!chunk.rest.contains_key("model"));
    assert!(!chunk.rest.contains_key("choices"));
    let reparsed: ChatChunk =
        serde_json::from_value(serde_json::to_value(&chunk).expect("serialize")).expect("reparse");
    assert_eq!(chunk, reparsed);
}

#[test]
fn chat_chunk_rejects_empty_choices() {
    // A chunk with no choices (for example a usage-only summary object)
    // is malformed: logged and skipped, never relayed.
    let chunk = ChatChunk {
        model: "m".to_owned(),
        choices: vec![],
        rest: Map::new(),
    };
    assert!(chunk.validate().is_err());
}

#[test]
fn chat_chunk_round_trips_an_empty_delta() {
    // The terminal chunk legitimately contains an empty delta plus a
    // finish_reason; it must survive the round-trip.
    let json = serde_json::json!({
        "model": "backend",
        "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }],
    });
    let chunk: ChatChunk = serde_json::from_value(json).expect("parse chunk");
    assert_eq!(
        chunk.choices[0]
            .rest
            .get("finish_reason")
            .and_then(Value::as_str),
        Some("stop")
    );
    let reparsed: ChatChunk =
        serde_json::from_value(serde_json::to_value(&chunk).expect("serialize")).expect("reparse");
    assert_eq!(chunk, reparsed);
}
