# `promptforge-core` design

## Status and authority

This is the authoritative design for `promptforge-core`. It distinguishes behavior present in the current tree from settled lifecycle work that is planned but not shipped. `design-core-orig.md` preserves the previous design unchanged as history.

## Shipped now

### Crate boundary

`promptforge-core` is a library. A caller parses Markdown into `Prompt`, binds it into `BoundPrompt`, supplies input, a run-scoped `Store`, and `RunOptions`, then awaits `execute::run`. Parsed prompts remain accepted temporarily so hosts can migrate to the separate binding phase without a flag day. There is no executor object and no state outliving the call.

The crate currently exposes `client`, `execute`, `lua`, `observe`, `parser`, `store`, `subst`, and `tools`. The crate root re-exports `Error`, `Result`, and `promptforge_version`.

### Current prompt and execution lifecycle

A prompt currently consists of YAML frontmatter without concrete tools, exactly one required H1 title, an optional compiled shared library, and one or more H2 sections. Ordinary Markdown before the H1 is ignored. The shared library is reserved by an exact unindented `lua prompt` Markdown fence immediately after the H1, allowing blank lines but no prose before it. Reserved-looking marker lines inside a longer Markdown fence remain prose. Each section parses into an optional compiled Lua preamble, prose, and an optional compiled Lua epilog. Top-level sections execute in file order. Child sections parse but do not execute.

Each section gets a fresh persistent `SectionVm`. The bound H1 shared program loads first, host values are injected, and the optional preamble, model reply binding, and optional epilog all use the same environment. A scalar preamble return skips model and epilog and ends the run. Otherwise the executor closes the effective alias scope, substitutes prose, validates a non-empty turn's selected identities, and enters the model tool loop. Final text is bound as `reply`, and a scalar epilog return ends the run. Empty prose skips scope validation and the model, leaves `reply` nil, and still runs the epilog. Teardown destroys the VM before the section finishes. Falling off the final section returns `default_return`, the last model reply, or `"done"` in that order.

The `Store` remains the sole intentional mutable channel across section boundaries. Direct `Store` calls are ordinary library operations and are unobserved. Store calls made through the Lua execution harness are observed without exposing paths or contents.

### Stable live-tool identity

Every callable `Tool` exposes a stable `ToolId` consisting of a server and a name in that server's namespace. The built-in live identities are `("promptforge", "web_search")` and `("promptforge", "web_fetch")`. Identity is structural and remains independent of the concrete wire name used by the current model transport.

`Tool` also exposes its exact model-facing description and parameter schema. `ToolRegistry` preserves supplied order and repeated identities, and resolves live instances by `ToolId`. The registry remains a faithful collection, while prompt binding validates the complete registry atomically and rejects any repeated live identity.

The executor advertises only the effective prompt-local aliases for one section. Each schema uses the callable live tool's concrete description and parameter schema under that alias. A returned alias resolves through the frozen alias-to-`ToolId` map and then through the run's live registry, so transport wire names never become prompt dependencies.

### Picker near-duplicate analysis

`promptforge-tool-picker` exposes `ToolPicker::near_duplicates(ids)` for validating a selected set of stable tool identities without embedding any new text. It compares every selected catalog pair using the vectors already stored by the picker and reports pairs whose cosine similarity is at or above the picker's configured `duplicate_threshold`.

The analysis is independent of capability needs, query scores, and server boundaries. Every requested identity must be present in the picker: all are validated before pair comparison, and an absent identity returns an error carrying that `ToolId` instead of an incomplete result. Repeated requested identities are idempotent set membership, so they do not repeat work or produce self-pairs, and results follow catalog pair order regardless of request order. Each `NearDuplicate` carries both descriptors and the measured score so later core binding can produce complete diagnostics without reaching into picker internals.

### Report-only observer seam

The shipped observer contract has one operation:

```rust
pub trait Observer: Send + Sync {
    fn observe(&self, section: &str, detail: &str);
}
```

Every observing API receives an always-present `&dyn Observer`. `NullObserver` is the silent implementation. There is no optional observer path, structured event enum, event serialization, clone requirement, or observer wire format.

`section` identifies the current H2 section. Run-wide activity uses the required H1 title after parsing. Parse boundaries use the fixed label `Prompt` before and after structural parsing, while shared-library compilation uses the H1 title.

`detail` is a stable operational statement from `observe::detail`. The shipped vocabulary reports run start and outcome, section start and finish, model turn outcome, tool call outcome, and every Lua-harness store operation outcome.

Reports are synchronous, non-blocking, report-only, and never consulted for a decision. Recording and null observers must produce identical results, errors, execution order, and store side effects.

Reports never contain raw prompt prose, model input or output, tool arguments or results, store paths or contents, credentials, or fetched content. Current details intentionally carry no payload metadata.

The MCP adapter recognizes only pinned exact details for cosmetic progress and turn counting. `Run started` emits progress zero with the H1 title, and each `Section started` increments progress with the H2 heading. Unknown details are tolerated and logged at debug level. Recognition never affects execution.

### Current safety and failure behavior

