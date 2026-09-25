The ids that name a run's chains and tasks, the provenance stamped on every effect and event, and the records of who started a task and why it was abandoned.

This module gives a host the keys for its log. Every effect and event carries a [`Provenance`], which says which task produced it and where it falls in that task's sequence. With it, a host can split a log by task, put each task's items in order, answer a prompt that reads a task's history, and match a replayed run against its record. The ids behind it are plain text paths such as `0.2.1`, so they read as the hierarchy they name in a log line or a UI, and they parse and serialize as that same text. By the end of this page you can compute, render, parse, sort, and persist every id a run reports, and read the task origin and abandon reason on task events.

# Where this fits

[`Run::step`](crate::Run::step) returns [`Step::Pending`](crate::Step::Pending), where each effect is a tuple of an [`EffectId`](crate::effect::EffectId), a [`Provenance`], and an [`Effect`](crate::effect::Effect). The host answers through [`Run::resume`](crate::Run::resume) under the [`EffectId`](crate::effect::EffectId), which is an in-flight handle local to the run. It logs the effect under the [`Provenance`], which is the stable key for matching a replay against its record. The run ends with [`Step::Done`](crate::Step::Done). Every [`Event`](crate::event::Event) exposes its [`Provenance`] through [`Event::provenance`](crate::event::Event::provenance), so the host fills a record's task and sequence columns without matching on the variant.

