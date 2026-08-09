# `promptforge-core` produces bound, isolated Markdown prompt runs

## Executive summary

`promptforge-core` turns one PromptForge Markdown source into a validated run result. A host creates one execution id, parses the source into `Prompt`, binds semantic tool and model declarations into an immutable `BoundPrompt`, supplies one raw input string, a complete live tool registry, a model catalog, a run-scoped virtual file store, and an observer, then awaits `execute::run` under that same id.

The language requires one H1, permits one immediately leading shared `lua` program, and executes top-level H2 sections in file order. Each section is an ordered sequence of alternating `lua` and prose blocks sharing one isolated Lua 5.4 VM and one accumulating conversation. Non-final prose is single-shot; the final prose block runs the full tool loop. Lua may call `model:infer`, `execute`, or `_G["goto"]` for explicit inference and control flow. No Lua memory crosses sections; `StoreRef` and the previous section's `reply` text are the intentional mutable channels.

**Principle: prose in Markdown, code in Lua, no mixing.** Model-facing text lives in prose blocks. Programmable logic lives in exact `lua` fences. A `lua` fence in the middle of prose is prose; substitution never rewrites Lua source.

Prompts depend on semantic aliases, never deployment vendor strings. Binding freezes one-to-one alias maps before execution. The observer reports deterministic borrowed `(execution, section, detail)` strings and cannot steer behavior. Expected failures return errors. The authoritative result is one string: scalar Lua return, frontmatter `default_return`, last model reply, or `"done"`.

`design-core-orig.md` is byte-for-byte historical and is not part of the current contract.

## Standing rule

**No defaults. Everything explicit. Implicit is the enemy of precision.**

A prompt declares tools, models, context, thinking, and temperature. The host supplies credentials and the gateway URL. Omitting a declaration never means "pick something sensible."

## Key design choices

1. **A run is a free function over caller-owned resources.** Parse, bind, then `execute::run`. No executor object or process-global run state. The caller owns the execution id, prompt, live tools, model catalog, store, observer, optional gateway client, and optional `DebugCapture`.

2. **One required H1 is the semantic prompt boundary.** Frontmatter is followed by exactly one non-empty H1. The H1 supplies the stable run title and the only location for shared declarations.

3. **One exact leading `lua` fence carries shared code.** Blank lines may separate H1 from the unindented fence; only the leading position is reserved. The removed `lua prompt` info-token form is a parse error in that position.

4. **A section is an ordered list of blocks, not a fixed three-phase template.** `Section.blocks` is `Vec<Block>` where `Block = Lua(program) | Prose { text, loop_capable }`. Any number of alternating lua/prose blocks is legal. Today's prologue/prose/epilog is exactly `[Lua, Prose, Lua]` and needs no migration. Compatibility helpers `Section::prologue`, `Section::prose`, and `Section::epilog` still project the classic shape when present.

5. **Non-final prose is single-shot; final prose is the tool loop.** At parse time the last prose block is marked `loop_capable`. Non-final prose runs one model round (tool calls allowed in that round) then control continues to the next lua block. Final prose keeps calling tools until the model produces text - the same loop as before. A section with no prose (lua-only early return) still works. Want two full loops in one logical unit: call `execute` on another section.

6. **One conversation accumulates across all blocks in a section.** Context grows through every prose and every `model:infer` within the section. Between sections the conversation is discarded. `goto` also clears cross-section `reply` context before the target runs.

7. **Lua compiles at parse time; bytecode stays process-local.** `LuaProgram` retains source for diagnostics and private Lua 5.4 bytecode. A successful `Prompt` is syntactically executable; bytecode is never persisted.

8. **One isolated VM survives the whole section lifecycle.** Shared load, host injection, every lua block, model awaits, `reply` binding, and teardown use one `SectionVm`. The VM is destroyed before fall-through.

9. **Scalar Lua return is the only early exit from the run.** A top-level string, integer, number, or boolean return becomes the run result; nil continues; unsupported values fail. Inside `execute()`, a scalar return ends that subroutine and becomes its reply string; `_G["goto"]` inside `execute()` is a hard error.

