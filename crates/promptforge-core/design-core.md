# `promptforge-core` produces isolated Markdown prompt runs

## Executive summary

`promptforge-core` turns one PromptForge Markdown source into a validated run result. A host creates one execution id, parses the source into `Prompt`, then supplies that prompt, one raw input string, a prepared tool picker, a complete live tool registry, a model catalog, a run-scoped virtual file store, and an observer to `execute::run` under the same id.

The language requires one H1 and executes top-level H2 sections in file order. Ordinary H1 blocks alternate exactly like section blocks:

```
[lua] [prose] [lua] [prose] ... [lua]
```

Those H1 blocks run live exactly once, in source order. `tools.need`, `models.need`, and `models.always` resolve when reached, H1 may call `model:infer`, and H1 `var` and store effects become run state. One exact `lua shared` fence may appear anywhere in H1. The parser compiles that separate library chunk as `Prompt.replay`; each section VM loads it before any host APIs or frozen capability objects are installed.

Each section is an ordered sequence of alternating `lua` and prose blocks sharing one isolated Lua 5.4 VM and one accumulating conversation. Non-final prose is single-shot; the final prose block runs the full tool loop. Lua may call `model:infer`, `execute`, or `jump` for explicit inference and control flow. No Lua memory crosses sections; captured H1 `var`, `StoreRef`, and the previous section's `reply` text are the intentional mutable channels.

**Principle: prose in Markdown, code in Lua, no mixing.** Model-facing text lives in prose blocks. Programmable logic lives in exact `lua` fences. A `lua` fence in the middle of prose is prose; substitution never rewrites Lua source.

Prompts depend on semantic aliases, never deployment vendor strings. Live H1 resolution captures one-to-one alias maps for section VMs. The observer reports deterministic borrowed `(execution, section, detail)` strings and cannot steer behavior. Expected failures return errors. The authoritative result is one string: scalar Lua return, frontmatter `default_return`, last model reply, or `"done"`.

## Standing rule

**No defaults. Everything explicit. Implicit is the enemy of precision.**

A prompt declares tools, models, context, thinking, and temperature. The host supplies credentials and the gateway URL. Omitting a declaration never means "pick something sensible."

## Key design choices

1. **A run is a free function over caller-owned resources.** Parse, then `execute::run`. The caller owns the execution id, prompt, picker, live tools, model catalog, store, observer, optional gateway client, and optional `DebugCapture`; there is no executor object or process-global run state.

2. **One required H1 is the semantic prompt boundary and a live program.** Frontmatter is followed by exactly one non-empty H1. Its ordinary Lua and prose blocks execute exactly once before the first H2. Capability resolution, inference, store access, and serializable `var` mutation are available there.

3. **One explicit `lua shared` fence carries the section library.** It may appear anywhere in H1 and is removed from the live block sequence. More than one, or any beneath H1, is a parse error. A plain H1 `lua` fence is always live code, with no single-fence compatibility magic. `Prompt.replay` stores the compiled library chunk.

4. **A section is an ordered list of blocks, not a fixed three-phase template.** `Section.blocks` is `Vec<Block>` where `Block = Lua(program) | Prose { text, loop_capable }`. Any number of alternating lua/prose blocks is legal. Today's prologue/prose/epilog is exactly `[Lua, Prose, Lua]` and needs no migration. Compatibility helpers `Section::prologue`, `Section::prose`, and `Section::epilog` still project the classic shape when present.

5. **Non-final prose is single-shot; final prose is the tool loop.** At parse time the last prose block is marked `loop_capable`. Non-final prose runs one model round (tool calls allowed in that round) then control continues to the next lua block. Final prose keeps calling tools until the model produces text - the same loop as before. A section with no prose (lua-only early return) still works. Want two full loops in one logical unit: call `execute` on another section.

6. **One conversation accumulates across all blocks in a section.** Context grows through every prose and every `model:infer` within the section. Between sections the conversation is discarded. `jump` also clears cross-section `reply` context before the target runs.

