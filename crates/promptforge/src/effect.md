The effects a run issues, the answers a host returns, and the records a log stores for both.

A leaf request a section makes - a model round, a bound tool call, a wait for operator input, a store operation, a timer, a read of a task's history - is not performed where the prompt makes it. The run builds an [`Effect`], a plain description of the work, and returns it from [`Run::step`](crate::Run::step) under an [`EffectId`]; the host performs it and hands the [`EffectAnswer`] back through [`Run::resume`](crate::Run::resume) under the same id. The engine decides what to do and what the answer means; performing is the host's job.

# Performing each kind

- [`Effect::Chat`]: one model round over its messages, with its tool schemas advertised, under the binding's frozen options. Answer with [`EffectAnswer::Chat`] holding the [`Completion`](crate::model::Completion) or the [`CompletionError`](crate::model::CompletionError); the [`transport`](crate::transport) module shows how to build one. Its `stream` flag says whether the host forwards the round's live deltas to whoever watches the reply.
- [`Effect::ToolCall`]: one bound tool call, naming the tool's stable [`ToolId`](crate::tools::ToolId) and the prompt-local alias it was called by. The host resolves the id to its own implementation and answers with [`EffectAnswer::ToolCall`] holding the tool's [`ToolOutput`](crate::tools::ToolOutput) or [`ToolError`](crate::tools::ToolError); the engine applies its trust and count rules afterward.
- [`Effect::UserInput`]: one wait for operator input. Answer with [`EffectAnswer::UserInput`]; the [`input`](crate::input) module covers the outcomes.
- [`Effect::Store`]: one store operation under the chain's access capability. Answer with [`EffectAnswer::Store`], usually by running [`perform_store_op`](crate::vfs::perform_store_op). The host uses the capability as given and never derives, widens, or retains store scope from it.
- [`Effect::Timer`]: one sleep, the timeout behind a timed wait. Answer with [`EffectAnswer::Timer`] when it fires.
- [`Effect::TaskEvents`]: one read of a task's reported history. Answer with [`EffectAnswer::TaskEvents`] from the host's own log: every event whose provenance names the task with a sequence number after the reader's `last`, in order. A host that commits a step's events before performing its effects gives a reading task everything reported before the read was issued.

Each answer must match the kind of the effect it answers; a mismatch, an answer for an id the run never issued, or a second answer for one effect is an internal error that ends the run.

# One answer per effect

Every issued effect receives exactly one answer. [`EffectAnswer::Dropped`] is the answer for an effect the host gave up on - a cancelled run, or an effect whose task ended first - and it counts like any other: a chain still waiting resumes with a cancelled error, and [`Step::Done`](crate::Step::Done) arrives only once every effect has its answer.

# Records

An [`Effect`] may hold a live handle (the store capability), and an [`EffectAnswer`] may hold values a log cannot keep whole (a completion's bodies, an error's boxed cause), so neither serializes itself. [`Effect::record`] projects an effect onto its [`EffectRecord`], the request minus its handles, and [`EffectAnswer::record`] projects an answer onto its [`AnswerRecord`], the outcome with every failure rendered as text. Both records round-trip through serde: a run log stores them, and a replay compares a re-executed run's records against them. A completed round records as a [`ChatAnswerRecord`], a tool's output as a [`ToolAnswerRecord`], an input outcome as an [`InputAnswerRecord`], and a store outcome as a [`StoreAnswerRecord`].

```
use promptforge::effect::{AnswerRecord, Effect, EffectAnswer, EffectRecord};

let effect = Effect::Timer { seconds: 0.5 };
assert_eq!(effect.record(), EffectRecord::Timer { seconds: 0.5 });
assert_eq!(EffectAnswer::Dropped.record(), AnswerRecord::Dropped);
```
