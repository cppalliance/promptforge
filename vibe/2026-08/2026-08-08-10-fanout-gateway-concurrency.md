---
name: Fanout and gateway concurrency
overview: Wire llama --parallel to gateway lane concurrency; set P=3 in gemma.toml and P=10 in qwen.toml (common.toml stays model-agnostic); run core fanout arms concurrently under JoinSet. Execute with vibe-rulebook (one testable commit per step, coder then review-and-fix subagents); Rust edits follow rust-rulebook; this plan follows prompts-rulebook.
todos:
  - id: step-1
    content: "Commit 1: LaunchOptions.parallel from local_model_concurrency; server_args; unit tests; design-gateway"
    status: completed
  - id: step-2
    content: "Commit 2: common.toml device only; gemma.toml lane generative=3; qwen.toml lane generative=10"
    status: completed
  - id: step-3
    content: "Commit 3: Concurrent fanout JoinSet + AtomicU32 turns + fail-fast abort + design-core/README/STATUS"
    status: completed
  - id: step-4
    content: "Commit 4: Fanout concurrency fixture test + gateway multi-admit IT if harness allows"
    status: completed
isProject: false
---

# Gateway + core concurrency

Governing rulebooks (read before coding; do not paste into subagent prompts - pass path + tag):

- [vibe-rulebook.md](c:\Users\Vinnie\src\cursor\tools-public\rulebooks\vibe-rulebook.md) - one testable commit per step; coder then review-and-fix; Verify on schedule; main context stays clean
- [rust-rulebook.md](c:\Users\Vinnie\src\cursor\tools-public\rulebooks\rust-rulebook.md) - Result for expected failure; test with code; fmt + clippy -D warnings before each commit; no MutexGuard across await
- [prompts-rulebook.md](c:\Users\Vinnie\src\cursor\tools-public\rulebooks\prompts-rulebook.md) - this plan is self-contained; one reading per instruction

## 1. What you are building

Local and remote completes already queue through the gateway. Fanout still runs arms one by one, and llama always starts with `--parallel 1`, so briefer topics never overlap. After this work: gateway lane concurrency sets both admit limit and `--parallel`; Gemma uses 3 slots; Qwen uses 10; core fanout starts every arm at once and returns replies in list order.

## 2. High-level components (dependency order)

1. **Gateway local launch** - depends on nothing in this plan. Emits `--parallel P` from resolved lane concurrency. Without this, concurrent fanout only queues behind one llama slot.
2. **Workspace profiles** - depends on (1). `common.toml` adds only `local-gpu`; each leaf declares lane `generative` with its own concurrency (3 / 10) and binds the model.
3. **Core fanout** - depends on (1) for useful overlap; code-correct without it. Spawns all arms concurrently; gateway admits up to P.
4. **Regression tests + docs** - depends on (1) and (3). Locks behavior and design text.

## 3. Pieces inside each component

**Gateway:** `LaunchOptions.parallel`, `launch_options` plumbing, `server_args`, unit test for default P=1 and lane P=N, `design-gateway.md` one-knob note.

**Profiles:** `common.toml` shared device only; each of `gemma.toml` / `qwen.toml` adds `[[device.lane]]` + model `device`/`lane` bind.

**Core fanout:** `run_one_arm`, `JoinSet` spawn, `AtomicU32` turn index, ordered reply vec, fail-fast `abort_all`, design-core / README / STATUS / module docs.

**Tests:** preamble-only two-arm store write fixture; optional gateway IT for concurrency greater than 1.

## 4. Steps (one commit each)

### Step 1 - Gateway `--parallel` from lane concurrency

Intent: llama slot count equals `Config::local_model_concurrency` for that model.

Do:
- Add `parallel: u32` to `LaunchOptions` in [server.rs](promptforge/crates/promptforge-gateway/src/local/server.rs).
- Set it from the resolved concurrency in [local/mod.rs](promptforge/crates/promptforge-gateway/src/local/mod.rs) when building launch options (value already validated >= 1).
- Replace hardcoded `"1"` in `server_args` with that field.
- Update `launch_args_match_local_model_defaults` so default-no-lane still expects `--parallel` `1`; add a case that a lane with concurrency N emits `--parallel` N.
- State in [design-gateway.md](promptforge/crates/promptforge-gateway/design-gateway.md): local lane concurrency is the admit limit and llama `--parallel`.

Test command the coder names: `cargo test -p promptforge-gateway launch_args` (or the exact filter covering those tests).

Decision: no new `[[local_model]]` field. Falsifier: a deployment needs `--parallel` different from admit limit - then split the knobs in a later commit.

### Step 2 - Profile lanes (Gemma 3, Qwen 10)

Intent: each leaf selects P; shared include stays model-agnostic.

Do in workspace root (not inside `promptforge/` crate tree):

- [common.toml](c:\Users\Vinnie\src\cursor\common.toml): add only the device

```toml
[[device]]
id = "local-gpu"
type = "local"
```

- [gemma.toml](c:\Users\Vinnie\src\cursor\gemma.toml): after include, add lane + bind

