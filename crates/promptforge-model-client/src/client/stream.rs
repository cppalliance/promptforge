//! SSE consumption for the always-streaming completion transport.
//!
//! [`SseScanner`] splits the raw byte stream into `data:` payloads, and
//! [`StreamAccumulator`] folds those payloads back into the buffered
//! chat-completion body shape. The strict turn rules stay in
//! [`crate::normalize`]: the accumulator only reassembles, so streamed and
//! buffered turns are judged by exactly one rule set.
//!
//! The progress subscription in [`crate::model`] carries its own SSE decoder
//! deliberately, and neither can substitute for the other: that one decodes
//! blank-line-terminated event blocks into typed progress items and stays
//! lossy (an undecodable block is one `Err` item in a telemetry stream),
//! while this one hands raw `data:` payloads to a transport loop that meters
//! bytes and timing and hard-fails on the first malformed chunk, because a
//! completion's product must be whole.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::transport::escape_controls;
use super::wire::StreamDelta;
use crate::{Error, Result};

/// Splits a raw SSE byte stream into `data:` payloads.
///
/// Blank lines, `:` comments, and non-`data:` fields (`event:`, `id:`,
/// `retry:`) are skipped; the caller sees only payload text.
pub(crate) struct SseScanner {
    buffer: Vec<u8>,
}

impl SseScanner {
    /// A scanner with an empty buffer.
    pub(crate) fn new() -> SseScanner {
        SseScanner { buffer: Vec::new() }
    }