7. **Lua compiles at parse time; bytecode stays process-local.** `LuaProgram` retains source for diagnostics and private Lua 5.4 bytecode. A successful `Prompt` is syntactically executable; bytecode is never persisted.

8. **One isolated VM survives the whole section lifecycle.** `Prompt.replay` loads first, while host APIs are unavailable. Rust then installs captured Tool and Model objects, followed by host injection. Every section Lua block, model await, `reply` update, and teardown uses that `SectionVm`. The VM is destroyed before fall-through.

9. **Scalar Lua return is the only early exit from the run.** A top-level string, integer, number, or boolean return becomes the run result; nil continues; unsupported values fail. Inside `execute()`, a scalar return ends that subroutine and becomes its reply string; `jump` inside `execute()` is a hard error.

10. **The store and reply are the intentional mutable state across sections.** One caller-owned `StoreRef` exposes a run-scoped virtual filesystem: write, append, read_lines (numbered), read (verbatim), inject (verbatim + untrusted envelope), str_replace, delete, glob, and `exists(path)` (boolean, no error on missing). The previous section's model reply carries forward as Lua `reply` (nil in section 1) and as `{{ reply }}` in prose. Section VMs and conversations are discarded.

    The store can be pre-populated by the caller before `execute::run` is called. The prompt itself is agnostic to the source of store content - it reads and writes the store without knowing whether a path was seeded by the caller or written by an earlier section. This is the mechanism input files use: the caller seeds the store, the prompt consumes it. Frontmatter declares the contract with two optional keys: `input` names files the prompt expects to find in the store at entry, and `output` names files the prompt will produce. Each entry is a `FileDecl` exposing `path()` and `description()`. `MemStore::with_files` and `StoreRef::with_files` construct a store pre-seeded from a list of path/content pairs, validating every path at construction (empty paths, path traversal, and absolute paths are refused). Declarations are metadata for the gateway layer - the store itself enforces nothing beyond its existing path rules - so a prompt that reads a path it did not declare succeeds, and one that fails to write a declared output is not a store error.

11. **Tool, Model, and Section are first-class Lua objects.**
    - `tools.need` returns a Tool userdata: `.name`, `.description` (mutable), `.parameters`, `.wire_name`, `.untrusted`. Assigning `.description` before `tools.add` overrides the model-facing schema text.
    - `tools.add` accepts strings, Tool objects, and arrays of either.
    - `models.need` / `models.always` return a Model userdata: `.name`, `.model_id`, `.description`, `.context`, `.thinking`, `.temperature`, `.max_tokens`, `.dialect`.
    - The `tasks` table maps `"## Name"` to Section userdata: `.name`, `.has_prose`. Pass a string or Section object to `execute` / `jump`.

12. **`model:infer(prompt, opts?)` is explicit inference from Lua.** Available during live H1 and section execution via `block_in_place` + `Handle::block_on` (same pattern as fanout). It snapshot-reads the current tool bag, runs the tool loop, returns text, sets the `reply` global, increments `tools.calls`, and publishes `sys.reply_finish_reason`. Multiple infers: last completed inference wins for `reply`. The implicit prose path stays executor-driven and does not call `infer`.

13. **`ToolBag` caches schemas behind a generation counter.** `tools.add` bumps `ToolRuntime::generation`. `model:infer` rebuilds schemas/dispatch only on generation mismatch. Counts persist across infer and prose calls within the section; newly added tools seed at 0. The implicit prose path still uses `prepare_effective_scope`.

14. **`execute(target, input?)` runs a section as a subroutine.** Fresh VM, fresh conversation, a fresh copy of the captured H1 `var`, same store/observer/execution id/gateway/registry. Recursion capped at 8. Returns the section's reply string. Target is `"## Name"` or a Section object.