```toml
[[device.lane]]
device = "local-gpu"
id = "generative"
concurrency = 3

[[local_model]]
# ...existing fields...
device = "local-gpu"
lane = "generative"
```

- [qwen.toml](c:\Users\Vinnie\src\cursor\qwen.toml): same shape with `concurrency = 10`.

Reason: include merge appends/replaces by id; a leaf that loads alone carries its own lane. Putting `gemma`/`qwen` lane ids in `common.toml` would couple every profile to those model names.

Leave [profiles/analytical.toml](promptforge/profiles/analytical.toml) generative concurrency at 1.

No automated test in this commit. Verify by reading the three TOML files after the commit. Manual gateway restart is post-plan ops, not this commit.

### Step 3 - Concurrent fanout

Intent: every list item starts as its own task; replies stay list-ordered; first error aborts siblings.

Do:
- Refactor [fanout.rs](promptforge/crates/promptforge-core/src/fanout.rs): extract `run_one_arm`; spawn all arms on `tokio::task::JoinSet` inside the existing `block_in_place` / `block_on` bridge; clone `StoreRef` and `GatewayClient` per task.
- Replace shared `mut turns` with `AtomicU32` fetch_add for debug turn indices.
- On first `Err`, call `abort_all` and return that error; on success return `Vec<String>` in item order.
- Update [design-core.md](promptforge/crates/promptforge-core/design-core.md): concurrent arms; ordered replies; fail-fast abort; remove "parallel fanout" from non-goals; keep nested fanout as non-goal.
- Update fanout module docs, [README.md](promptforge/README.md), [STATUS.md](promptforge/STATUS.md).

Test: existing suite filters still pass - `cargo test -p promptforge-core fanout` and `cargo test -p promptforge-core-tests fanout`.

Decision: fail-fast aborts siblings (not wait-all). Falsifier: authors need every arm's partial result on failure - then switch to wait-all in a later commit.

Decision: no fanout-internal concurrency cap; gateway queue throttles. Falsifier: CPU-bound preamble storms without model calls - then add a host cap.

### Step 4 - Concurrency regression tests

Intent: a test fails if fanout becomes sequential-only again or if `--parallel` ignores the lane.

Do:
- Add a core-tests (or core unit) fixture: fanout of at least 2 preamble-only arms; each writes a distinct store path; assert both paths exist and reply order matches list order.
- If [tests/it/main.rs](promptforge/crates/promptforge-gateway/tests/it/main.rs) already has a blocking-upstream concurrency harness, add one case that concurrency 2 admits two in-flight requests; if that harness cannot express it without a large rewrite, skip the IT and record the skip reason in the commit message (gateway unit tests from step 1 remain the lock).

Test command: `cargo test -p promptforge-core-tests fanout` and `cargo test -p promptforge-gateway`.

## Execution protocol

Follow vibe-rulebook. Per step:

1. Dispatch **coder** subagent: plan path `c:\Users\Vinnie\.cursor\plans\fanout_and_gateway_concurrency_75956c94.plan.md`, step number only; instruct it to grep `<code-review>` is not its job. Coder implements code + tests + docs for that step; runs only the step's named test filter; returns under 500 tokens.
2. Main commits with a message that states why.
3. Dispatch **review-and-fix** subagent: same plan path + step; apply vibe `<code-review>` and rust-rulebook non-negotiables; overwrite `cabinet/_scratch/vibe-review-fanout-concurrency/vibe-review.md`; one fix round; return under 1000 tokens.
4. Main amends if the tree is dirty and amend rules allow; otherwise new fix commit (vibe rule 7).
5. **Verify** on steps 1, 3, 4 and on any step that dirtied after review: `cargo fmt --all --check`, `cargo clippy -p promptforge-gateway -p promptforge-core --all-targets -- -D warnings`, then the step test filters. Verify returns one line: pass, or fail + log path. Main does not read the log body.

Main context may hold: this plan, step number, commit hashes, bounded git lines, scratch paths, Verify status line. Main must not hold: source dumps, full diffs, build logs, `vibe-review.md` body.

## Data flow

| Step | Needs | Produces |
|---|---|---|
| 1 | Existing `local_model_concurrency` | llama `--parallel P` = lane P |
| 2 | Step 1 semantics | leaf TOMLs own lane P; common has device only |
| 3 | Cloneable store/client; JoinSet | concurrent arms, ordered replies |
| 4 | Steps 1 and 3 behavior | failing tests if concurrency regresses |

## Out of scope

- Nested fanout
- Fanout-internal concurrency cap
- Ordered observer frame buffering
- Auto-tuning P from VRAM
- Live model CI

## Local verify after the plan finishes

1. Restart gateway with `gemma.toml` or `qwen.toml`.
2. Confirm process args show `--parallel 3` or `--parallel 10` respectively.
3. Run briefer; stderr shows multiple `Fanout arm started` lines before the first `Fanout arm finished`.

## Binding restatement

Ship concurrent fanout and lane-driven `--parallel`. Gemma leaf P = 3. Qwen leaf P = 10. `common.toml` names no model. One testable commit per step under vibe-rulebook. Rust changes obey rust-rulebook.
