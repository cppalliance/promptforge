---
name: Core-support API refinements
overview: "Apply the four recommended API actions to promptforge-core-support: GuardNonce::wrap method + Display, missing detail constants, label/Display doc clarification, and hierarchical cancellation (CancelHandle::child) for the future agentic harness, with call-site migration and tests."
todos:
  - id: guardnonce-wrap
    content: "untrusted: GuardNonce::wrap method, deprecate free wrap, Display + Eq/Hash, debug_assert, migrate 2 call sites"
    status: completed
  - id: observe-consts
    content: "observe: add 3 MODEL_CATALOG_VALIDATION detail consts + label/Display doc sentence"
    status: completed
  - id: cancel-child
    content: "cancel: swap internals to tokio-util CancellationToken, add CancelHandle::child() via child_token(), keep tests as contract tests"
    status: completed
  - id: tests
    content: Tests for all three modules per crate AGENTS.md rule
    status: completed
  - id: agents-md
    content: "AGENTS.md: 3 surgical edits - fix dependency-graph claim, add determinism invariant, add closed-inventory rule. No rewrite."
    status: completed
  - id: verify
    content: cargo test (3 crates) + clippy --workspace clean
    status: completed
isProject: false
---

# Core-Support Public API Refinements

## Context

Code review of `promptforge-core-support` surfaced four API actions; the user additionally confirmed the agentic harness will need an orchestrator to cancel subagents, so hierarchical cancellation lands now. After comparing a hand-rolled parent-link against `tokio_util::sync::CancellationToken`, the user chose CancellationToken: `child_token()` is exactly the orchestrator/subagent semantics, it deletes the hand-rolled race-prone `Notify` code, and `tokio-util` is already a workspace dependency (`promptforge-mcp-server` uses it, including `CancellationToken` in its transport layer). Only two production call sites use the free `wrap` function, so migration is cheap.

## Changes

### 1. `untrusted`: `GuardNonce::wrap` method, `Display`, equality ([src/untrusted.rs](promptforge/crates/promptforge-core-support/src/untrusted.rs))

- Move the body of free `fn wrap(nonce, content)` into `impl GuardNonce { #[must_use] pub fn wrap(&self, content: &str) -> String }`.
- Keep the free `wrap` as a one-line delegate marked `#[deprecated(note = "use GuardNonce::wrap")]` - the crate has crates.io publish metadata, so soft-removal rather than a breaking delete.
- Add `impl fmt::Display for GuardNonce` rendering the 32 hex digits, with a doc note: the value is not secret (it appears verbatim in every envelope and preface); only *construction* is controlled. Enables log correlation of envelopes to runs.
- Derive `PartialEq, Eq, Hash` on `GuardNonce` (currently only `Clone, Debug`).
- Add `debug_assert!(nonce.is_ascii() && nonce.len() == 32)` in `neutralize` to pin the byte-slicing invariant.
- Update module docs that reference `[wrap]` to point at the method.
- Migrate the two production call sites to method syntax:
  - [promptforge-lua/src/host.rs](promptforge/crates/promptforge-lua/src/host.rs) line 103: `promptforge_core_support::untrusted::wrap(&nonce, &s)` -> `nonce.wrap(&s)`
  - [promptforge-core/src/execute/tool_loop.rs](promptforge/crates/promptforge-core/src/execute/tool_loop.rs) line 263: `untrusted::wrap(nonce, output.text())` -> `nonce.wrap(output.text())`
- Migrate the crate's own tests to the method so deprecation warnings do not fire in-tree.

### 2. `observe`: missing constants + doc sentence ([src/observe.rs](promptforge/crates/promptforge-core-support/src/observe.rs))

- Add `MODEL_CATALOG_VALIDATION_STARTED`, `MODEL_CATALOG_VALIDATION_SUCCEEDED`, `MODEL_CATALOG_VALIDATION_FAILED` consts to the `detail` module between `TOOL_SCOPE_VALIDATION_FAILED` and `STORE_WRITE_SUCCEEDED`, matching enum/label order.
- Add one sentence to `Observation::label` docs stating the relationship: `Display` is the human trace line for any variant; `label` is the stable machine key for fixed variants only (`None` for `Lua`/`Other`).

