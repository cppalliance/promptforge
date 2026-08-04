# `promptforge-core`: a library that runs one markdown prompt file through a model and returns a string

## Executive summary

This crate is a library with no binary. It parses one markdown prompt file, walks its top-level sections in file order, runs each section's Lua block, substitutes its prose, takes one model round trip per section with a tool-call loop, and returns a single `String`. A run is the async free function `execute::run(prompt, args, tools, store, opts)`; there is no executor object, no configuration file, and nothing that outlives the call.

Control flow is fall-through and nothing else. Sections run in the order they appear, the context is destroyed at every boundary, and the one early exit is a Lua chunk returning a value at top level, which no model can reach. Child sections parse and never execute. Running off the last section ends the run with the frontmatter's `default_return`, else the last model reply, else `"done"`.

Three things cross a section boundary and none of them is a conversation: the run's `store`, which is a run-scoped virtual filesystem exposed to Lua and to no model; the observer's view of what happened; and the last model reply the run has produced, retained as one string so that a run falling off the end has something to return. Everything else - the model's messages, the section's `var` table, its scoped tools - is rebuilt from nothing on entry.

The caller supplies the whole tool pool and each section scopes a subset of it by name from its own Lua, so the model is never shown a tool the prompt author did not name for that section. A scoped name with no matching tool is a hard error rather than a silent drop, and a tool that declares its output untrusted has that output wrapped in a nonce-tagged guard block before it enters the conversation.

What is not here matters as much: no slots, no extensions, no declared outputs, no `goto`, `Task`, or fan-out, no preconditions or postconditions, no structured run state a model can file into, no validation phase, and no persistence of any kind past the end of a run.

## The key design choices

1. **A run is a free function and its options are two fields.** `async fn execute::run(prompt, args, tools, store, opts) -> Result<String>` with `RunOptions { observer, client }`. There is no executor type to construct, because nothing about a run outlives it: the prompt, the pool, and the store are all the caller's, and the two remaining knobs are where progress goes and which gateway is used. Growing this later is additive - a field on `RunOptions`, or a builder over it - which is why the positional list was acceptable in the first place.

2. **Control flow is fall-through over top-level sections, and the only early exit is a Lua top-level `return`.** A section's chunk returning a scalar makes that scalar the run's result and ends the run there. The exit is reachable only from the prompt author's own Lua: no tool ends a run, so no model can. File order is therefore the entire control-flow graph, there is nothing to validate at boot, and a prompt reads top to bottom. The cost is that an author cannot express a branch, and reversing this - declared exits, a jump, a subagent call - is the change that would give this crate a control-flow graph and a boot check to walk it.

3. **The context is destroyed at every section boundary and the store is what survives.** Nothing of the model's conversation, and nothing of the section's `var` table, reaches the next section. A section that needs to hand bulk content forward writes it to the store and the next one reads it back. This is what keeps a long prompt's context from accumulating, and it is why the store is a caller-created handle threaded through the whole run rather than something a section makes for itself.

4. **A source is a promptforge prompt only when its frontmatter declares `promptforge:`, and detection is lenient where execution is strict.** `promptforge_version(source) -> Option<u32>` never errors: no frontmatter, unclosed frontmatter, invalid YAML, and an absent key all read as `None`, so a caller can ask "is this one of mine" of an arbitrary file on disk. `execute::run` then gates hard on the same key before doing any work. Two readings of one fact, deliberately: the cheap question a file walker asks, and the refusal a runner owes.

5. **The run's input is one raw string, not a validated object.** `args` reaches Lua as a global and prose as `{{ args }}`, uninterpreted. A prompt wanting structure parses it in its own Lua. The alternative, a JSON Schema in frontmatter validated before the run, is a real feature and its absence is felt at exactly one place: a malformed input fails somewhere inside the prompt rather than at its edge.

6. **Substitution resolves `args`, `var`, and `sys`, in one pass, and a missing path fails the run.** `{{ var.<path> }}` is what the section's own Lua block just wrote, read back as JSON; `{{ sys.<path> }}` is runtime metadata. There is no recursion and no arithmetic, because a template language inside a prompt is a second programming language competing with the Lua block directly above it. An unresolvable path is `Error::Substitution` naming the path rather than an empty string, since a prompt silently missing a value produces confident output about nothing.

7. **Tool scoping is opt-in per section, and a scoped name with no tool is an error.** The caller passes the whole pool; a section advertises only what its Lua named with `tools.add(...)`, and a section with no Lua block advertises nothing. Opt-in rather than opt-out is the tool-count discipline made structural: the model's choice stays small by default rather than by an author remembering to narrow it. `Error::UnknownScopedTool` rather than a silent drop, because the failure it prevents - a typo quietly removing the tool a section was written around - is invisible in the output.

