//! Tests for the streaming accumulator and the SSE data-line scanner.

use serde_json::Value;

use super::*;
use crate::client::CompletionResult;
use crate::model::CompletionErrorKind;

fn no_delta(_: StreamDelta) {}

/// Feeds every `data:` payload into a fresh accumulator and returns it.
fn accumulate(payloads: &[Value]) -> StreamAccumulator {
    let mut accumulator = StreamAccumulator::new();
    for payload in payloads {
        accumulator
            .apply(&payload.to_string(), &no_delta)
            .expect("fixture payloads are well-formed");
    }
    accumulator
}

fn content_chunk(text: &str) -> Value {
    serde_json::json!({
        "model": "qwen3-30b",
        "choices": [{ "index": 0, "delta": { "content": text }, "finish_reason": null }]
    })
}

#[test]
fn scanner_splits_data_lines_and_skips_noise() {
    let mut scanner = SseScanner::new();
    scanner.extend(b": comment\nevent: message\ndata: {\"a\":1}\r\n\ndata: [DO");
    assert_eq!(scanner.next_data().as_deref(), Some("{\"a\":1}"));
    assert_eq!(scanner.next_data(), None, "partial line stays buffered");
    scanner.extend(b"NE]\n");
    assert_eq!(scanner.next_data().as_deref(), Some("[DONE]"));
}

#[test]
fn streamed_accumulation_matches_the_buffered_fixture_byte_for_byte() {
    // The buffered llama.cpp fixture from the normalize suite, split
    // into a streamed form: the reassembled body must normalize to the
    // same turn and metadata, with the answer text byte-identical.
    let usage =
        serde_json::json!({ "completion_tokens": 3, "prompt_tokens": 7, "total_tokens": 10 });
    let timings = serde_json::json!({
        "prompt_n": 7, "prompt_ms": 12.5, "prompt_per_second": 560.0,
        "predicted_n": 3, "predicted_ms": 30.5, "predicted_per_second": 98.5
    });
    let accumulator = accumulate(&[
        content_chunk("Hel"),
        content_chunk("lo \u{1F980}"),
        content_chunk("!"),
        serde_json::json!({
            "model": "qwen3-30b",
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }]
        }),
        serde_json::json!({
            "model": "qwen3-30b",
            "choices": [],
            "usage": usage,
            "timings": timings
        }),
    ]);
    let body = accumulator.into_body();
    assert_eq!(
        body.pointer("/choices/0/message/content")
            .and_then(Value::as_str),
        Some("Hello \u{1F980}!"),
        "fragments concatenate byte-for-byte"
    );
    assert_eq!(
        body.pointer("/choices/0/finish_reason")
            .and_then(Value::as_str),
        Some("stop")
    );
    assert_eq!(body.get("model").and_then(Value::as_str), Some("qwen3-30b"));
    assert_eq!(body.get("usage"), Some(&usage), "usage kept verbatim");
    assert_eq!(body.get("timings"), Some(&timings), "timings kept verbatim");
}

#[test]
fn finish_normalizes_the_turn_and_returns_the_metadata() {
    let accumulator = accumulate(&[
        content_chunk("Hel"),
        content_chunk("lo!"),
        serde_json::json!({
            "model": "qwen3-30b",
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }]
        }),
        serde_json::json!({
            "model": "qwen3-30b",
            "choices": [],
            "usage": { "completion_tokens": 3, "prompt_tokens": 7, "total_tokens": 10 }
        }),
    ]);
    let request = serde_json::json!({ "model": "m" });
    let completion = accumulator
        .finish(request.clone(), None)
        .expect("a streamed text turn finishes");
    match completion.result() {
        CompletionResult::Text(text) => assert_eq!(text, "Hello!"),
        other => panic!("expected text, got {other:?}"),
    }
    assert_eq!(completion.finish_reason(), Some("stop"));
    assert_eq!(completion.model(), "qwen3-30b");
    assert_eq!(completion.usage().map(|usage| usage.total_tokens), Some(10));
    assert_eq!(completion.request_body, request);
    assert!(completion.client_timing().is_none());
}

