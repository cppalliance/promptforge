//! The streamed round end to end: what goes on the wire, and how the
//! stream comes back as one completion.

use promptforge::model::{CompletionResult, Message, StreamDelta};

use super::*;
use crate::CompletionErrorKind;

#[tokio::test]
async fn complete_sends_completion_options_and_stream_flags_on_the_wire() {
    use std::sync::{Arc, Mutex};

    use axum::Router;
    use axum::extract::Json;
    use axum::routing::post;

    let captured: Arc<Mutex<Option<Value>>> = Arc::new(Mutex::new(None));
    let slot = Arc::clone(&captured);
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move |Json(body): Json<Value>| {
            let slot = Arc::clone(&slot);
            async move {
                *slot.lock().expect("capture lock") = Some(body);
                (
                    [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                    ok_stream(),
                )
            }
        }),
    );
    let client = client_for(app).await;
    let options = CompletionOptions::new("analyst")
        .with_temperature(0.0)
        .expect("0.0 is valid")
        .with_max_tokens(std::num::NonZeroU32::new(128).expect("128 is non-zero"))
        .with_thinking(false);
    client
        .complete(&[Message::user("hi")], None, &options, |_| {})
        .await
        .unwrap();
    let body = captured.lock().expect("capture lock").clone().unwrap();
    assert_eq!(body["model"], "analyst");
    assert_eq!(body["temperature"], 0.0);
    assert_eq!(body["max_tokens"], 128);
    assert_eq!(body["chat_template_kwargs"]["enable_thinking"], false);
    // The one completion method always streams and always asks for the
    // final usage chunk.
    assert_eq!(body["stream"], true);
    assert_eq!(body["stream_options"]["include_usage"], true);
}

#[tokio::test]
async fn complete_hard_fails_on_empty_model_reply() {
    // A stream that holds only reasoning and a stop finish has no
    // product; the accumulated turn must fail exactly like the buffered
    // equivalent, with the finish_reason surviving.
    let client = sse_client(sse_body(&[
        serde_json::json!({ "choices": [{ "index": 0,
            "delta": { "reasoning_content": "ignored" } }] }),
        serde_json::json!({ "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }] }),
    ]))
    .await;
    let err = client
        .complete(&[Message::user("hi")], None, &openai_options(), |_| {})
        .await
        .expect_err("empty product must fail");
    assert_eq!(err.kind(), CompletionErrorKind::EmptyReply);
    assert_eq!(
        err.finish_reason(),
        Some("stop"),
        "the finish_reason must survive the conversion into CompletionError"
    );
    assert!(matches!(Error::from(err), Error::EmptyModelReply { .. }));
}

#[tokio::test]
async fn streamed_text_usage_timings_and_client_timing_accumulate() {
    // The llama.cpp streamed shape: content fragments, a finish chunk, and
    // the include_usage summary chunk holding usage plus timings. The
    // accumulated completion must match the buffered equivalent while the
    // deltas reach the callback in order, and the client's own clock must
    // populate ClientTiming.
    let client = sse_client(sse_body(&[
        content_chunk("Hel"),
        content_chunk("lo!"),
        serde_json::json!({
            "model": "qwen3-30b",
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }]
        }),
        serde_json::json!({
            "model": "qwen3-30b",
            "choices": [],
            "usage": { "completion_tokens": 3, "prompt_tokens": 7, "total_tokens": 10 },
            "timings": {
                "prompt_n": 7, "prompt_ms": 12.5, "prompt_per_second": 560.0,
                "predicted_n": 3, "predicted_ms": 30.5, "predicted_per_second": 98.5
            }
        }),
    ]))
    .await;
    let seen = std::sync::Mutex::new(Vec::new());
    let completion = client
        .complete(&[Message::user("hi")], None, &openai_options(), |delta| {
            seen.lock().expect("delta log").push(delta);
        })
        .await
        .expect("a streamed text turn completes");
    match completion.result() {
        CompletionResult::Text(text) => assert_eq!(text, "Hello!"),
        other => panic!("expected text, got {other:?}"),
    }
    assert_eq!(
        *seen.lock().expect("delta log"),
        vec![
            StreamDelta::Text("Hel".to_owned()),
            StreamDelta::Text("lo!".to_owned()),
        ],
        "each content fragment reaches the callback live, in order"
    );
    assert_eq!(completion.finish_reason(), Some("stop"));
    assert_eq!(completion.model(), "qwen3-30b");
    let usage = completion.usage().expect("usage from the final chunk");
    assert_eq!(usage.total_tokens, 10);
    let timings = completion
        .llama_timings()
        .expect("timings from the final chunk");
    assert_eq!(timings.predicted_n, 3);
    let timing = completion
        .client_timing()
        .expect("the streaming transport measures its own clock");
    assert!(
        timing.ttft_ms.is_some_and(|ttft| ttft >= 0.0),
        "TTFT is measured once the first delta arrives: {timing:?}"
    );
    assert!(
        timing.mean_itl_ms.is_some_and(|itl| itl >= 0.0),
        "mean ITL is measured with two delta chunks: {timing:?}"
    );
    assert!(timing.e2e_ms >= 0.0);
}

