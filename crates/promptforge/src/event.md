Events reported by a parse and a run, and how a host logs, persists, and reads them back.

An event is a value that says what happened: a section started, a store write succeeded, a model replied, a task ended. This module holds the [`Event`] enum and all its variants, [`DebugMode`], which turns on raw request and response capture, and [`ReplyOrigin`], which tells a chat reply from an inference result. With them a host keeps a complete, ordered log of a parse and a run, writes it out as JSON lines, rebuilds a conversation transcript, follows the task tree, and answers a prompt that reads its own task history back.

# Where this fits

Events reach the host from two places. [`Prompt::parse`](crate::Prompt::parse) returns a [`Vec`] of parse events beside its [`Result`], before any run exists. After that, every [`Step::Pending`](crate::Step::Pending) and [`Step::Done`](crate::Step::Done) from [`Run::step`](crate::Run::step) holds a batch in [`Step::Pending::events`](crate::Step#variant.Pending.field.events) or [`Step::Done::events`](crate::Step#variant.Done.field.events). The host appends each batch to its log in order. [`Step::Done`](crate::Step::Done) holds the last batch, which includes the run's end boundary.

The log feeds back into the run in one place. When a prompt reads a task's history, the run issues an [`Effect::TaskEvents`](crate::effect::Effect::TaskEvents), and the host answers with an [`EffectAnswer::TaskEvents`](crate::effect::EffectAnswer::TaskEvents) that holds events filtered from its own log.

Events never steer the run. A host learns the outcome from [`Run::decided`](crate::Run::decided) and the [`RunResult`](crate::RunResult) in [`Step::Done`](crate::Step::Done), never by watching for an end event.

# Logging a parse and a run

This program parses a prompt, logs the parse events, runs the prompt, logs the run's events after them, and writes the whole log as JSON lines.

````
use std::collections::HashSet;
use std::sync::Arc;

use promptforge::effect::{Effect, EffectAnswer};
use promptforge::event::Event;
use promptforge::timestamp::Timestamp;
use promptforge::vfs::perform_store_op;
use promptforge::{Prompt, Run, RunContext, Step};

let source = concat!(
    "---\n",
    "name: notes\n",
    "description: keeps a note\n",
    "promptforge: 0\n",
    "---\n",
    "\n",
    "# Notes\n",
    "\n",
    "## Save\n",
    "\n",
    "```lua\n",
    "store.write('todo.md', 'ship it')\n",
    "return store.read('todo.md')\n",
    "```\n",
);
let (parsed, parse_events) = Prompt::parse(source, "notes-1");
let prompt = Arc::new(parsed?);
let mut log: Vec<Event> = parse_events;
let parse_count = log.len();

let ctx = RunContext::new("notes-1", 7, Timestamp::UNIX_EPOCH)
    .provenance_start(u32::try_from(parse_count)?);
let mut run = Run::new(prompt, "", ctx);
loop {
    match run.step() {
        Step::Pending { effects, events } => {
            log.extend(events);
            for (id, _provenance, effect) in effects {
                let answer = match effect {
                    Effect::Store { access, op } => EffectAnswer::Store(perform_store_op(&access, op)),
                    _ => EffectAnswer::Dropped,
                };
                run.resume(id, answer);
            }
        }
        Step::Done { events, .. } => {
            log.extend(events);
            break;
        }
    }
}

assert!(matches!(log.first(), Some(Event::ParseStarted { .. })));
assert!(matches!(log.get(parse_count - 1), Some(Event::ParseSucceeded { .. })));
let Some(Event::RunStarted { section, provenance, .. }) = log.get(parse_count) else {
    panic!("the run's events open with RunStarted");
};
assert_eq!(section, "Notes");
assert_eq!(provenance.seq, u32::try_from(parse_count)?);
assert!(log.iter().any(|event| matches!(event, Event::RunSucceeded { .. })));

let keys: HashSet<_> = log.iter().map(|event| event.provenance()).collect();
assert_eq!(keys.len(), log.len());

let lines = log
    .iter()
    .map(|event| serde_json::to_string(event))
    .collect::<Result<Vec<_>, _>>()?;
for (line, event) in lines.iter().zip(&log) {
    assert!(!line.contains('\n'));
    let back: Event = serde_json::from_str(line)?;
    assert_eq!(&back, event);
}
assert!(lines.iter().any(|line| line.starts_with(r#"{"kind":"store_write_succeeded""#)));
# Ok::<(), Box<dyn std::error::Error>>(())
````

Here is what each part does.

1. **Log the parse events first.** [`Prompt::parse`](crate::Prompt::parse) returns its events whether the parse succeeds or fails. They always start with [`Event::ParseStarted`] and end with [`Event::ParseSucceeded`] when the parse returns a [`Prompt`](crate::Prompt), or [`Event::ParseFailed`] when it returns a [`ParseError`](crate::ParseError). In between comes one [`Event::LuaCompilationStarted`] and [`Event::LuaCompilationSucceeded`] pair per Lua block. When a block fails to compile, the parse reports [`Event::LuaCompilationFailed`], fails with [`ParseErrorKind::Lua`](crate::ParseErrorKind::Lua), and ends with [`Event::ParseFailed`]. Parse events never contain Lua source text.
2. **Continue the sequence.** Parse events are stamped under task `0` with sequence numbers from zero. A run's root task also counts from zero by default, so a log that holds both would repeat keys. [`RunContext::provenance_start`](crate::RunContext::provenance_start) takes the number of logged parse events as a [`u32`], and the run's root task continues from there. The example checks that [`Event::RunStarted`] holds exactly that sequence number.
3. **Append every batch.** The loop is the host loop from the crate page, and every batch goes into the same log in order. [`Event::RunStarted`] opens the first step's events. [`Event::RunSucceeded`] or [`Event::RunFailed`] arrives in a later step, at the latest in [`Step::Done`](crate::Step::Done). The run boundaries report under the prompt's H1 title, here `"Notes"`.
4. **Check the keys.** [`Event::provenance`] returns each event's key without a match on its variant. The example collects the keys into a set to show that no key repeats across the parse and the run.
5. **Write JSON lines.** Each event serializes with serde to one line with no newline, and reading the line back gives an equal event. The store write's line starts with `{"kind":"store_write_succeeded"`.

# Coordinates and provenance

Every variant starts with the same three fields, its coordinates. [`Event::execution`], [`Event::section`], and [`Event::provenance`] read them from any event, so a host fills a log record's columns the same way for every kind.

- **Execution** is the caller-chosen run identifier: the name given to [`RunContext::new`](crate::RunContext::new) for run events, and the `execution` argument of [`Prompt::parse`](crate::Prompt::parse) for parse events.
- **Section** is the reporting scope. For most events it is the H2 heading text of the section that reported it, or an agent's name. Parse events use `"Prompt"`, and [`Event::RunStarted`], [`Event::RunSucceeded`], and [`Event::RunFailed`] use the prompt's H1 title, the value of [`Prompt::title`](crate::Prompt::title). It is prompt-authored text.
- **Provenance** is the replay key, a [`Provenance`](crate::ids::Provenance). Its [`Provenance::task`](crate::ids::Provenance::task) is the nearest enclosing task, a [`TaskId`](crate::ids::TaskId), and its [`Provenance::seq`](crate::ids::Provenance::seq) is the event's position within that task, a [`u32`].

The main walk and the parse are task `0`. A `call` child reports under its caller's task, and a spawned chain gets its own task. Each task's sequence counts from zero by default and is dense. An issued effect's provenance draws from the same per-task counter as that task's events, so the events and effects of one task share one dense sequence, and a host can interleave them in one ordered log.

[`RunContext::provenance_start`](crate::RunContext::provenance_start) seeds only the root task. Spawned tasks still count from zero, and their keys stay unique because their task ids differ.

# Events as JSON lines

Every variant serializes with serde to a single-line JSON object tagged by a `"kind"` field. The kind is the variant name in snake case, such as `section_started`, `store_write_succeeded`, or `assistant_reply`. The object holds `"kind"` first, then the three coordinates, then the variant's payload fields in declaration order. A [`Provenance`](crate::ids::Provenance) serializes as `{"task":"<task id>","seq":<n>}`, and a [`TaskId`](crate::ids::TaskId) as its dotted string, such as `"0.2"`. A serialized event never contains a newline, and deserializing the line gives back an equal event. So a host persists events one JSON line each and reads them back with serde.

Here are three lines: a section start on the root task, a store write on the spawned task `0.2`, and a task notice with its payload.

````json
{"kind":"section_started","execution":"run-1","section":"Gather","provenance":{"task":"0","seq":4}}
{"kind":"store_write_succeeded","execution":"run-1","section":"Gather","provenance":{"task":"0.2","seq":9}}
{"kind":"task_notice","execution":"run-1","section":"Gather","provenance":{"task":"0","seq":10},"turn":3,"task":"0.1","text":"Task id=0.1 (## Worker) completed: done"}
````

Every variant's fields are public, so a host can also build an event with struct-literal syntax, for example to write a test fixture or to rebuild events while replaying a log:

````
use promptforge::event::Event;
use promptforge::ids::Provenance;

let event = Event::SectionStarted {
    execution: "run-1".to_owned(),
    section: "Gather".to_owned(),
    provenance: Provenance { task: "0".parse()?, seq: 4 },
};
assert_eq!(
    serde_json::to_string(&event)?,
    r#"{"kind":"section_started","execution":"run-1","section":"Gather","provenance":{"task":"0","seq":4}}"#
);
assert_eq!(event.provenance().seq, 4);
# Ok::<(), Box<dyn std::error::Error>>(())
````

[`Event`] is `#[non_exhaustive]`, so a `match` on it keeps a wildcard arm for kinds a later engine adds. It is [`Send`] and [`Sync`], so events move and are shared across threads freely. It has no [`Default`], [`Display`](std::fmt::Display), or [`FromStr`](std::str::FromStr) impl, so serde is the one text form.

# Answering a task-history read

A prompt can read a task's own history back. The run then issues an [`Effect::TaskEvents`](crate::effect::Effect::TaskEvents) with two fields. [`Effect::TaskEvents::task`](crate::effect::Effect#variant.TaskEvents.field.task) is the [`TaskId`](crate::ids::TaskId) to read, and [`Effect::TaskEvents::last`](crate::effect::Effect#variant.TaskEvents.field.last) is an [`Option`] of [`u32`]. The host answers with [`EffectAnswer::TaskEvents`](crate::effect::EffectAnswer::TaskEvents), holding the events from its own log whose [`Provenance::task`](crate::ids::Provenance::task) equals the task and, when the last value is [`Some`], whose [`Provenance::seq`](crate::ids::Provenance::seq) is greater than it, in log order. An empty [`Vec`] is a valid answer.

````
use promptforge::event::Event;
use promptforge::ids::{Provenance, TaskId};

fn task_events(log: &[Event], task: &TaskId, last: Option<u32>) -> Vec<Event> {
    log.iter()
        .filter(|event| {
            let provenance = event.provenance();
            provenance.task == *task && last.map_or(true, |last| provenance.seq > last)
        })
        .cloned()
        .collect()
}

let root: TaskId = "0".parse()?;
let worker: TaskId = "0.1".parse()?;
let log = vec![
    Event::SectionStarted {
        execution: "run-1".to_owned(),
        section: "Gather".to_owned(),
        provenance: Provenance { task: root, seq: 0 },
    },
    Event::StoreWriteSucceeded {
        execution: "run-1".to_owned(),
        section: "Worker".to_owned(),
        provenance: Provenance { task: worker.clone(), seq: 0 },
    },
    Event::StoreReadSucceeded {
        execution: "run-1".to_owned(),
        section: "Worker".to_owned(),
        provenance: Provenance { task: worker.clone(), seq: 1 },
    },
];

assert_eq!(task_events(&log, &worker, None).len(), 2);
assert_eq!(task_events(&log, &worker, Some(0)), vec![log[2].clone()]);
assert!(task_events(&log, &worker, Some(1)).is_empty());
# Ok::<(), Box<dyn std::error::Error>>(())
````

# Building a transcript

A host builds a conversation transcript from six content events: [`Event::Thinking`], [`Event::AssistantReply`], [`Event::AssistantToolCalls`], [`Event::ToolResult`], [`Event::UserInput`], and [`Event::TaskNotice`]. The lifecycle events around them say how each model round and tool call went.

Each model round reports its events in this order:

1. [`Event::ModelTurnCompleted`], once the answer to the round's [`Effect::Chat`](crate::effect::Effect::Chat) is applied. A round that fails reports [`Event::ModelTurnFailed`] instead, and the failure surfaces through the run's error handling or the calling Lua code.
2. [`Event::ModelMetadataDegraded`], once for each metadata section of the response that was present but malformed, and once when the response named no model. The turn still succeeds, so a host can show it as a backend-quality warning.
3. [`Event::Thinking`], only when the response's reasoning content is present and non-empty.
4. [`Event::ModelTurnTruncated`], when the round produced text and its finish reason is `"length"`. A host flags the reply that follows as cut off by the model's length limit.
5. [`Event::AssistantReply`] when the round's outcome is text, or [`Event::AssistantToolCalls`] when the outcome is tool calls. [`Event::AssistantToolCalls`] is reported only in the chat arm, and it lists the requested calls before any of them run.

Each dispatched tool call then reports [`Event::ToolCallSucceeded`] or [`Event::ToolCallFailed`], followed by an [`Event::ToolResult`] with the same turn and the call's id. For a model-issued call, the [`Event::ToolResult`] arrives whether the tool succeeded or failed, because a failure's message is nonce-wrapped and delivered to the model as the result. For a script-issued call, it arrives only on success, and a failure propagates to the Lua caller. The exception is a model-issued call to a Lua-local tool whose handler raises or returns an unsupported value: that call reports [`Event::ToolCallFailed`] and no [`Event::ToolResult`], and the failure propagates to the caller of `models.loop` and ends the loop unless the author catches it.

[`Event::ToolResult::trusted`](Event#variant.ToolResult.field.trusted) says whether the dispatch treated the tool as trusted. The host decides that when it answers a tool call with [`ToolOutput::trusted`](crate::tools::ToolOutput::trusted) or [`ToolOutput::untrusted`](crate::tools::ToolOutput::untrusted). Untrusted output is nonce-wrapped before it is recorded, so for an untrusted tool [`Event::ToolResult::content`](Event#variant.ToolResult.field.content) already holds the wrapped text.

**Chat replies and inference results.** [`Event::AssistantReply::origin`](Event#variant.AssistantReply.field.origin) is a [`ReplyOrigin`] that names the path that produced the reply. [`ReplyOrigin::Chat`] marks a user-facing chat turn, and [`ReplyOrigin::Infer`] marks a programmatic `models.infer` round. A host appends chat replies to the visible conversation and can log inference replies without showing them as chat turns. A log line written before the origin field existed has no `"origin"` key, and it reads back as [`ReplyOrigin::Chat`]:

````
use promptforge::event::{Event, ReplyOrigin};

let old_line = concat!(
    r#"{"kind":"assistant_reply","execution":"run-1","section":"Gather","#,
    r#""provenance":{"task":"0","seq":5},"turn":1,"text":"hello","#,
    r#""finish_reason":"stop","model":"example-model","metrics":null}"#,
);
let reply: Event = serde_json::from_str(old_line)?;
let Event::AssistantReply { text, origin, .. } = reply else {
    panic!("the line is an assistant reply");
};
assert_eq!(text, "hello");
assert_eq!(origin, ReplyOrigin::Chat);
assert_eq!(serde_json::to_string(&ReplyOrigin::Infer)?, r#""infer""#);
# Ok::<(), Box<dyn std::error::Error>>(())
````

**Operator input.** The run reports [`Event::UserInputWaitStarted`] when a section begins waiting on operator input, before it issues the [`Effect::UserInput`](crate::effect::Effect::UserInput). A host can use it to show a "waiting for input" indicator. The host answers the effect with an [`EffectAnswer::UserInput`](crate::effect::EffectAnswer::UserInput). When the answer resolves to [`InputOutcome::Text`](crate::input::InputOutcome::Text), the run reports [`Event::UserInput`] with the operator's reply, byte-exact. The unavailable fallback, [`InputOutcome::Unavailable`](crate::input::InputOutcome::Unavailable), reports no [`Event::UserInput`].

# Following tasks

A task is a chain started by `tasks.spawn`, by each arm of `fanout`, or by the model's task tool. [`Event::TaskStarted`] reports each one. Its payload names the new task's id, the section where its chain starts, the principal that started it, and the full spawn seeds. Together they are enough to start the same chain again under the same id. [`Event::TaskStarted`] is reported under the spawning section and on the spawner's task sequence, not the new task's. The new task's own events hold the new id in [`Provenance::task`](crate::ids::Provenance::task). Those two facts are enough to rebuild the task tree from a log.

Every started task gets exactly one terminal event: [`Event::TaskSucceeded`], [`Event::TaskFailed`], [`Event::TaskCancelled`], or [`Event::TaskAbandoned`]. Each terminal event is reported under the task's target section and stamped with the task's own provenance.

- [`Event::TaskCancelled`] means the owner stopped the task on purpose, for example through `tasks.cancel`. A repeated cancel reports nothing more. A cancelled task backed by a pending request instead of a chain still reports it.
- [`Event::TaskAbandoned`] means the task's owner chain ended while the task was still live. Its reason is an [`AbandonReason`](crate::ids::AbandonReason).
- When the run ends, the engine settles every live task exactly once by abandoning it, before it reports [`Event::RunSucceeded`] or [`Event::RunFailed`]. Still-live tasks get [`AbandonReason::RunTerminated`](crate::ids::AbandonReason::RunTerminated), and nested tasks get [`AbandonReason::OwnerAborted`](crate::ids::AbandonReason::OwnerAborted). So the contract holds even for tasks stranded by a host cancel or a fatal answer.
- The contract covers the tasks that have an [`Event::TaskStarted`]. Internal timer slots are a wait's own bookkeeping and never an author-visible task, and they end without reporting [`Event::TaskCancelled`] or [`Event::TaskAbandoned`].

**Task notices.** When a task started by the model ends, the run reports an [`Event::TaskNotice`] under the owner's section. Its text is the engine's own sentence telling the model how the task ended, in one of four shapes: `Task id=<task> (## <target>) completed: <wrapped text>`, `... failed: <error>`, `... was canceled: the author cancelled it`, or `... was abandoned: <reason phrase>`. For example, `"Task id=0.1 (## Worker) completed: done"`. A completed task's final text is embedded nonce-wrapped as untrusted, and the rest of the sentence is the engine's. A task started by the author gets no notice.

# Lifecycle boundaries

The remaining lifecycle events mark operational boundaries. Apart from [`Event::Lua`] and [`Event::ModelMetadataDegraded`], they hold only their coordinates.

- **Sections.** [`Event::SectionStarted`] and [`Event::SectionFinished`] mark the start and the successful end of a top-level section, reported under its H2 heading text. There is no section-failed variant. Chains still suspended when the run ends are torn down without an [`Event::SectionFinished`].
- **The run.** At most one of [`Event::RunSucceeded`] and [`Event::RunFailed`] is reported, because a second decision keeps the first.
- **Section VM phases.** Each section's Lua VM reports four phases. Compilation reports [`Event::LuaCompilationStarted`], [`Event::LuaCompilationSucceeded`], and [`Event::LuaCompilationFailed`]. The shared-program load runs the prompt's `lua shared` library in the section VM and reports [`Event::LuaSharedLoadStarted`], [`Event::LuaSharedLoadSucceeded`], and [`Event::LuaSharedLoadFailed`]. Chunk execution reports [`Event::LuaChunkStarted`], [`Event::LuaChunkSucceeded`], and [`Event::LuaChunkFailed`]. Teardown reports [`Event::LuaTeardownStarted`] and [`Event::LuaTeardownSucceeded`], and there is no teardown-failed variant.
- **Tool scope.** [`Event::ToolScopeValidationStarted`], [`Event::ToolScopeValidationSucceeded`], and [`Event::ToolScopeValidationFailed`] report the check the engine runs when it builds a model round's advertised tool scope.
- **The store.** Every harness-mediated store operation reports a paired succeeded or failed event, for fourteen variants from [`Event::StoreWriteSucceeded`] to [`Event::StoreGlobFailed`]. They hold only the coordinates, so paths, contents, and error details stay out of the log. Together they form a store audit trail.
- **Author checkpoints.** A prompt author's Lua `log(message)` call is reported as [`Event::Lua`], and [`Event::Lua::message`](Event#variant.Lua.field.message) holds the text verbatim. It is the one author-controlled checkpoint in the event stream. The message is checked against a byte quota first, and an oversize message raises a Lua error instead of producing an event.

Error detail never appears in the lifecycle events. A parse error is in the [`ParseError`](crate::ParseError), a run error is in the [`RunResult`](crate::RunResult) of [`Step::Done`](crate::Step::Done), and store failures have no error detail at all.

# Debug capture

By default a run does not put raw model bodies in the event stream. [`RunContext::report_debug`](crate::RunContext::report_debug) with [`DebugMode::On`] turns that on: every model round then reports its raw request body as an [`Event::Request`] and its response body as an [`Event::Response`], both before that round's [`Event::ModelTurnCompleted`]. A context built without that call uses [`DebugMode::Off`], which reports neither event and never clones a body.

The same bodies already travel in the [`Effect::Chat`](crate::effect::Effect::Chat) and its [`EffectAnswer::Chat`](crate::effect::EffectAnswer::Chat), so a host that logs effects has them either way. Turn this on when you want the bodies inside the event log itself.

````
use promptforge::RunContext;
use promptforge::event::DebugMode;
use promptforge::timestamp::Timestamp;

assert_eq!(DebugMode::default(), DebugMode::Off);
let ctx = RunContext::new("debug-run", 7, Timestamp::UNIX_EPOCH).report_debug(DebugMode::On);
assert_eq!(ctx.name(), "debug-run");
````

# Untrusted content

The content variants hold text written by a model, a tool, or a user, and the debug variants hold raw, unredacted request bodies that include the full prompt. A host that persists or forwards events must treat all of that content as untrusted. These fields hold it:

- [`Event::Thinking::text`](Event#variant.Thinking.field.text), [`Event::AssistantReply::text`](Event#variant.AssistantReply.field.text), and [`Event::AssistantToolCalls::calls`](Event#variant.AssistantToolCalls.field.calls), written by the model.
- [`Event::ToolResult::content`](Event#variant.ToolResult.field.content), written by a tool, unless [`Event::ToolResult::trusted`](Event#variant.ToolResult.field.trusted) is `true`.
- [`Event::UserInput::text`](Event#variant.UserInput.field.text), written by the operator.
- [`Event::TaskNote::text`](Event#variant.TaskNote.field.text), and the spawn seeds in [`Event::TaskStarted`].
- [`Event::Request::body`](Event#variant.Request.field.body) and [`Event::Response::body`](Event#variant.Response.field.body), the raw bodies.
- [`Event::ModelMetadataDegraded::message`](Event#variant.ModelMetadataDegraded.field.message), the one lifecycle payload that may quote values from a backend's response.

The coordinates come from the host and the prompt author. The execution name is the host's, and the section is prompt-authored heading text.

# Variants the engine does not emit

Eight variants are declared but not currently emitted by the engine. A host should accept them when it reads a log, but it will not receive them from the current engine.

- [`Event::LuaReplyBindingStarted`], [`Event::LuaReplyBindingSucceeded`], and [`Event::LuaReplyBindingFailed`] are declared in the vocabulary, and the engine has no place that emits them.
- [`Event::ModelCatalogValidationStarted`], [`Event::ModelCatalogValidationSucceeded`], and [`Event::ModelCatalogValidationFailed`] are declared in the vocabulary, and the engine has no place that emits them.
- [`Event::TaskNote`] is documented as reported when a task sets a note through `tasks.note`. The engine's note handler only stores the note on the chain and does not emit the event.
- [`Event::TaskResumed`] is reserved. Nothing emits it until task resume lands, and it exists so the log schema has the kind from its first version.

# Reference

This part covers the three public types in the module: [`Event`], [`DebugMode`], and [`ReplyOrigin`].

## Event

[`Event`] is one thing that happened during a parse or a run, reported to the host as a value. The host receives events from [`Prompt::parse`](crate::Prompt::parse) and from every [`Step`](crate::Step). It builds them itself only when it deserializes its log, writes a test fixture, or answers an [`Effect::TaskEvents`](crate::effect::Effect::TaskEvents). Its serde shape, its thread safety, its `#[non_exhaustive]` marking, and its missing text impls are covered in [Events as JSON lines](#events-as-json-lines).

Three methods read the coordinates from any variant. Each takes `&self`, has no arguments, cannot fail, and is `#[must_use]`.

- [`Event::execution`] returns the execution coordinate, the caller-chosen run identifier, as a [`&str`](str).
- [`Event::section`] returns the section coordinate, the reporting scope, as a [`&str`](str).
- [`Event::provenance`] returns a reference to the event's [`Provenance`](crate::ids::Provenance), the replay key. A host writes a record's task and sequence columns from it alone.

Every variant's first three fields are its coordinates, with the same meaning in every variant: the execution field is a [`String`], the section field is a [`String`], and the provenance field is a [`Provenance`](crate::ids::Provenance). [Coordinates and provenance](#coordinates-and-provenance) explains what each holds. Each family below says which section and which task its events report under. Each entry links every field of its variant and describes the payload fields.

The kind names in the entries are the serialized `"kind"` values.

### Parse events

[`Prompt::parse`](crate::Prompt::parse) reports these, before any run exists. Their section is always `"Prompt"`, and their provenance is task `0` with a sequence that counts from zero within the parse. The host logs them and needs no other action.

- [`Event::ParseStarted`], kind `parse_started`: parsing began. It is always the first parse event. Fields: [`Event::ParseStarted::execution`](Event#variant.ParseStarted.field.execution), [`Event::ParseStarted::section`](Event#variant.ParseStarted.field.section), and [`Event::ParseStarted::provenance`](Event#variant.ParseStarted.field.provenance).
- [`Event::ParseSucceeded`], kind `parse_succeeded`: parsing and parse-time compilation finished, and the parse returned a [`Prompt`](crate::Prompt). It is the last parse event. Fields: [`Event::ParseSucceeded::execution`](Event#variant.ParseSucceeded.field.execution), [`Event::ParseSucceeded::section`](Event#variant.ParseSucceeded.field.section), and [`Event::ParseSucceeded::provenance`](Event#variant.ParseSucceeded.field.provenance).
- [`Event::ParseFailed`], kind `parse_failed`: parsing or parse-time compilation failed, and the parse returned a [`ParseError`](crate::ParseError). It is the last parse event. The error itself is in the [`ParseError`](crate::ParseError). Fields: [`Event::ParseFailed::execution`](Event#variant.ParseFailed.field.execution), [`Event::ParseFailed::section`](Event#variant.ParseFailed.field.section), and [`Event::ParseFailed::provenance`](Event#variant.ParseFailed.field.provenance).

### Run events

These mark the start and end of the run. Their section is the prompt's H1 title, the value of [`Prompt::title`](crate::Prompt::title), and their provenance is task `0`.

- [`Event::RunStarted`], kind `run_started`: the run passed its version gate and began. It opens the first step's events. Its sequence is the value given to [`RunContext::provenance_start`](crate::RunContext::provenance_start), `0` by default, because it is the root task's first stamp. Log it as the run's opening boundary. Fields: [`Event::RunStarted::execution`](Event#variant.RunStarted.field.execution), [`Event::RunStarted::section`](Event#variant.RunStarted.field.section), and [`Event::RunStarted::provenance`](Event#variant.RunStarted.field.provenance).
- [`Event::RunSucceeded`], kind `run_succeeded`: the run returned a value. It is reported after every task's terminal event, in a later step's events, at the latest in [`Step::Done`](crate::Step::Done). To learn whether the outcome is settled, ask [`Run::decided`](crate::Run::decided) instead of watching for this event. Fields: [`Event::RunSucceeded::execution`](Event#variant.RunSucceeded.field.execution), [`Event::RunSucceeded::section`](Event#variant.RunSucceeded.field.section), and [`Event::RunSucceeded::provenance`](Event#variant.RunSucceeded.field.provenance).
- [`Event::RunFailed`], kind `run_failed`: the run returned an error, including a host cancel or a fatal answer. It is reported after every task's terminal event. The error is in the [`RunResult`](crate::RunResult) of [`Step::Done`](crate::Step::Done). Fields: [`Event::RunFailed::execution`](Event#variant.RunFailed.field.execution), [`Event::RunFailed::section`](Event#variant.RunFailed.field.section), and [`Event::RunFailed::provenance`](Event#variant.RunFailed.field.provenance).

### Section events

These mark top-level sections. Their section is the section's H2 heading text, or an agent's name, and their provenance is the task running the section.

- [`Event::SectionStarted`], kind `section_started`: a top-level section began. Log it as the section's opening boundary. Fields: [`Event::SectionStarted::execution`](Event#variant.SectionStarted.field.execution), [`Event::SectionStarted::section`](Event#variant.SectionStarted.field.section), and [`Event::SectionStarted::provenance`](Event#variant.SectionStarted.field.provenance).
- [`Event::SectionFinished`], kind `section_finished`: a top-level section completed successfully. Log it as the section's closing boundary. A section that fails, or a chain still suspended when the run ends, reports no [`Event::SectionFinished`]. Fields: [`Event::SectionFinished::execution`](Event#variant.SectionFinished.field.execution), [`Event::SectionFinished::section`](Event#variant.SectionFinished.field.section), and [`Event::SectionFinished::provenance`](Event#variant.SectionFinished.field.provenance).

### Section VM events

A section's Lua VM reports these as it moves through its phases. Their section is the section whose VM reports, and their provenance is the reporting task. At parse time, the compilation events report under the parse's coordinates instead: the parse's execution argument, the scope whose Lua source is compiled, and task `0`. The host logs them.

- [`Event::LuaCompilationStarted`], kind `lua_compilation_started`: Lua source compilation began, at parse time or wherever Lua source is compiled. Fields: [`Event::LuaCompilationStarted::execution`](Event#variant.LuaCompilationStarted.field.execution), [`Event::LuaCompilationStarted::section`](Event#variant.LuaCompilationStarted.field.section), and [`Event::LuaCompilationStarted::provenance`](Event#variant.LuaCompilationStarted.field.provenance).
- [`Event::LuaCompilationSucceeded`], kind `lua_compilation_succeeded`: Lua source compiled. Fields: [`Event::LuaCompilationSucceeded::execution`](Event#variant.LuaCompilationSucceeded.field.execution), [`Event::LuaCompilationSucceeded::section`](Event#variant.LuaCompilationSucceeded.field.section), and [`Event::LuaCompilationSucceeded::provenance`](Event#variant.LuaCompilationSucceeded.field.provenance).
- [`Event::LuaCompilationFailed`], kind `lua_compilation_failed`: Lua source failed to compile. At parse time the parse then fails with [`ParseErrorKind::Lua`](crate::ParseErrorKind::Lua). The error detail is in the returned error. Fields: [`Event::LuaCompilationFailed::execution`](Event#variant.LuaCompilationFailed.field.execution), [`Event::LuaCompilationFailed::section`](Event#variant.LuaCompilationFailed.field.section), and [`Event::LuaCompilationFailed::provenance`](Event#variant.LuaCompilationFailed.field.provenance).
- [`Event::LuaSharedLoadStarted`], kind `lua_shared_load_started`: the section VM began loading and running the prompt's `lua shared` library. Fields: [`Event::LuaSharedLoadStarted::execution`](Event#variant.LuaSharedLoadStarted.field.execution), [`Event::LuaSharedLoadStarted::section`](Event#variant.LuaSharedLoadStarted.field.section), and [`Event::LuaSharedLoadStarted::provenance`](Event#variant.LuaSharedLoadStarted.field.provenance).
- [`Event::LuaSharedLoadSucceeded`], kind `lua_shared_load_succeeded`: the section VM loaded and ran the shared library. Fields: [`Event::LuaSharedLoadSucceeded::execution`](Event#variant.LuaSharedLoadSucceeded.field.execution), [`Event::LuaSharedLoadSucceeded::section`](Event#variant.LuaSharedLoadSucceeded.field.section), and [`Event::LuaSharedLoadSucceeded::provenance`](Event#variant.LuaSharedLoadSucceeded.field.provenance).
- [`Event::LuaSharedLoadFailed`], kind `lua_shared_load_failed`: the section VM failed to load or run the shared library. The failure surfaces as a run error. Fields: [`Event::LuaSharedLoadFailed::execution`](Event#variant.LuaSharedLoadFailed.field.execution), [`Event::LuaSharedLoadFailed::section`](Event#variant.LuaSharedLoadFailed.field.section), and [`Event::LuaSharedLoadFailed::provenance`](Event#variant.LuaSharedLoadFailed.field.provenance).
- [`Event::LuaChunkStarted`], kind `lua_chunk_started`: the section VM began running a Lua chunk. Fields: [`Event::LuaChunkStarted::execution`](Event#variant.LuaChunkStarted.field.execution), [`Event::LuaChunkStarted::section`](Event#variant.LuaChunkStarted.field.section), and [`Event::LuaChunkStarted::provenance`](Event#variant.LuaChunkStarted.field.provenance).
- [`Event::LuaChunkSucceeded`], kind `lua_chunk_succeeded`: the section VM ran a Lua chunk. Fields: [`Event::LuaChunkSucceeded::execution`](Event#variant.LuaChunkSucceeded.field.execution), [`Event::LuaChunkSucceeded::section`](Event#variant.LuaChunkSucceeded.field.section), and [`Event::LuaChunkSucceeded::provenance`](Event#variant.LuaChunkSucceeded.field.provenance).
- [`Event::LuaChunkFailed`], kind `lua_chunk_failed`: the section VM failed to run a Lua chunk. The error surfaces through the run's result. Fields: [`Event::LuaChunkFailed::execution`](Event#variant.LuaChunkFailed.field.execution), [`Event::LuaChunkFailed::section`](Event#variant.LuaChunkFailed.field.section), and [`Event::LuaChunkFailed::provenance`](Event#variant.LuaChunkFailed.field.provenance).
- [`Event::LuaReplyBindingStarted`], kind `lua_reply_binding_started`: the section VM began binding a model reply. Declared but not currently emitted. Fields: [`Event::LuaReplyBindingStarted::execution`](Event#variant.LuaReplyBindingStarted.field.execution), [`Event::LuaReplyBindingStarted::section`](Event#variant.LuaReplyBindingStarted.field.section), and [`Event::LuaReplyBindingStarted::provenance`](Event#variant.LuaReplyBindingStarted.field.provenance).
- [`Event::LuaReplyBindingSucceeded`], kind `lua_reply_binding_succeeded`: the section VM bound a model reply. Declared but not currently emitted. Fields: [`Event::LuaReplyBindingSucceeded::execution`](Event#variant.LuaReplyBindingSucceeded.field.execution), [`Event::LuaReplyBindingSucceeded::section`](Event#variant.LuaReplyBindingSucceeded.field.section), and [`Event::LuaReplyBindingSucceeded::provenance`](Event#variant.LuaReplyBindingSucceeded.field.provenance).
- [`Event::LuaReplyBindingFailed`], kind `lua_reply_binding_failed`: the section VM failed to bind a model reply. Declared but not currently emitted. Fields: [`Event::LuaReplyBindingFailed::execution`](Event#variant.LuaReplyBindingFailed.field.execution), [`Event::LuaReplyBindingFailed::section`](Event#variant.LuaReplyBindingFailed.field.section), and [`Event::LuaReplyBindingFailed::provenance`](Event#variant.LuaReplyBindingFailed.field.provenance).
- [`Event::LuaTeardownStarted`], kind `lua_teardown_started`: the section VM began teardown. Fields: [`Event::LuaTeardownStarted::execution`](Event#variant.LuaTeardownStarted.field.execution), [`Event::LuaTeardownStarted::section`](Event#variant.LuaTeardownStarted.field.section), and [`Event::LuaTeardownStarted::provenance`](Event#variant.LuaTeardownStarted.field.provenance).
- [`Event::LuaTeardownSucceeded`], kind `lua_teardown_succeeded`: the section VM finished teardown. There is no teardown-failed variant. Fields: [`Event::LuaTeardownSucceeded::execution`](Event#variant.LuaTeardownSucceeded.field.execution), [`Event::LuaTeardownSucceeded::section`](Event#variant.LuaTeardownSucceeded.field.section), and [`Event::LuaTeardownSucceeded::provenance`](Event#variant.LuaTeardownSucceeded.field.provenance).

### Author checkpoint

- [`Event::Lua`], kind `lua`: a prompt author's Lua `log(message)` call, after it passed the byte quota. A host logs it as the author's trace line. Its section is the section whose Lua called `log`, and its provenance is the calling task. Prompt authors must never put arguments, replies, tool data, credentials, paths, or store contents in the message. Fields: [`Event::Lua::execution`](Event#variant.Lua.field.execution), [`Event::Lua::section`](Event#variant.Lua.field.section), [`Event::Lua::provenance`](Event#variant.Lua.field.provenance), and:
  - [`Event::Lua::message`](Event#variant.Lua.field.message), a [`String`], is the author's checkpoint text, verbatim.

### Model turn events

These report each model round, answered through [`Effect::Chat`](crate::effect::Effect::Chat), and what it produced. Their section is the section that issued the model call, and their provenance is the calling task. [Building a transcript](#building-a-transcript) gives their order within one round.

- [`Event::ModelTurnCompleted`], kind `model_turn_completed`: a model round trip succeeded. It is reported once the round's answer is applied, and with [`DebugMode::On`] it follows the round's [`Event::Request`] and [`Event::Response`]. Fields: [`Event::ModelTurnCompleted::execution`](Event#variant.ModelTurnCompleted.field.execution), [`Event::ModelTurnCompleted::section`](Event#variant.ModelTurnCompleted.field.section), and [`Event::ModelTurnCompleted::provenance`](Event#variant.ModelTurnCompleted.field.provenance).
- [`Event::ModelTurnFailed`], kind `model_turn_failed`: a model round trip returned an error. The failure itself surfaces through the run's error handling or the calling Lua code. Fields: [`Event::ModelTurnFailed::execution`](Event#variant.ModelTurnFailed.field.execution), [`Event::ModelTurnFailed::section`](Event#variant.ModelTurnFailed.field.section), and [`Event::ModelTurnFailed::provenance`](Event#variant.ModelTurnFailed.field.provenance).
- [`Event::ModelTurnTruncated`], kind `model_turn_truncated`: the round produced text and its finish reason was `"length"`, so the model hit its length limit. It is reported just before that round's [`Event::AssistantReply`]. A host may flag the reply as cut off. Fields: [`Event::ModelTurnTruncated::execution`](Event#variant.ModelTurnTruncated.field.execution), [`Event::ModelTurnTruncated::section`](Event#variant.ModelTurnTruncated.field.section), and [`Event::ModelTurnTruncated::provenance`](Event#variant.ModelTurnTruncated.field.provenance).
- [`Event::ModelMetadataDegraded`], kind `model_metadata_degraded`: one metadata section of a completed response was present but malformed and was dropped, or the response named no model. The turn itself succeeded. Each degraded section reports once, after the turn's [`Event::ModelTurnCompleted`]. A host may surface it as a backend-quality warning. Fields: [`Event::ModelMetadataDegraded::execution`](Event#variant.ModelMetadataDegraded.field.execution), [`Event::ModelMetadataDegraded::section`](Event#variant.ModelMetadataDegraded.field.section), [`Event::ModelMetadataDegraded::provenance`](Event#variant.ModelMetadataDegraded.field.provenance), and:
  - [`Event::ModelMetadataDegraded::turn`](Event#variant.ModelMetadataDegraded.field.turn), a [`u32`], is the model-turn counter of the round that served the response.
  - [`Event::ModelMetadataDegraded::message`](Event#variant.ModelMetadataDegraded.field.message), a [`String`], is the engine's sentence naming the section and why it did not parse. One example says that the completion response named no string model, recorded as empty. Another says that a malformed usage section in the completion response was ignored, followed by the decoder's reason. It may quote backend values, so treat it as untrusted.
- [`Event::Thinking`], kind `thinking`: one completed block of model thinking, the response's reasoning content. It is reported only when that content is present and non-empty, after [`Event::ModelTurnCompleted`] and before the round's [`Event::AssistantReply`]. A host may show it in a transcript as a thinking side channel. Fields: [`Event::Thinking::execution`](Event#variant.Thinking.field.execution), [`Event::Thinking::section`](Event#variant.Thinking.field.section), [`Event::Thinking::provenance`](Event#variant.Thinking.field.provenance), and:
  - [`Event::Thinking::turn`](Event#variant.Thinking.field.turn), a [`u32`], is the model-turn counter of the round that produced the block.
  - [`Event::Thinking::model`](Event#variant.Thinking.field.model), a [`String`], is the model that produced it.
  - [`Event::Thinking::text`](Event#variant.Thinking.field.text), a [`String`], is the thinking text, untrusted model output.
- [`Event::AssistantReply`], kind `assistant_reply`: one completed text reply from a model round. It is reported once per round whose outcome is text. A host appends chat replies to the conversation and may treat inference replies apart. Fields: [`Event::AssistantReply::execution`](Event#variant.AssistantReply.field.execution), [`Event::AssistantReply::section`](Event#variant.AssistantReply.field.section), [`Event::AssistantReply::provenance`](Event#variant.AssistantReply.field.provenance), and:
  - [`Event::AssistantReply::turn`](Event#variant.AssistantReply.field.turn), a [`u32`], is the model-turn counter of the round that produced the reply.
  - [`Event::AssistantReply::text`](Event#variant.AssistantReply.field.text), a [`String`], is the reply text, untrusted model output.
  - [`Event::AssistantReply::finish_reason`](Event#variant.AssistantReply.field.finish_reason), an [`Option`] of [`String`], is the provider's stop label when it sent one, such as `"stop"` or `"length"`, and [`None`] otherwise.
  - [`Event::AssistantReply::model`](Event#variant.AssistantReply.field.model), a [`String`], is the model that produced the reply.
  - [`Event::AssistantReply::metrics`](Event#variant.AssistantReply.field.metrics), an [`Option`] of [`CallMetrics`](crate::metrics::CallMetrics), is everything the call measured, with optional usage, llama, vllm, and client sections. It is [`None`] when nothing was measured. The [`metrics`](crate::metrics) module page covers the sections.
  - [`Event::AssistantReply::origin`](Event#variant.AssistantReply.field.origin), a [`ReplyOrigin`], is [`ReplyOrigin::Chat`] for a user-facing chat turn and [`ReplyOrigin::Infer`] for a `models.infer` round. It serializes as `"chat"` or `"infer"`, and a line without the key reads back as [`ReplyOrigin::Chat`].
- [`Event::AssistantToolCalls`], kind `assistant_tool_calls`: one batch of tool calls the model requested, before any of them run. It is reported in the chat arm, in place of [`Event::AssistantReply`], when a round returns tool calls. A host shows the requested calls, and each dispatched call's outcome follows later as an [`Event::ToolResult`] with the same turn and the call's id. The exception is a call to a Lua-local tool whose handler raises or returns an unsupported value: it reports [`Event::ToolCallFailed`] and no [`Event::ToolResult`], and the failure propagates to the caller of `models.loop` and ends the loop unless the author catches it. Fields: [`Event::AssistantToolCalls::execution`](Event#variant.AssistantToolCalls.field.execution), [`Event::AssistantToolCalls::section`](Event#variant.AssistantToolCalls.field.section), [`Event::AssistantToolCalls::provenance`](Event#variant.AssistantToolCalls.field.provenance), and:
  - [`Event::AssistantToolCalls::turn`](Event#variant.AssistantToolCalls.field.turn), a [`u32`], is the model-turn counter of the round that requested the batch.
  - [`Event::AssistantToolCalls::model`](Event#variant.AssistantToolCalls.field.model), a [`String`], is the model that requested the calls.
  - [`Event::AssistantToolCalls::calls`](Event#variant.AssistantToolCalls.field.calls), a [`Vec`] of [`ToolCallEvent`](crate::metrics::ToolCallEvent), lists the calls. Each has an [`id`](crate::metrics::ToolCallEvent::id) and a [`name`](crate::metrics::ToolCallEvent::name), both [`String`], and [`arguments`](crate::metrics::ToolCallEvent::arguments), a [`serde_json::Value`](https://docs.rs/serde_json/latest/serde_json/enum.Value.html). The names and arguments are untrusted model-authored text.
- [`Event::ModelCatalogValidationStarted`], kind `model_catalog_validation_started`: live-catalog model binding validation began. Declared but not currently emitted. Its section is the reporting scope. Fields: [`Event::ModelCatalogValidationStarted::execution`](Event#variant.ModelCatalogValidationStarted.field.execution), [`Event::ModelCatalogValidationStarted::section`](Event#variant.ModelCatalogValidationStarted.field.section), and [`Event::ModelCatalogValidationStarted::provenance`](Event#variant.ModelCatalogValidationStarted.field.provenance).
- [`Event::ModelCatalogValidationSucceeded`], kind `model_catalog_validation_succeeded`: live-catalog model binding validation succeeded. Declared but not currently emitted. Fields: [`Event::ModelCatalogValidationSucceeded::execution`](Event#variant.ModelCatalogValidationSucceeded.field.execution), [`Event::ModelCatalogValidationSucceeded::section`](Event#variant.ModelCatalogValidationSucceeded.field.section), and [`Event::ModelCatalogValidationSucceeded::provenance`](Event#variant.ModelCatalogValidationSucceeded.field.provenance).
- [`Event::ModelCatalogValidationFailed`], kind `model_catalog_validation_failed`: live-catalog model binding validation failed. Declared but not currently emitted. Fields: [`Event::ModelCatalogValidationFailed::execution`](Event#variant.ModelCatalogValidationFailed.field.execution), [`Event::ModelCatalogValidationFailed::section`](Event#variant.ModelCatalogValidationFailed.field.section), and [`Event::ModelCatalogValidationFailed::provenance`](Event#variant.ModelCatalogValidationFailed.field.provenance).

### Tool events

These report tool scopes and tool calls. Their section is the section that dispatched the call or whose tool scope was checked, and their provenance is that task.

- [`Event::ToolScopeValidationStarted`], kind `tool_scope_validation_started`: the engine began checking a model-visible tool scope. It is reported when the engine builds a model round's advertised scope: the bound tools, the Lua-local tools, and the task built-ins. Fields: [`Event::ToolScopeValidationStarted::execution`](Event#variant.ToolScopeValidationStarted.field.execution), [`Event::ToolScopeValidationStarted::section`](Event#variant.ToolScopeValidationStarted.field.section), and [`Event::ToolScopeValidationStarted::provenance`](Event#variant.ToolScopeValidationStarted.field.provenance).
- [`Event::ToolScopeValidationSucceeded`], kind `tool_scope_validation_succeeded`: the tool scope passed the check. Fields: [`Event::ToolScopeValidationSucceeded::execution`](Event#variant.ToolScopeValidationSucceeded.field.execution), [`Event::ToolScopeValidationSucceeded::section`](Event#variant.ToolScopeValidationSucceeded.field.section), and [`Event::ToolScopeValidationSucceeded::provenance`](Event#variant.ToolScopeValidationSucceeded.field.provenance).
- [`Event::ToolScopeValidationFailed`], kind `tool_scope_validation_failed`: building the schema for the round's tool scope returned an error. That error goes back to the code that requested the model round. Fields: [`Event::ToolScopeValidationFailed::execution`](Event#variant.ToolScopeValidationFailed.field.execution), [`Event::ToolScopeValidationFailed::section`](Event#variant.ToolScopeValidationFailed.field.section), and [`Event::ToolScopeValidationFailed::provenance`](Event#variant.ToolScopeValidationFailed.field.provenance).
- [`Event::ToolCallSucceeded`], kind `tool_call_succeeded`: a tool dispatch returned output. It comes before the call's [`Event::ToolResult`]. Fields: [`Event::ToolCallSucceeded::execution`](Event#variant.ToolCallSucceeded.field.execution), [`Event::ToolCallSucceeded::section`](Event#variant.ToolCallSucceeded.field.section), and [`Event::ToolCallSucceeded::provenance`](Event#variant.ToolCallSucceeded.field.provenance).
- [`Event::ToolCallFailed`], kind `tool_call_failed`: a tool dispatch returned an error. For a model-issued call, the error message is nonce-wrapped and still delivered to the model as the result, and reported as an [`Event::ToolResult`]. For a script-issued call, the error propagates to the Lua caller. The exception is a model-issued call to a Lua-local tool whose handler raises or returns an unsupported value: it reports no [`Event::ToolResult`], and the failure propagates to the caller of `models.loop` and ends the loop unless the author catches it. Fields: [`Event::ToolCallFailed::execution`](Event#variant.ToolCallFailed.field.execution), [`Event::ToolCallFailed::section`](Event#variant.ToolCallFailed.field.section), and [`Event::ToolCallFailed::provenance`](Event#variant.ToolCallFailed.field.provenance).
- [`Event::ToolResult`], kind `tool_result`: the result of one dispatched tool call, as it was delivered to the model or script. It follows the call's [`Event::ToolCallSucceeded`] or [`Event::ToolCallFailed`]. A host records it in the transcript beside the matching request. For a model-issued call it is reported on success and failure, and for a script-issued call only on success. The exception is a model-issued call to a Lua-local tool whose handler raises or returns an unsupported value: it reports only [`Event::ToolCallFailed`], and the failure propagates to the caller of `models.loop` and ends the loop unless the author catches it. Fields: [`Event::ToolResult::execution`](Event#variant.ToolResult.field.execution), [`Event::ToolResult::section`](Event#variant.ToolResult.field.section), [`Event::ToolResult::provenance`](Event#variant.ToolResult.field.provenance), and:
  - [`Event::ToolResult::turn`](Event#variant.ToolResult.field.turn), a [`u32`], is the model-turn counter of the round that requested the call, the same turn as that round's [`Event::AssistantToolCalls`], even when a tool that ran earlier in the batch ran model rounds of its own. For a call issued by a script, it is the counter's value when the script dispatched the call.
  - [`Event::ToolResult::tool_call_id`](Event#variant.ToolResult.field.tool_call_id), a [`String`], is the provider-issued id of the call that this result answers. Providers recycle ids across rounds, so scope it by the turn. It is the empty string for a call issued by a script.
  - [`Event::ToolResult::alias`](Event#variant.ToolResult.field.alias), a [`String`], is the tool alias named in the call.
  - [`Event::ToolResult::content`](Event#variant.ToolResult.field.content), a [`String`], is the tool's output. It is untrusted unless the tool was trusted, and untrusted output is recorded already nonce-wrapped.
  - [`Event::ToolResult::trusted`](Event#variant.ToolResult.field.trusted), a [`bool`], is `true` only when the dispatch treated the tool as trusted, [`OutputTrust::Trusted`](crate::tools::OutputTrust::Trusted), so its output was not nonce-wrapped.

### Store events

Every harness-mediated store operation reports one of these pairs. Their section is the section that requested the operation, and their provenance is the requesting task. They hold only the coordinates, as described in [Lifecycle boundaries](#lifecycle-boundaries). A host logs them as a store audit trail.

- [`Event::StoreWriteSucceeded`], kind `store_write_succeeded`: a store write succeeded. Fields: [`Event::StoreWriteSucceeded::execution`](Event#variant.StoreWriteSucceeded.field.execution), [`Event::StoreWriteSucceeded::section`](Event#variant.StoreWriteSucceeded.field.section), and [`Event::StoreWriteSucceeded::provenance`](Event#variant.StoreWriteSucceeded.field.provenance).
- [`Event::StoreWriteFailed`], kind `store_write_failed`: a store write failed. Fields: [`Event::StoreWriteFailed::execution`](Event#variant.StoreWriteFailed.field.execution), [`Event::StoreWriteFailed::section`](Event#variant.StoreWriteFailed.field.section), and [`Event::StoreWriteFailed::provenance`](Event#variant.StoreWriteFailed.field.provenance).
- [`Event::StoreAppendSucceeded`], kind `store_append_succeeded`: a store append succeeded. Fields: [`Event::StoreAppendSucceeded::execution`](Event#variant.StoreAppendSucceeded.field.execution), [`Event::StoreAppendSucceeded::section`](Event#variant.StoreAppendSucceeded.field.section), and [`Event::StoreAppendSucceeded::provenance`](Event#variant.StoreAppendSucceeded.field.provenance).
- [`Event::StoreAppendFailed`], kind `store_append_failed`: a store append failed. Fields: [`Event::StoreAppendFailed::execution`](Event#variant.StoreAppendFailed.field.execution), [`Event::StoreAppendFailed::section`](Event#variant.StoreAppendFailed.field.section), and [`Event::StoreAppendFailed::provenance`](Event#variant.StoreAppendFailed.field.provenance).
- [`Event::StoreReadSucceeded`], kind `store_read_succeeded`: a verbatim store read succeeded. Fields: [`Event::StoreReadSucceeded::execution`](Event#variant.StoreReadSucceeded.field.execution), [`Event::StoreReadSucceeded::section`](Event#variant.StoreReadSucceeded.field.section), and [`Event::StoreReadSucceeded::provenance`](Event#variant.StoreReadSucceeded.field.provenance).
- [`Event::StoreReadFailed`], kind `store_read_failed`: a verbatim store read failed. Fields: [`Event::StoreReadFailed::execution`](Event#variant.StoreReadFailed.field.execution), [`Event::StoreReadFailed::section`](Event#variant.StoreReadFailed.field.section), and [`Event::StoreReadFailed::provenance`](Event#variant.StoreReadFailed.field.provenance).
- [`Event::StoreReadNumberedSucceeded`], kind `store_read_numbered_succeeded`: a line-numbered store read succeeded. Fields: [`Event::StoreReadNumberedSucceeded::execution`](Event#variant.StoreReadNumberedSucceeded.field.execution), [`Event::StoreReadNumberedSucceeded::section`](Event#variant.StoreReadNumberedSucceeded.field.section), and [`Event::StoreReadNumberedSucceeded::provenance`](Event#variant.StoreReadNumberedSucceeded.field.provenance).
- [`Event::StoreReadNumberedFailed`], kind `store_read_numbered_failed`: a line-numbered store read failed. Fields: [`Event::StoreReadNumberedFailed::execution`](Event#variant.StoreReadNumberedFailed.field.execution), [`Event::StoreReadNumberedFailed::section`](Event#variant.StoreReadNumberedFailed.field.section), and [`Event::StoreReadNumberedFailed::provenance`](Event#variant.StoreReadNumberedFailed.field.provenance).
- [`Event::StoreReplaceSucceeded`], kind `store_replace_succeeded`: a store replacement succeeded. Fields: [`Event::StoreReplaceSucceeded::execution`](Event#variant.StoreReplaceSucceeded.field.execution), [`Event::StoreReplaceSucceeded::section`](Event#variant.StoreReplaceSucceeded.field.section), and [`Event::StoreReplaceSucceeded::provenance`](Event#variant.StoreReplaceSucceeded.field.provenance).
- [`Event::StoreReplaceFailed`], kind `store_replace_failed`: a store replacement failed. Fields: [`Event::StoreReplaceFailed::execution`](Event#variant.StoreReplaceFailed.field.execution), [`Event::StoreReplaceFailed::section`](Event#variant.StoreReplaceFailed.field.section), and [`Event::StoreReplaceFailed::provenance`](Event#variant.StoreReplaceFailed.field.provenance).
- [`Event::StoreDeleteSucceeded`], kind `store_delete_succeeded`: a store deletion succeeded. Fields: [`Event::StoreDeleteSucceeded::execution`](Event#variant.StoreDeleteSucceeded.field.execution), [`Event::StoreDeleteSucceeded::section`](Event#variant.StoreDeleteSucceeded.field.section), and [`Event::StoreDeleteSucceeded::provenance`](Event#variant.StoreDeleteSucceeded.field.provenance).
- [`Event::StoreDeleteFailed`], kind `store_delete_failed`: a store deletion failed. Fields: [`Event::StoreDeleteFailed::execution`](Event#variant.StoreDeleteFailed.field.execution), [`Event::StoreDeleteFailed::section`](Event#variant.StoreDeleteFailed.field.section), and [`Event::StoreDeleteFailed::provenance`](Event#variant.StoreDeleteFailed.field.provenance).
- [`Event::StoreGlobSucceeded`], kind `store_glob_succeeded`: a store glob succeeded. Fields: [`Event::StoreGlobSucceeded::execution`](Event#variant.StoreGlobSucceeded.field.execution), [`Event::StoreGlobSucceeded::section`](Event#variant.StoreGlobSucceeded.field.section), and [`Event::StoreGlobSucceeded::provenance`](Event#variant.StoreGlobSucceeded.field.provenance).
- [`Event::StoreGlobFailed`], kind `store_glob_failed`: a store glob failed. Fields: [`Event::StoreGlobFailed::execution`](Event#variant.StoreGlobFailed.field.execution), [`Event::StoreGlobFailed::section`](Event#variant.StoreGlobFailed.field.section), and [`Event::StoreGlobFailed::provenance`](Event#variant.StoreGlobFailed.field.provenance).

### Input events

These report operator input. Their section is the section that asked for input, and their provenance is the asking task.

- [`Event::UserInputWaitStarted`], kind `user_input_wait_started`: a section began waiting on operator input. It is reported before the run issues the [`Effect::UserInput`](crate::effect::Effect::UserInput). A host can show that the run is waiting on the operator. Fields: [`Event::UserInputWaitStarted::execution`](Event#variant.UserInputWaitStarted.field.execution), [`Event::UserInputWaitStarted::section`](Event#variant.UserInputWaitStarted.field.section), and [`Event::UserInputWaitStarted::provenance`](Event#variant.UserInputWaitStarted.field.provenance).
- [`Event::UserInput`], kind `user_input`: the operator's reply. It is reported only when the [`EffectAnswer::UserInput`](crate::effect::EffectAnswer::UserInput) resolves to [`InputOutcome::Text`](crate::input::InputOutcome::Text), and never for [`InputOutcome::Unavailable`](crate::input::InputOutcome::Unavailable). A host appends it to the transcript as the operator's turn. Fields: [`Event::UserInput::execution`](Event#variant.UserInput.field.execution), [`Event::UserInput::section`](Event#variant.UserInput.field.section), [`Event::UserInput::provenance`](Event#variant.UserInput.field.provenance), and:
  - [`Event::UserInput::text`](Event#variant.UserInput.field.text), a [`String`], is the operator's text, byte-exact and untrusted.

### Task events

These report the task tree described in [Following tasks](#following-tasks). Each entry says which section and task it reports under, because they differ.

- [`Event::TaskStarted`], kind `task_started`: a task chain was started by `tasks.spawn`, by `fanout` for one of its arms, or by the model's task tool. A host records it to build the task tree and to be able to start the chain again. Its section is the spawning section, not the task's target, and its provenance is the spawner's task and next sequence number. The seeds are author-provided, so treat them as untrusted when forwarding. Fields: [`Event::TaskStarted::execution`](Event#variant.TaskStarted.field.execution), [`Event::TaskStarted::section`](Event#variant.TaskStarted.field.section), [`Event::TaskStarted::provenance`](Event#variant.TaskStarted.field.provenance), and:
  - [`Event::TaskStarted::task`](Event#variant.TaskStarted.field.task), a [`TaskId`](crate::ids::TaskId), is the new task's id, the dotted id of its chain, such as `"0.0"`. The task's own events hold this id in [`Provenance::task`](crate::ids::Provenance::task).
  - [`Event::TaskStarted::target`](Event#variant.TaskStarted.field.target), a [`String`], is the name of the section where the task's chain starts.
  - [`Event::TaskStarted::origin`](Event#variant.TaskStarted.field.origin), a [`TaskOrigin`](crate::ids::TaskOrigin), is the principal that started the task: [`TaskOrigin::Author`](crate::ids::TaskOrigin::Author) through `tasks.spawn` or `fanout`, or [`TaskOrigin::Model`](crate::ids::TaskOrigin::Model) through the model's task tool. It serializes as `"author"` or `"model"`.
  - [`Event::TaskStarted::input`](Event#variant.TaskStarted.field.input), an [`Option`] of [`String`], is the `opts.input` override of the chain's `args`, or [`None`] when none was given.
  - [`Event::TaskStarted::item`](Event#variant.TaskStarted.field.item), an [`Option`] of [`serde_json::Value`](https://docs.rs/serde_json/latest/serde_json/enum.Value.html), is the `opts.item` seed installed as the chain's item global, or [`None`] when none was given.
  - [`Event::TaskStarted::index`](Event#variant.TaskStarted.field.index), an [`Option`] of [`u64`], is the `opts.index` seed that the chain sees as `sys.index`, or [`None`] when none was given.
  - [`Event::TaskStarted::var`](Event#variant.TaskStarted.field.var), a [`serde_json::Value`](https://docs.rs/serde_json/latest/serde_json/enum.Value.html), is the snapshot of the spawner's scratch table that the chain's own table starts from.
- [`Event::TaskSucceeded`], kind `task_succeeded`: terminal, the task's chain ended with a result. A host marks the task finished. Its section is the task's target section, and its provenance is the task's own id and next sequence number. For a task started by the model, an [`Event::TaskNotice`] is also queued on the owner. Fields: [`Event::TaskSucceeded::execution`](Event#variant.TaskSucceeded.field.execution), [`Event::TaskSucceeded::section`](Event#variant.TaskSucceeded.field.section), [`Event::TaskSucceeded::provenance`](Event#variant.TaskSucceeded.field.provenance), and:
  - [`Event::TaskSucceeded::task`](Event#variant.TaskSucceeded.field.task), a [`TaskId`](crate::ids::TaskId), is the task's id.
- [`Event::TaskFailed`], kind `task_failed`: terminal, the task's chain ended with an error. A host marks the task failed. It reports under the task's target section and the task's own provenance, and a task started by the model also gets an [`Event::TaskNotice`]. Fields: [`Event::TaskFailed::execution`](Event#variant.TaskFailed.field.execution), [`Event::TaskFailed::section`](Event#variant.TaskFailed.field.section), [`Event::TaskFailed::provenance`](Event#variant.TaskFailed.field.provenance), and:
  - [`Event::TaskFailed::task`](Event#variant.TaskFailed.field.task), a [`TaskId`](crate::ids::TaskId), is the task's id.
- [`Event::TaskCancelled`], kind `task_cancelled`: terminal, the owner stopped the task on purpose, for example through `tasks.cancel`. A host marks the task cancelled. It is reported once, under the task's target section and the task's own provenance, after anything the task's chain owned. A repeated cancel reports nothing. Fields: [`Event::TaskCancelled::execution`](Event#variant.TaskCancelled.field.execution), [`Event::TaskCancelled::section`](Event#variant.TaskCancelled.field.section), [`Event::TaskCancelled::provenance`](Event#variant.TaskCancelled.field.provenance), and:
  - [`Event::TaskCancelled::task`](Event#variant.TaskCancelled.field.task), a [`TaskId`](crate::ids::TaskId), is the task's id.
- [`Event::TaskAbandoned`], kind `task_abandoned`: terminal, the task's owner chain ended while the task was live, so the engine ended the task. It differs from a cancellation because the task lost its owner instead of being stopped on purpose. A host marks the task abandoned and may show the reason. It reports under the task's target section and the task's own provenance, after anything its chain owned, and a task started by the model also gets an [`Event::TaskNotice`]. Fields: [`Event::TaskAbandoned::execution`](Event#variant.TaskAbandoned.field.execution), [`Event::TaskAbandoned::section`](Event#variant.TaskAbandoned.field.section), [`Event::TaskAbandoned::provenance`](Event#variant.TaskAbandoned.field.provenance), and:
  - [`Event::TaskAbandoned::task`](Event#variant.TaskAbandoned.field.task), a [`TaskId`](crate::ids::TaskId), is the task's id.
  - [`Event::TaskAbandoned::reason`](Event#variant.TaskAbandoned.field.reason), an [`AbandonReason`](crate::ids::AbandonReason), says how the owner ended. [`AbandonReason::OwnerReturned`](crate::ids::AbandonReason::OwnerReturned), `owner_returned` on the wire, means the owner ended normally without waiting on or cancelling the task. [`AbandonReason::OwnerFailed`](crate::ids::AbandonReason::OwnerFailed), `owner_failed`, means the owner failed. [`AbandonReason::ToolLoopExhausted`](crate::ids::AbandonReason::ToolLoopExhausted), `tool_loop_exhausted`, means the owner's model and tool loop ran past its round cap. [`AbandonReason::OwnerAborted`](crate::ids::AbandonReason::OwnerAborted), `owner_aborted`, means a fatal sibling's fail-fast or the owner's own owner ending first. [`AbandonReason::RunTerminated`](crate::ids::AbandonReason::RunTerminated), `run_terminated`, means the host cancelled the run or a fatal answer ended it. [`AbandonReason::why`](crate::ids::AbandonReason::why) gives a phrase such as `"the tool loop was exhausted"`. The enum is `#[non_exhaustive]`.
- [`Event::TaskResumed`], kind `task_resumed`: reserved for an existing task revived from its record instead of started anew. Declared but not currently emitted. A host should accept it when reading logs. Fields: [`Event::TaskResumed::execution`](Event#variant.TaskResumed.field.execution), [`Event::TaskResumed::section`](Event#variant.TaskResumed.field.section), [`Event::TaskResumed::provenance`](Event#variant.TaskResumed.field.provenance), and:
  - [`Event::TaskResumed::task`](Event#variant.TaskResumed.field.task), a [`TaskId`](crate::ids::TaskId), is the task's id.
- [`Event::TaskNotice`], kind `task_notice`: one notice queued for a task's owner, the engine's sentence telling the model how a task it started ended. It is reported for tasks started by the model only, when the task succeeds, fails, is cancelled by the author, or is abandoned. A host may show it as a system line in the owner's transcript. Its section is the owner's section, and its provenance is the owner's task and next sequence number. Fields: [`Event::TaskNotice::execution`](Event#variant.TaskNotice.field.execution), [`Event::TaskNotice::section`](Event#variant.TaskNotice.field.section), [`Event::TaskNotice::provenance`](Event#variant.TaskNotice.field.provenance), and:
  - [`Event::TaskNotice::turn`](Event#variant.TaskNotice.field.turn), a [`u32`], is the owner's model-turn counter when the notice was queued.
  - [`Event::TaskNotice::task`](Event#variant.TaskNotice.field.task), a [`TaskId`](crate::ids::TaskId), is the task that ended.
  - [`Event::TaskNotice::text`](Event#variant.TaskNotice.field.text), a [`String`], is the sentence shown to the model, in one of the shapes listed in [Following tasks](#following-tasks).
- [`Event::TaskNote`], kind `task_note`: a task set its own progress note through `tasks.note`, which its owner reads through `task_status`. Declared but not currently emitted, because the engine stores the note on the chain without reporting it. Its documented section is the task's target section, and its provenance is the noting task. Fields: [`Event::TaskNote::execution`](Event#variant.TaskNote.field.execution), [`Event::TaskNote::section`](Event#variant.TaskNote.field.section), [`Event::TaskNote::provenance`](Event#variant.TaskNote.field.provenance), and:
  - [`Event::TaskNote::task`](Event#variant.TaskNote.field.task), a [`TaskId`](crate::ids::TaskId), is the task that set the note.
  - [`Event::TaskNote::text`](Event#variant.TaskNote.field.text), a [`String`], is the note, untrusted and written by the task's model or its Lua.

### Debug events

These appear only when the run's context set [`DebugMode::On`]. Their section is the section that issued the model call, and their provenance is the calling task. The bodies are raw and unredacted and include the full prompt, so a debug capture stores them as sensitive data.

- [`Event::Request`], kind `request`: the JSON body sent to the chat-completions endpoint for one model turn. It is reported just before the round's [`Event::Response`] and [`Event::ModelTurnCompleted`]. The same body also travels in the [`Effect::Chat`](crate::effect::Effect::Chat). Fields: [`Event::Request::execution`](Event#variant.Request.field.execution), [`Event::Request::section`](Event#variant.Request.field.section), [`Event::Request::provenance`](Event#variant.Request.field.provenance), and:
  - [`Event::Request::turn`](Event#variant.Request.field.turn), a [`u32`], is the 1-based model-turn number within the run.
  - [`Event::Request::body`](Event#variant.Request.field.body), a [`serde_json::Value`](https://docs.rs/serde_json/latest/serde_json/enum.Value.html), is the serialized request body.
- [`Event::Response`], kind `response`: the JSON body returned for one model turn, with parsed metadata. It is reported right after the round's [`Event::Request`]. The same body also travels in the [`EffectAnswer::Chat`](crate::effect::EffectAnswer::Chat). Fields: [`Event::Response::execution`](Event#variant.Response.field.execution), [`Event::Response::section`](Event#variant.Response.field.section), [`Event::Response::provenance`](Event#variant.Response.field.provenance), and:
  - [`Event::Response::turn`](Event#variant.Response.field.turn), a [`u32`], is the 1-based model-turn number within the run.
  - [`Event::Response::body`](Event#variant.Response.field.body), a [`serde_json::Value`](https://docs.rs/serde_json/latest/serde_json/enum.Value.html), is the raw response body.
  - [`Event::Response::finish_reason`](Event#variant.Response.field.finish_reason), an [`Option`] of [`String`], is the choice's finish reason when the backend supplied one.
  - [`Event::Response::reasoning_content`](Event#variant.Response.field.reasoning_content), an [`Option`] of [`String`], is the message's reasoning content when the backend supplied it.

## DebugMode

[`DebugMode`] chooses whether a run also reports each model round's raw request and response bodies as [`Event::Request`] and [`Event::Response`]. The host names a variant and passes it to [`RunContext::report_debug`](crate::RunContext::report_debug), which takes the context by value and returns the updated context. [Debug capture](#debug-capture) shows the call.

- [`DebugMode::Off`]: model rounds report no [`Event::Request`] or [`Event::Response`] and never clone a body. It is the [`Default`], and a context built without [`RunContext::report_debug`](crate::RunContext::report_debug) uses it. Use it when the host already logs the [`Effect::Chat`](crate::effect::Effect::Chat) and its answer, or does not want unredacted prompts in the event stream.
- [`DebugMode::On`]: every model round reports an [`Event::Request`] followed by an [`Event::Response`], both before that round's [`Event::ModelTurnCompleted`]. Use it when a debug capture needs the bodies inside the event log, and treat the resulting events as sensitive.

[`DebugMode`] has no serde, [`FromStr`](std::str::FromStr), or [`Display`](std::fmt::Display) impl, so a host that stores the setting picks its own representation.

## ReplyOrigin

[`ReplyOrigin`] says which path produced an [`Event::AssistantReply`]: a user-facing chat turn or a programmatic inference round. A host reads it from [`Event::AssistantReply::origin`](Event#variant.AssistantReply.field.origin) to decide whether the reply belongs in the conversation. Hosts name a variant directly only in test fixtures.

- [`ReplyOrigin::Chat`]: a user-facing chat turn from the chat arm. The host appends the reply to the visible conversation. It serializes as `"chat"`. It is the [`Default`], and the value that an older log line with no `"origin"` key reads back as.
- [`ReplyOrigin::Infer`]: a programmatic `models.infer` round. The host may log the reply without adding it to the visible conversation. It serializes as `"infer"`.

[`ReplyOrigin`] is `#[non_exhaustive]`, so a `match` on it handles the two known origins and keeps a wildcard arm. It has no [`Display`](std::fmt::Display) or [`FromStr`](std::str::FromStr) impl.