15. **`jump(target)` transfers control.** The current section stops (no further blocks). Conversation and cross-section `reply` clear. The named top-level H2 runs next. No return to the caller. Unknown target is a hard error.

16. **Prompts resolve semantic needs live under local aliases.** H1 `tools.need(alias, description)` and `models.need(alias, description, opts?)` use case-sensitive aliases matching `[A-Za-z][A-Za-z0-9_-]{0,63}`. Each call immediately asks the prepared picker and returns a frozen Tool or Model object. Optional model `opts`: `context`, `thinking`, `temperature`, `max_tokens`. A need exposes nothing and selects no model. H2 `tools.add` scopes tools; H1 `tools.always` is for every model-facing section. H2 `models.use` selects at most one captured model; H1 `models.always` is the prompt-wide default. Non-empty model-facing prose without either fails with `Error::ModelRequired`.

17. **Live H1 resolution captures one-to-one maps during execution.** Conditional needs resolve only when their branch runs. Duplicate live IDs, duplicate aliases, two aliases selecting one tool ID, and selected IDs absent from the registry fail at the resolving call. Rust installs the captured handles directly into every section VM. Effective-scope near-duplicates fail before a model sees tools.

18. **Per-VM `tools.calls` counts measure model behaviour.** Seeded at 0 for every in-scope alias. Incremented when dispatch is attempted. Unknown keys are hard errors. Out-of-scope model tool calls are `Error::OutOfScopeToolCall`. Counts are not rolled up across fanout arms.

19. **`sys` is sealed runtime metadata.** Always after inject: `sys.when`, `sys.now`, `sys.id`, `sys.section_name`, `sys.execution`, `sys.section_count`. After H2 scope close: `sys.model` (bound catalog id). After prose or infer completes: `sys.reply_finish_reason` (`"stop"`, `"length"`, `"tool_calls"`, or nil). Unknown field reads and any writes raise. Prose substitution also resolves `{{ args }}`, `{{ reply }}`, `{{ var.x }}`, and the `sys.*` paths above.

20. **Fanout returns structured arm results.** `fanout("### Worker", "### List")` still runs list-only H3 siblings concurrently on a `JoinSet`. Each result object has `.text`, `.ok`, `.item`, `.exhausted`. Soft-degraded exhaustion sets `.ok = false` and `.exhausted = true` with stub text that retains failure metadata. `__tostring` returns `.text` so existing `tostring` / coercing `table.concat` callers keep working. Fatal arm errors abort siblings. Children never execute by fall-through.

21. **The observer reports facts; raw payloads use opt-in `DebugCapture`.** Always-present `&dyn Observer`; silence is `NullObserver`. Fixed details are payload-free. Constrained phase-local `log(message)` is the sole author-controlled `Lua: <message>` exception (UTF-8, ≤256 chars, no newlines/controls). `RunOptions::debug` receives owned request/response JSON without widening the observer.

22. **Untrusted and executable content stay visibly separated.** Restricted Lua VM, instruction budget, no `print`/loaders/reflection. Untrusted tool results are nonce-framed before model history.

23. **Tool dialects own prepare, parse, and history echo.** `openai` and `gemma3_tool_code` ship. Author prompts never name a dialect. Empty model product is always `Error::EmptyModelReply`.

24. **Complete prompt fixtures live at the public crate boundary** in unpublished `promptforge-core-tests`. Ordinary tests stay offline; opt-in real-model scenarios go through a temporary gateway sidecar. Interactive authoring is `promptforge-dev` against an already-running gateway.

## The public lifecycle

`Prompt::parse(source, execution, observer)` validates structure, compiles executable Lua, and returns a `Prompt` containing frontmatter, title, `h1_blocks`, optional `replay`, and the top-level section tree.

