//! Tests for the call metrics and the raw exchange `finish` builds: the
//! backend sections and the client clock merge into one `CallMetrics`, and
//! the request and reassembled response bodies travel as the raw exchange.

use promptforge::metrics::{LlamaTimings, Usage, VllmMetrics};
use serde_json::{Value, json};

use super::*;

/// Feeds every payload into a fresh accumulator, as the read loop does.
fn accumulate(payloads: &[Value]) -> StreamAccumulator {
    let mut accumulator = StreamAccumulator::new();
    for payload in payloads {
        accumulator
            .apply(&payload.to_string(), &|_| {})
            .expect("fixture payloads are well-formed");
    }
    accumulator
}

fn text_and_stop() -> [Value; 2] {
    [
        json!({
            "model": "qwen3-30b",
            "choices": [{ "index": 0, "delta": { "content": "Hello!" }, "finish_reason": null }]
        }),
        json!({
            "model": "qwen3-30b",
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }]
        }),
    ]
}

fn client_timing() -> ClientTiming {
    ClientTiming {
        ttft_ms: Some(9.5),
        mean_itl_ms: None,
        e2e_ms: 41.5,
    }
}

#[test]
fn finish_merges_every_reported_section_and_the_client_clock_into_one_call_metrics() {
    let [text, stop] = text_and_stop();
    let summary = json!({
        "model": "qwen3-30b",
        "choices": [],
        "usage": {
            "prompt_tokens": 20,
            "completion_tokens": 5,
            "total_tokens": 25,
            "prompt_tokens_details": { "cached_tokens": 16 },
            "completion_tokens_details": { "reasoning_tokens": 2 }
        },
        "timings": {
            "prompt_n": 20, "prompt_ms": 12.5, "prompt_per_second": 1600.0,
            "predicted_n": 5, "predicted_ms": 30.5, "predicted_per_second": 98.5
        },
        "metrics": { "time_to_first_token_ms": 8.5 }
    });
    let completion = accumulate(&[text, stop, summary])
        .finish(json!({ "model": "m" }), Some(client_timing()))
        .expect("a complete turn finishes");
    assert_eq!(
        completion.metrics(),
        Some(&CallMetrics {
            usage: Some(Usage {
                prompt_tokens: 20,
                completion_tokens: 5,
                total_tokens: 25,
                cached_tokens: Some(16),
                reasoning_tokens: Some(2),
            }),
            llama: Some(LlamaTimings {
                prompt_n: 20,
                prompt_ms: 12.5,
                prompt_per_second: 1600.0,
                predicted_n: 5,
                predicted_ms: 30.5,
                predicted_per_second: 98.5,
                draft_n: 0,
                draft_n_accepted: 0,
            }),
            vllm: Some(VllmMetrics {
                time_to_first_token_ms: Some(8.5),
                generation_time_ms: None,
                queue_time_ms: None,
                mean_itl_ms: None,
                tokens_per_second: None,
            }),
            client: Some(client_timing()),
        })
    );
}

#[test]
fn a_stream_with_no_usage_chunk_yields_no_usage() {
    let completion = accumulate(&text_and_stop())
        .finish(json!({ "model": "m" }), Some(client_timing()))
        .expect("a complete turn finishes");
    let metrics = completion
        .metrics()
        .expect("the client clock alone still makes metrics");
    assert_eq!(metrics.usage, None);
    assert_eq!(metrics.llama, None);
    assert_eq!(metrics.vllm, None);
    assert_eq!(metrics.client, Some(client_timing()));
}

#[test]
fn a_turn_nothing_measured_has_no_metrics_at_all() {
    let completion = accumulate(&text_and_stop())
        .finish(json!({ "model": "m" }), None)
        .expect("a complete turn finishes");
    assert_eq!(completion.metrics(), None);
}

#[test]
fn a_malformed_usage_section_degrades_and_leaves_the_other_sections() {
    let [text, stop] = text_and_stop();
    let summary = json!({
        "choices": [],
        "usage": "broken",
        "timings": {
            "prompt_n": 7, "prompt_ms": 12.5, "prompt_per_second": 560.0,
            "predicted_n": 3, "predicted_ms": 30.5, "predicted_per_second": 98.5
        }
    });
    let completion = accumulate(&[text, stop, summary])
        .finish(Value::Null, None)
        .expect("metadata never fails a usable turn");
    let metrics = completion.metrics().expect("the timings were measured");
    assert_eq!(metrics.usage, None, "a malformed usage degrades");
    assert!(metrics.llama.is_some(), "its sibling section survives");
    assert_eq!(completion.metadata_diagnostics().len(), 1);
}

#[test]
fn finish_attaches_the_request_and_the_reassembled_response_as_the_raw_exchange() {
    let request = json!({ "model": "m", "messages": [{ "role": "user", "content": "hi" }] });
    let accumulator = accumulate(&text_and_stop());
    let response = accumulate(&text_and_stop()).into_body();
    let completion = accumulator
        .finish(request.clone(), None)
        .expect("a complete turn finishes");
    let raw = completion
        .raw()
        .expect("the stream reader attaches the raw exchange");
    assert_eq!(raw.request(), &request);
    assert_eq!(raw.response(), &response);
    assert_eq!(
        raw.response().pointer("/choices/0/message/content"),
        Some(&json!("Hello!")),
        "the response is the buffered shape a non-streaming backend returns"
    );
}
