---
name: PromptForge refactor
overview: Execute the 83 review notes as an ordered refactor, run per vibe-rulebook and rust-rulebook.
todos: []
isProject: false
---

# PromptForge refactor plan

From the 83 review notes in `promptforge/review-notes.md` (committed; the decisions source of truth). Ordered by dependency. Each step names its files, behavior, and test.

Repo: `c:\Users\Vinnie\src\cursor\promptforge`. Crate under change: `promptforge-core` (`crates/promptforge-core/src/`). Integration prompts: `crates/promptforge-core-tests/prompts/`.

Done: step 2 (rename `walk_section_blocks` -> `run_one_section_impl`), commit `2c55b19`, 767 tests green.

## Execution protocol

Per `tools-public/rulebooks/vibe-rulebook.md` and `tools-public/rulebooks/rust-rulebook.md`:

- All work in subagents. Main context holds only: this plan, the step number, commit hashes, scratch paths, one-line statuses. Never source, diffs, or logs in main.
- At each step's start, TodoWrite the step checklist: Code, Commit, Review, Amend, Verify (cancel Verify when unscheduled). Do not start the next step until every item is completed or cancelled.
- Code: dispatch the coder subagent by reference (this plan's path + step number). The step carries its code, its test, and its doc lines. The coder applies the rust-rulebook and returns under 500 tokens: done/blocked, files touched, test command string.
- Pre-commit gate (tests before commit, all three green): `cargo fmt --all --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test -p promptforge-core`.
- Commit in main only: stage, commit (message names the step's intent), amend.
- Review: dispatch the review-and-fix subagent against the commit's diff. It applies the vibe-rulebook code-review block, overwrites `cabinet/_scratch/vibe-promptforge/vibe-review.md` with one-sentence failures, applies exactly one fix round, returns under 1000 tokens.
- Amend the commit when review-and-fix dirtied the tree (gate re-runs green first). No make-work commits; fold fixes into the commit they correct. Amend is user-authorized for this run's step commits.
- Verify: a subagent runs `cargo build --workspace` (compile check, all dependents) and `cargo test -p promptforge-core -p promptforge-core-tests` (behavior), and returns one line (pass, or fail plus a log path; main never reads the log). Known-environmental, pre-existing on HEAD and excluded from Verify: `promptforge-mcp-server`'s 7 integration tests fail because a real gateway occupies 127.0.0.1:8081 and the live MCP server locks `target\debug\promptforge-mcp-server.exe` (run with `CARGO_TARGET_DIR=target/gate` to dodge the lock). Scheduled: every 3rd step, the last step of each phase, the final step, and whenever review-and-fix dirtied the tree. On fail: coder fixes from the log path, amend or rule 7, Verify once more.
- Reversible calls are the executor's; record them in this plan with a falsifier. Hard-to-reverse calls go to the user. A bug in an earlier commit gets its own fix commit naming the bug (rule 7).
- Reduce line count where a step allows: prefer the deletion or consolidation that shrinks the tree over the addition that preserves it. The coder reports lines added/removed per file.

## Locked decisions

- `sys.index` (number, 1-based arm position within the current fanout) replaces `sys.taskid`. The three integration prompts using `sys.taskid` (`crates/promptforge-core-tests/prompts/execution/fanout-basic.md`, `fanout-epilog.md`, `fanout-store-writes.md`) and the docs move to `sys.index`. Falsifier: a fielded prompt reading `sys.taskid` as a string.
- `tools.add(alias, override?)` takes one alias; the array form `tools.add({"a", "b"})` covers bulk. `local/prompts/briefer.md`'s `tools.add("search", "fetch")` becomes the array form. Falsifier: a prompt needing per-alias overrides inside one bulk call.
- Fanout concurrency: the existing finite cap (`RunLimits::fanout_concurrency`, default 8, `NonZeroUsize`, enforced by `ArmWindow`) already satisfies note 62; step 10 is a pinning test, not a change. Falsifier: evidence the note meant collection size, not concurrency.
- `infer` has one shape (note 83): one round, no tools, fresh conversation, never sets `reply`, never touches `sys`. Forms: `models.infer(prompt)` (the section's current model) and `models.get(alias):infer(prompt)` (any declared model). The tool-loop infer (`InferContext::infer`, `ToolBag` and its generation cache, infer-side `tools.calls` seeding, `reply`/`sys.reply_finish_reason` updates) is deleted. A Lua block that needs tools uses `execute` on a section.
- Bare globals resolve in prose (notes 40, 47): `{{ x }}` reads the section-local global `x`. Scalars render naturally, tables as JSON, functions/userdata are an error, a missing global is an error. Falsifier: a prompt relying on `{{ x }}` failing as BadPath.
- H1 keeps `sys.id = 0`; the global counter hands 1.. to sections and arms. Falsifier: a prompt reading `sys.id` in H1 expecting the section sequence.
- Already satisfied by the code, test-only steps: step 10 (the cap exists and `config.rs` already pins the default), step 12 (arm no-reply is already `""` via `unwrap_or_default` in `fanout/arm.rs`).

## Phase 1: Structural

### Step 1. One block runner (notes 1, 5)

Merge `execute_live_h1`'s block loop into `run_one_section_impl`. H1 vs H2 is a caller-set mode, not a second loop.

- Files: `execute/block_walk.rs`, `execute/h1.rs`, `execute/engine.rs`, `execute.rs`.
- Behavior: `run_one_section_impl(vm, ctx, name, blocks, mode, sys, incoming_reply, client)` takes `BlockRunMode::LiveH1(&RuntimeResolution)` | `BlockRunMode::Section` and `(name, blocks)` instead of `&Section`. Live mode: Lua blocks run via `run_live_h1_block` (scoped live resolvers); prose binds the default model and the `always` scope from the producer's bindings-so-far, fresh `ToolCallCounts` per prose block, `prepare_scoped_tools` (no analysis validation, no global aliases), no `sys.model` enrichment, `set_global_string` reply. Section mode: today's behavior unchanged. The H1 shell keeps VM construction (`SectionVm::new`), limits, `inject_host`, `install_host_apis`, the infer hook with the live producer, `LiveH1State` extraction, and teardown. `run_one_section` (wrapper) unchanged in shape. `models.need`/`tools.need` outside live H1 stay runtime errors (already true via the H2 stub tables).
- Test: behavior-preserving; the existing suite (H1 tests in `execute/tests/live_infer.rs`, `exit_rules.rs`, section tests) stays green. Coder adds one test that a prompt with both H1 prose and H2 prose runs both through the shared loop.
- Verify: scheduled (step 1 of 3-step cycle: no; but review-dirty triggers it).

### Step 3. One borrowed frame (note 7)

Collapse `RunContext` / `WalkContext` / `BlockWalkContext` into one borrowed struct.

- Files: `execute/engine.rs`, `execute/block_walk.rs`, `execute/h1.rs`, `fanout/arm.rs`.
- Behavior: one struct (name it `RunFrame`) carries the union of fields; `item: Option<&Json>` (fanout only) and `initial_var: Option<&Json>` (top-level walk only) stay `Option`. Field-audit: drop unread fields; the `observer`/`observer_arc` and `debug`/`debug_arc` pairs collapse to one form each where a use-site deref suffices. `ControlContext` stays separate (owned, for Lua closures). H1 populates walk-only fields with empty defaults (`ToolBindings::default()` etc.); the impl reads them only in section mode.
- Test: behavior-preserving; existing suite stays green.
- Verify: no (unless review dirties).

### Step 4. `var` semantics (notes 16, 47, 48)

`var` persists across sections on the same walk; `execute`/`fanout` clone it in and discard it out; writes guarded at assignment; bare globals resolve in prose.

- Files: `execute/engine.rs`, `execute/block_walk.rs`, `execute/section_vm.rs`, `lua/vm.rs`, `lua/sys.rs`, `subst.rs`, `fanout/arm.rs`.
- Behavior:
  - The walk owns a `var: serde_json::Value`, seeded from H1's var (top level) or a caller snapshot (execute chain). Each section's VM is seeded from it; `run_one_section` returns the section's final `var` (read back before teardown) and `walk_siblings` rolls it forward. Jumps and child walks share the walk's var.
  - The `execute`/`fanout` control-global closures snapshot the caller VM's `var` at call time (the `&Lua` is in scope in `install_control_globals`) and pass the JSON into the callback; the contained chain / each arm seeds from that clone. `VmSeed` becomes a struct `{ var: Option<&Json>, item: Option<&Json> }` (arms get both).
  - Write guard: the `var` global becomes an empty proxy table over a hidden data table (same pattern as `seal_sys` in `lua/sys.rs`): `__newindex` validates JSON-representability via the serde bridge (function/userdata/thread rejected at the assigning line; nested tables deep-checked) and writes through; `__index` is the data table; `__metatable` locked. `vm.var()` reads the data table.
  - Bare-global substitution: `substitute` gains a globals-lookup callback supplied by the block walk (reads the VM global, converts to JSON). An unknown first segment resolves as a bare global; missing global errors; dotted paths index into the resolved JSON.
- Test: var persists H1->H2 and H2->H2 across fall-through and jump; `execute` clones in (child writes invisible to caller); each fanout arm gets a fresh clone; `var.f = function() end` errors at the assigning line; `{{ x }}` resolves a bare global; `{{ missing }}` errors.
- Verify: scheduled (3rd commit).

### Steps 5+6. `sys.id` global counter; `sys.index` replaces `sys.taskid` (notes 46, 82)

- Files: `execute/engine.rs`, `execute/support.rs`, `execute/h1.rs`, `fanout/arm.rs`, `fanout/mod.rs`, `crates/promptforge-core-tests/prompts/execution/fanout-basic.md`, `fanout-epilog.md`, `fanout-store-writes.md`.
- Behavior: a run-global counter (`Arc<AtomicU64>`-style, created in `run()`, threaded via the frame and `ControlContext` into every chain and fanout; fanout does NOT reset it, unlike `turns`). Every section entry and every arm takes the next id; entering the same section twice yields two ids. H1 keeps id 0. The per-chain `entered` counting and the arm `parent_id`-as-id plumbing are deleted. Arm `sys` gains `index` (number, 1-based, per fanout; nested fanout restarts at 1; absent outside fanout); `taskid` is removed.
- Test: same section entered twice gets two ids; an execute chain's sections continue the global sequence; arms get unique global ids and per-fanout `sys.index` from 1; reading `sys.index` outside a fanout errors; the three integration prompts pass with `sys.index`.
- Verify: no (unless review dirties).

## Phase 2: API changes

### Step 7. Frozen Tool objects (note 36)

- Files: `lua/handles.rs`, `lua/live.rs`, `lua/tools_bridge.rs`, `execute/scope.rs` (comments), `local/prompts/briefer.md`, `local/briefer.md`.
- Behavior: `LuaToolHandle` loses the `.description` setter and the override flag. `tools.need(alias, catalog_desc, override?)` records `model_description`; `tools.always(alias, override?)` updates it; `tools.add(alias, override?)` takes one alias (string or handle), array form for bulk (no per-element overrides). Precedence add > need/always > catalog falls out of the existing `binding_for_scope` chain.
- Test: override at need/always/add each reaches the advertised schema; add beats need; catalog text when no override; `handle.description = "x"` errors; `tools.add({"a","b"})` adds both.
- Verify: no (unless review dirties).

### Step 8. Drop the `tasks` table (note 42)

- Files: `lua/tools_bridge.rs`, `lua/handles.rs`, `lua/vm.rs`, `execute/section_vm.rs`, `execute/engine.rs`, `execute.rs`, `fanout/mod.rs`, `fanout/arm.rs`, `execute/tests/exec_flow.rs`, `fanout/tests.rs`.
- Behavior: `install_tasks_table`, `LuaSectionHandle`, the `task_handles` plumbing, and the userdata arm of `resolve_section_target` are deleted. `execute`/`jump`/`fanout`/`list_from_section` take heading strings only. Tests using `tasks['## X']` move to strings.
- Test: the `tasks` global is absent (reading it errors); string-target control flow stays green on the migrated tests.
- Verify: scheduled (6th commit).

### Step 9. One `infer` shape (note 83)

- Files: `execute/tools.rs`, `lua_models/userdata.rs`, `lua_models/mod.rs`, `lua/vm.rs`, `execute/engine.rs`, `execute/h1.rs`, `fanout/arm.rs`, `execute.rs`, `execute/tests/tool_bag.rs` (deleted), `execute/tests/live_infer.rs`, `execute/tests/debug_and_counts.rs`, `execute/tests/mod.rs`.
- Behavior: `InferContext` keeps only the direct path. `handle:infer(prompt)` routes to it with the handle's frozen binding; `models.infer(prompt)` resolves the current model and runs the same path. Neither advertises tools, sets `reply`, or touches `sys`. Delete: `InferContext::infer`, `prepare_tools`, `ToolBag`, `PreparedTools`, `CachedToolState`, and the `counts_slot`/`sys_live`/`local_tools`/`shared_tools`/`analysis`/`max_tool_iterations` fields and `attach_infer_hook` parameters. `run_tool_loop` loses its only production caller: delete the wrapper and port its test callers in `execute/tests/mod.rs`/`tool_loop.rs` to `run_prose_inference` with `ProseMode::Loop`. The "tool calls but no tools advertised" guard in `infer_direct` stays.
- Test: `models.get(alias):infer` sends the alias's model (assert the request body's model field at the scripted gateway); `models.infer` uses the current model; neither sets `reply` nor `sys.reply_finish_reason`; infer works in a fanout arm; `tool_bag.rs` deleted, its coverage ported or dropped with reason.
- Verify: scheduled (7th commit, end of phase 2).

## Phase 3: Behavior changes

### Steps 10-12. Fanout pins and the empty-collection error (notes 62, 65, 67)

- Files: `fanout/mod.rs`, `fanout/tests.rs`, `execute/tests/exec_flow.rs`.
- Behavior: `run_fanout_arms` (or `make_fanout_callback`, before any scheduling) rejects an empty collection with `Error::Lua`. Pin the existing finite concurrency cap and the existing arm no-reply `""` behavior with tests.
- Test: `fanout(worker, {})` errors; an arm with no reply yields `.text == ""` and `.ok == true`; the cap default stays pinned (already in `config.rs` tests - confirm).
- Verify: no (unless review dirties).

### Step 13. Store write-write race (note 74)

- Files: `store.rs`, `store/error.rs`, `lua/host.rs`, `execute/section_vm.rs`, `fanout/arm.rs`, `fanout/mod.rs`.
- Behavior: `StoreRef` keeps a write registry `path -> (fanout_token, arm_index)`. Arm `store.write` calls carry the token; a write to a path already written by a different arm of the same fanout is a hard `StoreError::WriteRace` (surfaces as a Lua error, aborts siblings per note 63). `append` is untracked; walk-section writes are untracked; a later fanout overwrites the record (sequential fanouts stay legal). The writer token threads through `install_store_table` and the arm's VM setup. (Plan's file list corrected: the check lives in `StoreRef`, not the backends - backends cannot see writer identity.)
- Test: two arms writing one path fail the run with the race error; two arms appending one path succeed; an arm rewriting its own path succeeds; sequential fanouts to one path succeed.
- Verify: scheduled (9th commit).

### Step 14. Store delete silent (note 55)

- Files: `store/mem.rs`, `store/file.rs`, `store/error.rs`, `store/tests.rs`.
- Behavior: `delete` on a missing path returns `Ok(())` (idempotent). Trait docs updated.
- Test: delete-missing succeeds; delete-existing removes.
- Verify: scheduled (10th commit, end of phase 3).

## Phase 4: Docs

### Step 15. Docs sweep

- Files: `promptforge.md`, `guide/src/lua.md`, `guide/src/prompt-files.md`, `guide/src/fanout.md`, `guide/src/models.md`, `guide/src/store.md`, module comments in `execute.rs`, `execute/engine.rs`, `execute/block_walk.rs`. Regenerate any derived guide via `crates/make-user-guide` if that is its role (coder confirms).
- Content: preamble/prologue/epilog are positions, not phases; `var` persists on a walk and is cloned into `execute`/`fanout`; bare globals in prose; `sys.id` global, `sys.index` arm position, `sys.taskid` gone; frozen tools and the override forms; no `tasks` table; the single `infer` shape; empty-collection error; write-write race; silent delete; off-walk `---` has two roles (leading takes the section off fall-through; after any block it cuts the section to a comment). Each behavior step already carried its one-line doc deltas; this step is the consistency and terminology sweep.
- Test: docs build (`mdbook build guide` if mdbook is present; otherwise a careful read), plus a subagent cross-check that every Lua-visible name in the docs matches the code.
- Verify: scheduled (11th commit, end of phase 4).

## Phase 5: Verification

### Step 16. Engine read-through

- A subagent reads `execute/engine.rs` fully against the 83 notes, confirms the notes match the code, and appends drift findings to `promptforge/review-notes.md` as new notes (append-only convention).

### Step 17. Test audit and final Verify

- A subagent audits `execute/tests/`, `fanout/tests.rs`, `lua/tests.rs`, `store/tests.rs` against the locked rules, lists behaviors lacking a pinning test, and the coder adds them.
- Final Verify: `cargo test --workspace` green.
