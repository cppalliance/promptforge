# `promptforge-core` design

## Status and authority

This is the authoritative design for `promptforge-core`. It distinguishes behavior present in the current tree from settled lifecycle work that is planned but not shipped. `design-core-orig.md` preserves the previous design unchanged as history.

## Shipped now

### Crate boundary

`promptforge-core` is a library. A caller parses Markdown into `Prompt`, supplies input, a concrete tool pool, a run-scoped `Store`, and `RunOptions`, then awaits `execute::run`. There is no executor object and no state outliving the call.

The crate currently exposes `client`, `execute`, `lua`, `observe`, `parser`, `store`, `subst`, and `tools`. The crate root re-exports `Error`, `Result`, and `promptforge_version`.

### Current prompt and execution lifecycle

A prompt currently consists of YAML frontmatter, an optional H1 title, and one or more H2 sections. Each section may have one leading Lua fence and prose. Top-level sections execute in file order. Child sections parse but do not execute.

Each section currently gets a fresh Lua VM. Its Lua can read `args` and `sys`, write `var`, scope concrete tools with `tools.add`, and use the run-scoped virtual store. A scalar top-level return ends the run. Otherwise substituted non-empty prose enters the model tool loop. Falling off the final section returns `default_return`, the last model reply, or `"done"` in that order.

The `Store` remains the sole intentional mutable channel across section boundaries. Direct `Store` calls are ordinary library operations and are unobserved. Store calls made through the Lua execution harness are observed without exposing paths or contents.

### Stable live-tool identity

Every callable `Tool` exposes a stable `ToolId` consisting of a server and a name in that server's namespace. The built-in live identities are `("promptforge", "web_search")` and `("promptforge", "web_fetch")`. Identity is structural and remains independent of the concrete wire name used by the current model transport.

`Tool` also exposes its exact model-facing description and parameter schema. `ToolRegistry` preserves supplied order and repeated identities, and resolves live instances by `ToolId`. It intentionally does not reject repeated IDs: atomic collision validation belongs to the later binding-uniqueness step.

The existing executor still scopes, advertises, and dispatches tools by `wire_name` until alias binding ships. The wire name is therefore a temporary transport detail, not an identity key for new APIs.

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

`section` identifies the current H2 section. Run-wide activity uses the H1 title after parsing. Until the planned required-H1 grammar ships, a prompt without an H1 uses the fixed label `Prompt`.

`detail` is a stable operational statement from `observe::detail`. The shipped vocabulary reports run start and outcome, section start and finish, model turn outcome, tool call outcome, and every Lua-harness store operation outcome.

Reports are synchronous, non-blocking, report-only, and never consulted for a decision. Recording and null observers must produce identical results, errors, execution order, and store side effects.

Reports never contain raw prompt prose, model input or output, tool arguments or results, store paths or contents, credentials, or fetched content. Current details intentionally carry no payload metadata.

The MCP adapter recognizes only pinned exact details for cosmetic progress and turn counting. `Run started` emits progress zero with the H1 title, and each `Section started` increments progress with the H2 heading. Unknown details are tolerated and logged at debug level. Recognition never affects execution.

### Current safety and failure behavior

The Lua VM loads only the selected safe standard libraries, removes code-loading and reflection globals, and enforces an instruction budget. Tool output marked untrusted is nonce-framed before it enters model history. Expected parse, Lua, substitution, model, tool, and store failures return errors rather than panicking.

The version gate runs before observation begins. A refused source emits no reports. Once a run begins, success and failure both produce a final run observation.

## Planned, not shipped

Everything in this section is settled design for later steps. None of it is implemented by the report-only observer change.

### Capability binding and picker validation

Prompt-local capability aliases will bind one-to-one to the shipped live `ToolId` values through `promptforge-tool-picker`. Binding will reject absent, duplicate, ambiguous, colliding, and registry-mismatched identities. Before a model turn, core will apply the shipped picker near-duplicate analysis to the effective scope.

### Compiled Lua and persistent section VMs

`LuaProgram` will retain source and process-local Lua 5.4 bytecode compiled once at parse time. `SectionVm` will own one isolated VM per section and preserve one environment across shared-library load, preamble, model await, `reply` binding, and epilog. Bytecode will never be persisted and Lua memory will never cross sections.

### Required H1 and three-phase grammar

The grammar will require one H1, allow one immediately following `lua prompt` shared-library fence, and split each H2 into an optional leading Lua preamble, prose, and optional trailing Lua epilog. Exact fence forms will reserve executable regions while other fences remain prose. YAML frontmatter will no longer carry concrete tools.

### Semantic capability phases

H1 code will declare `tools.need(alias, description)` and optional prompt-wide `tools.always(alias)`. H2 preambles will scope declared aliases with `tools.add(alias)`. Needs will bind once before execution and replay from immutable cached bindings without additional embeddings.

The model will see selected concrete descriptions and schemas under prompt-local aliases. Calls will dispatch through `ToolId`. Before a model turn, the picker will reject near-duplicate tools in that section's effective scope.

### Complete section lifecycle

Execution will run shared bytecode, the H2 preamble, effective-scope validation, the model turn, `reply` binding, and the epilog in one section VM. A scalar preamble return will skip model and epilog. A scalar epilog return will finish after the model. The VM will then be destroyed.

Hosts will parse, bind, and execute in separate phases while passing the same observer reference through each phase. Async hosts will move synchronous binding to `spawn_blocking`. No mutex guard will cross an await.

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
9. Later lifecycle behavior remains planned until its owning step lands with tests and documentation.

*2026-08-05 23:45 - GPT-5.6 Sol*