The Lua VM loads only the selected safe standard libraries, removes code-loading and reflection globals, and enforces an instruction budget. Tool output marked untrusted is nonce-framed before it enters model history. Expected parse, Lua, substitution, model, tool, and store failures return errors rather than panicking.

The version gate runs before observation begins. A refused source emits no reports. Once a run begins, success and failure both produce a final run observation.

### Parse-time Lua programs

`LuaProgram` retains its original source and compiles it once into process-local Lua 5.4 bytecode without executing it. Loading creates a function in a caller-supplied VM but does not call it, so one program can seed multiple independent VMs while each VM supplies its own globals. The bytecode is private, is never persisted, and is not treated as a portable serialization format.

Compilation takes an explicit prompt-region location. Malformed source returns `Error::LuaCompile` carrying that location, the retained source, and the Lua compiler diagnostic. Fixed compilation start, success, and failure reports expose none of the source or location payload. Parsing compiles the optional H1 shared library and every reserved H2 preamble and epilog once. Bound execution loads those compiled programs directly across the section lifecycle, while the separate `run_chunk` compatibility API remains available for one-phase callers.

### Three-phase section grammar

Each H2 through H6 section is split into an optional leading Lua preamble, prose, and an optional trailing Lua epilog. Reserved executable regions require exact unindented ` ```lua ` opening lines and exact unindented ` ``` ` closing lines. Blank lines may precede the preamble and follow the epilog. A lone reserved fence is the preamble, preserving the existing one-fence meaning; two fences may surround empty prose.

Only positional boundary fences are reserved. Exact Lua fences between prose remain prose without compilation. Indented markers, longer backtick runs, different capitalization, extra info tokens, other languages, and marker-looking lines inside longer Markdown fences also remain prose. A reserved boundary fence with no exact close is a location-bearing parse error rather than prose.

`Section` stores `preamble` and `epilog` as compiled `LuaProgram` values around its prose. Compilation locations identify the section heading and phase. Compilation observations use only the fixed payload-free details, while diagnostics retain source and location in the returned error. Substitution remains a prose-only operation and never rewrites Lua source.

### Persistent section VM seam

`SectionVm` owns one hardened, isolated, sendable Lua 5.4 VM for a section lifecycle. Its optional shared program runs before host values exist. `inject_host` then installs `args`, `sys`, a new `var`, the run-scoped `store`, a send-safe ordered `tools.add` recorder, and an initially nil `reply`. Host globals are installed with raw table writes so a shared-library global metatable cannot intercept delayed injection. Distinct `run_preamble`, `bind_reply`, `run_epilog`, and consuming `teardown` operations make every lifecycle phase explicit rather than asking a generic runner's caller to identify the phase out of band.

Every compiled program run after injection uses the same Lua environment. A host binds the model result with `bind_reply`, then runs an epilog in that environment. Top-level nil means no return; string, integer, number, and boolean returns become result strings; other return types fail. `var` and scoped tool names can be read without exposing the underlying VM. Shared loading, preamble execution, reply binding, epilog execution, and teardown each report fixed payload-free start and outcome details in operation order.

One instruction hook and counter belong to the VM, so shared-library, preamble, and epilog execution consume one section-wide budget rather than resetting it at phase boundaries. Store functions are installed inside an `mlua` scope for each synchronous Lua phase: they borrow the caller's observer long enough to report each operation immediately, but neither observer nor scoped callback remains in the VM while the host awaits a model. The `mlua` `send` feature and send-safe tool recorder let the owned VM move with an async run without putting a mutex around Lua or holding a guard across an await.

The existing `run_chunk` compatibility path still uses `SectionVm` for one phase and retains expression evaluation compatibility. The model executor now constructs the shared/preamble/reply/epilog lifecycle directly.

### Lua declaration and scope modes

Core exposes a deterministic `ToolResolver` seam instead of depending on a concrete picker. `bind_tool_declarations` executes one compiled H1 shared program in binding mode. `tools.need(alias, description)` resolves each capability once to a stable `ToolId`; `tools.always(alias)` marks a previously declared alias as prompt-wide; `tools.add` is refused. Aliases are accepted exactly when they match `[A-Za-z][A-Za-z0-9_-]{0,63}`. There is no normalization. Duplicate aliases return a structured binding error that cannot be suppressed with Lua `pcall`; invalid or duplicate `tools.always` declarations also fail binding.

The resulting `ToolBindings` is immutable and ordered. `SectionVm::new_with_bindings` executes the shared program in every fresh section VM under replay mode, checks every `tools.need` and `tools.always` call against the frozen declaration sequence, and never invokes the resolver. Changed values, changed order, omitted calls, and extra calls are replay mismatches. Phase checks belong to the host callbacks, so a shared function captured during replay cannot invoke declaration operations from an H2.

After host injection, the section's `tools` table is in H2 recording mode. Only `tools.add(alias...)` is accepted, and every alias must have a frozen binding. Additions are first-seen ordered and idempotent; aliases already in `tools.always` are not repeated. `close_tool_scope` returns an immutable effective scope with prompt-wide aliases first and H2 additions second, then permanently closes recording so an epilog cannot widen the model-visible scope.

