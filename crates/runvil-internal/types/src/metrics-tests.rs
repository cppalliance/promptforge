//! Tests for the metrics vocabulary: constructors, serde round trips, and the
//! persisted line shape.

use serde_json::json;

use super::*;
use crate::test_support::{assert_wire, round_trip};

fn full_metrics() -> CallMetrics {
    CallMetrics::new()
        .with_usage(Some(
            Usage::new(7, 3, 10)
                .with_cached_tokens(Some(2))
                .with_reasoning_tokens(Some(1)),
        ))
        .with_llama(Some(
            LlamaTimings::new(7, 12.5, 560.0, 3, 30.5, 98.5)
                .with_draft_n(4)
                .with_draft_n_accepted(2),
        ))
        .with_vllm(Some(
            VllmMetrics::new()
                .with_time_to_first_token_ms(Some(8.5))
                .with_generation_time_ms(Some(22.5))
                .with_queue_time_ms(Some(1.5))
                .with_mean_itl_ms(Some(7.5))
                .with_tokens_per_second(Some(133.5)),
        ))
        .with_client(Some(
            ClientTiming::new(41.5)
                .with_ttft_ms(Some(9.5))
                .with_mean_itl_ms(Some(8.25)),
        ))
}

#[test]
fn a_constructor_leaves_every_optional_field_at_its_default() {
    assert_eq!(CallMetrics::new(), CallMetrics::default());
    assert_eq!(CallMetrics::new().usage, None);
    let usage = Usage::new(7, 3, 10);
    assert_eq!((usage.cached_tokens, usage.reasoning_tokens), (None, None));
    let llama = LlamaTimings::new(7, 12.5, 560.0, 3, 30.5, 98.5);
    assert_eq!(
        (llama.draft_n, llama.draft_n_accepted),
        (0, 0),
        "an absent draft counter means no drafted tokens"
    );
    assert_eq!(VllmMetrics::new(), VllmMetrics::default());
    let client = ClientTiming::new(41.5);
    assert_eq!((client.ttft_ms, client.mean_itl_ms), (None, None));
    let call = ToolCallEvent::new("call_1", "fetch", json!({}));
    assert_eq!(call.tool, None);
}

#[test]
fn every_vocabulary_type_round_trips_through_serde() {
    let metrics = full_metrics();
    let usage = metrics.usage.as_ref().expect("usage is populated");
    assert_eq!(&round_trip(usage), usage);
    let llama = metrics.llama.as_ref().expect("llama is populated");
    assert_eq!(&round_trip(llama), llama);
    let vllm = metrics.vllm.as_ref().expect("vllm is populated");
    assert_eq!(&round_trip(vllm), vllm);
    let client = metrics.client.as_ref().expect("client is populated");
    assert_eq!(&round_trip(client), client);
    assert_eq!(round_trip(&metrics), metrics);
    for call in [
        ToolCallEvent::new(
            "call_1",
            "read_file",
            json!({ "path": "notes.txt", "lines": 3 }),
        ),
        ToolCallEvent::new("call_1", "fetch", json!({}))
            .with_tool(Some(ToolId::parse("web/fetch").expect("a valid tool id"))),
    ] {
        assert_eq!(round_trip(&call), call);
    }
}

#[test]
fn a_tool_call_without_a_bound_tool_keeps_its_logged_shape() {
    // A call logged before the field existed reads back with no tool,
    // and a call with none writes the same line it always did.
    let line = r#"{"id":"call_1","name":"fetch","arguments":{}}"#;
    let call: ToolCallEvent = serde_json::from_str(line).expect("an older call deserializes");
    assert_eq!(call.tool, None);
    assert_wire(&call, line);
}

#[test]
fn metrics_line_shape_is_stable() {
    // The pinned line is the persisted-log schema for a call's metrics:
    // a change that renames a field, reorders serialization, or makes
    // an absent field required breaks every log written before it.
    let line = concat!(
        r#"{"usage":{"prompt_tokens":7,"completion_tokens":3,"#,
        r#""total_tokens":10,"cached_tokens":2,"reasoning_tokens":1},"#,
        r#""llama":{"prompt_n":7,"prompt_ms":12.5,"prompt_per_second":560.0,"#,
        r#""predicted_n":3,"predicted_ms":30.5,"predicted_per_second":98.5,"#,
        r#""draft_n":4,"draft_n_accepted":2},"#,
        r#""vllm":{"time_to_first_token_ms":8.5,"generation_time_ms":22.5,"#,
        r#""queue_time_ms":1.5,"mean_itl_ms":7.5,"tokens_per_second":133.5},"#,
        r#""client":{"ttft_ms":9.5,"mean_itl_ms":8.25,"e2e_ms":41.5}}"#,
    );
    assert_wire(&full_metrics(), line);
    // Absent sections are omitted from the line.
    assert_wire(&CallMetrics::new(), "{}");
}

/// A non-finite `f64` has no JSON number form, so `serde_json` writes it
/// as `null`; it would read back as a null or absent field, not the value
/// that went in. Replay comparison needs bit-for-bit equality, so the
/// producers reject non-finite values at the source instead of letting
/// the serializer erase them: a request temperature through
/// `Temperature::new`, and a timer's seconds through the task timeout
/// parse with the scheduler's defensive repeat.
#[test]
fn a_non_finite_f64_serializes_to_null() {
    for non_finite in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(
            serde_json::to_string(&non_finite).expect("a non-finite float must serialize"),
            "null",
            "a non-finite float has no JSON number form"
        );
    }
    let metrics = VllmMetrics::new().with_mean_itl_ms(Some(f64::NAN));
    assert_eq!(
        serde_json::to_string(&metrics).expect("metrics must serialize"),
        r#"{"mean_itl_ms":null}"#,
        "a non-finite timing reaches the log as null"
    );
}

/// `Value::Object` is a `BTreeMap`, so serialization emits keys in sorted
/// order no matter the order they were inserted. No crate enables
/// `serde_json`'s `preserve_order`; were one to, insertion order would
/// leak into the log line and two equal payloads could print differently.
#[test]
fn object_keys_serialize_in_canonical_order() {
    // Object keys must be sorted, not held in insertion order.
    assert_wire(&json!({ "z": 1, "a": 2, "m": 3 }), r#"{"a":2,"m":3,"z":1}"#);
}
