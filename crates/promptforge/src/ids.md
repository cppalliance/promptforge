Group a run's log by task, and follow each task from its start to how it ended.

You need this when you store a log you will search later, or when you show a run's tasks to a person.

# Where this fits

From the [crate overview](crate), you know that a run hands you effects and reports events as it goes. The [event page](crate::event) shows that every event carries a provenance, and each effect arrives with one too. This page explains the task id inside that provenance, and the task events that start and end each task.

# Group a log by task

Your program stores each run's effects and events, and later you want to search them by task, or show a run's tasks side by side.

A section can start another section to run beside it, and the started one is a *task*.

Each walk over sibling sections is a *chain*: the main walk, each task, and each `call`. In a section's Lua, `call('## Target')` runs that section on a new chain and waits for its result, while `tasks.spawn` returns at once and lets the task run alongside.

Every effect and event records the task whose chain made it, plus a counter local to that task. That pair is its *provenance*.

Grouping by task feels like grouping log lines by thread id. Unlike a thread id, a task id is a path from the root chain `0`, and the same run gives the same ids every time.

````
# use std::collections::BTreeMap;
# use std::error::Error;
# use std::num::NonZeroU32;
# use std::sync::Arc;
# use promptforge::effect::{Effect, EffectAnswer};
# use promptforge::event::Event;
# use promptforge::ids::{Provenance, TaskId};
# use promptforge::model::{Completion, CompletionResult, ModelDescriptor, ModelId, ThinkingMode};
# use promptforge::timestamp::Timestamp;
# use promptforge::{Environment, Prompt, Run, RunContext, RunResult, Step};
# fn start(source: &str) -> Result<(Run, Vec<Event>), Box<dyn Error>> {
#     let (parsed, parse_events) = Prompt::parse(source, "greeter");
#     let prompt = parsed?;
#     let window = NonZeroU32::new(8_192).ok_or("a context window is never zero")?;
#     let model = ModelDescriptor::new(ModelId::gateway("canned")?, "Always replies hi there", window, ThinkingMode::Never);
#     let ctx = RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH)
#         .provenance_start(u32::try_from(parse_events.len())?)
#         .model(model);
#     let (ctx, requirements) = Environment::new().prepare(&prompt, ctx);
#     if let Some(refusal) = requirements.refusal() {
#         return Err(refusal.into());
#     }
#     Ok((Run::new(Arc::new(prompt), "", ctx), parse_events))
# }
# fn drive(
#     mut run: Run,
#     mut answer: impl FnMut(Effect) -> Option<EffectAnswer>,
# ) -> (RunResult, Vec<Event>, Vec<Provenance>) {
#     let (mut events, mut effects, mut held) = (Vec::new(), Vec::new(), Vec::new());
#     loop {
#         match run.step() {
#             Step::Pending { effects: batch, events: reported } => {
#                 assert!(!batch.is_empty() || run.decided(), "the run waits on an effect this Harness holds");
#                 events.extend(reported);
#                 for (id, provenance, effect) in batch {
#                     effects.push(provenance);
#                     match answer(effect) {
#                         Some(reply) => run.resume(id, reply),
#                         None => held.push(id),
#                     }
#                 }
#                 if run.decided() {
#                     for id in held.drain(..) {
#                         run.resume(id, EffectAnswer::Dropped);
#                     }
#                 }
#             }
#             Step::Done { result, events: reported } => {
#                 events.extend(reported);
#                 return (result, events, effects);
#             }
#         }
#     }
# }
# fn reply(result: CompletionResult) -> EffectAnswer {
#     EffectAnswer::Chat(Completion::from_result(result, "canned").map(Box::new).map_err(Into::into))
# }
# fn canned(effect: Effect) -> Option<EffectAnswer> {
#     match effect {
#         Effect::Chat { .. } => Some(reply(CompletionResult::Text("hi there".to_owned()))),
#         _ => Some(EffectAnswer::Dropped),
#     }
# }
// 1. The greeter's section starts two tasks on `## Reply`, waits for both, and joins their replies.
const GREETER: &str = concat!(
#     "---\n",
#     "name: greeter\n",
#     "description: Greets through tasks.\n",
#     "promptforge: 0\n",
#     "models:\n",
#     "  writer: {}\n",
#     "---\n\n",
#     "# Greeter\n\n",
#     "## Greet\n\n",
#     "```lua\n",
    "local first = tasks.spawn('## Reply', { input = 'hello' })\n",
    "local second = tasks.spawn('## Reply', { input = 'bye' })\n",
    "local results = tasks.join({ first, second })\n",
    "return results[1].result .. ' ' .. results[2].result\n",
#     "```\n\n",
#     "## Reply\n\n",
#     "```lua\n",
#     "models.use('writer')\n",
#     "return models.infer(args)\n",
#     "```\n",
);