10. **The store and reply are the intentional mutable state across sections.** One caller-owned `StoreRef` exposes a run-scoped virtual filesystem: write, append, read_lines (numbered), read (verbatim), inject (verbatim + untrusted envelope), str_replace, delete, glob, and `exists(path)` (boolean, no error on missing). The previous section's model reply carries forward as Lua `reply` (nil in section 1) and as `{{ reply }}` in prose. Section VMs and conversations are discarded.

11. **Tool, Model, and Section are first-class Lua objects.**
    - `tools.need` returns a Tool userdata: `.name`, `.description` (mutable), `.parameters`, `.wire_name`, `.untrusted`. Assigning `.description` before `tools.add` overrides the model-facing schema text.
    - `tools.add` accepts strings, Tool objects, and arrays of either.
    - `models.need` / `models.always` return a Model userdata: `.name`, `.model_id`, `.description`, `.context`, `.thinking`, `.temperature`, `.max_tokens`, `.dialect`.
    - The `tasks` table maps `"## Name"` to Section userdata: `.name`, `.has_prose`. Pass a string or Section object to `execute` / `_G["goto"]`.

12. **`model:infer(prompt, opts?)` is explicit inference from Lua.** Available during section execution via `block_in_place` + `Handle::block_on` (same pattern as fanout). It snapshot-reads the current tool bag, runs the tool loop, returns text, sets the `reply` global, increments `tools.calls`, and publishes `sys.reply_finish_reason`. Multiple infers: last completed inference wins for `reply`. The implicit prose path stays executor-driven and does not call `infer`.

13. **`ToolBag` caches schemas behind a generation counter.** `tools.add` bumps `ToolRuntime::generation`. `model:infer` rebuilds schemas/dispatch only on generation mismatch. Counts persist across infer and prose calls within the section; newly added tools seed at 0. The implicit prose path still uses `prepare_effective_scope`.

14. **`execute(target, input?)` runs a section as a subroutine.** Fresh VM, fresh conversation, fresh `var`, same store/observer/execution id/gateway/registry. Recursion capped at 8. Returns the section's reply string. Target is `"## Name"` or a Section object.

15. **`_G["goto"](target)` transfers control.** Lua 5.4 reserves `goto` as a keyword, so the API is only callable through that global index. The current section stops (no further blocks). Conversation and cross-section `reply` clear. The named top-level H2 runs next. No return to the caller. Unknown target is a hard error.

16. **Prompts declare semantic needs under local aliases.** H1 `tools.need(alias, description)` and `models.need(alias, description, opts?)` use case-sensitive aliases matching `[A-Za-z][A-Za-z0-9_-]{0,63}`. Optional model `opts`: `context`, `thinking`, `temperature`, `max_tokens`. Declaring a need exposes nothing and selects no model. H2 `tools.add` scopes tools; H1 `tools.always` is for every model-facing section. H2 `models.use` selects at most one binding; H1 `models.always` is the prompt-wide default. Non-empty model-facing prose without either fails with `Error::ModelRequired`.

17. **Binding freezes one-to-one maps before execution.** `bind_prompt` resolves tools through one prepared `ToolPicker`, validates the live `ToolRegistry`, filters `ModelCatalog` for models, and freezes `ModelBindings`. Duplicate live IDs, duplicate aliases, two aliases selecting one tool ID, and selected IDs absent from the registry all fail before execution. Effective-scope near-duplicates fail before a model sees tools.

18. **Per-VM `tools.calls` counts measure model behaviour.** Seeded at 0 for every in-scope alias. Incremented when dispatch is attempted. Unknown keys are hard errors. Out-of-scope model tool calls are `Error::OutOfScopeToolCall`. Counts are not rolled up across fanout arms.

19. **`sys` is sealed runtime metadata.** Always after inject: `sys.when`, `sys.now`, `sys.id`, `sys.section_name`, `sys.execution`, `sys.section_count`. After H2 scope close: `sys.model` (bound catalog id). After prose or infer completes: `sys.reply_finish_reason` (`"stop"`, `"length"`, `"tool_calls"`, or nil). Unknown field reads and any writes raise. Prose substitution also resolves `{{ args }}`, `{{ reply }}`, `{{ var.x }}`, and the `sys.*` paths above.

