---
name: untrusted global and VM reorder
overview: "Seven steps, one commit each: (1-5) add a global `untrusted(s)`, extend `store.read` with optional line bounds, add `store.read_numbered` with the same bounds, and remove `store.inject` and `store.read_lines`; (6) reorder section VM startup so the full host environment is installed before the shared library replays as the section's first chunk; (7) generate the design document."
todos:
  - id: step-1-untrusted-global
    content: "Step 1: untrusted() global - install site, tests, docs"
    status: completed
  - id: step-2-read-bounds
    content: "Step 2: store.read(path[, start[, end]]) - StoreRef::read_range, Lua overload, tests, docs"
    status: completed
  - id: step-3-read-numbered
    content: "Step 3: store.read_numbered(path[, start[, end]]) - move numbering logic, Lua binding, tests, docs"
    status: completed
  - id: step-4-remove-inject
    content: "Step 4: remove store.inject, migrate papergate and callers, live smoke"
    status: completed
  - id: step-5-remove-read-lines
    content: "Step 5: remove store.read_lines, migrate all callers, workspace Verify"
    status: completed
  - id: step-6-vm-reorder
    content: "Step 6: section VM reorder - replay as first chunk, delete scoped machinery, tests, docs, Verify"
    status: completed
  - id: step-7-design-doc
    content: "Step 7: generate design-promptforge-store-and-vm-rework.md via the design-doc block"
    status: completed
isProject: false
---

# PromptForge: store reads and section VM rework

## Target

PromptForge's Lua surface gains a global `untrusted(s)` that guard-wraps any string for model-facing injection. The store gains bounded reads - `store.read(path[, start[, end]])` verbatim and `store.read_numbered(path[, start[, end]])` with absolute line numbers - and loses the two fused methods `store.inject` and `store.read_lines`. Section VM startup becomes one linear sequence with the full host environment installed before the shared library replays as the section's first chunk, fixing in passing the bug that shared replay ran under default rather than configured limits.

## Decisions

