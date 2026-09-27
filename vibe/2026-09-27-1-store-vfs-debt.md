---
name: store-into-vfs debt removal
overview: "Remove the 12 debts the Debt Collector retained from the store-into-VFS work (f4f92899..73bc2b6d): pass the scope through a changed public Vfs::acquire and delete the process-wide scope map, end a run's scope when the run ends, fix claim pruning and claim_write, unblock admission for call chains, route captured store functions through the effect path, and correct the store probe, store-view error paths, and InvalidRange docs."
todos:
  - id: scope-through-acquire
    content: "D1-10: add AcquireContext, change Vfs::acquire to take it, rewire VfsRef/Router/spawn/store_view, delete SCOPES/remember_scope/forwarded_scope, update every implementer and doc, regenerate public-api.txt; directly-wrapped VfsRef test"
    status: pending
  - id: end-run-scope
    content: "C-33: closed flag on Scope, detail::end_scope called at Done and on early drop, closed scopes refuse with PermissionDenied, drop-timing doc pass; held-access post-run read test and after-Done refusal test"
    status: pending
  - id: fix-pruning
    content: "D1-1, D1-2: ordered pruning only with one live scope, remove empty regions, reset the prune threshold from the surviving count; extend two_live_scopes_writing_one_path_conflict past 4096 claims, empty-region test"
    status: pending
  - id: claim-write-two-phase
    content: "D1-5, D1-6: claim_write checks everything before recording, adds ancestors[P].created and paths[ancestor].write checks; rename/copy combined claim; both-order sibling tests and refused-claim-records-nothing tests"
    status: pending
  - id: call-chain-admission
    content: "D1-3: a call chain parked on a join releases the nearest holding task's slots and retakes them on wake; stall guard fires when nothing queued can be admitted; call-in-arm fanout test at tasks.concurrency(1)"
    status: pending
  - id: captured-store-functions
    content: "D1-4, D1-31: store function values follow the load phase so captured locals go through Effect::Store after load; captured-local Determinism test and Effect::Store test"
    status: pending
  - id: store-probe
    content: "D1-7: the probe stats the store root through the one-mount router (NotFound on root is success); refusing-backend test expects Done with RunErrorKind::Store"
    status: pending
  - id: store-error-reporting
    content: "D1-8, D1-9: StoreScoped trims only the leading slash; vfs.md line 582 names both InvalidRange producers; /my/store path test"
    status: pending
  - id: exit-checks
    content: Full local check run (tests, clippy, fmt, rustdoc -D warnings, xtask api --check, guide build) and stale-term sweep
    status: pending
isProject: false
---

# Remove the debt the store-into-VFS plan added

<product-contract>

## Product Requirements

- Scope and target work:
  - The target is the 13 commits `f4f92899..73bc2b6d` on `master` in the `promptforge` repository, which carry out `vibe/2026-09-26-4-store-into-vfs.md`. The baseline is `upstream/master` at `f4f92899`.
  - Findings were checked against `HEAD` `3088311d`. It adds only a test-driver fix (`70ded04d`) and plan bookkeeping, and no finding changes except C-33, whose test-driver half that fix already covers.
  - This plan removes 12 retained debts: 9 introduced (D1-1, D1-2, D1-3, D1-6, D1-7, D1-8, D1-9, D1-10, C-33) and 3 cheap fixes (D1-4, D1-5, D1-31).
  - Paths are relative to the repository root, and line numbers are at `3088311d`. Once earlier work items land, treat line numbers as locators and find the named symbol.
- Cleanup goals:
  - Store verdicts and post-run reads stop depending on claim-table size, access drop timing, or finish order.
  - Admission never deadlocks a valid prompt, and a scheduler that can't make progress reports a stall.
  - Every public doc statement the target touched matches the code.
  - The process-wide scope map is gone.
- Non-goals:
  - No change to the Lua `store` or `tasks` surface, and no new `VfsError` variant.
  - No work on the rejected candidates listed under Deferred and Out of Scope.
  - No cross-run determinism beyond what C-33 and D1-1 require.
- Success criteria:
  - Every regression test in the Testing Plan fails at `3088311d` and passes after its work item.
  - The full local check run passes, including `xtask api --check` against a regenerated `crates/promptforge/public-api.txt`.
  - `remember_scope`, `forwarded_scope`, and `SCOPES` appear nowhere under `crates/`, and no doc says a backend receives a bare `ExecId` from `Vfs::acquire`.

## Functional Specification

### Debt Inventory

- Debt added:
  - D1-1 (introduced, `5085db88`): claim pruning ignores other live scopes.
    - `prune_ordered_epochs` in `crates/promptforge-internal/vfs/src/handle.rs` (lines 970-1041) drops any epoch ordered before its own scope's live identities, without consulting other live scopes. But `ClaimsTables::conflicts` (401-416) is the only cross-scope check.
    - Impact: once a shared base's ledger passes `PRUNE_AT` (4096 entries), a second run's write to a path a live first run wrote can land. With a host base, that overwrite reaches disk.
    - Contradicts `crates/promptforge/src/vfs.md` lines 90, 92, 329 and 499.
    - Target: an epoch that another live scope could conflict with is never pruned.
  - D1-2 (introduced, `5085db88`): pruning never removes emptied entries.
    - `prune` (915-923) never removes emptied region keys from `paths`, `children`, `patterns`, `subtrees` or `ancestors`. The baseline removed them.
    - Impact: after 4096 distinct regions, every claim runs a full prune under the ledger mutex, and the key set grows without bound.
    - Target: emptied regions are removed, and the prune threshold resets from the surviving entry count.
  - D1-3 (introduced, `eb7f478d`): admission can deadlock when a task's call chain spawns and joins.
    - A call chain never holds slots, because `start_chain` sets `holding: false`. So `park_wait` (`crates/promptforge-internal/engine/src/execute/scheduler/tasks.rs` 710-722) releases nothing when a call chain parks on `join_any` or `await_tasks`, while the calling task keeps its slot.
    - Impact: a task that calls a section which spawns and joins deadlocks at a limit of 1, or at the default of 8 with 8 or more such arms.
    - The stall guard in `drive.rs` (44-53) only fires when `spawned` is empty, so the engine returns `Pending` with no effects instead of reporting a stall.
    - Contradicts the plan's nested-fanout guarantee and the `drive.rs` module doc.
    - Target: a call chain parked on a join releases the calling task's slots, and a queue where nothing can be admitted reports a stall.
  - D1-6 (single-claim half introduced by `5085db88`): a refused operation leaves phantom claims.
    - `Claims::claim_write` (446-535) records the write epoch and earlier may-create entries before a later may-create check can refuse.
    - The two-path half predates the target: `Access::rename` (1807-1818) and `Access::copy` (1827-1835) keep the first claim when the second refuses. It's cheap to fix in the same change.
    - Target: a refused operation records nothing.
  - D1-7 (introduced, `61c74282`): the store probe never reaches the store backend.
    - The run's store probe (`crates/promptforge-internal/engine/src/execute/run.rs` 252-261) only calls `detail::store_view`. That never touches the store backend, because routers acquire mounts lazily.
    - The `Run::new` doc (105-111) and the `prepare_state` doc promise that a failing store backend fails the run up front. The baseline checked this by stat'ing the store mount.
    - Target: the probe reaches the store backend.
  - D1-8 (introduced, `d5291768` and `61c74282`): `VfsError::InvalidRange` has two producers, and the docs name one.
    - `Access::with_line_range` (handle.rs 1924-1955) checks before reading, with the reasons "start is below 1" and "end is before start".
    - The Lua store path's `resolve_line_range` (`crates/promptforge-internal/lua/src/host.rs` 322-345) checks after reading, with "start must be at least 1", "end must not be before start" and "start is required when end is given".
    - `vfs.md` line 582 documents only the first producer.
    - Target: the variant doc names both producers and both sets of reasons.
  - D1-9 (introduced, `0adf90ab`): store-view errors can name the wrong file.
    - `relativize_error` (handle.rs 1131-1185) receives canonical paths from `admit` and mount-relative paths from `StoreScoped` (2103-2108), and can't tell them apart.
    - Impact: with the store at `/my/store`, a missing `my/store/x.md` is reported as `x.md`.
    - Target: store-view errors name the path the caller supplied.
  - D1-10 (introduced as prospective debt, `5085db88`): a process-wide scope map with a full sweep on every registration.
    - `SCOPES` (handle.rs 317-336) is a process-wide `Mutex<HashMap<ExecId, Weak<Scope>>>`, swept in full on every acquire and every spawn (`remember_scope`, 323-327).
    - Impact: a fanout of N arms costs time proportional to N squared, all under one global lock.
    - The map exists only because `Vfs::acquire(&mut self, id: ExecId)` (`crates/promptforge-internal/vfs/src/traits.rs` 44, `public-api.txt` 669) passes no scope. No host implements `Vfs` today.
    - Target: the scope travels through `Vfs::acquire`, and the map is deleted.
  - C-33 (introduced, `5085db88` and `61c74282`): the docs say drop timing never matters, but it does.
    - `vfs.md` line 11, `crates/promptforge/src/effect.md` line 244, and the `perform_store` doc (`crates/harness-internal/runner/src/effect_loop-answering.rs` 58-66) all say dropping an effect's `Access` never affects correctness.
    - In fact a held store view keeps its identity, and therefore the run's scope, alive (`store_view` attaches at 1514-1517; see `Scope::release` at 232-239). So when a host answers first and drops later, a post-run read from a fresh scope conflicts.
    - `70ded04d` worked around this in `crates/promptforge-internal/engine/src/test_support/tokio_driver.rs` by dropping first, but left the docs unchanged.
    - Target: the run's scope ends when the run ends, which makes the docs true.
