---
name: PromptForge Orchestrator Design
overview: Markdown-driven pipeline runtime (Rust + mlua). A prompt is a function (parameters in, a string out, side effects possible) shaped as a bounded linear pipeline - H2 steps run top to bottom, `---`-marked H2s are subroutines, forks/cycles/fanouts allowed; H3-H6 are document structure, not steps; larger programs compose from multiple prompt files. A file is a promptforge prompt only if its YAML frontmatter declares a `promptforge:` version; promptforge offers a detection function and runs only its own prompts (plain prompts without that version are the caller's concern, not promptforge's). Control flow is deterministic and declared in Lua (cycles legal, bounded by budgets); state lives in a run-scoped store/VFS shared by Lua and the model; capabilities (files, shell, network) are injected by the host (sandbox or real). Design docs in promptforge-design are provisional; this plan and the conversation override them.
todos:
  - id: mark-docs-provisional
    content: Add provisional status to design document headers. This plan overrides them.
    status: pending
  - id: fix-reachability
    content: Sections can be dormant, cyclic, freely organized. Unreachable is not an error.
    status: pending
  - id: build-version-gate
    content: "Rung 1: detection function for the `promptforge:` version; run only promptforge prompts (absent = not ours, caller handles), refuse an unsupported major."
    status: pending
  - id: build-store-vfs
    content: "Rung 2: run-scoped store/VFS in core (write/append/read-numbered/str_replace/delete/glob), Lua + model equal access, capability injection (MemVfs vs RealFs). Files-first."
    status: pending
  - id: build-substitution
    content: "Rung 3: one-pass {{ }} resolver over args, sys (now/date/elapsed/id), var, reply, result (scalars stringify, tables -> JSON, nil hard error, no formulas). Parameterized single-step prompts."
    status: pending
  - id: build-control-flow
    content: "Rung 4: fall-through + goto + pass + task + reference blocks (sect.block/sect.list) + result/injection + the three budgets (nesting/step/tool). Deterministic Lua transitions; cycles legal."
    status: pending
  - id: build-fanout
    content: "Rung 5: fanout (homogeneous over a list, heterogeneous over tasks) + keyed-store reduce + structured store. Runs Diligence/Briefer."
    status: pending
  - id: build-cyclic
    content: "Rung 6: cyclic state-machine pipelines (mentograph-shaped) driven by Lua conditionals. Model-surface transfer stays deferred."
    status: pending
  - id: generate-design-doc
    content: "Final step, after all rungs: spawn one subagent to read this plan, grep <design-doc>, and write design-promptforge-orchestrator.md from the finished build; it supersedes the provisional promptforge-design docs."
    status: pending
isProject: false
---

# PromptForge Orchestrator

A Rust runtime that executes pipelines defined in a single markdown file. The markdown is the program, the model is the CPU, embedded Lua (mlua) is the microcode, the Rust harness is the decoder. A new pipeline is a new markdown file, not new code.

A prompt is a **function**: parameters in, a string out, side effects possible. It is a **bounded linear pipeline** - forks, cycles, and fanouts are allowed, but it is not a general recursive program and it has a natural size limit. Larger programs are composed from multiple prompt files, each called as a function. **H2 headings are the pipeline steps; H3-H6 are document structure within a step, not steps themselves.**

Design documents in `promptforge-design/design/` are **provisional**; this plan and the conversation override them.

## Built so far

Parser (recursive H2-H6 + frontmatter + leading Lua fence), the gateway (holds vendor + Brave keys, routes models, `/v1/tools/web_search`), `promptforge-webfetch` crate (SSRF-guarded `web_fetch`), the tool-call loop with a configurable cap (`max_tool_iterations`), per-section opt-in tool scoping via `tools.add`, guard-wrapping of untrusted tool output, and a live multi-turn research prompt. Everything below in "Control flow, blocks, and state" is unbuilt design except where noted.

## File format and versioning

A **promptforge prompt** is identified by a `promptforge:` version in its YAML frontmatter block - an integer major (or a semver whose **major** is the compatibility axis). promptforge exposes a **detection function** that reports that version (or its absence) for any source text, and it **runs only its own prompts**.