Task events name their task with a [`TaskId`]. [`Event::TaskStarted`](crate::event::Event::TaskStarted) adds a [`TaskOrigin`] in [`origin`](crate::event::Event#variant.TaskStarted.field.origin), and [`Event::TaskAbandoned`](crate::event::Event::TaskAbandoned) adds an [`AbandonReason`] in [`reason`](crate::event::Event#variant.TaskAbandoned.field.reason).

The log feeds back into the run through [`Effect::TaskEvents`](crate::effect::Effect::TaskEvents). The run issues it when the author's `tasks.events` or the model's `task_events` built-in reads a task's history. The host answers with [`EffectAnswer::TaskEvents`](crate::effect::EffectAnswer::TaskEvents), holding the logged events whose [`Provenance::task`] matches. When the effect names the reader's last sequence number, the host keeps only events whose [`Provenance::seq`] is past it. The [`effect`](crate::effect) page gives the exact filter.

Parse events and run events share one provenance space. [`Prompt::parse`](crate::Prompt::parse) stamps its events under task `0` with sequence numbers from zero. A host that logs them in the same stream as the run passes their count to [`RunContext::provenance_start`](crate::RunContext::provenance_start), so every key in the log stays unique.

# Provenance

A [`Provenance`] has two public fields. [`Provenance::task`] is the [`TaskId`] of the task that produced the item. A task id is written as a dot-separated path of numbers, such as `0` for the main walk and `0.1` for a task it started. [`Provenance::seq`] is a [`u32`], the item's position within that task. The counter is local to the task and shared by its effects and its events, so the two kinds order against each other within one task.

This example keeps a small log keyed by provenance, puts it in order, reads one task's items after a known sequence number, and writes one key as JSON and back:

````
use promptforge::ids::{Provenance, TaskId};

let main: TaskId = "0".parse()?;
let worker: TaskId = "0.1".parse()?;

let mut log = vec![
    Provenance { task: worker.clone(), seq: 1 },
    Provenance { task: main.clone(), seq: 4 },
    Provenance { task: worker.clone(), seq: 0 },
];
log.sort();
assert_eq!(log[0], Provenance { task: main, seq: 4 });
assert_eq!(log[1], Provenance { task: worker.clone(), seq: 0 });

let after_first: Vec<&Provenance> = log
    .iter()
    .filter(|key| key.task == worker && key.seq > 0)
    .collect();
assert_eq!(after_first, [&Provenance { task: worker, seq: 1 }]);

let line = serde_json::to_string(&log[1])?;
assert_eq!(line, r#"{"task":"0.1","seq":0}"#);
let back: Provenance = serde_json::from_str(&line)?;
assert_eq!(back, log[1]);
# Ok::<(), Box<dyn std::error::Error>>(())
````

Here is what each part does.

1. **Parse the task ids.** A [`TaskId`] parses from its path text with [`str::parse`]. The [Id text](#id-text) section gives the exact rules.
2. **Sort.** [`Provenance`] orders by task path first, then by sequence number. The main walk's task `0` sorts before its child `0.1`, and within task `0.1`, sequence `0` comes before `1`.
3. **Filter.** Comparing [`Provenance::task`] and [`Provenance::seq`] is all a host needs to slice a log by task. This is the same filter that answers an [`Effect::TaskEvents`](crate::effect::Effect::TaskEvents).
4. **Persist.** A [`Provenance`] serializes as a JSON object with the key `"task"` holding the path string and the key `"seq"` holding a number, and it reads back to an equal value.

**Which task an item reports under.** The main walk reports as task `0`. A `call` child reports under its parent's task. That is unambiguous because a `call` blocks its parent, so the two never interleave. A spawned task reports under its own task id.

**Provenance is stable across runs.** Two runs of the same prompt with the same inputs and answers allocate the same ids and stamp the same provenance on the same items, however their chains interleave, because every counter is local to the chain or task that advances it. The [`EffectId`](crate::effect::EffectId) need not reproduce, so a host matches effects across runs by [`Provenance`].

# Chain ids and task ids

A [`ChainId`] names one chain: the main walk, a `call` child, or a spawned task. The [crate page](crate) defines chains. The engine reports ids as [`TaskId`] values, and a host uses [`ChainId`] to compute or predict ids and to parse them.

````
use promptforge::ids::{ChainId, TaskId};

let root = ChainId::root();
assert_eq!(root.to_string(), "0");

let research = root.child(2);
let nested = research.child(1);
assert_eq!(nested.to_string(), "0.2.1");
assert_eq!(TaskId::from(nested.clone()).to_string(), "0.2.1");

assert_eq!(root.child(3).entry(7), "0.3.7");
assert_ne!(root.entry(3), root.child(3).entry(0));

let mut owned = vec![root.child(10), root.child(0), root.child(9)];
owned.sort();
assert_eq!(owned, [root.child(0), root.child(9), root.child(10)]);
assert!(root < research && research < nested);
````

**The root.** [`ChainId::root`] returns the main walk's id, the single component `0`.

**Children.** [`ChainId::child`] appends one index to a path. A chain keeps one child counter, and `call` children and spawned tasks share it, so if a chain's first two children are a `call` and then a spawned task, they get `0` and `1` under it. Both ids in the example above are built this way: `0.2` is the root's child with index `2`, and `0.2.1` is that chain's child with index `1`.

**Task ids.** A task's id is the same path as the id of the chain that runs it. [`TaskId`] implements [`From`] of [`ChainId`], so `TaskId::from(chain)` converts a chain id into a task id to key a task table. The conversion wraps the path unchanged. The separate type keeps a task-keyed table from accepting an arbitrary chain id by accident. The conversion goes one way only. A [`TaskId`] has no accessor back to its [`ChainId`].

**Section entry ids.** Each time a chain enters a section, the entry gets an id, which the section's Lua reads as `sys.id`. [`ChainId::entry`] computes it by extending the chain's path with the chain's local entry index. It returns a [`String`], not a [`ChainId`], because a section entry is not a chain. The text has the same shape as a chain path and would parse as one, but it names an entry. A parent's entry id and its child chain's entry id never collide, because the paths differ in length.

**Ordering.** Ids sort as paths. A chain sorts before its descendants, and siblings sort by child index as numbers, so `0.9` comes before `0.10`. The tasks owned by one chain are its direct children, so sorting their ids recovers their spawn order. Sort the ids themselves, not their text, because text sorting puts `0.10` before `0.9`.

# Id text

A [`ChainId`] and a [`TaskId`] render with [`Display`](std::fmt::Display) as their decimal components joined by `.`, with no prefix, suffix, or padding: `0`, `0.2`, `0.2.0`, `0.2.1`. A [`TaskId`] renders exactly as its chain id does. Both implement [`FromStr`](std::str::FromStr) with [`ParseIdError`] as the error, so they parse back from that text with [`str::parse`], and a rendered id parses back to an equal value.

Parsing accepts text that follows these rules:

- The text holds one or more components separated by `.`. The empty string is rejected.
- Each component is non-empty and made only of the ASCII digits `0` to `9`. A `-` or `+` sign, or whitespace, is rejected.
- Each component fits in a [`u32`]. So `99999999999` is rejected.
- The first component need not be `0`. A path such as `7.1` parses.
- Leading zeros are accepted and dropped on render. `0.007` parses equal to `0.7` and renders back as `0.7`.

Malformed text fails with a [`ParseIdError`]. [`ParseIdError::input`] returns the exact rejected text, and the [`Display`](std::fmt::Display) message is ``invalid chain id `{input}`: required a dot-separated path of decimal components``. The message says "chain id" even when a [`TaskId`] failed to parse.

````
use promptforge::ids::{ChainId, ParseIdError, TaskId};

let chain: ChainId = "0.12.0".parse()?;
assert_eq!(chain, ChainId::root().child(12).child(0));
assert_eq!(chain.to_string().parse::<ChainId>()?, chain);

let task: TaskId = "0.12.0".parse()?;
assert_eq!(task, TaskId::from(chain));

let padded: ChainId = "0.007".parse()?;
assert_eq!(padded.to_string(), "0.7");
assert!("7.1".parse::<ChainId>().is_ok());

for bad in ["", ".", "0.", ".0", "0..1", "a", "0.-1", "0.+1", "0. 1", "99999999999"] {
    let error: ParseIdError = bad.parse::<ChainId>().err().ok_or("the text is rejected")?;
    assert_eq!(error.input(), bad);
}

let error = "0..1".parse::<TaskId>().err().ok_or("the text is rejected")?;
assert_eq!(
    error.to_string(),
    "invalid chain id `0..1`: required a dot-separated path of decimal components",
);
# Ok::<(), Box<dyn std::error::Error>>(())
````

# Task origins and abandon reasons

A [`TaskOrigin`] says who started a task. [`TaskOrigin::Author`] means the prompt's author started it with `tasks.spawn`. [`TaskOrigin::Model`] means the model started it with its `task` tool. The host reads it from the [`origin`](crate::event::Event#variant.TaskStarted.field.origin) field of [`Event::TaskStarted`](crate::event::Event::TaskStarted).

What happens to a task still running when its owner chain ends depends on its origin. An author task still live when its owner ends normally is the author's bug, and it fails the owner's chain. A model task that outlives its owner is abandoned and reported with [`Event::TaskAbandoned`](crate::event::Event::TaskAbandoned).

An [`AbandonReason`] says how the owner ended while the task was live. The host reads it from the [`reason`](crate::event::Event#variant.TaskAbandoned.field.reason) field of [`Event::TaskAbandoned`](crate::event::Event::TaskAbandoned). Abandoned is kept apart from cancelled, reported as [`Event::TaskCancelled`](crate::event::Event::TaskCancelled), because "lost its owner" and "was stopped on purpose" are different facts for the log, the UI, and the model notice. When the run itself ends, every task still live is abandoned before [`Event::RunSucceeded`](crate::event::Event::RunSucceeded) or [`Event::RunFailed`](crate::event::Event::RunFailed) is reported. A task stranded directly by the run's end gets [`AbandonReason::RunTerminated`], and a task nested under one gets [`AbandonReason::OwnerAborted`].

Neither enum implements [`Display`](std::fmt::Display) or [`FromStr`](std::str::FromStr). For text, use [`TaskOrigin::tag`] and [`TaskOrigin::from_tag`] for an origin, and [`AbandonReason::why`] for a reason. Both enums are `#[non_exhaustive]`, so a `match` on either needs a wildcard arm.

````
use promptforge::ids::{AbandonReason, TaskOrigin};

assert_eq!(TaskOrigin::Author.tag(), "author");
assert_eq!(TaskOrigin::from_tag("model"), Some(TaskOrigin::Model));
assert_eq!(TaskOrigin::from_tag("Author"), None);

fn on_owner_end(origin: TaskOrigin) -> &'static str {
    match origin {
        TaskOrigin::Author => "fails the owner's chain",
        TaskOrigin::Model => "is abandoned and reported",
        _ => "unknown origin",
    }
}
assert_eq!(on_owner_end(TaskOrigin::Model), "is abandoned and reported");

let reason = AbandonReason::ToolLoopExhausted;
let line = format!("task 0.1 was abandoned: {}", reason.why());
assert_eq!(line, "task 0.1 was abandoned: the tool loop was exhausted");
````

# Serde shapes

Every type on this page except [`ParseIdError`] implements serde's [`Serialize`](https://docs.rs/serde/latest/serde/trait.Serialize.html) and [`Deserialize`](https://docs.rs/serde/latest/serde/trait.Deserialize.html). The shapes hold in JSON and in any other serde format. In JSON they look like this:

| Type | JSON shape | Example |
|---|---|---|
| [`ChainId`] | a string holding the path text | `"0.2"` |
| [`TaskId`] | a string holding the path text | `"0.2"` |
| [`Provenance`] | an object with `"task"` as a path string, then `"seq"` as a number | `{"task":"0.2","seq":7}` |
| [`TaskOrigin`] | a lowercase string, the same text as [`TaskOrigin::tag`] | `"author"`, `"model"` |
| [`AbandonReason`] | a snake_case string | `"owner_returned"`, `"owner_failed"`, `"tool_loop_exhausted"`, `"owner_aborted"`, `"run_terminated"` |

An id deserializes by reading a string and parsing it with the rules in [Id text](#id-text). Malformed text such as `"0.x"` fails with a serde error that carries the [`ParseIdError`] message.

````
use promptforge::ids::{AbandonReason, ChainId, Provenance, TaskId, TaskOrigin};

let chain = ChainId::root().child(2);
assert_eq!(serde_json::to_string(&chain)?, r#""0.2""#);
assert!(serde_json::from_str::<ChainId>(r#""0.x""#).is_err());

let task: TaskId = serde_json::from_str(r#""0.2""#)?;
assert_eq!(task, TaskId::from(chain));

let key = Provenance { task, seq: 7 };
assert_eq!(serde_json::to_string(&key)?, r#"{"task":"0.2","seq":7}"#);
assert_eq!(serde_json::from_str::<Provenance>(r#"{"task":"0.2","seq":7}"#)?, key);

assert_eq!(serde_json::to_string(&TaskOrigin::Model)?, r#""model""#);
assert_eq!(serde_json::from_str::<TaskOrigin>(r#""author""#)?, TaskOrigin::Author);
assert_eq!(
    serde_json::to_string(&AbandonReason::RunTerminated)?,
    r#""run_terminated""#,
);
# Ok::<(), Box<dyn std::error::Error>>(())
````

# Reference

This part covers every item in the module, in dependency order: the two id types, the parse error, provenance, and the two task enums. Every method here is infallible and `#[must_use]`.

## ChainId

[`ChainId`] is the hierarchical id of one chain, a path of child indices from the root chain. No other facade item carries one, because the engine reports tasks as [`TaskId`] everywhere. A host uses it to compute or predict ids and to parse them.

The host gets one from [`ChainId::root`] and [`ChainId::child`], by parsing path text with [`str::parse`], or by deserializing it. It has no [`Default`], and its inner path is private.

- [`ChainId::root`] takes no arguments and returns the main walk's id, which renders as `"0"`.
- [`ChainId::child`] takes `&self` and `index`, a [`u32`], and returns a new [`ChainId`] with `index` appended. `self` is unchanged. The `index` is the child's zero-based position in this chain's child counter, which `call` children and spawned tasks share. Any [`u32`] is valid. For example, `ChainId::root().child(2)` is `0.2`.
- [`ChainId::entry`] takes `&self` and `index`, a [`u32`], and returns a [`String`] holding this path with `.{index}` appended. The `index` is the zero-based position of a section entry in this chain's entry counter. The result is the section entry's `sys.id` value in Lua. For example, `ChainId::root().child(3).entry(7)` is `"0.3.7"`. [Chain ids and task ids](#chain-ids-and-task-ids) explains why it returns text.

Trait impls:

- [`Display`](std::fmt::Display) and [`FromStr`](std::str::FromStr) use the path text described in [Id text](#id-text). The parse error is [`ParseIdError`].
- serde uses the same path text as a string, for example JSON `"0.2"`.
- [`Ord`] compares the component lists in order: a chain before its descendants, siblings by child index.

## TaskId

[`TaskId`] is the id of one task, the same path as the id of the chain that runs it. It keys task tables, and its separate type keeps an arbitrary [`ChainId`] out of them.

The host receives one on task events, on effects, and in every [`Provenance`]. It can also build one from a [`ChainId`] through [`From`], parse path text such as `"0.2"` with [`str::parse`], or deserialize one. It has no [`Default`], and there is no accessor back to its [`ChainId`].

A [`TaskId`] appears in these places:

- the task events [`Event::TaskStarted`](crate::event::Event::TaskStarted), [`Event::TaskSucceeded`](crate::event::Event::TaskSucceeded), [`Event::TaskFailed`](crate::event::Event::TaskFailed), [`Event::TaskCancelled`](crate::event::Event::TaskCancelled), [`Event::TaskAbandoned`](crate::event::Event::TaskAbandoned), [`Event::TaskNote`](crate::event::Event::TaskNote), [`Event::TaskNotice`](crate::event::Event::TaskNotice), and [`Event::TaskResumed`](crate::event::Event::TaskResumed)
- [`Effect::TaskEvents`](crate::effect::Effect::TaskEvents) and [`EffectRecord::TaskEvents`](crate::effect::EffectRecord::TaskEvents)
- [`Provenance::task`]

Two of those events are not currently emitted. [`Event::TaskNote`](crate::event::Event::TaskNote) is declared, but the `tasks.note` handler stores the note on the chain without reporting the event. [`Event::TaskResumed`](crate::event::Event::TaskResumed) is reserved, and nothing emits it until resume lands. A host should accept both when it reads a log, but the current engine never sends them.

Trait impls:

- [`From`] of [`ChainId`] wraps the chain id unchanged.
- [`Display`](std::fmt::Display), [`FromStr`](std::str::FromStr), and serde are identical to [`ChainId`]'s, so task `0.2` renders as `0.2` and serializes as JSON `"0.2"`. The main walk is task `0`.
- [`Ord`] orders as the chain id does, as described under [Chain ids and task ids](#chain-ids-and-task-ids).

## ParseIdError

[`ParseIdError`] is the error returned when text fails to parse as a [`ChainId`] or [`TaskId`]. The host receives it from [`str::parse`] and never builds one. [Id text](#id-text) lists what is rejected.

- [`ParseIdError::input`] takes `&self` and returns the exact rejected text as a [`&str`](str), for example `"0..1"`. Use it to report which value was bad.

It implements [`Display`](std::fmt::Display) with the message ``invalid chain id `{input}`: required a dot-separated path of decimal components``, the same for both id types. It implements [`std::error::Error`] with no [`source`](std::error::Error::source). It has no serde and no [`Default`], and its `input` field is private.

## Provenance

[`Provenance`] is the replay key stamped on every effect and event: the nearest enclosing task plus the item's position within that task. The host receives it from [`Event::provenance`](crate::event::Event::provenance) and as the middle element of each tuple in [`Step::Pending::effects`](crate::Step#variant.Pending.field.effects). It can also build one with a struct literal, because both fields are public, or deserialize one. It has no [`Default`].

- [`Provenance::task`], a [`TaskId`], is the task whose chain produced the item. The main walk is task `0`, and a `call` child reports under its parent's task.
- [`Provenance::seq`], a [`u32`], is the item's position among the task's effects and events. The counter is local to the task and shared by both kinds. Parse events start at `0` under task `0`, and [`RunContext::provenance_start`](crate::RunContext::provenance_start) moves the run's root counter past them.

Trait impls:

- serde gives a JSON object with its keys in declaration order, `"task"` as the path string, then `"seq"` as a number: `{"task":"0.2","seq":7}`.
- [`Ord`] compares [`Provenance::task`] first, then [`Provenance::seq`]. So task `0.2` at sequence `7` sorts before task `0.3` at sequence `0`.

## TaskOrigin

[`TaskOrigin`] is the principal that started a task. The host receives it in [`Event::TaskStarted::origin`](crate::event::Event#variant.TaskStarted.field.origin). It can also name a variant, convert a tag with [`TaskOrigin::from_tag`], or deserialize one. It is `#[non_exhaustive]` and has no [`Default`].

- [`TaskOrigin::Author`]: the prompt's author started the task with `tasks.spawn`, or with `fanout` over it. If the task is still live when its owner ends normally, the owner's chain fails with the `tasks_live` error. Treat that failure as a defect in the prompt.
- [`TaskOrigin::Model`]: the model started the task with its `task` tool. If the task outlives its owner, it is abandoned and reported with an [`AbandonReason`], and the owner's chain does not fail.
- [`TaskOrigin::tag`] takes `self` and returns the tag as a [`&'static str`](str): `"author"` for [`TaskOrigin::Author`] and `"model"` for [`TaskOrigin::Model`]. The Lua shims and the `tasks.pending` filter use these strings.
- [`TaskOrigin::from_tag`] takes `tag`, a [`&str`](str) such as one read from a filter or a config file, and returns an [`Option`] of [`TaskOrigin`]. It returns [`Some`] for exactly `"author"` or `"model"`, and [`None`] for anything else. Matching is case-sensitive, so `"Author"` gives [`None`].

It serializes as a lowercase string, the same text as [`TaskOrigin::tag`]. It has no [`Display`](std::fmt::Display) and no [`FromStr`](std::str::FromStr).

## AbandonReason

[`AbandonReason`] says how a task's owner ended while the task was still live. The host receives it in [`Event::TaskAbandoned::reason`](crate::event::Event#variant.TaskAbandoned.field.reason) and never builds one, though it can deserialize one. It is `#[non_exhaustive]` and has no [`Default`]. In every case the host logs the event, and it can show [`AbandonReason::why`] to a person.

- [`AbandonReason::OwnerReturned`]: the owner ended normally, with a scalar return or an exhausted walk, without waiting on or cancelling the task. For an author task this case is the `tasks_live` error, and a model task is abandoned quietly. It serializes as `"owner_returned"`.
- [`AbandonReason::OwnerFailed`]: the owner chain failed while the task was live. It serializes as `"owner_failed"`.
- [`AbandonReason::ToolLoopExhausted`]: the owner's model and tool loop ran past its round cap. It is a failure kept apart from [`AbandonReason::OwnerFailed`] because the model notice must say that the model's own task outlived the loop that started it. It serializes as `"tool_loop_exhausted"`.
- [`AbandonReason::OwnerAborted`]: the owner was aborted from outside, by a fatal sibling's fail-fast or by its own owner ending first. It serializes as `"owner_aborted"`.
- [`AbandonReason::RunTerminated`]: the run itself ended while the task was live, because the host cancelled it or supplied a fatal answer. It serializes as `"run_terminated"`.

[`AbandonReason::why`] takes `self` and returns a [`&'static str`](str) phrase for a trace line or notice:

- [`AbandonReason::OwnerReturned`] gives `"the section ended"`.
- [`AbandonReason::OwnerFailed`] gives `"the owner failed"`.
- [`AbandonReason::ToolLoopExhausted`] gives `"the tool loop was exhausted"`.
- [`AbandonReason::OwnerAborted`] gives `"the owner was aborted"`.
- [`AbandonReason::RunTerminated`] gives `"the run ended"`.

The engine's model-facing notice uses the same phrase, as `Task id={task} (## {target}) was abandoned: {why}`. [`AbandonReason`] has no [`Display`](std::fmt::Display) and no [`FromStr`](std::str::FromStr).

