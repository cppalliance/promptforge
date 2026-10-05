---
name: promptforge debt removal
overview: Debt Collector review of the 13 promptforge commits on master over upstream/master found no debt added. At the user's request the plan fixes three items anyway - end each tool call's VFS identity when its call ends (D1-2, D1-3, D1-4), redact the Workshop gateway key in Debug output (D1-16), and fix one stale test-helper comment (D1-13).
todos:
  - id: end-tool-identity
    content: "D1-2/3/4: add a VFS primitive that ends a tool call's identity (refuse, join, drop snapshot), call it from apply_answer and abort_effect, with regression tests"
    status: pending
  - id: redact-gateway-config
    content: "D1-16: hand-written redacting Debug on workshop_support::GatewayConfig plus a regression test"
    status: pending
  - id: fix-web-tests-comment
    content: "D1-13: reword crates/harness-web/src/web-tests.rs lines 29-30 to drop the VFS claim"
    status: pending
  - id: run-gates
    content: Run the full root AGENTS.md gates (nextest, clippy, fmt, docs, xtask api --check, build-xtask)
    status: pending
isProject: false
---

# Debt removal plan: promptforge master over upstream/master

<product-contract>

## Product Requirements

- **Scope and target work**
  - Repository `c:\Users\Vinnie\cursor\promptforge`, branch `master`.
  - Baseline `fd7737394` (merge base with `upstream/master`, equal to its tip). Endpoint `8f36fb553`. Disposition is the clean worktree, identical to the endpoint.
  - 13 target commits:
    - Supply chain: `e45d37ef3` (quiet cargo-deny in the pre-push hook), `c8b131725` (replace yanked `chacha20` 0.10.1, deny yanked crates)
    - Docs and rulebooks: `d8b11bce9` (move archdoc invariants into crate docs), `b8e7ae4c8` (remove stale rulebook claims, redact a gateway key)
    - Plan `vibe/2026-10-04-1-tool-context.md`: `34270d278` (per-call forked filesystem access), `bd46b0766` (lend each tool its call's access and origin through `ToolContext`), `9024ec50f` (remove `RunServices::vfs`), `b5efc123c` (close)
    - Plan `vibe/2026-10-04-2-plugin-rename.md`: `77b210b2a` (rename the capability activation unit to Plugin, frontmatter `capabilities:` to `plugins:`), `f9b35597d` (define Plugin, sweep comments, widen the vocabulary guard), `aed5a207d` (design records), `f0429fe3b` (close)
    - `8f36fb553` (rewrite stale comments, drop unused `external_closure`)
- **Cleanup goals and non-goals**
  - Review found no introduced or worsened debt and no cheap fixes under the Debt Collector's rules. The user chose to fix three items anyway:
    - D1-2, D1-3, D1-4: a tool call's VFS identity outlives its call.
    - D1-16: the Workshop gateway bearer key prints through a derived `Debug`.
    - D1-13: one stale comment left by `9024ec50f`.
  - Non-goals: the rest of D1-1 (prose drift beyond that one comment), task identities (they can be joined more than once and keep today's behavior), any public API change, and plan or architecture record updates.
- **Success criteria**
  - Once a tool call is answered or aborted, every operation through its access is refused with `VfsError::PermissionDenied`, and its clock snapshot is gone.
  - Cancelling a task with a tool call in flight never ends the run with a store conflict caused by that tool's writes.
  - `{:?}` formatting of `workshop_support::Config` or `GatewayConfig` never contains the API key.
  - Each new regression test fails on `8f36fb553` and passes after its fix. The root `AGENTS.md` gates (lines 52-58) pass, and `crates/promptforge/public-api.txt` is unchanged.

## Functional Specification

### Debt Inventory

- **Debt added: none.** Zero introduced, zero worsened.
  - Every hard-to-reverse `Design:` label in the target was tested as prospective debt and failed the bar:
    - The doc-only "must not use it after answering" rule on `Effect::ToolCall::access` (`34270d278`) fails closed under the VFS claims model, and the one in-repo performer already conforms.
    - The frontmatter rename `capabilities:` to `plugins:` (`77b210b2a`) is a recorded owner decision and fails loudly with a serde error naming the accepted keys.
    - The Workshop `ContractResponse` field rename changed server and UI in the same commit.
    - The new `ToolContext` surface has private fields and no `Clone`, and the borrow it lends cannot be turned into an owned `Access`.
    - Removing `RunServices::vfs` (`9024ec50f`) left no consumer.
- **Cheap fixes: none** under the rules. D1-13 is a wrong comment, which is neither incorrect behavior nor dead code. It is fixed below by user choice.
- **Fixed by user choice (rejected or out of scope under the rules)**
  - D1-2, D1-3, D1-4, a tool call's identity outlives its call. `34270d278` forks a VFS identity per bound tool call (`crates/promptforge-internal/engine/src/execute/scheduler/tool_call.rs` lines 189-224) and joins it only in `apply_answer` (`scheduler/apply.rs` lines 76-80). Three symptoms share that root:
    - D1-2: `abort_effect` (`scheduler/chain.rs` lines 280-288) discards the pending entry and never joins or stops the call's identity. An in-flight tool keeps writing after `tasks.cancel` (`scheduler/waits.rs` around line 343) or after its owner returns with live tasks (`scheduler/task_end.rs` around line 188). If the tool writes a path before the owner touches it, the owner's operation ends the run with a fatal store conflict (`apply.rs` lines 96-102); if the owner gets there first, nothing fails. The outcome depends on timing.
    - D1-3: a fork shares the parent's `seen` map (`crates/promptforge-internal/vfs/src/handle/scope.rs` line 199), a join clones the owner's map through `Arc::make_mut` (line 244), and identity records are never removed (lines 39-43). Each finished call keeps its fork-time snapshot, so N sequential tool calls in one chain retain about N squared over 2 entries until the run ends.
    - D1-4: `Effect::ToolCall::access` is a public `Arc<Access>` whose "must not use it after answering" rule (`engine/src/execute/run/effect.rs` lines 115-119) only the doc states. The stock Harness performer already drops it before the answer is applied (`crates/harness-internal/runner/src/performers-tools.rs` lines 47-71, `effect_loop.rs` lines 340-351).
  - D1-16, `GatewayConfig` (`crates/workshop/support/src/config.rs` lines 115-123) and its container `Config` (line 23) derive `Debug` over `pub api_key: String`. Every other holder of the key redacts it, for example `ResolvedGateway` (`crates/workshop/gateway/src/resolve.rs` lines 31-41) and `SecretString` in `crates/harness-gateway-client/src/config.rs`. `b8e7ae4c8` redacted one holder and missed this one. Nothing formats it today.
  - D1-13, `crates/harness-web/src/web-tests.rs` lines 29-30 say "Run services over a default VFS and `cancel`" above a function that ends in `RunServices::with_host(cancel, host)` and builds no VFS.
- **Exposed pre-existing debt (reported, not remediated): D1-1.** Prose that states code facts drifts from the code, and no gate reads most of it, so stale-doc sweeps recur.
  - Evidence: at least nine earlier correction commits from `d77a23e86` (2026-08-03) through `cb2e88d57` (2026-10-03), plus two inside the target (`b8e7ae4c8`, `8f36fb553`) separated by two unrelated plans. `d8b11bce9` moved a false gateway invariant into `crates/gateway/app/src/lib.rs`, and `b8e7ae4c8` deleted it one commit later.
  - Existing protection is partial: `crates/workshop/ui/test/docs-claims.mjs` checks rulebooks only; `rustdoc::broken_intra_doc_links = "deny"` (root `Cargo.toml` line 268) covers linked items only; the `build-xtask` retired-symbol scan strips comments before matching.
  - Drift is mostly semantic: of 112 backticked identifiers in the prose `8f36fb553` removed, only 7 no longer exist anywhere in `crates/`.
- **Rejected candidates: 21**, of which D1-2, D1-3, D1-4, D1-13, and D1-16 are now fixed by user choice.
  - 8 residual-but-acceptable (D1-2, D1-3, D1-4, D1-5, D1-6, D1-8, D1-9, D1-13): explicitly deferred, fail closed, owner decisions, changed in lockstep, test-only, or comment-only.
  - 2 weak/speculative (D1-7, D1-16): size-only leads, and a derived `Debug` over the gateway key with no code path that prints it.
  - 7 false (D1-10, D1-11, D1-15, D1-19, D1-20, D1-21, D1-22): verified equivalent behavior, clean removals, or premises that hold.
  - 4 unrelated pre-existing (D1-12, D1-14, D1-17, D1-18): the target only renamed the code or deleted docs that were already false.

</product-contract>
<implementation-contract>

## Technical Design

- **VFS: end a single identity (D1-2, D1-3, D1-4)** in `promptforge-vfs`
  - Add `ended: bool` to `Identity` (`crates/promptforge-internal/vfs/src/handle/scope.rs` lines 47-64), false at fork.
  - Add `Scope::end(&self, owner: Option<ExecId>, child: ExecId)`. Under one lock acquisition it marks `child` ended, merges `child`'s clock into `owner` exactly as `Scope::join` does (lines 230-247) when `owner` is `Some`, then replaces `child`'s `seen` with an empty map. Keep `own` and `forked`, which describe what the child already did. Factor the merge out of `join` so both share it.
  - Refuse an ended identity beside every `refuse_if_closed` call that acts for an identity: `Access::admit` (`handle/access.rs` lines 104-106), `Access::spawn` (lines 37-39), and `Access::store_view` (`handle/store_view.rs` lines 179-181). Return `VfsError::PermissionDenied` with a reason saying the tool call that owned this access has ended. Check the flag in the same lock acquisition that admits the operation, so nothing is admitted after `end`.
  - Expose it to the Engine as `detail::end_access(scope: &ScopeHandle, owner: Option<ExecId>, child: ExecId)` in `crates/promptforge-internal/vfs/src/detail.rs`, beside `access_join` (line 100) and `end_scope` (line 42).
  - Task identities are unchanged. They can be joined more than once (`join_task` on delivery and notices, then `join_owned_tasks` at chain end and teardown, `scheduler/task_end.rs` lines 115-137), so dropping their snapshot after one join would turn later joins into no-ops.
- **Engine: call it at both ends of a tool call** in `promptforge-engine`
  - `apply_answer` (`scheduler/apply.rs` lines 70-80): replace the `access_join(access, call.exec)` branch with `end_access(scope, owner, call.exec)`, where `owner` is the parked chain's `access_id` when the chain still has an access. This runs for every answer, including `Dropped`.
  - `abort_effect` (`scheduler/chain.rs` lines 280-288): keep the removed `Pending { chain, resume }`; when `resume` is `Continuation::ToolCall(call)`, call `end_access` the same way before recording the id as orphaned. What the tool already wrote is joined into the parked chain, as a `Dropped` answer is today, and anything it tries later is refused. The orphan early return in `Scheduler::resume` (`scheduler/drive.rs` lines 95-97) stays.
  - Take the scope from the scheduler's `scope: Option<ScopeHandle>` (`execute/scheduler.rs` line 279). When it is `None` the run's scope has closed and every access is already refused, so skip the call.
  - `Effect::ToolCall` keeps `pub access: Arc<Access>`. No facade change.
- **Docs that become false, rewritten in the same change**
  - `engine/src/execute/run/effect.rs` lines 115-119: operations through the access are refused once the answer is applied or the call is aborted.
  - `vfs/src/handle/scope.rs` lines 39-43 and 213-215, `vfs/src/detail.rs` lines 1-10 and 92-98, `scheduler/apply.rs` lines 13-15 and 56-59, `scheduler/pending.rs` lines 66-67, and the module doc of `engine/src/execute/tests/tool_call_access.rs` lines 1-9.
  - Follow root `AGENTS.md` line 47: comments state constraints, not history. Do not edit plan files under `vibe/`.
- **Workshop: redact the gateway key (D1-16)** in `workshop-support`
  - Drop `Debug` from the derive list on `GatewayConfig` (`crates/workshop/support/src/config.rs` line 115) and add a hand-written `impl std::fmt::Debug` that prints `base_url` and shows `api_key` as `"<redacted>"`, matching `ResolvedGateway` in `crates/workshop/gateway/src/resolve.rs` lines 31-41. `Config` keeps its derive, since it formats the field through this impl.
- **harness-web comment (D1-13)**
  - Reword `crates/harness-web/src/web-tests.rs` lines 29-30 to "Run services over `cancel`, holding the search provider and this runtime's handle as each flag says."

</implementation-contract>
<verification-contract>

## Testing Plan

Each regression test must fail on `8f36fb553` before its fix lands. If one passes there, the finding is false: keep the test and skip that change.

- **D1-4 regression**, `an_access_kept_past_its_answer_is_refused` in `crates/promptforge-internal/engine/src/execute/tests/tool_call_access.rs`. A script makes two sequential tool calls. The driver keeps a clone of the first call's `Arc<Access>`, answers it, and while handling the second call writes through the kept clone. Expect `VfsError::PermissionDenied`. The scope is still open, so on `8f36fb553` the write succeeds and the test fails. Reuse the module's driver (`drive` in the engine's `test_support.rs` lines 93-127, and the `use_access` helper around lines 105-133 of `tool_call_access.rs`).
- **D1-2 regression**, `a_cancelled_tasks_tool_call_is_joined_and_its_later_writes_are_refused` in the same file. A script spawns a task whose chain calls a tool. The driver holds that tool call unanswered and writes `a.txt` through its access. The owner cancels the task with `tasks.cancel`, observes the cancellation, and reads `a.txt`. The driver then writes `b.txt` through the held access and answers the orphaned effect. Expect the owner to read the tool's content with no store conflict, and the `b.txt` write to be refused with `PermissionDenied`. On `8f36fb553` the owner's read is a fatal store conflict. Model the held-effect loop on `an_orphaned_effects_real_answer_is_discarded_and_still_counts_as_the_answer` (`engine/src/execute/run/tests.rs`) and the cancel-then-observe script on `TRACE_11` (`engine/src/execute/tests/happens_before.rs`).
- **D1-3 regression**, `an_ended_identity_keeps_no_clock_snapshot` in `crates/promptforge-internal/vfs/src/handle/tests/happens_before.rs`, beside `a_forked_child_shares_its_parents_clock_snapshot_instead_of_copying_it`. Fork a child from a parent that has already seen other identities, write through the child, end it into the parent. Check that the child's record holds an empty `seen`, that the parent's `seen` has `Arc::strong_count == 1` (so the next join does not clone), and that the parent reads the child's write without a conflict. Before adding `end`, a variant using `join` shows the child still holding its snapshot, which confirms D1-3.
- **VFS refusal coverage**, `an_ended_identity_refuses_operations_spawns_and_views_while_the_scope_stays_open` in `crates/promptforge-internal/vfs/src/detail-tests.rs`, beside `an_ended_scope_refuses_spawns_views_and_arm_operations_and_frees_its_claims`. Covers all three refusal sites.
- **D1-16 regression**, `debug_redacts_the_gateway_api_key` in `crates/workshop/support/tests/it/config.rs`. Build a `GatewayConfig` with `api_key: "secret-key"` (or parse a config that sets it), format the `GatewayConfig` and its containing `Config` with `{:?}`, and assert neither contains `secret-key`, mirroring `debug_redacts_the_api_key` in `crates/workshop/gateway/src/resolve-tests.rs` lines 332-343. Fails on `8f36fb553`.
- **D1-13** needs no test. The docs and clippy gates cover the file.
- **Existing tests that must keep passing**
  - `tool_call_access.rs`: `a_tool_reads_a_file_the_chain_wrote_just_before_the_call`, `the_chain_reads_a_file_the_tool_wrote`, `a_tool_in_a_spawned_task_forks_from_the_task_and_the_owner_reads_its_write_after_the_join`, `a_tool_that_writes_and_is_answered_dropped_is_still_joined`
  - the VFS suites `handle/tests/happens_before.rs` and `handle/tests/refusals.rs`
  - `cancellation_interrupts_a_slow_script_tools_call` (`engine/src/execute/tests/scheduler/failures-script-tools.rs`), `cancel_ends_a_parked_task_idempotently_and_reports_task_cancelled_once` (`engine/src/execute/tests/waits.rs`), and `an_orphaned_effects_real_answer_is_discarded_and_still_counts_as_the_answer`
- **Focused runs**: `cargo nextest run --locked -p promptforge-vfs -p promptforge-engine --all-features` and `cargo nextest run --locked -p workshop-support`.
- **Exit checks**: the root `AGENTS.md` gates on lines 52-58, in particular `cargo +<pinned nightly> xtask api --check` to confirm `crates/promptforge/public-api.txt` is unchanged, `cargo test -p build-xtask`, and the docs gate with `RUSTDOCFLAGS="-D warnings"`.

</verification-contract>
<decision-record>

## Decision Record

- **Reversible decisions and consequences**
  - Kept every classification. The analysis and challenge passes agreed on all 22 candidates, so no disagreement needed settling.
  - Took the challenger's corrections into the inventory. D1-1's identifier-resolution remedy is demoted because drift is mostly semantic. The governing rule is root `AGENTS.md` line 47, not a remove-first rule, which existed only inside the plugin-rename plan. D1-4's owned `Arc<Access>` on the effect was the agent's design, not an owner-weighed trade-off.
  - D1-3 stays rejected under the rules: no stated memory bound covers Rust-side scope memory and no workload was shown to reach a harmful N. It is the candidate closest to the line, and its growth contradicts the tool-context design record's linear cost note.
  - After the review the user expanded scope to D1-2, D1-3, D1-4, D1-13, and D1-16. None needs a hard-to-reverse change: the VFS and Engine changes are internal, `Effect::ToolCall` keeps its public shape, and the `Debug` change touches no wire or persisted format.
  - Join at abort, then refuse. An aborted tool call's writes so far are joined into its parked chain, the way a `Dropped` answer already is, and its later operations are refused. Consequence: a cancelled task's in-flight tool can no longer land writes, including on a real-directory mount, after the cancel; its late operations return `PermissionDenied`, and the Engine discards its answer as before.
  - End only tool identities, because task identities can be joined more than once.
  - Drop only `seen` on end, and keep `own` and `forked`, which conflict checks against the child's existing claims may still read.
- **User-resolved architecture choices**
  - None. Scope expansion was the user's choice, but no remediation is hard to reverse.
- **Rejected alternatives**
  - End without joining on abort: simpler, but keeps the timing-dependent fatal conflict for writes made before the cancel.
  - Make `Effect::ToolCall::access` a borrowed or lease type: enforces the rule at compile time but changes the public `Effect` enum, which is hard to reverse.
  - Fix D1-3 alone by compacting `seen` after a join: leaves D1-2 and D1-4, and one primitive covers all three.
  - A structurally shared map for `seen`: adds a dependency and still retains every snapshot.
  - Wrap `api_key` in a secret type: changes a public field type used across Workshop crates and test helpers, where a hand-written `Debug` is enough.
  - Remove the dead items in D1-14: they predate the target, and `TaskResumed` is a public event variant reserved by `vibe/2026-09/2026-09-18-4-sans-io-engine-harness.md` lines 285 and 343.
- **Assumptions and risks**
  - Tool VFS operations run inside polls of the Harness's flight futures on the effect loop, not concurrently with the Engine's step. Checking `ended` in the same lock acquisition that admits an operation keeps `end` correct even if that changes.
  - Conflict checks may read a non-acting identity's `own` and `forked` but not its `seen`. Confirm while implementing. If a check reads a claimant's `seen`, keep the snapshot for that case and report it.
  - A Host that keeps a clone of `Arc<Access>` from `Effect::ToolCall` and uses it after answering now gets `PermissionDenied`. The field doc already forbade that use.
  - `upstream/master` is the local remote-tracking ref, not freshly fetched (tip committed 2026-10-04 10:02 -0700).
  - Five target commits have no design record: `e45d37ef3`, `c8b131725`, `d8b11bce9`, `b8e7ae4c8`, and `8f36fb553` (`Plan: none`).
  - The review executed nothing. Behavior and the D1-3 memory figures come from reading code.
  - The large prose commits (`77b210b2a`, `f9b35597d`, `aed5a207d`, `8f36fb553`, `d8b11bce9`) were checked at their behavioral sites and by residual search, not line by line.

### Deferred and Out of Scope

- D1-1, prose drift beyond the D1-13 comment (exposed). Revisit when the user expands scope. Ranked remedies: remove restating prose under root `AGENTS.md` line 47; optionally a yes/no gate refusing deferral phrasing in comments with a tight phrase list; seed `RETIRED` in `crates/workshop/ui/test/docs-claims.mjs` with phrases each rename retires; an identifier-resolution check last, if at all.
- Snapshot retention for task identities, the pre-existing form of D1-3. Revisit if task-heavy scripts show memory growth; a fix needs a last-join signal, because tasks can be joined more than once.
- D1-5, `ToolCallOrigin` is an exhaustive public struct persisted in `EffectRecord::ToolCall` (`crates/promptforge-internal/engine/src/execute/run/effect.rs` lines 241-243 and 263-271). Revisit if the origin needs a new field. Adding `#[non_exhaustive]` and `#[serde(default)]` fields is a public interface change and needs owner approval.
- D1-6, two near-identical `#[cfg(test)]` `TestContext` fixtures in `crates/harness-internal/plugins/src/test_support.rs` and `crates/harness-web/src/test_support.rs`. Revisit if a third copy appears.
- D1-8, a user-made `chat.md` in the Workshop agents folder that still says `capabilities:` fails at launch and shadows the built-in chat agent (`crates/workshop/agents/src/discovery.rs` lines 40-60 and 80-94). Revisit if users report it. The error names the new key.
- D1-14, the `TaskResumed` doc (`crates/promptforge-internal/types/src/event.rs` lines 334-335) no longer points to its plan reservation. Revisit at the next dead-code sweep and check the sans-io plan before removing it.
- D1-12, activation failures are logged and swallowed in `crates/harness-internal/plugins/src/activation.rs`. Unrelated pre-existing; the target only renamed it.
- D1-17, `GET /admin/env` returns vendor keys in plaintext to the walled config page. Unrelated pre-existing and deliberate behind the loopback wall and bearer key.
- D1-18, the Workshop server keeps its own loopback `Host` and `Origin` policy in `crates/workshop/server/src/cross_site.rs`, separate from `crates/shared-loopback/src/origin.rs`. Unrelated pre-existing. Merging them is a trust-boundary change; revisit if the policies are shown to diverge on a hostile request.
- Updating `vibe/2026-10-04-1-tool-context.md`, whose orphan step and linear cost note this work makes stale. Plan files are history.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build --locked` builds the workspace default member, package `gateway` in `crates/gateway/app` (binary `promptforge-gateway`), per `default-members` in `Cargo.toml`. The Workshop desktop app is opt-in: `cargo build --locked -p workshop` (CI first installs Tauri system packages and stages the gateway sidecar with `node tools/stage-gateway-sidecar.mjs stage --target <triple> --source <gateway binary>`), or the `cargo workshop` alias for the `crates/build-workshop` orchestrator (`.cargo/config.toml`). UI bundles reach `$OUT_DIR/ui-dist` through build scripts, so CI runs `npm ci --prefix crates/workshop` and `npm ci --prefix crates/gateway/config-ui/ui` before any build.
- Focused test command pattern: `cargo nextest run --locked -p <crate> --all-features <test-name-filter>`; add `--test it` to target a crate's `tests/it` integration binary. Drop `--all-features` for `workshop`, `workshop-server`, and `workshop-server-api`. Gateway process tests run as `cargo test --locked -p gateway --no-default-features --features test-fixtures --test it <test-name>` (`.github/workflows/ci.yml`). A single JS test file runs with `node --test <file>` from its package directory.
- Component test command pattern: `cargo nextest run --locked -p <crate> --all-features` (the three Workshop app crates without `--all-features`). Structural checks: `cargo test -p build-xtask`. Workshop JS packages: `npm test --workspace <ui|look|platform>` run from `crates/workshop`. Gateway config UI: `npm test` run from `crates/gateway/config-ui/ui`.
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api` (`AGENTS.md`, `.github/workflows/ci.yml`). The workspace run includes `build-xtask`. CI also runs `npm test --workspaces --if-present` in `crates/workshop` and `npm test` in `crates/gateway/config-ui/ui`.
- Linter command: `CARGO_BUILD_WARNINGS=deny cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features` and `CARGO_BUILD_WARNINGS=deny cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`, plus the headless build-shape check `cargo check -p gateway --no-default-features`. Never add a standalone `cargo check --workspace` (`AGENTS.md`). TypeScript: `npm run typecheck --workspaces --if-present` in `crates/workshop` and `npm run typecheck` in `crates/gateway/config-ui/ui`. Supply chain: `cargo deny check` and `cargo hakari verify`.
- Formatter check command: `cargo fmt --all --check` (`rustfmt.toml`; also the `.githooks/pre-commit` hook). No JS formatter found.
- Docs command: `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`, plus, with the same `RUSTDOCFLAGS`, `cargo doc -p promptforge --no-deps`, `cargo doc -p harness --no-deps`, `cargo doc --locked --no-deps --all-features -p promptforge-engine --document-private-items`, and `cargo doc --locked --no-deps -p workshop-server --document-private-items`. Facade surface: `cargo +nightly-2026-09-05 xtask api --check` (pin in `crates/build-xtask/src/api/toolchain.rs`; `--bless` updates the committed `crates/promptforge/public-api.txt`), then `cargo nextest run --locked -p build-xtask --run-ignored only` on the same nightly.
- Test placement and naming conventions:
  - Unit tests live in `#[cfg(test)] mod tests`, either inline, in a sibling `<stem>-tests.rs` wired with `#[path = "<stem>-tests.rs"] mod tests;` (`crates/harness-internal/runner/src/environment.rs`), or in a `src/<module>/tests/` directory once there are three or more files (`crates/promptforge-internal/engine/src/execute/tests/`).
  - Integration tests compile into one binary per crate at `tests/it/main.rs` with modules beside it (`crates/gateway/app/tests/it/`, `crates/harness-internal/runner/tests/it/`, every Workshop library crate). `crates/harness` and `crates/promptforge` use `tests/suite/`. Prompt fixtures go in `tests/prompts/`, data in `tests/fixtures/`, and helpers in `tests/common/` or `tests/it/support/`.
  - Test names are snake_case sentences stating the behavior, such as `a_direct_launch_recovers_the_lease_from_a_terminated_owner`.
  - `clippy.toml` allows `unwrap` and `expect` only in tests. `build-xtask` fixtures that need rustdoc JSON are `#[ignore]`d on stable. `.config/nextest.toml` sets a 60s slow timeout and a `heavy` group for `gateway-stt` and `gateway-stt-backend-whisper`.
  - JS tests run under `node --test` over `test/**/*.mjs` and `src/**/*.test.mjs`.
- Directory map:
  - `crates/promptforge/`: the Engine facade, the family's one public crate; `public-api.txt` is the committed surface listing.
  - `crates/promptforge-internal/`: private Engine crates `engine`, `lua`, `model-client`, `parser`, `types`, `vfs`.
  - `crates/harness/`, `crates/harness-gateway-client/`, `crates/harness-web/`: the public Harness crates. `crates/harness-internal/`: private `runner` and `plugins`.
  - `crates/gateway/`: the private gateway family: `app` (package `gateway`), `cloud-providers`, `config`, `config-ui` (TypeScript UI in `ui/`), `local`, `logging`, `progress`, `protocol`, `routing`, `web-search`, and the nested `stt/` subsystem (`api` as package `gateway-stt`, `engine`, `backend-whisper`, `whisper-ffi`).
  - `crates/gateway-api-types/`, `crates/gateway-api-discovery/`: the gateway's public pair.
  - `crates/workshop/`: the Workshop Host. Rust crates `desktop` (Tauri app, package `workshop`), `server`, `server-api`, `gateway`, `menu`, `protocol`, `registry`, `status`, `support`, `user-state`, `workspace`, `run-log`, `agents`; npm workspace packages `ui`, `look`, `platform`.
  - `crates/shared-error-source/`, `crates/shared-loopback/`: shared leaf crates. `crates/shared-ui/`: a TypeScript and CSS package for the gateway config UI, not a Rust crate.
  - `crates/build-*`: tooling. `build-xtask` (structural checks and the `api`, `tidy`, `site`, `new-crate` subcommands), `build-workshop` (`cargo workshop`), `build-ui`, `build-user-guide`, `build-llama-cuda`, `build-ceiling`. `crates/workspace-hack/` is the cargo-hakari crate (`.config/hakari.toml`).
  - `guide/`: user guide books, chrome, and landing page. `prompts/`: example prompts. `local/`: local gateway config, env files, profiles, prompts, and STT fixtures. `tools/`: Node scripts for staging the gateway sidecar and a live TTS check. `vibe/`: plan records. `.github/workflows/`: CI and release workflows. `.githooks/`: pre-commit fmt; pre-push headless check, clippy, and `cargo deny`.
- Component boundaries (enforced by `cargo test -p build-xtask` through `crates/build-xtask/src/product.rs` and `crates/build-xtask/src/tidy.rs`):
  - Engine (`promptforge`, `promptforge-*`) depends on no gateway, Workshop, or Harness crate. Other families reach it only through `promptforge`, the one outside crate allowed into `crates/promptforge-internal/` besides the family itself.
  - Harness (`harness`, `harness-*`) names only `promptforge` and `workspace-hack` outside its family, so no gateway, shared, Workshop, or `build-*` crate. Other crates reach it only through `harness`, `harness-gateway-client`, and `harness-web`. `crates/harness-internal/` is open only to the family's crates inside it and `harness`.
  - Gateway (`gateway`, `gateway-*`) depends on no Engine, Workshop, or Harness crate. `crates/gateway/` is private to the family. Workshop crates may name only `gateway-api-types` and `gateway-api-discovery`. Within the family, the `crates/gateway/stt/` subsystem is reachable only through `gateway-stt`.
  - Workshop tiers: vocabulary crates (`workshop-protocol`, `workshop-registry`, `workshop-support`) name no Workshop crate; services (`workshop-gateway`, `workshop-menu`, `workshop-status`) name vocabulary only; features (`workshop-agents`, `workshop-run-log`, `workshop-user-state`, `workshop-workspace`) name vocabulary and services; `workshop-server` may name every lower tier. The desktop app `workshop` names only `workshop-server-api`, which names only `workshop-server`.
  - `shared-*` crates depend on no product crate. A Cargo cycle rejection means the design is wrong, not the graph (`AGENTS.md`).
- Conventions summary:
  - Rust edition 2024 on stable (`rust-toolchain.toml`), resolver 3. Every dependency version lives in root `[workspace.dependencies]`, with a comment justifying each pin or feature choice, and every member inherits `workspace-hack`.
  - Members inherit `[workspace.lints]`: clippy `all` and `pedantic` at deny, `unwrap_used`, `expect_used`, and `allow_attributes` at deny (suppress with `#[expect(lint, reason = "...")]`), `unsafe_code` at deny, `missing_docs` and `unreachable_pub` at warn, which the gate denies. `clippy.toml` bans process-global installers outside binary entry points.
  - Every crate's `build.rs` runs the `build-ceiling` check, which fails the build when any Rust file in the crate is over 500 lines (`crates/build-ceiling/src/lib.rs`). Harness crates include its source by `#[path]` instead of depending on it.
  - Source directories stay flat: one or two child files sit beside the parent as `foo-bar.rs` wired by `#[path]`, three or more become a `foo/` directory, and a group on the wrong side is converted when touched (`AGENTS.md`).
  - Every `workshop-*` and `harness-*` crate's crate docs carry a `## Invariants` section; Workshop crates state their tier there (`crates/workshop/server/src/lib.rs`).
  - Engine, Harness, Host, and Plugin are capitalized defined terms with one meaning each, enforced by `crates/workshop/ui/test/docs-claims.mjs` in every `AGENTS.md`, `## Invariants` doc, and `.cursor/rules` file.
  - Comments explain only non-obvious constraints, and every workaround cites its upstream issue URL. Error and status messages are written for model consumption, naming required versus actual.
  - Behavior changes ship with tests in the same change. JSON that reaches a recorder or replay comparison round-trips exactly (`serde_json` with `float_roundtrip`, sorted keys, finite numbers).
  - Workshop UI CSS uses tokens from `@workshop/look` and `ui/src/tokens/component.css`; persisted UI values go through the `ui-storage` adapter.
  - Commits use short imperative subjects; a finished plan gets a `Close plan: <slug>` commit, and plan records live in `vibe/YYYY-MM-DD-N-<slug>.md`.

</project-survey>
<execution-plan>

## Execution Instructions

<step-1>

### Step 1: Add a VFS primitive that ends one identity [completed]

- Component: Tool-call identity lifetime (D1-2, D1-3, D1-4)
- Component order: first of three. The components share no code and could land in any order. This one goes first because it is the largest and riskiest change, so the gates of every later step also run over it.
- Piece: the VFS end primitive in `promptforge-vfs`. It is built before the Engine wiring in Step 2, because Step 2 calls `detail::end_access`, so the two pieces are sequential.
- Artifacts:
  - `Identity` in `crates/promptforge-internal/vfs/src/handle/scope.rs` (lines 47-64): add `ended: bool`, false at fork.
  - `Scope::end(&self, owner: Option<ExecId>, child: ExecId)` in the same file. Under one lock acquisition it marks `child` ended, merges `child`'s clock into `owner` when `owner` is `Some`, then replaces `child`'s `seen` with an empty map. It keeps `own` and `forked`. Factor the merge out of `Scope::join` (lines 230-247) into one private helper that both call, with no change to what `join` does.
  - Refusal of an ended identity beside each `refuse_if_closed` call that acts for an identity: `Access::admit` (`handle/access.rs` lines 104-106), `Access::spawn` (lines 37-39), and `Access::store_view` (`handle/store_view.rs` lines 179-181). Return `VfsError::PermissionDenied` with a reason saying the tool call that owned this access has ended. Read the flag in the same lock acquisition that admits the operation, so nothing is admitted after `end`.
  - `detail::end_access(scope: &ScopeHandle, owner: Option<ExecId>, child: ExecId)` in `crates/promptforge-internal/vfs/src/detail.rs`, beside `access_join` (line 100) and `end_scope` (line 42).
  - Task identities and every existing `join` caller stay unchanged.
- Docs: rewrite `scope.rs` lines 39-43 (the `identities` field) and 213-215 (`release`), and `detail.rs` lines 92-98 (`access_join`), so they say an identity ended through `end` keeps no clock snapshot. Leave the `detail.rs` module doc (lines 1-10) for Step 2, because it describes how the Engine uses these operations, which does not change until then. Comments state constraints, not history (root `AGENTS.md` line 47).
- Tests:
  - `an_ended_identity_keeps_no_clock_snapshot` in `crates/promptforge-internal/vfs/src/handle/tests/happens_before.rs`, beside `a_forked_child_shares_its_parents_clock_snapshot_instead_of_copying_it`, as the Testing Plan describes. Write it with `join` first and confirm the child still holds its snapshot, which confirms D1-3 at `8f36fb553`. If the child holds none, keep the test and report D1-3 as false; the primitive still lands for D1-2 and D1-4. Then switch the test to `end`.
  - `an_ended_identity_refuses_operations_spawns_and_views_while_the_scope_stays_open`, covering all three refusal sites. The Testing Plan places it in `crates/promptforge-internal/vfs/src/detail-tests.rs`, beside `an_ended_scope_refuses_spawns_views_and_arm_operations_and_frees_its_claims`. That file is already 471 lines, and the build ceiling (`crates/build-ceiling/src/lib.rs`) fails any Rust file over 500. If the test does not fit, put it in `handle/tests/refusals.rs` (109 lines) and widen that module's doc to cover an ended identity.
- Check while implementing: whether any conflict check reads a non-acting identity's `seen`. If one does, keep the snapshot for that case and report it.
- Verify: `cargo nextest run --locked -p promptforge-vfs -p promptforge-engine --all-features` passes, including the VFS suites `handle/tests/happens_before.rs` and `handle/tests/refusals.rs`. The Engine suite passes unchanged, because nothing in the Engine calls `end_access` yet.
- Commit: `Add a VFS primitive that ends one identity`, containing the primitive, the refusal checks, the rewritten VFS docs, and both tests.

</step-1>

<step-2>

### Step 2: End each tool call's identity when it is answered or aborted [completed]

- Component: Tool-call identity lifetime (D1-2, D1-3, D1-4)
- Piece: the Engine wiring in `promptforge-engine`. It comes after Step 1 because it calls `detail::end_access`.
- Tests first, in `crates/promptforge-internal/engine/src/execute/tests/tool_call_access.rs`:
  - `an_access_kept_past_its_answer_is_refused` (D1-4) and `a_cancelled_tasks_tool_call_is_joined_and_its_later_writes_are_refused` (D1-2), as the Testing Plan describes. Reuse `drive` in `crates/promptforge-internal/engine/src/test_support.rs` (lines 93-127) and the module's `use_access` helper. Model the held-effect loop on `an_orphaned_effects_real_answer_is_discarded_and_still_counts_as_the_answer` (`engine/src/execute/run/tests.rs`) and the cancel-then-observe script on `TRACE_11` (`engine/src/execute/tests/happens_before.rs`).
  - Confirm both fail before editing the Engine. Step 1 changes no Engine behavior, so a failure here is a failure at `8f36fb553`. If one passes, keep it and skip the edit it guards: the `apply_answer` change for D1-4, the `abort_effect` change for D1-2.
  - The file is 207 lines now. If the two tests push it past 500, move the D1-2 test to a sibling `tests/tool_call_access-cancel.rs` wired by `#[path]`, the way `tool_call_arm-local-handlers.rs` sits beside `tool_call_arm.rs`.
- Artifacts:
  - `apply_answer` in `crates/promptforge-internal/engine/src/execute/scheduler/apply.rs` (lines 70-80): replace the `access_join(access, call.exec)` branch with `end_access(scope, owner, call.exec)`, where `owner` is the parked chain's `access_id` when the chain still holds an access. This runs for every answer, `Dropped` included.
  - `abort_effect` in `scheduler/chain.rs` (lines 280-288): keep the removed `Pending { chain, resume }`. When `resume` is `Continuation::ToolCall(call)`, call `end_access` the same way before recording the id as orphaned. The orphan early return in `Scheduler::resume` (`scheduler/drive.rs` lines 95-97) stays.
  - Both take the scope from `Scheduler`'s `scope: Option<ScopeHandle>` (`execute/scheduler.rs` line 279). When it is `None`, the run's scope has closed and every access is already refused, so skip the call.
  - `Effect::ToolCall` keeps `pub access: Arc<Access>`. No facade change.
- Docs: rewrite `engine/src/execute/run/effect.rs` lines 115-119 to say operations through the access are refused once the answer is applied or the call is aborted. Also rewrite `scheduler/apply.rs` lines 13-15 and 56-59, `scheduler/pending.rs` lines 66-67, the module doc of `tool_call_access.rs` (lines 1-9), and the module doc of `crates/promptforge-internal/vfs/src/detail.rs` (lines 1-10), which says the Engine joins tool call identities on the call's answer. Do not edit plan files under `vibe/`.
- Verify: under `cargo nextest run --locked -p promptforge-vfs -p promptforge-engine --all-features`, the two new tests pass, along with the four existing `tool_call_access.rs` tests (`a_tool_reads_a_file_the_chain_wrote_just_before_the_call`, `the_chain_reads_a_file_the_tool_wrote`, `a_tool_in_a_spawned_task_forks_from_the_task_and_the_owner_reads_its_write_after_the_join`, `a_tool_that_writes_and_is_answered_dropped_is_still_joined`), `cancellation_interrupts_a_slow_script_tools_call`, `cancel_ends_a_parked_task_idempotently_and_reports_task_cancelled_once`, `an_orphaned_effects_real_answer_is_discarded_and_still_counts_as_the_answer`, and the VFS suites. `cargo +nightly-2026-09-05 xtask api --check` shows `crates/promptforge/public-api.txt` unchanged.
- Commit: `End each tool call's identity when it is answered or aborted`, containing the Engine edits, the rewritten docs, and both tests. The message says it resolves the deferral recorded on `34270d278` ("The Engine does not join a tool call orphaned by an aborted chain").

</step-2>

<step-3>

### Step 3: Redact the gateway key in Workshop config Debug output [completed]

- Component: Workshop gateway key redaction (D1-16)
- Component order: second. It shares no code with the other two. It comes after the larger Engine change and before the comment fix, which closes the plan with the full gates.
- Piece: one piece in `workshop-support`, built as one step, because the `Debug` impl and its test are one behavior slice.
- Test first: `debug_redacts_the_gateway_api_key` in `crates/workshop/support/tests/it/config.rs`, as the Testing Plan describes, mirroring `debug_redacts_the_api_key` in `crates/workshop/gateway/src/resolve-tests.rs` lines 332-343. It formats both a `GatewayConfig` and its containing `Config` with `{:?}`. Confirm it fails. If it passes, keep it and skip the change.
- Artifacts: drop `Debug` from the derive list on `GatewayConfig` (`crates/workshop/support/src/config.rs` line 115) and add a hand-written `impl std::fmt::Debug for GatewayConfig` that prints `base_url` and shows `api_key` as `"<redacted>"`, matching `ResolvedGateway` in `crates/workshop/gateway/src/resolve.rs` lines 31-41. `Config` (line 23) keeps its derive, since it formats the field through this impl. The public field type of `api_key` stays `String`.
- Verify: `cargo nextest run --locked -p workshop-support` passes.
- Commit: `Redact the gateway key in Workshop config Debug output`, containing the impl and the test.

</step-3>

<step-4>

### Step 4: Fix the stale harness-web test comment and run the exit gates

- Component: harness-web comment fix (D1-13)
- Component order: last. It is the smallest change, and its own check (the docs and clippy gates) is part of the full exit run that closes the plan.
- Piece: one piece, built as one step.
- Artifacts: reword the comment at `crates/harness-web/src/web-tests.rs` lines 29-30 to "Run services over `cancel`, holding the search provider and this runtime's handle as each flag says." No new test, because the comment carries no behavior.
- Verify, once Steps 1-3 have landed: run the full root `AGENTS.md` gates (lines 52-58). These are both nextest runs, both clippy runs with `CARGO_BUILD_WARNINGS=deny` plus `cargo check -p gateway --no-default-features`, `cargo fmt --all --check`, the workspace docs run and `cargo doc -p promptforge --no-deps` with `RUSTDOCFLAGS="-D warnings"`, `cargo +nightly-2026-09-05 xtask api --check` with `crates/promptforge/public-api.txt` unchanged, and `cargo test -p build-xtask`. Fix any failure before committing, and report which earlier step caused it.
- Commit: `Fix the stale harness-web test comment`, containing the comment change and any gate fix.

</step-4>

</execution-plan>
