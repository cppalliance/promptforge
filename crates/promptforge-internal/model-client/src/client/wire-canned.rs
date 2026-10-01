//! Completions built without a transport: what a Harness that ran no HTTP
//! hands the Engine - a test performer playing the model from a script, a
//! replay answering from its record. The wire types are `#[non_exhaustive]`
//! so their shape can grow without breaking readers; these constructors
//! are the one way to build them from outside the crate, and they refuse
//! the tool-call shapes the wire decoder refuses, so a Harness-built tool call
//! has the shape a decoded one has.

use std::collections::HashSet;

use serde_json::Value;

use super::{Completion, CompletionResult, ToolCall};
use crate::Error;
use crate::normalize::{
    check_call_arguments, check_call_id, check_call_name, check_unique_call_id, empty_reply_error,
};

impl Completion {
    /// A completion from a bare result with no transport metadata. `model`
    /// is the name the completion reports; every optional field is absent
    /// and both bodies are JSON `null`. A text result is not validated.
    ///
    /// # Errors
    /// Returns [`Error::EmptyModelReply`] for an empty tool-call batch, and
    /// [`Error::MalformedResponse`] when two calls in the batch share an id,
    /// as the wire decoder does.
    pub fn from_result(
        result: CompletionResult,
        model: impl Into<String>,
    ) -> std::result::Result<Completion, Error> {
        if let CompletionResult::ToolCalls(calls) = &result {
            if calls.is_empty() {
                return Err(empty_reply_error(false, None));
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
            usage: None,
            llama_timings: None,
            vllm_metrics: None,
            client_timing: None,
            metadata_diagnostics: Vec::new(),
            request_body: Value::Null,
            response_body: Value::Null,
        })
    }
}

impl ToolCall {
    /// A tool call from its parts. `arguments` is the parsed argument
    /// payload, as the wire decoder would have left it.
    ///
    /// # Errors
    /// Returns [`Error::MalformedResponse`] when `id` or `name` is blank or
    /// `arguments` is not a JSON object, as the wire decoder does.
    pub fn from_parts(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: Value,
    ) -> std::result::Result<ToolCall, Error> {
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