- Cheap fixes (listed separately from debt added):
  - D1-4 (a file touched by `61c74282`): a captured store function escapes the uncatchable-conflict rule.
    - After load, `install_store_shims` (`crates/promptforge-internal/lua/src/coro.rs` 447-459) replaces the `store` table's fields. But a function value the shared library captured, such as `local write = store.write`, is still the direct closure from `install_store_table` (`lua/src/host.rs` 408-455).
    - The engine reads the conflict slot only after `replay_shared` (`crates/promptforge-internal/engine/src/execute/section_vm.rs` 145-160).
    - Impact: a later conflict through the captured closure can be caught with `pcall`. Uncaught, it ends the run as `Store` instead of `Determinism`.
    - Contradicts `vfs.md` line 595 and the repair `61c74282` claimed.
  - D1-31 (same mechanism as D1-4): the captured direct closure also runs synchronously, outside `Effect::Store`, so it skips the host's performer and `EffectRecord::Store`. This contradicts `vfs.md` line 9 and the `install_store_shims` doc.
  - D1-5 (a file touched by `5085db88`): writes to a path and its descendant don't conflict.
    - `claim_write(P)` never checks `ancestors[P].created`, and the may-create loop checks only `paths[ancestor].reads`.
    - Impact: sibling writes to `d` and `d/f`, or a delete of `d` racing a write of `d/f`, never conflict. Which arm fails, and whether with `IsADirectory`, `NotADirectory` or `DirectoryNotEmpty`, depends on timing.
- Exposed pre-existing debt: none.
- Rejected candidates, 22 in total:
  - 9 residual-but-acceptable, because the plan accepted them as tradeoffs or they're test-only code: D1-12, D1-13, D1-14, D1-15, D1-16, D1-18, D1-23, D1-26, D1-27.
  - 9 weak or speculative, because they have no demonstrated consequence or can't be reached today: D1-17, D1-22, D1-24, D1-25, D1-28, D1-29, D1-30, D1-32, C-34.
  - 4 false, because the code already prevents them: D1-11, D1-19, D1-20, D1-21.

</product-contract>
<implementation-contract>

## Technical Design

- Public interface (D1-10):
  - `Vfs::acquire` becomes `fn acquire(&mut self, cx: &AcquireContext) -> Result<Box<dyn VfsAccess>, VfsError>`. `Vfs::release(&mut self, id: ExecId)` and `Vfs::read_only` are unchanged, and the trait stays object-safe and `Send`.
  - `AcquireContext` is a new public type in `crates/promptforge-internal/vfs/src/traits.rs`.
    - It is re-exported by the facade's `vfs` module (around line 40 of the vfs crate's `lib.rs`, and around lines 143 and 161 of `crates/promptforge/src/lib.rs`).
    - It holds the `ExecId` and an `Arc<Scope>` in private fields.
    - It implements `Clone` and `Debug`, is `Send + Sync`, has no public constructor, and exposes only `id(&self) -> ExecId`.
    - A backend that wraps another `Vfs` forwards `cx` unchanged.
  - Rewiring the acquire paths:
    - `VfsRef::acquire(&self, origin)` creates a fresh scope and context.
    - `impl Vfs for VfsRef` joins `cx`'s scope directly, registering the scope in its own tables and attaching the identity.
    - `Access::spawn` forks first, then builds the child's context, then calls the backend's `acquire`.
    - `Router::acquire` keeps a clone of `cx` on `RoutingAccess` and passes it to each mount's lazy `acquire` in `with_mount`.
    - `Access::store_view` passes the chain's context.
  - Delete `SCOPES`, `scopes()`, `remember_scope`, `forwarded_scope`, and the map lookup in `join_scope`.
  - Update every implementer:
    - The production backends: `MemoryBackend`, `HostBackend`, `Router` and `VfsRef`.
    - The test stubs in `vfs/src/handle.rs` and `vfs/src/router.rs`, `FailingBackend` in `lua/src/tests.rs`, and `GatedStore` in `engine/src/execute/tests/scheduler/store_gate.rs`.
    - The `Sealed` and `Capped` examples in `vfs.md`.
- Scope lifecycle (C-33):
  - `Scope` gains a dedicated closed flag, and `ended()` is true when that flag is set or `live` is zero. Don't force `live` to zero instead, because a later `release` would underflow.
  - A new `detail::end_scope` sets the flag.
    - The engine keeps what it needs to call it, for example a crate-internal scope handle captured where the run acquires its root identity (`h1.rs::start_live_h1` or `walk.rs`).
    - The engine calls it when `Run::step` reaches `Done` (`drive.rs` 57-68, after every pending and orphaned effect is answered), and when a live run is dropped before `Done`.
  - Once the scope is closed, `admit`, `store_view`, `spawn` and `attach` refuse before any claim or backend call. They return `VfsError::PermissionDenied`, with a reason saying the run that owned the access has ended.
  - `prune_dead_scopes` purges a closed scope's claims.
  - When a host drops an access goes back to being hygiene only. Keep the early drops in the harness and `tokio_driver.rs`, and reword the `tokio_driver.rs` comment from `70ded04d` to say so.
- Claims ledger (D1-1, D1-2, D1-5, D1-6):
  - `prune_ordered_epochs` collapses a live scope's ordered epochs on a region into one clock-0 epoch instead of dropping them, so every other live scope, including one that registers later, still conflicts with that claim. Pruning of dead scopes is unchanged.
  - `prune` removes every region whose `write`, `reads` and `created` are all empty, then sets the next prune threshold to the larger of `PRUNE_AT` and twice the surviving entry count.
  - `claim_write` runs every check first and records only after all of them pass. It adds two checks: `ancestors[P].created` against unordered epochs, and `paths[ancestor].write` in the may-create loop.
  - `rename` and `copy` check both regions under one tables lock before recording either.
- Scheduler (D1-3):
  - When a chain that holds no slots parks on a join with real members, walk up through `parent` to the nearest chain with `holding` set. Release that chain's slots, and record which chain released them.
  - On wake, re-queue through `resuming`, so those slots are taken back before the chain continues.
  - A call chain in the root walk finds no holding ancestor and releases nothing.
  - The drive stall guard also fires when `ready` and `pending` are empty and no chain in `resuming` or `spawned` can be admitted.
- Lua store functions (D1-4, D1-31):
  - Each `store` function value follows the load phase, whichever reference the prompt holds.
  - During shared library load, it runs directly and records conflicts in the conflict slot, as it does today.
  - Once `install_store_shims` has run, it yields exactly like `store.*`. Its conflicts then end the run with `Determinism`, and the host performs it as an `Effect::Store`.
  - One way to build this: `install_store_table` installs Lua dispatchers that check a phase flag, and the shim install sets the flag.
