Every kind of outside work that a run hands to its host, the answer for each kind, and the log records for both.

A run never performs outside work itself. Each model round, tool call, wait for operator input, store operation, timer, and read of a task's history reaches your program as an [`Effect`], and your program sends back exactly one [`EffectAnswer`]. This module is that whole contract: six effect kinds, seven answer kinds, and a pair of serializable records for logging them. By the end of this page you can answer every kind of effect, give up on one cleanly, and log each effect with its answer.

# Where this fits

[`Run::step`](crate::Run::step) returns [`Step::Pending`](crate::Step::Pending), whose [`effects`](crate::Step#variant.Pending.field.effects) list holds tuples of an [`EffectId`], a [`Provenance`](crate::ids::Provenance), and an [`Effect`]. The host appends the step's [`events`](crate::Step#variant.Pending.field.events) to its log first. Then it performs each [`Effect`] by kind and hands the result back as the matching [`EffectAnswer`] through [`Run::resume`](crate::Run::resume), under the same [`EffectId`]. The loop repeats until [`Step::Done`](crate::Step::Done). The [crate page](crate) explains the loop, and this page explains what goes inside it.

Each effect kind has one performer on the host side:

- [`Effect::Chat`] is a model round, answered with [`EffectAnswer::Chat`].
- [`Effect::ToolCall`] is a call into the host's own tool implementation, answered with [`EffectAnswer::ToolCall`].
- [`Effect::UserInput`] is a question for the host's operator, answered with [`EffectAnswer::UserInput`].
- [`Effect::Store`] is a store operation, performed with [`perform_store_op`](crate::vfs::perform_store_op) and answered with [`EffectAnswer::Store`].
- [`Effect::Timer`] is a sleep, answered with [`EffectAnswer::Timer`].
- [`Effect::TaskEvents`] is a filter over the host's own event log, answered with [`EffectAnswer::TaskEvents`].

The seventh answer, [`EffectAnswer::Dropped`], gives up on any kind of effect.

# A host that answers every kind

This host drives a prompt whose one section asks the operator a question, writes the reply to the store, reads it back, and returns it. The host has no operator, so it answers the question as unavailable.

````
use std::sync::Arc;
use std::time::Duration;

use promptforge::effect::{Effect, EffectAnswer};
use promptforge::event::Event;
use promptforge::input::InputOutcome;
use promptforge::model::{Completion, CompletionResult};
use promptforge::timestamp::Timestamp;
use promptforge::tools::ToolError;
use promptforge::vfs::perform_store_op;
use promptforge::{Prompt, Run, RunContext, RunResult, Step};

let source = concat!(
    "---\n",
    "name: asker\n",
    "description: asks the operator\n",
    "promptforge: 0\n",
    "---\n",
    "\n",
    "# Asker\n",
    "\n",
    "## Ask\n",
    "\n",
    "```lua\n",
    "local text = user_input()\n",
    "store.write('reply.md', text)\n",
    "return store.read('reply.md')\n",
    "```\n",
);
let (parsed, _parse_events) = Prompt::parse(source, "asker");
let ctx = RunContext::new("asker", 7, Timestamp::UNIX_EPOCH);
let mut run = Run::new(Arc::new(parsed?), "", ctx);

let mut log: Vec<Event> = Vec::new();
let result = loop {
    match run.step() {
        Step::Pending { effects, events } => {
            log.extend(events);
            for (id, _provenance, effect) in effects {
                let answer = match effect {
                    Effect::Chat { .. } => {
                        let reply = CompletionResult::Text("a canned reply".to_owned());
                        EffectAnswer::Chat(Ok(Box::new(Completion::from_result(reply, "canned"))))
                    }
                    Effect::ToolCall { .. } => {
                        EffectAnswer::ToolCall(Err(ToolError::message("this host has no tools")))
                    }
                    Effect::UserInput { .. } => EffectAnswer::UserInput(Ok(InputOutcome::Unavailable)),
                    Effect::Store { access, op } => EffectAnswer::Store(perform_store_op(&access, op)),
                    Effect::Timer { seconds } => {
                        std::thread::sleep(Duration::try_from_secs_f64(seconds).unwrap_or(Duration::ZERO));
                        EffectAnswer::Timer
                    }
                    Effect::TaskEvents { task, last } => EffectAnswer::TaskEvents(
                        log.iter()
                            .filter(|event| {
                                let provenance = event.provenance();
                                provenance.task == task && last.is_none_or(|seen| provenance.seq > seen)
                            })
                            .cloned()
                            .collect(),
                    ),
                };
                run.resume(id, answer);
            }
        }
        Step::Done { result, events } => {
            log.extend(events);
            break result;
        }
    }
};

match result {
    RunResult::Ok(text) => {
        assert_eq!(text, "User input is unavailable in this host; continue without it.");
    }
    other => panic!("the run should succeed: {other:?}"),
}
# Ok::<(), Box<dyn std::error::Error>>(())
````

Here is what each part does.

1. **The prompt.** The section calls `user_input()`, which issues an [`Effect::UserInput`]. Its two store calls each issue an [`Effect::Store`]. The other four arms never fire for this prompt, but each one shows the shape of its answer.
2. **One arm per kind.** Neither [`Effect`] nor [`EffectAnswer`] is `#[non_exhaustive]`, so the `match` lists all six effect kinds and needs no wildcard arm.
3. **The unavailable answer.** This host has no operator, so it answers [`InputOutcome::Unavailable`](crate::input::InputOutcome::Unavailable). The section resumes with a fixed sentence in place of operator text, stores it, and returns it, so the sentence becomes the text of [`RunResult::Ok`](crate::RunResult::Ok).
4. **Answering in place.** Every arm here produces its answer on the calling thread before the next effect. A real host may perform one step's effects concurrently and resume them in any order.

# One answer per effect

Every issued effect receives exactly one answer, and [`Step::Done`](crate::Step::Done) arrives only once every issued effect has its answer.

**Giving up.** [`EffectAnswer::Dropped`] answers an effect without performing it. It is valid for every effect kind, and it counts as that effect's one answer. A host drops an effect when the run was cancelled, when the effect's task ended first, or once [`Run::decided`](crate::Run::decided) returns `true`. If a chain still waits on a dropped effect, the chain resumes with a cancelled error, and if nothing handles that error, the run ends with [`RunResult::Cancelled`](crate::RunResult::Cancelled).

**Pairing.** [`Run::resume`](crate::Run::resume) checks every answer without panicking or returning an error. An answer whose kind does not match its effect, an answer for an id the run never issued, and a second answer for one effect all end the run with [`RunErrorKind::Internal`](crate::RunErrorKind::Internal). An answer for an effect whose chain stopped waiting is discarded, but it still counts as that effect's answer. After [`Step::Done`](crate::Step::Done), every answer is ignored.

# Answering each kind

This section takes the six effect kinds in turn. For each one it names the answer variant, says how the host produces the answer, and says what the run does with it.

**Chat.** [`Effect::Chat`] asks for one model round. The host sees it for each round of a section's `models.loop`, and for a nested `models.infer`. Answer it with [`EffectAnswer::Chat`], which holds a [`Result`] of a [`Box`] of a [`Completion`](crate::model::Completion) or a [`CompletionError`](crate::model::CompletionError). Both kinds of round take the same answer.

To produce the answer, build the request body with [`build_request_body`](crate::transport::build_request_body) from the effect's [`messages`](Effect#variant.Chat.field.messages), [`tools`](Effect#variant.Chat.field.tools), and [`options`](Effect#variant.Chat.field.options). Send the body with your HTTP client, and read the response with [`read_completion_stream`](crate::transport::read_completion_stream), which returns the [`Completion`](crate::model::Completion). Put it in a [`Box`] and answer [`Ok`]. When the request fails, convert the transport's [`ClientError`](crate::transport::ClientError) into a [`CompletionError`](crate::model::CompletionError) with [`From`], and answer [`Err`]. The [`transport`](crate::transport) module page covers both calls. For a canned reply in a test, [`Completion::from_result`](crate::model::Completion::from_result) builds a completion from a [`CompletionResult`](crate::model::CompletionResult) and a model name, as the example above does.

The effect's [`stream`](Effect#variant.Chat.field.stream) flag tells the host whether to forward the round's live deltas to its delta consumer. It is `true` for the rounds of `models.loop`. It is `false` for a nested `models.infer`, where only the completed reply matters, and then the host passes a no-op delta callback to [`read_completion_stream`](crate::transport::read_completion_stream).

The run reads the answer this way. A backend error that reports a provider context overflow becomes the overflow answer under a failed turn. An empty-reply error becomes a completed round with no reply. Any other error fails the turn. If the model requests a tool outside the set advertised for the round, the call fails as out of scope. The host never sees an over-window request for a `models.loop` round, because the run refuses it before issuing the effect.

**ToolCall.** [`Effect::ToolCall`] asks for one call to a bound tool. The host sees it when a section's script calls a bound tool, or when a model round requests one. Answer it with [`EffectAnswer::ToolCall`], which holds a [`Result`] of a [`ToolOutput`](crate::tools::ToolOutput) or a [`ToolError`](crate::tools::ToolError).

To produce the answer, resolve the effect's [`tool`](Effect#variant.ToolCall.field.tool) to your own implementation. It is the tool's stable [`ToolId`](crate::tools::ToolId), and it names the implementation behind the tool slot that [`Environment::prepare`](crate::Environment::prepare) filled. The [`alias`](Effect#variant.ToolCall.field.alias) is only the prompt's local name, so never resolve by it. Call the implementation with the effect's [`args`](Effect#variant.ToolCall.field.args). Build a success with [`ToolOutput::trusted`](crate::tools::ToolOutput::trusted) or [`ToolOutput::untrusted`](crate::tools::ToolOutput::untrusted), and a failure with [`ToolError::message`](crate::tools::ToolError::message) or [`ToolError::with_source`](crate::tools::ToolError::with_source), optionally refined with [`ToolError::with_kind`](crate::tools::ToolError::with_kind). When the id resolves to nothing in your table, answer an error, as the example above does. The [`tools`](crate::tools) module page covers tool implementations.

The run counts the call when it issues the effect, before the host runs the tool. After the answer arrives, the run applies its trust rule, which wraps untrusted output in a nonce envelope. A local tool is a Lua function on the section's own Lua state, and the run answers it internally, so it never becomes an [`Effect::ToolCall`].

**UserInput.** [`Effect::UserInput`] is one wait for operator input. The host sees one for every `user_input()` call, whether or not it has an operator. Answer it with [`EffectAnswer::UserInput`], which holds a [`Result`] of an [`InputOutcome`](crate::input::InputOutcome) or an [`InputError`](crate::input::InputError). In Lua, `user_input()` returns two values: the text and an `available` flag. There are three answers:

- [`InputOutcome::Text`](crate::input::InputOutcome::Text) in [`Ok`] carries the operator's text, which the section receives byte-exact. The section resumes with `available` set to true.
- [`InputOutcome::Unavailable`](crate::input::InputOutcome::Unavailable) in [`Ok`] means the host has no input to give. The section resumes with the fixed sentence "User input is unavailable in this host; continue without it." and `available` set to false.
- An [`InputError`](crate::input::InputError) in [`Err`] reports that the host's input handling failed. Build it with [`InputError::message`](crate::input::InputError::message) or [`InputError::with_source`](crate::input::InputError::with_source). It raises a [`RunErrorKind::Input`](crate::RunErrorKind::Input) failure at the Lua call site.

A blocking host may hold the effect until the operator answers, and the rest of the run keeps moving meanwhile. The [`input`](crate::input) module page covers the outcomes.

**Store.** [`Effect::Store`] is one store operation under the chain's access capability. The host sees one for every `store.*` call, whatever backend serves the store. Answer it with [`EffectAnswer::Store`], which holds a [`Result`] of a [`StoreOutcome`](crate::vfs::StoreOutcome) or a [`StoreError`](crate::vfs::StoreError). To produce the answer, pass a reference to the effect's [`access`](Effect#variant.Store.field.access) and its [`op`](Effect#variant.Store.field.op) to [`perform_store_op`](crate::vfs::perform_store_op). Its return value is exactly the variant's payload, so wrap it in [`EffectAnswer::Store`] as it is.

Use the access capability exactly as given. Never derive, widen, or keep store scope from it. Drop your handle to it when the operation completes and before you answer, so its claims are released before a resumed chain can take overlapping claims. [`perform_store_op`](crate::vfs::perform_store_op) is synchronous, because the store is synchronous by design, so an async host runs it off its executor, for example with tokio's [`spawn_blocking`](https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html). A store answer that reports a claims conflict ends the run at once with [`RunErrorKind::Determinism`](crate::RunErrorKind::Determinism).

**Timer.** [`Effect::Timer`] is one sleep of [`seconds`](Effect#variant.Timer.field.seconds). It is the internal timeout behind a timed wait, which an author sets with `opts.timeout` and a model sets with the timeout of its `await_tasks` call. Answer it with [`EffectAnswer::Timer`], which carries no data, once that many seconds have passed. The example converts the value with [`Duration::try_from_secs_f64`](std::time::Duration::try_from_secs_f64) and sleeps with [`sleep`](std::thread::sleep).

A timer resumes no chain. Its firing completes an internal task slot and wakes whatever waits on it. Dropping a timer instead moves its slot to cancelled without waking its owner, so a host drops a live timer only when it is cancelling the run.

**TaskEvents.** [`Effect::TaskEvents`] is one read of a task's reported history. The run issues it for an author's `tasks.events(task, opts?)` call and for a model's `task_events` built-in. Answer it with [`EffectAnswer::TaskEvents`], which holds a [`Vec`] of [`Event`](crate::event::Event) values taken from the host's own log.

To produce the answer, keep every event in your log whose [`Event::provenance`](crate::event::Event::provenance) has a [`Provenance::task`](crate::ids::Provenance::task) equal to the effect's [`task`](Effect#variant.TaskEvents.field.task). When the effect's [`last`](Effect#variant.TaskEvents.field.last) is [`Some`], keep only events whose [`Provenance::seq`](crate::ids::Provenance::seq) is greater than it. When it is [`None`], keep all of the task's events. Return them in log order. An empty [`Vec`] is a valid answer.

Commit each step's events to the log before performing that step's effects, so the read sees everything reported before it was issued. The reader receives the events as untrusted, nonce-wrapped JSON lines, or the trusted sentence "no new events" when the answer is empty.

# Logging effects and answers

[`Effect`] and [`EffectAnswer`] do not serialize, and they are not [`Clone`]. An effect can hold a live handle such as the store access capability, and an answer can hold values that a log cannot keep whole, such as a completion's bodies or an error's boxed cause. So a logging host projects each one onto a record.

- [`Effect::record`] returns an [`EffectRecord`], the same request minus its live handles.
- [`EffectAnswer::record`] returns an [`AnswerRecord`], the same outcome with every failure rendered as its [`Display`](std::fmt::Display) text.

Both methods borrow, so order matters. Call [`Effect::record`] before the host moves the effect's fields out, because performing an [`Effect::Store`] or an [`Effect::Chat`] consumes them. Call [`EffectAnswer::record`] before handing the answer to [`Run::resume`](crate::Run::resume), which consumes it.

Both records derive serde's [`Serialize`](https://docs.rs/serde/latest/serde/trait.Serialize.html) and [`Deserialize`](https://docs.rs/serde/latest/serde/trait.Deserialize.html) with no attributes, so they use serde's default externally tagged form. A struct variant becomes an object under its name, such as `{"Timer":{"seconds":0.5}}`. A unit variant becomes its bare name, such as `"Dropped"`. An inner [`Result`] becomes `{"Ok": ...}` or `{"Err": "..."}`. They round-trip through serde, for example as JSON, so a host can store a run log and later compare a re-executed run's records against it. Replay itself is not built yet. The records define what a log stores and what a future replay would compare, and nothing re-executes a log today.

````
use promptforge::effect::{
    AnswerRecord, ChatAnswerRecord, Effect, EffectAnswer, EffectRecord, ToolAnswerRecord,
};
use promptforge::model::{Completion, CompletionResult};
use promptforge::tools::{ToolError, ToolOutput};

let effect = Effect::Timer { seconds: 0.5 };
let record = effect.record();
assert_eq!(record, EffectRecord::Timer { seconds: 0.5 });
assert_eq!(serde_json::to_string(&record)?, r#"{"Timer":{"seconds":0.5}}"#);

let reply = CompletionResult::Text("the reply".to_owned());
let answer = EffectAnswer::Chat(Ok(Box::new(Completion::from_result(reply, "test-model"))));
assert_eq!(
    answer.record(),
    AnswerRecord::Chat(Ok(ChatAnswerRecord {
        model: "test-model".to_owned(),
        finish_reason: None,
        reply: Some("the reply".to_owned()),
        tool_calls: Vec::new(),
    })),
);

let trusted = EffectAnswer::ToolCall(Ok(ToolOutput::trusted("done"))).record();
assert_eq!(
    trusted,
    AnswerRecord::ToolCall(Ok(ToolAnswerRecord { text: "done".to_owned(), trusted: true })),
);
let failed = EffectAnswer::ToolCall(Err(ToolError::message("backend failed"))).record();
assert_eq!(failed, AnswerRecord::ToolCall(Err("backend failed".to_owned())));

let dropped = EffectAnswer::Dropped.record();
assert_eq!(serde_json::to_string(&dropped)?, r#""Dropped""#);
let stored: AnswerRecord = serde_json::from_str(r#""Dropped""#)?;
assert_eq!(stored, dropped);
# Ok::<(), Box<dyn std::error::Error>>(())
````

The timer's record keeps the effect's `0.5` seconds, and its JSON is the externally tagged form. The canned completion records as a [`ChatAnswerRecord`] with its reply text and no finish reason. The failed tool call records only its message. The dropped answer's record reads back from JSON unchanged.

# Reference

This part covers every item in the module: the effect handle, the effect and answer enums, and the records a log stores for them.

## EffectId

[`EffectId`] is the run-wide handle of one in-flight effect, the key that pairs an issued [`Effect`] with its [`EffectAnswer`]. The host receives it as the first element of each tuple in [`Step::Pending::effects`](crate::Step#variant.Pending.field.effects) and passes the same id back to [`Run::resume`](crate::Run::resume). It has no public constructor and no serde form, so a host only ever receives one from a run.

- [`EffectId::get`] takes the id by value and returns its raw [`u64`] handle, so a host can key its own log or task table by it. It cannot fail.

[`EffectId`] implements [`Display`](std::fmt::Display), which writes the same raw number, and it can serve as a [`HashMap`](std::collections::HashMap) key. The id comes from a run-wide counter and means something only within the run that issued it. It need not reproduce across runs, so a log that matches effects across runs uses the effect's [`Provenance`](crate::ids::Provenance) instead.

## Effect

[`Effect`] is one piece of work the run asks the host to perform. The host receives it from [`Run::step`](crate::Run::step) in [`Step::Pending::effects`](crate::Step#variant.Pending.field.effects), in issue order, paired with its [`EffectId`] and the [`Provenance`](crate::ids::Provenance) of the task that built it. The variants and their fields are public, so a value can be built directly, as the records example builds a timer. A host never needs to build one to drive a run. [`Effect`] has no serde form and is not [`Clone`], so a host logs it through [`Effect::record`]. [Answering each kind](#answering-each-kind) says how to answer each variant.

- [`Effect::Chat`]: one model round over its messages, with its tool schemas advertised, under the binding's frozen options. The host sees it for each round of a section's `models.loop` and for a nested `models.infer`. Answer it with [`EffectAnswer::Chat`].
  - [`Effect::Chat::binding`](Effect#variant.Chat.field.binding), a [`ModelBinding`](crate::model::ModelBinding), is the round's binding: its alias, model id, frozen invocation, and context window. Read [`ModelBinding::id`](crate::model::ModelBinding::id), [`ModelBinding::alias`](crate::model::ModelBinding::alias), and [`ModelBinding::invocation`](crate::model::ModelBinding::invocation) when routing the request. The effect's record takes its model name, alias, temperature, token cap, and thinking switch from the binding.
  - [`Effect::Chat::messages`](Effect#variant.Chat.field.messages), a [`Vec`] of [`Message`](crate::model::Message), is the projected conversation in wire order. Pass it as the first argument of [`build_request_body`](crate::transport::build_request_body). A nested `models.infer` carries exactly one message, built with [`Message::user`](crate::model::Message::user) from its prompt.
  - [`Effect::Chat::tools`](Effect#variant.Chat.field.tools), a [`Vec`] of [`ToolSchema`](crate::model::ToolSchema), is the set of tool schemas advertised for the round. It is always empty for a nested `models.infer`. Pass it to [`build_request_body`](crate::transport::build_request_body), which omits the tools field from the body when the slice is empty.
  - [`Effect::Chat::options`](Effect#variant.Chat.field.options), a [`CompletionOptions`](crate::model::CompletionOptions), holds the per-request fields, built from the binding with [`ModelBinding::completion_options`](crate::model::ModelBinding::completion_options) when the run issued the effect. They name the model on the wire. Pass them as the third argument of [`build_request_body`](crate::transport::build_request_body).
  - [`Effect::Chat::stream`](Effect#variant.Chat.field.stream), a [`bool`], says whether the host forwards the round's live deltas to its delta consumer. It is `true` for a `models.loop` round and `false` for a nested `models.infer`. It is not recorded, because a delta is not an event and the flag changes nothing in the request body.
- [`Effect::ToolCall`]: one call to a bound tool. The host sees it when a section's script calls a bound tool, or when a model round requests one. Answer it with [`EffectAnswer::ToolCall`].
  - [`Effect::ToolCall::tool`](Effect#variant.ToolCall.field.tool), a [`ToolId`](crate::tools::ToolId), is the tool's stable identity, in `namespace/pack/name` form. The host resolves this field to its implementation.
  - [`Effect::ToolCall::alias`](Effect#variant.ToolCall.field.alias), a [`String`], is the prompt-local name used in the call. It is kept for the record and plays no part in resolving the implementation.
  - [`Effect::ToolCall::args`](Effect#variant.ToolCall.field.args), a [`serde_json::Value`](https://docs.rs/serde_json/latest/serde_json/enum.Value.html), holds the call's arguments. Pass it to the tool implementation.
- [`Effect::UserInput`]: one wait for operator input on behalf of one section. The host sees one for every `user_input()` call. Answer it with [`EffectAnswer::UserInput`].
  - [`Effect::UserInput::execution`](Effect#variant.UserInput.field.execution), a [`String`], is the run's execution identifier, which is the `name` the host passed to [`RunContext::new`](crate::RunContext::new).
  - [`Effect::UserInput::section`](Effect#variant.UserInput.field.section), a [`String`], is the name of the section asking for input.
- [`Effect::Store`]: one store operation under the chain's access capability. The host sees one for every `store.*` call. Answer it with [`EffectAnswer::Store`].
  - [`Effect::Store::access`](Effect#variant.Store.field.access), an [`Arc`](std::sync::Arc) of an [`Access`](crate::vfs::Access), is the chain's access capability. The run mints it from the chain's claims, and it is released when the operation completes. Pass a reference to it to [`perform_store_op`](crate::vfs::perform_store_op). It is not recorded.
  - [`Effect::Store::op`](Effect#variant.Store.field.op), a [`StoreOp`](crate::vfs::StoreOp), is the validated operation: a write, append, read, numbered read, string replace, delete, glob, or existence check. Pass it by value to [`perform_store_op`](crate::vfs::perform_store_op).
- [`Effect::Timer`]: one sleep, the internal timeout behind a timed wait. Answer it with [`EffectAnswer::Timer`].
  - [`Effect::Timer::seconds`](Effect#variant.Timer.field.seconds), an [`f64`], is the sleep duration in seconds. It is non-negative and finite, because the run checks it with [`Duration::try_from_secs_f64`](std::time::Duration::try_from_secs_f64) before issuing the effect.
- [`Effect::TaskEvents`]: one read of a task's reported history. Answer it with [`EffectAnswer::TaskEvents`].
  - [`Effect::TaskEvents::task`](Effect#variant.TaskEvents.field.task), a [`TaskId`](crate::ids::TaskId), is the task whose events are read. Compare it with each logged event's [`Provenance::task`](crate::ids::Provenance::task).
  - [`Effect::TaskEvents::last`](Effect#variant.TaskEvents.field.last), an [`Option`] of [`u32`], is the highest sequence number already seen by the reader. [`None`] asks for all of the task's events. [`Some`] asks only for events whose [`Provenance::seq`](crate::ids::Provenance::seq) is greater than the value.

[`Effect::record`] borrows the effect and returns its [`EffectRecord`]. It cannot fail. The record of an [`Effect::Chat`] flattens the binding to the model name, alias, and frozen invocation, and stores the messages in wire form and the tools by name. The record of an [`Effect::Store`] keeps only the operation.

## EffectAnswer

[`EffectAnswer`] is the host's reply to one [`Effect`]: one variant per effect kind, plus [`EffectAnswer::Dropped`] for an effect the host gave up on. The host builds it from the result of performing the effect and passes it to [`Run::resume`](crate::Run::resume) under the effect's [`EffectId`]. Each variant with a payload wraps the exact result type that its performer returns. [`EffectAnswer`] has no serde form and is not [`Clone`], so a host logs it through [`EffectAnswer::record`].

- [`EffectAnswer::Chat`] holds a [`Result`] of a [`Box`] of a [`Completion`](crate::model::Completion) or a [`CompletionError`](crate::model::CompletionError). It answers an [`Effect::Chat`], including one from a nested `models.infer`. The completion is boxed because it holds both the request and response bodies.
- [`EffectAnswer::ToolCall`] holds a [`Result`] of the tool's own [`ToolOutput`](crate::tools::ToolOutput) or [`ToolError`](crate::tools::ToolError). It answers an [`Effect::ToolCall`], and the run applies its trust rule after it arrives.
- [`EffectAnswer::UserInput`] holds a [`Result`] of an [`InputOutcome`](crate::input::InputOutcome) or an [`InputError`](crate::input::InputError). It answers an [`Effect::UserInput`].
- [`EffectAnswer::Store`] holds a [`Result`] of a [`StoreOutcome`](crate::vfs::StoreOutcome) or a [`StoreError`](crate::vfs::StoreError), which is exactly the return type of [`perform_store_op`](crate::vfs::perform_store_op). It answers an [`Effect::Store`].
- [`EffectAnswer::Timer`] carries no data. It answers an [`Effect::Timer`] once the effect's [`seconds`](Effect#variant.Timer.field.seconds) have passed.
- [`EffectAnswer::TaskEvents`] holds a [`Vec`] of [`Event`](crate::event::Event) values: the task's events after the read's [`last`](Effect#variant.TaskEvents.field.last), in the host's log order. It answers an [`Effect::TaskEvents`].
- [`EffectAnswer::Dropped`] carries no data. It answers any kind of effect without performing it, as [One answer per effect](#one-answer-per-effect) describes.

[`EffectAnswer::record`] borrows the answer and returns its [`AnswerRecord`]. It cannot fail. A failure is recorded as its [`Display`](std::fmt::Display) text. A completion is recorded as a [`ChatAnswerRecord`], because the round's bodies travel as debug events and its metrics travel in the turn's event. A tool output becomes a [`ToolAnswerRecord`], input and store outcomes become an [`InputAnswerRecord`] and a [`StoreAnswerRecord`], and task events are cloned.

## EffectRecord

[`EffectRecord`] is an [`Effect`] minus its live handles, which is what a run log stores for the effect. The host gets one from [`Effect::record`], or deserializes one from a stored log. Every variant can also be built directly. It uses serde's externally tagged form, described in [Logging effects and answers](#logging-effects-and-answers).

- [`EffectRecord::Chat`]: one model round, recorded from an [`Effect::Chat`]. It reads like the request body the host would build. Neither [`Effect::Chat::stream`](Effect#variant.Chat.field.stream) nor the full [`CompletionOptions`](crate::model::CompletionOptions) is recorded.
  - [`EffectRecord::Chat::model`](EffectRecord#variant.Chat.field.model), a [`String`], is the bound model's name, from [`ModelId::name`](crate::model::ModelId::name) of the binding's id, for example `"test-model"`.
  - [`EffectRecord::Chat::alias`](EffectRecord#variant.Chat.field.alias), a [`String`], is the prompt-local alias of the round's binding, from [`ModelBinding::alias`](crate::model::ModelBinding::alias), for example `"writer"`.
  - [`EffectRecord::Chat::messages`](EffectRecord#variant.Chat.field.messages), a [`Vec`] of [`serde_json::Value`](https://docs.rs/serde_json/latest/serde_json/enum.Value.html), is the conversation with one wire-form message per entry, for example `{"role":"user","content":"ask"}`.
  - [`EffectRecord::Chat::tools`](EffectRecord#variant.Chat.field.tools), a [`Vec`] of [`String`], holds the advertised tool names in schema order. It is empty when the round advertised none.
  - [`EffectRecord::Chat::temperature`](EffectRecord#variant.Chat.field.temperature), an [`Option`] of [`f64`], is the frozen sampling temperature from the binding's invocation, when the binding declared one.
  - [`EffectRecord::Chat::max_tokens`](EffectRecord#variant.Chat.field.max_tokens), an [`Option`] of [`u32`], is the frozen generation cap from the binding's invocation, when the binding declared one. [`Effect::record`] never produces `Some(0)`, because the cap it copies is non-zero.
  - [`EffectRecord::Chat::thinking`](EffectRecord#variant.Chat.field.thinking), an [`Option`] of [`bool`], is the frozen thinking switch from the binding's invocation, when the binding declared one.
- [`EffectRecord::ToolCall`]: one call to a bound tool, recorded from an [`Effect::ToolCall`] with all three fields cloned.
  - [`EffectRecord::ToolCall::tool`](EffectRecord#variant.ToolCall.field.tool), a [`ToolId`](crate::tools::ToolId), is the tool's stable identity. It serializes as its `namespace/pack/name` string and is validated when deserialized.
  - [`EffectRecord::ToolCall::alias`](EffectRecord#variant.ToolCall.field.alias), a [`String`], is the prompt-local alias named in the call.
  - [`EffectRecord::ToolCall::args`](EffectRecord#variant.ToolCall.field.args), a [`serde_json::Value`](https://docs.rs/serde_json/latest/serde_json/enum.Value.html), holds the call's arguments.
- [`EffectRecord::UserInput`]: one wait for operator input, recorded from an [`Effect::UserInput`] with both fields cloned.
  - [`EffectRecord::UserInput::execution`](EffectRecord#variant.UserInput.field.execution), a [`String`], is the run's execution identifier, the name given to [`RunContext::new`](crate::RunContext::new).
  - [`EffectRecord::UserInput::section`](EffectRecord#variant.UserInput.field.section), a [`String`], is the name of the section that asked.
- [`EffectRecord::Store`]: one store operation, recorded from an [`Effect::Store`] without its access capability.
  - [`EffectRecord::Store::op`](EffectRecord#variant.Store.field.op), a [`StoreOp`](crate::vfs::StoreOp), is the validated operation. It serializes through its own serde form.
- [`EffectRecord::Timer`]: one sleep, recorded from an [`Effect::Timer`].
  - [`EffectRecord::Timer::seconds`](EffectRecord#variant.Timer.field.seconds), an [`f64`], is the duration in seconds.
- [`EffectRecord::TaskEvents`]: one read of a task's reported history, recorded from an [`Effect::TaskEvents`].
  - [`EffectRecord::TaskEvents::task`](EffectRecord#variant.TaskEvents.field.task), a [`TaskId`](crate::ids::TaskId), is the task whose events were read. It serializes as its dot-separated path string, for example `"0.2"`.
  - [`EffectRecord::TaskEvents::last`](EffectRecord#variant.TaskEvents.field.last), an [`Option`] of [`u32`], is the highest sequence number already seen by the reader, or [`None`] when it had seen none.

## AnswerRecord

[`AnswerRecord`] is an [`EffectAnswer`] as a run log stores it, with one variant per answer kind. The host gets one from [`EffectAnswer::record`], or deserializes one from a stored log. Every variant can also be built directly. The four variants that hold a [`Result`] put the failure's [`Display`](std::fmt::Display) text in [`Err`] as a [`String`].

- [`AnswerRecord::Chat`] holds a [`Result`] of a [`ChatAnswerRecord`] or the [`CompletionError`](crate::model::CompletionError)'s text. It is recorded from an [`EffectAnswer::Chat`].
- [`AnswerRecord::ToolCall`] holds a [`Result`] of a [`ToolAnswerRecord`] or the [`ToolError`](crate::tools::ToolError)'s text. It is recorded from an [`EffectAnswer::ToolCall`]. The text is the model-safe message only, so a cause attached with [`ToolError::with_source`](crate::tools::ToolError::with_source) is not recorded.
- [`AnswerRecord::UserInput`] holds a [`Result`] of an [`InputAnswerRecord`] or the [`InputError`](crate::input::InputError)'s message. It is recorded from an [`EffectAnswer::UserInput`].
- [`AnswerRecord::Store`] holds a [`Result`] of a [`StoreAnswerRecord`] or the [`StoreError`](crate::vfs::StoreError)'s text. It is recorded from an [`EffectAnswer::Store`].
- [`AnswerRecord::Timer`] carries no data. It records that the timer fired.
- [`AnswerRecord::TaskEvents`] holds a [`Vec`] of [`Event`](crate::event::Event) values, a clone of the answered events.
- [`AnswerRecord::Dropped`] carries no data. It records that the host dropped the effect without performing it.

## ChatAnswerRecord

[`ChatAnswerRecord`] is a completed model round as a run log records it: the serving model, the finish reason, and either the reply text or the names of the requested tools. It is the success payload of [`AnswerRecord::Chat`]. The request and response bodies, the metrics, and the tool-call ids and arguments are not recorded. The host usually gets one from [`EffectAnswer::record`]. It can also convert a [`Completion`](crate::model::Completion) reference with [`From`], write a struct literal, since all four fields are public, or deserialize one from a stored log. It serializes as a JSON object with one key per field, named exactly as the fields below.

- [`ChatAnswerRecord::model`], a [`String`], is the model that served the round, as named in the response body, copied from [`Completion::model`](crate::model::Completion::model). It is empty when the body named none.
- [`ChatAnswerRecord::finish_reason`], an [`Option`] of [`String`], is the provider's finish reason when it sent one, such as `"stop"` or `"tool_calls"`. It is [`None`] for a completion built with [`Completion::from_result`](crate::model::Completion::from_result).
- [`ChatAnswerRecord::reply`], an [`Option`] of [`String`], is the reply text when the round's result was [`CompletionResult::Text`](crate::model::CompletionResult::Text). It is [`None`] when the round requested tools.
- [`ChatAnswerRecord::tool_calls`], a [`Vec`] of [`String`], holds the names of the tools requested by the model, in call order, when the round's result was [`CompletionResult::ToolCalls`](crate::model::CompletionResult::ToolCalls). It is empty for a text round.

A [`CompletionResult`](crate::model::CompletionResult) variant that this build does not know records with [`ChatAnswerRecord::reply`] as [`None`] and an empty [`ChatAnswerRecord::tool_calls`].

## ToolAnswerRecord

[`ToolAnswerRecord`] is a tool's own output as a run log records it. It is the success payload of [`AnswerRecord::ToolCall`]. The host usually gets one from [`EffectAnswer::record`], and it can also write a struct literal, since both fields are public, or deserialize one from a stored log. It serializes as a JSON object with one key per field, named exactly as the fields below.

- [`ToolAnswerRecord::text`], a [`String`], is the output text before the run's trust rule applies, so untrusted output appears here without its envelope.
- [`ToolAnswerRecord::trusted`], a [`bool`], is `true` when the tool built its output with [`ToolOutput::trusted`](crate::tools::ToolOutput::trusted), and `false` otherwise.

## InputAnswerRecord

[`InputAnswerRecord`] is the successful outcome of an input wait as a run log records it. It is the success payload of [`AnswerRecord::UserInput`]. The host gets one from [`EffectAnswer::record`], builds one directly, or deserializes one. It serializes as `{"Text":"..."}` or `"Unavailable"`.

- [`InputAnswerRecord::Text`] holds a [`String`], the operator's text, recorded byte-exact from [`InputOutcome::Text`](crate::input::InputOutcome::Text).
- [`InputAnswerRecord::Unavailable`] records that the host had no input to give, from [`InputOutcome::Unavailable`](crate::input::InputOutcome::Unavailable). The fallback sentence that the section received is not stored.

## StoreAnswerRecord

[`StoreAnswerRecord`] is the successful outcome of a store operation as a run log records it, with one variant per [`StoreOutcome`](crate::vfs::StoreOutcome) variant. It is the success payload of [`AnswerRecord::Store`]. The host gets one from [`EffectAnswer::record`], builds one directly, or deserializes one. It uses serde's externally tagged form.

- [`StoreAnswerRecord::Unit`] records a mutating operation that succeeded with no value, such as a write, append, string replace, or delete. It comes from [`StoreOutcome::Unit`](crate::vfs::StoreOutcome::Unit).
- [`StoreAnswerRecord::Text`] holds a [`String`], the text of a read or a numbered read, which may be bounded. It comes from [`StoreOutcome::Text`](crate::vfs::StoreOutcome::Text).
- [`StoreAnswerRecord::Paths`] holds a [`Vec`] of [`String`], the sorted paths that matched a glob. It comes from [`StoreOutcome::Paths`](crate::vfs::StoreOutcome::Paths).
- [`StoreAnswerRecord::Bool`] holds a [`bool`], the result of an existence check. It comes from [`StoreOutcome::Bool`](crate::vfs::StoreOutcome::Bool).

