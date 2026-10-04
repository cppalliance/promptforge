//! Completions built without a transport: what a Harness that ran no HTTP
//! hands the Engine - a test performer playing the model from a script, a
//! replay answering from its record. The wire types are `#[non_exhaustive]`
//! so their shape can grow without breaking readers; these constructors
//! are the one way to build them from outside the crate, and they refuse
//! the tool-call shapes the wire decoder refuses, so a Harness-built tool call
//! has the shape a decoded one has.

use std::collections::HashSet;

use promptforge_types::metrics::CallMetrics;
use serde_json::Value;

use super::{Completion, CompletionResult, RawExchange, ToolCall};
use crate::model::CompletionError;
use crate::normalize::{
    check_call_arguments, check_call_id, check_call_name, check_unique_call_id, empty_reply_error,
};

impl Completion {
    /// Builds a completion from a result alone, with no metadata about the
    /// model call.
    ///
    /// `model` is the model name the completion reports. Every optional
    /// field is absent, so `metrics` and `raw` are `None`. A text result is
    /// not validated. [`with_metrics`](Completion::with_metrics),
    /// [`with_raw`](Completion::with_raw),
    /// [`with_finish_reason`](Completion::with_finish_reason),
    /// [`with_reasoning_content`](Completion::with_reasoning_content),
    /// [`with_metadata_diagnostics`](Completion::with_metadata_diagnostics),
    /// and [`with_model`](Completion::with_model) add what the caller knows
    /// beyond the result.
    ///
    /// # Errors
    /// Returns an `EmptyReply`-kind [`CompletionError`] for an empty
    /// tool-call batch. Returns a `MalformedResponse`-kind one when two calls
    /// in the batch share an id. A completion decoded from a model response
    /// must pass the same checks.
    pub fn from_result(
        result: CompletionResult,
        model: impl Into<String>,
    ) -> Result<Completion, CompletionError> {
        if let CompletionResult::ToolCalls(calls) = &result {
            if calls.is_empty() {
                return Err(empty_reply_error());
            }
            let mut seen = HashSet::new();
            for call in calls {
                check_unique_call_id(&mut seen, &call.id)?;
            }
        }
        Ok(Completion {
            result,
            finish_reason: None,
            reasoning_content: None,
            model: model.into(),
            metrics: None,
            metadata_diagnostics: Vec::new(),
            raw: None,
        })
    }

    /// Returns the completion with `metrics` set to what the model call
    /// measured. A caller with nothing to report leaves the completion as
    /// [`from_result`](Completion::from_result) built it.
    #[must_use]
    pub fn with_metrics(mut self, metrics: CallMetrics) -> Completion {
        self.metrics = Some(metrics);
        self
    }

    /// Returns the completion with `raw` set to the request and response
    /// that debug capture records.
    ///
    /// Only a caller that has the request and response attaches them.
    /// Debug capture is off unless the application turns it on.
    #[must_use]
    pub fn with_raw(mut self, raw: RawExchange) -> Completion {
        self.raw = Some(raw);
        self
    }

    /// Returns the completion with `finish_reason` as the stop label the
    /// backend supplied.
    #[must_use]
    pub fn with_finish_reason(mut self, finish_reason: impl Into<String>) -> Completion {
        self.finish_reason = Some(finish_reason.into());
        self
    }

    /// Returns the completion with `reasoning_content` as the reasoning side
    /// channel the backend supplied. It is never promoted into the answer.
    #[must_use]
    pub fn with_reasoning_content(mut self, reasoning_content: impl Into<String>) -> Completion {
        self.reasoning_content = Some(reasoning_content.into());
        self
    }

    /// Returns the completion with `diagnostics` as its metadata
    /// diagnostics, replacing any it held.
    ///
    /// Each diagnostic is one line about a metadata section of the response
    /// that was present but malformed. The Engine reports each line as a
    /// `model_metadata_degraded` event.
    #[must_use]
    pub fn with_metadata_diagnostics(mut self, diagnostics: Vec<String>) -> Completion {
        self.metadata_diagnostics = diagnostics;
        self
    }

    /// Returns the completion with `model` as the name of the model that
    /// served it, replacing the name it held.
    ///
    /// A caller that routed the round to a model labels the completion with
    /// that model's name. The round's reply and thinking events and its
    /// answer record report this name.
    #[must_use]
    pub fn with_model(mut self, model: impl Into<String>) -> Completion {
        self.model = model.into();
        self
    }
}

impl ToolCall {
    /// Builds a tool call from its id, tool name, and arguments.
    ///
    /// `arguments` is the parsed argument payload, already decoded from
    /// JSON text.
    ///
    /// # Errors
    /// Returns a `MalformedResponse`-kind [`CompletionError`] when `id` or
    /// `name` is blank or `arguments` is not a JSON object. A tool call
    /// decoded from a model response must pass the same checks.
    pub fn from_parts(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: Value,
    ) -> Result<ToolCall, CompletionError> {
        let id = id.into();
        let name = name.into();
        check_call_id(&id)?;
        check_call_name(&name)?;
        check_call_arguments(&arguments)?;
        Ok(ToolCall {
            id,
            name,
            arguments,
        })
    }
}
