//! Tests for the lenient call-metadata parse and its degrade policy.

use super::*;

/// Parses the metadata and counts its diagnostics, so the tests can pin
/// both halves of the degrade policy: malformed sections report, and
/// well-formed or absent sections stay silent.
fn with_warn_count(body: &Value) -> (ResponseMetadata, usize) {
    let metadata = response_metadata(body);
    let warnings = metadata.diagnostics.len();
    (metadata, warnings)
}

/// One assistant text choice, shared by the metadata fixture bodies.
fn reply_choice() -> Value {
    serde_json::json!([{
        "index": 0,
        "message": { "role": "assistant", "content": "hi" },
        "finish_reason": "stop"
    }])
}

#[test]
fn llama_body_parses_model_usage_and_timings() {
    let body = serde_json::json!({
        "id": "chatcmpl-llama",
        "object": "chat.completion",
        "created": 1_726_000_000_u64,
        "model": "qwen3-30b",
        "choices": reply_choice(),
        "usage": { "completion_tokens": 3, "prompt_tokens": 7, "total_tokens": 10 },
        "timings": {
            "prompt_n": 7,
            "prompt_ms": 12.5,
            "prompt_per_token_ms": 1.75,
            "prompt_per_second": 560.0,
            "predicted_n": 3,
            "predicted_ms": 30.5,
            "predicted_per_token_ms": 10.25,
            "predicted_per_second": 98.5,
            "draft_n": 4,
            "draft_n_accepted": 2
        }
    });

    let (metadata, warnings) = with_warn_count(&body);
    assert_eq!(warnings, 0, "a well-formed body must not warn");
    assert_eq!(metadata.model, "qwen3-30b");
    assert_eq!(
        metadata.metrics.usage,
        Some(Usage {
            prompt_tokens: 7,
            completion_tokens: 3,
            total_tokens: 10,
            cached_tokens: None,
            reasoning_tokens: None,
        }),
        "flat llama.cpp usage reports the token counts only"
    );
    assert_eq!(
        metadata.metrics.llama,
        Some(LlamaTimings {
            prompt_n: 7,
            prompt_ms: 12.5,
            prompt_per_second: 560.0,
            predicted_n: 3,
            predicted_ms: 30.5,
            predicted_per_second: 98.5,
            draft_n: 4,
            draft_n_accepted: 2,
        })
    );
    assert_eq!(metadata.metrics.vllm, None);
    assert_eq!(
        metadata.metrics.client, None,
        "only the transport's own clock fills the client section"
    );
}

#[test]
fn llama_timings_without_draft_counters_default_to_zero() {
    // Without a configured draft model llama.cpp omits the draft
    // counters entirely; zero drafted tokens is the truthful reading,
    // so the common non-speculative body must not degrade to None.
    let body = serde_json::json!({
        "model": "qwen3-30b",
        "choices": reply_choice(),
        "timings": {
            "prompt_n": 7,
            "prompt_ms": 12.5,
            "prompt_per_second": 560.0,
            "predicted_n": 3,
            "predicted_ms": 30.5,
            "predicted_per_second": 98.5
        }
    });

    let (metadata, warnings) = with_warn_count(&body);
    assert_eq!(warnings, 0);
    let timings = metadata.metrics.llama.unwrap();
    assert_eq!(timings.draft_n, 0);
    assert_eq!(timings.draft_n_accepted, 0);
}

#[test]
fn vllm_body_parses_metrics_and_cached_tokens() {
    let body = serde_json::json!({
        "id": "chatcmpl-vllm",
        "object": "chat.completion",
        "model": "meta-llama/Llama-3.1-8B-Instruct",
        "choices": reply_choice(),
        "usage": {
            "prompt_tokens": 20,
            "completion_tokens": 5,
            "total_tokens": 25,
            "prompt_tokens_details": { "cached_tokens": 16 }
        },
        "metrics": {
            "time_to_first_token_ms": 8.5,
            "generation_time_ms": 22.5,
            "queue_time_ms": 1.5,
            "mean_itl_ms": 7.5,
            "tokens_per_second": 133.5
        }
    });

    let (metadata, warnings) = with_warn_count(&body);
    assert_eq!(warnings, 0, "a well-formed body must not warn");
    assert_eq!(metadata.model, "meta-llama/Llama-3.1-8B-Instruct");
    assert_eq!(
        metadata.metrics.usage,
        Some(Usage {
            prompt_tokens: 20,
            completion_tokens: 5,
            total_tokens: 25,
            cached_tokens: Some(16),
            reasoning_tokens: None,
        }),
        "the prompt_tokens_details cache detail must flatten into usage"
    );
    assert_eq!(metadata.metrics.llama, None);
    assert_eq!(
        metadata.metrics.vllm,
        Some(VllmMetrics {
            time_to_first_token_ms: Some(8.5),
            generation_time_ms: Some(22.5),
            queue_time_ms: Some(1.5),
            mean_itl_ms: Some(7.5),
            tokens_per_second: Some(133.5),
        })
    );
}