- Store probe (D1-7): a crate-internal `detail` helper stats the store root through the one-mount router, which makes the router acquire the store backend.
  - The helper treats `NotFound` for the root as success.
  - `prepare_state` uses the helper, and a backend error fails the run with `RunErrorKind::Store`.
- Store-view errors (D1-9): `StoreScoped` maps mount-relative backend paths by trimming only the leading `/`. `strip_root` stays for the canonical paths that come from `admit`.
- Docs:
  - `vfs.md`: lines 9-11, 90, 92, 94, 182, 374, 499, 557-561, 582, 595 and 847, plus the `Sealed` and `Capped` examples.
  - Store-effect docs: `effect.md` lines 150 and 244, `engine/src/execute/run-effect.rs` lines 104-109, the `perform_store` doc, `harness-internal/runner/src/performers.rs` lines 96-99, and `performers-host.rs` lines 43-45.
  - Doc comments in `vfs/src/traits.rs` (lines 15-18 and 37-44) and in `vfs/src/handle.rs` (lines 312-316, 1305-1313 and 1336-1339).
  - The `Run::new` and `prepare_state` docs stay as written once D1-7 lands.
  - Regenerate `crates/promptforge/public-api.txt` with `cargo +<pinned nightly> xtask api --bless`, using the nightly named in `crates/build-xtask/src/api/toolchain.rs`.

</implementation-contract>
<verification-contract>

## Testing Plan

Every regression test below must fail at `3088311d` and pass after its work item.

- Verification scope:
  - A step's focused and component checks cover only the packages the step touches: `cargo test -p <package>` for each touched package, narrowed with a test-name filter when the step's new tests share one, plus clippy for those packages at a component's end.
  - The workspace-wide exit checks run once, as the final step's full verification, not after every step.
- D1-10:
  - Add a test in `vfs/src/handle.rs` where a `VfsRef` directly wraps another `VfsRef` (`VfsRef::new(inner)`). The parent writes `/p` and spawns a child, and the child reads `/p` without a conflict.
  - Keep these green: `a_mounted_handle_applies_its_own_claims_under_the_callers_identity` (router.rs 747), `an_overlay_shares_the_bases_claims_table` (940), the spawn, fork and join suite in handle.rs, and `the_handle_and_capability_are_send_and_sync` (2971).
  - Add `AcquireContext` to that `Send + Sync` assertion.
- C-33:
  - An engine test drives a run to `Done` while holding every `Effect::Store` access until after `Done`. It then reads a path the run wrote through a fresh `acquire`, and gets the contents without a conflict.
  - A second test checks that an operation through a held access after `Done` fails with `PermissionDenied` and leaves the store unchanged.
  - The existing fanout regression (`fanout.rs` 169-220) stays green.
- D1-1: extend `two_live_scopes_writing_one_path_conflict` (handle.rs 2771) so that scope A writes the shared path and then makes more than 4096 other distinct claims. Scope B's write to the shared path must still get `Conflict`.
- D1-2: a unit test in handle.rs drives more than 4096 distinct paths through a scope, drops the scope, and makes one claim from a new scope. It checks that the emptied regions are gone and that the next prune threshold is set above the surviving entry count.
- D1-3:
  - Extend `call_inside_a_fanout_arm_runs_a_contained_chain` (`engine/tests/suite/exec_flow.rs` 1125) so the called section fans out under `tasks.concurrency(1)`. The run must complete.
  - Drive this test with a driver that fails on a stall or a timeout, so the test fails at `3088311d` instead of hanging.
  - Add a scheduler test that leaves a queued chain unadmittable and expects a stall report, not `Pending` with no effects.
  - Keep `a_nested_fanout_does_not_deadlock_under_a_ceiling_of_one` and `nested_tasks_join_transitively_and_do_not_deadlock_at_a_ceiling_of_one` green.
- D1-4 and D1-31: add a test next to `a_shared_library_conflict_caught_with_pcall_still_ends_the_run_with_determinism` (exec_flow.rs 2483), with a shared block that captures `store.write` in a local behind a helper.
  - Two fanout arms call the helper on one path under `pcall`, and the run must end with `Determinism`.
  - A single call to the helper after load must reach the host as an `Effect::Store`.
- D1-5: tests with two forked siblings, run in both orders, for `write('/d/f')` against `write('/d')` and for `write('/d/f')` against `remove('/d')`. Each expects `Conflict`. Use `a_glob_racing_a_siblings_write_conflicts_in_either_order` (handle.rs 2785) and `an_exists_racing_a_write_into_the_directory_conflicts` (2814) as templates.
- D1-6: mirror `a_denied_operation_never_registers_a_claim` (handle.rs 2869) for a claim refusal.
  - Scope A runs `exists('/d')`, and then scope B's `write('/d/f')` is refused. Scope C's `read('/d/f')` must not conflict.
  - Add the same check for a `rename` whose destination claim is refused. The source must be left with no claim.
- D1-7: an engine test uses a custom `Vfs` whose `acquire` refuses, declared through `VfsRefBuilder::store`. It expects the first `step` of the run from `Run::new` to be `Done`, with `RunErrorKind::Store`.
- D1-9: a test in `vfs/src/detail.rs`, with the store at `/my/store`, reads the missing `my/store/x.md` and expects `NotFound` naming `my/store/x.md`.
- D1-8: doc-only. Verify by reading `vfs.md` line 582 against both producers, and by the rustdoc build.
- Exit checks:
  - The full local check run: workspace tests including doctests, both clippy runs, `cargo fmt --all --check`, rustdoc with `RUSTDOCFLAGS="-D warnings"`, `cargo +<pinned nightly> xtask api --check`, and the user guide book build.
  - A sweep under `crates/` for stale terms: `remember_scope`, `forwarded_scope`, `SCOPES`, "process-wide", and any sentence that makes drop order a correctness rule.

</verification-contract>
<decision-record>

## Decision Record

- Architecture choices the user resolved:
  - C-33, option B: the engine ends the run's scope when the run ends, so the published host contract, that drop timing never affects correctness, stays true.
    - Rejected: documenting a drop-before-answer rule for hosts, which would put back the host obligation the store-into-VFS plan removed.
  - D1-10, option C: change the public `Vfs::acquire` so it passes the scope, and delete the process-wide map. The owner noted that no host implements `Vfs` yet, so the break costs only in-repo implementers and a facade snapshot update.
    - Rejected: sweeping less often and keeping the map, which lowers the cost but keeps the global.
    - Rejected: private `Any`-based forwarding for compositions the crate builds itself. That adds a second forwarding path and still can't delete the map.
- Run directions from the operator:
  - Keep the step count tight: six steps or fewer. Merge work items whose tests can share one run, such as the claims-ledger fixes in `handle.rs`, and fold the exit checks into the final step's full verification instead of giving them a step of their own.
  - Scope verification to the packages each step touches, as the Testing Plan's verification scope describes.
- Reversible decisions and their consequences:
  - `AcquireContext` is a working name. Renaming it before any host implements `Vfs` costs one snapshot regeneration.
  - Operations on a closed scope refuse with `PermissionDenied` rather than a new `VfsError` variant, to avoid a public enum change for a misuse path. Consequence: hosts match an existing variant and read its reason.
  - The scope ends at `Done`, not at `Scheduler::end`. That way, orphaned store effects answered during `Phase::Ending` still run inside the live scope.
  - Ordered pruning collapses a live scope's ordered epochs into one clock-0 epoch per region instead of dropping them. Consequence: a live scope keeps one claim per region it touched until it ends. Pruning of dead scopes and removal of empty regions (D1-2) still bound the ledger. Rejected: skipping ordered pruning while more than one scope is live, which misses a scope that registers after the prune.
  - D1-4 and D1-31: captured store functions go through the effect path instead of refusing to run after load, so prompts that capture `store.*` keep working.
    - Rejected: refusing after load, which breaks those prompts.
    - Rejected: checking the conflict slot after every chunk step, which leaves D1-31 open.
  - D1-5 makes conflicts stricter. A racy pair that used to fail with a timing-dependent `IsADirectory`, `NotADirectory` or `DirectoryNotEmpty` now always ends the run with `Determinism`.
  - D1-7 restores the baseline's up-front probe instead of weakening the `Run::new` doc. The probe treats a missing store root as success, so a host directory that's created lazily still runs.
  - D1-8 is doc-only. Rejected: unifying the reason strings, which would change model-facing text the guide documents.
