---
name: Harness debt removal
overview: "Remove the debt the 21 commits since upstream/master (594eb22a7..b29cd4fce) added to runvil: one STT startup test hang introduced by 1b25e9ead, plus two pieces of engine test-support surface the internal-test-harnesses plan left orphaned. Also, at the user's request, close four rejected leads: the duplicate run-log seed parser, three stale docs, the shared store in the happens-before sweeps, and the host-only Engine graph guard."
todos:
  - id: d1-4-run-log-seed
    content: "D1-4: run-log to_seed parses through RunSeed's serde Deserialize, keeps LogError::Corrupt, drops hex_digit; verify with run-log nextest (append.rs seed round-trip and refusal)"
    status: completed
  - id: stale-docs
    content: "Docs: rewrite Scenario::cancel_when doc (D1-13), lua vm_tally comment, and the RandomState::new ban reason in all three Engine clippy.toml files"
    status: completed
  - id: d1-18-fresh-store
    content: "D1-18: add the store-probe regression test, confirm it fails, then give every happens-before run its own store and check files per run; stop if an existing trace fails"
    status: completed
  - id: d1-7-target-all
    content: "D1-7: add the wasm32-only graph fixture test, confirm it fails, then add --target all to forbidden_engine_graph; stop if the real workspace reports a new reach"
    status: completed
  - id: d1-1-stt-hang
    content: "D1-1: extract construct_with_stuck_interim, add the failing regression test, then add move scenario + drop(while_blocked) before join; verify with nextest -p gateway-stt-engine"
    status: completed
  - id: d1-2-d1-3-trim
    content: "D1-2/D1-3: complete_now returns Result directly (doc + 2 call sites); delete engine Seeded::pick and its self-test lines; verify engine nextest + clippy with and without test-support"
    status: completed
  - id: exit-gate
    content: "Exit gate: full AGENTS.md nextest, clippy (CARGO_BUILD_WARNINGS=deny), and docs gate"
    status: completed
isProject: false
---

# Debt Removal: internal-test-harnesses range

<product-contract>

## Product Requirements

- Scope and target work
  - Repository: `runvil`, branch `master`.
  - Baseline: `upstream/master` = 594eb22a7d0a0c2cd8e921b0081ff1959c5b6267. This is the local tracking ref as of the last fetch.
  - Endpoint: `HEAD` = b29cd4fce29a69c2c2de605d39b6bc4b90b24185. The worktree was clean at analysis time.
  - Target: 21 linear commits. Nineteen (47c5d8025 through b9126c55d) belong to the plan `vibe/2026-10-14-4-internal-test-harnesses.md`. Two follow-ups have no `Plan:` trailer: 1b25e9ead ("Let STT startup tests decide when the deadline expires") and b29cd4fce ("Sort Engine crate discovery so guard findings keep one order").
- Cleanup goals
  - A missed construction rendezvous in the STT startup cleanup tests returns promptly, as the fixture documents, instead of hanging until the test runner kills it (D1-1).
  - Remove test-support surface that outlived the plan step that needed it (D1-2, D1-3).
  - User-expanded scope: these are leads the debt criteria rejected, which the user chose to close anyway.
    - The run log parses its persisted seed with the same parser that defines the format (D1-4).
    - Three docs state what the code does: `Scenario::cancel_when` (D1-13), the lua `vm_tally` comment, and the `RandomState::new` ban reason.
    - Every run in the happens-before sweeps starts from an empty store, so no run can read a file another run wrote (D1-18).
    - The Engine resolved-graph guard checks every platform, not just the host (D1-7).
- Non-goals
  - The remaining 17 rejected candidates, and any other pre-existing debt.
  - Redesigning the test harnesses, the `Scenario` stepper, or the duplicated `TempRoot` and `Seeded` copies.
  - Line-count targets. Deleted lines are context, never a success criterion.
- Success criteria
  - The new D1-1 regression test fails at the endpoint on its named assertion and passes after the fix.
  - The four existing `run_timed_out_construction` callers still pass.
  - The engine suite, clippy, and docs gates pass with `CARGO_BUILD_WARNINGS=deny`, with `complete_now` returning only the answer and with no `pick` left on the engine `Seeded`.
  - The D1-18 store-probe test and the D1-7 non-host graph test each fail at the endpoint on their named assertion and pass after the change.
  - No existing happens-before trace and no real-workspace graph guard result changes. If one does, that's a newly exposed defect: stop and report it.

## Functional Specification

### Debt Inventory

- Debt added
  - D1-1 (introduced, 1b25e9ead): STT startup tests hang instead of failing when the construction rendezvous misses.
    - Evidence:
      - `SttEngine::new_with_stuck_role` in `crates/gateway/stt/engine/src/engine.rs` (lines 36-50) blocks the stuck role on an untimed `expire.recv()` at line 44. `recv()` returns only on a send, or when every sender has dropped.
      - In `run_timed_out_construction` (`crates/gateway/stt/engine/src/test_fixtures/tests-scenario-cleanup.rs` lines 45-65), the scenario `|| { let _ = expire.send(()); }` is not a `move` closure. Under edition 2024 it only borrows the sender, so the sender lives in the caller's frame.
      - `ScriptedModelFactory::with_construction_blocked` (`crates/gateway/stt/engine/src/test_fixtures.rs` lines 109-157) has a failure branch at lines 137-143. On a missed rendezvous it drops `blocks`, then joins the constructor, and never calls or drops `while_blocked`.
      - Once released, the constructor reaches `wait(&interim_init, DecodeMode::Interim)` (engine.rs line 98, or line 84 on the final-spawn error path), which blocks on `expire.recv()` forever. The join never returns.
    - Contract contradicted: the fixture documents "a bounded scenario". It takes explicit `rendezvous_timeout` and `result_timeout` parameters and returns `None` for a missed rendezvous.
    - Impact:
      - Four tests call the helper: `tests-scenario-cleanup-construction.rs` lines 10, 31, 49, and 69.
      - When the 1 s rendezvous misses, each becomes a roughly 180 s nextest timeout (`.config/nextest.toml`: `slow-timeout = { period = "60s", terminate-after = 3 }`). The timeout carries no message about the missed rendezvous. Under plain `cargo test` the test never ends.
      - Before 1b25e9ead, the 20 ms startup deadline let the constructor return on its own.
    - Reversal cost: low. The change is confined to test fixtures, with no product surface.
    - Target state: every return path of `with_construction_blocked` stays bounded, including when the scenario owns the release signal.
- Cheap fixes
  - D1-2: `ScriptedChat::complete_now` still returns a delay that no caller uses.
    - Origin: 936367d1e added the `(Duration, Result<Completion, CompletionError>)` return so the async wrapper could wait out the delay. 0e5614a6a deleted that wrapper.
    - Current callers: there are only two, `crates/runvil-internal/engine/src/test_support/scenario-driving.rs` line 73 and `crates/runvil-internal/engine/benches/models_loop.rs` line 109. Both bind the delay to `_`.
    - Reservation: the plan reserved the delay only "until Step 15 deletes" the wrapper. No `Deferred:` trailer reserves it.
    - The signature is at `crates/runvil-internal/engine/src/test_support/scripted-chat.rs` lines 313-346.
  - D1-3: the engine `Seeded::pick` is kept alive only by its own self-test.
    - Origin: 936367d1e created it, and plan Step 7 kept every method alive through the self-test "before Step 14 uses it". Step 14 (6409ac358) used `shuffle` and never `pick`.
    - Current callers: the only call is in the self-test, at `crates/runvil-internal/engine/src/test_support/seeded.rs` line 85. The types crate's separate `Seeded` copy has real `pick` callers in `types/src/untrusted/tests.rs` (lines 238 and 393) and is out of scope.
    - `seed()` stays. The plan specifies it, and the self-test uses it to build the replay generator.
  - D1-2 and D1-3 share one cause: test-support surface sized for an intermediate plan step outlived that step.
