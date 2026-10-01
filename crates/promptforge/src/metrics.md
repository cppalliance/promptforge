[`CallMetrics`] tells you how many tokens one model call used and how long it took.

You need this when you report the cost, speed, or token usage of your model calls.

# Where this fits

Your program, the [Harness](crate), already answers model calls and logs the events each run reports, as the [event page](crate::event) shows. A run is one execution of one prompt, and an event is its record of something that happened. A call's metrics ride on the [`Event::AssistantReply`](crate::event::Event::AssistantReply) event that the run reports after a model call ends in a text reply. This page shows you how to read them.

One type here is not a measurement: [`ToolCallEvent`] is the record of one tool call a model asked for, which you read from [`Event::AssistantToolCalls`](crate::event::Event::AssistantToolCalls).

# Read a call's metrics

Your program runs prompts against a model server, and you want to report what each model call cost and how fast it was.

One call's measurements come as separate reports, one per source, and each is there only when its source sent it. Together they form a [`CallMetrics`] value. Reading one feels like reading the optional headers on an HTTP response. Unlike headers, one report comes from your own client's clock.

`usage`, `llama`, and `vllm` come from the server, and `client` comes from your clock. `llama` holds a llama.cpp server's timings and `vllm` holds a vLLM server's metrics, and at most the one for the server that served the call is normally present. `usage` counts tokens, for cost, and `client` times the call, for speed.

The greeter is the small program built on the crate page that runs one prompt file offline, with every model answer canned.

````
# use promptforge::event::{Event, ReplyOrigin};
# use promptforge::ids::{ChainId, Provenance, TaskId};
# use promptforge::model::{Completion, CompletionResult};
use promptforge::metrics::{CallMetrics, ClientTiming, Usage};

// 1. The greeter's canned reply measures nothing, so a test builds the metrics itself.
# let reply = CompletionResult::Text("hi there".to_owned());
let canned = Completion::from_result(reply, "canned");
assert!(canned.is_ok_and(|c| c.usage().is_none() && c.client_timing().is_none()));
let usage = Usage {
    prompt_tokens: 12,
    completion_tokens: 5,
    total_tokens: 17,
    cached_tokens: None,
    reasoning_tokens: None,
};
let client = ClientTiming { ttft_ms: None, mean_itl_ms: None, e2e_ms: 40.0 };
let metrics = CallMetrics { usage: Some(usage), llama: None, vllm: None, client: Some(client) };

// 2. Put them on the reply event the run reports for the greeter's chat round.
let mut event = Event::AssistantReply {
#     execution: "greeter".to_owned(),
#     section: "Greet".to_owned(),
#     provenance: Provenance { task: TaskId::from(ChainId::root()), seq: 0 },
#     turn: 1,
    text: "hi there".to_owned(),
#     finish_reason: Some("stop".to_owned()),
#     model: "canned".to_owned(),
    metrics: Some(metrics),
#     origin: ReplyOrigin::Chat,
};

// 3. Read each section only when it is present, and call a missing one unknown.
fn summary(event: &Event) -> String {
    let Event::AssistantReply { metrics: Some(metrics), .. } = event else {
        return "not measured".to_owned();
    };
    let tokens = match &metrics.usage {
        Some(usage) => format!("{} tokens", usage.total_tokens),
        None => "tokens unknown".to_owned(),
    };
    let time = match &metrics.client {
        Some(client) => format!("{} ms", client.e2e_ms),
        None => "time unknown".to_owned(),
    };
    format!("{tokens}, {time}")
}
let line = summary(&event);
println!("{line}");
assert_eq!(line, "17 tokens, 40 ms");

// 4. Take away the usage section, as a server that reports no token counts would.
if let Event::AssistantReply { metrics: Some(metrics), .. } = &mut event {
    metrics.usage = None;
}
assert_eq!(summary(&event), "tokens unknown, 40 ms");
````