8. **`store` is a run-scoped virtual filesystem, and the name is worth stating precisely.** Six operations: `write`, `append`, `read`, `str_replace`, `delete`, `glob`. It holds text keyed by logical path. It is not structured run state, it holds no records, and nothing queries it by collection or key. It is exposed to the Lua VM and to nothing else: there are no model-facing file tools, so a model cannot read or write a virtual file at all. Text keyed by path rather than records queried by collection is the choice, and structured run state - typed collections a prompt files into and queries back - is the alternative that lost: what a section actually needs to hand forward is the prose it just produced, and a schema over that prose would have to be declared before a prompt author knows what shape it wants, while a path is a name an author picks as they go. The cost of that generality is that nothing can be asked of the store except by path, so a prompt wanting to select across what it wrote scans and parses in its own Lua. Reversing this is expensive in one direction only: adding a queryable state layer beside these six operations is additive, but taking the filesystem away would rewrite every prompt that carries anything between sections, since this is the sole channel for it.

9. **Store reads are numbered lines and store edits are anchored and unique.** `read` returns each line prefixed with its 1-based number right-aligned to the width of the highest, then `"| "`, which is for navigation and error messages and is not a wire format. `str_replace` requires its anchor to occur exactly once: zero matches is `AnchorNotFound` and more than one is `AnchorAmbiguous` carrying the count, so an edit never lands on an arbitrary match. Both shapes exist because the eventual editor of these files is a model, and an offset-based edit against a numbered read is the pairing that goes wrong silently.

10. **An untrusted tool's result is wrapped before it enters the conversation, and the delimiter is unguessable.** `Tool::untrusted_output` defaults to `false`; a tool that returns `true` has its result framed by a sentence saying the enclosed text is data to analyze rather than instructions to follow, then `<untrusted_input_{nonce}>` tags. The nonce is one random `u64` in hex, generated once per section, and it lives in the tag name rather than in an attribute so the closing delimiter cannot be guessed. Any literal occurrence of either tag in the content is defanged by replacing its leading `<` with `&lt;`, so fetched content cannot forge the close and break out.

11. **The sandbox is a hand-maintained blocklist over `lua54`, with an instruction budget and no memory ceiling.** `mlua` is built with `lua54`; only `string`, `table`, and `math` are loaded, and twelve base globals are then set to nil by hand. A hook fires every 10,000 instructions and aborts after 1,000 firings, so a block gets roughly ten million. Neither number is measured: both are first cuts chosen to be generous enough that no plausible prompt reaches them, and a reader who cannot tell a calibrated default from a guess will trust both equally. This is the one security-shaped decision in the crate and it is the weakest: a blocklist is only as good as its list, an allowlist-by-language-design engine is the alternative, and nothing has recorded a decision either way. A block can currently exhaust memory where it cannot exhaust time.

12. **A fresh VM per section, not one per run.** Nothing a Lua block defines survives into the next section, which is the same clearing the conversation gets and for the same reason. The cost is per-section VM construction and the loss of any Lua-level carry-forward, which the store covers.

13. **The observer is synchronous, is a report and never a decision, and serializes one way.** `on_event` sits on the run's own path and must not block, await, or perform I/O; an implementation that forwards elsewhere queues and returns. Dropping every event leaves the run's result unchanged, which is what lets `NullObserver` be what a caller wanting silence passes rather than an `Option<&dyn Observer>` the executor has to branch on. `Event` derives `Serialize` and not `Deserialize`, because nothing in this crate reads an event back.

14. **`RunStarted::sections` is a bound rather than a prediction, and it is documented as one.** It counts the top-level sections the prompt declares. An early return means the run visits fewer, so a client rendering a fraction from it is rendering a ceiling. A denominator that can only be an over-estimate is the honest version of the number a progress bar wants, and the alternative of emitting none at all leaves a client with no way to size anything.

15. **One error enum for parsing, transport, and execution, with the transport source boxed.** `Error` is `#[non_exhaustive]`, built with `thiserror`, and its `Http` variant boxes its source so no dependency's error type reaches the public API and a change of HTTP client is not a breaking change. There is no split between a parse error, a validation error, and a run error because there is no validation step to own the middle one. `StoreError` is separate and also `#[non_exhaustive]`, since a virtual-file failure is a caller's to handle at a different granularity.

## The boundary is one library wide, and the caller owns everything that crosses it