- Assumptions and risks:
  - The analysis only read code. Nothing was built or run, and the performance impact of D1-2 and D1-10 is inferred, not measured.
  - The language guide commit (`2a49f4b2`) and the living docs commit (`de7e7ddb`) were only spot-checked, so a stale sentence there may remain. The exit sweep partly covers this.
  - C-33's fix touches scope liveness, which every claim path reads. Land it after D1-10 and before D1-1 and D1-2, so the prune changes build on the final meaning of `ended()`.
  - The serial test drivers (`serial_driver.rs` lines 71 and 198-204, and the capabilities `support.rs` lines 82-86) drop the access after resuming. After C-33 that ordering is harmless, and no test should depend on it.

### Deferred and Out of Scope

- D1-12: the store view's `grep` refuses every query, because `GrepQuery::root` is always absolute. Revisit when Lua `grep` or a public store view lands.
- D1-13: `Scope::join` merges a child's clock without advancing it. That's reachable only through the orphaned effects of a cancelled task. Revisit if cancellation outcomes must become deterministic.
- D1-14: `VfsError`'s plain struct variants freeze their field sets. The plan accepted this tradeoff.
- D1-24: pattern regions build up for a run's whole life and are recompiled on every claim. Revisit if the number of distinct globs in a run grows large.
- D1-28: scans of the task arena at chain end cost time proportional to N squared per fanout. Revisit if fanouts grow into the thousands of arms.
- D1-29: the `vfs.md` store example passes a plain `Access` where the text says "store view", which is correct only with the default store at `/`. Revisit with any edit to that store example.
- C-34: a `VfsRef` wrapped directly in another `VfsRef` splits spawned children into a new scope. The D1-10 change removes the mechanism, and the D1-10 regression test covers this case.
- The archived plan `vibe/2026-09-26-4-store-into-vfs.md`, including its Step 13, stays as a historical record.
- The remaining rejected candidates need no work: D1-11, D1-15, D1-16, D1-17, D1-18, D1-19, D1-20, D1-21, D1-22, D1-23, D1-25, D1-26, D1-27, D1-30, D1-32.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build --locked -p <package>`. Plain `cargo build` builds only the gateway, the sole default member (`crates/gateway/app`, binary `promptforge-gateway`). `cargo workshop` (the alias for `run -p build-workshop --`) is the one-command desktop build: it builds the gateway, stages the sidecar, builds the `workshop` package in the same profile and target, and removes the staged copy. A bare `cargo build -p workshop` needs a gateway already staged at `crates/workshop/desktop/binaries/promptforge-gateway-<target-triple>` (`promptforge-gateway-x86_64-pc-windows-msvc.exe` is staged now). Clippy doubles as the type check, so never run a standalone `cargo check --workspace` beside it.
- Focused test command pattern: `cargo nextest run --locked -p <package> --all-features <filter>`, where the filter is a substring of the test's module path and name (for example `-p promptforge-vfs read_returns_the_exact_bytes_stored` or `-p promptforge-engine suite::vfs`). nextest skips doctests, so use `cargo test -p <package> --all-features --doc <filter>` for those. The workshop trio (`workshop`, `workshop-server`, `workshop-server-api`) drops `--all-features`. For the UIs, run `node --test <file>` inside `crates/workshop/ui` or `crates/gateway/config-ui/ui`; the workshop UI needs `npm run build` first because its jsdom tests read `dist/`.
- Component test command pattern: `cargo nextest run --locked -p <package> --all-features`, then `cargo test -p <package> --all-features --doc`. The workshop trio runs as `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api` plus `cargo test --doc -p workshop -p workshop-server -p workshop-server-api`, with the gateway sidecar staged: CI runs `cargo build --locked -p gateway --no-default-features`, then `node tools/stage-gateway-sidecar.mjs stage --target <triple> --source target/debug/promptforge-gateway[.exe]`, and afterward `node tools/stage-gateway-sidecar.mjs remove --target <triple>`. CI adds `cargo nextest run --locked -p workshop-workspace --all-features` and `cargo nextest run --locked -p workshop-server --features headless`. For a UI package, run `npm run typecheck`, `npm run build`, then `npm test` in its directory. For a Node tool, run `node --test tools/<name>.test.mjs`.
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`. The workspace run includes `build-xtask`, the boundary and structural harness. CI also runs the headless gate `cargo check -p gateway --no-default-features`, the workshop doctests and extra partitions above, five named process-ownership race tests listed in `ci.yml` (one in `gateway-api-discovery`, four in `gateway` under `--no-default-features --features test-fixtures --test it`), and the `ui` job (`npm run typecheck`, `npm run build`, `npm test`) in both UI packages. `gateway-config-ui` and `workshop-server` bundle their UIs through `build-ui`, so each UI needs its `npm ci` install before any cargo build that includes them (both are installed now).
- Linter command: `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings`, and for the workshop trio `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`. AGENTS.md names one standalone check to run beside clippy, the headless gate `cargo check -p gateway --no-default-features`. CI's clippy job also runs `cargo rustc --locked -p <crate> --lib -- -F unsafe-code` for `gateway-stt`, `gateway-stt-engine`, and `gateway-stt-backend-whisper`, and `cargo check --locked -p gateway-whisper-ffi --lib`, all under `RUSTFLAGS=-D warnings`. `cargo test -p build-xtask` enforces the architecture rules, and `cargo xtask tidy` prints the same report. Supply chain: `cargo deny check` and `cargo audit`, both installed locally. The pre-push hook runs the headless gate, the workspace clippy, and `cargo deny check` when installed. The UIs have no linter beyond `npm run typecheck` (`tsc --noEmit`).
- Formatter check command: `cargo fmt --all --check` (rustfmt `style_edition = "2024"`), which is also the pre-commit hook. No TypeScript or CSS formatter is configured.
- Docs command: under `RUSTDOCFLAGS="-D warnings"` (in PowerShell, `$env:RUSTDOCFLAGS="-D warnings"`), run `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`, then the facade docs with default features, `cargo doc -p promptforge --no-deps` and `cargo doc -p harness --no-deps`, then `cargo doc --locked --no-deps -p workshop-server --document-private-items`. User guide: `cargo xtask site --books-only`. Facade surface: `cargo +nightly-2026-09-05 xtask api --check` (the nightly pinned in `crates/build-xtask/src/api/toolchain.rs`, installed locally), checked against the committed `crates/promptforge/public-api.txt` and regenerated with `--bless`; its nightly-only fixtures run with `cargo +nightly-2026-09-05 nextest run --locked -p build-xtask --run-ignored only`.
- Test placement and naming conventions:
  - Unit tests live inside each crate in one of three forms. Small sets sit inline as `#[cfg(test)] mod tests { ... }` at the end of the module (11 of the 13 `promptforge-vfs` modules, all but `grep.rs` and `stat.rs`; engine `error.rs`, `bindings.rs`, and `config-limits.rs`). Larger sets move to a kebab sibling `foo-tests.rs` wired with `#[cfg(test)] #[path = "foo-tests.rs"] mod tests;` (engine `run-tests.rs`, `context-tests.rs`, `fanout-tests.rs`; lua `coro-tests.rs`, `models-tests.rs`; types `event-tests.rs`). Sets of three or more files move to a `tests/` subdirectory beside a `tests.rs`, wired with `#[cfg(test)] mod tests;` (engine `src/execute/tests/` and `src/lua/tests/`, lua `src/protocol/tests/`).
  - The engine has no integration test binary; all its tests are unit tests under `src/`. Scheduler tests sit in `src/execute/tests/scheduler/` (`concurrency.rs`, `failures.rs`, `fanout.rs`, `live_h1.rs`, `walk.rs`, plus `store_gate.rs`, a gated-store fixture with no tests of its own). Suite-level tests sit in `src/execute/tests/suite/` (`vfs.rs`, `fanout.rs`, `execution.rs`, `exec_flow.rs`, `parsing.rs`, `prepare.rs`, `args_surface.rs`, `lazy_prose.rs`, and the shared `support.rs`), so their full names start with `execute::tests::suite::`.
  - Integration tests compile as one binary per crate: `tests/it/main.rs` (harness-internal runner, models, capabilities, log, and sessions; gateway `app` and `stt/api`; `gateway-api-discovery`; `build-ui`; most workshop crates) or `tests/suite/main.rs` (the `promptforge` and `harness` facades). Each `main.rs` declares one `mod` per topic file. Four of the 19 add a shared `support` module (including the `promptforge` suite and `harness-runner`), and seven open with a crate-level `#![expect(clippy::expect_used, ..., reason = "...")]` for their helpers. A few crates keep single-file targets (`gateway/stt/backend-whisper/tests/native_whisper.rs`, `gateway/stt/engine/tests/feature_boundary.rs`, `build-workshop/tests/interruption.rs`).
  - Prompt fixtures are Markdown programs under the engine's `tests/prompts/{valid,invalid,execution}/` (for example `execution/store-triad.md`, `execution/store-fallthrough.md`, and `execution/fanout-store-writes.md`) and the facade's `tests/prompts/invalid/`.
  - Test names are snake_case behavior sentences, such as `read_returns_the_exact_bytes_stored`, `write_at_a_directory_path_is_rejected_without_touching_the_tree`, and `fanout_interleaving_is_invariant_across_memory_and_host_backends`. Tests may return `Result` (the VFS tests return `Result<(), VfsError>`), and `clippy.toml` allows `unwrap` and `expect` in tests. Async tests use `#[tokio::test]`, and interleaving tests use `#[tokio::test(flavor = "multi_thread", worker_threads = 2)]`. Nightly-only `build-xtask` fixtures are `#[ignore]`d and run with `--run-ignored only`.
  - Companion crates' suites reach the engine's test drivers (`test_support::drive` and `test_support::drive_tokio`) through its `test-support` feature, enabled only from dev-dependencies; `promptforge-lua` and `promptforge-parser` expose `test-support` features the same way. Criterion benches use `harness = false`: engine `benches/models_loop.rs` (requires `test-support`) and lua `benches/surface.rs`.
  - UI tests are `node:test` files: `crates/workshop/ui/test/**/*.mjs` and `src/**/*.test.mjs`, and `crates/gateway/config-ui/ui/src/**/*.test.mjs`. Node tool tests sit beside their scripts as `tools/<name>.test.mjs`.
