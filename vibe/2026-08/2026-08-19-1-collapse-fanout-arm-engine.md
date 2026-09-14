---
name: collapse the fanout arm engine
overview: Collapse the duplication between run_one_arm (fanout/arm.rs) and run_one_section (execute/engine.rs) into ONE engine with two thin drivers. Two behavior-identical carve-out commits (VM-lifecycle, then the block walk itself), then the arm becomes an adapter over the shared engine, then arms gain full control globals and jump, then the item-cap removal, then consolidation down to the forced delta list, then the design document.
todos:
  - id: step-1-vm-lifecycle
    content: "Step 1: carve the VM-lifecycle sequence out of run_one_section (behavior-identical)"
    status: pending
  - id: step-2-block-walk
    content: "Step 2: carve the block walk out of run_one_section (returns SectionFlow), Verify"
    status: pending
  - id: step-3-arm-adapter
    content: "Step 3: the arm becomes an adapter over the one engine (stubs kept), Verify"
    status: pending
  - id: step-4-arm-capabilities
    content: "Step 4: arms gain full control globals and jump (decision 3)"
    status: pending
  - id: step-5-cap-removal
    content: "Step 5: remove the fanout item cap (keep the concurrency bound)"
    status: pending
  - id: step-6-consolidation
    content: "Step 6: consolidation - delete surviving duplication, check against the delta list, Verify"
    status: pending
  - id: step-7-design-doc
    content: "Step 7: generate design-promptforge-arm-engine-collapse.md via the design-doc block, Verify"
    status: pending
isProject: false
---

# PromptForge: collapse the fanout arm engine

## Target

`run_one_arm` (`crates/promptforge-core/src/fanout/arm.rs`) duplicates most of `run_one_section` (`crates/promptforge-core/src/execute/engine.rs`)'s block lifecycle. This plan does five things: (1) carves the engine out of `run_one_section` so the section lifecycle lives in one place (steps 1-2, behavior-identical); (2) rewrites the arm as an adapter over that engine - worker templates run the full block walk, with the infer hook and lazy client creation (step 3, a behavior change); (3) gives arms full control globals and jump (step 4, decision 3, a behavior change); (4) removes the total fanout item cap, leaving only the concurrency bound (step 5, decision 5, a behavior change); and (5) consolidates the arm driver down to the forced delta list (step 6). Steps 1-2 are proven by the existing suite staying green unchanged; steps 3-5 carry their own tests for the new behavior.

## Decisions

