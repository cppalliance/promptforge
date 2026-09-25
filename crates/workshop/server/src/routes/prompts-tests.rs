//! Tests for the `POST /prompts/contract` route: the wire contract the
//! Run window renders, and the `422` parse-failure envelope.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt as _;

use crate::app::fixtures::{body_bytes, state_for};
use crate::app::router;

/// Posts `body` to `/prompts/contract` on the assembled server router and
/// returns the status with the decoded JSON body.
async fn post_contract(body: serde_json::Value) -> (StatusCode, serde_json::Value) {
    let (state, _state_dir) = state_for("http://127.0.0.1:1");
    let request = Request::builder()
        .method("POST")
        .uri("/prompts/contract")
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("static request parts are valid");
    let response = router(state)
        .oneshot(request)
        .await
        .expect("the router is infallible");
    let status = response.status();
    let bytes = body_bytes(response).await;
    let json = serde_json::from_slice(&bytes).expect("the body is JSON");
    (status, json)
}

/// A prompt exercising every frontmatter key in the contract.
const FULL_PROMPT: &str = r"---
name: full
description: does everything
promptforge: 0
max_tool_iterations: 5
input:
  path: paper.md
  description: the paper
output:
  path: out.md
  description: the result
capabilities:
  - web/search
  - ref: fs/local
    optional: true
tools:
  search: web/search/query
args:
  topic:
    type: string
    description: the topic
  count:
    type: integer
    optional: true
    default: 3
models:
  writer:
    keywords: [frontier, thinking]
    min_context: 128000
    description: writes prose
---

# Full

## Only

Body text.
";

#[tokio::test]
async fn a_full_frontmatter_prompt_answers_every_contract_section() {
    let body = serde_json::json!({ "name": "full", "text": FULL_PROMPT });
    let (status, json) = post_contract(body).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["name"], "full");
    assert_eq!(json["description"], "does everything");
    assert_eq!(json["promptforge"], 0);
    assert_eq!(json["max_tool_iterations"], 5);
    assert_eq!(
        json["input"],
        serde_json::json!({ "path": "paper.md", "description": "the paper" })
    );
    assert_eq!(
        json["output"],
        serde_json::json!({ "path": "out.md", "description": "the result" })
    );
    // Capabilities keep declaration order; the map-backed sections are
    // sorted by alias, name, or label.
    assert_eq!(
        json["capabilities"],
        serde_json::json!([
            { "id": "web/search", "optional": false },
            { "id": "fs/local", "optional": true },
        ])
    );
    assert_eq!(
        json["tools"],
        serde_json::json!([
            { "alias": "search", "kind": "exact", "path": "web/search/query" },
        ])
    );
    assert_eq!(
        json["args"],
        serde_json::json!({
            "implicit": false,
            "fields": [
                { "name": "count", "type": "integer", "optional": true, "default": 3, "description": null },
                { "name": "topic", "type": "string", "optional": false, "default": null, "description": "the topic" },
            ],
        })
    );
    assert_eq!(
        json["models"],
        serde_json::json!([
            { "label": "writer", "keywords": ["frontier", "thinking"], "min_context": 128_000, "description": "writes prose" },
        ])
    );
}

#[tokio::test]
async fn a_broken_yaml_key_answers_parse_frontmatter_with_the_line() {
    let text = "---\nname: [unclosed\ndescription: x\n---\n\n# T\n\n## S\n\np\n";
    let body = serde_json::json!({ "name": "broken", "text": text });
    let (status, json) = post_contract(body).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(json["error"]["code"], "parse_frontmatter");
    let message = json["error"]["message"].as_str().expect("a message");
    assert!(
        message.starts_with("line 2: "),
        "the YAML failure line is prefixed: {message}"
    );
}

#[tokio::test]
async fn a_prompt_without_args_yields_the_implicit_prose_field() {
    let text = "---\nname: plain\ndescription: no args\n---\n\n# Plain\n\n## S\n\np\n";
    let body = serde_json::json!({ "name": "plain", "text": text });
    let (status, json) = post_contract(body).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json["args"],
        serde_json::json!({
            "implicit": true,
            "fields": [
                { "name": "prose", "type": "string", "optional": true, "default": null, "description": "Freeform input for this prompt" },
            ],
        })
    );
}

#[tokio::test]
async fn a_lua_error_answers_parse_lua() {
    let text =
        "---\nname: badlua\ndescription: bad lua\n---\n\n# Bad\n\n## S\n\n```lua\nreturn (\n```\n";
    let body = serde_json::json!({ "name": "badlua", "text": text });
    let (status, json) = post_contract(body).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(json["error"]["code"], "parse_lua");
}

#[tokio::test]
async fn the_dto_serializes_kind_tags_and_nulls() {
    let text = "---\nname: tags\ndescription: kind tags\ntools:\n  exact_one: ns/pack/tool\n  other_one: ns/pack/other\n---\n\n# Tags\n\n## S\n\np\n";
    let body = serde_json::json!({ "name": "tags", "text": text });
    let (status, json) = post_contract(body).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["tools"][0]["kind"], "exact");
    assert_eq!(json["tools"][0]["path"], "ns/pack/tool");
    assert_eq!(json["tools"][1]["kind"], "exact");
    assert_eq!(json["tools"][1]["path"], "ns/pack/other");
    assert!(json["tools"][1].get("want").is_none());
    // Absent declarations serialize as explicit nulls, not missing keys.
    assert!(json["input"].is_null());
    assert!(json["output"].is_null());
    assert!(json["max_tool_iterations"].is_null());
    assert!(json["promptforge"].is_null());
}
