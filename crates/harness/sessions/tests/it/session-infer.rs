//! The mock-gateway session round trip: a model-backed agent program drives
//! real inference- and chat-origin `assistant_reply` events through the
//! harness, and the tool-less infer reproduction pins the reply between the
//! completed turn and the section chunk success.

use super::*;

use axum::Router;
use axum::http::header::CONTENT_TYPE;
use axum::routing::{get, post};
use harness_runner::spawn::spawn_tagged;
use harness_runner::test_support::mock_tag;

/// A session program that infers once, then chats once: the mixed
/// model-round sequence the reply-id rule must number in order. The second
/// `user_input()` parks the run between the two rounds, so the infer reply
/// settles the accepted turn before any chat round exists.
const MIXED: &str = "---\nname: mixed\ndescription: infers then chats\npromptforge: 0\n\
    models:\n  writer: {}\n---\n\n\
    # Mixed\n\n```lua\nmodels.default('writer')\n```\n\n\
    ## Only\n\n```lua\n\
    local first = user_input()\n\
    local inferred = models.infer(first)\n\
    local second = user_input()\n\
    local msgs = messages.new()\n\
    msgs:user(second)\n\
    models.loop(msgs)\n\
    return inferred .. '|' .. msgs[#msgs].content\n\
    ```\n";

/// The model id the mock gateway advertises and the catalog binds.
const MOCK_MODEL: &str = "mock-model";

/// The reply the mock gateway streams for every round.
const MOCK_REPLY: &str = "from the mock";

/// One round's stream: the reply in one content chunk, then a stop.
fn reply_stream() -> String {
    let mut body = String::new();
    for event in [
        serde_json::json!({
            "model": MOCK_MODEL,
            "choices": [{ "index": 0, "delta": { "content": MOCK_REPLY }, "finish_reason": null }]
        }),
        serde_json::json!({
            "model": MOCK_MODEL,
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }]
        }),
    ] {
        body.push_str("data: ");
        body.push_str(&event.to_string());
        body.push_str("\n\n");
    }
    body.push_str("data: [DONE]\n\n");
    body
}

/// The catalog the mock gateway serves at `GET /v1/models`: the one
/// inference model the harness binds its roles to.
fn catalog_body() -> serde_json::Value {
    serde_json::json!({
        "data": [{
            "id": MOCK_MODEL,
            "description": "the mock model",
            "context": 131_072,
            "thinking": "switchable",
        }]
    })
}