### 3. `cancel`: hierarchical cancellation via `CancellationToken` ([src/cancel.rs](promptforge/crates/promptforge-core-support/src/cancel.rs))

Design (tokio-util, per user decision):

- Add `tokio-util` to [Cargo.toml](promptforge/crates/promptforge-core-support/Cargo.toml) with `default-features = false` and only the feature the `sync` module needs (verify during implementation: `promptforge-mcp-server` uses `CancellationToken` with just `codec` enabled, so the gate is minimal or none; confirm the exact minimal feature set and use that). Prefer a workspace-managed entry if the root Cargo.toml convention supports it; otherwise mirror `promptforge-mcp-server`'s declaration.
- Replace the internals of `CancelHandle` - `cancelled: Arc<AtomicBool>` and `notify: Arc<Notify>` - with a single `token: CancellationToken`. The struct stays `#[non_exhaustive]` with private fields, so this is invisible downstream.
- Method bodies become delegations: `cancel()` -> `token.cancel()`, `is_cancelled()` -> `token.is_cancelled()`, `cancelled()` -> `token.cancelled().await`. All documented semantics (clone shares state, idempotent, irreversible, drop-safe) map 1:1.
- Delete the hand-rolled enable-check-await lost-wakeup machinery. Keep the existing tests - they become API-contract tests pinning our documented semantics regardless of internals; they should pass unmodified. The `cancel_between_check_and_wait_is_not_lost` test reaches into `handle.notify` directly and must be rewritten against the public API (drop a waiter task, race a cancel, assert completion).
- New constructor:

```rust
/// Returns a fresh handle cancelled when this handle (or any ancestor) is
/// cancelled. Cancelling the child never affects the parent or siblings.
#[must_use]
pub fn child(&self) -> CancelHandle
```

  implemented as `CancelHandle { token: self.token.child_token() }` - arbitrary-depth trees, no registry, no cycles.
- Update the `cancelled()` doc comment, which currently describes the `Notified::enable` sequence, to describe semantics only (implementation detail no longer ours).
- Document the harness pattern on `child()`: orchestrator holds the run handle; each subagent task installs `run_handle.child()` via `scope`, so Ctrl-C at the run level cancels all subagents while the orchestrator can cancel one subagent without touching the rest.
- The task-local layer (`scope`, `maybe_scope`, `current`, `wait_cancelled`, `is_cancelled`) is untouched.
- The existing `Send + Sync + 'static` assertion test must still pass (`CancellationToken` satisfies all three).

### 4. Tests (same change, per crate AGENTS.md rule)

- `untrusted`: method output byte-identical to the documented envelope shape; `Display` renders 32 lowercase hex; `Eq`/`Hash` smoke test; existing property tests re-pointed at the method.
- `observe`: const values match their variants (`detail::MODEL_CATALOG_VALIDATION_STARTED == Observation::ModelCatalogValidationStarted`, etc.).
- `cancel`: parent cancel propagates to child (both `is_cancelled` poll and `cancelled()` wait); child cancel leaves parent and sibling unaffected; grandchild chain propagates; child of a pre-cancelled parent is born cancelled; waiters on a child wake on parent cancel; `scope(child, ...)` observed through `wait_cancelled()`.

### 5. Minimal `AGENTS.md` fix ([crates/promptforge-core-support/AGENTS.md](promptforge/crates/promptforge-core-support/AGENTS.md))

Three surgical edits, no rewrite - the file stays short:

- Fix the dependency sentence that states aspiration as fact (it caused a real misreading of the blast radius): "No dependencies on other promptforge crates - this crate sits at the bottom of the graph so nothing cycles." Drop the "every promptforge crate may depend on this one" claim.
- Add one line for the determinism invariant: "One nonce per run; identical content must produce a byte-identical envelope (KV-cache sharing and snapshot tests depend on it)."
- Add one line for the inventory posture: "The control-markup inventory is closed on purpose: additive table entries with a family rationale only, never matcher generalization."