1. **`untrusted(s)` is a global, not a store method.** `untrusted::wrap` is already a pure string-to-string function (`crates/promptforge-core/src/untrusted.rs:63`); the store was only one of its two callers (the tool loop wraps untrusted tool output directly at `execute/tool_loop.rs:331`). A store-bound inject forces a write-then-inject round trip for any string not already in a file. Rejected: keeping wrapping store-bound.
2. **`store.inject` is removed.** One way to wrap: `untrusted(store.read(p))`. Accepted as a prompt-facing break; the engine frontmatter stays `promptforge: 1` because everything else is additive.
3. **Line numbering stays in the store as `read_numbered`.** Numbering a range needs the range's origin, and the store holds that provenance at slice time. Rejected: a `numbered(text, start)` global - it forced either repeating the start literal at call sites or reading a whole file to slice 400 lines. A pure `numbered` global can still be added later if numbering non-store strings shows up in practice.
4. **Bounds are 1-based inclusive (start, end), clamping.** Matches the dissect schema (`start_line`/`end_line` in papergate's `add_section`) and citation usage ("lines 22-82"). Rejected: (start, count) - it forces `end - start + 1` arithmetic at every call site, the documented small-model failure mode; with numbered text, start and end are extractive copies while count is always computed.
5. **Section startup order: build VM, apply limits, inject host values, install host APIs, install control globals, replay shared, install captured bindings, run chunks.** One linear path replaces the two-phase construction. Fixes the latent bug that shared replay ran under default limits, because `apply_lua_limits` currently lands after `new_for_section` replays.
6. **Replay runs with the full environment.** Shared top-level code is the section's first chunk in every respect: `log`, `store`, `args`, `sys`, `var`, `reply`, `tools`, `models`, `untrusted`, `execute`, `fanout` all available. Footguns possible; tighten later with evidence - the gated-environment sketch under Out of scope is the tightening path. Rejected: gating now (roughly 130 lines and a flag protocol with no observed confusion to justify it yet); rejected: status quo (blocks `log`/`store`/`args` at load, the asymmetry that prompted this rework).
7. **`jump` during replay is a hard error.** Delivered by mapping the chunk path's `LuaBlockResult::Jump` outcome to "jump is not available during shared library load". Rejected: following the jump - load-time control transfer has no coherent meaning.
8. **Captured bindings install after replay.** Preserves today's collision semantics: a declared tool/model alias wins over a same-named shared global. Consequence: during replay the `tools`/`models` tables are fully functional, but the bare alias userdata globals (e.g. `echo`, `analyst`) do not exist yet.
9. **Absent shared library means an empty compiled chunk.** Replay is unconditional; no `Option` branch in section startup. As built (step 6): the empty chunk is substituted once at the `run_sections` root via `LuaProgram::empty()` and `shared` is a non-Option `&LuaProgram` through `WalkContext` and the fanout/execute paths. Consequence: `LUA_SHARED_LOAD_STARTED/SUCCEEDED` now fire for every section even without a shared library (empty chunk replay).
10. **Two passes, shipped separately.** The Lua API work (steps 1-5) is independent of the VM reorder (step 6): smaller diffs, separate evidence.

## Execution protocol

- Build loop per `tools-public/rulebooks/vibe-rulebook.md`: one testable commit per step carrying code, test, and docs; dispatch the coder subagent with the plan path and step number; review-and-fix applies the `<code-review>` block from that file, overwriting `vibe-review.md`; amend the step's commit if review dirtied the tree.
- Rust obligations per `tools-public/rulebooks/rust-rulebook.md`: rustdoc with `# Errors` and doctest `# Examples` on every new public `StoreRef` method; no `unwrap` outside tests; `cargo fmt --all --check` and `cargo clippy --all-targets --all-features -- -D warnings` green before every commit; tests live in the existing modules (`store/tests.rs`, `lua/tests.rs`, `execute/tests/`).
- House conventions (vibe rule 3): follow existing promptforge-core patterns - observer detail naming in `observe.rs`, the mlua binding pattern in `host.rs`, doc-comment style in `store.rs`.
- Decision currency: where a step's implementation contradicts, extends, or resolves a decision recorded here, the step revises this plan in the same commit, naming what forced the change.
- Verify runs on steps 3, 5, and 6 (every third step and end of each component), plus a live `promptforge papergate` smoke after steps 4 and 6 (gateway and MCP server per `promptforge/promptforge.md`).
- Debt removal is part of every step, not a phase: review-and-fix applies the plan-local `<debt-review>` block below alongside `<code-review>`. Findings inside the step's file set fold into the step's commit; anything larger is recorded under Found debt with file, finding, and size, and is never fixed in passing.
- Tests cover absence as well as presence: removed names error on access, blocked actions fail, invalid inputs are rejected. Each step runs its own tests before committing; Verify runs the workspace suite on the schedule above.

<debt-review>
For every file the step's diff touches, review the whole file, not just the diff:
1. Is every function in the file needed? Name any candidate for removal and why it is dead.
2. Can any functions be combined without obscuring them?
3. Is any function obsolete given this step's change - superseded, duplicated, or unreachable?
4. How does the file interact with the other files this step touched or the next step will touch? Name any coupling that can be simplified.
5. What simplification is available that the diff did not take?
6. Do the tests cover the absence of behavior - removed names erroring, blocked actions failing, invalid inputs rejected - and not only the presence of new behavior?
Fold each finding that stays inside the step's file set into the same commit. Record anything larger in this plan under Found debt with the file, the finding, and the estimated size; do not fix it in passing.
</debt-review>

## Found debt

- **Pre-existing `cargo fmt --all` drift** (found at step 1 review): 19 files across promptforge-core, promptforge-dev, and promptforge-mcp-server are not rustfmt-clean on master. Outside every step's file set. Fix as one formatting-only commit (rust rulebook: own commit, add the hash to `.git-blame-ignore-revs`) after step 6 lands, so the drift fix does not entangle with the reorder diff. Size: small.
- **`guide/promptforge-report.md` presents the removed `store.inject` as current API** (found at step 4 review): lines 663-674 describe it as the language's untrusted-injection mechanism, line 929 recommends elevating it, and line 931 recommends a general untrusted-value abstraction "later" - which step 1's `untrusted()` global now is. The document is a design report whose living-vs-historical status is the user's call. If living: update 663-674 to `untrusted(store.read(path))`, mark the 929/931 recommendations as landed. Size: small.
- **`crates/promptforge-core/design-core-residue.md:55` still describes the store as exposing `read_lines` and `inject`** (found at step 5 review; already stale after step 4). Same living-vs-historical question as the report. If living: update to the eight-method surface (`read`, `read_numbered`, `write`, `append`, `str_replace`, `delete`, `glob`, `exists`). Size: trivial.
- **`crates/promptforge-core/src/lua/tools_bridge.rs:269` doc on `install_tasks_table` describes a "phase-local author diagnostic callback" borrowing through `Scope`** (found at step 6 review): pre-existing, matches nothing the function does. Fix: rewrite the doc to describe what the function actually does. Size: trivial.

## Steps

### Step 1: `untrusted()` global

- Code: add `install_untrusted(lua)` in `crates/promptforge-core/src/lua/host.rs` - one `lua.create_function(|_, s: String| Ok(crate::untrusted::wrap(&s)))` installed as the global `untrusted`. Non-string input fails through mlua's automatic type error. Install in `SectionVm::new` (`crates/promptforge-core/src/lua/vm.rs:232`) immediately after `harden(&vm.lua)`; that single site covers H1, H2 sections, fanout workers, shared replay, and local tool handlers, in every phase.
- Test (`lua/tests.rs`): `untrusted("a < b")` escapes `<` and wraps with a fresh nonce per call (two calls on the same input differ); callable from shared-library top level and from a section chunk.
- Docs: `guide/src/lua.md` API table; `promptforge.md` quickref Lua API table.

### Step 2: `store.read(path[, start[, end]])`

- Code: add `StoreRef::read_range(path, start, end)` in `crates/promptforge-core/src/store.rs` - lock, `read`, slice lines in Rust so only the slice crosses the Lua boundary; the `Store` trait in `store/mem.rs` is unchanged (no test-fake churn). Bounds, evaluated in this order: `start < 1` is a `StoreError`; `start > line_count` returns `""` (range entirely beyond EOF); omitted `end` means the last line, and a given `end` clamps to it; `end < start` at this point is a `StoreError`. Lua: overload `read` on both store tables in `host.rs` (permanent and scoped) - `read(path)` whole file, `read(path, start)` from `start` to EOF, `read(path, start, end)` slice - dispatched to `read`/`read_range` and reported under the existing `STORE_READ_SUCCEEDED/FAILED` details.
- Test (`store/tests.rs`, `lua/tests.rs`): whole-file, start-only, start+end, clamped end, beyond-EOF empty, both error cases.
- Docs: rustdoc `# Errors` + doctest on `read_range`; `guide/src/store.md`; quickref.

### Step 3: `store.read_numbered(path[, start[, end]])`

- Code: move the numbering logic out of `MemStore::read_lines` (`store/mem.rs:281`) into a helper shared with `read_range` - the existing format (`N| line`, numbers right-aligned to the width of the largest emitted number) is the oracle, so move it, do not rewrite it. Add `StoreRef::read_range_numbered(path, start, end)`; with no bounds the whole file is numbered from 1. Lua: `read_numbered` on both store tables with the same optional bounds as `read`; new `STORE_READ_NUMBERED_SUCCEEDED/FAILED` details in `observe.rs`.
- Test: no-bounds output equals the current `read_lines` output byte-for-byte; a ranged read numbers absolutely (`read_numbered(p, 84, 85)` yields `84| ...`, `85| ...`); padding width across the 99-to-100 boundary; clamping and error cases shared with step 2.
- Docs: rustdoc `# Errors` + doctest; `guide/src/store.md`; quickref.

### Step 4: remove `store.inject`

- Code: remove `StoreRef::inject` (`store.rs:230-234`), both Lua bindings (`host.rs` permanent ~258-269, scoped ~419-433), the `STORE_INJECT_SUCCEEDED/FAILED` details (`observe.rs`), and the `untrusted.rs:6` doc mention. Migrate `promptforge/local/prompts/papergate.md:24` to `var.paper = untrusted(store.read("paper.md"))` and every remaining caller found by grep (`promptforge-core-tests/prompts/execution/store-triad.md`, the test suites).
- Test: `store.inject` is absent from Lua (indexing it errors); papergate parses; suite green.
- Verify: live `promptforge papergate` smoke against the migrated prompt.

### Step 5: remove `store.read_lines`

- Code: remove the `Store` trait method (`store/mem.rs:88`) and its `MemStore` impl (~281), the `StoreRef` wrapper, both Lua bindings, and the `STORE_READ_LINES_*` details. Migrate every caller to `read` or `read_numbered`: `execute/tests/local_tools.rs:105,129`, `execute/tests/model_and_reply.rs:171,272,700`, `execute/tests/observations.rs:82,230`, `execute/tests/exec_flow.rs:63`, `lua/tests.rs` (61, 989, 1046, 1775-1807, 1875-1907, 1962), `store/tests.rs` (20, 62-91), plus any prompt files grep finds.
- Test: `store.read_lines` absent from Lua; migrated tests green.
- Verify: full workspace `cargo test` (end of component).

### Step 6: section VM reorder

- Code (`execute/engine.rs:224-246`, `fanout/arm.rs:142`, `lua/vm.rs`): section startup becomes the linear sequence - `SectionVm::new` (harden, budget, `untrusted`), `apply_lua_limits`, `inject_host_with_var`, `install_host_apis`, `install_control_globals`, replay shared via `run_loaded_with_control` (an empty compiled chunk when `prompt.replay` is `None`), `install_captured_bindings`, then chunks. A `LuaBlockResult::Jump` from replay becomes the hard error "jump is not available during shared library load". During replay the `tools`/`models` tables work; the bare alias userdata globals install only after replay (decision 8). Delete `run_loaded_without_host`, `run_loaded_with_log`, `install_log_scoped`, `install_store_table_scoped`, and the nil-out cleanup; `new_for_section` collapses into the linear sequence.
- Test: invert `host_leaked_early` (shared sees the full environment at load); `jump` during replay errors; a shared function calling `tools.add` or mutating `var` succeeds when invoked from a later chunk; shared-defined globals are visible to section chunks; absent shared library takes the same path; replay consumes the configured `RunLimits`, not defaults.
- Docs: `guide/src/prompt-files.md` (shared replays per section VM as the first chunk, full environment, `jump` excluded), `guide/src/lua.md`, quickref, `SectionVm` doc comments.
- Verify: full workspace `cargo test`; live `promptforge papergate` smoke (papergate has no shared library, so it exercises the empty-chunk path).

### Step 7: design document

After implementation is complete, generate the design document: spawn one subagent whose entire prompt is - read this plan at c:\Users\Vinnie\.cursor\plans\untrusted_global_and_vm_reorder_42176da9.plan.md, grep for `<design-doc>`, and follow the block inside it. Move the generated `design-promptforge-store-and-vm-rework.md` into `crates/promptforge-core/` beside `design-core.md`.

<design-doc>
OUTPUT A DESIGN DOCUMENT, NOT CODE. Write one markdown file, design-promptforge-store-and-vm-rework.md,
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

## Out of scope

- **Replay gating (deferred; the tightening path for decision 6):** if real prompts show confusing top-level behavior at replay, add a phase-aware proxy `_ENV` for the replay chunk only - `__index`/`__newindex` Rust closures over the real globals plus an `Arc<AtomicBool>` replay flag on `SectionVm`, raising a phase error on `tools`/`models`/`var`/`reply`/`jump` access while the flag is set and forwarding otherwise. Because the gate lives in `__index` (evaluated at access time), shared functions called later resolve through it with the flag clear and work; nothing cacheable stays bricked. Needs `LuaProgram::load_with_environment` (`program.rs:163` area, `lua.load(bytes).set_environment(env).into_function()`). Self-contained at one call site; roughly 130 lines with tests.
- **A pure `numbered(text, start)` global:** add only if numbering non-store strings shows up in practice (decision 3).
- **No change to `promptforge: 1` frontmatter:** the `store.inject` removal is an accepted prompt-facing break, not an engine major.