Types are reached through public modules rather than a flat re-export: `promptforge_core::client`, `execute`, `lua`, `observe`, `parser`, `store`, `subst`, and `tools`. The crate root re-exports only `Error`, `Result`, and `promptforge_version`. The module path is therefore part of the public surface and moving a type between modules is a breaking change, which is the price of a reader being able to see from a `use` line which part of the runtime a type belongs to.

What it does not do today, none of it a boundary drawn on purpose and all of it simply unbuilt: no executor type, no slot or tool resolution maps, no extension mechanism, no declared outputs, no `goto`, `Task`, or fan-out, no preconditions or postconditions, no per-run limits beyond the tool-loop cap and the Lua instruction budget, no structured run state, and no persistence of any kind past the end of a run.

Two things it does do that a library described as knowing nothing about its deployment would not. It reads three environment variables - `PROMPTFORGE_TOKEN`, `PROMPTFORGE_BASE_URL`, and `PROMPTFORGE_MODEL` - when a caller passes no gateway client, through `GatewayClient::from_env`, which `execute::run` calls itself on the first section that needs a model. And it contains a search tool: `tools::web_search::WebSearch`. Neither holds a vendor credential, since the gateway holds those, but both are in this crate and a caller cannot opt out of either by construction.

## The engine version gate is separate from the author's own version

A source is a promptforge prompt only when its frontmatter declares a `promptforge:` key. `promptforge_version(source) -> Option<u32>` reports it and never errors.

`execute::run` gates on it before doing any work. The supported major is 1. Another major is `Error::UnsupportedVersion`; no version at all is `Error::Parse("not a promptforge prompt: no promptforge version")`. A run refused by the gate emits no observer events, because it never started.

The gate is distinct from `Frontmatter::version`, which is the author's own contract number for the prompt's interface and is a `u32` the crate reads and does nothing with.

## Parsing is total and produces no side effects

`Prompt::parse(&str) -> Result<Prompt>` turns bytes into an inert tree. A `Prompt` carries the parsed `Frontmatter`, the first H1's text as `title`, the prose between that H1 and the next heading as `description_text`, and the top-level sections. `Prompt::entry()` returns the first top-level section, whatever it is called; there is no privileged section name and a test asserts exactly that.

`Frontmatter` has seven fields and does not deny unknown ones, so a key it does not name is read past in silence. Three are required - `name`, `description`, and `version: u32` - and four default: `promptforge: Option<u32>`, `tools: Vec<String>`, `default_return: Option<String>`, and `max_tool_iterations: Option<usize>`. `tools` is parsed and never read by this crate; the CLI and the MCP server are the two callers that consume it to decide which tools to bind for a run.

A `Section` carries its heading text as `name` (the address, without the `##` marker), a numeric `level` from 2 through 6, an optional `lua` block, its `prose`, and its `children`. Nesting is recursive through H6, and a skipped level is tolerated: an H4 directly under an H2 becomes a child of that H2.

A section's Lua is exactly one code fence, tagged `lua`, appearing first in the section's content. A fence in any other language, a fence that is not first, and an unterminated fence all stay in the prose. Parsing fails on a missing opening `---`, an unclosed frontmatter block, YAML that does not deserialize, and a body with no `##` sections. A leading byte-order mark is stripped.

## The run falls through the top-level sections and clears the context between them

`execute::run(prompt, args, tools, store, opts)` returns one `String`. `args` is a single raw input string, not an object and not schema-validated. `tools` is the run's whole pool. `store` is the run's virtual-file handle, created once by the caller and threaded through every section. `opts` is `RunOptions { observer, client }`.

Each top-level section, in file order:

1. Its Lua chunk runs. If the chunk returns a value at top level, that value is the run's result and the run ends there - this is the return fence, and it is the only early exit. Otherwise the executor reads back the `var` table and the names the block passed to `tools.add`.
2. Those names are resolved against the pool. A name with no matching tool is `Error::UnknownScopedTool`, never a silent drop. The resolved subset is the only thing this section shows to the model and the only thing it can dispatch; a section with no Lua block, or one that never calls `tools.add`, advertises nothing.
3. The prose is substituted. If what remains is not blank, the section takes one tool-call loop against the gateway.
4. Control falls through to the next top-level section with the context cleared. Nothing of the section's own conversation or its `var` table crosses the boundary; the store, the run's last model reply, and `sys.when` are what the next section still sees.

Child sections are parsed and are never executed. Running off the last section ends the run, and the result is `default_return` if the frontmatter declares one, else the last model reply the run produced, else the string `"done"`.

## Substitution resolves `args`, `var`, and `sys`

