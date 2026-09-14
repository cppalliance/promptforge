---
name: RunContext seed, per-run nonce, H1 control-global errors
overview: Introduce RunContext (starting with Arc<Prompt> plus a per-run GuardNonce) as the first parameter of execute-subtree functions; make the untrusted guard nonce per-run for KV-cache prefix stability; install clear-error stubs for control globals called from H1, with tests pinning all of it.
todos:
  - id: context-module
    content: "Create execute/context.rs with RunContext { prompt: Arc<Prompt> } (prompt ONLY - nonce lands in phase 2 with its readers), constructor, accessors, unit tests"
    status: pending
  - id: wire-module
    content: Register mod context in execute.rs and re-export RunContext; update module-layout doc comment
    status: pending
  - id: thread-run
    content: "Construct RunContext in run (pass &ctx into run_body - prompt.title used after); replace prompt: &Prompt with ctx in execute_live_h1 and run_sections; rename RunFrame ctx bindings to frame in engine.rs"
    status: pending
  - id: nonce-signature
    content: "untrusted.rs: GuardNonce pub(crate) + Clone, fresh() pub(crate) (mlua capture needs owned clone); change wrap to wrap(&GuardNonce, &str); update module docs. NOTE: atomic with the two threading todos - the wrap signature change breaks both callers at once"
    status: pending
  - id: nonce-thread-tool-loop
    content: "Thread the nonce RunContext -> RunFrame -> from_run_fields (covers from_walk AND from_fanout) -> ControlContext -> walk_context + FanoutContext -> SectionProgress -> tool_loop wrap site; fix 6 test constructors of SectionProgress"
    status: pending
  - id: nonce-thread-lua
    content: "Pass &GuardNonce into SectionVm::new (new_for_section delegates) so install_untrusted captures an owned clone; fix all construction sites: h1.rs:55, engine.rs:345, arm.rs:214, vm.rs:1037 test helper, execute/tests 897/969, model/tests:98, 25 lua/tests.rs sites via a new test helper"
    status: pending
  - id: nonce-tests
    content: "Invert the three freshness tests to stability tests; add cross-run distinctness test"
    status: pending
  - id: h1-stubs
    content: "Install clear-error stubs for execute/jump/fanout/list_from_section in the H1 VM setup (one commit with h1-stub-tests; independent of phases 1-2 but shares h1.rs/vm.rs - land after them to avoid churn)"
    status: pending
  - id: h1-stub-tests
    content: "Tests: each control global called from H1 fails with the clear message"
    status: pending
  - id: verify
    content: "cargo build / test / clippy green after EACH commit (vibe-rulebook: scheduled verify gates the next step); full suite at the end"
    status: pending
isProject: false
---

# RunContext Seed + Per-Run Nonce + H1 Control-Global Errors

## Goal

Three coordinated changes to `promptforge-core`:

1. **RunContext seed**: a single ambient-state object for the execute subtree, passed as parameter one (`ctx: &RunContext`). Starts with one field, `Arc<Prompt>`; the `nonce` field arrives in Phase 2 together with its readers (a never-read field trips dead_code in non-test builds).
2. **Per-run guard nonce**: `untrusted::wrap` uses one nonce minted at run start instead of a fresh nonce per call, so identical untrusted content produces byte-identical envelopes - preserving KV-cache prefix sharing across fanout arms and tool-loop rounds.
3. **H1 control-global stubs**: calling `execute`/`jump`/`fanout`/`list_from_section` from H1 fails with a clear message instead of Lua's stock nil-call error.

## Design decisions (settled in discussion)

