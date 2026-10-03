Log, show, and debug what happens during a run, using the events that parsing and each step hand back.

You need this page when you keep a run log, show a transcript, or debug a model's traffic.

# Where this fits

The Harness loop in [Run a prompt](crate#run-a-prompt) already receives events from [`Prompt::parse`](crate::Prompt::parse) and [`Run::step`](crate::Run::step), and sets them aside. This page shows what to do with them. An [event](crate) is a record of something that happened during a run, for your log. Recording every event, or none, never changes what a run does.

# Log a run's events

Your Harness runs prompts, and you want a log of everything that happened in each parse and run, one JSON line per event.

Events feel like [`tracing`](https://docs.rs/tracing/latest/tracing/) events: records you can keep or drop without touching your program's logic. Unlike `tracing`, there is no global subscriber. The parse and every step hand the events back to you as values. Each one reports something that already happened, and the run never reads it back. That report is an [`Event`], and every event carries three coordinates: `execution`, the run's name; `section`, the scope that reported it; and `provenance`, its [task](crate::ids) and its position, `seq`, in that task's order.

````
# use std::sync::Arc;
# use promptforge::effect::{Effect, EffectAnswer};
# use promptforge::timestamp::Timestamp;
# use promptforge::vfs::perform_vfs_op;
# use promptforge::{Prompt, Run, RunContext, RunResult, Step};
use std::collections::HashSet;

use promptforge::event::Event;

# let source = concat!(
#     "---\n",
#     "name: greeter\n",
#     "description: Writes a note to the store and reads it back.\n",
#     "promptforge: 0\n",
#     "---\n\n",
#     "# Greeter\n\n",
#     "## Greet\n\n",
#     "```lua\n",
#     "store.write('note.md', 'hello ' .. args)\n",
#     "return store.read('note.md')\n",
#     "```\n",
# );
# fn answer(effect: Effect) -> EffectAnswer {
#     match effect {
#         Effect::Vfs { access, op } => EffectAnswer::Vfs(perform_vfs_op(&access, op)),
#         _ => EffectAnswer::Dropped,
#     }
# }
# fn drive(mut run: Run, mut record: impl FnMut(Vec<Event>)) -> RunResult {
#     loop {
#         match run.step() {
#             Step::Pending { effects, events } => {
#                 record(events);
#                 for (id, _provenance, effect) in effects {
#                     run.resume(id, answer(effect));
#                 }
#             }
#             Step::Done { result, events } => {
#                 record(events);
#                 return result;
#             }
#         }
#     }
# }
// 1. Parse the greeter, and start the log with its parse events before you check the result.
let (parsed, parse_events) = Prompt::parse(source, "greeter");
let mut log: Vec<Event> = parse_events;
let prompt = Arc::new(parsed?);

// 2. Number the run's events on from the parse's, run with the argument "world", and log every batch.
let ctx = RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH).provenance_start(u32::try_from(log.len())?);
let result = drive(Run::new(Arc::clone(&prompt), "world", ctx), |events| log.extend(events));
assert!(matches!(result, RunResult::Ok(text) if text == "hello world"));

// 3. Write each event as one JSON line, check that it reads back equal, and print its kind.
for event in &log {
    let line = serde_json::to_string(event)?;
    assert_eq!(&serde_json::from_str::<Event>(&line)?, event);
    let record: serde_json::Value = serde_json::from_str(&line)?;
    println!("{}", record["kind"]);
}

// 4. No two events share a task and seq pair, and a run you do not log returns the same result.
let keys: HashSet<_> = log.iter().map(Event::provenance).collect();
assert_eq!(keys.len(), log.len());
let quiet = drive(Run::new(prompt, "world", RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH)), drop);
assert!(matches!(quiet, RunResult::Ok(text) if text == "hello world"));
# Ok::<(), Box<dyn std::error::Error>>(())
````