- Directory map:
  - `crates/` holds every Rust crate plus `shared-ui`, a TypeScript and CSS package both UIs consume (not a Rust crate). Its root is the public layer: `promptforge`, `harness`, `gateway-api-types`, `gateway-api-discovery`, `shared-error-source`, `shared-loopback`, the `build-*` tooling crates (`build-xtask`, `build-ui`, `build-user-guide`, `build-workshop`, `build-llama-cuda`), and `workspace-hack`.
  - Four manifestless family containers sit under `crates/`: `promptforge-internal/` (engine, types, vfs, lua, parser, model-client), `harness-internal/` (runner, models, capabilities, log, sessions, web, webfetch, web-search), `gateway/` (app, cloud-providers, config, config-ui, local, logging, progress, protocol, routing, web-search, and `stt/` with api, engine, backend-whisper, and whisper-ffi), and `workshop/` (desktop, server, server-api, gateway, menu, protocol, registry, status, support, user-state, workspace, and the `ui/` SPA). 29 `AGENTS.md` files under `crates/` hold crate-local and container-local rules.
  - Binaries: `promptforge-gateway` (gateway `app`), `promptforge-workshop` (workshop `desktop`), `shared-cloud-providers` (gateway `cloud-providers`), and the tooling mains `build-xtask`, `build-workshop`, `build-user-guide`, and `build-llama-cuda`.
  - `guide/` holds the user guide chapters under `src/{language,gateway,workshop}/`, plus `books/`, `chrome/`, `landing/`, `CONTRIBUTING.md`, and the single-file exports `promptforge-{language,gateway,workshop}-guide.md`, which `build-user-guide` writes and nobody hand-edits.
  - `prompts/` holds example prompt programs (`hello.md`, `greet.md`, `echo.md`, `analyst-example.md`, `research-person.md`).
  - `tools/` holds Node scripts with `node:test` suites (`stage-gateway-sidecar.mjs`, `gateway-tts-live.mjs`), `dokuman-facade.md`, and `scripts/`, a set of Python maintenance scripts (`gates.py`, `survey.py`, `facade.py`, `check_docs.py`, and others).
  - `vibe/` holds living design docs: `archdoc.md`, dated plan records named `YYYY-MM-DD-N-<slug>.md` (older ones in `2026-07/`, `2026-08/`, and `2026-09/`), reference notes, and a gitignored `scratch/`.
  - `.github/workflows/` holds CI (`ci.yml` is the gate, rolled up by the `ci-green` job) plus release, nightly, docs site, native library, installer smoke, and STT Miri workflows. `.github/fixtures/` holds the package smoke gateway config.
  - `.githooks/` holds `pre-commit` (the fmt check) and `pre-push` (the headless gate, workspace clippy, and `cargo deny check`). `.cargo/config.toml` sets rust-lld and the static CRT for `x86_64-pc-windows-msvc` and defines the `xtask` and `workshop` aliases. `.config/` holds `nextest.toml` (default and ci profiles, plus a `heavy` test group for the STT crates) and `hakari.toml` (cargo-hakari settings for `workspace-hack`). `.cursor/rules/` holds the tracked `workshop-architecture.mdc` and `workshop-spa.mdc`.
  - Root files: `Cargo.toml` (the workspace), `deny.toml`, `dist-workspace.toml` (cargo-dist, which releases only the gateway), `gateway.local.example.toml`, `rust-toolchain.toml` (stable), `rustfmt.toml`, and `clippy.toml`.
  - Gitignored and untracked: `local/` (developer gateway config, env files, profiles, prompts, and STT fixtures), `target/`, and `target-msrv/`. `images/` holds README art.
- Component boundaries:
  - PromptForge family: the `promptforge` facade depends on engine, lua, model-client, parser, types, and vfs. `promptforge-engine` depends on lua, model-client, parser, types, and vfs; `promptforge-lua` on model-client, types, and vfs; `promptforge-parser` on lua and types; and `promptforge-model-client` on types. `promptforge-types` depends on no workspace crate, and `promptforge-vfs` depends on nothing at all (std only, not even `workspace-hack`), which its manifest test enforces. Internal crates list `promptforge` only as a doctest dev-dependency. The family never depends on harness, gateway, or workshop crates.
  - The store crate is gone. The store is a VFS mount declared through `VfsRefBuilder::store`, the engine alone derives each store view from the chain's access at dispatch, and a host performing a `Store` effect uses only the view it was given. Of lua's executor-facing items, the facade re-exports only `StoreOp` and `StoreOutcome`.
  - Harness family: the `harness` facade depends on harness-log, harness-runner, and harness-sessions. `harness-runner` depends on capabilities and log; `harness-models` on runner; `harness-webfetch` and `harness-web-search` on capabilities; `harness-web` on capabilities, webfetch, and web-search; and `harness-sessions` sits on top of capabilities, log, models, runner, and web. Every `harness-internal` crate except `harness-log` also depends on `promptforge`, and `harness-log` has no normal workspace dependency. No harness crate depends on gateway, shared, or workshop crates.
  - Gateway family: the public pair is `gateway-api-types` (no workspace dependencies) and `gateway-api-discovery` (on `shared-error-source`). Inside `gateway/`, `logging` has no workspace dependencies, `config` and `progress` sit on api-types, `protocol` on api-types and config, `routing` and `web-search` on config and protocol, and `local` on config, progress, protocol, routing, and `shared-error-source`. The `stt/` subsystem shows the family only `gateway-stt` (the `stt/api` crate), which sits on config, local, progress, the stt engine, and backend-whisper; backend-whisper wraps whisper-ffi. `config-ui` uses `build-ui` and `shared-loopback`. `app` (package `gateway`) composes the rest of the family except `cloud-providers`, which no workspace crate depends on. No gateway crate depends on promptforge, harness, or workshop crates.
  - Workshop family: `workshop` (desktop) depends only on `workshop-server-api` and `gateway-api-discovery`, and `workshop-server-api` wraps `workshop-server`. `workshop-server` depends on `harness`, `promptforge`, `gateway-api-discovery`, `shared-loopback`, `build-ui`, and eight siblings (gateway, menu, protocol, registry, status, support, user-state, workspace). `workshop-protocol` and `workshop-support` have no workspace dependencies, `workshop-registry` sits on protocol, and status, menu, user-state, workspace, and gateway sit on protocol, registry, and support. `workshop-gateway` is the only workshop crate that names `gateway-api-types`. Tiers flow one way: server, then features, then services, then vocabulary.
  - Cross-family rules: a crate in a family container depends only on crates at the `crates/` root and its own siblings, `shared-*` crates depend on no product crate, and `build-*` crates are tooling, exempt from container privacy. `cargo test -p build-xtask` enforces the topology, the single-public-crate facades, and the `## Invariants` markers.
  - `vibe/archdoc.md` differs from the tree in two places: it lists a CLI component, but no CLI crate or binary exists, and it says the Lua boundary depends only on the shared substrate, while its manifest names model-client, types, and vfs.