Nothing else moves in: the cancellation contract, semver/`#[non_exhaustive]` discipline, and threat-model docs already live in the module docs, which are the spec.

### 6. Verification

- `cargo test -p promptforge-core-support`
- `cargo test -p promptforge-core -p promptforge-lua` (migrated call sites)
- `cargo clippy --workspace` clean under workspace lints

## Data flow / sequencing

Steps 1-3 are independent modules and could run in any order; step 1's call-site migration must land with the deprecation to keep the workspace warning-free. Step 4 ships with each step. Step 5 (AGENTS.md) is three prose edits and can land any time. Step 6 runs last.

## Execution discipline (added at execution approval)

- Single commit carrying all fixes, per user instruction. Vibe-rulebook bounded path: one step, per-step checklist (Code -> Commit -> Review -> Amend -> Verify).
- Governing rulebooks: `tools-public/rulebooks/vibe-rulebook.md` (process) and `tools-public/rulebooks/rust-rulebook.md` (code).
- Governing AGENTS.md manifest for touched files: `promptforge/AGENTS.md`, `promptforge/crates/promptforge-core-support/AGENTS.md`, `promptforge/crates/promptforge-core/AGENTS.md` (tool_loop.rs), `promptforge/crates/promptforge-lua/AGENTS.md` (host.rs).
- `#[deprecated(since = "0.2.0", note = "use GuardNonce::wrap")]` - 0.2.0 is the uniform version staged for crates.io republication.
- `tokio-util` is declared once in `[workspace.dependencies]` with `default-features = false` (rust-rulebook section 8); `promptforge-core-support` inherits with `.workspace = true`. The minimal feature gate for `tokio_util::sync::CancellationToken` is verified empirically during implementation.
- Worktree was clean on `master` at `8ed1632` at run start.

## Out of scope

- No changes to the `untrusted(s)` Lua global or user-guide docs (behavior unchanged).
- `detail` remains `#[doc(hidden)]`; no semver reclassification.


---

## Recovered rationale

Recovered from the producing chat sessions by the plan ledger on 2026-09-04. Everything below this heading is derived annotation, not part of the original plan.

# Enrichment: Core-support API refinements (8caa1f22)

## Origin

The plan grew out of a code review of `promptforge-core-support`, followed by a dedicated public-API review. The reviews judged the crate high-quality (strong docs, property tests, a pinned lost-wakeup regression) and surfaced the four actions the plan carries. Two review judgments explain the shape of the changes:

- The `GuardNonce` secrecy argument only justifies hiding *construction*, not *reading*: the nonce's hex digits already appear verbatim in every envelope and preface, so exposing them "weakens nothing" (paraphrase of the review). The concrete cost of hiding was that hosts could not correlate a suspicious envelope back to a run in logs, and downstream tests had to parse the nonce out of the envelope. Hence `Display` (chosen over a public `as_str`) plus trivially derivable `Eq`/`Hash`.
- The free `wrap` was soft-deprecated rather than deleted because the crate carries crates.io publish metadata; only two production call sites existed, so migration was cheap.

## The decisive user inputs (verbatim)

- `"nonce.wrap sounds right to me, also plan the recommended actions. furthermore, note this: when we implement the agentic harness, the model acting as orchestrator will want to be able to cancel subagents"` - this single sentence moved hierarchical cancellation from the review's "defer until needed" recommendation into the current plan, and is the entire reason `CancelHandle::child()` exists.
- `"so clearly we want the tokio piece"` - the decision to adopt `tokio_util::sync::CancellationToken` over the hand-rolled parent-link, after a full pros/cons discussion.
- `"I dont want to start bloating AGENT.md"` - the reason the AGENTS.md step is three surgical edits instead of the planned rewrite.
- `"I want all the fixes in a single commit using @tools-public/rulebooks/vibe-rulebook.md discipline and also apply @tools-public/rulebooks/rust-rulebook.md"` - the execution discipline.

## Discarded alternative: hand-rolled parent-link vs CancellationToken

The plan initially specified a parent-link design (`parent: Option<Arc<CancelHandle>>`, `is_cancelled` walking the chain, `tokio::select!` over own and parent waiters, ~30 lines plus tests). The user asked to hash out the tokio question before committing. The comparison that drove the flip:

