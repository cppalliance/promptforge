//! SSE reassembly for the always-streaming completion protocol.
//!
//! [`SseScanner`] splits the raw byte stream into `data:` payloads, and
//! [`StreamAccumulator`] folds those payloads back into the buffered
//! chat-completion body shape, then [`finishes`](StreamAccumulator::finish)
//! it into a [`Completion`]. The strict turn rules stay in [`super::parse`]:
//! the accumulator only reassembles, so streamed and buffered turns are
//! judged by one rule set.
//!
//! No HTTP happens here. The transport that reads the bytes off the wire
//! hands them to the read loop, which feeds the scanner, hands payloads to
//! the accumulator, and takes the completion from `finish`.
//!
//! The progress subscription in the model vocabulary deliberately has its
//! own SSE decoder, and neither can substitute for the other: that one
//! decodes blank-line-terminated event blocks into typed progress items and
//! stays lossy (an undecodable block is one `Err` item in a telemetry
//! stream), while this one hands raw `data:` payloads to a transport loop
//! that meters bytes and timing and hard-fails on the first malformed
//! chunk, because a completion's product must be whole.

use std::collections::BTreeMap;

use promptforge::metrics::{CallMetrics, ClientTiming};
use promptforge::model::{Completion, CompletionError, RawExchange};
use serde_json::{Map, Value};

use super::classify::classify_stream_error;
use super::delta::StreamDelta;
use super::parse::{normalize, response_metadata};
use crate::failure::malformed;

/// Splits a raw SSE byte stream into `data:` payloads.
///
/// Blank lines, `:` comments, and non-`data:` fields (`event:`, `id:`,
/// `retry:`) are skipped; the caller sees only payload text.
///
/// Crate-private: a transport reaches it only through
/// [`read_completion_stream`](crate::read_completion_stream).
#[derive(Debug, Default)]
pub(super) struct SseScanner {
    buffer: Vec<u8>,
    /// How much of `buffer` is already known to hold no `\n`.
    scanned: usize,
}

impl SseScanner {
    /// A scanner with an empty buffer.
    #[must_use]
    pub(super) fn new() -> SseScanner {
        SseScanner {
            buffer: Vec::new(),
            scanned: 0,
        }
    }

    /// Buffers freshly received bytes for line extraction.
    pub(super) fn extend(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    /// Returns the next complete `data:` payload, or `None` until one is
    /// fully buffered.
    pub(super) fn next_data(&mut self) -> Option<String> {
        loop {
            let Some(offset) = self.buffer[self.scanned..]
                .iter()
                .position(|byte| *byte == b'\n')
            else {
                self.scanned = self.buffer.len();
                return None;
            };
            let end = self.scanned + offset;
            self.scanned = 0;
            let line: Vec<u8> = self.buffer.drain(..=end).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim_end_matches(['\r', '\n']);
            if line.is_empty() || line.starts_with(':') {
                continue;
            }
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            return Some(data.trim_start().to_owned());
        }
    }
}

/// The outcome of applying one `data:` payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Applied {
    /// The payload advanced the accumulation; `delta` is true when it
    /// included answer text, reasoning, or a tool-call fragment (the
    /// TTFT/ITL clock ticks on those, never on role or summary chunks).
    Chunk {
        /// Whether the chunk included generated content.
        delta: bool,
    },
    /// The payload was the terminal `[DONE]` sentinel.
    Done,
}

/// One tool call assembled from streamed fragments, keyed by the fragment
/// `index`. `id`, `name`, and `arguments` each grow by string concatenation
/// as fragments arrive, per the `OpenAI` streaming contract.
#[derive(Debug, Default)]
struct ToolCallParts {
    id: String,
    name: String,
    arguments: String,
}

