//! Canonical model-call metrics vocabulary.
//!
//! Everything measured about one model call - token accounting, backend
//! timings, and the calling client's own clock - plus the tool-call record
//! a model's request includes. The [`Event`](crate::event::Event) content
//! variants embed these values, the model client parses response bodies
//! into them, and the Workshop protocol renders them; they cross every
//! boundary as plain serde data.
//!
//! # Serialized form
//! Every type here serializes with serde; absent optional fields are
//! omitted and deserialize back as `None`.

use serde::{Deserialize, Serialize};

/// One tool call requested by the model: its id, name, and raw arguments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallEvent {
    /// The provider-issued tool-call id. Providers recycle ids like
    /// `call_1` across rounds, so consumers scope the id by turn.
    pub id: String,
    /// The tool name the model called.
    pub name: String,
    /// The call arguments exactly as the model produced them.
    pub arguments: serde_json::Value,
}

/// Everything measured about one model call, from every source that
/// reported.
///
/// Each section is present when its source reported it: `usage` and the
/// backend sections come from the serving backend, `client` from the calling
/// client's own clock.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CallMetrics {
    /// Token accounting, when the backend reported usage.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// llama.cpp server timings, when that backend served the call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llama: Option<LlamaTimings>,
    /// vLLM request metrics, when that backend served the call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vllm: Option<VllmMetrics>,
    /// Timing measured by the calling client itself.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client: Option<ClientTiming>,
}

/// Token accounting for one model call, as the backend reported it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// Tokens in the prompt.
    pub prompt_tokens: u32,
    /// Tokens generated in the completion.
    pub completion_tokens: u32,
    /// Prompt plus completion tokens.
    pub total_tokens: u32,
    /// Prompt tokens served from a prefix cache, when the backend reports
    /// the detail.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_tokens: Option<u32>,
    /// Tokens spent on reasoning, when the backend reports the detail.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u32>,
}

/// llama.cpp `timings` for one call, as the server reported them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlamaTimings {
    /// Prompt tokens processed.
    pub prompt_n: u32,
    /// Wall-clock milliseconds spent processing the prompt.
    pub prompt_ms: f64,
    /// Prompt processing rate in tokens per second.
    pub prompt_per_second: f64,
    /// Tokens predicted.
    pub predicted_n: u32,
    /// Wall-clock milliseconds spent predicting.
    pub predicted_ms: f64,
    /// Prediction rate in tokens per second.
    pub predicted_per_second: f64,
    /// Draft tokens proposed by speculative decoding.
    pub draft_n: u32,
    /// Draft tokens the target model accepted.
    pub draft_n_accepted: u32,
}

/// vLLM per-request metrics for one call.
///
/// Every field is optional because vLLM omits what it did not measure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VllmMetrics {
    /// Milliseconds from request start to the first generated token.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_to_first_token_ms: Option<f64>,
    /// Milliseconds spent generating.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_time_ms: Option<f64>,
    /// Milliseconds the request waited in the scheduler queue.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queue_time_ms: Option<f64>,
    /// Mean inter-token latency in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mean_itl_ms: Option<f64>,
    /// Generation rate in tokens per second.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens_per_second: Option<f64>,
}

/// Timing one call end to end, measured by the calling client's own clock.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClientTiming {
    /// Milliseconds from sending the request to the first streamed token,
    /// when the stream produced one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttft_ms: Option<f64>,
    /// Mean inter-token latency in milliseconds, when at least two tokens
    /// streamed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mean_itl_ms: Option<f64>,
    /// Milliseconds from sending the request to the completed response.
    pub e2e_ms: f64,
}

#[cfg(test)]
mod tests {
    use serde::de::DeserializeOwned;
    use serde_json::json;

    use super::*;

    fn full_metrics() -> CallMetrics {
        CallMetrics {
            usage: Some(Usage {
                prompt_tokens: 7,
                completion_tokens: 3,
                total_tokens: 10,
                cached_tokens: Some(2),
                reasoning_tokens: Some(1),
            }),
            llama: Some(LlamaTimings {
                prompt_n: 7,
                prompt_ms: 12.5,
                prompt_per_second: 560.0,
                predicted_n: 3,
                predicted_ms: 30.5,
                predicted_per_second: 98.5,
                draft_n: 4,
                draft_n_accepted: 2,
            }),
            vllm: Some(VllmMetrics {
                time_to_first_token_ms: Some(8.5),
                generation_time_ms: Some(22.5),
                queue_time_ms: Some(1.5),
                mean_itl_ms: Some(7.5),
                tokens_per_second: Some(133.5),
            }),
            client: Some(ClientTiming {
                ttft_ms: Some(9.5),
                mean_itl_ms: Some(8.25),
                e2e_ms: 41.5,
            }),
        }
    }

    fn round_trips<T>(value: &T)
    where
        T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug,
    {
        let line = serde_json::to_string(value).expect("every vocabulary type must serialize");
        let back: T = serde_json::from_str(&line).expect("its own output must deserialize");
        assert_eq!(&back, value);
    }

    #[test]
    fn every_vocabulary_type_round_trips_through_serde() {
        let metrics = full_metrics();
        round_trips(metrics.usage.as_ref().expect("usage is populated"));
        round_trips(metrics.llama.as_ref().expect("llama is populated"));
        round_trips(metrics.vllm.as_ref().expect("vllm is populated"));
        round_trips(metrics.client.as_ref().expect("client is populated"));
        round_trips(&metrics);
        round_trips(&ToolCallEvent {
            id: "call_1".to_owned(),
            name: "read_file".to_owned(),
            arguments: json!({ "path": "notes.txt", "lines": 3 }),
        });
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
        assert_eq!(
            serde_json::to_string(&full_metrics()).expect("metrics must serialize"),
            line
        );
        let empty = CallMetrics {
            usage: None,
            llama: None,
            vllm: None,
            client: None,
        };
        assert_eq!(
            serde_json::to_string(&empty).expect("metrics must serialize"),
            "{}",
            "absent sections are omitted from the line"
        );
    }
}
