//! Canonical metrics and runtime-event vocabulary, with the read-side
//! [`EventLog`] history interface.
//!
//! The write side and the read side are deliberately different types. The
//! [`Observer`](crate::observe::Observer) content methods report each event
//! as it happens and are never read back; the [`EventLog`] is the explicit,
//! indexed history a host chooses to hand an executor as a run input. Keeping
//! the two apart is what keeps the observation vocabulary report-only.
//!
//! A [`RuntimeEvent`] records what happened - a completed reply, a tool-call
//! batch, a tool result, thinking, user input - never assembled framing: no
//! system prompts, no injected files, no tool schemas. Event content is
//! untrusted model-, tool-, or user-authored data; see the sensitivity notes
//! in [`observe`](crate::observe).
//!
//! # Serialized form
//! Every type here serializes with serde. One [`RuntimeEvent`] serialized
//! compactly is one JSONL line; absent optional fields are omitted from the
//! line and deserialize back as `None`. A persisting log stores these lines
//! behind a versioned header line owned by the persistence layer.
//! [`RuntimeEventKind`] labels follow the Agent Client Protocol
//! `sessionUpdate` names where an equivalent exists; the enum documents the
//! label table and the kinds reserved for future producers.
//!
//! # Examples
//! ```
//! use promptforge_core_support::events::{RuntimeEvent, RuntimeEventKind};
//!
//! let event = RuntimeEvent {
//!     kind: RuntimeEventKind::UserInput,
//!     section: "chat".to_owned(),
//!     chain_id: 0,
//!     depth: 0,
//!     turn: 1,
//!     content: "hello".to_owned(),
//!     model: None,
//!     tool_call_id: None,
//!     finish_reason: None,
//!     metrics: None,
//! };
//! let line = serde_json::to_string(&event)?;
//! assert_eq!(
//!     line,
//!     r#"{"kind":"user_message","section":"chat","chain_id":0,"depth":0,"turn":1,"content":"hello"}"#
//! );
//! assert_eq!(serde_json::from_str::<RuntimeEvent>(&line)?, event);
//! # Ok::<(), serde_json::Error>(())
//! ```

use serde::{Deserialize, Serialize};

/// Read-side run history: append-only, indexed from zero.
///
/// Distinct from [`Observer`](crate::observe::Observer) by design: the
/// Observer is report-only and never read back, while an `EventLog` is an
/// explicit run input a host supplies when it wants an executor to see its
/// own history. Implementations are append-only, so an index once valid
/// stays valid and its entry never changes; [`get`](Self::get) serves one
/// entry per call, so a reader converts entries one at a time instead of
/// copying the log in bulk.
///
/// # Examples
/// ```
/// use promptforge_core_support::events::{EventLog, RuntimeEvent, RuntimeEventKind};
///
/// struct VecLog(Vec<RuntimeEvent>);
///
/// impl EventLog for VecLog {
///     fn len(&self) -> u64 {
///         self.0.len() as u64
///     }
///     fn get(&self, index: u64) -> Option<RuntimeEvent> {
///         usize::try_from(index).ok().and_then(|i| self.0.get(i).cloned())
///     }
/// }
///
/// let log = VecLog(vec![RuntimeEvent {
///     kind: RuntimeEventKind::UserInput,
///     section: "chat".to_owned(),
///     chain_id: 0,
///     depth: 0,
///     turn: 1,
///     content: "hello".to_owned(),
///     model: None,
///     tool_call_id: None,
///     finish_reason: None,
///     metrics: None,
/// }]);
/// assert_eq!(log.len(), 1);
/// assert_eq!(log.get(0).map(|event| event.content), Some("hello".to_owned()));
/// assert_eq!(log.get(1), None);
/// ```
#[expect(
    clippy::len_without_is_empty,
    reason = "the trait is pinned to exactly len + get; emptiness is len() == 0"
)]
pub trait EventLog: Send + Sync {
    /// Returns the number of events recorded so far.
    fn len(&self) -> u64;

    /// Returns the event at `index`, or `None` at or past
    /// [`len`](Self::len).
    fn get(&self, index: u64) -> Option<RuntimeEvent>;
}