/// Accumulates streamed chunks into the buffered chat-completion shape.
///
/// Only the first choice (`index == 0`) is accumulated, mirroring the
/// buffered normalizer, which reads `choices[0]` alone. Metadata sections
/// (`usage`, llama.cpp `timings`, vLLM `metrics`) are kept verbatim from
/// whichever chunk held them last, including the empty-choices summary
/// chunk `stream_options.include_usage` appends, and are handed to the
/// lenient metadata parser unjudged.
///
/// Crate-private: a transport reaches it only through
/// [`read_completion_stream`](crate::read_completion_stream).
#[derive(Debug, Default)]
pub(super) struct StreamAccumulator {
    /// Answer text; `None` until the first `content` fragment arrives.
    content: Option<String>,
    /// Reasoning side-channel text; `None` until the first fragment.
    reasoning: Option<String>,
    tool_calls: BTreeMap<u64, ToolCallParts>,
    finish_reason: Option<String>,
    model: Option<String>,
    /// Raw top-level metadata sections, latest occurrence wins.
    sections: Map<String, Value>,
}

impl StreamAccumulator {
    /// An empty accumulator.
    #[must_use]
    pub(super) fn new() -> StreamAccumulator {
        StreamAccumulator::default()
    }

    /// Applies one `data:` payload, invoking `on_delta` for each text or
    /// reasoning fragment it contains.
    ///
    /// # Errors
    /// Returns a `MalformedResponse`-kind [`CompletionError`] when the
    /// payload is not valid JSON or a recognized field has the wrong
    /// shape, and the [`classify_stream_error`] result when the payload is
    /// a mid-stream error envelope (`Transport` unless its text names a
    /// known cause).
    pub(super) fn apply(
        &mut self,
        data: &str,
        on_delta: &impl Fn(StreamDelta),
    ) -> Result<Applied, CompletionError> {
        if data == "[DONE]" {
            return Ok(Applied::Done);
        }
        let chunk: Value = serde_json::from_str(data)
            .map_err(|error| malformed("stream chunk was not valid JSON").with_source(error))?;
        // A mid-stream `error` envelope is how the gateway (and llama.cpp)
        // report a failure after the 200 has already been sent: the
        // completion died in flight, so it is a transport failure unless
        // the bounded, control-escaped message names a known cause.
        if let Some(envelope) = chunk.get("error").filter(|error| !error.is_null()) {
            let message = envelope
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("stream error envelope omitted its message");
            return Err(classify_stream_error(&escape_controls(message, 2000)));
        }
        if let Some(Value::String(model)) = chunk.get("model")
            && !model.is_empty()
        {
            self.model = Some(model.clone());
        }
        for key in ["usage", "timings", "metrics"] {
            if let Some(section) = chunk.get(key)
                && !section.is_null()
            {
                self.sections.insert(key.to_owned(), section.clone());
            }
        }
        // Absent or empty `choices` is the summary-chunk shape
        // (`stream_options.include_usage`): metadata only, nothing to index.
        let choices = match chunk.get("choices") {
            None | Some(Value::Null) => return Ok(Applied::Chunk { delta: false }),
            Some(Value::Array(choices)) => choices,
            Some(_) => {
                return Err(malformed(
                    "stream chunk `choices` was present but not an array",
                ));
            }
        };
        let mut held_delta = false;
        for choice in choices {
            if self.apply_choice(choice, on_delta)? {
                held_delta = true;
            }
        }
        Ok(Applied::Chunk { delta: held_delta })
    }

    /// Applies one streamed choice, returning whether it held content.
    fn apply_choice(
        &mut self,
        choice: &Value,
        on_delta: &impl Fn(StreamDelta),
    ) -> Result<bool, CompletionError> {
        let Some(index) = choice.get("index").and_then(Value::as_u64) else {
            return Err(malformed("stream choice had no integer index"));
        };
        // Mirror the buffered normalizer: the first choice is the turn.
        if index != 0 {
            return Ok(false);
        }
        match choice.get("finish_reason") {
            None | Some(Value::Null) => {}
            Some(Value::String(reason)) => self.finish_reason = Some(reason.clone()),
            Some(_) => {
                return Err(malformed(
                    "stream choice `finish_reason` was present but not a string",
                ));
            }
        }
        let delta = match choice.get("delta") {
            // A finish-only chunk may omit the delta entirely.
            None | Some(Value::Null) => return Ok(false),
            Some(delta @ Value::Object(_)) => delta,
            Some(_) => {
                return Err(malformed(
                    "stream choice `delta` was present but not an object",
                ));
            }
        };
        let mut held = false;
        if let Some(text) = append_string_fragment(delta, "content", &mut self.content, "content")?
            && !text.is_empty()
        {
            held = true;
            on_delta(StreamDelta::Text(text));
        }
        for key in ["reasoning_content", "reasoning", "thinking"] {
            if let Some(text) = append_string_fragment(delta, key, &mut self.reasoning, key)?
                && !text.is_empty()
            {
                held = true;
                on_delta(StreamDelta::Reasoning(text));
            }
        }
        match delta.get("tool_calls") {
            None | Some(Value::Null) => {}
            Some(Value::Array(fragments)) => {
                for fragment in fragments {
                    self.apply_tool_fragment(fragment)?;
                }
                if !fragments.is_empty() {
                    held = true;
                }
            }
            Some(_) => {
                return Err(malformed(
                    "stream delta `tool_calls` was present but not an array",
                ));
            }
        }
        Ok(held)
    }