/// Serves the model catalog and a streaming chat completion on a loopback
/// port, returning the base URL to bind a harness to.
async fn mock_gateway() -> String {
    async fn models() -> axum::Json<serde_json::Value> {
        axum::Json(catalog_body())
    }
    async fn completions() -> ([(axum::http::HeaderName, &'static str); 1], String) {
        ([(CONTENT_TYPE, "text/event-stream")], reply_stream())
    }
    let app = Router::new()
        .route("/v1/models", get(models))
        .route("/v1/chat/completions", post(completions));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    spawn_tagged(mock_tag(), async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

/// A harness over a fresh `<dir>/agents` directory holding `name.md` with
/// `program`, bound to `base_url` with a catalog whose one chat-capable entry
/// names the mock gateway's model.
fn harness_for(dir: &Path, base_url: &str, name: &str, program: &str) -> Harness {
    let agents = dir.join("agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(agents.join(format!("{name}.md")), program).unwrap();
    let harness = Harness::new(HarnessConfig {
        agents_path: agents,
        state_dir: dir.join("state"),
    });
    harness.set_gateway(GatewayBinding {
        base_url: base_url.to_owned(),
        key: "k".to_owned(),
        generation: 1,
    });
    harness.set_catalog(CatalogBinding {
        generation: 1,
        models: vec![serde_json::json!({ "kind": "chat", "id": MOCK_MODEL })],
    });
    harness
}

/// A harness holding `mixed.md`: [`harness_for`] with [`MIXED`].
fn mixed_harness(dir: &Path, base_url: &str) -> Harness {
    harness_for(dir, base_url, "mixed", MIXED)
}

/// A session program whose only model call is a tool-less `models.infer`:
/// the report's reproduction prompt. It parks on nothing, so it runs to
/// completion and the transcript holds its whole event stream.
const INFERS: &str = "---\nname: infers\ndescription: infers only\npromptforge: 0\n\
    models:\n  writer: {}\n---\n\n\
    # Infers\n\n```lua\nmodels.default('writer')\n```\n\n\
    ## Only\n\n```lua\nreturn models.infer('prose')\n```\n";

/// A harness holding `infers.md` that makes one tool-less infer call and
/// returns: [`harness_for`] with [`INFERS`].
fn infer_harness(dir: &Path, base_url: &str) -> Harness {
    harness_for(dir, base_url, "infers", INFERS)
}

#[tokio::test]
async fn a_mixed_infer_then_chat_session_numbers_each_reply_in_round_order() {
    let dir = tempfile::tempdir().unwrap();
    let base = mock_gateway().await;
    let harness = mixed_harness(dir.path(), &base);
    let session = launch_agent(&harness, "mixed").await;
    let mut waits = session.subscribe_waits();
    let first = required_token(&mut waits).await;

    session
        .send_input(&first, "first".to_owned(), || {})
        .unwrap();
    // The infer reply precedes this second question; the chat reply follows
    // the second answer.
    let second = required_token(&mut waits).await;
    session
        .send_input(&second, "second".to_owned(), || {})
        .unwrap();

    // The program returned after the chat: the session ends on its own.
    wait_for(&session, SessionState::Closed).await;

    let events = session.transcript(0).await.unwrap();
    let replies: Vec<(&str, Option<u64>)> = events
        .iter()
        .filter_map(|event| {
            let kind = event.event.get("kind")?.as_str()?;
            if kind != "assistant_reply" {
                return None;
            }
            let origin = event
                .event
                .get("origin")
                .and_then(serde_json::Value::as_str)?;
            Some((origin, event.reply))
        })
        .collect();
    assert_eq!(
        replies,
        vec![("infer", Some(0)), ("chat", Some(1))],
        "the infer reply takes round 0 and advances; the chat reply takes round 1"
    );
}

#[tokio::test]
async fn an_infer_reply_settles_the_accepted_turn_so_a_new_catalog_retires_the_run() {
    let dir = tempfile::tempdir().unwrap();
    let base = mock_gateway().await;
    let harness = mixed_harness(dir.path(), &base);
    let session = launch_agent(&harness, "mixed").await;
    let mut waits = session.subscribe_waits();
    let first = required_token(&mut waits).await;

    // The answer arms the accepted turn; the infer reply settles it and the
    // program parks on its second question.
    session
        .send_input(&first, "first".to_owned(), || {})
        .unwrap();
    let second = required_token(&mut waits).await;
    assert_ne!(second, first, "wait tokens are single-use");

    // The infer settled the accepted turn, so a replacement catalog retires
    // the run at once. Were an infer-origin `AssistantReply` outside the
    // settle arm, the still open accepted turn would defer the retirement
    // onto a settlement that never arrives, and this test would time out on
    // the cancelled frame.
    harness.set_catalog(CatalogBinding {
        generation: 2,
        models: vec![serde_json::json!({
            "kind": "chat",
            "id": MOCK_MODEL,
            "description": "other",
        })],
    });
    cancelled_frame(&mut waits, &second).await;
    let third = required_token(&mut waits).await;
    assert_ne!(third, second, "the relaunch parks under a fresh token");
    assert_eq!(
        session.run_ids().len(),
        2,
        "the settled infer turn let the new catalog retire the run"
    );

    assert!(harness.close(session.id()));
    wait_for(&session, SessionState::Closed).await;
}

#[tokio::test]
async fn a_tool_less_infer_reply_sits_between_the_completed_turn_and_the_chunk_success() {
    // The report's reproduction: a section whose only model call is
    // `return models.infer(prose)`. Its session stream must carry the
    // programmatic reply after the completed boundary and before the
    // section's chunk success, with the model's text and an infer origin
    // (not a chat turn).
    let dir = tempfile::tempdir().unwrap();
    let base = mock_gateway().await;
    let harness = infer_harness(dir.path(), &base);
    let session = launch_agent(&harness, "infers").await;
    wait_for(&session, SessionState::Closed).await;

    let events = session.transcript(0).await.unwrap();
    let kinds: Vec<&str> = events
        .iter()
        .filter_map(|event| event.event.get("kind")?.as_str())
        .collect();
    let completed = kinds
        .iter()
        .position(|kind| *kind == "model_turn_completed")
        .expect("the model turn completed");
    let reply = kinds
        .iter()
        .position(|kind| *kind == "assistant_reply")
        .expect("the tool-less infer reported a reply");
    assert!(
        completed < reply,
        "the infer reply follows the completed turn: {kinds:?}"
    );
    assert!(
        kinds[reply + 1..].contains(&"lua_chunk_succeeded"),
        "the infer reply precedes the section's chunk success: {kinds:?}"
    );
    let reply_event = events
        .iter()
        .find(|event| {
            event.event.get("kind").and_then(serde_json::Value::as_str) == Some("assistant_reply")
        })
        .expect("the tool-less infer reported a reply");
    let text = reply_event
        .event
        .get("text")
        .and_then(serde_json::Value::as_str);
    assert_eq!(
        text,
        Some(MOCK_REPLY),
        "the infer reply carries the model text"
    );
    assert_eq!(
        reply_event
            .event
            .get("origin")
            .and_then(serde_json::Value::as_str),
        Some("infer"),
        "the infer round's reply is an assistant reply tagged infer: {:?}",
        reply_event.event
    );
    assert!(
        !events.iter().any(|event| {
            event
                .event
                .get("origin")
                .and_then(serde_json::Value::as_str)
                == Some("chat")
        }),
        "a tool-less infer emits no chat-origin reply: {kinds:?}"
    );
}