- Exposed pre-existing debt: none.
- User-expanded scope: rejected by the debt criteria, added by the user
  - D1-4: the run log keeps a near-copy of the seed parser (from 7a46c075a).
    - The writer stores `meta.seed.to_string()`, which is `RunSeed`'s `Display` in `crates/runvil-internal/types/src/seed.rs`.
    - The reader parses it with its own `to_seed` and `hex_digit` in `crates/workshop/run-log/src/read.rs` (lines 122-154). These repeat `parse_hex` and `nibble` (seed.rs lines 127-153) line for line.
    - `RunSeed` already has a public serde `Deserialize` that runs `parse_hex` (seed.rs lines 64-71), and the run log already depends on `serde_json`.
    - Risk: one persisted format with two parsers in two crates. They agree today and drift silently if the format changes.
  - D1-13: the `Scenario::cancel_when` doc (`crates/runvil-internal/engine/src/test_support/scenario.rs` lines 301-305) says the cancel "lands before the next step or while it runs".
    - What the code does: `Driving::step_ordered` (`scenario-driving.rs` line 174) joins the canceller through `await_cancel` only when a step answered nothing and held nothing new. A cancel can therefore land several steps after its signal.
    - Every current caller is unaffected. A future caller that trusts the doc could write a racy test.
  - The lua `vm_tally` doc is stale.
    - `crates/runvil-internal/lua/src/vm.rs` line 123 still says "(the tokio driver is `current_thread`)".
    - 0e5614a6a deleted the Engine's tokio driver. The thread-local tally is still correct, because the serial `Scenario` and the rig step a run on the test's own thread.
  - The `RandomState::new` ban reason points production code at a test-only type.
    - The three Engine files each have the ban at line 29: `crates/runvil-internal/clippy.toml`, `crates/runvil/clippy.toml`, and `crates/runvil-plugin/clippy.toml`.
    - Each says "use `Seeded` instead". Both `Seeded` copies are `#[cfg(test)]` and crate-private, so non-test Engine code that hits the ban is pointed at a type it can't reach.
  - D1-18: the happens-before sweeps share one store across runs.
    - `trace_scenario` (`crates/runvil-internal/engine/src/execute/tests/happens_before.rs` lines 243-247) overrides `Scenario`'s fresh per-run store with `.vfs(store.vfs())`. All three serial batchings and all three shuffled seeds of a trace then run over one `TestStore`.
    - Risk: a run can read a file an earlier run wrote. For example, Summarize's read of `gathered.md` in `PIPELINE`, or the read of `partial.txt` in `TRACE_11`, would still pass in an interleaving whose own write never happened. That weakens what the sweep proves.
    - This predates the target. 6409ac358 rewrote the file and kept the sharing.
  - D1-7: the resolved-graph guard checks the host platform only.
    - `forbidden_engine_graph` (`crates/build-xtask/src/engine_deps.rs` lines 183-190) passes no target selection to `cargo tree`, so the host-only default applies.
    - A forbidden crate (one of `FORBIDDEN`, lines 38-45) that's reachable only through another platform's dependency edge is never reported. Nothing reaches one today.
- Rejected candidates, 21 in total. Four of them (D1-4, D1-7, D1-13, D1-18) are in scope above at the user's request.
  - Residual-but-acceptable, 11: D1-4, D1-5, D1-6, D1-9, D1-10, D1-11, D1-14, D1-15, D1-16, D1-17, D1-24.
    - Each is deliberate and documented: duplication forced by boundary rules, user-requested public surface, a no-migration layout bump with a non-destructive set-aside, test-only helpers, or resolved deferrals.
  - Weak or speculative, 6: D1-7, D1-8, D1-12, D1-13, D1-20, D1-22.
    - Each gap is real in principle, but no current code reaches it: platform-only dependency edges, unbanned clock paths, `below(0)`, the timing wording in the `cancel_when` doc, the plan's line-count reporting, and YAML trimming.
  - False, 2:
    - D1-21: no public `Debug` path exposes the seed or the nonce.
    - D1-23: there was only one `read_dir` ordering episode, so the repeat-cause criterion isn't met.
  - Unrelated pre-existing, 2:
    - D1-18: the happens-before sweeps share one store.
    - D1-19: the bench records each round inside the measured loop.

</product-contract>
<implementation-contract>

## Technical Design

- STT test fixtures (D1-1). No production code changes, and `new_with_stuck_role` stays as it is.
  - In `with_construction_blocked`'s failure branch (`test_fixtures.rs` lines 137-143), add `drop(while_blocked);` before `constructor.join()`. That releases any signal the scenario owns before the fixture waits on construction. The scope closure already captures `while_blocked` by value, so this compiles beside the success path's `while_blocked()`.
  - In `tests-scenario-cleanup.rs`, extract the body of `run_timed_out_construction` into a helper that both the existing callers and the regression test go through:

```rust
fn construct_with_stuck_interim(
    decoder: &ScriptedDecoder,
    rendezvous: Duration,
    late_by: Duration,
) -> Option<(Result<SttEngine, TranscribeError>, ())> {
    let factory = ScriptedModelFactory::new(decoder.clone());
    let (expire, expired) = std::sync::mpsc::channel();
    factory.with_construction_blocked(
        rendezvous,
        WAIT,
        move |factory| {
            std::thread::sleep(late_by);
            SttEngine::new_with_stuck_role(factory, policy(), DecodeMode::Interim, &expired)
        },
        move || {
            let _ = expire.send(());
        },
    )
}
```

  - The scenario must be a `move` closure so that it owns the sender. Neither edit works alone. With `move` but no drop, the fixture still holds the scenario during the join. With the drop but no `move`, the borrowed sender still lives in the caller's frame.
  - `run_timed_out_construction` becomes `construct_with_stuck_interim(decoder, WAIT, Duration::ZERO).expect(..)` and keeps its existing assertions.
  - The STT engine is a gateway crate, outside the Engine clippy determinism bans. The sibling test `construction_rendezvous_timeout_releases_a_late_arrival_and_permits_a_follow_up` already uses `std::thread::sleep` there.
- Engine test support (D1-2, D1-3)
  - Change `ScriptedChat::complete_now` to return `Result<Completion, CompletionError>`. Keep the internal `reply.delay()` comparison against `timeout`.
  - Rewrite the doc at scripted-chat.rs lines 313-321 so it no longer mentions returning a delay. The `answer` doc at line 147 and the intra-doc links in `scenario-serial.rs` lines 15-18 still hold.
  - Simplify the two call sites to `let answer = chat.complete_now(..);`.
  - The function is `pub` only inside the `test-support`-gated `test_support` module. The crate's own `models_loop` bench is the sole consumer of that feature, and the facade's `public-api.txt` is unaffected.
  - Delete `Seeded::pick` (seeded.rs lines 47-50), and delete its self-test lines 84-86 (`items`, `picked`, and the `contains` check).
- Run log seed parsing (D1-4)
  - Replace the body of `to_seed` in `crates/workshop/run-log/src/read.rs` with a parse through `RunSeed`'s own `Deserialize`, and delete `hex_digit`:

```rust
/// A stored `seed` column, parsed as `RunSeed`'s serde form defines it.
fn to_seed(text: String) -> Result<RunSeed, LogError> {
    serde_json::from_value(serde_json::Value::String(text))
        .map_err(|error| LogError::Corrupt(format!("seed: {error}")))
}
```

  - The caller at line 27 becomes `to_seed(row.get::<String>(3)?)?`.
  - Keep the explicit `LogError::Corrupt` mapping. The blanket `From<serde_json::Error>` in `error.rs` would turn a bad seed into `LogError::Payload`, which changes the error class.
  - No API changes. `RunSeed` gains no `FromStr` and no public parse error, which is what the original Decision Record wanted to avoid.
  - The refusal message becomes "seed: a run seed is 64 lowercase hex characters, found ..." (from `REQUIRED` in seed.rs line 21).
- Docs
  - `Scenario::cancel_when` (scenario.rs lines 301-305): replace the timing sentence with what the stepper guarantees. Suggested wording: "The thread waits for that effect untimed, so the cancel lands at some step after the signal, not necessarily the next. The stepper joins the thread once a step answers nothing and holds nothing new, so a run left waiting only on held effects always sees the cancel before it moves again. Use it only where the outcome is the same whichever step the cancel lands in." Keep the closing clause about joining before the trace is handed back.
  - `vm_tally` (lua `vm.rs` lines 120-126): replace "(the tokio driver is `current_thread`)" with the current fact, for example "(the serial `Scenario` and the rig step it there)".
  - `RandomState::new` ban reason: change it identically in all three Engine `clippy.toml` files to name reachable replacements. For example: "use a `BTreeMap` or `BTreeSet` where order is observable, or `Seeded` in tests, instead, because std's `RandomState` seeds itself from the OS, so its order differs on every run".
    - Only the reason text changes. `DETERMINISM_BANS` in `crates/build-xtask/src/engine_clippy.rs` lists paths, not reasons, and `ban_violations` requires only a non-blank reason. No build-xtask test pins the old text.
