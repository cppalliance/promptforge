The events a run reports for its host to log.

# Reporting as values

A run reports itself as it goes, as values. Every boundary - the run's start and end, each section, model turn, tool call, and store operation - becomes an [`Event`] in the run's event buffer, stamped with the [`Provenance`](crate::ids::Provenance) of the chain that reported it: its nearest enclosing task and that task's next sequence number. Every [`Run::step`](crate::Run::step) drains the buffer and returns the batch to the host, which appends it to its log.

Reporting is a side channel and never a decision. Nothing the run does depends on who reads its events: recording every event or dropping them all leaves a run's outputs, errors, and ordering unchanged. A host that needs to know whether the outcome is settled asks [`Run::decided`](crate::Run::decided) rather than watching for an end event.

# Coordinates

Every variant holds three coordinates ahead of its payload, readable without matching on the variant through [`Event::execution`], [`Event::section`], and [`Event::provenance`]:

- `execution`: the caller-chosen run identifier, the name given to [`RunContext::new`](crate::RunContext::new).
- `section`: the reporting scope, the prompt's H2 heading text or an agent's name.
- `provenance`: the replay key. A host writes a record's task and sequence columns from it alone.

# Kinds of event

- Lifecycle events mark operational boundaries: parsing, the run, sections, model turns, tool calls, the section VM's phases, scope and catalog validation, store operations, and input waits. Most hold nothing beyond their coordinates.
- Task events report a task chain's start, its resumption, and its end: succeeded, failed, cancelled, or abandoned.
- Content events hold what a model, tool, or user produced: thinking, an [`AssistantReply`](Event::AssistantReply) with its [`ReplyOrigin`] and [`CallMetrics`](crate::metrics::CallMetrics), the model's requested tool calls, a tool's result, the operator's input, and task notices and notes.
- Debug events hold a model round's raw request and response bodies. A run reports them only when its context asks with [`DebugMode::On`] through [`RunContext::report_debug`](crate::RunContext::report_debug); the bodies already travel in the `Chat` effect and its answer, so a host that logs effects has them either way.

[`Event`] is non-exhaustive, so a host keeps a wildcard arm for kinds a later engine adds. [`ReplyOrigin`] tells a chat turn's reply, which belongs in the conversation, from a programmatic inference round's.

# Sensitivity

Lifecycle events hold only their coordinates, and those are author-controlled: the execution name is the host's, and the section is prompt-authored heading text. The exception is a degraded model-metadata report, whose message may quote values from a backend's response. Content events hold model-, tool-, or user-authored text, task events hold the author's spawn seeds, and debug events hold verbatim request and response bodies. A host that persists or forwards events treats all of it as untrusted.

# Serialized form

One event serializes to one JSON object tagged by `kind` (the variant name in snake case), with the three coordinates and then the payload fields beside it:

```
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
```