// 2. Run the greeter with a canned reply for each task, group every effect and event by its provenance's task, sort each group by provenance, and list the started tasks.
fn run_greeter() -> Result<(Vec<TaskId>, BTreeMap<TaskId, Vec<Provenance>>), Box<dyn Error>> {
    let (run, parse_events) = start(GREETER)?;
    let (result, run_events, effects) = drive(run, canned);
    assert!(matches!(result, RunResult::Ok(text) if text == "hi there hi there"));
    let events: Vec<Event> = parse_events.into_iter().chain(run_events).collect();
    let mut groups: BTreeMap<TaskId, Vec<Provenance>> = BTreeMap::new();
    for provenance in effects.into_iter().chain(events.iter().map(|event| event.provenance().clone())) {
        groups.entry(provenance.task.clone()).or_default().push(provenance);
    }
    groups.values_mut().for_each(|records| records.sort());
    let started = events.iter().filter_map(|event| match event {
        Event::TaskStarted { task, .. } => Some(task.clone()),
        _ => None,
    }).collect();
    Ok((started, groups))
}

// 3. Print each task's group, in task id order, with the sequence numbers its records carry.
let (started, groups) = run_greeter()?;
for (task, records) in &groups {
    let seqs: Vec<u32> = records.iter().map(|record| record.seq).collect();
    println!("task {task}: {seqs:?}");
}

// 4. The main task and the two started tasks each get a group, the started ids sort in start order, and a second run gives the same groups.
assert_eq!(groups.len(), 3);
let mut sorted = started.clone();
sorted.sort();
assert_eq!(sorted, started);
assert_eq!(started, ["0.0".parse::<TaskId>()?, "0.1".parse::<TaskId>()?]);
assert_eq!(run_greeter()?.1, groups);

// 5. Parsed ids compare as values, and a damaged id reports its whole text.
assert_eq!("0.01".parse::<TaskId>()?, "0.1".parse::<TaskId>()?);
let error = "0..1".parse::<TaskId>().err().ok_or("0..1 is not a task id")?;
assert_eq!(error.input(), "0..1");
# Ok::<(), Box<dyn std::error::Error>>(())
````

1. `## Greet` starts two tasks on `## Reply` with `tasks.spawn`, and waits for both with `tasks.join`. `## Reply` asks the model, so each task asks on its own.
2. `run_greeter` drives the greeter offline with the hidden `start`, `drive`, and `canned`. [`Prompt::parse`](crate::Prompt::parse) stamps its events under task `0` from `seq` 0, and so does a run by default, which would repeat `(task, seq)` pairs in one log. `start` passes the parse event count to [`RunContext::provenance_start`](crate::RunContext::provenance_start), so the run's task `0` counts on after the parse events. Started tasks still count from 0. It groups every record by [`Provenance::task`], a [`TaskId`], in a [`BTreeMap`](std::collections::BTreeMap), and sorts each group by [`Provenance`], which orders by task path, then by `seq`.
3. The loop prints task `0`, the main walk, then `0.0` and `0.1`. A task's effects and events share one `seq` counter, so they interleave in the order the task made them. When the log keeps parse events out, or the run uses `provenance_start` as here, no two records share a `(task, seq)` pair. Key your log table on it, and store each effect's answer under that effect's provenance.
4. The asserts show three groups. A `call` reports under its caller's task, so it never gets a group of its own. Each chain numbers its children with one counter shared by `call` children and started tasks, so spawn, call, spawn gives tasks `.0` and `.2`. Sorting one chain's task ids gives their start order, and a gap is a `call`, not a missing task. A second run gives the same groups, but the [`EffectId`](crate::effect::EffectId) need not repeat, so diff runs by provenance.
5. Store a task id as its dotted text, which is how it serializes, and read it back with `parse::<TaskId>()`. Parsing accepts leading zeros, so compare parsed ids, not text: `"0.01"` equals `0.1`. `"0..1"` fails with a [`ParseIdError`], whose [`input`](ParseIdError::input) is the whole rejected text. Parsing accepts paths off the root, such as `5.3`, so check the root yourself.