`subst::substitute` runs over a section's prose after its Lua block and before the model sees it. `{{ args }}` is the raw input string. `{{ var.<path> }}` reads the table the block wrote, read back from Lua as JSON. `{{ sys.<path> }}` reads runtime metadata: `when`, fixed at the start of the run, `now`, evaluated per section, and `id`, the 1-based index of the section.

One pass, no recursion, no arithmetic. Scalars render as strings and arrays and objects render as JSON. An unknown namespace, a missing key, and a null value are each `Error::Substitution` naming the path. An unclosed `{{` is also `Error::Substitution`, but it names no path, because until the closing `}}` is found there is no path to name; it reports the unclosed delimiter alone.

## The Lua block runs in a hand-hardened `lua54` VM

One `lua` fence per section, run before the section's model turn, in a VM built fresh for that section. Only `string`, `table`, and `math` are loaded, and the runtime then sets twelve base globals to nil: `load`, `loadstring`, `dofile`, `loadfile`, `collectgarbage`, `require`, `getfenv`, `setfenv`, `rawget`, `rawset`, `rawequal`, and `rawlen`. `io`, `os`, `package`, `coroutine`, and `debug` are never loaded in the first place.

An instruction hook fires every 10,000 instructions and aborts after 1,000 firings, so a block gets roughly ten million instructions. Both figures are unmeasured first cuts, set generously so that only a runaway block reaches them. Exceeding it raises `lua instruction budget exceeded`, which reaches the caller as `Error::Lua`. There is no memory ceiling.

Five names are in scope, and every one of them is core:

| Name | Purpose |
|---|---|
| `args` | The run's raw input string. |
| `sys` | Runtime metadata: `when`, `now`, `id`. |
| `var` | A writable table, read back as JSON for prose substitution. |
| `tools` | `tools.add(...)` records names for this section. There is no `tools.remove`. |
| `store` | The run's virtual files. |

`tools.add` takes any number of names, records them in first-seen order, de-duplicates, and validates nothing; the executor resolves them afterwards. `store` is a host capability rather than a scoped tool, so it is present whether or not the block asks for anything.

A chunk's top-level return value ends the run. Only a scalar is accepted - string, integer, number, or boolean - and returning a table is `Error::Lua`.

## `store` is a run-scoped virtual filesystem, not structured run state

`Store` is a cheaply cloneable handle over `Arc<Mutex<Box<dyn FileStore + Send + Sync>>>`, so the same files are reachable from the synchronous Lua VM and from an asynchronous tool. `Store::memory()` builds one over `MemVfs`, the in-memory backend, and `FileStore` is the backend contract a filesystem or network backend would implement.

Six operations: `write`, `append`, `read`, `str_replace`, `delete`, and `glob`. Two of the shapes are deliberate rather than incidental. `read` returns numbered lines - the 1-based number right-aligned to the width of the highest, then `"| "` - which is for navigation and error messages and is not a wire format. `str_replace` is anchored rather than offset-based and requires the anchor to occur exactly once: zero matches is `StoreError::AnchorNotFound` and more than one is `StoreError::AnchorAmbiguous` carrying the count, so an edit never lands on an arbitrary match. `glob` supports `*` within one path segment and `**` across segments.

The caller creates one handle and passes it in, and every section gets that same handle, which is what makes the store the one thing that survives a section boundary. It is exposed to Lua and to nothing else: there are no model-facing file tools.

## Tools are a dyn-dispatched trait, scoped per section, with untrusted output fenced

`Tool` has five methods: `name`, `description`, `parameters_schema` returning a JSON Schema value, an async `call(Value) -> Result<String>`, and `untrusted_output`, which defaults to `false`.

The loop for a section runs to a cap - the prompt's `max_tool_iterations` when it declares one, otherwise 24. The 24 is an unmeasured first cut, chosen generously enough that a section doing real work never meets it, so it functions as a runaway guard rather than a tuned budget; a prompt that needs more says so in its frontmatter. Each round trip either yields text, which is the section's reply and returns immediately, or a batch of tool calls. For a batch, the assistant turn is echoed back into the history verbatim in the OpenAI wire shape, each call is dispatched, and each result is appended as a `tool` turn before the conversation is re-sent. A call naming a tool that was not provided is `Error::UnknownTool`; the cap reached without a text reply is `Error::ToolLoopExhausted`.

A tool declaring `untrusted_output` has its result wrapped before it enters the history: a sentence saying the enclosed text is data to analyze rather than instructions to follow, then the content between `<untrusted_input_{nonce}>` tags. The nonce is one random `u64` in hex, generated once per section, and it lives in the tag name rather than in an attribute so the closing delimiter is unguessable. Any literal occurrence of either tag inside the content is defanged by replacing its leading `<` with `&lt;`, so fetched content cannot forge the close and break out.