    /// Merges one tool-call fragment into its index-keyed buffer.
    fn apply_tool_fragment(&mut self, fragment: &Value) -> Result<(), CompletionError> {
        let Some(index) = fragment.get("index").and_then(Value::as_u64) else {
            return Err(malformed("stream tool-call fragment had no integer index"));
        };
        let parts = self.tool_calls.entry(index).or_default();
        match fragment.get("id") {
            None | Some(Value::Null) => {}
            Some(Value::String(id)) => parts.id.push_str(id),
            Some(_) => {
                return Err(malformed("stream tool-call fragment `id` was not a string"));
            }
        }
        let function = match fragment.get("function") {
            None | Some(Value::Null) => return Ok(()),
            Some(function @ Value::Object(_)) => function,
            Some(_) => {
                return Err(malformed(
                    "stream tool-call fragment `function` was not an object",
                ));
            }
        };
        for (key, slot) in [
            ("name", &mut parts.name),
            ("arguments", &mut parts.arguments),
        ] {
            match function.get(key) {
                None | Some(Value::Null) => {}
                Some(Value::String(piece)) => slot.push_str(piece),
                Some(_) => {
                    return Err(malformed(format!(
                        "stream tool-call fragment `{key}` was not a string"
                    )));
                }
            }
        }
        Ok(())
    }

    /// Finishes the accumulation into the [`Completion`] the turn produced:
    /// the truncation rule, the strict turn normalizer, and the lenient
    /// metadata parser, in that order. `request_body` is the body the
    /// transport sent; the completion is labeled with the model it names,
    /// in place of the name the response gave, and with the reassembled
    /// response it becomes the completion's [`RawExchange`] for the debug
    /// capture. `client_timing`
    /// is what the transport measured on its own clock; it joins the
    /// backend's sections in the completion's [`CallMetrics`], which is
    /// absent when nothing was measured.
    ///
    /// # Errors
    /// Returns a `MalformedResponse`-kind [`CompletionError`] when a
    /// tool-call batch was cut short by a `length` or `content_filter`
    /// finish (partial arguments must not execute), the normalizer's own
    /// errors (`EmptyReply` for a turn with neither non-empty tool calls nor
    /// non-empty text), and the validating constructor's (a
    /// `MalformedResponse` for two calls sharing an id).
    pub(super) fn finish(
        self,
        request_body: Value,
        client_timing: Option<ClientTiming>,
    ) -> Result<Completion, CompletionError> {
        // The truncation rule runs before normalization: a tool-call batch
        // cut short by `length` or `content_filter` may hold partial JSON
        // arguments, and partial arguments must not execute.
        if !self.tool_calls.is_empty()
            && matches!(
                self.finish_reason.as_deref(),
                Some("length" | "content_filter")
            )
        {
            let reason = self.finish_reason.unwrap_or_default();
            return Err(malformed(format!(
                "tool-call batch truncated by finish_reason {reason:?}: \
                 partial arguments must not execute"
            )));
        }
        let response_body = self.into_body();
        let turn = normalize(&response_body)?;
        let metadata = response_metadata(&response_body);
        let metrics = CallMetrics {
            client: client_timing,
            ..metadata.metrics
        };
        let measured = metrics.usage.is_some()
            || metrics.llama.is_some()
            || metrics.vllm.is_some()
            || metrics.client.is_some();
        let mut completion = Completion::from_result(turn.outcome, metadata.model)?
            .with_metadata_diagnostics(metadata.diagnostics);
        if let Some(Value::String(model)) = request_body.get("model") {
            completion = completion.with_model(model.clone());
        }
        if let Some(reason) = turn.finish_reason {
            completion = completion.with_finish_reason(reason);
        }
        if let Some(reasoning) = turn.reasoning_content {
            completion = completion.with_reasoning_content(reasoning);
        }
        if measured {
            completion = completion.with_metrics(metrics);
        }
        Ok(completion.with_raw(RawExchange::new(request_body, response_body)))
    }

