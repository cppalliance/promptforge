# `promptforge-core` design residue

> Historical and contradictory material only. Nothing in this file defines the current grammar, API, behavior, or intended architecture. Use `design-core.md` for the current design.

## Provenance

This residue consolidates material removed from three predecessor files:

- The prior `design-core.md`, which mostly described the current section-lifecycle architecture but also contained stale absolutes and implementation-sensitive claims.
- `design-core-orig.md`, a full design of the runtime before live H1 resolution, ordered section blocks, prompt-local model binding, explicit control flow, and structured fanout results.
- `design-core-recovered.md`, whose only recovered content was a provenance warning that it described the runtime before the section-lifecycle migration. It contained no additional recovered design claims.

The exact predecessor files remain recoverable in the workspace trash area.

## Superseded runtime shape from `design-core-orig.md`

The original design described all of the following as current. Every item below is superseded:

- `Prompt::parse(&str)` with no execution id or observer.
- `execute::run(prompt, args, tools, store, opts)` without `ResolutionContext`.
- `RunOptions` containing only observer and client.
- A frontmatter author `version` field and a frontmatter `tools` list consumed outside core.
- H1 prose as inert `description_text`, not a live block program.
- Exactly one leading section Lua fence followed by one prose body.
- Exactly one model round-trip loop per top-level section.
- Pure file-order fall-through with no `execute`, `jump`, or fanout.
- Child sections that parse but can never execute.
- A fresh empty `var` table for every section and no previous reply exposed to Lua or substitution.
- Tool selection by concrete name from the caller's pool, with no semantic picker, stable identity binding, prompt-local alias, model catalog, or near-duplicate analysis.
- A single gateway model selected from `PROMPTFORGE_MODEL`, rather than prompt-local resolved Model objects.
- An observer receiving a serialized `Event` enum through `on_event`.
- Store `read` returning numbered lines, with no separate `read_lines`, verbatim `read`, `inject`, or `exists`.
- A `Tool` trait whose public identity was effectively its concrete name and which lacked the current stable `ToolId` separation.
- OpenAI-only tool history behavior with no dialect abstraction.
- No debug-capture seam and no run-scoped cancellation facility.

## Historical alternatives and rationale

These notes explain choices considered by the old design. They are retained as history, not current decisions:

- Structured input validation through a JSON Schema was considered as an alternative to one raw `args` string.
- Queryable typed run state was considered as an alternative to virtual files. The old rationale favored path-addressed text because prompt authors commonly hand prose between stages.
- A language-level allowlist sandbox was considered as an alternative to hardening Lua 5.4 by removing globals.
- A Lua memory ceiling was identified as desirable but uncalibrated.
- The Lua instruction budget and 24-round model cap were acknowledged as first-cut guards rather than measured limits.
- Splitting parse, validation, transport, and execution failures into separate public error types was considered but not adopted in the old runtime.
- An executor object was considered unnecessary because run resources are caller-owned and run-scoped.

## Contradictions removed from the prior canonical document

The previous canonical document contained several formulations too absolute or too coupled to transient implementation details:

- "No defaults. Everything explicit" conflicted with the default model-loop cap and gateway environment fallbacks. The current design preserves explicit prompt capability selection while documenting operational fallbacks.
- It described the old original document as remaining beside the canonical file. Consolidation moved all predecessor files to recoverable trash.
- It said only store and reply were mutable cross-section state while elsewhere documenting captured H1 `var`. The current design distinguishes the shared store, H1 seed state, and prior reply string.
- It implied all explicit Lua inference reports through the caller's observer. The current implementation's inference hook has its own reporting limitation, so the canonical design does not promise that detail.
- It described store access as both Lua-only and available to later model file tools. Current core exposes store operations to Lua and does not define model file tools.
- It treated the exact internal `model:infer` Tokio bridging technique and schema-cache mechanics as architectural API. The canonical design retains behavioral intent and only the cache invariant that affects scope freshness.
- It stated that every `execute` subroutine receives a fresh copy of captured H1 `var`. The current top-level section path explicitly receives the H1 seed, while the subroutine path is not described that way consistently in source. The canonical design does not promise an unsupported copy rule.
- It generalized ordered-block execution to fanout workers. The current fanout implementation still addresses workers through classic prologue, final prose, and epilog projections.

## Historical API and behavior details retained only for archaeology

The old design recorded these concrete implementation facts. They may help explain old tests or commits, but must not guide new prompt authoring:

- `GatewayClient::from_env` used `PROMPTFORGE_TOKEN`, `PROMPTFORGE_BASE_URL`, and `PROMPTFORGE_MODEL`, with a localhost base URL and public default model fallback.
- The old observer variants were `RunStarted`, `SectionStarted`, `SectionFinished`, `ModelTurn`, `ToolCalled`, and `RunFinished`.
- The old parser tolerated skipped heading levels and stripped a leading byte-order mark. Those behaviors may still exist, but their presence in the old contract is not evidence of current architectural importance.
- The old sandbox estimate was a hook every 10,000 instructions, aborting after 1,000 firings, with no memory ceiling.
- The old store consisted of `write`, `append`, numbered `read`, `str_replace`, `delete`, and `glob`.
- The old crate shipped `tools::web_search::WebSearch` as its sole built-in tool and routed it through the gateway.
- The old in-process Axum gateway fixture was the primary offline tool-loop test strategy.

## Recovered-only material

The recovered file supplied no lost sections. Its complete substantive claim was that it was a historical recovery artifact from before the section-lifecycle migration and was not authoritative. That provenance is preserved here.