/// One durable record of something that happened during a run.
///
/// `content` and every other free-text field is untrusted data authored by a
/// model, a tool, or a user. The event records what happened, never the
/// framing assembled around it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeEvent {
    /// What kind of thing happened.
    pub kind: RuntimeEventKind,
    /// The reporting scope: a document prompt's section heading, or an
    /// agent's name.
    pub section: String,
    /// The fanout chain the event was reported under (0 outside fanout).
    pub chain_id: u32,
    /// The nesting depth the event was reported under (0 at the top level).
    pub depth: u32,
    /// The model-turn counter the event was reported under.
    pub turn: u32,
    /// The kind-specific untrusted payload: reply text, thinking text, tool
    /// result content, user input, or a rendering of a tool-call batch.
    pub content: String,
    /// The model that produced the event, for model-attributed kinds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The provider-issued tool-call id the event answers to, for tool
    /// kinds. Providers recycle ids like `call_1` across rounds, so
    /// consumers scope the id by turn.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// The provider's finish reason, when it sent one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    /// Everything measured about the model call that produced the event.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metrics: Option<CallMetrics>,
}

/// The kind of one [`RuntimeEvent`].
///
/// Serialized labels follow the Agent Client Protocol `sessionUpdate` names
/// where an equivalent exists, so persisted logs stay ACP-conversant:
///
/// | Variant | Label |
/// |---|---|
/// | [`AssistantReply`](Self::AssistantReply) | `agent_message` |
/// | [`AssistantToolCalls`](Self::AssistantToolCalls) | `tool_call` |
/// | [`ToolResult`](Self::ToolResult) | `tool_call_update` |
/// | [`Thinking`](Self::Thinking) | `agent_thought` |
/// | [`UserInput`](Self::UserInput) | `user_message` |
///
/// Section-lifecycle vocabulary is deliberately absent:
/// [`Observation`](crate::observe::Observation) owns it.
///
/// Two kinds are reserved for future producers and stay undeclared until one
/// exists: `plan` (a snapshot-replace plan update carrying a required
/// `planId`) and the five-status tool state (`pending` / `in_progress` /
/// `completed` / `failed` / `cancelled`) for tool-call progress reporting.
/// The enum is `#[non_exhaustive]` so those additions stay non-breaking; a
/// consumer matching on kinds tolerates unknown variants through a wildcard
/// arm, as with [`Observation`](crate::observe::Observation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum RuntimeEventKind {
    /// A completed assistant reply.
    #[serde(rename = "agent_message")]
    AssistantReply,
    /// A batch of tool calls the model requested.
    #[serde(rename = "tool_call")]
    AssistantToolCalls,
    /// The result of one dispatched tool call.
    #[serde(rename = "tool_call_update")]
    ToolResult,
    /// A completed block of model thinking.
    #[serde(rename = "agent_thought")]
    Thinking,
    /// Text the user supplied.
    #[serde(rename = "user_message")]
    UserInput,
}

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

    fn full_event() -> RuntimeEvent {
        RuntimeEvent {
            kind: RuntimeEventKind::AssistantReply,
            section: "chat".to_owned(),
            chain_id: 1,
            depth: 0,
            turn: 2,
            content: "hello".to_owned(),
            model: Some("llama-3".to_owned()),
            tool_call_id: None,
            finish_reason: Some("stop".to_owned()),
            metrics: Some(full_metrics()),
        }
    }

    fn minimal_event() -> RuntimeEvent {
        RuntimeEvent {
            kind: RuntimeEventKind::UserInput,
            section: "chat".to_owned(),
            chain_id: 0,
            depth: 0,
            turn: 0,
            content: "hi".to_owned(),
            model: None,
            tool_call_id: None,
            finish_reason: None,
            metrics: None,
        }
    }

    fn tool_result_event() -> RuntimeEvent {
        RuntimeEvent {
            kind: RuntimeEventKind::ToolResult,
            section: "chat".to_owned(),
            chain_id: 0,
            depth: 1,
            turn: 3,
            content: "file contents".to_owned(),
            model: None,
            tool_call_id: Some("call_1".to_owned()),
            finish_reason: None,
            metrics: None,
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
        round_trips(&RuntimeEventKind::AssistantReply);
        round_trips(&full_event());
        round_trips(&minimal_event());
        round_trips(&tool_result_event());
    }

    #[test]
    fn runtime_event_jsonl_line_shape_is_stable() {
        // These pinned lines are the persisted-log schema: a change that
        // renames a field, reorders serialization, or makes an absent field
        // required breaks every log written before it, so it must fail here.
        let full_line = concat!(
            r#"{"kind":"agent_message","section":"chat","chain_id":1,"depth":0,"#,
            r#""turn":2,"content":"hello","model":"llama-3","finish_reason":"stop","#,
            r#""metrics":{"usage":{"prompt_tokens":7,"completion_tokens":3,"#,
            r#""total_tokens":10,"cached_tokens":2,"reasoning_tokens":1},"#,
            r#""llama":{"prompt_n":7,"prompt_ms":12.5,"prompt_per_second":560.0,"#,
            r#""predicted_n":3,"predicted_ms":30.5,"predicted_per_second":98.5,"#,
            r#""draft_n":4,"draft_n_accepted":2},"#,
            r#""vllm":{"time_to_first_token_ms":8.5,"generation_time_ms":22.5,"#,
            r#""queue_time_ms":1.5,"mean_itl_ms":7.5,"tokens_per_second":133.5},"#,
            r#""client":{"ttft_ms":9.5,"mean_itl_ms":8.25,"e2e_ms":41.5}}}"#,
        );
        let serialized = serde_json::to_string(&full_event()).expect("event must serialize");
        assert_eq!(serialized, full_line);
        assert!(
            !serialized.contains('\n'),
            "one event must serialize to one JSONL line"
        );

        // Absent optional fields are omitted from the line, and a line
        // without them still deserializes.
        let minimal_line = r#"{"kind":"user_message","section":"chat","chain_id":0,"depth":0,"turn":0,"content":"hi"}"#;
        assert_eq!(
            serde_json::to_string(&minimal_event()).expect("event must serialize"),
            minimal_line
        );
        assert_eq!(
            serde_json::from_str::<RuntimeEvent>(minimal_line).expect("pinned line must parse"),
            minimal_event()
        );
        assert_eq!(
            serde_json::from_str::<RuntimeEvent>(full_line).expect("pinned line must parse"),
            full_event()
        );
    }

    #[test]
    fn kind_labels_follow_acp_session_update_names() {
        let labels = [
            (RuntimeEventKind::AssistantReply, "agent_message"),
            (RuntimeEventKind::AssistantToolCalls, "tool_call"),
            (RuntimeEventKind::ToolResult, "tool_call_update"),
            (RuntimeEventKind::Thinking, "agent_thought"),
            (RuntimeEventKind::UserInput, "user_message"),
        ];
        for (kind, label) in labels {
            let quoted = format!("\"{label}\"");
            assert_eq!(
                serde_json::to_string(&kind).expect("kind must serialize"),
                quoted,
                "{kind:?} must keep its pinned label"
            );
            assert_eq!(
                serde_json::from_str::<RuntimeEventKind>(&quoted).expect("pinned label must parse"),
                kind
            );
        }
    }

    #[test]
    fn event_log_serves_indexed_single_entry_access() {
        struct VecLog(Vec<RuntimeEvent>);

        impl EventLog for VecLog {
            fn len(&self) -> u64 {
                u64::try_from(self.0.len()).expect("test log length fits in u64")
            }
            fn get(&self, index: u64) -> Option<RuntimeEvent> {
                usize::try_from(index)
                    .ok()
                    .and_then(|i| self.0.get(i).cloned())
            }
        }

        fn assert_send_sync<T: Send + Sync + ?Sized>() {}
        assert_send_sync::<dyn EventLog>();

        let log = VecLog(vec![minimal_event(), full_event()]);
        let log: &dyn EventLog = &log;
        assert_eq!(log.len(), 2);
        assert_eq!(log.get(0), Some(minimal_event()));
        assert_eq!(log.get(1), Some(full_event()));
        assert_eq!(log.get(2), None, "reads at or past len must return None");
    }
}