- **`promptforge:` present** -> a promptforge prompt. Run it under that major's rules (sections, Lua, transfers, store, budgets); everything below describes major **1**. An unsupported major is **refused, never silently degraded**.
- **`promptforge:` absent** (no key, or no frontmatter at all) -> **not** a promptforge prompt. Detection reports nothing and promptforge declines to run it; what to do with a plain prompt (hand it to an ordinary harness, etc.) is the **caller's** concern. promptforge does not execute plain prompts.

The engine major is distinct from any author-facing `version:` for the prompt's own revision. Detection is lenient: malformed or absent frontmatter simply reads as "not a promptforge prompt", never an error.

## Control flow, blocks, and state (consolidated)

### Section model, headings, and reach

**H1 is the document title** (the program's name). **H2 headings are the pipeline's steps** - the only execution units, transfer targets, and addressable sections. **H3-H6 organize the enclosing H2's material**: they load as part of their H2's prompt and are readable as data (`sect.block`/`sect.list`/`sect.children`), but are never executed or transferred to. Beyond prose organization, they are how a step carries its own structured input - most importantly, the items a step maps over. `fanout("check-rule", sect.children())` runs one worker per H3, each H3's content as that worker's brief; `sect.list` does the same for a bulleted list. So H3s under a step are both its documentation and its fan-out input. An H3 is **private to its step** - only that step reads it. A list two steps must share is therefore promoted to a `---` H2 definition (global, addressable by name): H3 = private fan-out input, shared `---` H2 = shared fan-out input.

**Heading = identifier + comment.** An H2 heading's first whitespace-delimited word is its **identifier**, lowercase-normalized, matching `[A-Za-z][A-Za-z0-9_-]*` (kebab or snake); anything after it is a human comment, shown in the outline but ignored for addressing. So `## Summarize-Research map reduce to get report` has identifier `summarize-research`. An H2 whose first word is not a valid identifier is a load-time error. All addressing (`goto`/`task`/`fanout`) uses the identifier, e.g. `task("summarize-research")` - never the `##` prefix or the comment. H3-H6 may carry an identifier the same way when you want to read them as data; a decorative subheading needs none.

**Statement vs definition.** An H2 is a **statement** by default and runs in the top-to-bottom pipeline. An H2 immediately followed by a `---` line (only whitespace between them, before the body) is a **definition**: skipped by fall-through, still callable by name - like `def`, a subroutine that runs only when `task`/`goto`/`fanout` names it, then (for `task`) returns. The `---` rides on the heading, so cut-and-paste keeps a definition a definition. The H2 statement sequence is the program's `main()`; definitions are its subroutines.

Each section is **prose** (instructions the model reads) plus up to two optional **Lua blocks**, split by when they run:

- **Leading config block** - the *first* fenced `lua` block, with only whitespace (or the optional `---` definition marker) between the heading and its fence. Runs **before** the model turn: `model`, `tools.add`, `assert` (precondition), `var`, `context.inject`. It is the section's preamble.
- **Trailing exit block** - the *last* fenced `lua` block, with **only whitespace between it and the next heading or EOF**. Runs **after** the model turn, so it sees the model's output (available as `reply`), and its return value is the section's exit. It reads like the flowchart node's out-edges under the section's action, and it **subsumes `check()`** (post-turn logic lives here). The "model turn" is the whole tool-call loop: it ends at the **first assistant message with no tool call** (its text is the section's output), and the exit block runs exactly once, then. Hitting the tool cap is a fatal `ToolLoopExhausted`, not an exit - the block does not run. Conversational multi-turn (a human in the loop) is therefore not one multi-turn section but a `goto` cycle of single-response sections, re-running the leading Lua on each re-entry.

A section may have either block, both, or neither; both are position-anchored (not "any `lua` fence") so a code sample in the prose is never mistaken for config or exit. **Every entry into an H2 starts a fresh context** - whether reached by fall-through, `goto`, `task`, or `fanout` - so no step ever inherits another step's conversation. Steps communicate only through the store, files, `args`, and the per-transfer `result`. Cycles are legal, dormant definitions are legal, unreachable steps are not an error.

**Reach.** The pipeline is flat: every H2 may transfer to any other H2 in the file (statement or `---` definition). H2 identifiers are **unique across the file** (a duplicate is a load-time error). There is no nesting and therefore no scope resolution - one level, all names visible. Cycles are allowed (e.g. the mentograph Elicit/gate/Walk loop); termination is guaranteed by budgets, not by structure.

### Control flow

All transfer is **deterministic and declared in Lua**; the model does work inside a step (calls tools) but **never routes**. `goto`/`task`/`fanout` are **Lua-only verbs** - never in the model's tool set, so the model cannot delegate or route on its own. (Model-callable delegation - a "prompted task" - is a separate, deferred design; see To explore later.) Fall-through runs the **next H2**; falling off the last statement ends the run. `task` calls a `---` definition and **returns** to the call site; `goto` is a one-way jump; `fanout` maps in parallel. Every deviation is a step's Lua chunk return value, so reading the Lua reconstructs the whole control graph.

**Sync verbs**

| Verb | Form | Effect |
|---|---|---|
| *(fall-through)* | chunk returns `nil` | run the next H2 statement; at end of file, end the run |
| `pass` | `pass(inject)` | fall through to the next H2, injecting `inject` as its `{{ result }}` (no naming) |
| `goto` | `goto(target, inject?)` | one-way jump; context clears |
| `task` | `task(target, inject?)` | call-and-return; fresh context; blocks; returns only the result (a value or store reference), not its transcript |
| `fanout` | `fanout(target, list)` or `fanout(tasks)` | parallel map + keyed reduce |
| `return` | `return value` | end the run |

**Async verbs** (future tranche)

| Verb | Form | Effect |
|---|---|---|
| `spawn` | `spawn(target, inject?)` | launch in background; returns an id |
| `await` / `await_all` | `await(id)` / `await_all(ids)` | block for one / all results |
| `cancel` | `cancel(id)` | kill a spawned task |

**Exit & routing.** Every `target` is a section **identifier**, resolved by the reach rule. A section's exit is its **trailing** block's return (post-turn, so it sees the model output and subsumes `check()`), else its leading block: `nil` -> fall through with no payload; a `pass(v)` descriptor -> fall through to the next H2 with `v` as its `{{ result }}` (successor unnamed); a `goto`/`task`/`fanout` descriptor -> that transfer; a plain value -> end the run with that value. The model's only influence on flow is *data* - it files a fact/record, Lua reads it and routes - so branching is always a visible Lua condition, never model-decided. Cycles are first-class, bounded by budgets. Model-surface transfer stays deferred (it reintroduces invisible routing; revisit only if a real workload needs a model-callable `task`).

**Targets & injection.** The **injection** - the second arg to `goto`/`task`, or the sole arg to `pass` - is a string prepended to the target's fresh context (for `task`, the worker's brief). `{{ }}` in it resolves in the **caller's** scope at the call site; the target reads it as `{{ result }}` (nil only on a bare `nil` fall-through). To forward the model's own output, pass `reply` (the exit block's handle to the turn's output), e.g. `pass(reply)`. This is the "poor man's argument list" - call-site parameters with no schema, and `pass` gives the same to the unnamed next step.

**`fanout`.** Homogeneous `fanout(target, list)`: each element is one worker's injection on the same target (same prompt, different data; `list` from `sect.list`, `sect.children()`, or a Lua table). Heterogeneous `fanout(tasks)`: a list of `{target=, inject=}` when target/brief vary, each brief carrying an explicit **boundary** ("you own X, not Y"). Each worker runs fresh and writes a **keyed slot** (index / content-hash) in a store collection - never blind append; the runtime **refuses clobber**, holds a **barrier** to completion, then the reduce section reads the collection **sorted by key** (a strong model does the reduce). Bounded concurrency (harvest-as-ready), `tolerated_failures` + per-item status + **retry-only-the-failed**, workers pass references/artifacts not blobs.

**`task`.** Agent-as-tool, not handoff: the caller keeps **control of the flow** (the run returns to the caller's routing), *not* a shared context. The target runs in a **fresh context** with the **injection brief only** (never caller history) and returns only a **distilled result** - its final answer as a single value/string, or a reference to what it wrote in the store, per a declared result contract - never its raw multi-turn transcript. "Distilled" is the point: the caller inherits the answer, not the sub-agent's tokens, so delegation never bloats the caller. The caller's own model turn already ended at its exit; `task`'s return value is a Lua value its exit logic stores or forwards - the caller's conversation is never resumed or extended. A `task` target is a single `---` H2 definition; it runs and returns to the call site (multi-step reusable logic that outgrows one H2 belongs in a separate file, called as a function). Content-keyed identity (`hash(target, inject)`) enables dedup and safe retries. `task` is the reuse verb (call a shared section in a Lua loop); `fanout` is the batch verb. Async `spawn` tasks live for the run, file into the store as they go, and are cancelled at run end (no auto-join); patterns: background worker, self-looping worker, model-managed ids.

### Substitution namespaces

`{{ }}` runs in Rust after Lua, before the model turn - one pass, no recursion, no formulas; scalars stringify, tables become JSON, nil is a hard error.

| Namespace | Source |
|---|---|
| `{{ args }}` | the raw input string (run-global) |
| `{{ sys.* }}` | system metadata: `sys.now` (RFC3339), `sys.date`, `sys.elapsed` (since run start), `sys.id` (run id) - resolved at each step's prompt build |
| `{{ state.x }}` | accumulated tool-call state |
| `{{ var.x }}` | per-section table |
| `{{ result }}` | value/injection passed by the caller |

### Reference blocks and list extraction

A reference block is a section read purely as data - nothing transfers to it. It comes in two scopes: an **H3-H6 block private to its enclosing H2** (a step reads its own subheads via `sect.*`), and a **`---`-marked H2 definition that is global** (readable by name from any step, position irrelevant). That is the sharing rule in one line: private fan-out input is an H3; input two steps share is promoted to a `---` H2.

Two accessors:

- `sect.block("name")` -> the section's prose as raw text.
- `sect.list("name")` -> the bulleted/numbered lines in that section, in order, split into an array of strings (other prose ignored).

`sect.block()`/`sect.list()` resolve against the current step's H3-H6 blocks plus the file's `---` H2 definitions; an identifier matching neither is a load-time error. XML tags are not used for harness-level structure - they stay free for the model/author (emphasis, nonce delimiters, injection defense). This is what lets a list drive a fanout: `fanout("check-rule", sect.list("source-quality"))`.

### Store and files (in core)

- **Virtual files are the primary bulk-state mechanism**, run-scoped, and **shared by both the model's file tools and Lua** (equal access). Ops: `write`/`append`/`read` (numbered lines) / `str_replace` (anchor-based unique-or-error edit) / `delete` / `glob`. Edit-in-place is anchor `str_replace`, not line/char offsets (the pattern that actually works for models); numbered reads are for navigation and error messages only.
- **Structured store** exists for one operation: group-by on a typed field across ~30-45 records (Diligence/Briefer/Assay). Files suffice below that.
- **Capabilities are injected by the host.** A `FileStore` trait has two backends: `MemVfs` (sandbox) and `RealFs` (rooted at a workspace, optionally read-only). The CLI/embedder chooses - sandbox for untrusted-content pipelines, real for IDE/agent use. Shell and network are the same shape (off / allowlisted / open). The model never names a real path; "disable the sandbox" = the host wires in the real backend. Presets: `sandboxed` (default) and `trusted`.
- **Real-file input** is mounted by the trusted launcher at the boundary: the CLI reads the file (eagerly, by content) and seeds the store at a logical path; the model reads it via `read_file`. Output promotion to real disk is an audited caller step, never a model tool.
- **Guard-wrap (built)**: a tool whose `untrusted_output()` is true (`web_fetch`) has its result wrapped in `<untrusted_input_{nonce}>...</untrusted_input_{nonce}>` with a data-not-commands rule before it reaches the model. Reduces injection; does not remove the `web_fetch` URL exfiltration channel - isolation (scoped, data-free web-reading sections) is the hard control.

### Budgets (runaway guards, three separate axes)

Backstops, not the termination mechanism - primary termination is the explicit Lua conditions. Set generously; each error names where it tripped; none defaults to infinity; a hit is a visible typed error, never a silent best-effort continue.

| Budget | Counts | Catches | Scope |
|---|---|---|---|
| **Nesting limit** | active `task` call-depth | unbounded/mutual recursion via `task` | per run, max depth |
| **Step budget** | total transitions (`goto`+`task`) | non-terminating `goto` cycles | per run, cumulative; nested `task` steps decrement the *global* budget |
| **Tool budget** | tool calls | a section/run hammering tools | per section (`max_tool_iterations`, built) + per run |

## Host objects and functions

| Name | Access | What it is |
|---|---|---|
| `args` | read | the single raw input string, run-global |
| `sys` | read | system metadata: `sys.now` (RFC3339), `sys.date`, `sys.elapsed` (since run start), `sys.id`; fixed within a step, re-resolved on each entry |
| `reply` | read (exit block) | the model's output from this step's turn |
| `state` | read | accumulated tool-call state |
| `store` | read (query) | `count`, `exists`, `get`, `filter`, `group_by` |
| `var` | read-write | `{{ var.x }}` substitution table |
| `tools` | read-write | scoped tool-set builder (`tools.add`) |
| `context` | write | inject text into the model's initial prompt |
| `sect` | read | `sect.children()` (the current H2's H3 blocks, for fanout), `sect.block(name)`, `sect.list(name)` |
| `progress` | write | mid-section observer events |
| `model` / `assert` | Lua-only | pick model slot / precondition (both in the leading config block) |
| `json.encode` / `json.decode` | Lua-only | table <-> JSON |

## Tranche ladder

1. Version gate: detection function for the `promptforge:` version; run only promptforge prompts (absent = not ours, declined for the caller), refuse an unsupported major.
2. Run-scoped store/VFS in core (files-first, Lua + model equal access, capability injection).
3. Substitution engine + params: the one-pass `{{ }}` resolver over `args`, `sys` (now/date/elapsed/id), `var`, `reply`, `result` - scalars stringify, tables -> JSON, nil is a hard error, no formulas. Delivers parameterized single-step prompts.
4. Fall-through + `goto`/`pass`/`task` + reference blocks (`sect.block`/`sect.list`) + `result`/injection + the three budgets.
5. `fanout` (homogeneous list + heterogeneous tasks) + keyed-store reduce + structured store. Runs Diligence/Briefer.
6. Cyclic state-machine pipelines (mentograph-shaped), Lua-conditional transitions.
7. (Future) `spawn`/`await`/`cancel`.

## Build method

Designed per `architect.md`, built per `vibe-how-to.md`; both govern this plan and the agent's behavior.

- Each **rung** above is a high-level component in dependency order (resolution levels 1-2). At build time a rung is zoomed into **steps** - each the largest slice of behavior one test covers - and each step is one commit carrying code, test, and docs (levels 3-4). Explode one rung at a time, not the whole plan up front.
- Work every step in **subagents**: one writes the code, a second reviews the diff in a fresh context against the general code-review checks plus the project `<review>` block below and writes findings to `vibe-review.md`, a third applies the fixes. Git (stage/commit/amend) stays in the main context.
- Make **reversible** choices in-flight and log each with its falsifier; **escalate irreversible** ones. Every commit moves toward the goal; a bug traced to an earlier commit is fixed in its own commit.
- The plan must stand alone before it runs - it may reference files by path but nothing that lives only in chat.

## Review checks (project-specific)

<review>
Read after the general code-review block; applied to the commit's diff.
1. Rust follows `tools-public/how-to/rust-how-to.md` (layout, ownership, errors, API/semver, docs, testing, lints): public error enums and their data-carrying variants are `#[non_exhaustive]`; errors are typed via `thiserror` with lowercase no-period `Display`; no `unwrap`/`expect` outside tests; `?` with `From` over manual matches; lints live in `[lints]`/`[workspace.lints]`, not crate-root `#![deny]`; every public item documented with `# Errors`/`# Panics`/`# Safety` and doctest examples.
2. Public items are documented and rustdoc runs clean under `-D warnings` (no intra-doc links to private items).
3. Any tool returning model-visible content from an untrusted source overrides `untrusted_output()` and its result is guard-wrapped with the per-section nonce.
4. Capabilities (files/shell/network) are host-injected; the model never names a real path; the sandbox backend is the default.
5. The mlua sandbox keeps its instruction budget and restricted library set; no new global escapes it.
6. No secrets, keys, or vendor URLs in code or prompts; they resolve through gateway/host config.
7. Versioning: a file under an unsupported `promptforge:` major is refused, never silently degraded.
</review>

## Rung 1 steps (version gate)

promptforge detects its own prompts and runs only those; a plain prompt (no `promptforge:` version) is declined for the caller to handle - promptforge never runs one. Supported major = 1. Each step is one commit (code + test + docs), written/reviewed/fixed in subagents.

1. **Detection + frontmatter field.** Add `pub fn promptforge_version(source: &str) -> Option<u32>` returning the `promptforge` major when the source has a YAML frontmatter block declaring it, else `None` (no frontmatter, malformed YAML, or key absent -> `None`; never errors). Add `promptforge: Option<u32>` (`#[serde(default)]`) to `Frontmatter` so the full parse also captures it (leave `name`/`description`/`version` unchanged). Test: present -> `Some(n)`; key absent -> `None`; plain text with no frontmatter -> `None`; malformed frontmatter -> `None`.
2. **Version support in the run entry.** Add `Error::UnsupportedVersion(u32)` (lib.rs). At the top of `execute::run`, read `prompt.frontmatter.promptforge`: `Some(1)` -> proceed (current behavior); `Some(other)` -> `Err(UnsupportedVersion(other))`; `None` -> `Err(Parse("not a promptforge prompt: no promptforge version"))`. Add `promptforge: 1` to the inline markdown in existing `execute` tests so they still run. Test: major 1 proceeds; major 2 -> `UnsupportedVersion`; missing -> the not-a-prompt error.
3. **CLI gate + docs + fixtures.** In the CLI `run`, call `promptforge_version` first; if `None`, report that the file is not a promptforge prompt and exit non-zero; otherwise parse + run as today. Add `promptforge: 1` to the fixture prompts under `prompts/`. Update README + module docs (fix the stale "first/entry section" and the `max_tool_iterations` default drift). Test: existing CLI tool-selection test passes.

## Rung 2 steps (store/VFS)

Logged decisions (reversible): the store is a run-scoped, cheaply cloneable `Store` handle wrapping `Arc<Mutex<dyn FileStore>>`, because async `Tool::call` (`&self`, crosses `.await`) and the sync Lua VM both touch it - `RefCell` is insufficient. Lua gets an **always-on `store` table** (a deterministic host capability, like `sect`/`var`); the **model** gets file access only through normal per-section scoping (`tools.add`). Paths are logical strings; `read` returns numbered lines (navigation/error messages only); `str_replace` is anchor-based, unique-or-typed-error. Falsifier: if lock contention or ergonomics bite, revisit the handle type. Each step is one commit (code + test + docs), written/reviewed/fixed in subagents.

1. **FileStore trait + MemVfs (core `store` module).** Define `FileStore` with `write`/`append`/`read` (numbered lines)/`str_replace` (unique anchor or typed error)/`delete`/`glob`, each returning typed results; an in-memory `MemVfs` backend; and a cloneable `Store` handle over `Arc<Mutex<dyn FileStore>>`. No execution wiring yet. Tests: each op, plus `str_replace` not-found/ambiguous, `read` numbering format, `glob` matching, delete-missing.
2. **Thread the store + Lua `store` API.** Add a `Store` to `execute::run` (created once per run) and to `lua::run_chunk`; expose a Lua `store` table (the six ops) over the shared handle. Update the CLI to build a `MemVfs`-backed `Store`; update execute/lua test helpers. Test: cross-section persistence (section A's Lua writes, section B's Lua reads it); each op via Lua.
3. **Model-facing file tools.** `Tool` impls (`write_file`/`read_file`/`append_file`/`str_replace`/`delete_file`/`glob`) over the same `Store`, added to the run pool and opt-in per section via `tools.add`. Test (mock gateway): the model writes then reads a file; a later section's Lua sees it; an unscoped section cannot call them.
4. **RealFs backend + CLI selection.** Add `RealFs` (rooted at a workspace dir, optional read-only) with path-traversal refusal; the CLI selects the backend (default sandboxed `MemVfs`; a flag/env picks a real root). Test: rooted ops; `..`/absolute-path escapes refused; read-only refuses writes. (Real-file input mount and audited output promotion are a later sub-step.)

## Key design principles

- **The orchestration layer is an ordinary computer; the model is its one stochastic instruction.** Sections are statements, `---`-marked sections are definitions (subroutines/data), `task` is call-and-return, `goto` is a jump, `fanout` is a parallel map, cycles are loops, the store is memory, headings are identifiers, and the budgets are the stack and fuel limits. Deterministic scaffolding around a noisy unit whose output the trailing block validates.
- **Context-clearing** on every transition; state externalized to the store. (Evidence: a scoped harness + externalized state took a 1.5B model from 1% to 93.5% - the harness matters more than model size, which is promptforge's whole bet.)
- **Deterministic control flow in Lua**; the model does generative work, not routing.
- **A prompt is a bounded linear pipeline, not a general program.** Parameters in, a string out, side effects possible; forks/cycles/fanouts allowed. When a program outgrows one file, split it into multiple prompt files called as functions - don't nest.
- **Single scoped section is the default**; `task`/`fanout` are opt-in for loosely-coupled, read-heavy work ("read in parallel, write in sequence"). Strongest model at the reduce.
- **5-10 tools per section**; scoping is load-bearing for mid-size models.
- **Prompts are deployment-agnostic** - tool bindings, model slots, and capability backends resolve in host config.

## To explore later

**Prompted task (agent-as-tool)** - a model-callable delegate tool: the model spawns a sub-agent mid-turn in a fresh context and gets a distilled result back into the step's tool loop, opt-in per step via `tools.add`. Complementary to Lua `task` (deterministic between-step orchestration) but the delegation shape becomes model-decided and invisible in the Lua graph. A large separate design (result contract, budgets, visibility) - deferred.

Also: async tasks; intra-process prompt-to-prompt `task("other.md")`; model-surface `goto` with curated `result` (in tension with readability); MCP client/server; gateway admission (retry-with-backoff on 503) and endpoint pinning; transient upstream 400 retry (in `GatewayClient`).

Deferred `rust-how-to.md` conformance (from the repo audit, mechanical fixes already landed in the sweep): split the crate-wide `promptforge-core::Error` into per-fallibility error types so a signature exposes only the variants it can produce (A1); replace deprecated `serde_yaml` with a maintained fork, re-testing frontmatter parse/error behavior (A2); consider splitting the ~1548-line `promptforge-webfetch/src/lib.rs` into modules (A3).

## Key files

- [design-promptforge.md](promptforge-design/design/design-promptforge.md) - prompt language (provisional)
- [design.md](promptforge-design/design/design.md) - system architecture (provisional)
- [example-diligence.md](promptforge-design/example-diligence.md) - idiom-complete teaching example
- research: `cabinet/_research/2026-07-29-fanout-*` - fan-out / task / budgets prior-art (two rounds)
- [how-to-write-prompts.md](tools-public/how-to/how-to-write-prompts.md) - prompt engineering rules

## Design document (generated last)

After **all** implementation is complete, the plan's final step generates the design document - so it reflects what was actually built, not this pre-build sketch. Spawn one subagent whose entire prompt is: *read this plan at its path, grep for `<design-doc>`, and follow the block inside it.* The agent never spawns this generator during design; running the plan does, once the build is done. The generated `design-promptforge-orchestrator.md` **supersedes** the provisional docs in `promptforge-design/design/`; on generation, the superseded files move to `cabinet/_trash/` per workspace rules.

<design-doc>
OUTPUT A DESIGN DOCUMENT, NOT CODE. Write one markdown file, design-promptforge-orchestrator.md,
that explains the design of what this plan describes. You run as the final step
of the plan, after the implementation is complete, so describe the design as
built, reconciling against the finished work any decision the implementation
changed from what this plan first recorded.

NO IMPLEMENTATION CODE - no function bodies, no private machinery, no
step-by-step algorithm walkthroughs. You MAY include any normative artifact the
design needs to remove ambiguity: public signatures, schemas, state or
transition tables, wire formats, configuration syntax, sequence diagrams, and
pseudocode. Each such artifact must express a design contract, not an
implementation technique; include one only where prose cannot say the same
thing as precisely, and show the artifact alone, not the surrounding machinery.

FOR EVERY DESIGN ELEMENT, STATE THREE THINGS: what is observed (by the user or
by an external consumer), how it is structured, and WHY - the motivation, the
rationale, the principle. For a costly-to-reverse element, "why" must include
what reversing it later would cost.

DESIGN-ELEMENT TEST - include something only if changing it would change ANY of:
  (a) ANYTHING THE USER SEES, READS, WRITES, TYPES, OR NAMES. For a library the
      user is the caller, so this is the PUBLIC API - its operations and their
      contracts (ownership, lifetime, thread-safety, error and complexity
      guarantees). It also includes every config file or frontmatter the user
      edits, and - critically - the NAMES of everything the user sees. A name
      is a design decision: `goto` is a good one, `clear_and_transfer_control`
      is a bad one. Naming is design.
  (b) the shape or structure of the system.
  (c) something costly or hard to reverse that the user never sees - the ABI,
      an on-disk or persisted format that outlives a version, a high-reach
      convention that touches everything, or a cross-cutting quality trade-off
      (security, failure modes, data lifecycle, performance).
If it is none of these - merely how you implement the design behind those
surfaces, such as a private helper type, an internal algorithm choice, a
dependency version pin, or a serialization used only between your own
components - it is implementation. Leave it out.

A public interface is design; a private type is implementation - the same
struct is on opposite sides of the line depending on whether the user sees it.
Describe an interface's shape and contract in prose by default; show the actual
artifact - a signature, a schema, a state table - wherever that artifact is
itself the load-bearing decision and prose would blur it. No fixed budget binds
these; each earns its place only by being load-bearing.

COMPRESS BEFORE WRITING - only if the design carries far more ditchable detail
than load-bearing decisions (roughly 10 to 1 or worse). If it is already lean,
skip this. Run the pass in order, cheapest cut first, and stop once the ratio
is healthy:
  1. Drop a default only when changing it would change no observable behavior
     and carry no meaningful risk. A consequential default - a timeout,
     ownership, a security posture, a retry policy, a resource limit, a
     compatibility choice, a failure mode - resolved a real fork and stays.
  2. Move anything decidable later at little or no extra cost to a "decide by
     use" list, or drop it. A cheaply-deferrable element is not a headline one.
  3. Replace an enumeration with the rule that generates it.
  4. Merge consequences into the decision that forces them, and sibling
     elements into their shared pattern.
  5. Name a known pattern instead of re-deriving it.
  6. Rank what remains and keep about 10 to 15 headline elements; demote the
     rest to one line.
  7. Delete anything whose removal would still let a competent builder build
     the right thing.

STRUCTURE - three fixed sections, then whatever the design earns:
  - A title stating what building this produces.
  - An executive summary that stands alone; a reader acts on it without the body.
  - A numbered list of the 10 to 15 key design choices, each a short paragraph.
Then, for a reader who stops early:
  - Write headings that state the point, not the topic ("Labels compute at
    boot, off the critical path", not "Labels").
  - Keep rationale in prose; do not bulletize an argument. Enumerate only
    parallel items (decisions, constraints, options).
  - State the evidence before the value word: never "fast" before the number.
  - Where a choice resolved a real fork, name the alternative and why it lost.
  - Order by importance; put a dependency first only where the reader needs it
    to follow what comes next, so cutting from the bottom never removes the core.
  - Add no YAML frontmatter. Close with one italic line naming the date and the
    model. Name no tool, rulebook, or source document for the document's own
    rules or structure.

CHECK BEFORE FINISHING, and fix any no: no implementation code, and every
normative artifact expresses a contract rather than a technique; every element
states what, how, and why; headings state points; no argument is bulletized;
the compression ratio is healthy; no source document is named. If the plan
carries no key design choices, write no document and return the reason.
</design-doc>