1. This is the store-only greeter from the crate page, whose Lua writes `hello ` joined with the run's argument and returns what it reads back. It needs no model or tool, so every event here comes from parsing, the `## Greet` section and its Lua, and the store. [`Prompt::parse`](crate::Prompt::parse) returns its events even when parsing fails, and then they include [`ParseFailed`](Event::ParseFailed). So start the log with them before `parsed?` checks the result, and a failed parse still leaves a record of what was tried.
2. [`RunContext::new`](crate::RunContext::new) takes the run's name, `"greeter"`, which run events carry as `execution`. Its `7` seeds the marker that wraps untrusted text, so a live Harness draws it from a secure random source, and its last value is the start time sections read as `sys.when`. Parse events number under task `0` from zero, and so does the run's main walk, so its first events would reuse the parse's `(0, 0)`, `(0, 1)`, and on. Pass the parse's event count to [`RunContext::provenance_start`](crate::RunContext::provenance_start) to prevent that. It moves only task `0`'s start; other tasks count from zero under their own ids. The hidden `drive` helper is the step and answer loop from [Run a prompt](crate#run-a-prompt), handing every step's events to your closure.
3. [`serde_json::to_string`](https://docs.rs/serde_json/latest/serde_json/fn.to_string.html) writes one event as one flat JSON object. Its `kind` field holds the variant name in snake_case, and the three coordinates, `execution`, `section`, and `provenance`, sit beside the payload fields instead of nested under the variant name. So each event becomes one log line you can filter by `kind`. Reading the line back with [`serde_json::from_str`](https://docs.rs/serde_json/latest/serde_json/fn.from_str.html) gives an event equal to the one you wrote, metrics floats included. The log is a faithful copy you can load into tests and tools.
4. [`Event::provenance`] works on any event without a match on its variant, and so do [`Event::execution`] and [`Event::section`], because every variant has all three. Your log's task and `seq` columns come from one code path. The set proves that no two lines share a task and `seq` pair across the parse and the run. The second run passes `drop` as its recorder, logs nothing, and still returns `hello world`. Log every event, some, or none, as you like: the run's outputs, errors, and ordering stay the same either way.

A log line whose `kind` your version of this crate does not know fails to deserialize, because there is no catch-all variant to fall back to. If you read logs that a newer version wrote, handle that error.

You might expect a task's events to number 0, 1, 2 with no gaps, and read a gap as a lost event. Instead, the effects a task issues draw from the same counter, so the numbers are dense only across events and effects together.

Write every event as one JSON line keyed by its provenance, and the run behaves the same whether you keep them or not. Next, [Show a transcript](#show-a-transcript) turns the same stream into something a person can read.

# Show a transcript

You want to show a person what the model said and what the tools returned during a run.

Building a transcript feels like folding over an iterator of enum values and keeping the ones you care about. Unlike a plain token stream, most events are bookkeeping, and only a few hold text a person wants to read. Those few are the content events, and each one says where it came from.

````
# use std::collections::HashSet;
# use std::sync::Arc;
# use promptforge::effect::{Effect, EffectAnswer};
# use promptforge::event::Event;
# use promptforge::timestamp::Timestamp;
# use promptforge::vfs::perform_vfs_op;
# use promptforge::{Prompt, Run, RunContext, RunResult, Step};
use promptforge::model::{Completion, CompletionResult, ModelDescriptor, ModelId, ThinkingMode};
use promptforge::{Environment, tools::{ToolCatalog, ToolDescriptor, ToolId, ToolOutput}};
// 1. The greeter asks its model to reply to the note, and shouts the reply with a tool.
let source = concat!(
    "---\nname: greeter\ndescription: Replies to a note and shouts the reply.\npromptforge: 0\n",
    "models:\n  writer: {}\ntools:\n  shout: example/text/shout\n---\n\n# Greeter\n\n## Greet\n\n",
    "```lua\nstore.write('note.md', 'hello ' .. args)\nmodels.use('writer')\n",
    "return tools.call('shout', { text = models.infer(store.read('note.md')) })\n```\n",
);
# let (parsed, _parse_events) = Prompt::parse(source, "greeter");
# let prompt = parsed?;
// 2. Offer one canned model and the shout tool, and prepare the context.
let model = ModelDescriptor::new(ModelId::gateway("canned")?, "Replies hi there", 8_192u32.try_into()?, ThinkingMode::Never);
let schema = serde_json::json!({"type": "object", "properties": {"text": {"type": "string"}}});
let shout = ToolDescriptor::new(ToolId::parse("example/text/shout")?, "shout", "Shouts the text.", schema);
let environment = Environment::new().tools(ToolCatalog::new(&[shout])?);
let (ctx, requirements) = environment.prepare(&prompt, RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH).model(model));
assert!(requirements.refusal().is_none());
// 3. Answer the model round with a canned reply, and the tool call with canned output.
fn answer(effect: Effect) -> EffectAnswer {
    match effect {
        Effect::Vfs { access, op } => EffectAnswer::Vfs(perform_vfs_op(&access, op)),
        Effect::Chat { .. } => EffectAnswer::Chat(
            Completion::from_result(CompletionResult::Text("hi there".to_owned()), "canned").map(Box::new).map_err(Into::into),
        ),
        Effect::ToolCall { .. } => EffectAnswer::ToolCall(Ok(ToolOutput::trusted("HI THERE"))),
        _ => EffectAnswer::Dropped,
    }
}
# fn drive(mut run: Run, mut record: impl FnMut(Vec<Event>)) -> RunResult {
#     loop {
#         match run.step() {
#             Step::Pending { effects, events } => {
#                 record(events);
#                 for (id, _provenance, effect) in effects {
#                     run.resume(id, answer(effect));
#                 }
#             }
#             Step::Done { result, events } => {
#                 record(events);
#                 return result;
#             }
#         }
#     }
# }
// 4. Run the greeter with "world", keeping every event.
let mut log = Vec::new();
assert!(matches!(drive(Run::new(Arc::new(prompt), "world", ctx), |events| log.extend(events)), RunResult::Ok(text) if text == "HI THERE"));
// 5. Keep the content events, and pair each tool result with its call by turn and id.
let (mut asked, mut transcript) = (HashSet::new(), Vec::new());
for event in &log {
    match event {
        Event::AssistantToolCalls { turn, calls, .. } => asked.extend(calls.iter().map(|call| (*turn, call.id.clone()))),
        Event::AssistantReply { text, origin, .. } => transcript.push(format!("{origin:?} reply: {text}")),
        Event::ToolResult { turn, tool_call_id, alias, content, .. } => {
            let caller = if asked.contains(&(*turn, tool_call_id.clone())) { "model" } else { "script" };
            transcript.push(format!("{alias} for the {caller}: {content}"));
        }
        _ => {}
    }
}
println!("{}", transcript.join("\n"));
assert_eq!(transcript, ["Infer reply: hi there", "shout for the script: HI THERE"]);
# Ok::<(), Box<dyn std::error::Error>>(())
````

1. The greeter now declares a model role named `writer` and a tool slot named `shout`, both [defined on the crate page](crate). Its Lua passes the note to `models.infer` and hands the reply to `shout` through `tools.call`.
2. [`ModelDescriptor::new`](crate::model::ModelDescriptor::new) takes the model's id, a description, a context window in tokens that `try_into()?` refuses at zero, and a [`ThinkingMode`](crate::model::ThinkingMode), where `Never` means no `Thinking` events. [`Environment::prepare`](crate::Environment::prepare) fills `writer` with `canned`, the context's only model, and `shout` by its tool id in the catalog, as [Answer a model](crate#answer-a-model) and [Call a tool](crate#call-a-tool) show.
3. `answer` gains two arms: the chat effect gets `hi there`, and the tool call gets the trusted output `HI THERE`, which reaches its event unwrapped. [`Completion::from_result`](crate::model::Completion::from_result) builds a completion with no transport, served by the model named `"canned"`. It returns a `Result`, refusing an empty tool-call batch or repeated call ids, and `.map(Box::new).map_err(Into::into)` shapes it for [`EffectAnswer::Chat`](crate::effect::EffectAnswer::Chat).
4. The same `drive` loop keeps every event.
5. The loop keeps three content events and passes every other variant to the wildcard arm.
   - [`AssistantToolCalls`](Event::AssistantToolCalls) holds the calls a model asked for. [`AssistantReply`](Event::AssistantReply) holds model text, and [`ToolResult`](Event::ToolResult) holds each call's output. A fourth, [`Thinking`](Event::Thinking), holds a model's thinking blocks. The other events mark boundaries and hold no text to show.
   - To match a variant you skip, write it with braces, such as `Event::SectionStarted { .. }`, because every variant has the three coordinates as fields. End the match with a wildcard arm, because later versions can add variants.
   - Pair a `ToolResult` with the call it answers by `turn` and `tool_call_id` together. Providers reuse call ids across rounds, so the id alone can match the wrong call. When a script, not the model, made the call, `tool_call_id` is empty, so the greeter's result prints as the script's.
   - Read the reply's `origin`, a [`ReplyOrigin`], to place it. `Chat` belongs in the conversation. `Infer` came from a `models.infer` call, like this one, and your transcript can show it apart or leave it out.
   - Group a round's `Thinking`, `AssistantReply`, and `AssistantToolCalls` by their `round`, the id the round's [`Chat`](crate::effect::Effect::Chat) effect held, to show them together or to replace the live pieces you streamed for that round.

Treat reply, thinking, tool call, and tool result text as untrusted, and escape it before you render it. The text comes from a model, a tool, or a user, and can hold markup or instructions. `ToolResult.trusted` is `true` only for tool output marked [`OutputTrust::Trusted`](crate::tools::OutputTrust::Trusted).

A reply without `origin` reads back as `Chat`. Every model content event needs its `round`, so a log written before rounds were numbered does not load.

When a model's reply is cut off by its token limit, the run reports a [`ModelTurnTruncated`](Event::ModelTurnTruncated) event with no payload. Show it as a notice that the reply was cut short, because a cut-off reply otherwise reads like a finished one. Show [`ModelMetadataDegraded`](Event::ModelMetadataDegraded) as a warning: the turn succeeded, so its reply is still good. It reports a response with no `model` name, or each of its metadata sections, the top-level `usage`, `timings`, and `metrics` objects, that fails to parse, not a prompt's `##` section.

You might expect a `models.infer` call to report as its own kind of event. Instead, it reports as an `AssistantReply` just like a chat turn, and only `origin` tells them apart.

Show the content events, pair tool results by turn and id, and let `origin` decide what belongs in the conversation. Next, [Capture raw model traffic](#capture-raw-model-traffic) shows what each model round sent and received.

# Capture raw model traffic

A model is behaving strangely, and you want to see exactly what each round sent and received.

Capture feels like the verbose flag on an HTTP client that dumps request and response bodies. Unlike a log flag, the bodies arrive as ordinary events in the same stream, as JSON values. Capture copies each model round's raw bodies into the event stream, and nothing else about the run changes.

````
# use std::sync::Arc;
# use promptforge::effect::{Effect, EffectAnswer};
# use promptforge::event::Event;
# use promptforge::model::{Completion, CompletionResult, ModelDescriptor, ModelId, ThinkingMode};
# use promptforge::timestamp::Timestamp;
# use promptforge::vfs::perform_vfs_op;
# use promptforge::{Environment, tools::{ToolCatalog, ToolDescriptor, ToolId, ToolOutput}};
# use promptforge::{Prompt, Run, RunContext, RunResult, Step};
# let source = concat!(
#     "---\nname: greeter\ndescription: Replies to a note and shouts the reply.\npromptforge: 0\n",
#     "models:\n  writer: {}\ntools:\n  shout: example/text/shout\n---\n\n# Greeter\n\n## Greet\n\n",
#     "```lua\nstore.write('note.md', 'hello ' .. args)\nmodels.use('writer')\n",
#     "return tools.call('shout', { text = models.infer(store.read('note.md')) })\n```\n",
# );
# let (parsed, _parse_events) = Prompt::parse(source, "greeter");
# let prompt = Arc::new(parsed?);
# let model = ModelDescriptor::new(ModelId::gateway("canned")?, "Replies hi there", 8_192u32.try_into()?, ThinkingMode::Never);
# let schema = serde_json::json!({"type": "object", "properties": {"text": {"type": "string"}}});
# let shout = ToolDescriptor::new(ToolId::parse("example/text/shout")?, "shout", "Shouts the text.", schema);
# let environment = Environment::new().tools(ToolCatalog::new(&[shout])?);
# fn answer(effect: Effect) -> EffectAnswer {
#     match effect {
#         Effect::Vfs { access, op } => EffectAnswer::Vfs(perform_vfs_op(&access, op)),
#         Effect::Chat { .. } => EffectAnswer::Chat(
#             Completion::from_result(CompletionResult::Text("hi there".to_owned()), "canned").map(Box::new).map_err(Into::into),
#         ),
#         Effect::ToolCall { .. } => EffectAnswer::ToolCall(Ok(ToolOutput::trusted("HI THERE"))),
#         _ => EffectAnswer::Dropped,
#     }
# }
# fn drive(mut run: Run, mut record: impl FnMut(Vec<Event>)) -> RunResult {
#     loop {
#         match run.step() {
#             Step::Pending { effects, events } => {
#                 record(events);
#                 for (id, _provenance, effect) in effects {
#                     run.resume(id, answer(effect));
#                 }
#             }
#             Step::Done { result, events } => {
#                 record(events);
#                 return result;
#             }
#         }
#     }
# }
use promptforge::event::DebugMode;

// 1. Turn capture on while you build the context, then prepare and run the greeter.
let ctx = RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH).model(model.clone()).report_debug(DebugMode::On);
let (ctx, _requirements) = environment.prepare(&prompt, ctx);
let mut log = Vec::new();
let captured = drive(Run::new(Arc::clone(&prompt), "world", ctx), |events| log.extend(events));

// 2. Print each captured body with its turn, and collect the turns.
let (mut requests, mut responses) = (Vec::new(), Vec::new());
for event in &log {
    match event {
        Event::Request { turn, body, .. } => {
            println!("request, turn {turn}: {body}");
            requests.push(*turn);
        }
        Event::Response { turn, body, .. } => {
            println!("response, turn {turn}: {body}");
            responses.push(*turn);
        }
        _ => {}
    }
}

// 3. The one model round reports exactly one request and one response, on the same turn.
assert_eq!(requests.len(), 1);
assert_eq!(requests, responses);

// 4. Run again with capture off: neither event appears, and the result is the same.
let ctx = RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH).model(model);
let (ctx, _requirements) = environment.prepare(&prompt, ctx);
let mut quiet_log = Vec::new();
let quiet = drive(Run::new(prompt, "world", ctx), |events| quiet_log.extend(events));
assert!(!quiet_log.iter().any(|event| matches!(event, Event::Request { .. } | Event::Response { .. })));
assert!(matches!((captured, quiet), (RunResult::Ok(on), RunResult::Ok(off)) if on == off && on == "HI THERE"));
# Ok::<(), Box<dyn std::error::Error>>(())
````

1. Pass [`DebugMode::On`] to [`RunContext::report_debug`](crate::RunContext::report_debug) when you build the context, before prepare. It is the only switch, and you set it before the run starts. Each model round then reports an [`Event::Request`] with the raw body sent and an [`Event::Response`] with the raw body returned. The greeter is the one from [Show a transcript](#show-a-transcript).
2. The bodies arrive as ordinary events in the same stream. Here both print as `null`, because a canned [`Completion`](crate::model::Completion) carries no [`RawExchange`](crate::model::RawExchange). A transport is your code that sends each model round over HTTP, such as the `harness-gateway-client` crate. The completion that crate's `read_completion_stream` returns carries a `RawExchange` holding the exact request body the transport built and sent, and the response body. The response body is not the bytes on the wire: the reader rebuilds it from the streamed chunks into the shape a non-streaming backend would return. A broker that does not use HTTP can attach its own pair with [`Completion::with_raw`](crate::model::Completion::with_raw); a completion with none reports `null` for both bodies. The `Response` also holds the backend's `finish_reason` and `reasoning_content` when the backend supplied them, so you see what the model was sent beside what it answered.
3. Pair a `Request` with its `Response` by `turn`. The greeter's one model round is captured once, and both events share its turn.
4. Without `report_debug`, the context uses [`DebugMode::Off`], the default. Neither event appears, and the run returns the same `HI THERE`, because capture only adds events and events never change the run.

Treat both bodies as private and untrusted. The request body is the exact JSON sent and the response body is rebuilt from the stream. Both are unredacted and hold the full prompt and any user text, and nothing redacts them for you. A debug log can leak a user's private text if you store or show it as is.

Leave capture off unless you are debugging. Capture copies each body into its event, so it costs the log space and the copy those large bodies take, on top of the privacy risk above. The code that answered a round can read its bodies from [`Completion::raw`](crate::model::Completion::raw) before it hands the completion over, but a Host that only reads events sees them through capture alone. For what an effect log keeps, see [Log effects and answers](crate::effect#log-effects-and-answers).

You might expect to need capture to see what was sent to the model. Instead, your transport builds the exact request body itself, as the `harness-gateway-client` crate's `build_request_body` does, and hands that same value to its `read_completion_stream`, so you can log the request body yourself.

Turn capture on only to debug, and guard the bodies like the private text they are. The [Reference](#reference) lists every event a run can report.

# Reference

## DebugMode

[`DebugMode`] says whether a run captures each model round's raw request and response bodies as [`Event::Request`] and [`Event::Response`]. Use it when you want those bodies in the event stream to debug a model's traffic, and pass it to [`RunContext::report_debug`](crate::RunContext::report_debug). With capture on, treat the bodies as private, since they hold the full prompt unredacted. [Capture raw model traffic](#capture-raw-model-traffic) teaches it.

- `Off`: the default, so most Harnesses never set it; model rounds emit no `Request` or `Response` events.
- `On`: every model round emits its raw request and response bodies.

## Event

[`Event`] reports one thing that happened during a run, from lifecycle boundaries to content, task starts and ends, and opt-in debug captures. You receive events from [`Prompt::parse`](crate::Prompt::parse) and [`Run::step`](crate::Run::step) to log, show, or debug. The report never changes what the run does, and every payload is untrusted, even `execution` and `section`. A record whose `kind` this version does not know fails to deserialize; keep the raw JSON for a newer version. [Log a run's events](#log-a-runs-events) teaches it.

- [`Event::execution`]: returns the caller-chosen run id of any variant.
- [`Event::section`]: returns the reporting scope: an H2 heading or agent name for run events, and `"Prompt"` for parse start and end events.
- [`Event::provenance`]: returns the event's task and position in it; a `call`, which runs while its caller waits, is not a [task](crate::ids) and reports its caller's.
- Serialized form: one flat JSON object tagged by snake_case `kind`, which reads back identical from its [`serde_json::to_value`](https://docs.rs/serde_json/latest/serde_json/fn.to_value.html) form.
- Task `seq` numbers: shared with the effects the task issues, so events alone can skip numbers; a gap is not a lost event.

| Variant | What it reports |
|---|---|
| [`ParseStarted`](Event::ParseStarted) | Prompt parsing began. It reports under task `0` before any run exists. |
| [`ParseSucceeded`](Event::ParseSucceeded) | Prompt parsing completed, including parse-time compilation. |
| [`ParseFailed`](Event::ParseFailed) | Prompt parsing returned an error. |
| [`RunStarted`](Event::RunStarted) | A run passed its version gate and began. |
| [`RunSucceeded`](Event::RunSucceeded) | A run returned a value. |
| [`RunFailed`](Event::RunFailed) | A run returned an error. |
| [`SectionStarted`](Event::SectionStarted) | A top-level section began. |
| [`SectionFinished`](Event::SectionFinished) | A top-level section completed successfully. |
| [`ModelTurnCompleted`](Event::ModelTurnCompleted) | A model round trip completed. |
| [`ModelTurnFailed`](Event::ModelTurnFailed) | A model round trip returned an error. |
| [`ModelTurnTruncated`](Event::ModelTurnTruncated) | A model's reply was cut off by its length limit. It has no payload. |
| [`ModelMetadataDegraded`](Event::ModelMetadataDegraded) | Carries `turn` and `message`. One metadata section of a completed turn's response was malformed and degraded to nothing, or the response named no model. It follows the turn's `ModelTurnCompleted`, once per degraded section, and the turn still succeeded. `message` names the section and why, and may quote backend values. |
| [`ToolCallSucceeded`](Event::ToolCallSucceeded) | A tool dispatch completed. |
| [`ToolCallFailed`](Event::ToolCallFailed) | A tool dispatch returned an error. |
| [`LuaCompilationStarted`](Event::LuaCompilationStarted) | Lua source compilation began. |
| [`LuaCompilationSucceeded`](Event::LuaCompilationSucceeded) | Lua source compilation completed. |
| [`LuaCompilationFailed`](Event::LuaCompilationFailed) | Lua source compilation returned an error. |
| [`LuaSharedLoadStarted`](Event::LuaSharedLoadStarted) | A section VM began loading and executing its shared program. |
| [`LuaSharedLoadSucceeded`](Event::LuaSharedLoadSucceeded) | A section VM finished its shared program. |
| [`LuaSharedLoadFailed`](Event::LuaSharedLoadFailed) | A section VM's shared program returned an error. |
| [`LuaChunkStarted`](Event::LuaChunkStarted) | A section VM began executing a Lua chunk. |
| [`LuaChunkSucceeded`](Event::LuaChunkSucceeded) | A section VM finished a Lua chunk. |
| [`LuaChunkFailed`](Event::LuaChunkFailed) | A section VM's Lua chunk returned an error. |
| [`LuaReplyBindingStarted`](Event::LuaReplyBindingStarted) | A section VM began binding a model reply. |
| [`LuaReplyBindingSucceeded`](Event::LuaReplyBindingSucceeded) | A section VM bound a model reply. |
| [`LuaReplyBindingFailed`](Event::LuaReplyBindingFailed) | Binding a model reply returned an error. |
| [`LuaTeardownStarted`](Event::LuaTeardownStarted) | A section VM began teardown. |
| [`LuaTeardownSucceeded`](Event::LuaTeardownSucceeded) | A section VM completed teardown. |
| [`ToolScopeValidationStarted`](Event::ToolScopeValidationStarted) | Validation of a model-visible tool scope began. |
| [`ToolScopeValidationSucceeded`](Event::ToolScopeValidationSucceeded) | A model-visible tool scope passed validation. |
| [`ToolScopeValidationFailed`](Event::ToolScopeValidationFailed) | A model-visible tool scope failed validation. |
| [`ModelCatalogValidationStarted`](Event::ModelCatalogValidationStarted) | Live-catalog validation of a model binding began. |
| [`ModelCatalogValidationSucceeded`](Event::ModelCatalogValidationSucceeded) | A model binding passed live-catalog validation. |
| [`ModelCatalogValidationFailed`](Event::ModelCatalogValidationFailed) | A model binding failed live-catalog validation. |
| [`VfsWriteSucceeded`](Event::VfsWriteSucceeded) | A store write completed. The `Vfs` events carry no payload and have no started variant. |
| [`VfsWriteFailed`](Event::VfsWriteFailed) | A store write returned an error. |
| [`VfsAppendSucceeded`](Event::VfsAppendSucceeded) | A store append completed. |
| [`VfsAppendFailed`](Event::VfsAppendFailed) | A store append returned an error. |
| [`VfsReadSucceeded`](Event::VfsReadSucceeded) | A verbatim store read completed. |
| [`VfsReadFailed`](Event::VfsReadFailed) | A verbatim store read returned an error. |
| [`VfsReadNumberedSucceeded`](Event::VfsReadNumberedSucceeded) | A numbered store read completed. |
| [`VfsReadNumberedFailed`](Event::VfsReadNumberedFailed) | A numbered store read returned an error. |
| [`VfsReplaceSucceeded`](Event::VfsReplaceSucceeded) | A store replace completed. |
| [`VfsReplaceFailed`](Event::VfsReplaceFailed) | A store replace returned an error. |
| [`VfsDeleteSucceeded`](Event::VfsDeleteSucceeded) | A store delete completed. |
| [`VfsDeleteFailed`](Event::VfsDeleteFailed) | A store delete returned an error. |
| [`VfsGlobSucceeded`](Event::VfsGlobSucceeded) | A store glob completed. |
| [`VfsGlobFailed`](Event::VfsGlobFailed) | A store glob returned an error. |
| [`VfsExistsSucceeded`](Event::VfsExistsSucceeded) | A store existence check completed. |
| [`VfsExistsFailed`](Event::VfsExistsFailed) | A store existence check returned an error. |
| [`Lua`](Event::Lua) | Carries `message`, the text of a Lua `log(message)` call, verbatim. It is the one checkpoint a prompt author controls, and authors must never put arguments, replies, tool data, credentials, paths, or store contents in it. |
| [`TaskStarted`](Event::TaskStarted) | Carries `task`, `target`, `origin`, `input`, `item`, `index`, and `var`. A chain started by `tasks.spawn`, a `fanout` arm, or the model's `task` tool. It reports under the spawning section, and its payload holds every seed needed to start the same chain again under the same id. |
| [`TaskSucceeded`](Event::TaskSucceeded) | Carries `task`. The task's chain ended with a result. It reports under the task's target section, as the other ending variants do. |
| [`TaskFailed`](Event::TaskFailed) | Carries `task`. The task's chain ended with an error. |
| [`TaskCancelled`](Event::TaskCancelled) | Carries `task`. Its owner cancelled it on purpose. It reports once, and a repeated cancel reports nothing. |
| [`TaskAbandoned`](Event::TaskAbandoned) | Carries `task` and `reason`. The owner chain ended while the task was live, and `reason` says how the owner ended. Unlike `TaskCancelled`, nobody stopped it on purpose. |
| [`TaskResumed`](Event::TaskResumed) | Carries `task`. A task was revived from its record. |
| [`Thinking`](Event::Thinking) | Carries `turn`, `round`, `model`, and `text`. One completed block of model thinking, where `text` is untrusted model output. `round` is the [`RoundId`](crate::ids::RoundId) the round's [`Chat`](crate::effect::Effect::Chat) effect held. |
| [`AssistantReply`](Event::AssistantReply) | Carries `turn`, `round`, `text`, `finish_reason`, `model`, `metrics`, and `origin`. One completed model round's text reply. `model` is the model the answer's [`Completion`](crate::model::Completion) names, `metrics` is present only when anything was measured, and a record without `origin` reads back as `chat`. |
| [`AssistantToolCalls`](Event::AssistantToolCalls) | Carries `turn`, `round`, `model`, and `calls`. One batch of tool calls the model asked for, not yet run, with untrusted names and arguments. |
| [`ToolResult`](Event::ToolResult) | Carries `turn`, `tool_call_id`, `alias`, `content`, and `trusted`. The result of one dispatched tool call. `tool_call_id` is empty when a script made the call, and identifies a call only together with `turn`. `content` is untrusted unless `trusted` is `true`, which holds only for trusted tool output that was not nonce-wrapped. |
| [`TaskNotice`](Event::TaskNotice) | Carries `turn`, `task`, and `text`. The run's sentence telling an owner's model how a task it started ended, as queued, under the owner's section. A completed task's final text is embedded nonce-wrapped. |
| [`TaskNote`](Event::TaskNote) | Carries `task` and `text`. A task's `tasks.note` progress note. |
| [`Request`](Event::Request) | Carries `turn` and `body`. The raw, unredacted JSON body sent to the chat-completions endpoint for one model turn, full prompt included. It appears only with [`DebugMode::On`]. |
| [`Response`](Event::Response) | Carries `turn`, `body`, `finish_reason`, and `reasoning_content`. The JSON body returned for one model turn, reassembled from the stream, with the choice's `finish_reason` and the message's `reasoning_content` when the backend supplied them. It appears only with `DebugMode::On`. |

## ReplyOrigin

[`ReplyOrigin`] says which path produced one [`Event::AssistantReply`]: a chat turn or an inference round. Read it to tell a user-facing chat turn from a programmatic inference round you may keep out of the conversation. The same value is the `origin` of each [`Round`](crate::effect::Round) on a `Chat` effect, where it says whether the round streams live pieces. It serializes as a bare snake_case string, `"chat"` or `"infer"`. A stored string this version does not know fails to deserialize, with no fallback; read that log with a version that knows it. [Show a transcript](#show-a-transcript) teaches it.

- `Chat`: the default, filled in when a reply's `origin` is missing; the reply belongs in the conversation.
- `Infer`: a programmatic round from `models.infer`, reported as the same `AssistantReply` variant; the Host may treat it apart.