`WebSearch` is the one tool the crate ships. It posts the arguments to the gateway's `POST /v1/tools/web_search` with the shared bearer token and returns the body verbatim, so the search provider's key never reaches this process. It validates that `query` is present before spending a round trip, and it does not declare `untrusted_output`.

## The gateway client speaks non-streaming chat completions

`GatewayClient` holds a base URL, the shared token, and one model name. `complete` sends a message array and, when the caller supplies one, a `tools` array, and returns `CompletionResult::Text` or `CompletionResult::ToolCalls`. Streaming is not supported. `Message`, `ToolSchema`, and `ToolCall` are the wire types; a tool call's `function.arguments` arrives as a JSON-encoded string and is held parsed, falling back to a string value when it is not valid JSON.

`GatewayClient::from_env` reads `PROMPTFORGE_TOKEN`, which is required and whose absence is `Error::MissingEnv`, `PROMPTFORGE_BASE_URL`, defaulting to `http://127.0.0.1:8081/v1`, and `PROMPTFORGE_MODEL`, defaulting to the public constant `DEFAULT_MODEL`. `RunOptions::client` is `None` for a caller that wants that - the CLI - and `Some` for a caller configured from a file, which is what the MCP server passes.

## The observer reports and never decides

```rust
pub trait Observer: Send + Sync {
    fn on_event(&self, ev: &Event);
}

#[non_exhaustive]
pub enum Event {
    RunStarted { prompt: String, sections: usize },
    SectionStarted { completed: u32, name: String },
    SectionFinished { name: String },
    ModelTurn { section: String, turn: u32 },
    ToolCalled { section: String, tool: String, ok: bool },
    RunFinished { turns: u32, elapsed_ms: u64, ok: bool },
}
```

`on_event` is synchronous, sits on the run's own path, and must not block, await, or perform I/O; an implementation that forwards elsewhere queues and returns. An event is a report and never a decision, so dropping every one of them leaves the run's result unchanged, which is what lets `NullObserver` be what a caller wanting silence passes.

`completed` counts sections entered including the current one, so the first is 1, and it never decreases. `RunStarted::sections` is how many top-level sections the prompt declares, documented as a bound rather than a prediction because an early return means fewer.

`Event` derives `Serialize` and serializes externally tagged - one object whose single key is the variant name. It does not derive `Deserialize`. A test holds the exact JSON of all six variants, and a second test's exhaustive match makes a new variant fail to compile until it is added to both.

## Errors are one enum

One `Error` type spans parsing, transport, and execution, `#[non_exhaustive]`, built with `thiserror`, with the transport variant boxing its source so no dependency's error type reaches the public API. The variants are `Parse`, `MissingEnv`, `Http`, `Backend { status, body }`, `MalformedResponse`, `Lua`, `Substitution`, `ToolLoopExhausted`, `UnknownTool`, `UnknownScopedTool`, and `UnsupportedVersion`. `StoreError` in the store module is separate and also `#[non_exhaustive]`.

There is no split between a parse error, a validation error, and a run error, because there is no validation step to own the middle one.

## The in-process axum gateway is what lets the tool loop be tested without a live service

Unit tests live beside the code they test and the executor's live in `src/execute/tests.rs`. What is covered today: parsing of every frontmatter field and each malformed case, the Lua fence split and its non-cases, recursive nesting and a skipped level, the first H2 being the entry whatever it is called, and the version gate in all four of its readings; substitution of each namespace, a table rendering as JSON, and each failure; the sandbox's absent globals and the instruction budget aborting a runaway loop; `tools.add` accumulating, de-duplicating, and recording from inside a branch; every store operation, its errors, and a Lua write being visible on the caller's handle; the tool loop against an in-process axum gateway, including the guard block appearing in the re-sent conversation; and the observer's event shapes.

The in-process axum gateway is the crate's test fixture and is what the other crate documents mean when they refer to testing without a live service. There is no recording extension, because there are no extensions.

## Decide by use

- The sandbox engine. The blocklist over `lua54` of key choice 11 works and has no decision recorded either way against an allowlist-by-language-design engine, which is the alternative. Deciding this needs a real prompt corpus to say what a block legitimately reaches for, and reversing it later breaks any prompt relying on a global the stricter engine does not offer.
- A memory ceiling on the Lua VM. A block can currently exhaust memory where it cannot exhaust time, and the number to cap it at is not knowable before a prompt has been seen doing real work.
- Whether the instruction budget and the 24-round-trip tool-loop cap keep their unmeasured values, which is a measurement against real prompts rather than a design question.

*2026-08-03 - claude-opus-5*