#[test]
fn finish_fails_a_tool_call_batch_truncated_by_length_or_content_filter() {
    // A length or content_filter finish with tool calls means the batch may
    // hold partial JSON arguments; the whole batch fails rather than
    // executing a fragment.
    for reason in ["length", "content_filter"] {
        let accumulator = accumulate(&[
            serde_json::json!({ "choices": [{ "index": 0, "delta": { "tool_calls": [{
                "index": 0, "id": "c1", "type": "function",
                "function": { "name": "t", "arguments": "{\"whole\":true}" }
            }] } }] }),
            serde_json::json!({
                "choices": [{ "index": 0, "delta": {}, "finish_reason": reason }]
            }),
        ]);
        let error = accumulator
            .finish(Value::Null, None)
            .expect_err("a truncated tool-call batch must fail");
        assert_eq!(
            error.kind(),
            CompletionErrorKind::MalformedResponse,
            "finish_reason {reason:?}"
        );
        assert!(
            error.to_string().contains("truncated"),
            "the error names the truncation: {error}"
        );
    }
}

#[test]
fn finish_returns_truncated_text_with_its_finish_reason() {
    // The truncation rule fails tool-call batches only: partial TEXT is
    // returned with finish_reason "length" so the caller can report it.
    let completion = accumulate(&[
        content_chunk("partial answ"),
        serde_json::json!({
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "length" }]
        }),
    ])
    .finish(Value::Null, None)
    .expect("truncated text is still a product");
    match completion.result() {
        CompletionResult::Text(text) => assert_eq!(text, "partial answ"),
        other => panic!("expected text, got {other:?}"),
    }
    assert_eq!(completion.finish_reason(), Some("length"));
}

#[test]
fn finish_hard_fails_on_an_empty_model_reply() {
    // A stream that holds only reasoning and a stop finish has no
    // product; the accumulated turn must fail exactly like the buffered
    // equivalent, with the finish_reason surviving.
    let error = accumulate(&[
        serde_json::json!({ "choices": [{ "index": 0,
            "delta": { "reasoning_content": "ignored" } }] }),
        serde_json::json!({ "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }] }),
    ])
    .finish(Value::Null, None)
    .expect_err("empty product must fail");
    assert_eq!(error.kind(), CompletionErrorKind::EmptyReply);
    assert_eq!(
        error.finish_reason(),
        Some("stop"),
        "the finish_reason must survive the conversion into CompletionError"
    );
    assert!(matches!(Error::from(error), Error::EmptyModelReply { .. }));
}

#[test]
fn tool_call_fragments_buffer_across_chunks_by_index() {
    // OpenAI streams a call's name once and its arguments in pieces;
    // interleaved fragments for two calls must land on their own
    // buffers, keyed by `index`, and reassemble whole.
    let accumulator = accumulate(&[
        serde_json::json!({ "choices": [{ "index": 0, "delta": { "tool_calls": [
            { "index": 0, "id": "call_a", "type": "function",
              "function": { "name": "search", "arguments": "{\"qu" } }
        ] } }] }),
        serde_json::json!({ "choices": [{ "index": 0, "delta": { "tool_calls": [
            { "index": 1, "id": "call_b", "type": "function",
              "function": { "name": "fetch", "arguments": "{\"url\":\"x\"}" } }
        ] } }] }),
        serde_json::json!({ "choices": [{ "index": 0, "delta": { "tool_calls": [
            { "index": 0, "function": { "arguments": "ery\":\"a\"}" } }
        ] } }] }),
        serde_json::json!({
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "tool_calls" }]
        }),
    ]);
    assert!(!accumulator.tool_calls.is_empty());
    let body = accumulator.into_body();
    let calls = body
        .pointer("/choices/0/message/tool_calls")
        .and_then(Value::as_array)
        .expect("tool calls reassembled");
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0]["id"], "call_a");
    assert_eq!(calls[0]["function"]["arguments"], "{\"query\":\"a\"}");
    assert_eq!(calls[1]["id"], "call_b");
    assert_eq!(calls[1]["function"]["name"], "fetch");
}

#[test]
fn reasoning_and_text_deltas_reach_the_callback_separated_in_order() {
    let seen = std::sync::Mutex::new(Vec::new());
    let mut accumulator = StreamAccumulator::new();
    let record = |delta: StreamDelta| seen.lock().expect("delta log").push(delta);
    for payload in [
        serde_json::json!({ "choices": [{ "index": 0,
            "delta": { "reasoning_content": "think" } }] }),
        serde_json::json!({ "choices": [{ "index": 0, "delta": { "content": "ans" } }] }),
        serde_json::json!({ "choices": [{ "index": 0, "delta": { "content": "wer" } }] }),
    ] {
        accumulator
            .apply(&payload.to_string(), &record)
            .expect("well-formed");
    }
    assert_eq!(
        *seen.lock().expect("delta log"),
        vec![
            StreamDelta::Reasoning("think".to_owned()),
            StreamDelta::Text("ans".to_owned()),
            StreamDelta::Text("wer".to_owned()),
        ]
    );
    let body = accumulator.into_body();
    assert_eq!(
        body.pointer("/choices/0/message/reasoning_content")
            .and_then(Value::as_str),
        Some("think"),
        "reasoning stays a side channel on the reassembled message"
    );
    assert_eq!(
        body.pointer("/choices/0/message/content")
            .and_then(Value::as_str),
        Some("answer")
    );
}

