# Non-authoritative `promptforge-core` design residue

> Historical material only. This file is not a grammar, API, or behavior reference. The authoritative current design is `design-core.md`.

This residue preserves the meaningful concepts from the predecessor design that were superseded by the live-H1 and ordered-section lifecycle. The predecessor files themselves were archived outside the repository.

## Superseded execution model

The original runtime was described as a simpler fall-through executor:

- `Prompt::parse(&str)` produced an inert tree without an execution id, observer, parse-time Lua compilation, H1 program, or shared library.
- H1 contained only a title and descriptive prose.
- Each H2 had at most one leading Lua fence and one prose body.
- A fresh VM executed the single Lua block, then the executor substituted and sent the prose through one tool loop.
- Top-level H2 sections ran only in file order. There was no `execute`, `jump`, or programmable graph.
- Child sections parsed but never executed.
- A section-local `var` table was rebuilt empty at every H2 boundary.
- The only early exit was a scalar return from the section's single Lua chunk.

The current design replaces this with live ordered H1 blocks, one optional `lua shared` library, alternating Lua and prose blocks in each section, explicit `execute` and `jump`, and captured H1 `var`.

## Superseded capability and model binding

The original host supplied a flat tool pool and section Lua called `tools.add` with concrete tool names. Missing names failed after Lua execution. There was no semantic picker, stable tool identity distinct from wire name, prompt-local alias map, near-duplicate analysis, model catalog, or explicit model selection.

The original gateway client owned one model name, either supplied directly or read from `PROMPTFORGE_MODEL`. A prompt could not declare model capabilities, context, thinking, temperature, maximum tokens, or dialect. The current design resolves tool and model needs live under semantic aliases and freezes model invocation details into bindings.

Historical frontmatter also included fields no longer in the current core contract:

- `version`, an author-owned interface version
- `tools`, a list parsed for CLI or MCP callers

The current frontmatter keeps `name`, `description`, `promptforge`, `default_return`, and `max_tool_iterations`.

## Superseded Lua and substitution surface

The original section VM exposed only:

- `args`
- `sys.when`, `sys.now`, and `sys.id`
- a new empty `var`
- `tools.add`
- `store`

Substitution recognized only `{{ args }}`, `{{ var.* }}`, and `{{ sys.* }}`. It had no `reply`, expanded system metadata, model objects, task objects, tool-call counts, inference method, logging callback, fanout, execute, or jump.

The earlier sandbox description emphasized a hand-maintained Lua 5.4 blocklist and an approximately ten-million-instruction budget with no memory ceiling. Those implementation cautions remain historical rationale, but they do not override the current design's restricted-VM contract.

## Superseded store contract

The original store exposed six operations: `write`, `append`, numbered `read`, `str_replace`, `delete`, and `glob`. Its `read` name meant numbered output.

The current store separates three read shapes:

- `read_lines` for numbered navigation
- `read` for verbatim trusted content
- `inject` for verbatim content inside an untrusted envelope

It also exposes `exists(path)`. The predecessor's principle that the caller owns one run-scoped `StoreRef`, shared across otherwise isolated sections, remains current.

## Superseded observer and client contracts

The original observer accepted an owned `Event` enum through `on_event`, including run, section, model-turn, tool-call, and final aggregate events. Events were serializable records with payload fields.

The current observer receives borrowed `(execution, section, detail)` strings, uses a fixed payload-free vocabulary apart from constrained Lua checkpoints, and delegates raw request and response capture to opt-in `DebugCapture`.

The original client spoke only the OpenAI-shaped native tool-call format. The current design routes tool preparation, parsing, and history echo through model-selected dialect implementations.

## Recovered-only material

The recovered predecessor contained only a provenance notice stating that it described the runtime before the section-lifecycle migration. It supplied no additional architecture, grammar, API, or behavior beyond that historical classification.

## Historical non-goals and cautions

The predecessor explicitly listed these then-unbuilt features: slots, extensions, declared outputs, control-flow jumps, tasks, fanout, preconditions, postconditions, structured run state, validation, and persistence beyond a run. Several are now implemented in different forms, especially capability bindings, `jump`, `execute`, and fanout. The remaining statements are historical and must not be treated as current exclusions.

The predecessor also recorded open measurement questions around the Lua instruction budget, memory ceilings, and the default 24-round model cap. The current design retains the model-loop default while allowing frontmatter override, but this residue records that the original numeric choices were guards rather than empirically tuned budgets.
