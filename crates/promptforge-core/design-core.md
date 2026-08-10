# `promptforge-core` current design

## Purpose and boundary

`promptforge-core` is an unpublished Rust library that turns one PromptForge Markdown source into one run result string. It parses and compiles the prompt, resolves prompt-local tool and model aliases against caller-supplied live catalogs, executes H1 and top-level H2 sections, drives model and tool calls, and reports operational facts through an observer.

The host owns everything that crosses the run boundary: the execution id, source and parsed `Prompt`, raw input string, semantic `ToolPicker`, complete live tool set, `ModelCatalog`, run-scoped `StoreRef`, observer, optional gateway client, and optional `DebugCapture`. Execution is the async free function `execute::run`; there is no executor object or process-global run state.

The crate root exposes public modules for cancellation, client transport, debugging, dialects, execution, Lua, models, normalization, observation, parsing, storage, substitution, tools, and untrusted-content framing. It re-exports the principal error types, `CancelHandle`, and `promptforge_version`.

## Source model and parsing

A prompt is YAML frontmatter followed by exactly one non-empty H1 and at least one H2 section. Frontmatter contains:

- Required `name` and `description`.
- Optional `promptforge` engine major. Detection is lenient through `promptforge_version`; execution requires the supported major.
- Optional `default_return`.
- Optional `max_tool_iterations`, with a runtime cap of 24 when absent.

`Prompt::parse(source, execution, observer)` has no runtime side effects. It validates Markdown structure, compiles executable Lua to process-local Lua 5.4 bytecode, reports parse and compilation boundaries, and returns:

- Parsed frontmatter.
- The H1 title.
- Ordered live H1 blocks.
- An optional shared Lua library in `Prompt.replay`.
- A recursive section tree whose top-level nodes are H2 sections in file order.

H1 and section content are ordered `Block` sequences. `Block` is either compiled Lua or prose carrying a `loop_capable` flag. Exact top-level, unindented, lowercase fences define executable code:

- ```` ```lua ```` opens a live Lua block.
- ```` ```lua shared ```` opens the one optional shared library, allowed only in H1.
- ```` ``` ```` closes either form.

Near-miss markers, other languages, indentation, longer fences, and marker-like text inside another fence remain prose. Reserved fences must close exactly. The removed leading H1 form ```` ```lua prompt ```` is a targeted parse error.

The last prose block in a block sequence is loop-capable. Earlier prose blocks are single-shot. Classic prologue, prose, and epilog is represented as `[Lua, Prose, Lua]`; compatibility accessors project that shape.

H3 through H6 headings form children. They do not execute by top-level fall-through. List-only child sections may be parsed into ordered items for explicit fanout.

## Run lifecycle

`execute::run(prompt, args, ResolutionContext, tools, store, RunOptions)` first gates the engine major, then:

1. Reports run start.
2. Executes live H1 blocks once in source order.
3. Captures resolved tool bindings, model bindings, and serializable H1 `var`.
4. Walks top-level H2 sections in file order, honoring explicit control transfer.
5. Returns a scalar Lua result, otherwise `default_return`, otherwise the last model reply, otherwise `"done"`.
6. Reports run success or failure.

`ResolutionContext` contains the semantic picker and live model catalog. `RunOptions` contains the execution id, observer, optional `GatewayClient`, and optional `DebugCapture`. If no client is supplied, model use may construct one from the documented environment configuration.

The raw run input is one string. Lua receives it as `args`; prose can reference `{{ args }}`. Core does not impose an input schema.

## Live H1

H1 is both the semantic prompt boundary and a live program. Ordinary H1 Lua and prose execute exactly once. H1 can resolve capabilities, call models, use the store, update `var`, log constrained checkpoints, and return a scalar result.

Resolution happens when the executing branch reaches it:

- `tools.need(alias, description)` resolves one live tool and returns a Tool object.
- `tools.always(...)` declares tools available to every model-facing section.
- `models.need(alias, description, opts?)` resolves one catalog model under optional hard constraints and freezes invocation options.
- `models.always(...)` selects the prompt-wide default model and may combine declaration with selection.

Aliases are prompt-local and case-sensitive. Duplicate aliases, repeated live identities, two aliases selecting one identity, picker abstention or ambiguity, and a picked identity absent from the live registry fail explicitly. Conditional declarations that do not execute create no binding.

The optional `lua shared` chunk is not a declaration pass and does not execute in H1. It is a pure section library compiled into `Prompt.replay`.

## Section VM and ordered blocks

Every section execution gets one isolated `SectionVm` that survives all blocks in that section and is destroyed at teardown. No Lua memory or model conversation crosses a normal section boundary.

VM setup order is an invariant:

1. Create the restricted Lua 5.4 VM.
2. Load `Prompt.replay` before host APIs or frozen capability objects exist.
3. Install captured Tool and Model objects.
4. Inject host state: `args`, sealed `sys`, H1-seeded `var`, `store`, `tasks`, `execute`, `jump`, and prior `reply`.

Shared-library top-level code therefore cannot call host APIs. Functions defined there may refer to host globals for call-time lookup after injection.

Section blocks run in source order in the same VM and one accumulating section conversation:

- Lua runs directly and may mutate section `var`, store state, tool scope, model selection, and `reply`.
- Before the first non-empty model-facing prose, tool and model scopes close, effective tools are validated, tool-call counters are installed, and `sys.model` is published when selected.
- Prose is substituted once and appended to the section conversation.
- Non-final prose performs one model round. If that round emits tool calls, they are dispatched and execution continues without requiring final text.
- Final prose runs the full tool loop until text or exhaustion.
- Empty or whitespace prose causes no model call.
- A section with no prose may still execute Lua and return or fall through.

The shared mutable channels across top-level sections are the caller-owned store, the captured H1 seed state, and the previous model reply string. The conversation and section VM are discarded. A jump clears the cross-section reply before entering its target.

## Lua control flow

A top-level Lua scalar return accepts string, integer, number, or boolean and becomes text. `nil` continues. Unsupported return types fail.

- A scalar return from H1 ends the run.
- A scalar return from a top-level section ends the run.
- `execute(target, input?)` runs a named top-level H2 as an isolated subroutine with a fresh VM and conversation, shared run resources, and recursion capped at 8. It returns the subroutine's scalar or reply text. `jump` inside `execute` is an error.
- `jump(target)` stops the current top-level section and transfers to the named H2 without returning.
- `tasks["## Name"]` exposes Section objects with `.name` and `.has_prose`; strings or Section objects may identify `execute` and `jump` targets.

Children execute only through explicit fanout. `fanout("### Worker", "### List")` maps a sibling worker over pre-parsed sibling list items concurrently. Each arm gets a fresh VM plus `item` and `sys.taskid`. Results preserve list order and expose `.text`, `.ok`, `.item`, and `.exhausted`; string coercion returns `.text`. Tool-loop exhaustion becomes a marked incomplete result, while fatal arm errors abort siblings.

Cancellation is cooperative through the run-scoped cancellation facility and interrupts model loops and fanout.

## Tools and models

Prompts depend on semantic aliases rather than deployment wire names.

A live `Tool` has stable `ToolId`, wire name, description, parameter schema, trust classification, and async call behavior. A resolved Tool object exposes prompt-local name, description, parameters, wire name, and trust state. Authors may override the model-facing description before adding the object to scope.

`tools.add` accepts aliases, Tool objects, or arrays. Prompt-wide `tools.always` and section additions form the effective scope. Scope is opt-in, frozen before prose, checked for semantic near-duplicates, advertised under local aliases, and dispatched by stable identity. Out-of-scope calls fail. `tools.calls` counts attempted dispatches by alias within the VM.

`ToolBag` generation-tracks H2 additions for `model:infer` and rebuilds schemas and dispatch maps only after mutation.

A `ModelCatalog` describes stable model identity, semantic description, context size, thinking capability, and tool dialect. `models.need` can constrain context and thinking and freeze temperature, maximum tokens, and thinking choice. `models.use` selects at most one section model; prompt-wide `models.always` is the fallback. Non-empty model-facing prose without a selected model fails.

`model:infer(prompt, opts?)` performs explicit inference from Lua using the selected Model object. It snapshots the current tool bag, runs the tool loop, returns text, updates global `reply`, advances tool-call counts, and publishes `sys.reply_finish_reason`. The last completed inference wins for `reply`.

Tool dialects own request preparation, tool-call parsing, and history echo. Built-in dialects are `openai` and `gemma3_tool_code`. Prompt authors select model capability, not dialect.

## State and substitution

`StoreRef` is a cloneable handle over a caller-owned, run-scoped virtual filesystem. Its operations are:

- `write` and `append`.
- `read_lines`, returning numbered lines for navigation.
- `read`, returning verbatim text.
- `inject`, returning verbatim text inside an untrusted-content envelope.
- `str_replace`, which requires exactly one matching anchor.
- `delete`, `glob`, and `exists`.

The store has no persistence beyond what its caller-provided backend supplies. Core provides `MemStore`. Store operations are synchronous and no store lock is held across an await.

Prose substitution is single-pass and never rewrites Lua source. Available forms are `{{ args }}`, `{{ reply }}`, `{{ item }}` in fanout, `{{ var.path }}`, and `{{ sys.path }}`. Scalars render as text and arrays or objects as JSON. Missing, null, malformed, or context-inapplicable paths fail instead of silently producing empty text.

`sys` is sealed against writes and unknown Lua-field reads. Its run and section fields include time, section identity, execution id, and section count. Model selection adds `sys.model`; completed inference adds `sys.reply_finish_reason`; fanout adds `sys.taskid`.

## Isolation, trust, and diagnostics

Lua source compiles during parsing and bytecode remains process-local. Runtime VMs expose a restricted library set, remove loaders and reflection-oriented globals, and enforce an instruction budget. Lua source remains available for mapped diagnostics.

Tool output marked untrusted is nonce-framed and delimiter-defanged before entering model history. `store.inject` applies the same framing to stored text deliberately reintroduced to a model.

`Observer` receives borrowed `(execution, section, detail)` strings synchronously. Reports are facts and are never consulted for decisions. Fixed details exclude prompt and model payloads, tool arguments and results, store paths and contents, credentials, and fetched content. The only author-controlled detail is constrained `log(message)` output. `NullObserver` is the silent implementation.

Raw request and response JSON belongs only to opt-in `DebugCapture`; it does not widen the observer contract.

Expected parsing, compilation, resolution, scope, substitution, model, tool, store, control-flow, cancellation, and transport failures return through the crate error surface. Empty model text is an error. Persistent bytecode, cross-section Lua memory, implicit child execution, dynamic nested fanout, structured persistent run state, and model-generated authoritative progress labels are outside this design.
