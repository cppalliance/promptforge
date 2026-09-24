//! Completions built without a transport: what a host that ran no HTTP
//! hands the engine - a test performer playing the model from a script, a
//! replay answering from its record. The wire types are `#[non_exhaustive]`
//! so their shape can grow without breaking readers; these constructors
//! are the one way to build them from outside the crate.

use serde_json::Value;

use super::{Completion, CompletionResult, ToolCall};

impl Completion {
    /// A completion from a bare result with no transport metadata. `model`
    /// is the name the completion reports; every optional field is absent
    /// and both bodies are JSON `null`.
    #[must_use]
    pub fn from_result(result: CompletionResult, model: impl Into<String>) -> Completion {
        Completion {
            result,
            finish_reason: None,
            reasoning_content: None,
            model: model.into(),
            usage: None,
            llama_timings: None,
            vllm_metrics: None,
            client_timing: None,
            metadata_diagnostics: Vec::new(),
            request_body: Value::Null,
            response_body: Value::Null,
        }
    }
}

impl ToolCall {
    /// A tool call from its parts. `arguments` is the parsed argument
    /// payload, as the wire decoder would have left it.
    #[must_use]
    pub fn from_parts(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: Value,
    ) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: name.into(),
            arguments,
        }
    }
}
