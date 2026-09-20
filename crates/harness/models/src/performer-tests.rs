//! The chat performer against the axum mock gateway: a streamed round's
//! deltas reach the sink in wire order, a round without a live consumer
//! sends none, and a sink nobody drains does not fail the round.

use std::num::NonZeroU32;

use harness_runner::performers::ChatPerformer;
use promptforge_api_runtime::model::{
    CompletionOptions, CompletionResult, Message, ModelBinding, ModelId, ModelInvocation,
    StreamDelta,
};
use tokio::sync::mpsc;

use super::GatewayChatPerformer;
use crate::transport::tests::{content_chunk, sse_body, sse_client};

/// A binding for the round; the performer runs the round under the
/// effect's frozen options, so the binding's own fields are inert here.
fn binding() -> ModelBinding {
    ModelBinding::new(
        "writer",
        "the round's model",
        ModelId::from_validated("gateway", "m"),
        ModelInvocation {
            temperature: None,
            max_tokens: None,
            thinking: None,
        },
        NonZeroU32::new(4096).expect("4096 is non-zero"),
    )
}

/// A three-fragment reply closed by a stop finish.
fn three_fragments() -> String {
    sse_body(&[
        content_chunk("one "),
        content_chunk("two "),
        content_chunk("three"),
        serde_json::json!({
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }]
        }),
    ])
}

/// Every delta the sink's receiver holds, in arrival order.
fn drain(rx: &mut mpsc::UnboundedReceiver<StreamDelta>) -> Vec<StreamDelta> {
    let mut deltas = Vec::new();
    while let Ok(delta) = rx.try_recv() {
        deltas.push(delta);
    }
    deltas
}

fn reply_of(result: &CompletionResult) -> &str {
    match result {
        CompletionResult::Text(text) => text,
        other => panic!("the round replies with text: {other:?}"),
    }
}

#[tokio::test]
async fn a_streamed_round_sends_its_deltas_to_the_sink_in_wire_order() {
    let client = sse_client(three_fragments()).await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let performer = GatewayChatPerformer::new(client, tx);

    let completion = performer
        .chat(
            binding(),
            vec![Message::user("hi")],
            Vec::new(),
            CompletionOptions::new("m"),
            true,
        )
        .await
        .expect("the mock round completes");
    assert_eq!(reply_of(completion.result()), "one two three");
    assert_eq!(
        drain(&mut rx),
        vec![
            StreamDelta::Text("one ".to_owned()),
            StreamDelta::Text("two ".to_owned()),
            StreamDelta::Text("three".to_owned()),
        ],
        "each fragment reaches the sink as it arrives, in the stream's order"
    );
}

#[tokio::test]
async fn a_round_without_a_live_consumer_sends_no_deltas() {
    let client = sse_client(three_fragments()).await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let performer = GatewayChatPerformer::new(client, tx);

    let completion = performer
        .chat(
            binding(),
            vec![Message::user("hi")],
            Vec::new(),
            CompletionOptions::new("m"),
            false,
        )
        .await
        .expect("the mock round completes");
    assert_eq!(
        reply_of(completion.result()),
        "one two three",
        "the completed reply still travels in the answer"
    );
    assert!(
        drain(&mut rx).is_empty(),
        "a nested infer's fragments have no consumer and drop at the performer"
    );
}

#[tokio::test]
async fn a_sink_nobody_drains_does_not_fail_the_round() {
    let client = sse_client(three_fragments()).await;
    let (tx, rx) = mpsc::unbounded_channel::<StreamDelta>();
    drop(rx);
    let performer = GatewayChatPerformer::new(client, tx);

    let completion = performer
        .chat(
            binding(),
            vec![Message::user("hi")],
            Vec::new(),
            CompletionOptions::new("m"),
            true,
        )
        .await
        .expect("a closed sink drops the deltas and the round still completes");
    assert_eq!(reply_of(completion.result()), "one two three");
}