#[test]
fn vllm_metrics_omit_what_was_not_measured() {
    let body = serde_json::json!({
        "model": "m",
        "choices": reply_choice(),
        "metrics": { "time_to_first_token_ms": 8.5 }
    });

    let (metadata, warnings) = with_warn_count(&body);
    assert_eq!(warnings, 0);
    assert_eq!(
        metadata.metrics.vllm,
        Some(VllmMetrics {
            time_to_first_token_ms: Some(8.5),
            generation_time_ms: None,
            queue_time_ms: None,
            mean_itl_ms: None,
            tokens_per_second: None,
        }),
        "fields vLLM did not measure stay None inside a parsed section"
    );
}

#[test]
fn frontier_body_parses_usage_detail_fields() {
    let body = serde_json::json!({
        "id": "chatcmpl-frontier",
        "object": "chat.completion",
        "model": "gpt-5.2",
        "choices": reply_choice(),
        "usage": {
            "prompt_tokens": 100,
            "completion_tokens": 40,
            "total_tokens": 140,
            "prompt_tokens_details": { "cached_tokens": 64, "audio_tokens": 0 },
            "completion_tokens_details": {
                "reasoning_tokens": 25,
                "audio_tokens": 0,
                "accepted_prediction_tokens": 0,
                "rejected_prediction_tokens": 0
            }
        }
    });

    let (metadata, warnings) = with_warn_count(&body);
    assert_eq!(warnings, 0, "a well-formed body must not warn");
    assert_eq!(metadata.model, "gpt-5.2");
    assert_eq!(
        metadata.metrics.usage,
        Some(Usage {
            prompt_tokens: 100,
            completion_tokens: 40,
            total_tokens: 140,
            cached_tokens: Some(64),
            reasoning_tokens: Some(25),
        })
    );
    assert_eq!(
        metadata.metrics.llama, None,
        "frontier bodies have no timings"
    );
    assert_eq!(
        metadata.metrics.vllm, None,
        "frontier bodies have no metrics"
    );
}

#[test]
fn absent_metadata_sections_are_none_without_warning() {
    let bare = serde_json::json!({ "model": "m", "choices": reply_choice() });
    let with_nulls = serde_json::json!({
        "model": "m",
        "choices": reply_choice(),
        "usage": null,
        "timings": null,
        "metrics": null
    });

    for body in [bare, with_nulls] {
        let (metadata, warnings) = with_warn_count(&body);
        assert_eq!(warnings, 0, "absence is normal, never a warning: {body}");
        assert_eq!(metadata.model, "m");
        assert_eq!(metadata.metrics.usage, None);
        assert_eq!(metadata.metrics.llama, None);
        assert_eq!(metadata.metrics.vllm, None);
    }
}

#[test]
fn malformed_metadata_degrades_to_none_with_a_warning() {
    // The deliberate degrade path: every section malformed at once, each
    // one warning and dropping to None, and the call still succeeds.
    let body = serde_json::json!({
        "model": 7,
        "choices": reply_choice(),
        "usage": { "prompt_tokens": "seven" },
        "timings": { "prompt_n": 7 },
        "metrics": ["not", "an", "object"]
    });

    let (metadata, warnings) = with_warn_count(&body);
    assert_eq!(metadata.model, "", "a non-string model records as empty");
    assert_eq!(
        metadata.metrics.usage, None,
        "non-numeric token counts degrade"
    );
    assert_eq!(
        metadata.metrics.llama, None,
        "timings missing required fields degrade"
    );
    assert_eq!(metadata.metrics.vllm, None, "a non-object metrics degrades");
    assert_eq!(warnings, 4, "each malformed section warns exactly once");
}

#[test]
fn metadata_sections_degrade_independently() {
    let body = serde_json::json!({
        "model": "qwen3-30b",
        "choices": reply_choice(),
        "usage": "broken",
        "timings": {
            "prompt_n": 7,
            "prompt_ms": 12.5,
            "prompt_per_second": 560.0,
            "predicted_n": 3,
            "predicted_ms": 30.5,
            "predicted_per_second": 98.5
        }
    });

    let (metadata, warnings) = with_warn_count(&body);
    assert_eq!(warnings, 1, "only the broken section warns");
    assert_eq!(metadata.metrics.usage, None);
    assert!(
        metadata.metrics.llama.is_some(),
        "a malformed sibling section must not take timings down with it"
    );
}

#[test]
fn missing_model_records_empty_and_warns() {
    let body = serde_json::json!({ "choices": reply_choice() });

    let (metadata, warnings) = with_warn_count(&body);
    assert_eq!(metadata.model, "");
    assert_eq!(warnings, 1, "an OpenAI-shaped body without a model warns");
}