- Happens-before sweeps (D1-18)
  - Give every run its own `TestStore`. `trace_scenario(source, store)` keeps its signature. What changes is that the sweep helpers make a fresh store for each run instead of capturing one store per test:
    - `serial_results` and `shuffled_results` take `impl Fn(&TestStore) -> Scenario` and build `TestStore::new()` inside each batching and seed iteration.
    - `serial_results` returns each run's store beside its result, because `drive_batched` returns no store. Shuffled runs read through `rig.trace().read(path)`.
    - `assert_every_interleaving_reads` and `assert_every_interleaving_conflicts` take the same `impl Fn(&TestStore) -> Scenario`.
  - Move per-test post-run file checks into every run. The seven-arm fanout and freeform tests check `research.md == SEVEN_PARTS` once, after the sweep. Instead, check it for each batching's store and each seed's trace. A small optional `(path, contents)` argument, or a sibling helper, both work.
  - Update the callers: the seven tests in `happens_before.rs` (lines 328-430) and the three in `happens_before-concurrency.rs` (lines 88, 105, and 131). Each `|| trace_scenario(X, &store)` becomes `|store| trace_scenario(X, store)`, and each per-test `let store = TestStore::new();` goes away.
  - Rewrite `trace_scenario`'s doc ("so every run of one trace shares the files the test reads afterward") to say each run gets an empty store.
- Engine graph guard (D1-7)
  - In `forbidden_engine_graph` (`crates/build-xtask/src/engine_deps.rs` lines 184-189), add `"--target", "all"` to the `cargo tree` arguments, and extend the doc and the `Reached` rendering at line 113 ("all features; ... edges") to say "all targets".
  - Don't add `--locked`. The graph fixtures from `graph_workspace` (`engine_deps-tests.rs` lines 224-249) have no `Cargo.lock`, and `cargo tree --locked` errors when the lock file is missing. On the real workspace, the gate's outer `--locked` already refuses a stale lock.

</implementation-contract>
<verification-contract>

## Testing Plan

- D1-1 regression test (focused)
  - Add a test named `missed_rendezvous_with_a_stuck_role_returns_instead_of_hanging` to `tests-scenario-cleanup-construction.rs`. It runs `construct_with_stuck_interim(&late, Duration::from_millis(10), Duration::from_millis(50))` on a spawned thread that sends `.is_none()` over a channel.
  - The test then checks `assert_eq!(done_rx.recv_timeout(WAIT * 10), Ok(true), "a missed rendezvous returns None instead of hanging on the stuck role")`, followed by `assert!(decoder.wait_until_worker_dropped(WAIT))` and `run_timed_out_construction(&decoder)`, the same follow-ups as the sibling tests.
  - The 10 ms rendezvous with a 50 ms late start reuses the sibling test's existing way of forcing a miss.
  - Failing assertion at the endpoint: write the helper first with the endpoint's borrowing `|| { .. }` scenario and no fixture drop. The helper thread then blocks in `constructor.join()` (`test_fixtures.rs` lines 139-141) while the constructor waits in `expire.recv()` (`engine.rs` line 44). `recv_timeout` returns `Err(RecvTimeoutError::Timeout)`, so the `assert_eq!` fails after 10 s.
  - If that assertion already passes at the endpoint, D1-1 is false. Keep the test and skip both fix edits.
- D1-1 integration: run `cargo nextest run --locked -p gateway-stt-engine --all-features`. The test fixtures compile only under the crate's `test-fixtures` feature, so without the flag nextest selects no tests. All four `run_timed_out_construction` callers must pass, along with `parked_final_construction_cleans_up_the_initialized_interim_worker`, which doesn't use the helper.
- D1-2 and D1-3 are dead surface, not incorrect behavior, so they get no regression test. Removal is proven by compiling the changed code:
  - Run `cargo nextest run --locked -p runvil-engine --all-features`.
  - Run `cargo clippy -p runvil-engine --all-targets --all-features` with `CARGO_BUILD_WARNINGS=deny`. This also builds the `models_loop` bench, which requires `test-support`.
  - Run `cargo clippy -p runvil-engine --all-targets` with `CARGO_BUILD_WARNINGS=deny`, without the feature.
  - Any remaining `pick` caller or tuple destructure fails to compile.
- D1-4 is a refactor, not a behavior fix, so it gets no new regression test. The existing test in `crates/workshop/run-log/tests/it/append.rs` (lines 268-313) already protects both sides:
  - The stored column equals `seed.to_string()`.
  - An overwritten `'not a seed'` is refused as `LogError::Corrupt` with a message containing "seed".
  - Verify with `cargo nextest run --locked -p workshop-run-log`, plus clippy for that package. It isn't one of the excluded workshop crates, so the workspace clippy run covers it.
- Docs: no tests. Verify with the docs gate and with `cargo test -p build-xtask`. The latter confirms the Engine clippy guard still accepts all three `clippy.toml` files once the ban reason changes.
- D1-18 regression test (focused)
  - Add a trace `STORE_PROBE` to `happens_before.rs`, built from the same frontmatter and `## Main` skeleton as the others, with the body `local seen = store.exists('probe.txt')`, then `store.write('probe.txt', 'x')`, then `if seen then return 'present' end return 'absent'`.
  - Add a test `every_interleaving_starts_from_an_empty_store`. Write it first in the endpoint form: `let store = TestStore::new(); assert_every_interleaving_reads(|| trace_scenario(STORE_PROBE, &store), "absent");`.
  - Failing assertion at the endpoint: the first batching, `Batching::OnePerStep`, writes `probe.txt` into the shared store. The second, `Batching::AllAtOnce`, then sees it and returns `"present"`. The `assert_eq!(ok_text(&result), Some(expected), "under {batching:?}: {result:?}")` in `assert_every_interleaving_reads` (lines 297-301) fails under `AllAtOnce`.
  - After the change, the test reads `assert_every_interleaving_reads(|store| trace_scenario(STORE_PROBE, store), "absent")` and passes.
- D1-18 integration
  - Run `cargo nextest run --locked -p runvil-engine --all-features`. Every existing happens-before and concurrency trace must still pass under per-run stores.
  - If one fails, an earlier run's files were masking an ordering defect. Stop and report it. Do not weaken the assertion, and do not restore the shared store.
- D1-7 regression test (focused)
  - Add a test `a_forbidden_crate_reached_only_on_another_platform_is_reported` to `crates/build-xtask/src/engine_deps-tests.rs`, modeled on `a_forbidden_crate_reached_through_a_dev_dependency_is_reported` (lines 259-287).
  - Its fixture: `vendor/driver` depends on the `tokio` stand-in only under `[target.wasm32-unknown-unknown.dependencies]`, and `crates/runvil-internal/engine` depends on `driver` normally. The manifest scan therefore finds nothing, and only an all-targets graph reaches `tokio`.
  - Failing assertion at the endpoint: host-only `cargo tree` never follows the wasm32 edge, so `graph_findings` returns nothing and `assert_eq!(found.len(), 1, "{found:?}")` fails with `[]`.
  - After `--target all`, it finds one finding naming `runvil-engine`, the resolved graph, and `tokio`.
- D1-7 integration
  - Run `cargo test -p build-xtask` on the Windows dev host and in Linux CI. Every existing graph fixture test must pass unchanged, and the real-workspace Engine guard must report nothing new.
  - If the real workspace now reaches a forbidden crate through a non-host edge, stop and report it. Removing that edge changes a dependency, which is the user's call.
