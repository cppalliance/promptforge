PromptForge runtime core.

This crate holds the pieces that turn a prompt markdown file into a run: the [`parser`] that reads the file into a [`parser::Prompt`], and the [`Run`] state machine, which runs H1 once before walking sections top to bottom (fall-through), issuing every model round, tool call, input wait, store operation, and timer as an [`Effect`] value the host performs and answers, and reporting every boundary as an [`Event`](promptforge_types::event::Event) value the host logs. The engine performs no I/O and reads no clock; the harness is its production host and reaches it through the `promptforge` facade. The host-facing vocabulary a run is configured with - the model and tool catalogs, the tool contract, the event enum - sits in the `promptforge-types` crate; the chat vocabulary a `Chat` effect holds and its answer returns is [`model`]. The store handle a host seeds or extracts comes from `shared-vfs` and `promptforge-vfs`. No model client is defined here: the harness owns the transport that performs a round and reaches the vocabulary through the facade.

A source is a promptforge prompt only when its frontmatter declares a `promptforge:` version; [`promptforge_version`] reports it (or `None`), and the runtime refuses a source that lacks a supported version.

# Examples

Detect a promptforge source and parse it into a [`Prompt`]:

```
use promptforge_engine::{Prompt, promptforge_version};

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
# Ok::<(), promptforge_engine::ParseError>(())
```

Executing a parsed prompt builds a [`Run`] over a [`RunContext`] prepared by an [`Environment`] (which holds the host roots and the catalog of tools the host activated); the store handle sits on the context, defaulting to the stock in-memory mount. The host then loops: [`Run::step`] returns the effects to perform and the events to log, and [`Run::resume`] hands each effect's answer back.