1. **One engine; the arm is a thin adapter** (user decision - an arm is normal flow). There is one block walk; `run_one_section` and `run_one_arm` both drive it. The seam is `SectionFlow`: the shared walk returns it, and each driver maps it to its own outcome - the chain continues on Jumped/Returned/FellThrough; the arm maps it to `LuaFanoutResult` and catches `ToolLoopExhausted` into the soft-degrade stub. The engine contains ZERO arm special-casing: every genuine delta is adapter code at the driver boundary - the driver builds the `sys` JSON (with `taskid`, parent `id`), seeds the `item` global, passes its own observer and `turns` values into the same parameter slots, wraps the call in `cancel::scope`, and maps the outcome. No policy object, no knobs inside the walk. The complete list of what genuinely distinguishes an arm (enumerated in The driver surface below): owned payload across the spawn boundary, `item` injection, `sys.taskid` with `sys.id` = parent id, `LuaFanoutResult` mapping, exhaustion stub, `ArmFinalizer`, proxy observer/debug side channels, cancel re-install, shared `turns` counter. Everything else the arm does differently today - stubbed control globals, no infer hook, no lazy client, single-prose-only, no conversation roll-forward, no per-block schema rebuild, no `ProseMode` - is drift, deleted in steps 3-4. Rejected: two drivers sharing helper functions (this plan's prior formulation) - it preserves seven accidental differences behind the word "driver".
2. **Two carve-out commits, then the arm becomes an adapter, then the capability flip, then the cap removal, then consolidation.** This is unification, not extraction-sharing: the engine IS the body of `run_one_section`, carved out whole; steps 1-2 use behavior-identical moves only so the suite proves each move. Step 1 carves the VM-lifecycle sequence (host injection through `install_captured_bindings`). Construction (`SectionVm::new_for_section`) and `apply_lua_limits` stay driver-owned, two carve-outs forced by review: construction failure handling genuinely differs per driver (the walk propagates; the arm finishes its finalizer, and its VM must outlive the cancel-scoped body for the epilogue teardown), and the walk's limits failure must keep propagating as a bare `?` - routing it through teardown would emit `LUA_TEARDOWN_STARTED`/`SUCCEEDED` events the old code never emitted, an observable change the suite cannot see. Step 2 carves the block walk itself - the for-loop over `section.blocks` with its conversation state, `seen_prose` gating, per-block schema rebuild, and `ProseMode` selection - returning `SectionFlow`; `run_one_section` becomes setup + infer hook + walk + teardown + observation wrapper. The arm's behavior is unchanged through both, so a red suite in steps 1-2 means the carve-out itself is defective. Step 3 rewrites `run_one_arm` as an adapter over the engine with the stub callbacks still installed (decision 1) - the block-walk behavior change. Step 4 installs the real control globals and jump (decision 3) - the capability behavior change. Step 5 removes the item cap (decision 5). Step 6 consolidates: whatever duplication survives is deleted, and the result is checked against the driver-surface list.
3. **Arms gain full capabilities** (user decision - there are no special cases), delivered in steps 3-4. `run_one_arm` currently stubs execute, fanout, and list_from_section, rejects jump, never installs `attach_infer_hook` (so `model:infer` is dead inside an arm today), requires a client to be handed to it (no lazy `env_client_with_limits`), and runs a hand-rolled prologue/single-prose/epilog instead of the block walk - no conversation roll-forward, no per-block schema rebuild, no `ProseMode`. Step 3 puts the arm on the shared block walk with the stubs still installed: the infer hook, lazy client creation, multi-prose workers, conversation roll-forward, per-block schema rebuild, and `ProseMode` all arrive there, and `SectionFlow::Jumped` still maps to the existing jump-rejection error. Step 4 flips the control surface: the real control globals resolved over the worker section's visible set (the set the worker was resolved from, minus the worker, plus its children), and jump drives a child walk. Threading (step 4): `FanoutContext` and `ArmPayload` gain the worker's home slice, the run's `task_handles`, and `execute_depth`; `make_fanout_callback` receives the caller's depth and each arm runs one level deeper, so recursion accounting accumulates across fanout boundaries instead of resetting, and `MAX_EXECUTE_DEPTH` remains the only recursion constraint. (Revised in step 4's fix round: the review found the arm reconstructing its home slice by tail-chopping the post-chain visible set, an unchecked cross-module layout invariant - `make_fanout_callback` now builds the home slice where the layout is constructed and threads it through, and each arm's control globals derive the visible set from it.) Jump semantics (step 4): a jump from any chunk of the arm's block walk resolves over the worker's visible set and drives a child walk from the target (the same chain-slice rule as the engine's execute callback); the arm's own remaining blocks are skipped - jump transfers control, it does not return - and the arm's result text is the child walk's returned value or final reply. The child walk counts its own `sys.id` from 1, matching a contained execute chain. The arm's own sys contract is unchanged: `id` stays the parent section's id and `taskid` stays the 1-based collection position - that is the arm's user-visible identity, and existing prompts depend on it. The exhaustion stub stays (a stuck arm should not kill sibling evidence). The existing stub-error assertion in `execute/tests/exec_flow.rs` (the `list_from_section() is not available inside a fanout arm` test) passes verbatim through step 3 and is replaced in step 4 by a test that the call works. Rejected: keeping any stubs (no reason for special cases).
4. **The carve-out steps (1-2) are behavior-identical, proven by the suite.** No test changes in steps 1-2 except mechanical call-site updates; any assertion change means the carve-out changed behavior and is wrong. Steps 3+ add new behavior (arm capabilities, item cap removal) and carry their own tests.
5. **No total item cap on fanout; only concurrent arms are bounded** (user decision), delivered in step 5 as its own commit - it is independent of the engine unification and must not be entangled with it. `max_fanout_items` is removed. The concurrency window (`fanout_concurrency`) already bounds how many arms are in flight at once; a large collection just takes longer, and whether that's worth it is the author's call. This is a public API break: `RunLimits::max_fanout_items()` (builder) and `RunLimits::fanout_items()` (getter) in `execute/config.rs` are deleted, along with the field itself - which since step 3 lives inside the `RunLimits` threaded through `FanoutContext` (step 5's review corrected this record: an earlier draft located the field on `FanoutContext` directly) - and the item-cap check in `run_fanout_arms`. No guide documents the cap, so no guide changes; the cap-rejection tests in `fanout/tests.rs` and `execute/tests/exec_flow.rs` are deleted and replaced by a test that a collection larger than the old default cap (1024) succeeds, using a pure-Lua worker with no client so the 1025-arm run stays fast. Rejected: keeping the cap as a "safety" limit - it prevents legitimate large mappings without preventing anything the concurrency bound doesn't already handle. Extended in step 6 (decision currency): the `fanout()` getter was renamed to `fanout_concurrency()` and the builder to `max_fanout_concurrency()` - post-cap-removal the bare `fanout()` name was ambiguous about which bound it returns; same commit, same API break. (Extended in step 6 per decision currency: the surviving getter `fanout()` was ambiguous once `fanout_items()` was gone, so it was renamed `fanout_concurrency()` and the builder moved to `max_fanout_concurrency()` - the debt review's suggested getter name collided with the existing builder, since Rust inherent impls allow no same-name setter/getter pair; the `max_*` builder spelling matches `max_tool_iterations`/`max_response_bytes`, and the three guide snippets naming the builder were updated to match.)

## Execution protocol

- Per `tools-public/rulebooks/vibe-rulebook.md`: one testable commit per step carrying code, test, and docs; coder subagent dispatched with the plan path and step number; review-and-fix applies `<code-review>` from that file against the step's diff plus the plan-local `<debt-review>` block below - one debt-review subagent per modified file, run in parallel, each reviewing the whole file - overwriting `cabinet/_scratch/vibe-arm-engine-collapse/vibe-review.md` (code-review findings) and writing `debt-<file-name>.md` per reviewed file (debt findings); amend on a dirtied tree. Fix/review policy (user decision, overriding the rulebook's one-fix-round default): FIXES ARE UNLIMITED - keep fixing until every finding is addressed; REVIEWS ARE CAPPED AT TWO per step - the initial review plus one re-review after the fixes. Findings still open after the second review are not reviewed again; they go to Found debt with an estimated size.
- Per `tools-public/rulebooks/rust-rulebook.md`: no `unwrap` outside tests; `cargo fmt --all` and `cargo clippy -p promptforge-core --all-targets --all-features -- -D warnings` green before each commit (master is now rustfmt-clean after the drift commit, so fmt should touch nothing outside the step's files).
- Decision currency: where a step's implementation contradicts, extends, or resolves a decision recorded here, the step revises this plan in the same commit, naming what forced the change.
- Verify (workspace `cargo test`) runs on the rulebook schedule: step 2 (end of the carve-out component), steps 3 and 6 (every 3rd step), step 7 (final step), and whenever review-and-fix dirtied the tree. Steps 1-2's proof is the existing suite staying green unchanged.
- Tests cover absence as well as presence.

<debt-review>
For EVERY file the step's diff touches, spawn one debt-review subagent per file (in parallel). Each subagent reviews the WHOLE file, not just the diff, hunting:
1. Duplicate code - blocks that repeat within the file or restate what a sibling module already provides.
2. Simplification opportunities - functions that can be combined without obscuring them, control flow that can collapse, machinery heavier than what it does.
3. Unused or dead functions - name each candidate for removal and why it is dead.
4. Obsolete functions - superseded, duplicated, or unreachable given this step's change.
5. Coupling - how the file interacts with the other files this step touched or the next step will touch; name any coupling that can be simplified.
6. Test gaps - do the tests cover the absence of behavior (removed names erroring, blocked actions failing, invalid inputs rejected) and not only the presence of new behavior?
Each subagent writes its findings as one-sentence entries (file and line, the problem, the single change that fixes it) to `cabinet/_scratch/vibe-arm-engine-collapse/debt-<file-name>.md`, one file per reviewed source file; an empty result means the file is clean. Fold each finding that stays inside the step's file set into the same commit - fixes are unlimited, reviews are capped at two per the execution protocol. Record anything larger in this plan under Found debt with the file, the finding, and the estimated size; do not fix it in passing.
</debt-review>

## Found debt

Populated by the `<debt-review>` block as steps run. (The `run_one_arm`/`run_one_section` duplication this plan addresses was the prior plan's recorded candidate; the remaining prior-plan items - the stale generated guides, the setext pin - are decided or parked there, not here.)

From step 1's review (deferred, each with its owning step):

- `execute/engine.rs`: ~17 repeated teardown-and-return sites through the block walk - step 2's carve restructures exactly this. Size: medium (dissolves into the carve).
- `execute/engine.rs`: duplicated callback capture lists (`execute_callback`/`fanout_callback` clone the same context fields) - steps 2/4 restructure this. Size: small once the walk is carved.
- `execute/engine.rs`: step-4 coupling - `visible_sections`, `section_position`, and `make_fanout_callback` are private to `engine.rs`, and the callback constructors are inline closures unreachable from `fanout/`; step 4's real arm callbacks need them shared. Size: small (visibility moves + extraction). Also step-4-owned from step 3's second review: the per-`execute()` `exec_task_handles.clone()` (engine.rs:411, part of the capture-list picture), and the unreachable, untested "caller not in slice" tolerant path in `visible_sections` (engine.rs:592-593) - step 4 touches `visible_sections` anyway; either delete the path or pin it with a test.

From step 3's review (deferred):

- `execute/engine.rs`: per-section O(tree) section-tree clones in the callback captures (`exec_siblings`/`exec_top`/`visible` clone the tree per section). Performance, not correctness. Unowned - candidate for step 6 if the consolidation touches the captures. Size: medium.
- `fanout/mod.rs`: `ArmInputs`/payload field-copy split - step 4 adds three more fields (visible set, task_handles, execute_depth) to that copy list, so the split belongs to step 4 or step 6. Size: small.
- `execute.rs`: the identical 11-argument tail of `execute_live_h1`/`run_sections` wants one borrowed context struct, but h1 borrows the client while `run_sections` consumes it - step 6 owns the driver surface. Size: medium. (Resolved in step 6: `RunContext` in `execute/engine.rs` bundles the shared tail, built once in `run`; the frame holds `&Option<GatewayClient>`, which h1 clones read-only and `run_sections` clones into its owned, lazily-created walk slot.)
- `execute/tests/exec_flow.rs` leftovers: a `section_fixture` builder, `loop_capable` parser assertions. Unowned, small. (Resolved in step 4's fix round: the parser assertions and the shared arm-fixture scaffold landed; the `use super::run;` line is NOT redundant - both globs (`super::*` and `super::super::*`) provide a `run` name and the explicit import disambiguates, verified by E0659 on removal. Still open: the file-wide `---\nname: t\n...` frontmatter restatement (~50 sites) beyond the arm scaffold. Unowned, small. Resolved in step 6: a file-local `flow_prompt!` macro fuses the shared frontmatter with each test's body literal via `concat!`, keeping every site typed `&'static str`; 63 sites converted, the one `max_tool_iterations` variant stays hand-written.)
- `fanout/tests.rs` leftovers: `sibling()` adoption in the first three tests, per-test import hoisting, window-test cleanup. Unowned, small. (Resolved in step 6: the `sibling()` adoption had already landed; the two in-test `use std::error::Error as _;` imports hoisted to module scope, and the window test dropped its redundant `max_outstanding` accumulator and needless `done` binding.)

Named by step 3's review per decision currency (already landed): `parse_heading_address` now checks the empty name before the whitespace gate's fallthrough, so a marker-only heading (`"###"`) reports "has no name" - the old order made that branch unreachable. Pinned by `resolve_sibling_marker_only_heading_errors_as_nameless`.

From step 5's review (deferred):

- `execute/config.rs`: the `fanout()` getter name is ambiguous now that `fanout_items()` is gone (it returns the concurrency bound) - a rename is a public API change beyond decision 5; step 6 owns the driver surface. Size: small. (Resolved in step 6: see decision 5's extension note.)
- `fanout/mod.rs`: module-split opportunity (scheduler vs arm vs proxies). Unowned. Size: medium.
- `design-promptforge-section-marker-and-fanout-collections.md` (lines 33, 74): describes the deleted item cap as live behavior. Historical design doc - do not rewrite; step 7's design document supersedes it.

From step 2's review (deferred, owned by step 3):

- `execute/block_walk.rs`: the walk reads 11 of 19 `WalkContext` fields - narrow the walk's input struct so the arm adapter supplies only what the walk reads. Size: small-medium.
- `execute/block_walk.rs`: prose substitution hardcodes `item: None` - the walk needs an `item` parameter or `{{item}}` worker prose hard-errors in step 3. Size: small.
- `execute/block_walk.rs` + `fanout/`: lazy client creation needs a whole `RunLimits`, but `ArmPayload` carries only decomposed limit fields - thread `RunLimits` through `FanoutContext`/`ArmPayload`. Size: small.

## The driver surface

The engine is one sequence: `SectionVm::new_for_section` → `apply_lua_limits` → `inject_host` → `install_host_apis` → `install_control_globals` → `replay_shared` → `install_captured_bindings` → infer hook → block walk (per block: scope/counts/local-schemas, model resolution + sys enrichment, prose substitution, `ProseMode` selection, tool loop, `bind_reply`, reply/conversation roll-forward) → `teardown` → `SectionFlow`.

The arm driver supplies exactly these deltas, and nothing else:

1. Owned `ArmPayload` (every input cloned across the `JoinSet` spawn boundary) vs borrowed `WalkContext`.
2. `item` injection (the collection member) vs `initial_var`.
3. `sys.taskid` (1-based collection position) and `sys.id` = the parent section's id, vs the chain-position `id`.
4. Outcome mapping: `SectionFlow` → `LuaFanoutResult`; `Jumped` drives a child walk per decision 3. `FellThrough` carries the engine's reply roll-forward: a no-output worker's arm text is the incoming reply, not `""` as the deleted hand-rolled body returned - engine semantics win per decision 1, pinned by `fanout_arm_without_output_inherits_the_incoming_reply` (named by step 3's review per decision currency).
5. Exhaustion: `ToolLoopExhausted` soft-degrades to the stub instead of propagating.
6. Observation: `FANOUT_ARM_STARTED` plus `ArmFinalizer`'s exactly-one terminal event, through `ProxyObserver`/`ProxyDebugCapture` bounded side channels, vs `SECTION_STARTED`/`SECTION_FINISHED` direct to the observer.
7. Cancel: re-install the explicit handle via `cancel::scope` vs inherit the task-local.
8. Turns: one `Arc<AtomicU32>` shared by all arms of the fanout vs the walk's own counter.

Everything else is shared. Through steps 1-2 the arm keeps its current body (stub callbacks included) - the stubs are part of step 1's parameterization. Step 3 rebodies the arm onto the engine with the stubs still installed; step 4 removes them.

## Steps

Each step is one commit, worked as the rulebook loop: Code (coder subagent, dispatched by reference with the plan path and step number) → Commit (message naming the step's intent) → Review (review-and-fix per the execution protocol: unlimited fixes, reviews capped at two) → Amend (on a dirtied tree) → Verify (on the execution protocol's schedule, cancelled otherwise).

### Step 1: carve the VM-lifecycle sequence out of `run_one_section`

- Code: carve the setup sequence - host injection → `install_host_apis` → `install_control_globals` → `replay_shared` → `install_captured_bindings` - out of `run_one_section` into the engine's setup half (home: `execute/section_vm.rs`). Construction (`SectionVm::new_for_section`) and `apply_lua_limits` stay driver-owned per decision 2. Parameterize only what differs: the sys extras (the arm's `taskid` and parent-id `id` vs the walk's section-id `id`), `initial_var` vs `item` injection (`VmSeed`), and the three control-global callbacks. The arm switches to calling this setup in this step but keeps its existing stub closures - removing them is step 4's behavior change, not this step's.
- Test: the existing suite stays green unchanged - that is the proof of no behavior change. In particular the stub-error assertion in `execute/tests/exec_flow.rs` must still pass verbatim.
- Docs: rustdoc on the carved-out setup; no guide changes (internal).
- Commit: "carve the VM-lifecycle sequence out of run_one_section".
- Review: review-and-fix per the execution protocol, including one debt-review subagent per modified file.
- Amend: amend on a dirtied tree.
- Verify: not scheduled - run only if review dirtied the tree.

### Step 2: carve the block walk out of `run_one_section`

- Code: carve the for-loop over `section.blocks` - conversation state, `seen_prose` gating, counts install, model resolution + sys enrichment, per-block schema/dispatch rebuild, prose substitution, `ProseMode` selection, the tool-loop invocation, `bind_reply`, reply roll-forward - out of `run_one_section` as the engine's walk half, returning `SectionFlow` (home: `execute/engine.rs` beside `run_one_section`, or a new `execute/block_walk.rs`; the coder picks the cleaner seam against the existing module layout). `run_one_section` becomes: step 1's setup half + infer hook + block walk + teardown + observation wrapper. The arm is untouched in this step.
- Test: the existing suite stays green unchanged.
- Docs: rustdoc on the carved-out walk; no guide changes.
- Commit: "carve the block walk out of run_one_section".
- Review: review-and-fix per the execution protocol, including one debt-review subagent per modified file.
- Amend: amend on a dirtied tree.
- Verify: scheduled (end of the carve-out component) - workspace `cargo test`.

### Step 3: the arm becomes an adapter over the one engine (decision 1)

- Code: rewrite `run_one_arm` as an adapter: build engine inputs from the `ArmPayload` (sys with `taskid`/parent `id`, the `item` global, the proxy observer, the shared `turns`), call step 1's setup half and step 2's block walk, map the outcome. The stub control-global callbacks STAY in this step, and `SectionFlow::Jumped` still maps to the existing `jump(...) is not allowed inside a fanout arm` error. New in this step: `attach_infer_hook` installed; lazy client creation (`env_client_with_limits`) when the arm was handed no client; the full block walk runs - multi-prose workers, conversation roll-forward, per-block schema rebuild, `ProseMode`. `SectionFlow::Returned`/`FellThrough` map to `LuaFanoutResult::success`; `Err(ToolLoopExhausted)` maps to the exhaustion stub. `ArmFinalizer`, the proxy channels, cancel re-install, and the shared `turns` counter are untouched. The arm's hand-rolled prologue/prose/epilog body is deleted.
- Test: new - a multi-prose worker runs every prose block with the reply rolling forward; `tools.add` between prose blocks reaches the next model turn; `model:infer` works inside an arm; an arm handed no client creates one lazily when prose needs it. Absence: the stub errors still fire verbatim (the `exec_flow.rs` assertion passes unchanged) and jump is still rejected with the existing message. The existing arm tests (exhaustion stub, cancel, observation) stay green.
- Docs: rustdoc on the changed items; update the `fanout` module docs' arm-body description.
- Commit: "run fanout arms on the shared block walk".
- Review: review-and-fix per the execution protocol, including one debt-review subagent per modified file.
- Amend: amend on a dirtied tree.
- Verify: scheduled (every 3rd step) - workspace `cargo test`.

### Step 4: arms gain full control globals and jump (decision 3)

- Code: replace the arm's stub control-global callbacks with the real ones, resolved over the worker section's visible set; `SectionFlow::Jumped` drives a child walk per decision 3's jump semantics instead of erroring. Grow `FanoutContext` and `ArmPayload` with the worker's visible set, the run's `task_handles`, and `execute_depth`; `make_fanout_callback` takes the caller's depth and each arm runs one level deeper.
- Test: new - jump inside a fanout arm starts a child walk and its reply becomes the arm's text; execute inside an arm runs a contained chain; fanout inside an arm maps over a collection; list_from_section inside an arm reads items; recursion across a fanout boundary accumulates depth and trips `MAX_EXECUTE_DEPTH`. The stub-error assertion in `execute/tests/exec_flow.rs` is replaced by the list_from_section-works test. The existing arm tests stay green.
- Docs: rustdoc; update any guide text that states the arm restrictions (the stub contract is not in the guides at plan time, but the step checks).
- Commit: "give fanout arms full control globals and jump".
- Review: review-and-fix per the execution protocol, including one debt-review subagent per modified file.
- Amend: amend on a dirtied tree.
- Verify: not scheduled - run only if review dirtied the tree.

### Step 5: remove the fanout item cap (decision 5)

- Code: delete `RunLimits::max_fanout_items()` (builder) and `RunLimits::fanout_items()` (getter) in `execute/config.rs`, the `max_fanout_items` field on `FanoutContext`, and the item-cap check in `run_fanout_arms`. The concurrency window (`fanout_concurrency`) is the only bound.
- Test: delete the cap-rejection tests in `fanout/tests.rs` AND `execute/tests/exec_flow.rs` (`fanout_oversized_collection_errors`, found by step 3's debt review); add a test that a collection larger than the old default cap (1024) succeeds, using a pure-Lua worker (immediate return, no client) so the 1025-arm run stays fast.
- Docs: none - no guide documents the cap.
- Commit: "remove the fanout item cap".
- Review: review-and-fix per the execution protocol, including one debt-review subagent per modified file.
- Amend: amend on a dirtied tree.
- Verify: not scheduled - run only if review dirtied the tree.

### Step 6: consolidation

- Code: whatever duplication survives steps 3-4 is deleted in this step - the arm module should contain only `ArmPayload`, `ArmFinalizer`, and the thin `run_one_arm` adapter (input construction + `SectionFlow` mapping + exhaustion catch + epilogue observation). Check the result against the eight-delta driver surface: any difference between the arm and the walk that is not on the list is either moved onto it (this plan revised in the same commit, naming what forced it) or eliminated. (Result: the arm module already matched the target shape after steps 3-5 - `ArmInputs` stays as the shared half of the owned-payload delta, built once per fanout. The consolidation collapsed the two call lists both drivers restated onto `ControlContext` methods: `sys_json(id, name)` (the run's `when`/`execution`/`section_count` plus a fresh `now`; the driver supplies only its id delta) and `attach_infer_hook(vm, client, name)` (eight run-wide slots; the driver supplies only its client snapshot and section name). No arm/walk difference beyond the eight deltas was found, so the driver surface stands unrevised.)
- Test: the suite stays green; the debt-review block applies in full to `fanout/arm.rs` and `execute/engine.rs`.
- Docs: rustdoc touch-ups from the consolidation.
- Commit: "consolidate the arm driver to its delta list".
- Review: review-and-fix per the execution protocol, including one debt-review subagent per modified file.
- Amend: amend on a dirtied tree.
- Verify: scheduled (every 3rd step) - workspace `cargo test`.

### Step 7: design document

- Code: spawn one subagent whose entire prompt is - read this plan at c:\Users\Vinnie\.cursor\plans\collapse_the_fanout_arm_engine_*.plan.md (use the actual path of this file), grep for `<design-doc>`, and follow the block inside it. Move the generated `design-promptforge-arm-engine-collapse.md` into `crates/promptforge-core/` beside `design-core.md`. If the plan carries no key design choices, write no document and return the reason.
- Test: none (documentation deliverable).
- Docs: the design document is the deliverable.
- Commit: "add the arm-engine-collapse design document".
- Review: review-and-fix per the execution protocol (the debt-review block is inert here - no source files touched).
- Amend: amend on a dirtied tree.
- Verify: scheduled (final step) - workspace `cargo test`.

<design-doc>
OUTPUT A DESIGN DOCUMENT, NOT CODE. Write one markdown file, design-promptforge-arm-engine-collapse.md,
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

- The stale generated user guides and the setext pin - decided or parked in the prior plans' Found debt, not this one.