- Architecture: no new structural checks. The existing build-xtask guards (`cargo test -p build-xtask`) are reused, and only the graph guard's `cargo tree` arguments change.
- Exit: the full gate from `AGENTS.md`.
  - The workspace nextest and clippy runs, and the separate workshop runs, all with `CARGO_BUILD_WARNINGS=deny`.
  - The docs gate (`cargo doc ... --all-features` with `RUSTDOCFLAGS="-D warnings"`), because `complete_now`'s doc and its intra-doc links change.

</verification-contract>
<decision-record>

## Decision Record

- Reversible decisions and consequences
  - D1-1 gets two edits: the fixture drops the unused scenario before the join, and the helper's scenario becomes a `move` closure. The fixture fix covers every caller whose scenario owns a release signal. Confidence: high, because the hang follows from the closure capture rules and the fixture's control flow.
  - D1-1's regression test goes through an extracted helper that the four existing callers also use. That way the failing test exercises both edits, not just the fixture half. The cost is about 15 lines of test plumbing.
  - The regression test bounds its own wait with a spawned thread and `recv_timeout`. A hang then fails in about 10 s on a named assertion, instead of the roughly 180 s nextest timeout.
  - D1-2 returns the `Result` directly. Every caller gets simpler, and nothing reads the delay.
  - D1-3 deletes the engine `pick` and keeps `seed()`. The engine and types `Seeded` copies then differ by one method, which is fine because they are independent copies by design.
  - D1-4 parses through `RunSeed`'s serde `Deserialize` and maps the error to `LogError::Corrupt` explicitly. The format then has one parser, and the error class stays the same. Confidence: high, because the existing append test pins both the stored form and the refusal.
  - D1-18 makes a fresh store per run inside the sweep helpers. Test bodies keep their shape, and every run's files are checked against that run alone. Confidence: medium, because an existing trace that fails under fresh stores would turn this into a defect investigation.
  - D1-7 adds `--target all` only. Confidence: medium, because a non-host reach in the real workspace would stop the work and need your decision.
- User-resolved architecture choices
  - The user widened the scope to D1-4, D1-7, D1-13, D1-18, and the two related doc leads (the `vm_tally` comment and the `RandomState::new` ban reason).
  - None of these remediations touches a public interface, a persisted or wire format, component ownership, dependency direction, or a trust boundary. The D1-4 refactor reads the same persisted form through the same parser the writer's format comes from.
- Rejected alternatives
  - Give `new_with_stuck_role` a timed `recv_timeout`. That brings back the clock dependence 1b25e9ead removed.
  - Tie the expiry to a drop guard the fixture holds. It works, but it's a larger change to a fixture that other tests share.
  - Extend `construction_rendezvous_timeout_releases_a_late_arrival_and_permits_a_follow_up` in place. That test asserts `SttEngine::new` behavior, so a sibling test keeps each test to one claim.
  - Keep `pick` for symmetry with the types copy. The self-test then remains its only use, which defeats the plan's own dead-code rule.
  - Keep the `complete_now` tuple and document the delay as unused. The orphaned output would remain.
  - Give `RunSeed` a `FromStr`, or a public `parse` function, for D1-4. It adds public API to the facade, which the original Decision Record rejected, and serde already provides the parse.
  - Use `?` with the blanket `From<serde_json::Error>` in `to_seed`. A bad seed would become `LogError::Payload`, and the existing append test would fail.
  - Add `--locked` to the graph guard's `cargo tree` call. The fixture workspaces have no lock file, so every graph fixture test would fail, and the outer gate's `--locked` already protects the real lock.
  - Pass explicit triples, such as Linux, Windows, and macOS, instead of `all`. That's a list to maintain, and it misses any platform left off the list.
  - Keep the shared store and add a per-run cleanup. Deleting files between runs still leaves the VFS's other state shared, while `Scenario`'s default fresh store already isolates runs.
- Assumptions
  - `upstream/master` was not re-fetched, so the baseline is the local ref at 594eb22a7.
  - Every consequence was inferred from reading code. No build or test was run during analysis.
- Risks
  - The forced miss (10 ms rendezvous against a 50 ms late start) shares the sibling test's load sensitivity. On a stalled host, the rendezvous could still succeed and the test would fail with `Ok(false)`. That's the same flake class the sibling test already carries.
  - The analysis sampled the bodies of the roughly 230 converted test files rather than reading each in full, so a defect confined to one of them could have been missed.
  - `cargo tree --target all` needs package metadata for every platform's dependencies. A host whose cargo cache lacks non-host packages has to download them. Offline, the guard fails closed with `Violation::Unresolved`. If CI runs offline, add a `cargo fetch`, which fetches every platform by default, before the build-xtask tests.
  - Per-run stores could expose a trace that only passed because of an earlier run's files. That's the purpose of the change, but it can turn a cleanup into a defect report.

### Deferred and Out of Scope

