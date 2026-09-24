PromptForge API: the one crate a host depends on to parse prompt files and drive their runs.

A host parses a source into a [`Prompt`], prepares a [`RunContext`] through an [`Environment`], and drives the [`Run`] state machine. The run performs no I/O, reads no clock, and holds no host trait objects: it asks for work as [effects](effect) and reports what happened as [events](event), and the host performs, answers, and logs. Every item here is re-exported from the engine's private crates, and each has exactly one path.

# Contents

## Running a prompt

- This page: [`Prompt`] and the [`ParseError`] a parse fails with; the [`Environment`] whose [`prepare`](Environment::prepare) fills a [`RunContext`] and reports the [`Requirements`] the host must still meet; the [`Run`], the [`Step`] each [`Run::step`] returns, and the [`RunResult`] a run ends with, whose failure is a [`RunError`] classified by [`RunErrorKind`]; and the [`RunLimits`] a run honors.
- [`prompt`]: what a prompt declares in its frontmatter - its [`Frontmatter`](prompt::Frontmatter), args, files, capabilities, model roles, and tool slots.
- [`cancel`]: the [`CancelHandle`](cancel::CancelHandle) a host cancels a run through.
- [`timestamp`]: the [`Timestamp`](timestamp::Timestamp) a run starts from.

## Performing effects

- [`effect`]: the [`Effect`](effect::Effect) a run asks for, the [`EffectAnswer`](effect::EffectAnswer) a host returns, and the records a log stores for both.
- [`model`]: what a model round exchanges - [`Message`](model::Message), [`ToolSchema`](model::ToolSchema), [`Completion`](model::Completion) - and the catalog and bindings a run resolves models through.
- [`transport`]: the sans-I/O codec a host performs a model round with.
- [`tools`]: the [`ToolDescriptor`](tools::ToolDescriptor)s a run binds against, the [`ToolId`](tools::ToolId) a tool call names, and the [`ToolOutput`](tools::ToolOutput) or [`ToolError`](tools::ToolError) it is answered with.
- [`input`]: what a user-input wait is answered with.
- [`vfs`]: the virtual filesystem a run's store lives in, how a host mounts it, and how a host performs a store effect.

## Recording a run

- [`event`]: the [`Event`](event::Event) values a run reports.
- [`ids`]: the identities of a run's chains and tasks, and the [`Provenance`](ids::Provenance) replay key on every effect and event.
- [`metrics`]: the model-call metrics a reply event holds.
- [`replay`]: the behavior [`Flags`](replay::Flags) a run records.

## Naming

- [`capabilities`]: capability identities and the global naming grammar that capability and tool ids share.

# The host loop

A [`Run`] is a deterministic state machine over one prompt. The host calls [`step`](Run::step), which drains every chain that can make progress and returns [`Step::Pending`] with the leaf effects those chains issued, each stamped with the [`Provenance`](ids::Provenance) of the task that built it, beside the events the step reported. The host performs the effects however it likes and hands each answer back through [`resume`](Run::resume), one call per arriving answer, then steps again. `step`, `resume`, and [`cancel`](Run::cancel) are infallible by design: a run's failures are values in [`RunResult::Failure`], so the host owns the loop and the retry policy without catching a panic.

[`Step::Done`] is withheld while any issued effect is unanswered, so a host that has answered every effect it was handed - a drop counts - can rely on the run's end being the end of every effect too. An effect a chain stopped waiting for (its task was cancelled or abandoned) still wants its one answer; the run discards it on arrival. After a `Pending` step, [`Run::decided`] tells the host the outcome is settled and anything still out may be dropped, so control never depends on reading the events.

A prompt whose section writes and reads its store issues two [`Store`](effect::Effect::Store) effects, which the host performs with [`perform_store_op`](vfs::perform_store_op):

```
use std::sync::Arc;

use promptforge::effect::{Effect, EffectAnswer};
use promptforge::timestamp::Timestamp;
use promptforge::vfs::perform_store_op;
use promptforge::{Prompt, Run, RunContext, RunResult, Step};

let source = "---\nname: notes\ndescription: keeps a note\npromptforge: 0\n---\n\n# Notes\n\n## Save\n\n```lua\nstore.write('todo.md', 'ship it')\nreturn store.read('todo.md')\n```\n";
let (prompt, _parse_events) = Prompt::parse(source, "host-loop");
let ctx = RunContext::new("host-loop", 7, Timestamp::UNIX_EPOCH);
let mut run = Run::new(Arc::new(prompt?), "", ctx);
let mut log = Vec::new();
let result = loop {
    match run.step() {
        Step::Pending { effects, events } => {
            log.extend(events);
            for (id, _provenance, effect) in effects {
                let answer = match effect {
                    Effect::Store { access, op } => EffectAnswer::Store(perform_store_op(&access, op)),
                    // This prompt issues nothing else; a real host performs every kind.
                    _ => EffectAnswer::Dropped,
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
let RunResult::Ok(text) = result else {
    panic!("the store round trip succeeds: {result:?}");
};
assert_eq!(text, "ship it");
assert!(!log.is_empty());
# Ok::<(), Box<dyn std::error::Error>>(())
```

# How a run walks a prompt

A run executes the prompt's H1 once, then walks its top-level sections in file order, creating one isolated section VM for each. The VM is fully equipped (host values, store, log, control globals) before the prompt's shared Lua library replays as the section's first chunk, and the section's blocks then run in order in that same VM. Prose never infers: each prose block stashes the pending Markdown, and the next Lua block reads it as its fresh read-only lazy `prose` template. A scalar Lua return ends the chain it fires in.

Running off the last section ends the run: the result is the last scalar return, else a generic completion.

The walk is level-independent and descends only on a jump. Lua `jump(target)` transfers control to a named section; a jump to a child heading starts a child-level walk over the jumper's children under the same rules, and the parent walk resumes after the jumper when that level is exhausted.

Lua `call()` starts a contained chain at a visible section, in a fresh VM, with recursion capped at 8. The chain runs from its target under every normal walk rule - fall-through, jumps, child chains - and the outer walk never moves while it runs. When the chain ends, because its level is exhausted or a return fires, its final text is the call's return value; a return ends only the chain it fires in.

Section Lua state never survives a section, but the run's store does: one store handle, set on the [`RunContext`], is shared by every section, so bulk state persists across the transitions that clear a section's context. The [`vfs`] module covers how a host mounts and extracts it.

# Determinism

Given the same context and the same sequence of answers, a run produces the same effects, events, and ids. The run takes its randomness and its clock from the host: the seed and the start instant are inputs to [`RunContext::new`], so a host that records both and replays the recorded answers reproduces the run. Every effect and event is keyed by its [`Provenance`](ids::Provenance), which is stable across runs however their chains interleave; the [`EffectId`](effect::EffectId) a host correlates an answer with is an opaque run-wide handle that need not reproduce.

# Concurrency without a runtime

The engine needs no async runtime of its own. Section Lua yields request values to the run's chain-stack scheduler, which turns each leaf request into an effect for the host, so the host's loop performs effects on whatever executor it likes, or on the calling thread. Concurrency, such as a fanout's arms, comes from interleaving chains at their effect boundaries rather than from worker threads: one `step` can hand out several effects at once, and the host may perform them in parallel and resume them in any order.