1. Step 1 uses the greeter from [Answer a model](crate#answer-a-model).
   - [`Completion::from_result`](crate::model::Completion::from_result) cans the greeter's text reply and leaves every measurement empty, so the assertion checks that there are no token counts and no client timing.
   - It returns a `Result` only because it refuses an empty tool-call batch or two calls with the same id, and a text reply is never refused.
   - With no server to measure anything, the test builds [`Usage`] and [`ClientTiming`] itself and sets all four sections, `llama` and `vllm` to `None`, because `CallMetrics` has no `Default`.
2. Step 2 puts the metrics on an [`Event::AssistantReply`](crate::event::Event::AssistantReply), whose `metrics` field is `None` when nothing measured the call. A real Harness never builds this event: the run reports it after each model call that ends in a text reply, and you read it from the run's events as the [event page](crate::event) shows. The hidden `turn` field is the run's model-turn counter, which tells you which round produced the reply, where a round is one model call and the answer the run gets back. [`ReplyOrigin::Chat`](crate::event::ReplyOrigin::Chat) marks a reply that belongs in the user-facing conversation. `models.infer` is the Lua function a prompt calls to ask a model for a reply directly, as the crate page's examples show, and [`ReplyOrigin::Infer`](crate::event::ReplyOrigin::Infer) marks a reply produced that way, which a Host may keep apart from the conversation.
3. `summary` matches `metrics: Some(..)` first, so an unmeasured call prints `not measured` rather than looking like a bug. It prints each missing section as unknown, never zero, and here prints `17 tokens, 40 ms`.
4. Step 4 sets `usage` to `None`, as a server that reports no token counts leaves it. `client` remains, so you still report speed: `tokens unknown, 40 ms`.

`cached_tokens` and `reasoning_tokens` are `None` when the server did not report that detail.

A short or empty reply still gives you `e2e_ms`, the total time, but `ttft_ms` and `mean_itl_ms` need streamed tokens.

Code that reads `llama` for throughput must match on `Some` and fall back, for example to `client`, when vLLM served the call, and the same goes the other way.

Why two sets of timings? The server's timings describe work inside the server, such as queueing and generation, while `client` times the whole call on your clock, from sending the request to the completed response. Compare numbers within one source, and use `client` for the time your program actually waited.

You might expect `usage` to read as zeros when the server says nothing about tokens. Instead, the whole `usage` section is `None`, and a call that nothing measured has no `metrics` at all.

A missing section means not reported, never zero. The [Reference](#reference) describes each section's fields and when each one is absent.

# Reference

Every type here reads and writes as JSON with serde, which is how the run log stores it. A finite float reads back as exactly the same number that was written, so you can compare stored timings for equality.

Non-finite values do not read back. `NaN` writes as `null`. In an optional field it reads back as `None`, so a stored `None` can mean not measured or not finite. In a required float field, such as `e2e_ms` or any rate or duration in [`LlamaTimings`], the read fails. Keep non-finite values out of any [`CallMetrics`] you build or store.

## CallMetrics

[`CallMetrics`] holds everything measured about one model call, with one optional section per source that reported. You read it from the `metrics` field of [`Event::AssistantReply`](crate::event::Event::AssistantReply) to learn a call's cost and speed, as [Read a call's metrics](#read-a-calls-metrics) teaches. An empty value writes as `{}`, because absent sections are omitted. Reading ignores unknown keys. Reading fails when a section is present but missing a required field, such as `prompt_n` in [`LlamaTimings`]; supply them all, or leave that section out.

- `usage`: the token accounting, present when the server reported usage.
- `llama`: the llama.cpp server's timings, present when that server served the call.
- `vllm`: vLLM's request metrics, present when that server served the call.
- `client`: timing from your own client's clock, not the server's.

## ClientTiming

[`ClientTiming`] records one call's latency from end to end, as your own client's clock measured it. Read it from `CallMetrics.client` to see latency apart from what the server reported. Absent optional fields are left out of the JSON rather than written as `null`. Reading an object without `e2e_ms` fails, because it is the one required field, so always supply it when you build or store one. [Read a call's metrics](#read-a-calls-metrics) teaches it.

- `ttft_ms`: milliseconds from sending the request to the first streamed token; `None` when the stream produced no token.
- `mean_itl_ms`: the mean inter-token latency in milliseconds; `None` unless at least two tokens streamed.
- `e2e_ms`: milliseconds from sending the request to the completed response.

## LlamaTimings

[`LlamaTimings`] holds the llama.cpp server's `timings` for one call, as the server reported them. Read it from `CallMetrics.llama` for prompt and prediction throughput, or for speculative decoding. In speculative decoding, the server uses a small draft model to propose tokens ahead, and the main model, called the target model, checks each one and keeps the ones it accepts. No field is optional, so reading fails when any of the eight is missing; supply all eight.

- `prompt_n`, `predicted_n`: the prompt tokens processed and the tokens predicted.
- `prompt_ms`, `predicted_ms`: wall-clock milliseconds spent processing the prompt and spent predicting.
- `prompt_per_second`, `predicted_per_second`: the rates as the server reported them, not derived from the counts and durations, so a recomputed ratio may differ.
- `draft_n`: tokens the draft model proposed; always present, even without speculative decoding, so its presence does not show that speculative decoding ran.
- `draft_n_accepted`: draft tokens the target model accepted and kept.

## ToolCallEvent

[`ToolCallEvent`] records one tool call the model requested: its id, name, and arguments. Read it from the `calls` field of [`Event::AssistantToolCalls`](crate::event::Event::AssistantToolCalls) to see which tools the model asked for. It is a request record, not a measurement, and it lives in this module next to the model call it belongs to. Reading fails when `id`, `name`, or `arguments` is missing; supply all three when you build or store one.

- `id`: the provider-issued id. Providers reuse ids like `call_1` across rounds, so an id is unique only within a turn; scope any key by turn.
- `name`: the tool name the model called.
- `arguments`: a parsed value whose object keys write in sorted order, so its text can differ from the model's; never compare it byte for byte.

## Usage

[`Usage`] holds the token accounting for one model call, as the server reported it. Read it from `CallMetrics.usage` to account for the tokens a call consumed. Reading fails when `prompt_tokens`, `completion_tokens`, or `total_tokens` is missing. Supply all three counts, and leave the two optional details out when the server did not report them. [Read a call's metrics](#read-a-calls-metrics) teaches it.

- `prompt_tokens`: the tokens in the prompt.
- `completion_tokens`: the tokens generated in the completion.
- `total_tokens`: the total as the server reported it, described as prompt plus completion tokens; copied, never computed, so it can disagree with their sum.
- `cached_tokens`: prompt tokens served from a prefix cache; `None`, and left out of the JSON, when not reported.
- `reasoning_tokens`: tokens spent on reasoning; `None`, and left out of the JSON, when not reported.

## VllmMetrics

[`VllmMetrics`] holds vLLM's per-request metrics for one call. Read it from `CallMetrics.vllm` when a vLLM server served the call, for queueing, first-token, and generation latency. Every field is optional, because vLLM leaves out what it did not measure, so handle any subset being present. Reading never fails for a missing field, and absent fields are left out of the JSON. A `NaN` writes as `null` and reads back as `None`.

- `time_to_first_token_ms`: milliseconds from request start to the first generated token.
- `generation_time_ms`: milliseconds spent generating.
- `queue_time_ms`: milliseconds the request waited in the scheduler queue.
- `mean_itl_ms`: the mean inter-token latency in milliseconds, as the server measured it.
- `tokens_per_second`: the generation rate in tokens per second.