- Named `RunContext` after `execute::run`, matching the existing `RunConfig`/`RunLimits`/`RunError` vocabulary; no collision with the public `ResolutionContext`.
- `Arc<Prompt>`, not `&Prompt`: spawn boundaries (fanout arms) require `'static`; `Arc` gives shared immutable access with runtime liveness. `Prompt: Clone` already holds, so `run` wraps internally and the public API (`run(prompt: &Prompt, ...)`) is unchanged.
- Context is immutable within a scope; scope changes fork-with-deltas (struct-update syntax). This pass has exactly one fork: `run_sections` already rebuilds the frame post-H1.
- Convention: context is parameter one, named `ctx`. Per-call data (`prose`, `conversation`, `item`) stays as parameters - only run/scope state lives in the context.
- The struct is `pub(crate)` - purely internal, no public API surface change.
- Per-run nonce rationale: the `<`-escaping is the load-bearing defense (no content-supplied tag survives regardless of nonce knowledge); the nonce is defense in depth, and per-call freshness breaks prefix caching at the envelope preface. Per-run keeps envelopes deterministic within a run while remaining unguessable across runs.
- Tool/model scope end-state (later pass, decided). Taxonomy: `ToolSet` is the CONTAINER struct (today's `ToolBindings` - the two Vecs that declarations insert into); `ToolView` is the read-only TRAIT. No wrapper type: `impl ToolView for Mutex<ToolSet>` directly (legal - local trait on foreign type), so `RunContext.tools: Arc<dyn ToolView>` IS an `Arc<Mutex<ToolSet>>`. Same for models: `ModelSet` / `ModelView`, `impl ModelView for Mutex<ModelSet>`. `RunContext` holds `tools: Arc<dyn ToolView>` and `models: Arc<dyn ModelView>` - same pattern as the existing `observer: Arc<dyn Observer>`. ONE implementation, created empty at run start and shared for the whole run - no FrozenToolSet, no swap at the walk fork. During H1 the Lua host bindings write through their own concrete `Arc<Mutex<ToolSet>>` handle; reads through the view lock briefly and return owned snapshots (bindings-so-far, always truthful). Frozenness after H1 is structural: the only write handle is dropped with the H1 VM, and the trait has no write methods, so no context holder can ever write. Constraint: trait methods return owned snapshots, not references (a mutex guard cannot outlive the call); matches the existing clone-per-prose-block scope rebuild. Accepted cost: H2 reads take an uncontended mutex lock (nanoseconds). No `Option`, no dummy, no phase enum. Rejected trait names: `ToolBindings`/`ToolBinds` (plural nouns name data, not capabilities; would collide with the container and the `tools.bind` DSL verb).
- `ToolBinding` carries `Arc<dyn Tool>` (later pass, decided): `bind()` is H1-only and is the only consumer of the full implementation catalog, so once bindings exist, the catalog is dead weight. Verified: every post-H1 registry use (description fallback and parameters schema in `prepare_scoped_tools`, dispatch in the tool loop) comes straight from the binding's Arc instead. Consequences: the implementation catalog becomes H1-phase input, not run state - no `RunContext` field, and it leaves `RunFrame`/`ControlContext`/`FanoutContext`/`ArmInputs`; `Error::UnknownScopedTool` is eliminated (unavailable implementations fail at bind time, at the H1 line that caused them); the dispatch map becomes alias -> binding. Costs: hand-implemented `PartialEq`/`Eq`/`Debug` on `ToolBinding` keyed on `id`; test fixtures need dummy `Arc<dyn Tool>`s.
- `SharedTools` is renamed `ToolCatalog` and becomes caller-provided (later pass, decided): the harness builds and validates the catalog once (`ToolCatalog::new`) and passes it in, mirroring how `ModelCatalog` already works - same concept, same API shape. Benefits: validation errors surface at the caller's construction site; one catalog is shared across many runs with zero re-cloning; the slice-to-SharedTools-to-ToolRegistry chain leaves the public mental model. `ToolRegistry` folds into `ToolCatalog` (its post-H1 dispatch role is eliminated by the `Arc<dyn Tool>` binding; only H1-phase validation/lookup remains). Costs: `ToolCatalog` and its error type become public API (real docs required); `run`'s signature changes (acceptable at 0.1). Open detail: fold the tool catalog into `ResolutionContext` (`ResolutionContext::new(&picker, &models, &tools)`) so all three H1 resolution inputs travel together - elegant but touches a second public type's signature; decide at implementation time.
- Model side (later pass, verified): the model world is already at the end-state in two respects. `ModelBinding` is self-sufficient data (alias, id, frozen `ModelInvocation`, dialect, context window - `ModelBinding::new` is atomic, MODEL-006); there is no `dyn Model` executable, invocation is an HTTP call through the shared `GatewayClient`. And `ModelCatalog` is already H1-phase-scoped: production references exist only in `h1.rs`, `resolve.rs`, and `transport.rs` (host-side fetch). So `RunContext` never holds the catalog. Asymmetries the migration must respect (do NOT force-fit the tool pattern): models are single-selection per section (`models.use` selects at most one; `models.default` is the prompt-wide fallback); model resolution is one-time at a section's first prose block, not rebuilt per block; there is no near-duplicate analysis for models (picker Duplicate/Ambiguous fail at bind time); `ModelBinding` has no `Eq` by design (f64 temperature, NaN reflexivity).
- Settled prompt vocabulary (decided): `bind` creates a binding (H1), `always`/`default` set the prompt-wide baseline (H1), `add`/`use` scope a section (H2). `tools.add` stays (accumulation, multi-tool) and `models.use` stays (single-selection) - the verbs encode the cardinality difference; normalizing to one verb would hide it. Rejected: `tools.use`, `models.bind_default` (default selects an existing binding, it does not bind), `models.use_default` (names the setter as if it were the consumer; sections consume the default by omitting `models.use`). If the single-model case dominates, consider a separate convenience op `models.bind_default` that binds AND marks default atomically - measure real prompt files first.
- Near-duplicate conflict detection moves to bind time and onto the binding (later pass, decided): on each `tools.bind`, the new description is compared against every existing binding's description via the picker (live during H1); each clash is recorded SYMMETRICALLY on both bindings as `conflicts: Vec<Conflict>` where `Conflict { alias: String, similarity: f64 }` (the score is kept because the picker is H1-phase - the diagnostic cannot be recomputed later). Rule: bind RECORDS, never fails - a conflict only errors when both halves enter one advertised scope. The scope check becomes purely local: does the entering binding's conflicts list intersect the scope's aliases. `ToolAnalysis` as a type is DELETED; `alias_to_id`/`id_to_alias` are pure reindexing of the bindings list, derived on demand. Consequences: no handoff computation step, no OnceLock, and RunContext is constructed once and NEVER forked at the H1-to-H2 boundary (bindings/models ride the views, `when` is set at run start, analysis no longer exists as a delta). The only remaining forks are fanout scope changes (proxy observer/debug, fresh turn counter). Open detail: whether the conflict check fires at scope rebuild (current behavior) or at `tools.add` entry time (fails closer to the cause) - decide at implementation; verify always-scope and local-tool paths either way. Implementation check: picker build cost per bind vs one batch build (fine at prompt scale either way, but look at what ToolPicker::build/rebuild does).
- Rename `tools.need` -> `tools.bind` and `models.need` -> `models.bind` (later pass, decided): completes the vocabulary family - `bind` creates a `ToolBinding`, failure is already `RunErrorKind::Binding`. Breaking prompt-language change; mechanical search-and-replace across prompt files, the user guide, and tests. Do it as its own pass, not folded into other work.

## Target end-state: the two-type architecture (after all recorded passes)

The engine converges on exactly two context types, replacing all five of today's (`RunFrame`, `ControlContext`, `FanoutContext`, `ArmInputs`, `SectionProgress`):

**`RunContext`** - the run. Ambient, immutable, constructed once in `run`, shared by reference or `Arc` everywhere. NEVER cloned, NEVER forked, by any code path: not at the H1-to-H2 boundary (views ride across, `when` set at start, conflicts live on bindings), not in `execute()`/`jump` chains (synchronous detours; only call data changes), not in fanout (see below).

**`SectionContext`** - one section entry within the run (a walked section, the live H1 pass, a fanout arm's worker). Owned, per-frame, mutable. The frame OBJECT: behavior lives here. One section entry = one SectionContext, regardless of arrival mode (fall-through, jump, execute); a jump ENDS the current frame (`SectionFlow::Jumped`) and the driver builds a fresh one for the target - only `reply` and `var` cross, as call data. H1 is simply a section - the level-1 one; it runs first and is never re-entered.

```rust
pub(crate) struct SectionContext {
    vm: SectionVm,            // owned; the frame's engine. SectionVm stays a
                              // standalone type in lua/ with its own test suite -
                              // composition, not merger
    sys: serde_json::Value,
    var: serde_json::Value,   // seeded in, read back out (rolls forward across walks)
    item: Option<serde_json::Value>,
    reply: Option<String>,    // rolls forward across walks
    conversation: Vec<Message>,
    counts: Option<ToolCallCounts>,
    completion_options: Option<CompletionOptions>,
    write_scope: Option<WriteScope>,
    execute_depth: usize,
    // Effective reporting handles: normally cloned straight out of the
    // RunContext; the fanout driver builds the proxy observer/debug and a
    // fresh turn counter ONCE PER FANOUT and hands clones into each arm's
    // SectionContext. This is why fanout never forks the RunContext.
    observer: Arc<dyn Observer>,
    debug: Option<Arc<dyn DebugCapture>>,
    turns: Arc<AtomicU32>,
}
```

Lifecycle as methods: `SectionContext::new(ctx, section, seed)` absorbs the whole setup sequence (VM construction, limits, host injection, control globals, shared replay, captured bindings - what `setup_section_vm` plus driver preambles do today); `frame.run(ctx)` is the block walk (`run_one_section_impl`'s parameter list becomes `self` + `ctx`); `frame.teardown(ctx)` is the explicit single teardown boundary. Teardown stays a METHOD, never `Drop` - the fanout arm's VM must outlive its cancel-scoped body so the epilogue can finalize first. All three drivers (H1, walk section, fanout arm) become construct-run-teardown of one frame type, differing only in seed and `BlockRunMode`.

Consequences: `ControlContext` is fully dead (its only reason to exist was being the thing fanout clones with deltas); the bounded side channels and drain loop in `run_fanout_arms` survive unchanged (back-pressure isolation is real), only the delivery route changes (proxy arrives via the frame, not a forged context); the `#[expect(clippy::too_many_arguments)]` annotations disappear as signatures converge on `(ctx: &RunContext, frame: &mut SectionContext, ...)`.

Naming: `SectionContext` - the frame is born and dies per section entry, and the scope ladder is Run > Section. Rejected: `WalkContext` (a walk is the traversal of a sibling slice - `walk_siblings` spans MANY sections; the name misplaces the frame a level up), `StackState` (over-promises push/pop semantics; fall-through is flat iteration, arms are siblings), `FrameState`.

## Target end-state RunContext (after all recorded passes)

```rust
#[derive(Clone)]
pub(crate) struct RunContext {
    // The document and run identity - never change
    prompt: Arc<Prompt>,
    execution: Arc<str>,          // execution id stamped on observations
    args: Arc<str>,               // the run's argument string
    when: Arc<str>,               // run timestamp, set once at start

    // Capabilities - read-only views over the shared containers;
    // writes happen only through H1's concrete Arc<Mutex<...>> handles
    tools: Arc<dyn ToolView>,
    models: Arc<dyn ModelView>,

    // Infrastructure
    store: StoreRef,              // already a shared handle
    limits: RunLimits,            // Copy struct of ceilings
    observer: Arc<dyn Observer>,
    debug: Option<Arc<dyn DebugCapture>>,
    cancel: Option<CancelHandle>,

    // The untrusted-envelope nonce, minted once per run
    nonce: GuardNonce,

    // Counters - the only mutable-in-place fields, via atomics
    turns: Arc<AtomicU32>,
    ids: Arc<AtomicU64>,
}
```

Deliberately absent:

- `shared_tools` - H1-phase input only; `ToolBinding` carries `Arc<dyn Tool>`, so nothing post-H1 needs the catalog of implementations.
- `max_tool_iterations`, `section_count`, `shared` - derivable from `prompt` + `limits`; become methods, not fields.
- `item` - per-arm call data, stays a parameter.
- `analysis` - deleted entirely; conflicts live on the bindings (see Design decisions), alias maps are derived on demand.
- The model catalog - already H1-phase-scoped today.

Fork deltas: NONE at the H1-to-H2 boundary - the context is constructed once and shared unchanged. The only forks are genuine scope changes: `observer` and `debug` (proxied per fanout), `turns` (fresh counter per fanout).

## Changes

### Phase 1: RunContext seed

One commit: `context-module` + `wire-module` + `thread-run` land together (separately they produce a never-constructed/never-read dead_code hazard in non-test builds).

New module `execute/context.rs`:

- `#[derive(Clone, Debug)] pub(crate) struct RunContext { prompt: Arc<Prompt> }` - prompt ONLY; the `nonce` field arrives in Phase 2 with its readers. (rust-rulebook: Debug on plain data; `Prompt: Debug` already holds.)
- `#[must_use] RunContext::new(prompt: &Prompt) -> Self` performs `Arc::new(prompt.clone())` (rust-rulebook: must_use on constructors; house style already does this).
- Accessor: `prompt(&self) -> &Prompt` - no `get_` prefix.
- Module doc (`//!` first line naming the job) stating the invariant: this is the execute subtree's ambient run state; new run-scoped concerns become fields here, not new parameters.

Wire-up:

- `mod context;` + `pub(crate) use context::RunContext;` in [execute.rs](promptforge/crates/promptforge-core/src/execute.rs); add `context` to the module-layout doc comment (lines 45-56).
- In `run` ([execute.rs:207](promptforge/crates/promptforge-core/src/execute.rs)): `let ctx = RunContext::new(prompt);` after the version gate (after line 224). SCAN NOTE: `prompt` is used again AFTER `run_body` completes (`prompt.title` at execute.rs:321), and throughout the body (`prompt.replay` :263, `prompt.frontmatter` :286-289, `prompt.sections` :291/:298) - pass `&ctx` into `run_body`; never move it.
- `execute_live_h1` ([h1.rs:28](promptforge/crates/promptforge-core/src/execute/h1.rs)): replace `prompt: &Prompt` with `ctx: &RunContext` as parameter one. SCAN NOTE: the body uses `prompt.title` 8+ times (:51,:53,:55,:62,:74,:82,:96,:111,:117), `prompt.sections.len()` (:53), `prompt.h1_blocks` (:97) - all become `ctx.prompt()`-based. Its RunFrame destructure at h1.rs:35-44 uses `..`, so the Phase-2 frame field is safe there.
- `run_sections` ([engine.rs:152](promptforge/crates/promptforge-core/src/execute/engine.rs)): same replacement; the walk's `RunFrame` picks up `nonce: ctx.nonce()` (see phase 2). SCAN NOTE - NAMING COLLISION: the rebuilt frame is bound as `let ctx` at engine.rs:164, and `walk_siblings` (:228) / `run_one_section` (:329) already take `ctx: &RunFrame`. Convention is `ctx` = `&RunContext`; rename the RunFrame bindings/params to `frame` in this file.
- Update the two call sites in `run` (lines 294, 302).

### Phase 2: per-run nonce

ONE ATOMIC COMMIT: `nonce-signature` + `nonce-thread-tool-loop` + `nonce-thread-lua` + `nonce-tests` land together - the `wrap` signature change breaks both call sites (tool_loop.rs:285, host.rs:99) simultaneously, so no sub-slice compiles alone.

`untrusted.rs` (lands first within the commit):

- `GuardNonce` becomes `pub(crate)` with `Clone` AND `Debug` derived (Debug is safe: the nonce is a hex string that appears in model-visible envelope text; rust-rulebook wants Debug on plain data); `fresh()` becomes `pub(crate)`. `Clone` is mandatory, not optional: mlua's `create_function` requires `Fn + Send + 'static`, so the Lua closure must capture an owned clone (verified by scan).
- `wrap(content: &str)` becomes `wrap(nonce: &GuardNonce, content: &str)`; `preface` already takes the nonce.
- Module docs updated: one nonce per run; escaping is the primary defense; determinism within a run is a feature (cache prefixes, snapshot tests).

`RunContext` gains its second field: `nonce: GuardNonce` - `new()` mints via `GuardNonce::fresh()`, accessor `nonce(&self) -> &GuardNonce`. The field and its readers land in the same commit, so no dead_code hazard.

Threading to the tool-loop wrap site ([tool_loop.rs:285](promptforge/crates/promptforge-core/src/execute/tool_loop.rs)):

- `RunFrame` gains `nonce: &'a GuardNonce` ([engine.rs:103](promptforge/crates/promptforge-core/src/execute/engine.rs)); built in `run` from the RunContext, carried over by the `run_sections` fork (`..*frame`). RunFrame is `#[derive(Clone, Copy)]` (engine.rs:102) - a `&'a GuardNonce` field keeps Copy.
- `ControlContext` gains `nonce: GuardNonce` ([engine.rs:546](promptforge/crates/promptforge-core/src/execute/engine.rs)). SCAN NOTE: add the field + one parameter to `from_run_fields` (engine.rs:579, literal at 597-614) - both `from_walk` (:619) and `from_fanout` (:645) delegate to it, so one parameter covers both paths. `walk_context` (:676, literal 677-695) re-borrows `nonce: &self.nonce`.
- SCAN NOTE - FanoutContext is the missing link: the arm's nonce chain is ControlContext -> `FanoutContext` (fanout/mod.rs:213, built by the `fanout_context` builder literal at engine.rs:716-737) -> `ArmInputs::from_context` -> `ControlContext::from_fanout`. `FanoutContext` gains `nonce: &'a GuardNonce`; the builder literal sets `nonce: &self.nonce`. ArmInputs needs NO change (it holds `Arc<ControlContext>`; the nonce rides inside).
- `SectionProgress` gains `nonce: &'a GuardNonce` ([tool_loop.rs:34](promptforge/crates/promptforge-core/src/execute/tool_loop.rs)); the wrap site calls `untrusted::wrap(progress.nonce, output.text())`. `run_prose_inference` destructures SectionProgress at tool_loop.rs:101-108 - bind `nonce` there; no signature change.
- SCAN NOTE - six TEST constructors of SectionProgress need the field: `silent_progress` helper (execute/tests/mod.rs:795-807) plus execute/tests/tool_loop.rs:156,227, tests/mod.rs:1194, debug_and_counts.rs:278, observations.rs:357. Production constructors are block_walk.rs:239 and :340 (both build from the walk's ctx - one frame field serves both).
- Rewrite the stale comment at tool_loop.rs:279-283 (it argues for per-call freshness on a threat the escaping already prevents).

Threading to the Lua wrap site ([host.rs:97-99](promptforge/crates/promptforge-core/src/lua/host.rs)):

- `SectionVm::new` and `SectionVm::new_for_section` ([vm.rs:212, 263](promptforge/crates/promptforge-core/src/lua/vm.rs)) gain a `nonce: &GuardNonce` parameter; `install_untrusted` (called only at vm.rs:245; `new_for_section` delegates to `new` at vm.rs:270) captures an OWNED clone (mlua `Fn + Send + 'static` bound - verified by scan).
- SCAN NOTE - complete construction-site list: h1.rs:55 (from RunContext), engine.rs:345 (walk, from RunFrame), arm.rs:214-220 (from ControlContext via the ArmInputs Arc), vm.rs:1037 (`#[cfg(test)] run_chunk` helper - the single mint point for lua/tests.rs `run()`-based tests), execute/tests/mod.rs:897 and :969, model/tests/mod.rs:98, and 25 inline sites in lua/tests.rs (21 `new` + 4 `new_for_section`, no shared helper - add a small test helper that mints one nonce and use it at every site).

Tests (the inversions):

- `untrusted.rs:180` `each_wrap_owns_a_fresh_unique_nonce` becomes: one nonce, many wraps, identical tags; escaping/property tests unchanged (nonce-agnostic).
- `execute/tests/mod.rs:1427` `untrusted_nonce_is_fresh_per_round` becomes `untrusted_nonce_is_stable_across_rounds`: every round's envelope in one run carries the same nonce.
- New: cross-run distinctness - two `run` calls produce different nonces (execute tests).
- `lua/tests.rs:2917` `untrusted_global_mints_a_fresh_nonce_per_call` becomes same-nonce-per-call within a VM.

### Phase 3: H1 control-global stubs

- Add a `SectionVm` method (e.g. `install_h1_control_stubs`) in the lua layer that installs `execute`, `jump`, `fanout`, `list_from_section` as functions raising a clear Lua error: `"<name> is only available in sections (## headings); H1 runs before sections exist"`.
- Call it in `execute_live_h1` after `install_host_apis` ([h1.rs:74](promptforge/crates/promptforge-core/src/execute/h1.rs)).
- The defensive recorded-jump path in `run_live_h1_block` stays (belt and suspenders).
- Tests (execute/tests): one per global - an H1 block calling it fails the run with the clear message in the error text.

### Docs (folded into the Phase 2 commit - no separate docs commit; rulebook: no make-work commits)

- `untrusted.rs` module header (phase 2 wording) PLUS the two fn docs found stale by scan: `GuardNonce` doc at :17 ("single-use" - becomes per-run) and `wrap` doc at :55 ("freshly minted nonce"). `fresh()` doc at :26 stays true (fresh per mint).
- User guide line 330 (verbatim today: "Each round uses a fresh nonce") becomes "One nonce per run; envelopes are deterministic within a run". No other freshness claims exist in the guide (:366 and :453 are freshness-neutral, verified).
- `store.rs:11-12` mention of the guard envelope: no freshness claim present - NO change needed (scan verified).

### Phase 3 notes from scan

- Stub install slot: after `install_host_apis` (h1.rs:74), no interference with `attach_infer_hook` (h1.rs:76-85) - disjoint globals.
- Confirmed motivation: `harden` (hardening.rs:14-33) installs no `_G` metatable, so a control-global call from H1 today is stock "attempt to call a nil value".
- `install_control_globals` is at vm.rs:493 (its doc comment starts at 478); production caller is section_vm.rs:131 only.
- Test-side note: the wiring comment at execute/tests/mod.rs:395 is orphaned above the EchoTool fixture; the actual wiring tests are at :1363 and :1473.

## Run 2: SectionContext consolidation (step decomposition)

LANDED 2026-08-22: step 4 = bc813d0 (SectionContext shell + walk driver), step 5 = cc71511 (H1 on SectionContext), step 6 = 6f2ea29 (fanout arms on SectionContext), step 7 = 5a54f24 (delete RunFrame/ControlContext/FanoutContext). Full workspace suite green at the end. Two recorded departures: run_one_section_impl stayed a free fn taking split borrows (stated in bc813d0's message), and fanout proxies are delivered by a per-fanout RunContext fork via with_effective_handles rather than frame-side plumbing (judged justified in review; stated in 5a54f24's message). RunFrame.item died in step 6 (arm was its only reader).

Behavior-neutral refactor; the existing suite is the oracle at every step. Each step is one commit, compile-green on its own. Steps 1-3 of run 1 landed (00ab6eb, 90dec97, f47f6cc): RunContext exists with prompt + nonce; SectionVm::new takes the nonce.

4. **SectionContext shell + the walk's driver.** New `execute/section_context.rs`: the struct from "Target end-state" above (vm, sys, var, item, reply, conversation, counts, completion_options, write_scope, execute_depth, observer, debug, turns). `SectionContext::new(ctx, ...)` absorbs `setup_section_vm` plus the walk's driver preamble (VM construction, limits, sys build, seed); `frame.run(ctx)` wraps `run_one_section_impl` (which becomes a method or a free fn taking `&mut SectionContext`); `frame.teardown(ctx)` wraps the VM teardown. Convert ONLY the walk's `run_one_section` (engine.rs) to construct-run-teardown. `SectionProgress` dissolves into the frame's fields (observer/debug/turns/completion_options) - `run_prose_inference` takes them from the frame. Behavior-neutral: same suite green.
5. **H1 on SectionContext.** `execute_live_h1` constructs a SectionContext in `BlockRunMode::LiveH1` (the mode stays a parameter/field); the LiveH1State extraction (bindings/models/var/reply/returned) reads out of the frame at the end. Suite green.
6. **Fanout arms on SectionContext.** `run_one_arm` becomes construct-run-teardown of a frame seeded with item/write_scope and the PROXY observer/debug + fresh turns (the effective-handles move: proxies arrive via the frame, not a forged context). `ArmInputs` shrinks to what the spawn boundary truly needs. The bounded side channels and drain loop in run_fanout_arms are untouched. Suite green.
7. **Delete the old contexts.** `RunFrame`, `ControlContext`, `FanoutContext`, `ArmInputs` remnants, and any dead re-exports are deleted; `make_control_globals` and `drive_contained_chain` capture `RunContext` (cheap Clone) plus the section slices directly. The `#[expect(clippy::too_many_arguments)]` annotations that lose their reason are removed. Suite green; this is the step where the line count drops.

Watch items (from the scans): `run_sections`' rebuilt-frame fork disappears when `when`/`bindings`/`models`/`analysis` stop being frame fields - decide per field where it lives during step 4 (the frame fork at engine.rs:164 is the walk's frame construction; under SectionContext it becomes the frame's seed). The `run` body's post-run `prompt.title` use (execute.rs:321) still constrains ctx to a shared borrow. Fanout test sentinels (PANIC_ARM_SENTINEL, FAIL_ARM_VM_SENTINEL) must keep working through the arm conversion.

## Run 3: toolset migration + catalog + rename (step decomposition)

LANDED 2026-08-22: step 8 = ef86da3 (need->bind rename, 52 files), step 9 = 836180c (ToolBinding carries Arc<dyn Tool>; DispatchTarget departure noted in message), step 10 = b0b7195 (ToolCatalog public + caller-provided via ResolutionContext; ToolRegistry folded in and deleted), step 11 = 375c7fb (ToolSet/ToolView on Mutex<ToolSet>, bind-time symmetric conflicts, ToolAnalysis deleted, dead ToolRegistryValidation observations removed), step 12 = ad7a830 (ModelSet/ModelView mirror). Final verify: full workspace suite green (1634 tests, 19 suites). Implementation choices recorded in the step-11 coder return: persistent picker (near_duplicates is pairwise cosine over already-indexed vectors, no rebuild) and the conflict check stays at scope rebuild (covers always and add paths in one site).

Implements the remaining recorded decisions. Each step is one commit, compile-green, suite as oracle. Order chosen so new code lands on final names and fat bindings before the container holds them.

8. **Rename `tools.need` -> `tools.bind`, `models.need` -> `models.bind`.** Mechanical, breaking the prompt language on purpose: Lua host registration and validation, error messages ("was not declared by tools.need" etc.), every test fixture, every prompt file under promptforge/local/prompts (and any other in-tree consumer), the user guide. No behavior change. Suite green.
9. **`ToolBinding` carries `Arc<dyn Tool>`.** Resolution attaches the implementation at bind time (the registry/catalog is consulted then, H1-phase). `prepare_scoped_tools` reads description fallback and parameters schema from the binding's Arc; dispatch goes through the binding (dispatch map becomes alias -> binding); `Error::UnknownScopedTool` is deleted (unavailable implementations fail at bind time). Hand-implemented `PartialEq`/`Eq`/`Debug` on ToolBinding keyed on id; test fixtures get dummy Arc<dyn Tool>s. Suite green.
10. **`SharedTools` -> `ToolCatalog`, caller-provided.** The harness builds and validates the catalog once and passes it in (mirroring ModelCatalog); `ToolRegistry` folds into it (only H1-phase validation/lookup remains after step 9); `run`'s signature changes (acceptable at 0.1). Open detail to settle at implementation: whether the tool catalog folds into ResolutionContext. Public error type for catalog construction gets real docs. Suite green; in-tree callers of run() updated.
11. **`ToolSet`/`ToolView` + bind-time conflicts + delete `ToolAnalysis`.** `ToolSet` is the container (today's ToolBindings); `ToolView` is the read-only trait implemented directly on `Mutex<ToolSet>`; `RunContext.tools: Arc<dyn ToolView>` is created empty at run start (killing the with_walk_state fork for bindings). On each tools.bind, the new description is compared against existing bindings via the picker and clashes are recorded SYMMETRICALLY as `conflicts: Vec<Conflict>` with `Conflict { alias, similarity }` (bind records, never fails). The scope check reads the entering binding's conflicts (decide scope-rebuild vs add-time at implementation; verify always-scope and local-tool paths). `ToolAnalysis` deletes; alias maps derive on demand. Check ToolPicker::build/rebuild cost per bind first. Suite green.
12. **Model side: `ModelSet`/`ModelView`.** Same treatment for models: ModelSet container (today's ModelBindings), ModelView trait on Mutex<ModelSet>, `RunContext.models: Arc<dyn ModelView>` from construction. Respect the asymmetries (single-selection via models.use/default, one-time resolution at first prose, no conflict analysis, no Eq on ModelBinding). Suite green.

## Explicitly out of scope (later passes)

All originally listed items have LANDED (Runs 1-3). Nothing currently queued here. Candidate future work surfaced during the runs:

- The gateway-side dialect/normalization refactoring (deliberately separate; the LiteLLM research file preserves the thread: cabinet/_research/2026-08-22-explore-litellm-normalization.md)
- Reasoning/CoT normalization with dual-write preservation, and conversation repair in the echo path (from the LiteLLM findings)
- `models.bind_default` convenience op - only if the single-model case dominates real prompt files (measure first)

## Verification

Per rust-rulebook section 12, before EVERY commit (not just at the end):

- `cargo fmt --all --check`
- `cargo clippy --all-targets --all-features -- -D warnings` (the bare form skips tests/benches - where most of this plan's churn lands)
- `cargo test --locked -p promptforge-core`
- Full workspace suite at the end: `cargo test --locked --workspace --all-features`

House conventions for the executor (rust-rulebook, matching existing code): any new lint suppression is `#[expect(lint, reason = "...")]`, never bare `#[allow]`; comments state the invariant and the reason, never narrate the next line; new tests land in the same commit as the code they cover.

- New tests: nonce stability within run, nonce distinctness across runs, four H1 stub error tests

## Commit structure (vibe-rulebook level 4: each commit is the largest slice one set of tests covers)

1. **RunContext seed** (`context-module` + `wire-module` + `thread-run`): `RunContext { prompt: Arc<Prompt> }` only. Tests: context module unit tests + existing suite green. Compiles green because the struct is constructed and read in the same commit.
2. **Per-run nonce** (`nonce-visibility` content + `nonce-signature` + `nonce-thread-tool-loop` + `nonce-thread-lua` + `nonce-tests` + docs): atomic - the `wrap` signature change breaks both call sites at once, and the `nonce` field lands together with its readers. Tests: the three inverted stability tests + cross-run distinctness + existing suite.
3. **H1 control-global stubs** (`h1-stubs` + `h1-stub-tests`): independent of 1-2; lands after them only to avoid h1.rs/vm.rs churn overlap. Tests: the four clear-error tests.

Verify after each commit; full suite at the end.

## Pre-execution scan (2026-08-22, eight file scanners, vibe-rulebook defect read)

Every plan claim about existing code was verified against the files (all line numbers exact except `install_control_globals`: vm.rs:493, not 478). Findings are folded into the phases above as SCAN NOTEs. Headline items:

- Phase-ordering defect found and fixed: `GuardNonce`/`fresh()` visibility + `Clone` moved into a Phase-1 prerequisite (`nonce-visibility` todo) - Phase 1 could not compile without it. SECOND READ (review pass): superseded - Phase 1 now seeds `RunContext { prompt }` only, and the visibility change heads Phase 2, so neither phase has a dead-code or missing-visibility hazard.
- `ctx` naming collision: `run_sections` binds its rebuilt RunFrame as `ctx` (engine.rs:164) and `walk_siblings`/`run_one_section` take `ctx: &RunFrame`; convention is `ctx` = `&RunContext`, so RunFrame bindings rename to `frame`.
- `FanoutContext` was the missing link in the arm nonce chain; one parameter on `from_run_fields` covers both `from_walk` and `from_fanout`.
- Test surface fully enumerated: 25 inline SectionVm constructions in lua/tests.rs (new test helper), vm.rs:1037, execute/tests:897/969, model/tests:98, six SectionProgress test constructors.
- Two extra stale fn docs (untrusted.rs:17, :55) joined the docs todo; store.rs verified needing no change.
- Confirmed safe: h1.rs RunFrame destructure uses `..`; phase-3 stub slot is disjoint from `attach_infer_hook`; hardening installs no globals metatable (stock nil error today); `prompt.title` after run_body means ctx is passed by reference, never moved.