20. **Fanout returns structured arm results.** `fanout("### Worker", "### List")` still runs list-only H3 siblings concurrently on a `JoinSet`. Each result object has `.text`, `.ok`, `.item`, `.exhausted`. Soft-degraded exhaustion sets `.ok = false` and `.exhausted = true` with stub text that retains failure metadata. `__tostring` returns `.text` so existing `tostring` / coercing `table.concat` callers keep working. Fatal arm errors abort siblings. Children never execute by fall-through.

21. **The observer reports facts; raw payloads use opt-in `DebugCapture`.** Always-present `&dyn Observer`; silence is `NullObserver`. Fixed details are payload-free. Constrained phase-local `log(message)` is the sole author-controlled `Lua: <message>` exception (UTF-8, ≤256 chars, no newlines/controls). `RunOptions::debug` receives owned request/response JSON without widening the observer.

22. **Untrusted and executable content stay visibly separated.** Restricted Lua VM, instruction budget, no `print`/loaders/reflection. Untrusted tool results are nonce-framed before model history.

23. **Tool dialects own prepare, parse, and history echo.** `openai` and `gemma3_tool_code` ship. Author prompts never name a dialect. Empty model product is always `Error::EmptyModelReply`.

24. **Complete prompt fixtures live at the public crate boundary** in unpublished `promptforge-core-tests`. Ordinary tests stay offline; opt-in real-model scenarios go through a temporary gateway sidecar. Interactive authoring is `promptforge-dev` against an already-running gateway.

## The public lifecycle

`Prompt::parse(source, execution, observer)` validates structure, compiles executable Lua, and returns a `Prompt` (frontmatter, title, optional shared program, H1 description, top-level section tree with `blocks`).

`bind::bind_prompt(prompt, picker, registry, models, execution, observer)` returns `BoundPrompt`. No chat completion or tool call during binding.

`execute::run(bound, args, tools, store, RunOptions)` gates the engine major, walks top-level H2 sections in source order (honoring `goto` transfers), and returns one string. After first prose (or scope close), a selected `models.use` or prompt-wide `models.always` supplies `CompletionOptions`. The model loop / infer path is capped by `max_tool_iterations` (default 24). Falling off the last section returns `default_return`, else last model reply, else `"done"`.

## Exact grammar keeps Markdown examples inert

Every reserved opening line is exactly ```` ```lua ```` and every closing line is exactly ```` ``` ````. Openers are unindented and lowercase with no extra info tokens. Longer markers, indentation, other languages, and nested marker-looking lines remain prose.

An H2 section may contain any number of exact leading/trailing/interior reserved fences alternating with prose. An exact Lua fence that is not a reserved boundary shape inside a longer fence remains prose. An unclosed reserved boundary is an error. H3 through H6 children parse into the tree; only explicit `fanout` runs children.

## Section execution

For each top-level section:

1. Create `SectionVm`, load shared bytecode, inject `args`, sealed `sys` (including `section_name`, `execution`, `section_count`), new `var`, `store`, replay-backed `tools` / `models`, `tasks`, `execute`, `_G["goto"]`, and previous `reply`.
2. Walk `blocks` in order.
3. Lua blocks run on the live VM. Scalar return ends the run (or the `execute` subroutine). `_G["goto"]` stops the section and transfers.
4. Before the first prose, close tool/model scope, seed `tools.calls`, enrich `sys.model`, prepare effective tool schemas.
5. Non-empty prose substitutes, then runs single-shot or full loop per `loop_capable`. Empty/whitespace prose skips the model. Bind non-empty final text to `reply`; publish `sys.reply_finish_reason`.
6. Teardown the VM. On goto: clear `reply`, jump to the target index. On scalar return: end the run. Else fall through.

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

Malformed frontmatter or Markdown, unsupported engine version, Lua failure, invalid log arguments, replay mismatch, invalid alias, tool/model binding abstention or ambiguity, identity collision, missing scoped alias, near-duplicate effective scope, substitution failure, model/tool/store failure, empty model product, tool-loop exhaustion, unknown goto target, execute depth overflow, and goto-inside-execute are returned through the crate error surface.

Persistent bytecode, cross-section Lua memory, child execution by fall-through, nested/dynamic fanout, reranking, and model-generated authoritative progress labels remain non-goals. OverlayStore, Lua-as-tool, model file tools, `tools.remove`, and store list/grep are out of scope for this design revision.

*2026-08-09 10:40 - Cursor Grok 4.5*