- For CancellationToken: `child_token()` is exactly the orchestrator/subagent semantics, battle-tested including tree-wide races (child created mid-cancel, waiter registered while an ancestor fires); it deletes the crate's riskiest code - the hand-rolled `Notify` machinery had already shipped one real lost-wakeup bug; `promptforge-mcp-server` already depends on `tokio-util` and already uses `CancellationToken` in its transport layer, so it was "move an already-vetted crate into the leaf position," not a foreign dependency; `drop_guard()` gives cancel-on-scope-exit for free.
- For the parent-link: zero new dependencies, incremental change on tested code, full semantic control (cancellation reasons, deadlines, subtree-vs-self distinctions).
- A key framing lowered the stakes: `CancelHandle` is `#[non_exhaustive]` with private fields, so the internals swap is invisible downstream and reversible at any time.

The leaf-crate dependency concern was then materially weakened by a user correction (below), leaving the decision "closer to a coin flip on taste" (assistant's words), which the user settled for tokio.

## The dependency-graph correction (why the AGENTS.md fix exists)

The assistant argued against tokio-util from the "leaf position": every promptforge crate would carry it forever. The user pushed back hard: `"core utils is not used by every crate... the workshop UI doesn't care about this. It's just a TypeScript HTML CSS... I want to know what crates really depend on this?"` Reading the actual Cargo.tomls showed 3 direct dependents (`core`, `parser`, `lua`), 5 transitive (the binaries on `promptforge-core`), and 21 of 29 workspace crates with no path to it. The assistant had been parroting the crate's AGENTS.md line "every promptforge crate may depend on this one" as if it described reality. That misreading is why the plan's AGENTS.md edit replaces the aspiration-as-fact sentence with the actual intent ("bottom of the graph, nothing cycles") - the file had demonstrably failed at its job.

A related user question - `"what is tokio/sync? is that synchronous I/O? that would be a mistake"` - was resolved by explaining `sync` is a Cargo feature flag enabling tokio's *async synchronization primitives* (`Notify`, oneshot, mpsc), not synchronous I/O; the async Lua-coroutine architecture was untouched. This exchange immediately preceded the tokio decision.

## Discarded alternative: the full AGENTS.md rewrite

The AGENTS.md review produced eight findings: the dependency misstatement, silence on third-party dependencies, the missing byte-identical-envelope determinism invariant (KV-cache prefix sharing and snapshot tests depend on it), the missing closed-inventory posture (additive table entries only, never matcher generalization - otherwise a helpful agent would eventually "improve" the matcher with regex and widen false positives onto prose), undocumented semver/`#[non_exhaustive]` discipline, the missing cancellation contract (idempotent, irreversible, task-local does not cross `tokio::spawn`), the lint-enforceable-but-not-enforced doc rule, and a missing verification command. After the user's bloat objection, only three edits survived: the dependency fix, the determinism invariant, and the closed-inventory rule. The rest was deliberately left in the module docs, which the plan declares "are the spec."

## Other review paths considered and set aside

- Renaming the task-local `is_cancelled()` to `is_current_cancelled()` to kill the shadowing with the handle method - rejected as a breaking rename, low severity.
- Collapsing `CancelHandle`'s two `Arc`s into one `Arc<Inner>` - not worth a refactor on its own (moot after the CancellationToken swap).
- Hoisting allocations in `wrap` - judged irrelevant at current call rates.
- The `wait_cancelled()` permanent-hang footgun when no scope is installed - reviewed and accepted as designed.

## Run chat deviations

None. The run chat contains no execution deviations: it is a post-hoc verification ("did this plan execute?") that confirmed every planned change had landed in commit `5312fcf` and that the plan file's todo statuses were simply stale; its only action was marking the todos completed. Execution itself was a single commit under the vibe-rulebook bounded path (Coder, Message, Review-and-Fix with zero findings, Verify pass), with one incidental note: the verification build regenerated tracked `ui/dist` artifacts in two unrelated crates, which were restored afterward.