#[tokio::test]
async fn streamed_reasoning_stays_a_side_channel() {
    let client = sse_client(sse_body(&[
        serde_json::json!({ "choices": [{ "index": 0,
            "delta": { "reasoning_content": "scratch" } }] }),
        content_chunk("answer"),
        serde_json::json!({
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }]
        }),
    ]))
    .await;
    let seen = std::sync::Mutex::new(Vec::new());
    let completion = client
        .complete(&[Message::user("hi")], None, &openai_options(), |delta| {
            seen.lock().expect("delta log").push(delta);
        })
        .await
        .expect("reasoning plus text completes");
    match completion.result() {
        CompletionResult::Text(text) => {
            assert_eq!(
                text, "answer",
                "reasoning is never promoted into the answer"
            );
        }
        other => panic!("expected text, got {other:?}"),
    }
    assert_eq!(completion.reasoning_content(), Some("scratch"));
    assert_eq!(
        *seen.lock().expect("delta log"),
        vec![
            StreamDelta::Reasoning("scratch".to_owned()),
            StreamDelta::Text("answer".to_owned()),
        ],
        "reasoning and text deltas arrive separated"
    );
}

#[tokio::test]
async fn streamed_tool_call_fragments_reassemble_into_the_batch() {
    let client = sse_client(sse_body(&[
        serde_json::json!({ "choices": [{ "index": 0, "delta": { "tool_calls": [{
            "index": 0, "id": "call_1", "type": "function",
            "function": { "name": "web_search", "arguments": "{\"qu" }
        }] } }] }),
        serde_json::json!({ "choices": [{ "index": 0, "delta": { "tool_calls": [{
            "index": 0, "function": { "arguments": "ery\":\"rust\"}" }
        }] } }] }),
        serde_json::json!({
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "tool_calls" }]
        }),
    ]))
    .await;
    let completion = client
        .complete(&[Message::user("hi")], None, &openai_options(), |_| {})
        .await
        .expect("a streamed tool-call turn completes");
    match completion.result() {
        CompletionResult::ToolCalls(calls) => {
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].id(), "call_1");
            assert_eq!(calls[0].name(), "web_search");
            assert_eq!(
                calls[0].arguments().to_json_string(),
                "{\"query\":\"rust\"}",
                "argument fragments buffer until the batch is whole"
            );
        }
        other => panic!("expected tool calls, got {other:?}"),
    }
}

#[tokio::test]
async fn truncated_tool_call_batch_fails_the_completion() {
    // A length or content_filter finish with tool calls means the batch may
    // hold partial JSON arguments; the whole batch fails rather than
    // executing a fragment.
    for reason in ["length", "content_filter"] {
        let client = sse_client(sse_body(&[
            serde_json::json!({ "choices": [{ "index": 0, "delta": { "tool_calls": [{
                "index": 0, "id": "c1", "type": "function",
                "function": { "name": "t", "arguments": "{\"whole\":true}" }
            }] } }] }),
            serde_json::json!({
                "choices": [{ "index": 0, "delta": {}, "finish_reason": reason }]
            }),
        ]))
        .await;
        let err = client
            .complete(&[Message::user("hi")], None, &openai_options(), |_| {})
            .await
            .expect_err("a truncated tool-call batch must fail");
        assert_eq!(
            err.kind(),
            CompletionErrorKind::MalformedResponse,
            "finish_reason {reason:?}"
        );
        assert!(
            err.to_string().contains("truncated"),
            "the error names the truncation: {err}"
        );
    }
}

#[tokio::test]
async fn truncated_text_still_returns_with_its_finish_reason() {
    // The truncation rule fails tool-call batches only: partial TEXT is
    // returned with finish_reason "length" so the caller can report it.
    let client = sse_client(sse_body(&[
        content_chunk("partial answ"),
        serde_json::json!({
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "length" }]
        }),
    ]))
    .await;
    let completion = client
        .complete(&[Message::user("hi")], None, &openai_options(), |_| {})
        .await
        .expect("truncated text is still a product");
    match completion.result() {
        CompletionResult::Text(text) => assert_eq!(text, "partial answ"),
        other => panic!("expected text, got {other:?}"),
    }
    assert_eq!(completion.finish_reason(), Some("length"));
}

#[tokio::test]
async fn mid_stream_error_envelope_is_a_transport_failure() {
    // The gateway relays a mid-flight failure as a data: error envelope on
    // an already-open 200 stream; the completion classifies it as a
    // transport failure, never as model output.
    let client = sse_client(sse_body(&[
        content_chunk("par"),
        serde_json::json!({ "error": {
            "message": "upstream died", "type": "upstream", "code": "upstream_transport"
        } }),
    ]))
    .await;
    let err = client
        .complete(&[Message::user("hi")], None, &openai_options(), |_| {})
        .await
        .expect_err("an error envelope must fail the completion");
    assert_eq!(err.kind(), CompletionErrorKind::Transport);
    let source = std::error::Error::source(&err)
        .expect("the envelope message must be the cause")
        .to_string();
    assert!(source.contains("upstream died"), "cause: {source}");
}
