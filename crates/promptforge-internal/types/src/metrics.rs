//! Canonical model-call metrics vocabulary.
//!
//! Everything measured about one model call - token accounting, backend
//! timings, and the calling client's own clock - plus the tool-call record
//! a model's request includes. The [`Event`](crate::event::Event) content
//! variants embed these values and the model client parses response bodies
//! into them; they cross every boundary as plain serde data.
//!
//! # Serialized form
//! Every type here serializes with serde; absent optional fields are
//! omitted and deserialize back as `None`.

use serde::{Deserialize, Serialize};

use crate::tools::ToolId;

/// Records one tool call the model requested.
///
/// It holds the call's id, name, and raw arguments, and the bound tool the
/// name resolved to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ToolCallEvent {
    /// The provider-issued tool-call id. Providers recycle ids like
    /// `call_1` across rounds, so consumers scope the id by turn.
    pub id: String,
    /// The tool name the model called: the prompt's alias, such as `fetch`.
    pub name: String,
    /// The call arguments exactly as the model produced them.
    pub arguments: serde_json::Value,
    /// The bound tool `name` resolved to; `None` for a Lua-local tool, a
    /// task built-in, or a name outside the round's scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<ToolId>,
}

impl ToolCallEvent {
    /// Builds the record of call `id` to `name` with `arguments`, bound to
    /// no tool.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: serde_json::Value,
    ) -> ToolCallEvent {
        ToolCallEvent {
            id: id.into(),
            name: name.into(),
            arguments,
            tool: None,
        }
    }

    /// Sets the bound tool the name resolved to.
    #[must_use]
    pub fn with_tool(mut self, tool: Option<ToolId>) -> ToolCallEvent {
        self.tool = tool;
        self
    }
}

/// Groups everything measured about one model call by the source that
/// measured it.
///
/// Each section is present when its source reported it. The serving backend
/// supplies `usage`, `llama`, and `vllm`. The calling client's own clock
/// supplies `client`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
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

impl CallMetrics {
    /// Builds metrics with every section absent.
    #[must_use]
    pub fn new() -> CallMetrics {
        CallMetrics::default()
    }

    /// Sets the backend's token accounting.
    #[must_use]
    pub fn with_usage(mut self, usage: Option<Usage>) -> CallMetrics {
        self.usage = usage;
        self
    }

    /// Sets the llama.cpp server timings.
    #[must_use]
    pub fn with_llama(mut self, llama: Option<LlamaTimings>) -> CallMetrics {
        self.llama = llama;
        self
    }

    /// Sets the vLLM request metrics.
    #[must_use]
    pub fn with_vllm(mut self, vllm: Option<VllmMetrics>) -> CallMetrics {
        self.vllm = vllm;
        self
    }

    /// Sets the timing the calling client measured.
    #[must_use]
    pub fn with_client(mut self, client: Option<ClientTiming>) -> CallMetrics {
        self.client = client;
        self
    }
}

/// Counts one model call's tokens as the backend reported them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
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

impl Usage {
    /// Builds the accounting from the prompt, completion, and total token
    /// counts, with neither detail reported.
    #[must_use]
    pub fn new(prompt_tokens: u32, completion_tokens: u32, total_tokens: u32) -> Usage {
        Usage {
            prompt_tokens,
            completion_tokens,
            total_tokens,
            cached_tokens: None,
            reasoning_tokens: None,
        }
    }

    /// Sets the prompt tokens served from a prefix cache.
    #[must_use]
    pub fn with_cached_tokens(mut self, cached_tokens: Option<u32>) -> Usage {
        self.cached_tokens = cached_tokens;
        self
    }

    /// Sets the tokens spent on reasoning.
    #[must_use]
    pub fn with_reasoning_tokens(mut self, reasoning_tokens: Option<u32>) -> Usage {
        self.reasoning_tokens = reasoning_tokens;
        self
    }
}

/// Holds the timings a llama.cpp server reported for one call.
///
/// The server reports them in the `timings` field of its response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
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

impl LlamaTimings {
    /// Builds the timings from the prompt and prediction measurements, with
    /// both draft counters at zero, as when no draft model ran.
    #[must_use]
    pub fn new(
        prompt_n: u32,
        prompt_ms: f64,
        prompt_per_second: f64,
        predicted_n: u32,
        predicted_ms: f64,
        predicted_per_second: f64,
    ) -> LlamaTimings {
        LlamaTimings {
            prompt_n,
            prompt_ms,
            prompt_per_second,
            predicted_n,
            predicted_ms,
            predicted_per_second,
            draft_n: 0,
            draft_n_accepted: 0,
        }
    }

    /// Sets the draft tokens speculative decoding proposed.
    #[must_use]
    pub fn with_draft_n(mut self, draft_n: u32) -> LlamaTimings {
        self.draft_n = draft_n;
        self
    }

    /// Sets the draft tokens the target model accepted.
    #[must_use]
    pub fn with_draft_n_accepted(mut self, draft_n_accepted: u32) -> LlamaTimings {
        self.draft_n_accepted = draft_n_accepted;
        self
    }
}

/// Holds vLLM's per-request metrics for one call.
///
/// Every field is optional because vLLM reports only what it measured.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
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

impl VllmMetrics {
    /// Builds metrics with every measurement absent.
    #[must_use]
    pub fn new() -> VllmMetrics {
        VllmMetrics::default()
    }

    /// Sets the milliseconds to the first generated token.
    #[must_use]
    pub fn with_time_to_first_token_ms(mut self, ms: Option<f64>) -> VllmMetrics {
        self.time_to_first_token_ms = ms;
        self
    }

    /// Sets the milliseconds spent generating.
    #[must_use]
    pub fn with_generation_time_ms(mut self, ms: Option<f64>) -> VllmMetrics {
        self.generation_time_ms = ms;
        self
    }

    /// Sets the milliseconds the request waited in the scheduler queue.
    #[must_use]
    pub fn with_queue_time_ms(mut self, ms: Option<f64>) -> VllmMetrics {
        self.queue_time_ms = ms;
        self
    }

    /// Sets the mean inter-token latency in milliseconds.
    #[must_use]
    pub fn with_mean_itl_ms(mut self, ms: Option<f64>) -> VllmMetrics {
        self.mean_itl_ms = ms;
        self
    }

    /// Sets the generation rate in tokens per second.
    #[must_use]
    pub fn with_tokens_per_second(mut self, rate: Option<f64>) -> VllmMetrics {
        self.tokens_per_second = rate;
        self
    }
}

/// Times one call end to end on the calling client's own clock.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
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

impl ClientTiming {
    /// Builds the timing of a call that took `e2e_ms` milliseconds, with no
    /// streamed-token timings.
    #[must_use]
    pub fn new(e2e_ms: f64) -> ClientTiming {
        ClientTiming {
            ttft_ms: None,
            mean_itl_ms: None,
            e2e_ms,
        }
    }

    /// Sets the milliseconds to the first streamed token.
    #[must_use]
    pub fn with_ttft_ms(mut self, ttft_ms: Option<f64>) -> ClientTiming {
        self.ttft_ms = ttft_ms;
        self
    }

    /// Sets the mean inter-token latency in milliseconds.
    #[must_use]
    pub fn with_mean_itl_ms(mut self, mean_itl_ms: Option<f64>) -> ClientTiming {
        self.mean_itl_ms = mean_itl_ms;
        self
    }
}

#[cfg(test)]
#[path = "metrics-tests.rs"]
mod tests;
