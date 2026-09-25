Token usage and timing for each model reply, plus one record per requested tool call.

A model round can come back measured. This module holds the plain-data types that carry those measurements: how many tokens the round used, how fast the serving backend processed and generated them, and how long the round took on the host's own clock. It also holds [`ToolCallEvent`], the record of one requested tool call. A host reads these values to log costs, watch latency, and pair each tool call with its result. By the end of this page you can read every number a reply reports, pair tool calls with results, and persist all of it as JSON.

# Where this fits

Metrics start with a model round. When [`Run::step`](crate::Run::step) hands out an [`Effect::Chat`](crate::effect::Effect::Chat), the host performs the round, typically by building the body with [`build_request_body`](crate::transport::build_request_body) and reading the stream with [`read_completion_stream`](crate::transport::read_completion_stream). The result is a [`Completion`](crate::model::Completion), and the host can already read its usage and timing there. The host answers the effect through [`Run::resume`](crate::Run::resume) with [`EffectAnswer::Chat`](crate::effect::EffectAnswer::Chat) holding [`Ok`] of the boxed [`Completion`](crate::model::Completion).

On a later step, the run reports what came back:

- [`Event::AssistantReply`](crate::event::Event::AssistantReply) carries the round's [`CallMetrics`] in its [`metrics`](crate::event::Event#variant.AssistantReply.field.metrics) field.
- [`Event::AssistantToolCalls`](crate::event::Event::AssistantToolCalls) carries one [`ToolCallEvent`] per requested tool call in its [`calls`](crate::event::Event#variant.AssistantToolCalls.field.calls) field.
- [`Event::ToolResult`](crate::event::Event::ToolResult) reports each call's result.
- [`Event::ModelMetadataDegraded`](crate::event::Event::ModelMetadataDegraded) reports backend metrics that arrived in a broken form. It arrives after the turn's [`Event::ModelTurnCompleted`](crate::event::Event::ModelTurnCompleted).

Metrics are a pure report. The run never reads them back to decide anything, so they are safe to persist as the host's log, and logging or dropping them never changes a run.

# Reading a reply's metrics

A host finds a round's measurements in the [`Event::AssistantReply::metrics`](crate::event::Event#variant.AssistantReply.field.metrics) field, an [`Option`] of a [`CallMetrics`]. The run sets it to [`Some`] when at least one of four measuring sections reported, and to [`None`] when none did. Each section is itself an [`Option`]:

- [`CallMetrics::usage`] is the token accounting from the backend, as a [`Usage`].
- [`CallMetrics::llama`] is the llama.cpp server timing, as a [`LlamaTimings`].
- [`CallMetrics::vllm`] is the vLLM request metrics, as a [`VllmMetrics`].
- [`CallMetrics::client`] is the timing measured on the calling client's own clock, as a [`ClientTiming`].

This program turns a reply's metrics into one log line:

````
use promptforge::event::Event;
use promptforge::metrics::{CallMetrics, ClientTiming, Usage};

/// One log line for a measured model reply.
fn reply_line(metrics: &CallMetrics) -> String {
    let tokens = match &metrics.usage {
        Some(usage) => format!("{} tokens", usage.total_tokens),
        None => "tokens not reported".to_owned(),
    };
    match &metrics.client {
        Some(client) => format!("{tokens} in {} ms", client.e2e_ms),
        None => tokens,
    }
}

/// Logs the metrics of every reply that has any.
fn on_event(event: &Event, log: &mut Vec<String>) {
    if let Event::AssistantReply { metrics: Some(metrics), .. } = event {
        log.push(reply_line(metrics));
    }
}

let measured = CallMetrics {
    usage: Some(Usage {
        prompt_tokens: 7,
        completion_tokens: 3,
        total_tokens: 10,
        cached_tokens: None,
        reasoning_tokens: None,
    }),
    llama: None,
    vllm: None,
    client: Some(ClientTiming { ttft_ms: Some(9.5), mean_itl_ms: None, e2e_ms: 41.5 }),
};
assert_eq!(reply_line(&measured), "10 tokens in 41.5 ms");

let client_only = CallMetrics {
    usage: None,
    llama: None,
    vllm: None,
    client: measured.client.clone(),
};
assert_eq!(reply_line(&client_only), "tokens not reported in 41.5 ms");
````

Here is what each part does.

1. **Match the reply.** `on_event` runs on every logged event. The pattern `metrics: Some(metrics)` skips a reply that nothing measured, and the `..` skips the reply's other fields, which the [`event`](crate::event) page covers.
2. **Read the token count.** [`Usage::total_tokens`] is the call's total token count, a [`u32`]. [`Usage::prompt_tokens`] and [`Usage::completion_tokens`] hold the two parts.
3. **Read the time.** [`ClientTiming::e2e_ms`] is the end-to-end time in milliseconds, an [`f64`]. It is the one timing value that is always present once the client section exists.
4. **Handle each missing section.** Any section can be [`None`] on its own, so the host checks every section before reading it. The second value has client timing but no usage, and the line still comes out.
5. **Build fixtures with struct literals.** Every field of every type here is public, and no type has a constructor or [`Default`]. A test builds a value by naming every field, as the example does.

# Reading metrics before answering

A host does not have to wait for the event. The [`Completion`](crate::model::Completion) that answers an [`Effect::Chat`](crate::effect::Effect::Chat) already holds three of the four sections, through three accessors. Each takes `&self` and returns an [`Option`] of a reference.

- [`Completion::usage`](crate::model::Completion::usage) returns the [`Usage`].
- [`Completion::llama_timings`](crate::model::Completion::llama_timings) returns the [`LlamaTimings`].
- [`Completion::client_timing`](crate::model::Completion::client_timing) returns the [`ClientTiming`].

These values are the same ones that later appear in the [`CallMetrics`]. [`Completion`](crate::model::Completion) has no accessor for vLLM metrics, so a host sees a [`VllmMetrics`] only through [`CallMetrics::vllm`] on the reply event.

````
use promptforge::model::Completion;

/// Logs a finished round's measurements before the host answers the effect.
fn log_round(completion: &Completion, log: &mut Vec<String>) {
    if let Some(usage) = completion.usage() {
        log.push(format!(
            "{} prompt and {} completion tokens",
            usage.prompt_tokens, usage.completion_tokens,
        ));
    }
    if let Some(llama) = completion.llama_timings() {
        log.push(format!("{} of {} draft tokens accepted", llama.draft_n_accepted, llama.draft_n));
    }
    if let Some(client) = completion.client_timing() {
        log.push(format!("{} ms end to end", client.e2e_ms));
    }
}
````

**The host supplies the clock.** Client timing is measured against a clock that the host hands to [`read_completion_stream`](crate::transport::read_completion_stream), which never reads a clock on its own. The host passes a `started` argument, an [`Instant`](std::time::Instant) read when the request is sent, and a `now` argument, a function that returns an [`Instant`](std::time::Instant). A live transport passes `started` and [`Instant::now`](std::time::Instant::now). The [`transport`](crate::transport) page shows the full call.

# Tool-call records

When a model asks for tools, [`Event::AssistantToolCalls`](crate::event::Event::AssistantToolCalls) reports the batch, one [`ToolCallEvent`] per call. Each record holds the provider's id for the call, the tool name, and the arguments.

**Pairing calls with results.** Each dispatched call's result arrives as an [`Event::ToolResult`](crate::event::Event::ToolResult). Its [`tool_call_id`](crate::event::Event#variant.ToolResult.field.tool_call_id) matches the [`ToolCallEvent::id`] of the answered call. Providers recycle ids such as `call_1` across rounds, so a host keys the pairing by turn as well as by id. A result dispatched by a script instead of requested by the model has the id `""` and pairs with no call.

**What the run guarantees.** Within one turn, every id is nonblank and unique, and every name is nonblank. A blank id, a duplicate id within the turn, or a blank name fails the round as a malformed response. [`ToolCallEvent::arguments`] is always a parsed JSON object, never a JSON-encoded string. Missing, null, non-string, invalid-JSON, and non-object arguments also fail the round as malformed instead of being coerced.

**What it does not guarantee.** The name and arguments are untrusted model output. The name is not guaranteed to match a tool that was advertised to the model.

This function pairs each tool result with the call that requested it:

````
use std::collections::HashMap;

use promptforge::event::Event;
use promptforge::metrics::ToolCallEvent;

/// Finds the requested call behind each tool result, in result order.
fn requests_for_results(events: &[Event]) -> Vec<Option<&ToolCallEvent>> {
    let mut requested = HashMap::new();
    let mut found = Vec::new();
    for event in events {
        match event {
            Event::AssistantToolCalls { turn, calls, .. } => {
                for call in calls {
                    requested.insert((*turn, call.id.as_str()), call);
                }
            }
            Event::ToolResult { turn, tool_call_id, .. } => {
                found.push(requested.get(&(*turn, tool_call_id.as_str())).copied());
            }
            _ => {}
        }
    }
    found
}

let call = ToolCallEvent {
    id: "call_1".to_owned(),
    name: "read_file".to_owned(),
    arguments: serde_json::json!({ "z": 1, "a": 2, "m": 3 }),
};
assert!(call.arguments.is_object());
assert_eq!(
    serde_json::to_string(&call)?,
    r#"{"id":"call_1","name":"read_file","arguments":{"a":2,"m":3,"z":1}}"#,
);
assert!(requests_for_results(&[]).is_empty());
# Ok::<(), Box<dyn std::error::Error>>(())
````

The serialized arguments always have their keys in sorted order, even when the model's output used a different order. A host that compares or hashes logged arguments can rely on that order.

# Broken sections

A broken metrics section never fails a call. When the backend sends a malformed `usage`, `timings`, or `metrics` section, that section becomes [`None`] and the other sections are kept. The turn itself still succeeds. The run reports each degraded section as an [`Event::ModelMetadataDegraded`](crate::event::Event::ModelMetadataDegraded), whose serialized kind is `model_metadata_degraded`, with the message `` malformed `{key}` in completion response ignored: {error} ``. A host that sees a section missing where it expected one can look for this event to learn why.

# Persisting metrics

Every type on this page serializes to JSON and reads back with serde. An optional field that is [`None`] is left out on write, and a missing optional field reads back as [`None`].

**The shape is stable.** The serialized form of a [`CallMetrics`] is pinned by a test as the persisted-log schema. A change that renamed a field, reordered serialization, or made an absent field required would break every log written before it. A [`CallMetrics`] with all four sections [`None`] serializes to `{}`.

**Non-finite numbers become null.** A timing value that is NaN or infinite reaches the log as `null`.

This example writes the pinned line, an empty value, and a NaN, and reads back a [`Usage`] with its optional fields missing:

````
use promptforge::metrics::{CallMetrics, ClientTiming, LlamaTimings, Usage, VllmMetrics};

let metrics = CallMetrics {
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
    client: Some(ClientTiming { ttft_ms: Some(9.5), mean_itl_ms: Some(8.25), e2e_ms: 41.5 }),
};
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
assert_eq!(serde_json::to_string(&metrics)?, line);

let empty = CallMetrics { usage: None, llama: None, vllm: None, client: None };
assert_eq!(serde_json::to_string(&empty)?, "{}");

let nan = VllmMetrics {
    time_to_first_token_ms: None,
    generation_time_ms: None,
    queue_time_ms: None,
    mean_itl_ms: Some(f64::NAN),
    tokens_per_second: None,
};
assert_eq!(serde_json::to_string(&nan)?, r#"{"mean_itl_ms":null}"#);

let usage: Usage =
    serde_json::from_str(r#"{"prompt_tokens":7,"completion_tokens":3,"total_tokens":10}"#)?;
assert_eq!(usage.total_tokens, 10);
assert_eq!(usage.cached_tokens, None);
assert_eq!(usage.reasoning_tokens, None);
# Ok::<(), Box<dyn std::error::Error>>(())
````

# Reference

This part covers all six types in the module, starting with [`CallMetrics`] and its four sections and ending with [`ToolCallEvent`]. The host receives them from events and completions.

## CallMetrics

[`CallMetrics`] holds everything measured about one model call, with one optional section per source. The host receives it in [`Event::AssistantReply::metrics`](crate::event::Event#variant.AssistantReply.field.metrics), which is [`None`] when no section reported. The run assembles it from the round's [`Completion`](crate::model::Completion).

- [`CallMetrics::usage`], an [`Option`] of a [`Usage`], is the token accounting for the call. It is [`Some`] when the backend reported a `usage` section.
- [`CallMetrics::llama`], an [`Option`] of a [`LlamaTimings`], is the llama.cpp server's timing. It is [`Some`] when a llama.cpp server served the call and sent a top-level `timings` object.
- [`CallMetrics::vllm`], an [`Option`] of a [`VllmMetrics`], is vLLM's request metrics. It is [`Some`] when vLLM served the call and sent a top-level `metrics` object.
- [`CallMetrics::client`], an [`Option`] of a [`ClientTiming`], is the timing measured on the calling client's own clock. [`read_completion_stream`](crate::transport::read_completion_stream) always measures it for a completed stream.

Its JSON form is an object keyed by the four field names, in field order, each left out when [`None`]. [Persisting metrics](#persisting-metrics) shows the full pinned line.

## Usage

[`Usage`] is the token accounting for one model call, as the backend reported it. The host reads it from [`CallMetrics::usage`] or from [`Completion::usage`](crate::model::Completion::usage). It is parsed from the `usage` object of the chat-completions response.

- [`Usage::prompt_tokens`], a [`u32`], is the number of tokens in the prompt.
- [`Usage::completion_tokens`], a [`u32`], is the number of tokens generated in the completion.
- [`Usage::total_tokens`], a [`u32`], is the prompt and completion tokens together. It is copied from the backend's own total and not recomputed from the other two fields.
- [`Usage::cached_tokens`], an [`Option`] of [`u32`], is how many prompt tokens were served from the backend's prefix cache. It is [`Some`] only when the backend reports that detail. The backend sends it nested, as `usage.prompt_tokens_details.cached_tokens`.
- [`Usage::reasoning_tokens`], an [`Option`] of [`u32`], is how many tokens went to the model's reasoning. It is [`Some`] only when the backend reports that detail. The backend sends it nested, as `usage.completion_tokens_details.reasoning_tokens`.

The JSON form of [`Usage`] is flat. All five keys sit side by side, in field order, and the two optional keys are left out when [`None`]. This is not the backend's nested shape.

## LlamaTimings

[`LlamaTimings`] is a llama.cpp server's timing report for one call: prompt processing, generation, and speculative-decoding draft counts. The host reads it from [`CallMetrics::llama`] or from [`Completion::llama_timings`](crate::model::Completion::llama_timings). It is parsed from the top-level `timings` object of the server's response.

- [`LlamaTimings::prompt_n`], a [`u32`], is the number of prompt tokens processed.
- [`LlamaTimings::prompt_ms`], an [`f64`], is the wall-clock milliseconds spent processing the prompt.
- [`LlamaTimings::prompt_per_second`], an [`f64`], is the prompt processing rate in tokens per second.
- [`LlamaTimings::predicted_n`], a [`u32`], is the number of tokens generated.
- [`LlamaTimings::predicted_ms`], an [`f64`], is the wall-clock milliseconds spent generating.
- [`LlamaTimings::predicted_per_second`], an [`f64`], is the generation rate in tokens per second.
- [`LlamaTimings::draft_n`], a [`u32`], is the number of tokens proposed by the draft model during speculative decoding. It is `0` when no draft model ran.
- [`LlamaTimings::draft_n_accepted`], a [`u32`], is how many drafted tokens were accepted by the target model. It is `0` when no draft model ran. Divide it by [`LlamaTimings::draft_n`] for the acceptance rate.

The first six fields are required in the server's `timings` object. The two draft counters are not. When a server response leaves one out, it becomes `0`, because an absent counter means zero drafted tokens, not an unknown value. That rule applies only to parsing a server response. The JSON form of [`LlamaTimings`] itself has all eight keys, in field order, and requires all eight on read.

## VllmMetrics

[`VllmMetrics`] is vLLM's per-request metrics for one call. The host reads it only from [`CallMetrics::vllm`]. It is parsed from the top-level `metrics` object of the response body. Every field is an [`Option`] of [`f64`], because vLLM leaves out what it did not measure.

- [`VllmMetrics::time_to_first_token_ms`] is the milliseconds from the start of the request to the first generated token, as vLLM measured it on the server. The host-clock counterpart is [`ClientTiming::ttft_ms`].
- [`VllmMetrics::generation_time_ms`] is the milliseconds spent generating.
- [`VllmMetrics::queue_time_ms`] is the milliseconds spent waiting in vLLM's scheduler queue.
- [`VllmMetrics::mean_itl_ms`] is the mean inter-token latency in milliseconds, as vLLM measured it.
- [`VllmMetrics::tokens_per_second`] is the generation rate in tokens per second.

The JSON form is an object with the five keys in field order, each left out when [`None`]. Unknown keys are ignored on read.

## ClientTiming

[`ClientTiming`] is the timing of one call from end to end, measured on the calling client's own clock instead of reported by the backend. [`read_completion_stream`](crate::transport::read_completion_stream) produces it from the `started` instant and the `now` clock that the host passes in. The host reads it from [`CallMetrics::client`] or from [`Completion::client_timing`](crate::model::Completion::client_timing).

- [`ClientTiming::ttft_ms`], an [`Option`] of [`f64`], is the milliseconds from sending the request to the first streamed token. It is [`None`] when the stream produced no content delta.
- [`ClientTiming::mean_itl_ms`], an [`Option`] of [`f64`], is the mean inter-token latency in milliseconds. It is the time from the first delta chunk to the last, divided by one less than the number of delta chunks. It counts delta chunks, not tokens, so it is [`None`] unless at least two delta chunks arrived.
- [`ClientTiming::e2e_ms`], an [`f64`], is the milliseconds from sending the request to the completed response, read after the stream's `[DONE]` sentinel.

Every value is rounded to a whole microsecond, so the stored text parses back exactly on replay. The JSON form is an object keyed by the three field names, in field order, with the two optional keys left out when [`None`].

## ToolCallEvent

[`ToolCallEvent`] is one tool call requested by the model: the provider's id for it, the tool name, and the arguments. The host receives it in the [`calls`](crate::event::Event#variant.AssistantToolCalls.field.calls) field of [`Event::AssistantToolCalls`](crate::event::Event::AssistantToolCalls), one per requested call in that round. [Tool-call records](#tool-call-records) covers the guarantees and the pairing with results.

- [`ToolCallEvent::id`], a [`String`], is the provider-issued tool-call id.
- [`ToolCallEvent::name`], a [`String`], is the called tool's name.
- [`ToolCallEvent::arguments`], a [`serde_json::Value`](https://docs.rs/serde_json/latest/serde_json/enum.Value.html), is the call's arguments as the model produced them, decoded from the backend's JSON-encoded string. A host that names the [`serde_json::Value`](https://docs.rs/serde_json/latest/serde_json/enum.Value.html) type needs its own `serde_json` dependency.

The JSON form is an object keyed by the three field names, in field order, with the arguments object's own keys in sorted order.