    /// Buffers freshly received bytes for line extraction.
    pub(crate) fn extend(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    /// Returns the next complete `data:` payload, or `None` until one is
    /// fully buffered.
    pub(crate) fn next_data(&mut self) -> Option<String> {
        loop {
            let end = self.buffer.iter().position(|byte| *byte == b'\n')?;
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
#[derive(Debug)]
pub(crate) enum Applied {
    /// The payload advanced the accumulation; `delta` is true when it
    /// carried answer text, reasoning, or a tool-call fragment (the
    /// TTFT/ITL clock ticks on those, never on role or summary chunks).
    Chunk {
        /// Whether the chunk carried generated content.
        delta: bool,
    },
    /// The payload was the terminal `[DONE]` sentinel.
    Done,
}

/// One tool call assembled from streamed fragments, keyed by the fragment
/// `index`. `id`, `name`, and `arguments` each grow by string concatenation
/// as fragments arrive, per the `OpenAI` streaming contract.
#[derive(Default)]
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
/// whichever chunk carried them last, including the empty-choices summary
/// chunk `stream_options.include_usage` appends, and are handed to the
/// lenient metadata parser unjudged.
pub(crate) struct StreamAccumulator {
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
    pub(crate) fn new() -> StreamAccumulator {
        StreamAccumulator {
            content: None,
            reasoning: None,
            tool_calls: BTreeMap::new(),
            finish_reason: None,
            model: None,
            sections: Map::new(),
        }
    }

    /// Whether any tool-call fragment has arrived.
    pub(crate) fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }

    /// The latest `finish_reason` a chunk carried, if any.
    pub(crate) fn finish_reason(&self) -> Option<&str> {
        self.finish_reason.as_deref()
    }

    /// Applies one `data:` payload, invoking `on_delta` for each text or
    /// reasoning fragment it carries.
    ///
    /// # Errors
    /// Returns [`Error::MalformedResponse`] (or the source-preserving
    /// variant) when the payload is not valid JSON or a recognized field has
    /// the wrong shape, and a transport-classified error when the payload is
    /// a mid-stream error envelope.
    pub(crate) fn apply(&mut self, data: &str, on_delta: &impl Fn(StreamDelta)) -> Result<Applied> {
        if data == "[DONE]" {
            return Ok(Applied::Done);
        }
        let chunk: Value =
            serde_json::from_str(data).map_err(|error| Error::MalformedResponseSource {
                message: "stream chunk was not valid JSON".to_owned(),
                source: Box::new(error),
            })?;
        // A mid-stream `error` envelope is how the gateway (and llama.cpp)
        // report a failure after the 200 has already been sent: the
        // completion died in flight, so it classifies as a transport
        // failure, with the bounded, control-escaped message as the cause.
        if let Some(envelope) = chunk.get("error") {
            let message = envelope
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("stream error envelope carried no message");
            return Err(Error::Http(Box::new(std::io::Error::other(format!(
                "completion stream reported an error: {}",
                escape_controls(message, 2000)
            )))));
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
                return Err(Error::MalformedResponse(
                    "stream chunk `choices` was present but not an array".into(),
                ));
            }
        };
        let mut carried_delta = false;
        for choice in choices {
            if self.apply_choice(choice, on_delta)? {
                carried_delta = true;
            }
        }
        Ok(Applied::Chunk {
            delta: carried_delta,
        })
    }

    /// Applies one streamed choice, returning whether it carried content.
    fn apply_choice(&mut self, choice: &Value, on_delta: &impl Fn(StreamDelta)) -> Result<bool> {
        let Some(index) = choice.get("index").and_then(Value::as_u64) else {
            return Err(Error::MalformedResponse(
                "stream choice had no integer index".into(),
            ));
        };
        // Mirror the buffered normalizer: the first choice is the turn.
        if index != 0 {
            return Ok(false);
        }
        match choice.get("finish_reason") {
            None | Some(Value::Null) => {}
            Some(Value::String(reason)) => self.finish_reason = Some(reason.clone()),
            Some(_) => {
                return Err(Error::MalformedResponse(
                    "stream choice `finish_reason` was present but not a string".into(),
                ));
            }
        }
        let delta = match choice.get("delta") {
            // A finish-only chunk may omit the delta entirely.
            None | Some(Value::Null) => return Ok(false),
            Some(delta @ Value::Object(_)) => delta,
            Some(_) => {
                return Err(Error::MalformedResponse(
                    "stream choice `delta` was present but not an object".into(),
                ));
            }
        };
        let mut carried = false;
        if let Some(text) = append_string_fragment(delta, "content", &mut self.content, "content")?
            && !text.is_empty()
        {
            carried = true;
            on_delta(StreamDelta::Text(text));
        }
        for key in ["reasoning_content", "reasoning", "thinking"] {
            if let Some(text) = append_string_fragment(delta, key, &mut self.reasoning, key)?
                && !text.is_empty()
            {
                carried = true;
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
                    carried = true;
                }
            }
            Some(_) => {
                return Err(Error::MalformedResponse(
                    "stream delta `tool_calls` was present but not an array".into(),
                ));
            }
        }
        Ok(carried)
    }

    /// Merges one tool-call fragment into its index-keyed buffer.
    fn apply_tool_fragment(&mut self, fragment: &Value) -> Result<()> {
        let Some(index) = fragment.get("index").and_then(Value::as_u64) else {
            return Err(Error::MalformedResponse(
                "stream tool-call fragment had no integer index".into(),
            ));
        };
        let parts = self.tool_calls.entry(index).or_default();
        match fragment.get("id") {
            None | Some(Value::Null) => {}
            Some(Value::String(id)) => parts.id.push_str(id),
            Some(_) => {
                return Err(Error::MalformedResponse(
                    "stream tool-call fragment `id` was not a string".into(),
                ));
            }
        }
        let function = match fragment.get("function") {
            None | Some(Value::Null) => return Ok(()),
            Some(function @ Value::Object(_)) => function,
            Some(_) => {
                return Err(Error::MalformedResponse(
                    "stream tool-call fragment `function` was not an object".into(),
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
                    return Err(Error::MalformedResponse(format!(
                        "stream tool-call fragment `{key}` was not a string"
                    )));
                }
            }
        }
        Ok(())
    }

    /// Reassembles the accumulation into the buffered chat-completion body
    /// shape, ready for the strict turn normalizer and the lenient metadata
    /// parser.
    pub(crate) fn into_body(self) -> Value {
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
) -> Result<Option<String>> {
    match delta.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => {
            slot.get_or_insert_with(String::new).push_str(text);
            Ok(Some(text.clone()))
        }
        Some(_) => Err(Error::MalformedResponse(format!(
            "stream delta `{label}` was present but not a string"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_delta(_: StreamDelta) {}

    /// Feeds every `data:` payload into a fresh accumulator and returns it.
    fn accumulate(payloads: &[Value]) -> StreamAccumulator {
        let mut accumulator = StreamAccumulator::new();
        for payload in payloads {
            accumulator
                .apply(&payload.to_string(), &no_delta)
                .expect("fixture payloads are well-formed");
        }
        accumulator
    }

    fn content_chunk(text: &str) -> Value {
        serde_json::json!({
            "model": "qwen3-30b",
            "choices": [{ "index": 0, "delta": { "content": text }, "finish_reason": null }]
        })
    }

    #[test]
    fn scanner_splits_data_lines_and_skips_noise() {
        let mut scanner = SseScanner::new();
        scanner.extend(b": comment\nevent: message\ndata: {\"a\":1}\r\n\ndata: [DO");
        assert_eq!(scanner.next_data().as_deref(), Some("{\"a\":1}"));
        assert_eq!(scanner.next_data(), None, "partial line stays buffered");
        scanner.extend(b"NE]\n");
        assert_eq!(scanner.next_data().as_deref(), Some("[DONE]"));
    }

    #[test]
    fn streamed_accumulation_matches_the_buffered_fixture_byte_for_byte() {
        // The buffered llama.cpp fixture from the normalize suite, split
        // into a streamed form: the reassembled body must normalize to the
        // same turn and metadata, with the answer text byte-identical.
        let usage =
            serde_json::json!({ "completion_tokens": 3, "prompt_tokens": 7, "total_tokens": 10 });
        let timings = serde_json::json!({
            "prompt_n": 7, "prompt_ms": 12.5, "prompt_per_second": 560.0,
            "predicted_n": 3, "predicted_ms": 30.5, "predicted_per_second": 98.5
        });
        let accumulator = accumulate(&[
            content_chunk("Hel"),
            content_chunk("lo \u{1F980}"),
            content_chunk("!"),
            serde_json::json!({
                "model": "qwen3-30b",
                "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }]
            }),
            serde_json::json!({
                "model": "qwen3-30b",
                "choices": [],
                "usage": usage,
                "timings": timings
            }),
        ]);
        let body = accumulator.into_body();
        assert_eq!(
            body.pointer("/choices/0/message/content")
                .and_then(Value::as_str),
            Some("Hello \u{1F980}!"),
            "fragments concatenate byte-for-byte"
        );
        assert_eq!(
            body.pointer("/choices/0/finish_reason")
                .and_then(Value::as_str),
            Some("stop")
        );
        assert_eq!(body.get("model").and_then(Value::as_str), Some("qwen3-30b"));
        assert_eq!(body.get("usage"), Some(&usage), "usage kept verbatim");
        assert_eq!(body.get("timings"), Some(&timings), "timings kept verbatim");
    }

    #[test]
    fn tool_call_fragments_buffer_across_chunks_by_index() {
        // OpenAI streams a call's name once and its arguments in pieces;
        // interleaved fragments for two calls must land on their own
        // buffers, keyed by `index`, and reassemble whole.
        let accumulator = accumulate(&[
            serde_json::json!({ "choices": [{ "index": 0, "delta": { "tool_calls": [
                { "index": 0, "id": "call_a", "type": "function",
                  "function": { "name": "search", "arguments": "{\"qu" } }
            ] } }] }),
            serde_json::json!({ "choices": [{ "index": 0, "delta": { "tool_calls": [
                { "index": 1, "id": "call_b", "type": "function",
                  "function": { "name": "fetch", "arguments": "{\"url\":\"x\"}" } }
            ] } }] }),
            serde_json::json!({ "choices": [{ "index": 0, "delta": { "tool_calls": [
                { "index": 0, "function": { "arguments": "ery\":\"a\"}" } }
            ] } }] }),
            serde_json::json!({
                "choices": [{ "index": 0, "delta": {}, "finish_reason": "tool_calls" }]
            }),
        ]);
        assert!(accumulator.has_tool_calls());
        let body = accumulator.into_body();
        let calls = body
            .pointer("/choices/0/message/tool_calls")
            .and_then(Value::as_array)
            .expect("tool calls reassembled");
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0]["id"], "call_a");
        assert_eq!(calls[0]["function"]["arguments"], "{\"query\":\"a\"}");
        assert_eq!(calls[1]["id"], "call_b");
        assert_eq!(calls[1]["function"]["name"], "fetch");
    }

    #[test]
    fn reasoning_and_text_deltas_reach_the_callback_separated_in_order() {
        let seen = std::sync::Mutex::new(Vec::new());
        let mut accumulator = StreamAccumulator::new();
        let record = |delta: StreamDelta| seen.lock().expect("delta log").push(delta);
        for payload in [
            serde_json::json!({ "choices": [{ "index": 0,
                "delta": { "reasoning_content": "think" } }] }),
            serde_json::json!({ "choices": [{ "index": 0, "delta": { "content": "ans" } }] }),
            serde_json::json!({ "choices": [{ "index": 0, "delta": { "content": "wer" } }] }),
        ] {
            accumulator
                .apply(&payload.to_string(), &record)
                .expect("well-formed");
        }
        assert_eq!(
            *seen.lock().expect("delta log"),
            vec![
                StreamDelta::Reasoning("think".to_owned()),
                StreamDelta::Text("ans".to_owned()),
                StreamDelta::Text("wer".to_owned()),
            ]
        );
        let body = accumulator.into_body();
        assert_eq!(
            body.pointer("/choices/0/message/reasoning_content")
                .and_then(Value::as_str),
            Some("think"),
            "reasoning stays a side channel on the reassembled message"
        );
        assert_eq!(
            body.pointer("/choices/0/message/content")
                .and_then(Value::as_str),
            Some("answer")
        );
    }

    #[test]
    fn empty_choices_usage_chunk_is_metadata_not_a_turn() {
        // The `stream_options.include_usage` summary chunk has an empty
        // `choices` array; it must be consumed as metadata, never indexed
        // for a choice and never counted as a content delta.
        let mut accumulator = StreamAccumulator::new();
        let applied = accumulator
            .apply(
                &serde_json::json!({ "choices": [], "usage": { "prompt_tokens": 1,
                    "completion_tokens": 2, "total_tokens": 3 } })
                .to_string(),
                &no_delta,
            )
            .expect("summary chunk is well-formed");
        assert!(matches!(applied, Applied::Chunk { delta: false }));
        let body = accumulator.into_body();
        assert_eq!(
            body.pointer("/usage/total_tokens").and_then(Value::as_u64),
            Some(3)
        );
    }

    #[test]
    fn error_envelope_fails_the_stream_with_the_escaped_message() {
        let mut accumulator = StreamAccumulator::new();
        let error = accumulator
            .apply(
                &serde_json::json!({ "error": { "message": "upstream\ndied", "code": "x" } })
                    .to_string(),
                &no_delta,
            )
            .expect_err("an error envelope must fail the stream");
        assert!(matches!(error, Error::Http(_)));
        let source = std::error::Error::source(&error)
            .expect("the envelope message rides as the cause")
            .to_string();
        assert!(source.contains("upstream\\ndied"), "escaped: {source}");
    }

    #[test]
    fn malformed_chunks_are_rejected_not_skipped() {
        let cases: [(&str, &str); 4] = [
            ("not json", "undecodable payload"),
            ("{\"choices\":{}}", "non-array choices"),
            (
                "{\"choices\":[{\"index\":0,\"delta\":{\"content\":7}}]}",
                "non-string content",
            ),
            (
                "{\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"id\":\"x\"}]}}]}",
                "fragment without index",
            ),
        ];
        for (payload, label) in cases {
            let mut accumulator = StreamAccumulator::new();
            let error = accumulator.apply(payload, &no_delta).expect_err(label);
            assert!(
                matches!(
                    error,
                    Error::MalformedResponse(_) | Error::MalformedResponseSource { .. }
                ),
                "{label}: {error:?}"
            );
        }
    }

    #[test]
    fn non_first_choices_are_ignored_like_the_buffered_parser() {
        let accumulator = accumulate(&[
            content_chunk("kept"),
            serde_json::json!({ "choices": [{ "index": 1,
                "delta": { "content": "dropped" } }] }),
        ]);
        let body = accumulator.into_body();
        assert_eq!(
            body.pointer("/choices/0/message/content")
                .and_then(Value::as_str),
            Some("kept")
        );
    }

    #[test]
    fn no_content_at_all_reassembles_null_content() {
        let accumulator = accumulate(&[serde_json::json!({
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }]
        })]);
        let body = accumulator.into_body();
        assert_eq!(
            body.pointer("/choices/0/message/content"),
            Some(&Value::Null),
            "a stream with no content fragments yields a null content"
        );
    }
}