- Conventions summary:
  - The workspace uses the Rust 2024 edition, resolver 3, the stable toolchain, and rustfmt `style_edition = "2024"`. Workspace lints forbid `unsafe_code`; warn on `missing_docs`, `missing_debug_implementations`, and `unreachable_pub`; deny `unsafe_op_in_unsafe_fn`, clippy `all` and `pedantic`, `unwrap_used`, and `expect_used`; and deny broken and private intra-doc links. The four crates that contain unsafe code (gateway `app`, `gateway-whisper-ffi`, `gateway-api-discovery`, and workshop `desktop`) keep local lint tables with `unsafe_code = "deny"` and pedantic at warn. Every other member sets `[lints] workspace = true`.
  - Dependencies inherit from `[workspace.dependencies]`, with a comment justifying each pin or feature choice. Every member except `promptforge-vfs` depends on `workspace-hack`. `.config/hakari.toml` says CI runs `cargo hakari verify`, but no workflow does.
  - Facades (`promptforge`, `harness`) are single-item re-exports grouped into documented role modules. The committed `crates/promptforge/public-api.txt` pins the `promptforge` surface, and no surface doc text names an internal crate.
  - The lib.rs of every `workshop-*` and `harness-*` crate opens with a `//!` doc holding a `## Invariants` marker, and files in marked crates stay at or under 500 lines (split first, then edit).
  - Source directories are flat unless a subdirectory holds three or more files; smaller groups become kebab siblings `foo-bar.rs` wired with `#[path = "foo-bar.rs"] mod bar;`.
  - Error and status messages are written for model readers: concise and self-contained, naming what is missing and giving required versus actual.
  - Behavior changes ship with tests in the same change. New structural checks (source parsers, snapshots, allowlists, counts, ceilings, topology checks) need explicit owner approval, even inside a plan.
  - Comments explain only non-obvious constraints, workarounds cite an upstream issue URL, and every unsafe block documents its safety invariants right before the block.
  - Run-log JSON round-trips exactly: sorted keys, `float_roundtrip`, finite numbers, and never `preserve_order`.
  - Cargo features gate only real constraints such as toolchain requirements and heavy native builds, never product shape, and feature-disabled builds must not leak optional types into core paths. Runtime and serve paths never compile native dependencies or invoke build tools, and library and serve paths return failures instead of exiting or installing process-global state.
  - The VFS public surface is load-bearing: add defaulted methods and never change existing signatures. Origin labels are most-specific (a section name for a chain, a tool id for a tool, a fixture name for a test). The VFS machinery modules hold no run concepts, the declared store lives with the handle, and the mode gate stays at the crate root.
  - Lua host capabilities are namespace functions over plain values with frozen, methodless handles. New operations take an optional leading handle argument instead of colon methods, and `messages.new()` builders are the one exception. Nothing in `promptforge-lua` performs a tool call: the executor issues effects, and the host performs them.
  - SPA rules: CSS sits beside its TypeScript, component CSS uses only `--ws-*` tokens, and the UI never touches `localStorage`; persisted state goes through the server's `ui-storage` adapter.

</project-survey>
<execution-plan>

## Execution Instructions

<step-1>

### Step 1: Pass the scope through `Vfs::acquire` (D1-10) [completed]

- Component: Scope lifecycle

- Placement: first of four components. Every later VFS step edits the acquire, spawn and store-view paths this step rewires, the Step 4 test stub needs the new `acquire` signature, and the plan asks for D1-10 to land first.
- Pieces: the scope-passing acquire (this step), then the run-bounded scope (Step 2). They're built in sequence because Step 2 closes the scope this step passes through `AcquireContext`, and keeping the public API break apart from the behavior change keeps each commit reviewable and bisectable.
- Depends on: nothing.
- Interface, in `crates/promptforge-internal/vfs/src/traits.rs`:
  - Add the public `AcquireContext`, holding the `ExecId` and an `Arc<Scope>` in private fields (`Scope` becomes `pub(crate)`). It implements `Clone` and `Debug`, is `Send + Sync`, has no public constructor, and exposes only `id(&self) -> ExecId`. Its crate-internal constructor also serves the crate's own tests that call a backend's `acquire(ExecId::vend())` directly, in `memory.rs`, `host.rs` and `router.rs`.
  - Change `Vfs::acquire` to `fn acquire(&mut self, cx: &AcquireContext) -> Result<Box<dyn VfsAccess>, VfsError>`. `Vfs::release(&mut self, id: ExecId)` and `Vfs::read_only` don't change, and the trait stays object-safe and `Send`.
  - Re-export `AcquireContext` from the vfs crate's `lib.rs` (around line 40) and from the facade's `vfs` module in `crates/promptforge/src/lib.rs` (around lines 143 and 161).
- Acquire paths, in `crates/promptforge-internal/vfs/src/handle.rs` and `router.rs`:
  - `VfsRef::acquire(&self, origin)` creates a fresh scope and its context.
  - `impl Vfs for VfsRef`, through `VfsRef::acquire_with`, joins the scope in `cx` directly: it registers the scope in its own claims tables and attaches the identity.
  - `Access::spawn` forks first, then builds the child's context, then calls the backend's `acquire`. When that acquire refuses, release the child's reference so the scope's live count stays exact. The parent's entry has already advanced by then, which is harmless, so reword the spawn doc's "A failed spawn leaves this capability's clock untouched".
  - `Router::acquire` keeps a clone of `cx` on `RoutingAccess` and passes it to each mount's lazy `acquire` in `with_mount`.
  - `Access::store_view` passes the chain's context.
  - Delete `SCOPES`, `scopes()`, `remember_scope`, `forwarded_scope`, and the map lookup in `join_scope`.
- Implementers: update `MemoryBackend` (`vfs/src/memory.rs`), `HostBackend` (`vfs/src/host.rs`), `Router` and `VfsRef`; the test stubs `StubFs`, `RefusingFs` and `RefuseSecond` in `handle.rs` and `StubFs` in `router.rs`; `FailingBackend` in `crates/promptforge-internal/lua/src/tests.rs`; and `GatedStore` in `crates/promptforge-internal/engine/src/execute/tests/scheduler/store_gate.rs`. A backend that wraps another `Vfs` forwards `cx` unchanged.
- Docs: `crates/promptforge/src/vfs.md` lines 182, 374, 499, 557-561 and 847, plus the `Sealed` and `Capped` examples; the doc comments at `vfs/src/traits.rs` lines 15-18 and 37-44 and at `vfs/src/handle.rs` lines 312-316, 1305-1313 and 1336-1339. No doc may say a backend receives a bare `ExecId` from `Vfs::acquire`.
- Snapshot: regenerate `crates/promptforge/public-api.txt` with `cargo +nightly-2026-09-05 xtask api --bless`, the nightly pinned in `crates/build-xtask/src/api/toolchain.rs`.
- Tests:
  - In `handle.rs`, a `VfsRef` that directly wraps another (`VfsRef::new(inner)`): the parent writes `/p` and spawns a child, and the child reads `/p` without a conflict. It fails before this step, and it also covers deferred item C-34.
  - Add `AcquireContext` to the `Send + Sync` assertion in `the_handle_and_capability_are_send_and_sync`.
  - Keep green: `a_mounted_handle_applies_its_own_claims_under_the_callers_identity` and `an_overlay_shares_the_bases_claims_table` in `router.rs`, and the spawn, fork and join suite in `handle.rs`.