`execute::run(prompt, args, ResolutionContext, tools, store, RunOptions)` gates the engine major, executes `h1_blocks` live exactly once, captures resolved Tool and Model objects plus the serialized H1 `var`, then walks top-level H2 sections in source order while honoring `jump` transfers. `ResolutionContext` carries the picker, complete tool registry, and model catalog. After first prose or scope close, a selected `models.use` or prompt-wide `models.always` supplies `CompletionOptions`. The model loop and infer path are capped by `max_tool_iterations` (default 24). Falling off the last section returns `default_return`, else last model reply, else `"done"`.

## Exact grammar keeps Markdown examples inert

Ordinary executable openings are exactly ```` ```lua ````. The one library opening is exactly ```` ```lua shared ````. Every closing line is exactly ```` ``` ````. Openers are unindented and lowercase. Longer markers, indentation, other languages, and nested marker-looking lines remain prose.

H1 and each H2 section may contain ordinary Lua and prose in alternating source order. `lua shared` is allowed once in H1 only and does not become an H1 block. An exact Lua fence that is not a reserved boundary shape inside a longer fence remains prose. An unclosed reserved boundary is an error. H3 through H6 children parse into the tree; only explicit `fanout` runs children.

## Section execution

Before the section walk, execute every live H1 block once. Resolver calls talk to the real picker, inference and store APIs are live, and the final serializable `var` is captured with the frozen capability maps.

For each top-level section:

1. Create `SectionVm` and load `Prompt.replay` before host injection. At library load time `args`, `sys`, `var`, `store`, `reply`, `log`, tools, models, and control-flow APIs are unavailable, so top-level host calls fail. Pure Lua definitions may refer to those globals for later call-time resolution.
2. Install captured Tool and Model handles directly from Rust, then inject `args`, sealed `sys` (including `section_name`, `execution`, `section_count`), H1-seeded `var`, `store`, `tasks`, `execute`, `jump`, and previous `reply`.
3. Walk `blocks` in order.
4. Lua blocks run on the live VM. Scalar return ends the run or the `execute` subroutine. `jump` stops the section and transfers.
5. Before the first prose, close tool/model scope, seed `tools.calls`, enrich `sys.model`, and prepare effective tool schemas.
6. Non-empty prose substitutes, then runs single-shot or full loop per `loop_capable`. Empty or whitespace prose skips the model. Bind non-empty final text to `reply`; publish `sys.reply_finish_reason`.
7. Teardown the VM. On jump: clear `reply`, jump to the target index. On scalar return: end the run. Else fall through.

## Observation

```rust
pub trait Observer: Send + Sync {
    fn observe(&self, execution: &str, section: &str, detail: &str);
}
```

Fixed vocabulary in `observe::detail`. After successful non-empty final-text bind, `finish_reason == "length"` also reports `Model turn truncated`. Empty product hard-fails as `EmptyModelReply` / `Model turn failed`.

```rust
pub trait DebugCapture: Send + Sync {
    fn on_event(&self, execution: &str, section: &str, turn_index: u32, event: DebugEvent);
}
```

## Host registries

Every `Tool` exposes stable `ToolId`, wire name, description, parameter schema, trust classification, and async call behavior. Hosts construct the live registry and derive the picker catalog from the same instances, plus a `ModelCatalog` from gateway `GET /v1/models` (or a pinned offline entry).

## Failures and non-goals

Malformed frontmatter or Markdown, invalid `lua shared` placement or multiplicity, unsupported engine version, Lua failure, invalid log arguments, invalid alias, live tool/model resolution abstention or ambiguity, identity collision, missing scoped alias, near-duplicate effective scope, substitution failure, model/tool/store failure, empty model product, tool-loop exhaustion, unknown jump target, execute depth overflow, and jump-inside-execute are returned through the crate error surface.

Persistent bytecode, cross-section Lua memory, child execution by fall-through, nested/dynamic fanout, reranking, and model-generated authoritative progress labels remain non-goals. OverlayStore, Lua-as-tool, model file tools, `tools.remove`, and store list/grep are out of scope for this design revision.

*2026-08-09 18:05 - GPT-5.6 Sol*