- `TestVm` has no `plugins(..)` option, per the `Deferred:` trailer on f6b77007a. Revisit when a section-VM test needs plugins.
- D1-8: the clippy bans don't cover `Instant::elapsed`, `RandomState::default`, or `HashMap::new`. Revisit when Engine code starts using one of them.
- D1-9: the guard checks only that a ban reason isn't blank, not that it names a replacement. Revisit if a reason without a replacement lands.
- D1-19: the bench records every round inside its measured loop. Revisit if the bench numbers are used for decisions.
- The other 13 rejected candidates listed in the Debt Inventory.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build --locked -p gateway` (the workspace's only default member). Crate build scripts bundle the UIs with esbuild, so run `npm ci --prefix crates/workshop` and `npm ci --prefix crates/gateway/config-ui/ui` first, as CI does. The desktop app is a separate, explicit build: `cargo build --locked -p workshop`, or `cargo workshop` (the `.cargo/config.toml` alias for `run -p build-workshop --`).
- Focused test command pattern: `cargo nextest run --locked -p <package> --all-features <test-name-filter>`, or `-E 'test(<name>)'` for a nextest filterset; drop `--all-features` when the package is `workshop`, `workshop-server`, or `workshop-server-api`. Integration test binaries are named `it` in most crates and `suite` in `runvil` and `harness`, so `-E 'binary(suite) & test(<name>)'` narrows to one. The two criterion benches run each benchmark once with `cargo bench --locked -p runvil-engine --features test-support --bench models_loop -- --test` and `cargo bench --locked -p runvil-lua --features test-support --bench surface -- --test`.
- Component test command pattern: `cargo nextest run --locked -p <package> --all-features` (again without `--all-features` for `workshop`, `workshop-server`, and `workshop-server-api`).
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`. CI also runs `npm test --workspaces --if-present` in `crates/workshop`, `npm test` in `crates/gateway/config-ui/ui`, and, on the pinned nightly, `cargo nextest run --locked -p build-xtask --run-ignored only`.
- Linter command: `CARGO_BUILD_WARNINGS=deny cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features` and `CARGO_BUILD_WARNINGS=deny cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`. Companion gates: `cargo check -p gateway --no-default-features` (headless build shape), `cargo test -p build-xtask` (boundary and structural checks: product and container boundaries, the Engine dependency guard `engine_deps.rs`, the Engine clippy-config guard `engine_clippy.rs`, `engine_guards.rs`, `harness_bans.rs`, the doctest ban, the `test-support` leak guard, the unsafe allowlist, tidy, and the facade shape check), `node --test crates/workshop/ui/test/docs-claims.mjs` (wording and rulebook scan), `cargo +nightly-2026-09-05 xtask api --check` (facade surface against `crates/runvil/public-api.txt`; `--bless` rewrites the listing; the nightly is pinned in `crates/build-xtask/src/api/toolchain.rs`), and `cargo deny check` plus `cargo audit` (CI supply-chain job). Never run a standalone `cargo check --workspace` beside clippy.
- Formatter check command: `cargo fmt --all --check` (rustfmt `style_edition = "2024"`; the pre-commit hook runs it).
- Docs command: `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`, plus, under the same `RUSTDOCFLAGS`, the default-feature facade builds `cargo doc -p runvil --no-deps` and `cargo doc -p harness --no-deps` and the private-item build `cargo doc --locked --no-deps --all-features -p runvil-engine --document-private-items`. The Windows CI job also runs `cargo doc --locked --no-deps -p workshop-server --document-private-items`.
- Test placement and naming conventions: Unit tests take three forms, all common: a sibling file `<module>-tests.rs` wired at the end of the module with `#[cfg(test)] #[path = "<module>-tests.rs"] mod tests;` (a large one splits into `tests-<label>.rs` siblings); an inline `#[cfg(test)] mod tests { }`; or a `src/**/tests/` directory, as the engine's `src/execute/tests/` and `src/lua/tests/`, `runvil-lua`'s and `runvil-parser`'s `src/tests/`, and several gateway crates use. Integration tests compile as one binary per crate at `tests/it/main.rs` (most crates) or `tests/suite/main.rs` (`runvil`, `harness`), with one `mod` file per area plus a `support` module. Engine prompt fixtures live in `crates/runvil/tests/prompts/`. Test functions are snake_case sentences that state the behavior, such as `one_parent_cancel_ends_every_run_holding_one_of_its_children`; Miri-run tests carry a `miri_` prefix. Engine test support lives in `crates/runvil-internal/engine/src/test_support.rs` and `src/test_support/` (the serial `Scenario` driver, `ScriptedChat`, `Seeded`, `FnTool`, recording, the rig, and temp roots), behind `#[cfg(any(test, feature = "test-support"))]`; `runvil-parser` gates its `test_support` the same way, and `runvil-types`, `runvil-vfs`, and `runvil-model-client` keep theirs `#[cfg(test)]`-only. The `test-support` features of `runvil-engine`, `runvil-lua`, `runvil-parser`, and `runvil-plugin` are enabled only from `[dev-dependencies]` or bench `required-features`. Temporary directory names come from the test's name, the process id, and a counter.
- Directory map: `crates/` holds all code. Its top level is the public and shared layer: `runvil` (the Engine's public API facade), `runvil-plugin` (the Plugin API), `harness` and `harness-gateway-client`, the linked Plugins `plugin-web`, `plugin-mcp`, and `plugin-user-input`, `gateway-api-types` and `gateway-api-discovery`, `shared-error-source` and `shared-loopback`, `shared-ui` (a TypeScript and CSS package, excluded from the Cargo workspace), and the `build-*` tooling (`build-xtask` behind `cargo xtask`, `build-workshop` behind `cargo workshop`, plus `build-ui`, `build-user-guide`, `build-llama-cuda`, and `build-ceiling`). Four manifestless containers hold each family's private crates: `crates/runvil-internal/` (Engine: `types`, `parser`, `lua`, `engine`, `vfs`, `model-client`), `crates/harness-internal/` (`runner`, package `harness-runner`), `crates/gateway/` (the `app` binary and its family crates, with STT nested under `stt/`), and `crates/workshop/` (the Tauri `desktop` app, `server`, `server-api`, the tier crates such as `run-log` and `agents`, and the npm workspace members `ui`, `look`, and `platform`). Outside `crates/`: `guide/` is the user guide source; `prompts/` holds sample prompt programs; `vibe/` holds dated implementation plans (current ones at its top level, older ones in monthly subfolders, plus `scratch/`); `tools/` holds Node scripts with `.test.mjs` siblings; `.github/workflows/` holds CI; `.githooks/` holds pre-commit (fmt) and pre-push (wording scan, headless gateway check, clippy, cargo deny); `.cargo/config.toml` sets the `xtask` and `workshop` aliases and the Windows `rust-lld` static-CRT link; `.config/nextest.toml` sets timeouts and the heavy STT test group; `.cursor/rules/` holds two Workshop rulebooks; root `deny.toml`, `clippy.toml`, and `rustfmt.toml` configure the gates; `local/` is gitignored operator config, and `images/` holds README art.
- Component boundaries: the `runvil` facade depends on all six `runvil-internal` crates; `runvil-engine` depends on the other five; `runvil-parser` depends on `runvil-lua` and `runvil-types`; `runvil-lua` on `runvil-model-client`, `runvil-types`, and `runvil-vfs`; `runvil-model-client` on `runvil-types`; `runvil-types` and `runvil-vfs` are leaves. `runvil-plugin` depends on `runvil-types` and `runvil-vfs`, and each linked Plugin depends only on `runvil-plugin`. `harness-runner` depends on `runvil` and `runvil-plugin`; `harness` on `harness-runner`, `runvil`, and `runvil-plugin`; `harness-gateway-client` on `harness`, `plugin-web`, and `runvil`. Workshop's `run-log` and `agents` depend on `harness`, and `workshop-server` is the Workshop composition root, depending on the Harness crates, every linked Plugin, and every Workshop tier crate; `workshop-server-api` re-exports `workshop-server` for the `workshop` desktop crate. The gateway family depends only on `gateway-api-types`, `gateway-api-discovery`, `shared-*`, and its own crates, never on the Engine or Harness. `cargo test -p build-xtask` enforces the product and container boundaries, the Workshop tier graph, and lint inheritance.
- Conventions summary: Rust edition 2024 on the stable toolchain with resolver 3 and every dependency declared once in `[workspace.dependencies]`, each non-obvious choice explained by a comment. Workspace lints deny clippy `all` and `pedantic`, `unwrap_used` and `expect_used` (allowed in tests by `clippy.toml`), bare `allow` attributes (use `#[expect(..., reason = "...")]`), and `unsafe_code`; `missing_docs`, `missing_debug_implementations`, and `unreachable_pub` warn and the gate denies warnings. The root `clippy.toml` bans process-global installers outside binary entry points; `crates/runvil/`, `crates/runvil-internal/`, and `crates/runvil-plugin/` each hold an identical `clippy.toml` that repeats those entries and adds the Engine determinism bans (no sleep, wall clock, timed wait, or OS-seeded `RandomState`), and Engine crates never depend on `tokio`, `tokio-util`, `async-trait`, `reqwest`, `getrandom`, or `fastrand`, tests included. Doc comments hold no compiled code blocks. Source directories are flat: a subdirectory needs at least three files, otherwise modules sit beside the parent as `parent-label.rs` wired with `#[path]`. Every source file stays under the 500-line ceiling `build_ceiling::check` enforces from each crate's `build.rs`. Engine, Harness, Host, and Plugin are capitalized defined terms, and Engine crates say "the caller" instead of naming the Host. Comments state only non-obvious constraints. Error messages are written for model consumption and name required versus actual. Behavior changes ship with tests in the same change. JSON reaching a recorder or replay round-trips exactly with sorted keys. Text files are LF. Commit subjects are imperative sentence case, and a finished plan lands as a `Close plan: <slug>` commit.

</project-survey>
<execution-plan>

## Execution Instructions

Objective: remove the debt the 21 commits from `594eb22a7` to `b29cd4fce` added to runvil (the D1-1 STT startup hang, and the D1-2 and D1-3 orphaned engine test-support surface), and close the user-added leads (D1-4 duplicate run-log seed parser, the three stale docs including D1-13, D1-18 shared happens-before store, D1-7 host-only Engine graph guard). No public API, persisted format, or production behavior changes. Path: Full, four components, five steps. Line numbers are at `b29cd4fce`.

Component order. No work item depends on another, so the order follows risk: work that can't stop the run lands first, and the two items that may stop on a newly exposed defect (D1-18, then D1-7) land last, so every certain fix is committed before a possible halt.

1. STT startup fixtures (`gateway-stt-engine`), Step 1. First because D1-1 is the only introduced debt with a real failure mode, it sits in one gateway crate with no Engine dependency, and its false-lead gate still leaves a commit.
2. Run log seed parsing (`workshop-run-log`), Step 2. Second because it's a one-function refactor already pinned by an existing test, with no stop rule.
3. Engine test harness (`runvil-engine` test support and happens-before sweeps, plus one `runvil-lua` comment and the Engine `clippy.toml` ban reason), Steps 3 and 4. Two pieces built sequentially: the surface trim and all three doc fixes first, then per-run stores, because only the second can stop on an exposed ordering defect. Third because of that stop risk. The ban reason rides with the other stale docs so that no certain fix waits behind D1-7's stop rule.
4. Engine guards (`build-xtask`'s `engine_deps` graph guard), Step 5. Last because D1-7 may download non-host package metadata and may stop for a dependency decision that belongs to the user.

The plan's exit gate is not a separate step. Step 5 is the final step, so the Run Sequence verifies it at FULL scope, which runs the survey's build, formatter check, workspace and workshop clippy with `CARGO_BUILD_WARNINGS=deny`, docs gate, and both full-suite nextest runs.

<step-1>

### Step 1: Bound the stuck-role STT construction when the rendezvous misses [completed]

- Component: STT startup fixtures
- Tests:
  - `cargo nextest run --locked -p gateway-stt-engine --all-features -E 'test(missed_rendezvous_with_a_stuck_role_returns_instead_of_hanging)'`

Fixes D1-1, introduced by `1b25e9ead`. Paths are under `crates/gateway/stt/engine/src/`. Component scope also runs the whole `gateway-stt-engine` suite, which covers the plan's `cargo nextest run --locked -p gateway-stt-engine` (the crate's only feature is `test-fixtures`).

- Debt being removed
  - `SttEngine::new_with_stuck_role` (`engine.rs` lines 36-50) blocks the stuck role on an untimed `expire.recv()` at line 44, which returns only on a send or once every sender has dropped.
  - `run_timed_out_construction` (`test_fixtures/tests-scenario-cleanup.rs` lines 45-65) passes the scenario `|| { let _ = expire.send(()); }` without `move`. Under edition 2024 it only borrows the sender, so the sender lives in the caller's frame.
  - `ScriptedModelFactory::with_construction_blocked` (`test_fixtures.rs` lines 109-157) has a failure branch at lines 137-143. On a missed rendezvous it drops `blocks`, joins the constructor, and never calls or drops `while_blocked`. The released constructor reaches `wait(&interim_init, DecodeMode::Interim)` (`engine.rs` line 98, or line 84 on the final-spawn error path), blocks on `expire.recv()` forever, and the join never returns.
  - The fixture documents "a bounded scenario", takes explicit `rendezvous_timeout` and `result_timeout`, and returns `None` for a missed rendezvous. Today a miss becomes a roughly 180 s nextest timeout with no message about the rendezvous (`.config/nextest.toml`: `slow-timeout = { period = "60s", terminate-after = 3 }`), and plain `cargo test` never ends.
- Order of work, all in one commit
  1. In `test_fixtures/tests-scenario-cleanup.rs`, extract the body of `run_timed_out_construction` into `fn construct_with_stuck_interim(decoder: &ScriptedDecoder, rendezvous: Duration, late_by: Duration) -> Option<(Result<SttEngine, TranscribeError>, ())>`, shaped as in the implementation contract: rendezvous timeout `rendezvous`, result timeout `WAIT`, and a `move |factory|` constructor that sleeps `late_by` and then calls `SttEngine::new_with_stuck_role(factory, policy(), DecodeMode::Interim, &expired)`. At first keep the endpoint's borrowing scenario `|| { let _ = expire.send(()); }`. `run_timed_out_construction` becomes `construct_with_stuck_interim(decoder, WAIT, Duration::ZERO).expect("construction reaches the blocked scenario and its bounded result")` and keeps its `TranscribeError::InterimStartupTimedOut` and `decoder.wait_until_worker_dropped(WAIT)` assertions.
  2. Add the regression test below and run it. It must fail on its `recv_timeout` assertion after about 10 s. If it passes, D1-1 is false: keep the helper and the test, skip edits 3 and 4, and flag it in the return.
  3. Make the helper's scenario a `move` closure so it owns the sender: `move || { let _ = expire.send(()); }`.
  4. In `with_construction_blocked`'s failure branch (`test_fixtures.rs` lines 137-143), add `drop(while_blocked);` before `constructor.join()`. The scope closure already captures `while_blocked` by value, so this compiles beside the success path's `while_blocked()` call at line 149.
  - Both edits are required. With `move` but no drop, the fixture still holds the scenario during the join. With the drop but no `move`, the borrowed sender still lives in the helper's frame.
- Regression test, in `test_fixtures/tests-scenario-cleanup-construction.rs`
  - Name it `missed_rendezvous_with_a_stuck_role_returns_instead_of_hanging` and place it beside `construction_rendezvous_timeout_releases_a_late_arrival_and_permits_a_follow_up` (lines 14-32). It reuses that test's way of forcing a miss: a 10 ms rendezvous against a 50 ms late start.
  - Make `let decoder = ScriptedDecoder::new();` and a `std::sync::mpsc::channel()` as `(done_tx, done_rx)`. Move a `decoder.clone()` and `done_tx` into a detached `std::thread::spawn` thread, run `construct_with_stuck_interim` there on that clone with `Duration::from_millis(10)` and `Duration::from_millis(50)`, and send its `.is_none()` over `done_tx`. Never use `std::thread::scope` here: a scope joins the hung thread and turns the failure back into a hang.
  - Then assert, in order: `assert_eq!(done_rx.recv_timeout(WAIT * 10), Ok(true), "a missed rendezvous returns None instead of hanging on the stuck role");`, `assert!(decoder.wait_until_worker_dropped(WAIT));`, and `run_timed_out_construction(&decoder);`, the same follow-ups as the sibling tests.
  - Expected failure at the endpoint: the helper thread blocks in `constructor.join()` (`test_fixtures.rs` lines 139-141) while the constructor waits in `expire.recv()` (`engine.rs` line 44), so `recv_timeout` returns `Err(RecvTimeoutError::Timeout)` and the `assert_eq!` fails.
- Existing tests that must pass unchanged: the four `run_timed_out_construction` callers (`tests-scenario-cleanup-construction.rs` lines 10, 31, 49, and 69) and `parked_final_construction_cleans_up_the_initialized_interim_worker`, which doesn't use the helper.
- Must not change
  - No production code. `SttEngine::new_with_stuck_role` keeps its untimed `recv()`.
  - Rejected: giving `new_with_stuck_role` a `recv_timeout` (it brings back the clock dependence `1b25e9ead` removed); tying the expiry to a drop guard the fixture holds (a larger change to a shared fixture); extending `construction_rendezvous_timeout_releases_a_late_arrival_and_permits_a_follow_up` in place (it asserts `SttEngine::new` behavior, and a sibling test keeps one claim per test).
  - `std::thread::sleep` is allowed: the STT engine is a gateway crate outside the Engine clippy determinism bans, and the sibling test already uses it.
- Known risk: the forced miss shares the sibling test's load sensitivity. On a stalled host the rendezvous could still succeed and the test fails with `Ok(false)`. That's the existing flake class, not a defect in this step.
- Constraints: `tests-scenario-cleanup.rs` is 65 lines, `tests-scenario-cleanup-construction.rs` 108, and `test_fixtures.rs` 203; all stay under 500.
- Acceptance: the regression test failed on its named assertion before edits 3 and 4 (or D1-1 is flagged false), and afterward the whole `gateway-stt-engine` suite passes, including the new test and the four existing callers.

</step-1>

<step-2>

### Step 2: Parse the run log seed through `RunSeed`'s serde form [completed]

- Component: Run log seed parsing
- Tests:
  - `cargo nextest run --locked -p workshop-run-log --all-features -E 'test(a_32_byte_seed_survives_a_run_log_round_trip)'`

Closes D1-4, a user-added lead from `7a46c075a`. The file is `crates/workshop/run-log/src/read.rs`. Component scope also runs the whole `workshop-run-log` suite and its clippy with warnings denied; it isn't one of the excluded workshop crates.

- Debt being removed
  - The writer stores `meta.seed.to_string()`, which is `RunSeed`'s `Display` in `crates/runvil-internal/types/src/seed.rs`. The reader re-parses it with its own `to_seed` (line 124) and `hex_digit` (line 149), which repeat `parse_hex` and `nibble` (seed.rs lines 127-153) line for line. One persisted format with two parsers in two crates drifts silently if the format changes.
- Changes
  - Replace `to_seed` with `fn to_seed(text: String) -> Result<RunSeed, LogError>` whose body is `serde_json::from_value(serde_json::Value::String(text)).map_err(|error| LogError::Corrupt(format!("seed: {error}")))`, with the doc "A stored `seed` column, parsed as `RunSeed`'s serde form defines it." `RunSeed` already has a public serde `Deserialize` that runs `parse_hex` (seed.rs lines 64-71), and the run log already depends on `serde_json`.
  - Delete `hex_digit` and any import only the old parser used.
  - The caller at line 27, `seed: to_seed(&row.get::<String>(3)?)?,`, becomes `seed: to_seed(row.get::<String>(3)?)?,`.
- Must not change
  - Keep the explicit `LogError::Corrupt` mapping. Don't use `?` with the blanket `From<serde_json::Error>` in `error.rs`: a bad seed would become `LogError::Payload`, which changes the error class and fails the existing append test.
  - No API change: `RunSeed` gains no `FromStr`, no public `parse` function, and no public parse error. The stored form stays `seed.to_string()`.
- Expected error text: a refused seed now reads "seed: a run seed is 64 lowercase hex characters, found ...", from `REQUIRED` at seed.rs line 21.
- Tests: no new regression test, because this is a refactor. `a_32_byte_seed_survives_a_run_log_round_trip` in `crates/workshop/run-log/tests/it/append.rs` (line 265) already pins both sides and must pass unchanged: the stored column equals `seed.to_string()`, and an overwritten `'not a seed'` is refused as `LogError::Corrupt` with a message containing "seed".
- Constraints: `read.rs` is 172 lines.
- Acceptance: the append test passes unchanged, no `hex_digit` remains in `read.rs`, and the `workshop-run-log` suite and clippy pass at component scope.

</step-2>

<step-3>

### Step 3: Trim orphaned engine test-support surface and correct its stale docs [completed]

- Component: Engine test harness
- Tests:
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p runvil-engine --all-targets --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p runvil-engine --all-targets`
  - `cargo bench --locked -p runvil-engine --features test-support --bench models_loop -- --test`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --locked --no-deps --all-features -p runvil-engine --document-private-items`
  - `cargo test -p build-xtask`
  - `cargo fmt --all --check`

Fixes D1-2 and D1-3, which share one cause: test-support surface sized for an intermediate step of the internal-test-harnesses plan outlived that step. Also corrects three stale docs: D1-13 (`Scenario::cancel_when`), the lua `vm_tally` comment, and the `RandomState::new` ban reason in the three Engine `clippy.toml` files. Engine paths are under `crates/runvil-internal/engine/`. Focused scope also runs the suites of the touched packages with `--all-features`: `runvil-engine`, `runvil-lua`, and, through their `clippy.toml` files, `runvil` and `runvil-plugin`. The workspace docs gate runs at Step 5's FULL scope, so it isn't repeated here.

- D1-2: `ScriptedChat::complete_now` returns a delay no caller uses
  - Origin: `936367d1e` added the `(Duration, Result<Completion, CompletionError>)` return so an async wrapper could wait out the delay, and `0e5614a6a` deleted that wrapper. The plan reserved the delay only "until Step 15 deletes" the wrapper, and no `Deferred:` trailer reserves it.
  - In `src/test_support/scripted-chat.rs` (lines 313-346, signature at line 322), return `Result<Completion, CompletionError>`. Keep the internal `reply.delay()` comparison against `timeout`.
  - Rewrite its doc (lines 313-321) so it no longer mentions returning a delay. The `answer` doc at line 147 and the intra-doc links in `src/test_support/scenario-serial.rs` lines 15-18 still hold; leave them.
  - Simplify both call sites, which bind the delay to `_` today, to `let answer = chat.complete_now(..);`: `src/test_support/scenario-driving.rs` line 73 and `benches/models_loop.rs` line 109.
  - `complete_now` is `pub` only inside the `test-support`-gated `test_support` module, the crate's own `models_loop` bench is that feature's sole consumer, and the facade's `crates/runvil/public-api.txt` is unaffected.
- D1-3: the engine `Seeded::pick` is kept alive only by its own self-test
  - Origin: `936367d1e` created it, plan Step 7 kept it alive through the self-test "before Step 14 uses it", and Step 14 (`6409ac358`) used `shuffle`, never `pick`.
  - In `src/test_support/seeded.rs`, delete `pick` (lines 47-50) and its self-test lines 84-86 in `seeded_failure_messages_name_the_seed` (`items`, `picked`, and the `contains` check). Keep `seed()` (line 20): the plan specifies it, and the self-test uses it to build the replay generator.
  - Don't touch the types crate's separate `Seeded` copy, whose `pick` has real callers in `crates/runvil-internal/types/src/untrusted/tests.rs` (lines 238 and 393). Don't keep the engine `pick` for symmetry: the two copies are independent by design, and a method used only by its self-test defeats the plan's own dead-code rule.
- D1-13: the `Scenario::cancel_when` doc (`src/test_support/scenario.rs` lines 301-305)
  - It says the cancel "lands before the next step or while it runs" (line 303). In fact `Driving::step_ordered` (`scenario-driving.rs` line 174) joins the canceller through `await_cancel` only when a step answered nothing and held nothing new, so a cancel can land several steps after its signal. No current caller is affected, but a future caller that trusts the doc could write a racy test.
  - Replace the timing sentence with: "The thread waits for that effect untimed, so the cancel lands at some step after the signal, not necessarily the next. The stepper joins the thread once a step answers nothing and holds nothing new, so a run left waiting only on held effects always sees the cancel before it moves again. Use it only where the outcome is the same whichever step the cancel lands in." Keep the closing clause about joining before the trace is handed back. No code changes.
- The `vm_tally` comment (`crates/runvil-internal/lua/src/vm.rs` lines 120-126)
  - Line 123 still says "(the tokio driver is `current_thread`)", but `0e5614a6a` deleted the Engine's tokio driver. Replace it with the current fact, for example "(the serial `Scenario` and the rig step it there)". The thread-local tally is still correct; only the comment changes. It documents a private `#[cfg(feature = "test-support")]` module, so no docs build renders it; review confirms the wording.
- The `RandomState::new` ban reason
  - The three Engine `clippy.toml` files, `crates/runvil-internal/clippy.toml`, `crates/runvil/clippy.toml`, and `crates/runvil-plugin/clippy.toml`, each carry at line 29 the `std::collections::hash_map::RandomState::new` ban with the reason "use `Seeded` instead, because std's `RandomState` seeds itself from the OS, so its order differs on every run". Both `Seeded` copies are `#[cfg(test)]` and crate-private, so non-test Engine code that hits the ban is pointed at a type it can't reach.
  - Change only the reason, identically in all three files, to "use a `BTreeMap` or `BTreeSet` where order is observable, or `Seeded` in tests, instead, because std's `RandomState` seeds itself from the OS, so its order differs on every run". The three files stay identical.
  - `DETERMINISM_BANS` in `crates/build-xtask/src/engine_clippy.rs` lists paths, not reasons, `ban_violations` requires only a non-blank reason, and no build-xtask test pins the old text. `cargo test -p build-xtask` confirms the guard still accepts all three files.
- Tests: no regression test. D1-2 and D1-3 are dead surface, not incorrect behavior, so removal is proven by compiling: any remaining `pick` caller or tuple destructure fails to build. The Tests commands build the engine with and without `test-support`, build and run the `models_loop` bench once, build the docs that hold `complete_now`'s and `cancel_when`'s text and the `scenario-serial.rs` intra-doc links, and run the build-xtask guards over the edited `clippy.toml` files.
- Must not change: the behavior of `ScriptedChat`, the `Scenario` stepper, or `Seeded`. No harness redesign; the duplicated `TempRoot` and `Seeded` copies stay.
- Constraints: `scenario-driving.rs` is 492 lines, so its call-site edit must not add lines. `scripted-chat.rs` is 347, `scenario.rs` 452, `seeded.rs` 108, `vm.rs` 390, and `models_loop.rs` 163; all stay under 500. Doc comments hold no compiled code blocks.
- Acceptance: every Tests command passes with warnings denied, `complete_now` returns only the answer, the engine `Seeded` has no `pick`, neither doc claims the old cancel timing or the tokio driver, and all three `clippy.toml` files carry the same new ban reason.

</step-3>

<step-4>

### Step 4: Give every happens-before run its own store [completed]

- Component: Engine test harness
- Tests:
  - `cargo nextest run --locked -p runvil-engine --all-features -E 'test(every_interleaving_starts_from_an_empty_store)'`

Closes D1-18, a user-added lead that predates the target (`6409ac358` rewrote the file and kept the sharing). Paths are under `crates/runvil-internal/engine/src/execute/tests/`. This step ends the component, so component scope runs the formatter check, clippy, and the full `--all-features` suites of the packages Steps 3 and 4 touched.

- Debt being removed
  - `trace_scenario` (`happens_before.rs` lines 243-247) overrides `Scenario`'s fresh per-run store with `.vfs(store.vfs())`, and each test passes one `TestStore`, so all three serial batchings and all three shuffled seeds of a trace run over one store.
  - A run can read a file an earlier run wrote. For example, Summarize's read of `gathered.md` in `PIPELINE`, or the read of `partial.txt` in `TRACE_11`, would still pass in an interleaving whose own write never happened. That weakens what the sweep proves.
- Order of work, in one commit
  1. Add a trace `STORE_PROBE` to `happens_before.rs`, built from the same frontmatter and `## Main` skeleton as the other traces, with the Lua body `local seen = store.exists('probe.txt')`, then `store.write('probe.txt', 'x')`, then `if seen then return 'present' end return 'absent'`.
  2. Add the test `every_interleaving_starts_from_an_empty_store` in its endpoint form, `let store = TestStore::new(); assert_every_interleaving_reads(|| trace_scenario(STORE_PROBE, &store), "absent");`, and run it. It must fail under `Batching::AllAtOnce`: the first batching, `Batching::OnePerStep`, writes `probe.txt` into the shared store, so `AllAtOnce` sees it and returns `"present"`, and the `assert_eq!(ok_text(&result), Some(expected), "under {batching:?}: {result:?}")` in `assert_every_interleaving_reads` (lines 297-301) fails. If it passes, D1-18 is false: keep the probe and the test, skip edits 3-6, and flag it in the return.
  3. Make the sweep helpers build a fresh `TestStore::new()` for every run. `trace_scenario(source: &str, store: &TestStore) -> Scenario` keeps its signature.
     - `serial_results` (line 252) and `shuffled_results` (line 268) take `impl Fn(&TestStore) -> Scenario` instead of `impl Fn() -> Scenario`, and build the store inside each batching and each seed iteration.
     - `serial_results` returns each run's store beside its batching and result, because `drive_batched` returns no store. Shuffled runs read files through `rig.trace().read(path)`.
     - `assert_every_interleaving_reads` (line 295) and `assert_every_interleaving_conflicts` (line 312) take the same `impl Fn(&TestStore) -> Scenario`.
  4. Move post-run file checks into every run. The seven-arm fanout test (line 330) and the freeform test (line 351) check `store.read("research.md")` against `SEVEN_PARTS` once, after the sweep (lines 334 and 354). Check it in each batching's store and each seed's trace instead, through a small optional `(path, contents)` argument or a sibling helper.
  5. Update the ten tests and their twelve call sites. Each `|| trace_scenario(X, &store)` becomes `|store| trace_scenario(X, store)`, chained calls such as `.chat(&gateway)`, `.hold(asks("slow"))`, and `.limits(ceiling(n))` stay, and each per-test `let store = TestStore::new();` goes away. In `happens_before.rs` the tests start their store at lines 330, 351, 361, 374 (two closures, `TRACE_5_PASS` and `TRACE_5_FAIL`), 397, 406 (two closures, `TRACE_10_PASS` and `TRACE_10_FAIL`), and 425. In `happens_before-concurrency.rs` they're at lines 88, 105, and 131. The probe test becomes `assert_every_interleaving_reads(|store| trace_scenario(STORE_PROBE, store), "absent");`.
  6. Rewrite `trace_scenario`'s doc ("so every run of one trace shares the files the test reads afterward") to say each run gets an empty store.
- Stop rule: every existing happens-before and concurrency trace must pass unchanged under per-run stores. If one fails, an earlier run's files were masking an ordering defect. Stop and report the trace and the failing batching or seed. Don't weaken any assertion and don't restore the shared store. Don't keep the shared store with per-run cleanup either, because deleting files between runs still leaves the VFS's other state shared.
- Constraints: `happens_before.rs` is 433 lines and must stay under 500 after the probe, the test, and the helper changes; `happens_before-concurrency.rs` is 136. Use `#[expect(..., reason = "...")]`, never `#[allow]`.
- Acceptance: the probe test failed under `AllAtOnce` before edit 3 (or D1-18 is flagged false), and afterward it passes with every existing trace unchanged in the full engine suite, and component-scope clippy and formatting pass.

</step-4>

<step-5>

### Step 5: Check the Engine graph on every platform [completed]

- Component: Engine guards
- Tests:
  - `cargo test -p build-xtask a_forbidden_crate_reached_only_on_another_platform_is_reported`
  - `cargo test -p build-xtask`

Closes D1-7, a user-added lead. This is the final step, so Verify runs it at FULL scope: the survey's build, formatter check, workspace and workshop clippy with `CARGO_BUILD_WARNINGS=deny`, docs gate, and both full-suite nextest runs. That FULL run is the plan's `AGENTS.md` exit gate, and every earlier step must still pass in it.

- D1-7: the resolved-graph guard checks the host platform only
  - `forbidden_engine_graph` (`crates/build-xtask/src/engine_deps.rs`, defined at line 168, `cargo tree` arguments from line 186) passes no target selection, so `cargo tree` uses its host-only default. A forbidden crate (one of `FORBIDDEN`, lines 38-45) reachable only through another platform's dependency edge is never reported. Nothing reaches one today.
  1. Add `a_forbidden_crate_reached_only_on_another_platform_is_reported` to `crates/build-xtask/src/engine_deps-tests.rs`, modeled on `a_forbidden_crate_reached_through_a_dev_dependency_is_reported` (line 260) and built with `graph_workspace` (line 224). Fixture: `vendor/driver` depends on the `tokio` stand-in only under `[target.wasm32-unknown-unknown.dependencies]`, and `crates/runvil-internal/engine` depends on `driver` normally, so the manifest scan finds nothing and only an all-targets graph reaches `tokio`. Assert `assert_eq!(found.len(), 1, "{found:?}")`, then that the one finding names `runvil-engine`, the resolved graph, and `tokio`.
  2. Run it. It must fail with `[]`, because host-only `cargo tree` never follows the wasm32 edge and `graph_findings` returns nothing. If it passes, D1-7 is false: keep the test, skip edit 3, and flag it in the return.
  3. Add `"--target", "all"` to the `cargo tree` arguments. Say "all targets" in the module doc (lines 18-19, which say the check runs `cargo tree` "with all features over its normal, build, and dev edges"), in `forbidden_engine_graph`'s doc (from line 161), and in the `Reached` rendering at line 113, which says "the resolved graph (all features; {GRAPH_EDGES} edges)" today.
  - Don't add `--locked` to the `cargo tree` call: the `graph_workspace` fixtures (`engine_deps-tests.rs` lines 224-249) have no `Cargo.lock`, `cargo tree --locked` errors when the lock file is missing, and the gate's outer `--locked` already refuses a stale lock on the real workspace. Don't pass an explicit list of triples instead of `all`; it misses any platform left off the list.
- Stop rule: every existing graph fixture test must pass unchanged, and the real-workspace Engine guard must report nothing new. If the real workspace now reaches a forbidden crate through a non-host edge, stop and report the crate and the edge. Removing that edge changes a dependency, which is the user's call.
- Network: `cargo tree --target all` needs package metadata for every platform's dependencies, so a host whose cargo cache lacks non-host packages downloads them. Offline, the guard fails closed with `Violation::Unresolved`. `.github/workflows/ci.yml` sets no offline mode at `b29cd4fce`, so add no `cargo fetch` unless a run shows `Violation::Unresolved`.
- Linux CI: the plan also requires `cargo test -p build-xtask` to pass in Linux CI. Verify runs on this Windows host only, so note in the return that the Linux half is confirmed by CI after push.
- Constraints: `engine_deps.rs` is 242 lines and `engine_deps-tests.rs` 378; both stay under 500. No new structural checks; only the graph guard's `cargo tree` arguments change.
- Acceptance: the new test failed with `[]` before edit 3 (or D1-7 is flagged false) and passes after it, `cargo test -p build-xtask` passes with no new real-workspace finding, and FULL verification passes.

</step-5>

</execution-plan>