- Verification:
  - `cargo nextest run --locked -p promptforge-vfs -p promptforge-lua -p promptforge-engine -p promptforge --all-features`
  - `cargo test -p promptforge-vfs -p promptforge --all-features --doc`, which runs the `Sealed` and `Capped` examples.
  - `cargo +nightly-2026-09-05 xtask api --check`
  - A search under `crates/` for `remember_scope`, `forwarded_scope` and `SCOPES` finds nothing.
- Commit: one commit with the code, tests, docs and snapshot, for example "Pass the scope through Vfs::acquire".

</step-1>

<step-2>

### Step 2: End the run's scope when the run ends (C-33) [completed]

- Component: Scope lifecycle

- Piece: the run-bounded scope, second in sequence after Step 1, because it closes the scope Step 1 passes through `AcquireContext`. It lands before Step 3 so the pruning changes build on the final meaning of `Scope::ended()`.
- Depends on: Step 1.
- Scope, in `crates/promptforge-internal/vfs/src/handle.rs`:
  - `Scope` gains a dedicated closed flag, and `Scope::ended()` is true when the flag is set or `live` is zero. Don't force `live` to zero instead, because a later `Scope::release` would underflow.
  - Once the flag is set, `Access::admit`, `Access::store_view`, `Access::spawn` and the scope join in `impl Vfs for VfsRef` (`Scope::attach`) refuse before any claim or backend call. They return `VfsError::PermissionDenied` with a reason saying the run that owned the access has ended. No new `VfsError` variant.
  - `prune_dead_scopes` purges a closed scope's claims.
- `crates/promptforge-internal/vfs/src/detail.rs`: add `detail::end_scope`, which sets the flag, and an opaque handle type there that the engine can hold. The facade re-exports neither.
- Engine, under `crates/promptforge-internal/engine/src/execute/scheduler/`:
  - Take the handle where the run acquires its root identity: `h1.rs::start_live_h1` and `walk.rs::install_root_slots`.
  - Call `detail::end_scope` when `Run::step` reaches `Done` in `drive.rs` (lines 57-68, after every pending and orphaned effect is answered), not at `Scheduler::end`, so orphaned store effects answered during `Phase::Ending` still run inside the live scope.
  - Also call it when a live run is dropped before `Done`. Do that from a crate-internal type such as `Scheduler`, not a new `Drop` impl on `Run`, because `public-api.txt` lists `Drop` impls and this step changes no facade surface.
- Drop-timing doc pass, so no sentence makes drop order a correctness rule: `crates/promptforge/src/vfs.md` lines 9-11, 90 and 94; `crates/promptforge/src/effect.md` lines 150 and 244; `engine/src/execute/run-effect.rs` lines 104-109; the `perform_store` doc in `crates/harness-internal/runner/src/effect_loop-answering.rs` (lines 58-66); `harness-internal/runner/src/performers.rs` lines 96-99; and `performers-host.rs` lines 43-45.
- Host drops: keep the early drops in the harness and in `engine/src/test_support/tokio_driver.rs`, and reword the comment from `70ded04d` there (around line 414) to say the drop is hygiene only. The serial drivers that drop after resuming (`engine/src/execute/tests/serial_driver.rs` lines 71 and 198-204, and `crates/harness-internal/capabilities/tests/it/support.rs` lines 82-86) stay as they are, and no test may depend on that order.
- Tests, in `engine/src/execute/tests/suite/vfs.rs`, each failing before this step:
  - Drive a run to `Done` while holding every `Effect::Store` access until after `Done`, for example with a loop over `Run::step` and `Run::resume` that answers through `perform_locally` from `serial_driver.rs` and keeps each effect. Then read a path the run wrote through a fresh `acquire`, and get the contents without a conflict.
  - An operation through a held access after `Done` fails with `PermissionDenied` and leaves the store unchanged.
  - Keep green: `fanout_arms_take_child_ids_in_collection_order_per_fanout_index_and_structured_results` (`engine/src/execute/tests/scheduler/fanout.rs` lines 169-220).
- Verification:
  - `cargo nextest run --locked -p promptforge-vfs -p promptforge-engine -p promptforge -p harness-runner --all-features`
  - `cargo test -p promptforge-vfs -p promptforge-engine -p promptforge -p harness-runner --all-features --doc`
  - `cargo +nightly-2026-09-05 xtask api --check` passes without a re-bless.
  - Component end: `cargo clippy -p promptforge-vfs -p promptforge-lua -p promptforge-engine -p promptforge -p harness-runner --all-targets --all-features -- -D warnings`
- Commit: one commit with the code, tests and docs, for example "End the run's scope when the run ends".

</step-2>

<step-3>

### Step 3: Make the claims ledger exact (D1-1, D1-2, D1-5, D1-6)

- Component: Claims ledger

- Placement: second of four components, because pruning reads the final meaning of `Scope::ended()` from Step 2, and every claim path goes through the acquire rewiring from Step 1.
- Pieces: pruning (D1-1, D1-2) and all-or-nothing claims (D1-5, D1-6), built together in this one step. Neither needs the other, but both edit `Claims` and `ClaimsTables` in `crates/promptforge-internal/vfs/src/handle.rs`, so they can't run in parallel, and their tests share one `promptforge-vfs` run, as the operator directs.
- Depends on: Step 2.
- Pruning:
  - `prune_ordered_epochs` never drops a live scope's last claim on a region. It collapses that scope's ordered epochs on the region into one clock-0 epoch, keeping the most recent identity for conflict messages. Every identity of the scope has seen a clock-0 epoch, so it never conflicts inside the scope, and every other live scope conflicts with it, including a scope that registers after the prune. That meets the D1-1 target: an epoch another live scope could conflict with is never pruned. Pruning of dead scopes is unchanged.
  - Don't also skip ordered pruning while more than one scope is live. That rule misses a scope that registers after the prune (`two_live_scopes_writing_one_path_conflict` acquires scope B after A's write), and the collapse makes it redundant.
  - `prune` removes every region in `paths`, `children`, `patterns`, `subtrees` and `ancestors` whose `write`, `reads` and `created` are all empty. It then sets the next prune threshold, a new `ClaimsTables` field beside `entries`, to the larger of `PRUNE_AT` and twice the surviving entry count.
- Claims:
  - `Claims::claim_write` runs every check first and records only after all of them pass. It adds two checks: `ancestors[P].created` against unordered epochs, and `paths[ancestor].write` in the may-create loop.
  - `Access::rename` and `Access::copy` check both regions under one tables lock before recording either.
- Docs: reword `crates/promptforge/src/vfs.md` line 92 wherever "Claims are never released during a scope's life" no longer matches pruning, and confirm lines 90 and 499 now hold.
- Tests, all in `handle.rs`, each failing before this step:
  - D1-1: extend `two_live_scopes_writing_one_path_conflict` so scope A writes the shared path and then makes more than 4096 other distinct claims. Scope B's write to the shared path still gets `Conflict`, both when B is acquired before A's extra claims and when it's acquired after them.
  - D1-2: drive more than 4096 distinct paths through a scope, drop the scope, and make one claim from a new scope, with the counts arranged so that claim triggers a prune. Check that the emptied regions are gone and that the next prune threshold sits above the surviving entry count.
  - D1-5: two forked siblings, run in both orders, for `write('/d/f')` against `write('/d')` and for `write('/d/f')` against `remove('/d')`. Each expects `Conflict`. Use `a_glob_racing_a_siblings_write_conflicts_in_either_order` and `an_exists_racing_a_write_into_the_directory_conflicts` as templates.
  - D1-6: mirror `a_denied_operation_never_registers_a_claim`. Scope A runs `exists('/d')`, then scope B's `write('/d/f')` is refused, and scope C's `read('/d/f')` doesn't conflict. Add the same check for a `rename` whose destination claim is refused: the source is left with no claim.
- Verification:
  - `cargo nextest run --locked -p promptforge-vfs --all-features`
  - `cargo test -p promptforge-vfs -p promptforge --all-features --doc`
  - Component end: `cargo clippy -p promptforge-vfs -p promptforge --all-targets --all-features -- -D warnings`