This run's task tree, with a `call` from `## Greet` added after the spawns:

````text
┌─ task 0: the main walk, ## Greet ───────────────────────┐
│                                                          │
│  ┌─ a `call` from ## Greet after the spawns ──────────┐  │
│  │ its chain is 0.2, but its records say task 0       │  │
│  └────────────────────────────────────────────────────┘  │
└───────┬─────────────────────────────┬────────────────────┘
        │ tasks.spawn                 │ tasks.spawn
        v                             v
┌─ task 0.0 ─────────────┐   ┌─ task 0.1 ─────────────┐
│ ## Reply, input hello  │   │ ## Reply, input bye    │
└────────────────────────┘   └────────────────────────┘
````

The `call` takes chain `0.2` from the shared counter, yet its records say task `0`, because it blocks its caller. A task's id is its chain's path, and `TaskId` wraps that chain id so a task-keyed map cannot take any chain by mistake. Build the main task's as `TaskId::from(ChainId::root())`.

You might expect task ids to sort like their text, so that `0.10` comes before `0.2`. Instead, `TaskId` compares each component as a number, so `0.2` comes first, and a task sorts right before the tasks it started.

Group by the provenance's task, order by the provenance itself, and the same run gives the same groups every time. Next, [Follow a task's life](#follow-a-tasks-life) shows who started each task and how it ended.

# Follow a task's life

You show a run's tasks to a person. For each one, you want to say who started it and how it ended.

The chain that started a task owns it: the main walk, a `call` child, or another task. A task ends only when its owner chain ends, not when the walk leaves its section by falling through or `jump`.

A task that runs reports [`TaskStarted`](crate::event::Event::TaskStarted) when it first runs, possibly after waiting for a slot under a concurrency limit, and one *terminal event*: [`TaskSucceeded`](crate::event::Event::TaskSucceeded), [`TaskFailed`](crate::event::Event::TaskFailed), [`TaskCancelled`](crate::event::Event::TaskCancelled), or [`TaskAbandoned`](crate::event::Event::TaskAbandoned). A task cancelled or abandoned while waiting reports neither.

Following a task feels like joining a spawned thread. Unlike dropping a [`JoinHandle`](std::thread::JoinHandle), which leaves the thread running, a task whose owner ends without waiting is ended and reported.

````
# use std::error::Error;
# use std::num::NonZeroU32;
# use std::sync::Arc;
# use promptforge::effect::{Effect, EffectAnswer};
# use promptforge::event::Event;
# use promptforge::ids::Provenance;
# use promptforge::model::{Completion, CompletionResult, ModelDescriptor, ModelId, ThinkingMode};
# use promptforge::timestamp::Timestamp;
# use promptforge::{Environment, Prompt, Run, RunContext, RunResult, Step};
# fn start(source: &str) -> Result<(Run, Vec<Event>), Box<dyn Error>> {
#     let (parsed, parse_events) = Prompt::parse(source, "greeter");
#     let prompt = parsed?;
#     let window = NonZeroU32::new(8_192).ok_or("a context window is never zero")?;
#     let model = ModelDescriptor::new(ModelId::gateway("canned")?, "Always replies hi there", window, ThinkingMode::Never);
#     let ctx = RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH)
#         .provenance_start(u32::try_from(parse_events.len())?)
#         .model(model);
#     let (ctx, requirements) = Environment::new().prepare(&prompt, ctx);
#     if let Some(refusal) = requirements.refusal() {
#         return Err(refusal.into());
#     }
#     Ok((Run::new(Arc::new(prompt), "", ctx), parse_events))
# }
# fn drive(
#     mut run: Run,
#     mut answer: impl FnMut(Effect) -> Option<EffectAnswer>,
# ) -> (RunResult, Vec<Event>, Vec<Provenance>) {
#     let (mut events, mut effects, mut held) = (Vec::new(), Vec::new(), Vec::new());
#     loop {
#         match run.step() {
#             Step::Pending { effects: batch, events: reported } => {
#                 assert!(!batch.is_empty() || run.decided(), "the run waits on an effect this Harness holds");
#                 events.extend(reported);
#                 for (id, provenance, effect) in batch {
#                     effects.push(provenance);
#                     match answer(effect) {
#                         Some(reply) => run.resume(id, reply),
#                         None => held.push(id),
#                     }
#                 }
#                 if run.decided() {
#                     for id in held.drain(..) {
#                         run.resume(id, EffectAnswer::Dropped);
#                     }
#                 }
#             }
#             Step::Done { result, events: reported } => {
#                 events.extend(reported);
#                 return (result, events, effects);
#             }
#         }
#     }
# }
# fn reply(result: CompletionResult) -> EffectAnswer {
#     EffectAnswer::Chat(Completion::from_result(result, "canned").map(Box::new).map_err(Into::into))
# }
use promptforge::ids::{AbandonReason, TaskOrigin};
use promptforge::model::ToolCall;

// 1. The greeter lets its model start tasks on `## Wait`, runs one model loop, and returns without waiting.
let source = concat!(
#     "---\n",
#     "name: greeter\n",
#     "description: Greets through tasks.\n",
#     "promptforge: 0\n",
#     "models:\n",
#     "  writer: {}\n",
#     "---\n\n",
#     "# Greeter\n\n",
#     "## Greet\n\n",
#     "```lua\n",
    "models.use('writer')\n",
    "tools.allow_tasks({ '## Wait' })\n",
    "local msgs = messages.new()\n",
    "msgs:user('hello')\n",
    "models.loop(msgs)\n",
    "return 'done'\n",
    "```\n\n",
    "## Wait\n\n",
    "```lua\n",
    "return store.read('note.md')\n",
    "```\n",
);

// 2. The canned model calls its `task` tool, then says bye; the task's store read is held, so it still waits.
let call = ToolCall::from_parts("call_1", "task", serde_json::json!({ "target": "## Wait" }))?;
let mut replies = vec![CompletionResult::Text("bye".to_owned()), CompletionResult::ToolCalls(vec![call])];
let (run, _parse_events) = start(source)?;
let (result, events, _effects) = drive(run, |effect| match effect {
    Effect::Chat { .. } => replies.pop().map(reply),
    _ => None,
});
assert!(matches!(result, RunResult::Ok(text) if text == "done"));

// 3. Read who started the task, find its end by the task id in the payload, and print both.
let (task, origin) = events.iter().find_map(|event| match event {
    Event::TaskStarted { task, origin, .. } => Some((task, *origin)),
    _ => None,
}).ok_or("the model starts a task")?;
let end = events.iter().find(|event| matches!(event,
    Event::TaskSucceeded { task: t, .. } | Event::TaskFailed { task: t, .. }
        | Event::TaskCancelled { task: t, .. } | Event::TaskAbandoned { task: t, .. } if t == task
)).ok_or("the task ends")?;
let Event::TaskAbandoned { reason, .. } = end else { panic!("the task ended as {end:?}") };
println!("task {task}: started by {}, abandoned because {}", origin.tag(), reason.why());

// 4. The model started the task, and its owner's return abandoned it.
assert_eq!((origin, origin.tag()), (TaskOrigin::Model, "model"));
assert_eq!(*reason, AbandonReason::OwnerReturned);
assert_eq!(reason.why(), "the section ended");
# Ok::<(), Box<dyn std::error::Error>>(())
````

1. `## Greet` offers the model the `task` tool for `## Wait` with `tools.allow_tasks`, runs one model loop, and returns `done` without waiting, ending the main walk, which owns the still-live task.
2. `replies` is listed last-first because `pop` takes from the end. `drive` holds the task's store read until [`Run::decided`](crate::Run::decided) shows the run has reported its end, then answers held effects with [`EffectAnswer::Dropped`](crate::effect::EffectAnswer::Dropped) so `step` can return `Done`. So the task still waits when `## Greet` returns.
3. Match a start to its end by the `task` field, not by section. [`Event::section`](crate::event::Event::section) names the H2 heading or agent a record was reported under, and `TaskStarted` reports under the starting section, the terminal event under the task's own. Grouping by provenance splits them too, since `TaskStarted` carries the starting chain's task, so a task's group from [Group a log by task](#group-a-log-by-task) holds its end but not its start.
4. The asserts show [`TaskOrigin::Model`] and [`AbandonReason::OwnerReturned`], whose [`why`](AbandonReason::why) is `the section ended`. A model's task that outlives its starting chain, not merely its section, is abandoned, and the run does not fail.

Read who started a task from `TaskStarted`'s `origin`: [`TaskOrigin::Author`] for `tasks.spawn` or `fanout`, `TaskOrigin::Model` for the model's `task` tool. `fanout('### Worker', items)` starts one task per item, each reading its own `item`, and cancels the rest when one fails. Store an origin as [`TaskOrigin::tag`], `author` or `model`, its serialized form, and read it back with [`TaskOrigin::from_tag`], which accepts only those exact strings.

When the starting chain ends normally, by returning or walking past its last section, with a task still live, both kinds end `TaskAbandoned` with `OwnerReturned`. An author's task also turns the owner chain's result into the `tasks_live` error, listing leaked task ids in spawn order: the run's error at the main walk, or the `call`'s error. A chain that already failed keeps its own. Wait on or cancel every task first.

Keep `TaskCancelled`, a deliberate stop by the owner reported once, apart from `TaskAbandoned`, the owner ending while the task was live. Show a person `why`; the reason serializes in snake_case, such as `owner_returned`. Each other reason points at a different thing to fix:

- [`OwnerFailed`](AbandonReason::OwnerFailed): the owner failed.
- [`ToolLoopExhausted`](AbandonReason::ToolLoopExhausted): the model's tool loop ran past its round cap.
- [`OwnerAborted`](AbandonReason::OwnerAborted): the owner was ended from outside, by its own owner ending first or by `fanout` cancelling it when another of its tasks failed; that owner ends `TaskCancelled`.
- [`RunTerminated`](AbandonReason::RunTerminated): the run ended, including by your cancel, and stranded this task directly.

You might expect cancelling a run to report its live tasks as cancelled. Instead, tasks it strands directly end `TaskAbandoned` with `RunTerminated`, and their own tasks with `OwnerAborted`, because only an owner stopping a task on purpose counts as cancelled.

`TaskStarted` says who, the terminal event says how, and abandoned means the owner went away, not that anyone stopped the task. The [Reference](#reference) covers each type.

# Reference

## ChainId

[`ChainId`] names one chain of a run, the main walk, a `call` child, or a started task, as a path of child indices from the root `0`. The same inputs give the same ids on every run. It orders by numeric path, never as text, and serializes as its dotted text. Parsing fails with [`ParseIdError`]; check that the text is a dotted decimal path, such as `0.2.0`. [Group a log by task](#group-a-log-by-task) teaches it.

- [`ChainId::root`] returns the main walk's id, `0`.
- [`ChainId::child`] extends this id by `index`, without checking that the index was ever allocated.
- [`ChainId::entry`] returns `"{self}.{index}"` as text, the `sys.id` a section reads on each entry; entries count apart from child chains, so it is no chain id.

## ParseIdError

[`ParseIdError`] reports that text did not parse as a [`ChainId`] or [`TaskId`] path. You get one when you parse an id read back from a log, for example with `?`. Only parsing makes one, so you cannot reuse it for your own errors. Report the rejected text beside its record, and treat that record as damaged or written by something other than a run. [Group a log by task](#group-a-log-by-task) shows one.

- [`ParseIdError::input`] returns the whole rejected text, not the offending component.

## Provenance

[`Provenance`] stamps every effect and event with the nearest enclosing task and the item's place in that task. It is the same on every run of the same prompt with the same inputs and answers, unlike the run-wide [`EffectId`](crate::effect::EffectId), so key replay and cross-run comparison on it. It orders by task path, then by `seq`. Group records by `task`, and sort each group by `seq`. [Group a log by task](#group-a-log-by-task) teaches it.

- [`Provenance::task`] is the task whose chain emitted the item. The main walk is task `0`, and a `call` child reports its caller's task.
- [`Provenance::seq`] counts effects and events per task, so events alone skip numbers. Parse events start task `0` at 0; the run continues only with [`RunContext::provenance_start`](crate::RunContext::provenance_start).

## TaskId

[`TaskId`] names one task by the id of the chain that runs it, wrapped so a task-keyed table cannot take an arbitrary chain by mistake. Use it to key tables or APIs by task, such as fetching one task's events. It displays, serializes, and orders exactly as its chain id does. Parsing fails with [`ParseIdError`] under the same rules as [`ChainId`]; check that the text is a dotted path such as `0.2`. [Group a log by task](#group-a-log-by-task) teaches it.

- There is no `root` constructor. Build the main task's id as `TaskId::from(ChainId::root())`, or parse `"0"`.
- The `From<ChainId>` conversion goes one way only, so keep the chain id yourself if you need it later.

## AbandonReason

[`AbandonReason`] says how a task's owner ended while the task was still live. Abandoned is kept apart from cancelled, because losing an owner and being stopped on purpose are different facts. Show [`AbandonReason::why`] to a person, since the type has no `Display`, and match the variant in code. It is `#[non_exhaustive]`, so a match outside the crate needs a wildcard arm. [Follow a task's life](#follow-a-tasks-life) teaches it.

| Variant | Stored as | The task was live when |
|---|---|---|
| [`OwnerReturned`](AbandonReason::OwnerReturned) | `owner_returned` | its owner ended normally without waiting on or cancelling it, because a section returned a value or the walk ran past the prompt's last section; an author's task is abandoned this way too, and its owner chain also fails with `tasks_live` |
| [`OwnerFailed`](AbandonReason::OwnerFailed) | `owner_failed` | its owner failed |
| [`ToolLoopExhausted`](AbandonReason::ToolLoopExhausted) | `tool_loop_exhausted` | its owner's model-tool loop ran past its round cap |
| [`OwnerAborted`](AbandonReason::OwnerAborted) | `owner_aborted` | its owner was ended from outside before it finished, because its own owner ended first, or because the owner was a `fanout` task that `fanout` cancelled when another of its tasks failed |
| [`RunTerminated`](AbandonReason::RunTerminated) | `run_terminated` | the run itself ended, cancelled by the Host or ended by a fatal answer, and the run's end stranded it directly; a task started by a stranded task ends with `OwnerAborted` |

- `why` returns the short phrase the task-abandoned trace line renders, such as `the section ended`.

## TaskOrigin

[`TaskOrigin`] names who started a task, beside the task's id wherever the task is reported: [`Author`](TaskOrigin::Author), through `tasks.spawn` and `fanout`, or [`Model`](TaskOrigin::Model), through the model's `task` tool. Both are abandoned with `TaskAbandoned` when they outlive their owner, but only an author's task also turns the owner chain's result into the `tasks_live` error. It has no `FromStr` or `Display`, and is `#[non_exhaustive]`, so add a wildcard arm. [Follow a task's life](#follow-a-tasks-life) teaches it.

- [`TaskOrigin::tag`] returns `author` or `model`, the same strings it serializes as.
- [`TaskOrigin::from_tag`] returns `None` for anything but those exact lowercase strings, without trimming, so pass the exact tag.