Binding, registry validation, replay, scope closure, and effective-scope validation report fixed start/outcome details containing no aliases, descriptions, identities, source, or other payloads. Execution replays frozen declarations, closes the recorder before any model turn, and consumes the resulting effective scope for validation, advertisement, and dispatch. Declaring a need alone exposes nothing.

### Validated capability binding

Core depends directly on `promptforge-tool-picker` for the synchronous prompt binding phase. `bind::bind_prompt` takes ownership of a parsed `Prompt`, a prepared picker, the complete live `ToolRegistry`, and an observer. It executes the optional H1 shared program once through the existing Lua declaration mode, then validates every identity boundary before returning an immutable `BoundPrompt`. A prompt with no H1 shared program produces empty frozen bindings and maps through the same observed binding and registry-validation boundaries.

The concrete resolver caches picker decisions by the exact capability description bytes for the duration of that one pass. Repeating an identical description under another alias replays the decision without embedding the need again. No trimming, folding, or normalization merges different descriptions. A successful `Outcome::Bind` becomes a stable core `ToolId`; the selected `ToolDescriptor` is retained in a read-only diagnostics map keyed by that identity.

Picker operation failures return `Error::Bind` with the exact capability and a string diagnostic. `Outcome::Absent`, `Outcome::Duplicate`, and `Outcome::Ambiguous` return their distinct core error variants, with duplicate and ambiguous candidate identities preserved in picker order. Resolver errors survive the Lua callback boundary as structured core errors, even if H1 Lua attempts to catch the callback failure with `pcall`.

Validation first scans the complete live registry and rejects a repeated stable identity. It then builds alias-to-ID and ID-to-alias maps in local temporary values, rejecting a repeated alias, two aliases selecting one ID, or any selected ID absent from the live registry. Only a wholly valid pair of maps is moved into `BoundPrompt`, so callers can never observe a partial result. The frozen declaration sequence, diagnostics, and both maps have shared accessors but no mutation path.

The pass reports fixed binding and registry-validation start and outcome details under the H1 title. Reports contain no aliases, capabilities, identities, candidates, catalog prose, live-registry data, or picker diagnostics. Binding is synchronous and does not construct an executor or invoke a tool.

### Aliased tool scope and dispatch

Binding asks the picker for near-duplicate pairs across the immutable selected identity set and retains that analysis in `BoundPrompt`. The analysis validates every selected identity against the picker catalog once. Before each non-empty model turn, execution filters those pairs to the effective `tools.always` plus H2 `tools.add` set. A pair at or above the configured duplicate threshold fails before the model call with both aliases, stable identities, concrete picker descriptions, behavioural hints, and score. Similar tools in separate sections remain valid because they never compete in one turn.

After validation, the executor builds model schemas in effective-scope order, with prompt-wide aliases first and first-seen H2 additions second. Names are local aliases while descriptions and parameter schemas come from the callable live instances. Calls resolve alias to frozen `ToolId`, then `ToolId` to the live registry entry. Missing aliases and missing live targets are expected errors. Tool arguments and results retain the existing guard and conversation behavior, but reports expose only fixed scope, model, and tool outcomes.

## Planned, not shipped

Everything in this section is settled design for later steps and remains unimplemented.

### Semantic capability phases

The shipped Lua modes implement declaration binding, exact replay, H2 recording, and scope closure. `bind_prompt` now routes the H1 source through concrete picker binding, while the executor will later consume `BoundPrompt` and route H1 replay and H2 source regions through their lifecycle modes.

The CLI now constructs its complete available live registry first and derives a matching picker catalog from those same concrete instances. It binds synchronously and executes the resulting `BoundPrompt`. The MCP host adapter still needs to adopt this path.

### Host binding integration

The CLI parses, binds, and executes in separate phases while passing the same observer reference through each phase. Its top-level caller installs `NullObserver` by default, and synchronous binding occurs directly before the async executor is entered. The async MCP host will move synchronous binding to `spawn_blocking`. Parsed-prompt execution compatibility can be removed after that host migrates. No mutex guard will cross an await.

### Explicit non-goals

Fan-out execution, branching, retries, child execution, persistent bytecode, compatibility parsing, reranking, model-generated progress labels, and cross-section Lua memory are not part of this lifecycle refactor.

## Invariants

1. Observations report facts and never decide behavior.
2. Trace strings exclude prompt, model, tool, store, credential, and fetched payloads.
3. `NullObserver` changes only visibility.
4. `Store` is the sole intentional cross-section mutable channel.
5. A section exposes only explicitly scoped tools.
6. A live tool's identity is independent of its current transport wire name.
7. Registry lookup compares stable `ToolId` values and does not silently make wire names into identity.
8. Expected failures return errors.
9. Lua bytecode remains process-local and private; retained source and explicit locations carry compilation diagnostics.
10. Host integration remains planned until its owning host lands with tests and documentation.

*2026-08-06 03:00 - GPT-5.6 Sol*