- Commit: one commit with the code, tests and doc, for example "Keep claims exact across pruning and refusals".

</step-3>

<step-4>

### Step 4: Make the store probe and store errors correct (D1-7, D1-8, D1-9)

- Component: Store view

- Placement: third of four components. It needs only Step 1, because the D1-7 test stub uses the new `acquire` signature, but it comes after Step 3 because the D1-9 fix edits `crates/promptforge-internal/vfs/src/handle.rs`, which Step 3 rewrites.
- Pieces: the store probe (D1-7) and store error reporting (D1-8, D1-9), built together in this one step. Neither needs the other, both are small, both touch `crates/promptforge-internal/vfs/src/detail.rs`, and their tests share one `promptforge-vfs` and `promptforge-engine` run.
- Depends on: Step 1, and Step 3 for `handle.rs`.
- Store probe (D1-7):
  - Add a crate-internal helper in `detail.rs` that stats the store root through the one-mount router, which makes the router acquire the store backend. It treats `NotFound` for the root as success, so a host directory that's created lazily still runs.
  - `prepare_state` in `crates/promptforge-internal/engine/src/execute/run.rs` (lines 252-261) calls the helper instead of the bare `detail::store_view`, and a backend error fails the run with `RunErrorKind::Store`.
  - The `Run::new` and `prepare_state` docs stay as written.
- Store-view errors (D1-9): `StoreScoped` in `handle.rs` (around lines 2097-2108) maps mount-relative backend paths by trimming only the leading `/`. `strip_root` stays for the canonical paths that reach `relativize_error` from `Access::admit`.
- `InvalidRange` doc (D1-8): `crates/promptforge/src/vfs.md` line 582 names both producers and both sets of reasons. `Access::with_line_range` in `handle.rs` checks before reading, with "start is below 1" and "end is before start". The Lua store path's `resolve_line_range` in `crates/promptforge-internal/lua/src/host.rs` checks after reading, with "start must be at least 1", "end must not be before start" and "start is required when end is given". Don't unify the reason strings.
- Tests, each failing before this step:
  - D1-7: next to `a_handle_without_a_declared_store_fails_the_run` in `crates/promptforge-internal/engine/src/execute/tests/suite/exec_flow.rs`, a custom `Vfs` whose `acquire` refuses, declared through `VfsRefBuilder::store`. The first `step` of the run from `Run::new` is `Done`, with `RunErrorKind::Store`.
  - D1-9: next to `errors_report_paths_in_the_callers_relative_form` in `detail.rs`, with the store at `/my/store`, reading the missing `my/store/x.md` gives `NotFound` naming `my/store/x.md`.
  - D1-8 is doc-only: read line 582 against both producers.
- Verification:
  - `cargo nextest run --locked -p promptforge-vfs -p promptforge-engine --all-features`
  - `cargo test -p promptforge-vfs -p promptforge-engine -p promptforge --all-features --doc`
  - With `$env:RUSTDOCFLAGS="-D warnings"`, `cargo doc -p promptforge --no-deps` builds.
  - Component end: `cargo clippy -p promptforge-vfs -p promptforge-engine -p promptforge --all-targets --all-features -- -D warnings`
- Commit: one commit with the code, tests and doc, for example "Probe the store backend and report store paths as given".

</step-4>

<step-5>

### Step 5: Unblock call-chain admission and route captured store functions through effects (D1-3, D1-4, D1-31), then run the exit checks

- Component: Executor and Lua VM boundary

- Placement: last of four components. It needs nothing from the VFS components, so it follows the dependency chain that starts at Step 1, and as the final step it also runs the workspace-wide exit checks.
- Pieces: call-chain admission (D1-3) and load-phase store functions (D1-4, D1-31), built together in this one step. Neither needs the other, and their regression tests share one `promptforge-engine` run in `exec_flow.rs`.
- Depends on: nothing for the fixes, and Steps 1-4 for the exit checks.
- Call-chain admission (D1-3), under `crates/promptforge-internal/engine/src/execute/scheduler/`:
  - In `tasks.rs`, when a chain that holds no slots parks on a join with real members (`park_wait`, lines 710-722, reached from `join_any` and `await_tasks`), walk up through `parent` to the nearest chain with `holding` set, release that chain's slots, and record which chain released them.
  - On wake, re-queue through `resuming`, so those slots are taken back before the chain continues. A call chain in the root walk finds no holding ancestor and releases nothing.
  - In `drive.rs`, the stall guard (lines 44-53) also fires when `ready` and `pending` are empty and no chain in `resuming` or `spawned` can be admitted. Update the module doc to match.
- Load-phase store functions (D1-4, D1-31), in `crates/promptforge-internal/lua/src/host.rs` and `lua/src/coro.rs`:
  - Each `store` function value follows the load phase, whichever reference the prompt holds. During shared library load it runs directly and records conflicts in the conflict slot, as it does today.
  - Once `install_store_shims` has run, it yields exactly like `store.*`, so its conflicts end the run with `Determinism` and the host performs it as an `Effect::Store`.
  - One way to build this: `install_store_table` installs Lua dispatchers that check a phase flag, and `install_store_shims` sets the flag.
  - The Lua `store` and `tasks` surfaces don't change. Update the `install_store_shims` doc, and confirm `crates/promptforge/src/vfs.md` lines 9 and 595 now hold.
- Tests, each failing before this step:
  - D1-3: extend `call_inside_a_fanout_arm_runs_a_contained_chain` (`crates/promptforge-internal/engine/src/execute/tests/suite/exec_flow.rs` line 1125) so the called section fans out under `tasks.concurrency(1)`. The run completes. Drive it so a stall fails the test instead of hanging it: bound it with `tokio::time::timeout`, under a driver that errors on a `Pending` step with nothing outstanding, as the tokio test driver in `engine/src/test_support/tokio_driver.rs` does.
  - D1-3: a scheduler test in `engine/src/execute/tests/scheduler/concurrency.rs` leaves a queued chain unadmittable and expects a stall report, not `Pending` with no effects.
  - D1-4 and D1-31: next to `a_shared_library_conflict_caught_with_pcall_still_ends_the_run_with_determinism` (`exec_flow.rs` line 2483), a shared block captures `store.write` in a local behind a helper. Two fanout arms call the helper on one path under `pcall`, and the run ends with `Determinism`. A single call to the helper after load reaches the host as an `Effect::Store`.
  - Keep green: `a_nested_fanout_does_not_deadlock_under_a_ceiling_of_one` (`engine/src/execute/tests/scheduler/concurrency.rs`) and `nested_tasks_join_transitively_and_do_not_deadlock_at_a_ceiling_of_one` (`engine/src/execute/tests/happens_before.rs`).
- Verification:
  - `cargo nextest run --locked -p promptforge-engine -p promptforge-lua --all-features`
  - `cargo test -p promptforge-engine -p promptforge-lua --all-features --doc`
  - Component end: `cargo clippy -p promptforge-engine -p promptforge-lua --all-targets --all-features -- -D warnings`
- Exit checks, run once here as the plan's full verification:
  - Tests: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api` and `cargo test --doc -p workshop -p workshop-server -p workshop-server-api`, with the gateway sidecar staged as the Project Survey describes.
  - Clippy: `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings` and `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`.
  - Format: `cargo fmt --all --check`.
  - Rustdoc, with `$env:RUSTDOCFLAGS="-D warnings"`: `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`, `cargo doc -p promptforge --no-deps`, `cargo doc -p harness --no-deps`, and `cargo doc --locked --no-deps -p workshop-server --document-private-items`.
  - Facade surface: `cargo +nightly-2026-09-05 xtask api --check`.
  - User guide: `cargo xtask site --books-only`.
  - Stale-term sweep under `crates/`: `remember_scope`, `forwarded_scope` and `SCOPES` appear nowhere, "process-wide" names no scope map (the `ExecId` counter's use is fine), no doc says a backend receives a bare `ExecId` from `Vfs::acquire`, and no sentence makes drop order a correctness rule.
- Commit: one commit with the code, tests and docs, for example "Release a waiting call chain's slots and route captured store functions through effects".

</step-5>

</execution-plan>
