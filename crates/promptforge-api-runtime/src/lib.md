PromptForge runtime core.

This crate holds the pieces that turn a prompt markdown file into a run: the [`parser`] that reads the file into a [`parser::Prompt`], and the [`Run`] state machine, which runs H1 once before walking sections top to bottom (fall-through), issuing every model round, tool call, input wait, store operation, and timer as an [`Effect`] value the host performs and answers, and reporting every boundary as an [`types::event::Event`] value the host logs. The engine performs no I/O and reads no clock; the harness is its production host. The host-facing vocabulary a run is configured with - the model and tool catalogs, the tool contract, the event enum - sits in the `promptforge-api-types` crate, re-exported here as [`types`] (`types::event`, `types::models`, `types::tools`), so a host depends on this one crate alone; the chat vocabulary a `Chat` effect holds and its answer returns is [`model`]. The store handle a host seeds or extracts comes from `shared-vfs` and `promptforge-vfs`. No model client is defined here: the harness owns the transport that performs a round and reaches the vocabulary through this crate.

A source is a promptforge prompt only when its frontmatter declares a `promptforge:` version; [`promptforge_version`] reports it (or `None`), and the runtime refuses a source that lacks a supported version.

# Examples

Detect a promptforge source and parse it into a [`Prompt`]:

```
use promptforge_api_runtime::{Prompt, promptforge_version};

let source = "---\nname: greeter\ndescription: says hi\npromptforge: 0\n---\n\n# Greeter\n\n## Say hi\n\nSay hello.\n\n```lua\nreturn models.infer(prose)\n```\n";

// Version detection gates whether the runtime will accept the source.
assert_eq!(promptforge_version(source), Some(0));
assert_eq!(promptforge_version("plain text, no frontmatter"), None);

// A parse returns its parse-time events beside the outcome.
let (prompt, events) = Prompt::parse(source, "doc-example");
let prompt = prompt?;
assert!(!events.is_empty());
assert_eq!(prompt.title(), "Greeter");
assert_eq!(prompt.sections()[0].name(), "Say hi");
# Ok::<(), promptforge_api_runtime::ParseError>(())
```

Executing a parsed prompt builds a [`Run`] over a [`RunContext`] prepared by an [`Environment`] (which holds the host roots and the catalog of tools the host activated); the store handle sits on the context, defaulting to the stock in-memory mount. The host then loops: [`Run::step`] returns the effects to perform and the events to log, and [`Run::resume`] hands each effect's answer back. A prompt that issues no effect is done in one step:

```
use std::sync::Arc;

use promptforge_api_runtime::types::timestamp::Timestamp;
use promptforge_api_runtime::{Environment, Prompt, Run, RunContext, RunResult, Step};

let source = "---\nname: greeter\ndescription: says hi\npromptforge: 0\n---\n\n# Greeter\n\n## Say hi\n\n```lua\nreturn 'hello'\n```\n";
let (prompt, _parse_events) = Prompt::parse(source, "run-example");
let prompt = prompt?;

// Capability-free agents use the default environment: an empty catalog.
// The host draws the run's seed and stamps its start: the engine reads
// neither the OS RNG nor the clock.
let env = Environment::new();
let seed: u64 = 0x5eed; // a CSPRNG draw in a real host
let started_at = Timestamp::from_unix_millis(1_700_000_000_000);
let (ctx, requirements) = env.prepare(&prompt, RunContext::new("run-example", seed, started_at));
assert!(requirements.is_satisfied());
let mut run = Run::new(Arc::new(prompt), "", ctx);
let Step::Done { result: RunResult::Ok(text), .. } = run.step() else {
    panic!("the greeter run is done in one step");
};
assert_eq!(text, "hello");
# Ok::<(), Box<dyn std::error::Error>>(())
```