#[test]
fn empty_choices_usage_chunk_is_metadata_not_a_turn() {
    // The `stream_options.include_usage` summary chunk has an empty
    // `choices` array; it must be consumed as metadata, never indexed
    // for a choice and never counted as a content delta.
    let mut accumulator = StreamAccumulator::new();
    let applied = accumulator
        .apply(
            &serde_json::json!({ "choices": [], "usage": { "prompt_tokens": 1,
                "completion_tokens": 2, "total_tokens": 3 } })
            .to_string(),
            &no_delta,
        )
        .expect("summary chunk is well-formed");
    assert_eq!(applied, Applied::Chunk { delta: false });
    let body = accumulator.into_body();
    assert_eq!(
        body.pointer("/usage/total_tokens").and_then(Value::as_u64),
        Some(3)
    );
}

#[test]
fn error_envelope_fails_the_stream_with_the_escaped_message() {
    let mut accumulator = StreamAccumulator::new();
    let error = accumulator
        .apply(
            &serde_json::json!({ "error": { "message": "upstream\ndied", "code": "x" } })
                .to_string(),
            &no_delta,
        )
        .expect_err("an error envelope must fail the stream");
    assert_eq!(error.kind(), CompletionErrorKind::Transport);
    let source = std::error::Error::source(&error)
        .expect("the envelope message is the cause")
        .to_string();
    assert!(source.contains("upstream\\ndied"), "escaped: {source}");
}

#[test]
fn malformed_chunks_are_rejected_not_skipped() {
    let cases: [(&str, &str); 4] = [
        ("not json", "undecodable payload"),
        ("{\"choices\":{}}", "non-array choices"),
        (
            "{\"choices\":[{\"index\":0,\"delta\":{\"content\":7}}]}",
            "non-string content",
        ),
        (
            "{\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"id\":\"x\"}]}}]}",
            "fragment without index",
        ),
    ];
    for (payload, label) in cases {
        let mut accumulator = StreamAccumulator::new();
        let error = accumulator.apply(payload, &no_delta).expect_err(label);
        assert_eq!(
            error.kind(),
            CompletionErrorKind::MalformedResponse,
            "{label}: {error:?}"
        );
    }
}

#[test]
fn a_malformed_chunk_preserves_the_decode_source() {
    let mut accumulator = StreamAccumulator::new();
    let error = accumulator
        .apply("{ not json", &no_delta)
        .expect_err("undecodable chunk must fail");
    let source =
        std::error::Error::source(&error).expect("the decode error must be a preserved source");
    assert!(
        source.downcast_ref::<serde_json::Error>().is_some(),
        "the preserved source must be the JSON decode error, got {source}"
    );
}

#[test]
fn non_first_choices_are_ignored_like_the_buffered_parser() {
    let accumulator = accumulate(&[
        content_chunk("kept"),
        serde_json::json!({ "choices": [{ "index": 1,
            "delta": { "content": "dropped" } }] }),
    ]);
    let body = accumulator.into_body();
    assert_eq!(
        body.pointer("/choices/0/message/content")
            .and_then(Value::as_str),
        Some("kept")
    );
}

#[test]
fn no_content_at_all_reassembles_null_content() {
    let accumulator = accumulate(&[serde_json::json!({
        "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }]
    })]);
    let body = accumulator.into_body();
    assert_eq!(
        body.pointer("/choices/0/message/content"),
        Some(&Value::Null),
        "a stream with no content fragments yields a null content"
    );
}

#[test]
fn escape_controls_neutralizes_control_bytes_and_bounds_length() {
    // F5: newlines and other control characters are escaped, not passed
    // through, so a body cannot forge log lines.
    let escaped = escape_controls("line1\nline2\r\u{7}end", 2000);
    assert!(!escaped.contains('\n'), "raw newline must be escaped");
    assert!(
        !escaped.contains('\r'),
        "raw carriage return must be escaped"
    );
    assert!(
        escaped.contains("\\n"),
        "escaped newline expected, got {escaped}"
    );
    assert_eq!(escape_controls("", 2000), "(empty body)");
    assert_eq!(escape_controls("abcdef", 3), "abc");
}