    /// Reassembles the accumulation into the buffered chat-completion body
    /// shape, ready for the strict turn normalizer and the lenient metadata
    /// parser.
    fn into_body(self) -> Value {
        let mut message = Map::new();
        message.insert("role".to_owned(), Value::String("assistant".to_owned()));
        message.insert(
            "content".to_owned(),
            match self.content {
                Some(text) => Value::String(text),
                None => Value::Null,
            },
        );
        if let Some(reasoning) = self.reasoning.filter(|text| !text.is_empty()) {
            message.insert("reasoning_content".to_owned(), Value::String(reasoning));
        }
        if !self.tool_calls.is_empty() {
            let calls: Vec<Value> = self
                .tool_calls
                .into_values()
                .map(|parts| {
                    serde_json::json!({
                        "id": parts.id,
                        "type": "function",
                        "function": { "name": parts.name, "arguments": parts.arguments },
                    })
                })
                .collect();
            message.insert("tool_calls".to_owned(), Value::Array(calls));
        }
        let mut choice = Map::new();
        choice.insert("index".to_owned(), Value::from(0));
        choice.insert("message".to_owned(), Value::Object(message));
        if let Some(reason) = self.finish_reason {
            choice.insert("finish_reason".to_owned(), Value::String(reason));
        }
        let mut body = Map::new();
        if let Some(model) = self.model {
            body.insert("model".to_owned(), Value::String(model));
        }
        body.insert(
            "choices".to_owned(),
            Value::Array(vec![Value::Object(choice)]),
        );
        for (key, value) in self.sections {
            body.insert(key, value);
        }
        Value::Object(body)
    }
}

/// Appends a string fragment under `key` from `delta` into `slot`,
/// returning the fragment when one was present.
///
/// Absent and JSON-null are no fragment; a present non-string is a
/// malformed shape named after `label`.
fn append_string_fragment(
    delta: &Value,
    key: &str,
    slot: &mut Option<String>,
    label: &str,
) -> Result<Option<String>, CompletionError> {
    match delta.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => {
            slot.get_or_insert_with(String::new).push_str(text);
            Ok(Some(text.clone()))
        }
        Some(_) => Err(malformed(format!(
            "stream delta `{label}` was present but not a string"
        ))),
    }
}

/// Escapes control characters in a diagnostic body and bounds it to `max` chars.
///
/// Control characters (including newlines and carriage returns) are rendered in
/// their `\u{..}`/`\n` escaped form so a backend body cannot forge log lines or
/// smuggle terminal control sequences into a diagnostic. An empty body is
/// reported as a fixed marker.
///
/// A transport runs a non-success status's error body through here before
/// handing it to [`classify_http_failure`](crate::classify_http_failure), so
/// every transport bounds and escapes a backend body by the same rule.
#[must_use]
pub fn escape_controls(body: &str, max: usize) -> String {
    if body.is_empty() {
        return "(empty body)".to_owned();
    }
    let mut escaped = String::with_capacity(body.len());
    for ch in body.chars().take(max) {
        if ch.is_control() {
            for part in ch.escape_default() {
                escaped.push(part);
            }
        } else {
            escaped.push(ch);
        }
    }
    escaped
}

#[cfg(test)]
#[path = "stream-tests.rs"]
mod tests;

#[cfg(test)]
#[path = "stream-metrics-tests.rs"]
mod metrics_tests;
