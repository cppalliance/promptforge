---
name: Internal-crate test harnesses
overview: "Refactor the test suites of the six crates under crates/runvil-internal onto a small, shared-vocabulary set of per-crate test utilities, cutting setup duplication without changing what any test checks. It also removes tokio from the engine, polices determinism structurally in every Engine crate, drops tempfile from the Engine crates, and widens the run seed to 256 bits with a 256-bit run id derived from it. The per-crate directions are broad strokes: decomposition re-inventories the code and writes the concrete steps. It runs before the rulebook-fix plan and takes over that plan's tokio, determinism, and test-setup findings."
todos:
  - id: reconsolidate
    content: "Decomposition: re-measure test lines, re-inventory helpers (including the rulebook items taken over and timed waits), write concrete steps; the first step saves the baseline test lists and line counts"
    status: completed
  - id: parser
    content: "parser: prompt source builder in the parser's test_support, shared with the engine, plus parse_ok / parse_err / assert_parse_error"
    status: completed
  - id: types-model-client
    content: "types and model-client: round_trip, assert_json, wire decode helper, one assert_send_sync; Seeded replaces rand in types, which drops its rand dev-dependency; clear the timed wait in types' cancel-tests.rs"
    status: completed
  - id: vfs
    content: "vfs: configurable test backend, handle and access helpers; TempRoot replaces tempfile in vfs, the engine, and the facade, and tempfile leaves all three manifests; the same commit clears the facade's timestamp.rs clock read"
    status: completed
  - id: lua
    content: "lua: TestVm builder with named handles, eval helpers, one TEST_SEED; the cancellation tests' wall-clock bound becomes deterministic"
    status: completed
  - id: engine-run
    content: "engine Run level: Seeded with its self-test and a synchronous ScriptedChat core first, then Scenario over the serial drive, FnTool, shared reply script, source builder; move every tokio test that inspects nothing below Run (drive_tokio, run_with_fixture, and most TokioDriver::new users) to plain #[test], mid-step cancels through a joined canceller thread"
    status: completed
  - id: engine-scheduler
    content: "engine scheduler level: serial cfg(test) rig for the remaining TokioDriver tests, the ones that need the effect tap, round capture, shuffles, held answers, or RunState internals"
    status: completed
  - id: engine-no-tokio
    content: "engine: delete tokio driver and async types, serial bench, drop tokio/async-trait, clear remaining engine sleeps, clock reads, and timed waits"
    status: completed
  - id: run-seed
    content: "256-bit RunSeed replaces the u64 seed; RunUid and GuardNonce derive from it by domain-tagged SHA-256; RunSeed's Debug is redacted and RunContext's Debug shows the run id; Harness draws 32 bytes; Workshop run-log stores hex and bumps LAYOUT_VERSION to 2; tests pass fixed seeds"
    status: completed
  - id: determinism-policing
    content: "Engine crates, two commits: (a) engine_deps.rs total with a per-crate all-features cargo tree graph check plus getrandom and fastrand, no exemptions, exemption code removed; (b) three clippy.toml bans plus build-xtask check, rules in root AGENTS.md"
    status: completed
isProject: false
---

# Internal-Crate Test Harnesses

### Context for a fresh reader

- Repository: `c:\Users\Vinnie\cursor\runvil` (git). Paths are relative to it unless they start with `../`.
- Run with the vibe coder (`../tools-public/planning/vibe-coder.md`). It owns the plan copy under `vibe/`, `vibe/ACTIVE`, and the close commit. Use `internal-test-harnesses` as the plan's vibe name, so the close commit is `Close plan: internal-test-harnesses`. The rulebook-fix plan waits for that commit.
- Shell: PowerShell on Windows. Run cargo commands one at a time, since they share the target directory's lock, and don't append `2>&1`. Judge a cargo step by its result lines and `$LASTEXITCODE`.
- Before editing a crate, read the root `AGENTS.md` and the nearest `AGENTS.md` above the crate, if there is one. Among the crates this plan touches, `crates/runvil-internal/engine`, `lua`, `types`, and `vfs` have their own, `crates/harness-gateway-client` has one, and `crates/workshop/run-log` falls under `crates/workshop/AGENTS.md`.
- A file leaves the repository by `Move-Item` to `../cabinet/_trash/`, never by deletion.
- Scratch files (test-list snapshots and line counts) go in `../cabinet/_scratch/vibe-coder-internal-test-harnesses/`.
- Baselines:
  - Counts marked "at the baseline" are from `7476ed81d` (`Close plan: remove-hakari`).
  - Counts marked "at `3b1ea0db1`" were rechecked at `3b1ea0db1` (`Close plan: sidecar-strict-fences`), the local HEAD when this plan was prepared. Since then `origin/master` has moved to `3b1ea0db1`, and the local HEAD is `594eb22a7` (`Move older vibe plans to October`), which touches only `vibe/`. Facts marked "at `594eb22a7`" were checked there.
  - Decomposition re-measures everything against the code as it stands.
- Plan order:
  - Three plans have landed since the baseline, all pushed to `origin`:
    - "No-Trace Fence Rule, Review Fixes, and Facade Test Harness" (`vibe/2026-10-14-1-no-trace-fence.md`, closed at `30a66c78d`). Besides its fence work, it built the facade harness (below). The harness answers timers at once, which removed the timer sleep in `crates/runvil/tests/suite/effect.rs`, and the `greeter` example (`crates/runvil/examples/greeter.rs`) now answers a timer with `EffectAnswer::Timer` at once.
    - "Facade Suite Dedup" (`vibe/2026-10-14-2-facade-suite-dedup.md`, closed at `ba9ee1f6c`). It made `tempfile` a dev-dependency of `crates/runvil`, the engine, and vfs, replacing three hand-written `TempDir` helpers, the facade's clock-named one in `prepare.rs` among them. It loosened the vfs crate's zero-dependency rule to allow dev-dependencies, and moved `every_shipped_prompt_parses_offline` into the engine suite's `parsing.rs`.
    - "Cache Sidecar Ordering, Strict Lua Fences, and Leftover Traces" (`vibe/2026-10-14-3-sidecar-strict-fences.md`, closed at `3b1ea0db1`). It made every non-exact Lua fence a parse error and grew the parser's fence tests in `src/tests/h1_fences.rs` and `src/tests/section_fences.rs`.
  - The rulebook-fix plan, "Rust Rulebook Fixes for the Engine Crates" (`C:\Users\Vinnie\.cursor\plans\rulebook_fix_commits_e2ab1cdf.plan.md`), runs after this plan's close commit. It has eleven commits, one per audit unit, and changes production error types and APIs. Its tokio, determinism, and test-setup findings move to this plan, as "Taken over from the rulebook-fix plan" under Technical Design lists. Nothing in this plan edits that plan.
- State at `3b1ea0db1`:
  - The facade's one remaining determinism site is the clock-built stamp in `crates/runvil/tests/suite/timestamp.rs`. The **vfs and TempRoot** item clears it, in its `TempRoot` commit.
  - tokio in the engine:
    - No production engine source names tokio.
    - The engine's tests and test support do. tokio is an optional dependency that only the `test-support` feature enables, and it is also a dev-dependency with `test-util`, alongside `async-trait` (`crates/runvil-internal/engine/Cargo.toml`).
    - The tokio test driver `test_support::drive_tokio` (`crates/runvil-internal/engine/src/test_support/tokio_driver.rs`) performs effects through async closures.
    - Most tokio tests don't call `drive_tokio`. At `594eb22a7`, about 60 test files build a `RunState` with a context builder such as `loop_context` and drive it with the `pub(crate)` `TokioDriver::new(&state, fixture, client).drive()`, Run-level files such as `execute/tests/chat_arm.rs`, `tool_loop.rs`, and `effects.rs` among them. `drive_tokio` and `run_with_fixture` appear in about 6. The same recount finds about 562 `#[tokio::test]` in 84 files.
    - `ScriptedChat` (`src/test_support/scripted-chat.rs`) is async, and fakes a slow reply with `tokio::time::timeout(timeout, tokio::time::sleep(delay))`.
    - The four engine clock reads all check that a cancel returns promptly. In three of them (`execute/tests/local_tools.rs`, `execute/tests/models_loop_compactors.rs`, `execute/tests/scheduler/failures-script-tools.rs`) the cancel lands while Lua runs `while true do end` inside one step. In the fourth (`execute/tests/tool_loop.rs`) it lands while a slow tool is out. `failures-script-tools.rs` already waits for a `started` counter before cancelling.
    - At the baseline, 564 of the engine's roughly 830 tests are `#[tokio::test]`, 129 of them multi-threaded, and about 64 to 72 test files use the tokio driver, depending on which driver names count, against 9 or 10 that use the serial `drive`.
    - The criterion bench `benches/models_loop.rs` (`required-features = ["test-support"]`) drives runs through the tokio driver.
    - No crate outside the engine uses the tokio driver.
    - `crates/build-xtask/src/engine_deps.rs` allows tokio and `async-trait` in dev-dependencies and behind `test-support`. `crates/build-xtask/src/test_support_leak.rs` reuses its `EXEMPTING_FEATURE` constant.
  - Forbidden and entropy crates in the Engine graph: `cargo tree -p <crate> --all-features -e normal,build,dev` over the eight Engine crates reaches one from five of them.
    - The engine reaches tokio and `async-trait`, which this plan removes.
    - `runvil-types` reaches `getrandom` through its `rand` 0.9 dev-dependency, which backs the five `rand::random` sites in `crates/runvil-internal/types/src/untrusted/tests.rs`.
    - `crates/runvil`, the engine, and `runvil-vfs` reach `getrandom` 0.3 and `fastrand` through their `tempfile` 3.27 dev-dependency, which names temporary directories randomly. Its three call sites are the facade's `tests/suite/prepare.rs`, the engine's `src/execute/tests/suite/vfs.rs`, and the shared helper in vfs's `src/real/tests.rs`. `tempfile` is vfs's only dev-dependency.
    - `runvil-plugin`, `runvil-lua`, `runvil-parser`, and `runvil-model-client` reach none.
  - The run seed:
    - It is a `u64`. The caller passes it to `RunContext::new` (`crates/runvil-internal/engine/src/execute/config.rs`), and the Engine derives the untrusted-envelope nonce from it in `GuardNonce::from_seed` (`crates/runvil-internal/types/src/untrusted.rs`) as SHA-256 over the domain tag `runvil.guard-nonce.v1` and the seed, keeping 16 bytes. A 64-bit seed caps the nonce's unpredictability at 64 bits.
    - The Harness draws it with `rand::random()` in `crates/harness-internal/runner/src/prepare.rs` and hands it to the recorder as `RunMeta.seed` (`crates/harness-internal/runner/src/recorder.rs`). `RunMeta` literals also appear in the run log's tests and in `crates/workshop/agents/src/tee-tests.rs` (package `workshop-agents`).
    - The Workshop run log stores it in an `INTEGER` column (`crates/workshop/run-log/src/schema.rs`, `append.rs`, `read.rs`). It stamps its layout in a `layout` table: `LAYOUT_VERSION = 1` in `schema.rs`, whose doc says to bump it with any change to the tables, so a file written in an older layout is set aside instead of written into.
    - `RunContext::new` has about 15 callers across the facade, its `greeter` example, the engine, its bench, the Harness, `harness-gateway-client`, and their tests. `GuardNonce::from_seed` has about 16: mostly lua and engine tests, plus `lua/src/vm/run.rs`, `engine/src/execute/context.rs`, and the lua bench `crates/runvil-internal/lua/benches/surface.rs`.
    - `RunContext`'s hand-written `Debug` (`config.rs`) prints the seed.
- Terms:
  - Engine, Harness, Host, and Plugin are as `AGENTS.md` defines them.
  - The *Engine crates* are `crates/runvil` (the facade), `crates/runvil-plugin`, and every crate under `crates/runvil-internal/`, the set `engine_guards::engine_crates` in `crates/build-xtask/src/engine_guards.rs` returns.
  - The *facade harness* is the facade suite's test harness (`Scenario`, `Driver`, `Trace`) in `crates/runvil/tests/suite/support.rs` and `support/scenario.rs`.
  - A *test utility* is shared test-only code inside one crate. It has nothing to do with the Harness.
  - The *serial driver* is `test_support::drive` in `crates/runvil-internal/engine/src/test_support.rs`. It steps a run on the calling thread and answers each effect through a closure.
  - A *work item* is one entry of the list under Execution Instructions, named in bold there, such as the **run seed** item. Decomposition turns each into one or more steps, so other sections name work items, never step numbers.
  - *Re-consolidation* is this plan's decomposition: re-measuring and re-inventorying against the code as it stands, then writing the concrete steps.
  - An *audit unit* is one of the eleven slices of the rulebook audit behind the rulebook-fix plan.

<product-contract>

## Product Requirements

About 54,000 lines of tests in `crates/runvil-internal/` repeat their setup file by file. The test infrastructure already exists, especially in the engine, but each file wraps it in private helpers of its own. This plan gives each crate a few shared utilities that use the facade harness's vocabulary, then rewrites the existing tests on top of them. It also removes tokio from the engine, polices determinism structurally, and gives every run a 256-bit seed and a 256-bit id. No existing test is removed or weakened. The only new tests are the guards' fixture tests, the `Seeded` self-test in each crate that gets `Seeded`, the `TempRoot` self-tests, and the run-seed tests.

- Problem and users: maintainers reading and writing tests in these crates. Sizes at the baseline, from an earlier count (the first step's count is the real baseline):
  - engine: about 34,000 lines
  - lua: about 11,500
  - vfs: about 3,800
  - parser: about 2,300
  - types: about 2,300
  - model-client: about 700
- Goals:
  - No tokio and no `async-trait` anywhere in the engine: not in dependencies, dev-dependencies, features, test support, tests, or the bench. Every engine test drives runs with the serial driver.
  - No Engine-crate test sleeps or is nondeterministic.
    - No test reads the wall clock, sleeps, waits with a timeout, or draws entropy from the OS.
    - Pseudo-random input is allowed only from a fixed seed, and a failure must print that seed.
    - Every clock, seed, and answer is a fixed value the test supplies, and outcomes never depend on thread or timer races.
    - At `3b1ea0db1`, production Engine code has no such call. The test sites, which decomposition re-inventories, are:
      - 13 sleeps, all in engine tests and test support (14 at the baseline; the facade's `effect.rs` sleep is gone)
      - 7 wall-clock reads: one in the facade's `timestamp.rs`, two in lua's `src/tests/cancellation.rs`, and four in engine tests (9 at the baseline; the hand-written `TempDir` clock reads are gone)
      - 5 randomness sites in `types` tests
      - 2 timed waits: the 10-second `Condvar::wait_timeout` backstop in the engine's `src/execute/tests/scheduler/store_gate.rs`, and a 5-second `recv_timeout` in `crates/runvil-internal/types/src/cancel-tests.rs`
  - No Engine crate depends on an entropy crate, even transitively through a dev-dependency. `tempfile` leaves the facade, engine, and vfs.
  - Every run carries a 256-bit seed the caller supplies and a 256-bit id the Engine derives from it.
  - Express existing tests compactly through shared utilities, with no private setup helper defined in more than one file of a crate.
  - Use one vocabulary across crates and the facade harness, so tests read the same everywhere.
- Non-goals:
  - Changing behavior under test, adding coverage, or fixing test defects the rulebook-fix plan keeps, such as assertions on `Debug` text and one-word `expect` messages. The exceptions are the rulebook findings listed under "Taken over from the rulebook-fix plan", which this plan owns, and the **run seed** item, which changes the nonce's derivation and the public `RunContext` signature.
  - Refactoring the facade suite, which the no-trace and facade-suite-dedup plans covered. Its `timestamp.rs` clock read, its `TempRoot` swap, and its seed call sites are in scope.
  - The Harness, Gateway, and Workshop crates, apart from the seed call sites the **run seed** item changes in the Harness, `harness-gateway-client`, the Workshop run log, and `workshop-agents`' tests.
  - Clippy's `iter_over_hash_type` lint (see the Decision Record's defaults).
- Success criteria:
  - `crates/runvil-internal/engine/Cargo.toml` names neither tokio nor `async-trait`, and no engine source names `tokio` or `async_trait`.
  - `crates/build-xtask/src/engine_deps.rs` fails when any Engine crate declares a forbidden crate in any dependency table, dev-dependencies included, or reaches one through its resolved graph.
  - `rg -n -g '*.rs' "thread::sleep|time::sleep|SystemTime::now|Instant::now|RandomState|wait_timeout|recv_timeout|park_timeout" crates/runvil crates/runvil-plugin crates/runvil-internal` returns no hits. The clippy gate passes with the determinism bans in force.
  - `cargo tree -p <crate> --all-features -e normal,build,dev` reaches no `getrandom` or `fastrand` from any Engine crate, and no Engine crate's manifest names `tempfile`.
  - `RunContext::new` takes a 256-bit `RunSeed`, `RunContext::uid` returns the run's 256-bit `RunUid`, and the Harness draws 32 bytes from the OS for each live run.
  - `cargo nextest list -p <crate>` shows the same test function names before and after for every package the plan touches, apart from the new tests named above. A test whose module path changes, because its file split or it moved between the Run and scheduler levels, counts as unchanged when its package and function name stay the same; the step lists each move in its commit message.
  - Each crate's test line count drops. Rough targets: engine 15 to 25 percent, lua 10 to 15 percent, the smaller crates where their shape allows. Decomposition may reset them after its count. The engine and lua must drop. The four smaller crates (parser, types, vfs, model-client) drop where their shape allows: their rewritten test files get shorter, and any rise in a total comes only from the utilities this plan requires that crate to host (the parser's cross-crate source builder, `Seeded`, `TempRoot`, the shared helpers) and their self-tests. The parser and the engine together must drop.
- Constraints:
  - Every source file stays under the per-file line ceiling that `build_ceiling::check` enforces. Splitting a file is acceptable.
  - Engine test support stays out of normal builds: it is `#[cfg(test)]`, or behind the `test-support` feature that only the engine's own bench enables. No crate outside the engine enables it.
  - The facade suite names Runvil items by `runvil::` paths only, so it can't use any crate's test support.
  - No new dependencies.
- Open questions: None. The ones left open while the plan was in broad strokes are settled as defaults in the Decision Record.

## Functional Specification

The users are test authors. Tests keep their names and their assertions; only setup and driving move into utilities. The one change Hosts see is the 256-bit run seed.

- Actors and workflows:
  - A maintainer writes or reads a test and sees only its scenario data and its assertions.
  - A Host builds a run with `RunContext::new(name, seed, started_at)`, where `seed` is a `RunSeed` of 32 bytes drawn from a CSPRNG for a live run, or the recorded seed for a replay.
- Inputs and outputs: `RunContext::uid()` returns the run's `RunUid`, 32 bytes shown as 64 lowercase hex characters.
- States and validation: None
- Errors and recovery: helpers are `#[track_caller]`, so a failure points at the test line, and a helper failure names what was being set up.
- Security and privacy behavior:
  - The untrusted-envelope nonce's unpredictability rises from 64 bits to its full 128, because the seed it derives from is 256 bits.
  - The run id is a one-way derivation of the seed, so showing it reveals neither the seed nor the nonce.
- Acceptance criteria: every crate passes its suite with unchanged test names, apart from the new tests, and fewer lines, and no setup helper is defined twice within a crate.

</product-contract>

<implementation-contract>

## Technical Design

Every crate gets the same three kinds of utility: a source builder, preloaded answers or fixtures, and a driver that returns a record to assert on. Each crate keeps its utilities in its own test support, and names that mean the same thing are spelled the same across crates. Engine paths in this section are under `crates/runvil-internal/engine/src/` and lua paths under `crates/runvil-internal/lua/src/` unless a full path is given.

- Architecture, the shared vocabulary:
  - **Source builder:** `prompt(front, &[(heading, lua)])` and a one-section form. Each line is trimmed, so sources can be indented raw strings. Tests whose subject is layout keep literal sources, for example line mapping, CRLF, BOM, and whitespace.
  - **Preloaded answers:** a reply script built from `text`, `calls`, and `fail`, and tools built from closures (`FnTool::new(id, schema, handler)`) instead of hand-written trait impls.
  - **Driver to record:** a `Scenario`-style builder that holds setup as data and returns a `Trace` with the result, the events, and paired effect and answer records. A step-by-step form serves tests that act mid-run.
  - **Seeded input:** `Seeded::new(seed)`, a small in-repo pseudo-random generator such as SplitMix64, for tests that need varied input or orderings. It exposes its seed, and every assertion made through it names the seed, so a failure can be replayed. The engine's seeded interleaving loops (`execute/tests/happens_before*.rs`) move onto it. The engine's copy lands with its self-test in the **engine Run level** item's first commit, because those loops convert in the two engine items. It lives in the test support of each crate that needs it, today `types` and the engine, because the plan adds no dependencies. In `types` it replaces the five `rand::random` sites, and `rand` leaves `crates/runvil-internal/types/Cargo.toml`, since its OS-seeded generator reaches `getrandom`.
  - **Temporary directory:** `TempRoot`, under "Temporary directories" below.
- Modules and interfaces, by crate:
  - **parser:** the source builder and parse assertions.
    - The source builder lives in the parser's `test_support` module, so the parser's tests and the engine's share it. The module is gated `#[cfg(feature = "test-support")]` today (`crates/runvil-internal/parser/src/lib.rs`), so the gate widens to `cfg(any(test, feature = "test-support"))`. The engine already enables that feature from its dev-dependencies and imports from the module in three files.
    - `parse_ok(src)`, `parse_err(src)`, and one `assert_parse_error(src, kind, fragment)` replace about 91 inline frontmatter literals and two copies of `parse`.
  - **types and model-client:** serde and wire helpers.
    - `round_trip(value)` and `assert_json(value, json!(..))` cover the round-trip and shape checks, about 71 `json!` uses between the two crates.
    - A decode helper serves model-client's wire tests, and a single `assert_send_sync` replaces the three copies.
    - `Seeded` in `types`, as above. Any input that fails under `Seeded` is committed as a fixed case.
    - The `recv_timeout` in `src/cancel-tests.rs` becomes a plain `recv()`. A hang then fails through nextest's timeout in `.config/nextest.toml` instead of a wall-clock bound in the test.
  - **vfs:** one configurable test backend.
    - A test backend whose operations can be overridden or recorded replaces the hand-written backends that each re-implement `acquire`, `release`, `stat`, `list`, `glob`, and `exists`, defined three to six times each today.
    - `TempRoot` replaces the `tempfile` helper in `src/real/tests.rs`.
    - A handle-builder shortcut and an `access()` helper cover the rest.
  - **Temporary directories,** in the facade, the engine, and vfs.
    - Each of the three crates gets one `TempRoot` in its own test code: the facade suite's `tests/suite/support.rs`, the engine's `#[cfg(test)]` test support, and vfs's `src/real/tests.rs`. Each copy uses std only.
    - `TempRoot::new(label)` makes `std::env::temp_dir()/runvil-tests/<crate>/<label>-<pid>-<n>`. `label` is the calling test's name, `pid` is `std::process::id()`, and `n` comes from one process-wide `AtomicU64`. It removes anything already at that path, creates the folder, and removes it on drop. `path()` returns it, and a failure to create or remove it names the path.
    - The process id keeps concurrent test processes apart: nextest runs each test in its own process, so an in-process counter alone restarts at 0 in every one. The counter keeps tests in one process apart, as plain `cargo test` runs them on threads. The label keeps the folder readable. Every run gets fresh names, with no clock and no random generator.
    - `tempfile` leaves the `[dev-dependencies]` of `crates/runvil/Cargo.toml`, `crates/runvil-internal/engine/Cargo.toml`, and `crates/runvil-internal/vfs/Cargo.toml`, along with the comment above each line. vfs's table goes, because `tempfile` is its only entry. Its manifest test still allows dev-dependencies. Refresh `Cargo.lock` once with `cargo metadata --format-version 1 | Out-Null`, without `--locked`; its diff only drops the three `"tempfile",` edges.
    - Each copy gets one self-test: two `TempRoot`s with one label get different paths, and dropping one removes its folder.
  - **lua:** a VM builder and eval helpers.
    - `TestVm::new().tools(..).plugins(..).models(..).argv(..).var(..).messages().build()` returns a struct with named handles (runtime, sets, access) instead of tuples. It also offers `eval::<T>(src)` and `eval_err(src)`.
    - It replaces the two `lua_over` copies, the three `section_vm` copies and their `_with_*` variants, `models_vm`, `h2_vm`, `shim_vm`, `vm()`, `lua_with_tools*`, the six `fresh_access` copies, and the construct-inject-install sequence written out about 19 times.
    - One seed constant, `TEST_SEED`, replaces the six nonce seeds in use today. It is a `u64` until the **run seed** item makes it a `RunSeed`.
    - The two cancellation tests in `src/tests/cancellation.rs` bound abort time with `Instant::now`. That bound becomes a deterministic one, such as how many times the instruction hook fired, so the tests lose the clock without losing strength.
  - **engine:** two layers, both serial, and no tokio.
    - **Driving without tokio.** The Engine is sans-IO, so the serial driver can express everything the tokio tests do:
      - Concurrency is the order in which the test answers a step's effects: issue order, reversed, or effects held and released by the test.
      - Time is a timer effect that the test answers at once or holds, never a sleep.
      - Cancellation is a cancel handle set between steps. When the cancel has to land while one step is still running, as with Lua running `while true do end`, a second thread sets it, and the test joins that thread before asserting. The thread waits for a signal from inside the step, such as the `started` counter `execute/tests/scheduler/failures-script-tools.rs` uses or a Lua-side flag, never for a sleep. The elapsed-time bound becomes a deterministic one, as in lua's cancellation tests, and a hang fails through nextest's timeout.
      - The scripted chat client and fixture tools answer synchronously. `ScriptedChat` gets a synchronous core in the **engine Run level** item's first commit, and the tokio driver wraps it until the **tokio removal** item deletes the wrapper. A slow reply that should time out answers with the timeout error at once instead of sleeping.
      - The tokio driver, its async performer types (`Performers`, `BoxFuture`, the async `ChatClient`), and every `#[tokio::test]` go away, and tests become plain `#[test]`.
    - **Run level.** One `Scenario` over the serial `drive` replaces the per-file context builders and drive wrappers. Context builders: `effect_context`, `offering_context`, `catalog_context`, `tool_context`, `loop_context`, `model_task_context`, `ceiling_context`, `to_context`. Drive wrappers: `run`, `run_fixture`, `run_offline`, `run_assembled`, `run_with_context`, `run_recorded`, `drive_serially`, `drive_recorded`, `drive_observed`, `drive_loop`, `identity_run`. These lists are a starting inventory; decomposition finds the rest. `Scenario` also replaces `TokioDriver::new(&state, fixture, client).drive()` in every test that inspects nothing below `Run`, which is most of the roughly 60 files that call it.
    - **Scheduler level.** A `#[cfg(test)]`-only rig for tests that work below `Run`. It builds the run state with the model set and tool set already filled in, and exposes a serial step-and-answer loop and the effect tap. It takes the place of `TokioDriver` for the tests that need the effect tap, round capture, shuffles, held answers, or `RunState` internals, most of them under `execute/tests/scheduler/`. A test moves up to `Scenario` only when it inspects nothing below `Run`.
    - The bench drives runs with the serial driver. The `test-support` feature stays, as `test-support = []`, because the bench is a separate target that can't see `#[cfg(test)]` code. It gates the serial driver and `ScriptedChat`, and the bench is its only consumer.
    - The source builder replaces `loop_prompt`, `owner_prompt`, `arm_prompt`, `full_id_prompt`, `capped_loop_prompt`, `jump_prompt`, `offering_prompt`, `with_test_title`, the seven `*_prompt!` macros (`effect_prompt!`, `prose_prompt!`, `require_prompt!`, `args_prompt!` twice, `flow_prompt!` twice), and the inline frontmatter literals (`rg -n 'name: t\\ndescription: d' crates/runvil-internal/engine/src/execute/tests`, about 200 lines in 46 files).
    - `FnTool` turns the hand-written fixture tools (echo, untrusted echo, structured, failing, scoped) into one-liners. It removes the repeated `#[expect(clippy::unnecessary_literal_bound)]` blocks, and `ScriptedChat` takes the shared reply constructors.

  - **Run seed and run id.** The caller's seed widens to 256 bits, and the Engine derives a 256-bit run id from it. No compatibility shims: signatures change outright, and every caller in the workspace changes in the same commit.
    - `RunSeed([u8; 32])`, in a new `seed` module of `runvil-types` and re-exported at the facade root beside `RunContext`, has `from_bytes([u8; 32])` and `as_bytes()`. `Display` and serde use 64 lowercase hex characters. `Debug` prints `RunSeed(..)` with no bytes, so a `{:?}` in a trace, panic, or test failure never shows the seed.
    - One derivation helper in the same module, `derive(domain: &[u8], seed: &RunSeed) -> [u8; 32]`, computes SHA-256 over the domain tag, then the seed's bytes. Every value derived from the seed goes through it with its own `runvil.<purpose>.v1` tag. `sha2` is already a normal dependency of `runvil-types`.
    - `GuardNonce::from_seed(&RunSeed)` keeps its `runvil.guard-nonce.v1` tag and its 32-hex-character form, taken from the first 16 bytes of `derive`. Its doc drops the 64-bit wording: the nonce's unpredictability is now its full 128 bits.
    - `RunUid([u8; 32])`, in the same module and re-exported beside `RunSeed`, is `derive(b"runvil.run-uid.v1", seed)`, with a hex `Display`. `RunContext::uid()` returns it. The name stays clear of the Harness recorder's `RunId(i64)`, which numbers recordings.
    - The run id is derived from the seed and is never the seed itself, so it can appear in logs, records, and the UI without revealing the seed, and with it the nonce.
    - A replay passes back the recorded seed, so it reproduces the run id along with the nonce, `sys.when`, effects, and events. The recorder's `RunId` still tells two recordings of one run apart.
    - The run id is neither stamped on every event nor stored on its own. Events keep the run's name, and any reader holding the recorded seed derives the id.
    - `RunContext::new(name, seed: RunSeed, started_at)` and `RunContext::seed() -> RunSeed`. The docs on `RunContext` and `new` keep the rule that a live run's seed comes from a CSPRNG. `RunContext`'s hand-written `Debug` in `config.rs` shows `uid` in place of `seed`.
    - The Harness's `prepare.rs` draws `RunSeed::from_bytes(rand::random())`. The Harness keeps `rand`, because the entropy ban covers only the Engine crates. `RunMeta.seed` becomes a `RunSeed`.
    - The Workshop run log's `runs.seed` column becomes `TEXT NOT NULL`, holding the 64 hex characters, in the `CREATE TABLE` in `schema.rs`. `append.rs` and `read.rs` convert, and the docs in `lib.rs` and `schema.rs` drop `seed` from the list of `u64` values stored as `i64`. `LAYOUT_VERSION` in `schema.rs` goes from 1 to 2, so the existing layout check sets aside a file written before this change instead of mixing integer and hex seeds in one column.
    - Tests pass fixed seeds only. Each crate's test support that builds runs or VMs has one `TEST_SEED: RunSeed` constant, which the facade `Scenario`, the engine `Scenario` and scheduler rig, `TestVm`, and the Harness's test support use. A test that needs several seeds draws them from `Seeded`.
    - Find every caller with `rg -n "RunContext::new\(|GuardNonce::from_seed\(|RunMeta \{|\.seed\b" crates`, and check `guide/` and the Workshop UI for text that calls the seed a number. Callers that are easy to miss: the lua bench `benches/surface.rs` and `crates/workshop/agents/src/tee-tests.rs`.
    - Re-bless the facade listing with `cargo +nightly-2026-09-05 xtask api --bless`, then confirm with `--check`.
  - **Dependency policing, which makes tokio and OS entropy structurally impossible in every Engine crate.**
    - The guard stays `crates/build-xtask/src/engine_deps.rs`, which already polices Engine manifests. Its crate set comes from `engine_guards::engine_crates`, so a new internal crate is covered when it lands. It runs in `cargo test -p build-xtask`, which is part of the full suite and CI.
    - It closes three gaps:
      - It scans every dependency table, direct and target-specific: `[dependencies]`, `[build-dependencies]`, and now `[dev-dependencies]`.
      - It drops the exemption for an optional dependency that only `test-support` enables. `is_exempt`, `enabling_features`, and `feature_lists` go with it, and `EXEMPTING_FEATURE` moves to `test_support_leak.rs`, its one remaining user.
      - It adds a resolved-graph check: no forbidden crate may be reachable from any Engine crate through its normal, build, or dev edges and their transitive normal and build edges.
        - The graph is resolved per Engine crate with all features enabled, the view `cargo tree -p <crate> --all-features -e normal,build,dev` gives. All features, so optional dependencies are seen. Per crate, so a feature another workspace crate turns on cannot flag an Engine crate through workspace feature unification.
        - It uses `cargo tree -p <crate>`, because `cargo metadata` resolves features for the whole workspace at once and can't give the per-crate view.
        - Fixture workspaces name local path crates `tokio`, `getrandom`, and `tempfile`, so the fixture tests resolve offline.
    - The forbidden list is `tokio`, `tokio-util`, `async-trait`, `reqwest`, and the entropy crates `getrandom`, the OS entropy source, and `fastrand`, whose global generator seeds itself. There is no exemption.
    - `rand` itself is not banned. Its OS-seeded generators depend on `getrandom`, so the graph check rejects them, while a seeded `rand` generator without OS entropy would pass. The plan still adds no dependency and uses `Seeded` instead.
    - No source scan is needed. A crate can name only its direct dependencies, so the declared check keeps every source and test from naming tokio, and the graph check keeps it from arriving through another crate.
  - **API-level determinism policing, in every Engine crate including its tests, benches, and examples.**
    - Clippy reads the nearest `clippy.toml` above a crate's manifest and does not merge it with the root file. So `crates/runvil-internal/clippy.toml` covers all six internal crates, and `crates/runvil/clippy.toml` and `crates/runvil-plugin/clippy.toml` cover the other two.
    - Each of the three files repeats the root `clippy.toml` settings, then adds `disallowed-methods` entries, each with a reason that names the replacement:
      - `std::thread::sleep`: a timer effect the test answers
      - `std::time::SystemTime::now` and `std::time::Instant::now`: a fixed `Timestamp`, or a count of what happened
      - `std::collections::hash_map::RandomState::new`, std's OS-seeded hasher: `Seeded`
      - `std::sync::Condvar::wait_timeout`, `std::sync::Condvar::wait_timeout_while`, `std::sync::mpsc::Receiver::recv_timeout`, and `std::thread::park_timeout`: an untimed wait, with nextest's timeout as the backstop
    - Seeded generation is not banned.
    - The gate's clippy runs `--all-targets` under `CARGO_BUILD_WARNINGS=deny`, so the bans cover test code.
    - Before relying on the bans, the step confirms two assumptions with one deliberate violation per ban, removed afterward: that clippy's config lookup stops at the nearest `clippy.toml` without merging, and that every banned path resolves, `RandomState::new` through its `std::collections::hash_map` re-export included. If clippy behaves otherwise, the bans move to a `build-xtask` source scan of the same paths.
    - A new `build-xtask` check fails in three cases:
      - An Engine crate resolves to a `clippy.toml` other than these three.
      - One of the three is missing a root entry or a determinism ban.
      - Any Engine source contains an `allow` or `expect` of `clippy::disallowed_methods`.
    - Threads stay allowed when a test joins them before asserting, as the cancel-from-another-thread tests do.
    - Real-filesystem tests stay allowed in the facade, the engine, and vfs on fresh `TempRoot` folders. Platform-specific behavior is `cfg`-gated, never skipped at run time.
  - **Rule text** goes in the root `AGENTS.md` under "Structural Rules", since no `AGENTS.md` closer to both `crates/runvil` and `crates/runvil-internal` exists:
    - Engine crates, tests included, never depend on tokio, tokio-util, async-trait, reqwest, getrandom, or fastrand, directly or transitively.
    - No Engine test sleeps, reads the wall clock, waits with a timeout, or draws entropy from the OS.
    - Pseudo-random test input comes only from a fixed seed that the failure prints.
    - Temporary directory names come from the test's name, the process id, and a counter.
    - The module doc of `engine_deps.rs` changes to match.
  - **Facade leftover**, cleared in the **vfs and TempRoot** item's `TempRoot` commit, which already edits the facade suite: `crates/runvil/tests/suite/timestamp.rs` uses a fixed `Timestamp`.
  - **Taken over from the rulebook-fix plan.** That plan runs after this one and drops these items. Step numbers are that plan's.
    - Already in the per-crate directions above: `FnTool` for its `TestTool` reduction (Step 8, including the 10 `unnecessary_literal_bound` expectations), `TestVm` for its lua test hub and its `fresh_access`, `lua_over`, setup-chain, and construct-inject-install dedup (Step 10), the one nonce seed, the cancellation tests' elapsed-time bound, `h2_vm`, `Seeded` in place of its `StdRng` rewrite of `types/src/untrusted/tests.rs` (Step 1), and the source builder for its prompt shell (Step 9).
    - New to this plan:
      - engine test support (Step 8):
        - `ScriptedChat` is shared with the bench through `test-support`, and the bench drops its `#[path]` include, its `#[expect(dead_code)]`, and `BenchClient`.
        - The `test-support` docs in `test_support.rs` name the bench as the feature's only consumer.
        - One test-model binding constructor replaces the hand-built 4096-window bindings in `execute/run/tests.rs`, `execute/run/effect-tests.rs`, `lua/tests.rs`, and `lua/tests/globals.rs`.
        - One copy each of `chunk` (`model/tests-always.rs`, `model/tests-integration.rs`), the `SectionVmSetup` assembly (`lua/tests.rs`, `lua/tests/globals.rs`), and `fresh_access` (`model/tests.rs`, `execute/tests/context.rs`).
      - engine execute tests (Step 9):
        - Its determinism work (paused time, current-thread runtimes, `multi_thread` reasons) becomes serial driving, and the `store_gate.rs` backstop goes with it.
        - The 12 seed loops print their seed through `Seeded`, and the serial loops print their batching, as `execute/tests/scheduler/concurrency-stepped.rs` does.
        - One definition each of `task`, `run_offline`, `TestStore`, `PARKED_CHILD`, `EXECUTION`, `Recorder`, and `task_lifecycle`, renaming the ones that genuinely differ, and no dead `tools` or `_tools` parameter (`execute/tests/suite/support.rs`, `execute/tests/suite/exec_flow.rs`).
      - lua tests (Step 10): the duplicated fixture builders beyond `lua_over`, such as `tool_at` and `scoped`, found by `rg -n "^fn \w+\(" crates/runvil-internal/lua/src -g '*test*'` and merged where bodies match.
    - Gone with the tokio driver, so neither plan does them: the `JoinSet` panic re-raise and its test, the test-hook `cfg` cleanup and the tap and shuffle field, the visibility of `drive_tokio`, `Performer`, and `Performers`, the `# Panics` docs on `tokio_driver.rs`, and `#[non_exhaustive]` on `Performers`. Also gone: its Step 11 hakari items, and its minimal workspace `tokio` entry, whose reason was the engine's optional tokio.
    - Its `tempfile` swap in the engine suite's `execute/tests/suite/vfs.rs` was done by facade-suite-dedup. `TempRoot` now replaces it.
- File and public API changes:
  - The one production change is the **run seed** item. It changes the public `RunContext::new` and `RunContext::seed`, adds `RunSeed`, `RunUid`, and `RunContext::uid`, changes `GuardNonce::from_seed`, the Harness's `RunMeta.seed`, and the Workshop run log's `seed` column, and re-blesses `crates/runvil/public-api.txt`. Otherwise only test support modules grow, and the engine's `test-support` surface shrinks as its tokio driver goes.
  - Three `clippy.toml` files are added: `crates/runvil-internal/`, `crates/runvil/`, and `crates/runvil-plugin/`.
  - `rand` leaves the dev-dependencies of `crates/runvil-internal/types/Cargo.toml`, and `tempfile` leaves those of the facade, the engine, and vfs.
  - The facade's `tests/suite/timestamp.rs` changes only to drop its clock read.
  - The root `AGENTS.md` gains the rules under "Rule text".
- Data, persistence, failure, security, and privacy constraints:
  - Utilities never sleep, never read the clock, never wait with a timeout, and draw pseudo-random values only through `Seeded`, whose seed every failure names.
  - A live run's seed comes from the OS through the Harness or the Host, never from a clock, because a guessable seed makes a guessable nonce.
  - A run-log file written before the **run seed** item has layout 1, so the layout check sets it aside and a fresh file starts. There is no migration, by the standing no-compatibility rule.

</implementation-contract>

<verification-contract>

## Testing Plan

The proof is unchanged test names, passing suites, and smaller line counts, checked one package at a time, plus the guards' fixture tests and the new self-tests.

- Baselines, saved by the first step before any edit:
  - `New-Item -ItemType Directory -Force ../cabinet/_scratch/vibe-coder-internal-test-harnesses | Out-Null`
  - For each package `<pkg>` in `runvil-parser`, `runvil-types`, `runvil-model-client`, `runvil-vfs`, `runvil-lua`, `runvil-engine`, `runvil`, `build-xtask`, `harness-runner`, `harness`, `harness-gateway-client`, `workshop-run-log`, and `workshop-agents`: `cargo nextest list --locked -p <pkg> --all-features | Out-File -Encoding utf8 ../cabinet/_scratch/vibe-coder-internal-test-harnesses/<pkg>-before.txt`
  - The test line counts, with the command below, into `../cabinet/_scratch/vibe-coder-internal-test-harnesses/lines-before.txt`.
  - If a baseline file is ever missing, rebuild it from a `git worktree` of the plan's starting commit placed in that scratch directory, never from an edited tree.
- The line count, from the repository root. It counts test files only (`tests/` and `test_support/` directories, `tests.rs`, `test_support.rs`, `*-tests.rs`, and `tests-*.rs`). Inline `mod tests { }` blocks in production files are not counted; the same command runs before and after.

```powershell
foreach ($c in 'engine','lua','vfs','parser','types','model-client') { $n = (Get-ChildItem "crates/runvil-internal/$c" -Recurse -Filter *.rs | Where-Object { $_.FullName -match '[\\/](tests|test_support)[\\/]|[\\/](tests|test_support)\.rs$|-tests\.rs$|[\\/]tests-[^\\/]+\.rs$' } | ForEach-Object { (Get-Content $_.FullName).Count } | Measure-Object -Sum).Sum; "$c $n" }
```

- The test-name comparison, after each step for every package it touched: `cargo nextest list --locked -p <pkg> --all-features | Out-File -Encoding utf8 ../cabinet/_scratch/vibe-coder-internal-test-harnesses/<pkg>-after.txt; Compare-Object (Get-Content ../cabinet/_scratch/vibe-coder-internal-test-harnesses/<pkg>-before.txt) (Get-Content ../cabinet/_scratch/vibe-coder-internal-test-harnesses/<pkg>-after.txt) | ForEach-Object { "$($_.SideIndicator) $($_.InputObject)" }`. Lines that differ only in module path are moves, from a split file or a test moved between the Run and scheduler levels, and the step lists them in its commit message. Then compare leaf names, the text after the last `::`: `Compare-Object (Get-Content <before> | ForEach-Object { ($_ -split '::')[-1] }) (Get-Content <after> | ForEach-Object { ($_ -split '::')[-1] }) | ForEach-Object { "$($_.SideIndicator) $($_.InputObject)" }`. The leaf comparison must print nothing marked `<=`, and only the new tests below marked `=>`.
- New tests, the only allowed additions:
  - `Seeded` self-tests in `types` and the engine: a failure message includes the seed.
  - `TempRoot` self-tests in the facade, the engine, and vfs: two roots with one label get different paths, and dropping one removes its folder.
  - Run-seed tests:
    - a known-answer test for `derive`
    - for one seed, the nonce and the run id differ from each other and from the seed's bytes, and repeat across calls
    - `RunSeed` round-trips through serde as 64 hex characters
    - `RunSeed`'s `Debug` shows no seed bytes, and `RunContext`'s `Debug` shows the run id and not the seed
    - a Workshop run-log round trip keeps a 32-byte seed
  - `build-xtask` fixture tests for the dependency guard, which fails in each of these cases:
    - tokio in `[dev-dependencies]`
    - tokio in a target-specific dev table
    - tokio as an optional dependency behind `test-support`
    - an Engine crate that dev-depends on a non-Engine crate that depends on tokio
    - `getrandom` reachable through any edge
    - an Engine crate that dev-depends on `tempfile`, which reaches `getrandom` and `fastrand`
    - and passes on a clean Engine crate
  - `build-xtask` fixture tests for the clippy-config check, which fails for an Engine crate whose nearest `clippy.toml` lacks a root entry or a ban, and for an Engine source that suppresses `clippy::disallowed_methods`.
- Changed expectations: tests that pin a literal nonce value are re-pinned under the 256-bit derivation, and any run-log layout test that names 1 as the current layout follows `LAYOUT_VERSION` to 2. Every other expected value stays as it is.
- Per-step checks, for the packages a step touches:
  - `cargo nextest run --locked -p <pkg> --all-features`, except `workshop-server`, which runs without `--all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p <pkg> --all-targets --all-features`
  - `cargo fmt --all --check`
  - the test-name comparison above
- Integration: the engine bench builds and runs on the serial driver: `cargo bench --locked -p runvil-engine --features test-support --bench models_loop -- --test`.
- Regression and structure, by the end:
  - `rg -n "tokio|async_trait" crates/runvil-internal/engine` returns nothing.
  - The determinism `rg` in the Success criteria returns nothing.
  - The resolved graph of each Engine crate reaches no forbidden crate: `foreach ($c in 'runvil','runvil-plugin','runvil-engine','runvil-lua','runvil-parser','runvil-types','runvil-vfs','runvil-model-client') { $hits = cargo tree --locked -q -p $c --all-features -e normal,build,dev --prefix none | ForEach-Object { ($_ -split ' ')[0] } | Where-Object { $_ -in 'tokio','tokio-util','async-trait','reqwest','getrandom','fastrand' } | Sort-Object -Unique; "{0}: {1}" -f $c, ($hits -join ', ') }` prints an empty list for every crate.
  - A search confirms that no setup helper is defined in more than one file of a crate.
  - The line counts are recorded against `lines-before.txt`.
- Exit criteria, from the verification list in `AGENTS.md`, run by the last step:
  - `npm ci --prefix crates/workshop` and `npm ci --prefix crates/gateway/config-ui/ui`
  - `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features`, then `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo check -p gateway --no-default-features`
  - `cargo fmt --all --check`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`, then `$env:RUSTDOCFLAGS='-D warnings'; cargo doc -p runvil --no-deps`
  - `cargo +nightly-2026-09-05 xtask api --check`
  - `node --test crates/workshop/ui/test/docs-claims.mjs`
  - `cargo deny --log-level error check`
  - every test-name comparison, the structure checks above, and the line counts reported against the baseline

</verification-contract>

<decision-record>

## Decision Record

The user asked for this as its own plan, kept to broad strokes because other plans were landing first. The design reuses the facade harness's vocabulary and keeps utilities inside each crate. It runs before the rulebook-fix plan, and it carries two later additions: no `tempfile` in the Engine crates, and the 256-bit run seed and run id.

- Decisions:
  - A separate plan for the internal crates. The user said: "Createa new plan to refactor the tests in the other crates."
  - Broad strokes, with detail written at decomposition against the code. The user said: "Don't get too detailed, because there are two other plans which have to land first, and the code will change."
  - No tokio anywhere in the engine, tests and test support included. The user said: "tokio is not allowed anywhere in the engine". The serial driver replaces the tokio driver, because the sans-IO Engine leaves effect order and timing to whoever answers its effects.
  - No Engine-crate test sleeps or is nondeterministic, enforced structurally. The user said: "my instinct tells me that no runvil test should ever sleep or be non-deterministic". When asked, the user scoped this to the Engine crates and chose structural enforcement over rule text alone.
  - Seeded randomness is deterministic and allowed. Only OS and clock entropy is banned, and a failure prints its seed. The user chose this refinement: "I want refinement 2."
  - Same names as the facade harness (`Scenario`, `Trace`, reply constructors), so tests read alike across the workspace.
  - Utilities live in each crate's own test support, since dependencies run one way (types and vfs, then lua, then parser, then engine) and the facade can't see test support.
  - Line counts for the four smaller crates follow the Product Requirements' "where their shape allows": their rewritten test files must get shorter, but a total may rise by the utilities the plan requires the crate to host and their self-tests. The engine and lua must drop, and the parser and engine together must drop, because the parser's builder serves the engine. Set during the run: in Step 1 the parser's one-line sources saved 115 lines against the builder's 170, and in Step 2 the types serde rewrites saved 56 lines against about 60 for `Seeded` and its self-test, which replace the `rand` dependency rather than test code.
  - Cross-plan conflicts are resolved inside this plan. The user said: "do not touch any other plan except internal-crate_test-harnessses". The user later asked for the rulebook-fix plan to be edited too, only to record the plan order and the handoff.
  - The `greeter` example was the no-trace plan's to fix. The user said: "greeter will be fixed by the time this plan runs, dont worry. I instructed the other vibe coder to fix greeter." That plan's Step 4 made it answer timers at once, without sleeping (`3cc16b932`).
  - This plan runs before the rulebook-fix plan, which starts after this plan's close commit. That plan's Steps 8 to 10 would otherwise harden and convert tokio test code this plan deletes, and deduplicate the same helpers in a different shape. Its production Steps 1 to 7 also get cheaper afterward, because their type changes reach a few shared utilities instead of about 70 test files. The user accepted this order: "Okay, update these two plans to reflect that they will follow your advice".
  - The findings of a 2026-10-14 review of an earlier draft of this plan are folded in. The user chose to fold in all of them.
  - The findings of a second 2026-10-14 review are folded in, and the user chose all of them: the Run and scheduler levels split by what a test inspects rather than by driver name, the engine's `Seeded` and a synchronous `ScriptedChat` core moved to the first engine commit, a pattern for cancels that land mid-step, the run log's layout bump, the missed seed callers, a leaf-name test comparison that allows moves, the `cargo tree` choice for the graph check, and determinism policing split into two commits with the `timestamp.rs` fix moved to the `TempRoot` commit.
  - `RunSeed`'s `Debug` is redacted, and `RunContext`'s `Debug` shows the run id. The user chose this, so a stray `{:?}` never shows the seed and with it the nonce.
  - Tokio removal and determinism policing are separate work items, so the guard's report-only graph run sits between them and each commit bisects on its own.
  - No entropy crate in any Engine crate, transitive dev edges included, so `tempfile` leaves the facade, the engine, and vfs. The user said: "even if this means having to bring back TempFile or whatever, I would prefer not to let him random/entropy crate dependencies even if they are transitive." This reverses the `tempfile` choice of the facade-suite-dedup plan for those three crates.
  - Temporary directory names use the test's name, the process id, and a counter, not the clock. The user had proposed SHA-256 of the current time and the test name, so every run gets a fresh name, then accepted dropping the clock: "if you think we dont need the then sure". The process id already makes each run's names fresh.
  - Every run has a 256-bit seed and a 256-bit id derived from it. The user said: "I do feel that the run itself should have the 256-bit unique ID", and proposed deriving every other unique value "by namespacing and sha256".
  - The seed change lands in this plan, with no separate plan and no compatibility shims. The user said: "no new plans. the people who would break are small (1 or 2 persons) and the fix is trivial (LLM can do it)".
  - Defaults set while preparing the plan for execution, which the user may override. Each settles a question the broad-strokes draft left open:
    - The prompt source builder lives in the parser's `test_support` module, with its gate widened to `cfg(any(test, feature = "test-support"))`. Of the crates that need it, only the parser is visible to both the parser's tests and the engine's.
    - Engine tests that inspect state below `Run` keep a scheduler-level rig. A test moves up to `Scenario` only when it inspects nothing below `Run`.
    - Timed waits are banned with the sleep and clock calls. Only two exist in the Engine crates' Rust code, both in tests, so the ban costs two edits.
    - The engine's `test-support` feature stays, emptied of tokio, because the bench can't see `#[cfg(test)]` code and needs the serial driver and `ScriptedChat`.
    - Clippy's `iter_over_hash_type` lint is deferred (see Deferred). It would catch some outcomes that depend on hash-map order, but the workspace lints are inherited by every crate, so enabling it for the Engine crates alone needs a per-crate mechanism the lint-inheritance check allows.
  - Defaults set at decomposition, which the user may override:
    - The dependency guard's three fixture tests that pin the removed `test-support` exemption by name (`a_forbidden_crate_in_dev_dependencies_only_passes`, `an_optional_entry_enabled_only_by_test_support_passes`, `a_dep_gated_entry_with_only_a_weak_feature_path_passes`) are renamed to end in `_is_reported` and assert the violation, in the **determinism policing** item's dependency-guard commit. They are the one exception to unchanged test names: the guard change reverses what they pin, and a `_passes` name on a test that asserts a violation would be false. `build_and_target_specific_tables_are_scanned_and_renames_resolved` keeps its name and now expects three violations, since its dev `tokio` is reported.
    - The Workshop run log decodes its hex `seed` column with a private helper and builds the seed with `RunSeed::from_bytes`, so `RunSeed` gains no `FromStr` and no parse error type in the facade's public API. A malformed value is a read error naming the column.
    - Engine tests keep their files when their driver changes, so module paths stay put; a test moves only when a file splits under the line ceiling.
- Rejected alternatives:
  - Keeping tokio as an engine dev-dependency for the tests, or moving timing tests to tokio's paused time: both contradict the no-tokio rule, and the serial driver makes timing deterministic without a runtime.
  - Determinism for the whole repository, including Harness, Gateway, and Workshop tests: the user scoped the rule to the Engine crates, and some Gateway tests are timing-based by nature. Revisit if those crates adopt deterministic drivers.
  - Determinism as rule text only: the user chose structural enforcement.
  - Banning all randomness, seeded included, through a `rand` crate ban: a fixed seed is deterministic, and banning it would rule out seeded property-style tests. The user chose to allow seeded generation.
  - Banning threads outright: tests that join before asserting are deterministic, and the cross-thread cancel tests need a second thread.
  - A shared test-utility crate for every Engine crate: it would need its own dependency edges into each crate and would cross the `test-support` boundary. Revisit if the per-crate utilities end up duplicating each other heavily.
  - Moving every scheduler-level engine test up to `Run`: some of them exist to test below `Run`.
  - Running the rulebook-fix plan first: its tokio driver hardening, paused-time conversion, and helper dedup would be deleted or redone here.
  - Running only the rulebook-fix plan's production Steps 1 to 7 first: every signature change would edit the tokio test files this plan is about to rewrite.
  - Exempting `tempfile` in the graph check: the user wants no entropy crate in an Engine crate, transitive edges included.
  - Seeding tests, or temporary directory names, from the clock: a clock-seeded test can't be replayed, and an atomic counter alone doesn't separate tests, because nextest gives each test its own process.
  - Seeding live runs from the clock in the Harness: a guessable seed makes a guessable nonce, so fetched content could forge the untrusted envelope. The Harness keeps the OS generator.
  - Using the seed itself as the run id: anything that shows the id would reveal the seed and so the nonce.
  - Stamping the run id on every event, or storing it beside the seed: no consumer needs it yet, and it is derivable from the recorded seed. Add a column or a stamp when a consumer needs one.
  - 128 or 160 bits: 128 is the useful floor, and 256 matches what SHA-256 takes in and gives back.
- Assumptions, risks, and notes:
  - The rulebook-fix plan's test findings that overlap this plan are owned here. Its later production changes, such as the `Duration` timer (its Step 6) and the scheduler state types (its Step 7), update this plan's `Scenario` and scheduler rig in place. It needs no change for the seed: none of its steps touches `RunContext::new` or the seed, and its Step 0 re-check picks up the new types.
  - Some audit findings assume tokio stays in the engine's tests: the 18 real-time tests the audit would move to paused time, the 129 unexplained multi-thread runtimes, the tokio driver that hangs on a performer panic, and its async scripted chat. Under the no-tokio rule, the serial conversion here replaces them, and the rulebook-fix plan leaves them to this plan.
  - Risk: the roughly 70 tokio-driver test files make the engine migration the largest piece. It can land in several commits, as long as the tokio removal comes after every test conversion.
  - Risk: the resolved-graph check may find a forbidden crate reaching an Engine crate through a dependency nobody has noticed. A `cargo tree` run at `3b1ea0db1` found the engine's own tokio and `async-trait` and the `rand` in `types`, which this plan removes, plus `getrandom` and `fastrand` through `tempfile` in the facade, the engine, and vfs, which `TempRoot` removes. The **determinism policing** item runs the check in report-only mode before making it fail the build.
  - The guard covers `crates/runvil-plugin` as well, because it is in the Engine crate set. It reaches no forbidden crate today.
  - Risk: the clippy assumptions under "API-level determinism policing" may not hold. The deliberate violations settle it before the bans are relied on, and a `build-xtask` source scan is the fallback.
  - Risk: the engine's scheduler-level tests may resist a common rig, which would leave that layer's savings small.
  - Risk: large engine test files may approach the line ceiling during migration. Splitting a file is acceptable.
  - Risk: the **run seed** item crosses the Engine, the Harness, `harness-gateway-client`, the Workshop run log, and `workshop-agents`' tests in one commit, because the `RunContext::new` signature change breaks every caller at once. Landing it after the lua and engine utilities keeps most test callers inside `TestVm`, the two `Scenario`s, and the scheduler rig.
  - Risk: a test that used `recv_timeout` or `wait_timeout` as a backstop now hangs instead of failing quickly when the code under test is broken. nextest's timeout in `.config/nextest.toml` turns the hang into a failure.

### Deferred and Out of Scope

- Deferred: clippy's `iter_over_hash_type` lint in the Engine crates. Iterating a `HashMap` or `HashSet` follows a per-process random order, so any outcome that depends on that order is nondeterministic, and `HashMap::new()` seeds randomly inside std without calling the banned `RandomState::new`. At `3b1ea0db1`, 7 non-test files in the Engine crates use hash collections. The lint flags only `for` loops over a hash collection, not iterator chains such as `.keys().map(..)`, so it is a partial guard; `disallowed-types` on `HashMap` and `HashSet` would be the full one, at the cost of production edits. Revisit with a per-crate lint mechanism the lint-inheritance check allows, or as a workspace lint.
- Out of scope: refactoring the facade suite, which the no-trace and facade-suite-dedup plans covered. Its `timestamp.rs` clock read, its `TempRoot` swap, and its seed call sites are in scope.
- Out of scope: the rulebook-fix plan's remaining findings, including its production type changes, failure-message rewrites, payload-free event tests, `expect` messages, `#[track_caller]` and `# Panics` docs on surviving helpers, the `Observer` argument struct, and import cleanup. That plan runs after this one.
- Out of scope: refactoring the tests of the Harness, Gateway, Workshop, linked Plugin, and `build-xtask` crates. This plan touches `build-xtask` only for the two guards and their fixture tests, `crates/runvil-plugin` only to give it a `clippy.toml`, and the Harness, `harness-gateway-client`, the Workshop run log, and `workshop-agents`' tests only for the seed change. Those crates keep `tempfile` and `rand`.

</decision-record>

<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build --locked -p gateway` (the workspace's only default member). Crate build scripts bundle the UIs, so run `npm ci --prefix crates/workshop` and `npm ci --prefix crates/gateway/config-ui/ui` first, as CI does. The desktop app is a separate, explicit build (`cargo workshop`).
- Focused test command pattern: `cargo nextest run --locked -p <package> --all-features <test-name-filter>`, or `-E 'test(<name>)'` for a nextest filterset; drop `--all-features` when the package is `workshop`, `workshop-server`, or `workshop-server-api`. The packages this plan touches are `runvil-parser`, `runvil-types`, `runvil-model-client`, `runvil-vfs`, `runvil-lua`, `runvil-engine`, `runvil` (facade suite binary `suite`, fixtures under `tests/prompts/`), `build-xtask`, and, for the run seed, `harness-runner`, `harness`, `harness-gateway-client`, `workshop-run-log`, and `workshop-agents`. The engine bench is a criterion target: `cargo bench --locked -p runvil-engine --features test-support --bench models_loop -- --test` runs each benchmark once.
- Component test command pattern: `cargo nextest run --locked -p <package> --all-features` (again without `--all-features` for `workshop`, `workshop-server`, `workshop-server-api`).
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`. CI also runs `npm test --workspaces --if-present` in `crates/workshop` and `npm test` in `crates/gateway/config-ui/ui`.
- Linter command: `CARGO_BUILD_WARNINGS=deny cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features` and `CARGO_BUILD_WARNINGS=deny cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`. Companion gates: `cargo check -p gateway --no-default-features` (headless build shape), `cargo test -p build-xtask` (boundary and structural checks, including the Engine manifest guard `engine_deps.rs`, the doctest ban, and the `test-support` leak guard), `node --test crates/workshop/ui/test/docs-claims.mjs` (wording and rulebook scan), `cargo +nightly-2026-09-05 xtask api --check` (facade surface against `crates/runvil/public-api.txt`; `--bless` rewrites it; the nightly is pinned in `crates/build-xtask/src/api/toolchain.rs`), and `cargo deny check`. Never run a standalone `cargo check --workspace` beside clippy.
- Formatter check command: `cargo fmt --all --check` (rustfmt `style_edition = "2024"`; the pre-commit hook runs it).
- Docs command: `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`, plus the facade build with default features: `RUSTDOCFLAGS="-D warnings" cargo doc -p runvil --no-deps`.
- Test placement and naming conventions: Unit tests take three forms, all common: a sibling file `<module>-tests.rs` wired at the end of the module with `#[cfg(test)] #[path = "<module>-tests.rs"] mod tests;` (a large one splits into `tests-<label>.rs` siblings); an inline `#[cfg(test)] mod tests { }`; or `#[cfg(test)] mod tests;` with a `tests.rs` or a `src/**/tests/` directory, as `runvil-lua`, `runvil-parser`, and the engine's `src/execute/tests/` do. Integration tests compile as one binary per crate at `tests/it/main.rs` (most crates) or `tests/suite/main.rs` (`runvil`, `harness`), with one `mod` file per area plus a `support` module. Engine prompt fixtures live in `crates/runvil/tests/prompts/`. Test functions are snake_case sentences that state the behavior, such as `a_direct_launch_recovers_the_lease_from_a_terminated_owner`; Miri-run tests carry a `miri_` prefix. Engine test support lives in `crates/runvil-internal/engine/src/test_support.rs` and `src/test_support/`, behind `#[cfg(test)]` or the `test-support` feature; the parser has a `test-support`-gated `test_support` module.
- Directory map: `crates/` holds all code. Its top level is the public and shared layer: `runvil` (the Engine's public API facade), `runvil-plugin` (the Plugin API), `harness` and `harness-gateway-client`, the linked Plugins `plugin-web`, `plugin-mcp`, and `plugin-user-input`, `gateway-api-types` and `gateway-api-discovery`, `shared-error-source` and `shared-loopback`, `shared-ui` (a TypeScript and CSS package), and the `build-*` tooling (`build-xtask` behind `cargo xtask`, `build-workshop` behind `cargo workshop`, plus `build-ui`, `build-user-guide`, `build-llama-cuda`, `build-ceiling`). Four manifestless containers hold each family's private crates: `crates/runvil-internal/` (Engine: `types`, `parser`, `lua`, `engine`, `vfs`, `model-client`), `crates/harness-internal/` (`runner`, package `harness-runner`), `crates/gateway/` (the `app` binary and its family crates), and `crates/workshop/` (the Tauri `desktop` app, `server`, `server-api`, the tier crates such as `run-log`, and the npm workspace members `ui`, `look`, `platform`). Outside `crates/`: `guide/` is the user guide source; `prompts/` holds sample prompt programs; `vibe/` holds dated implementation plans and `vibe/ACTIVE`; `tools/` holds Node scripts; `.github/workflows/` holds CI; `.githooks/` holds pre-commit (fmt) and pre-push (wording scan, headless gateway check, clippy, cargo deny); `.cargo/config.toml` sets the `xtask` and `workshop` aliases; `.config/nextest.toml` sets timeouts and the heavy STT test group; root `deny.toml`, `clippy.toml`, and `rustfmt.toml` configure the gates.
- Component boundaries: the `runvil` facade depends on all six `runvil-internal` crates; `runvil-engine` depends on the other five; `runvil-parser` depends on `runvil-lua` and `runvil-types`; `runvil-lua` on `runvil-model-client`, `runvil-types`, and `runvil-vfs`; `runvil-types` and `runvil-vfs` are leaves. `runvil-plugin` depends on `runvil-types` and `runvil-vfs`. `harness-runner` depends on `runvil` and `runvil-plugin`; `harness` on `harness-runner`, `runvil`, and `runvil-plugin`; `harness-gateway-client` on `harness`, `plugin-web`, and `runvil`. The Workshop's `run-log` implements the Harness's run recorder, and `workshop-server` is the Workshop composition root. Nothing in the gateway depends on the Engine or Harness. `cargo test -p build-xtask` enforces the product and container boundaries and lint inheritance.
- Conventions summary: Rust edition 2024 on the stable toolchain with resolver 3 and every dependency declared once in `[workspace.dependencies]`, each non-obvious choice explained by a comment. Workspace lints deny clippy `all` and `pedantic`, `unwrap_used` and `expect_used` (allowed in tests by `clippy.toml`), bare `allow` attributes (use `#[expect(..., reason = "...")]`), and `unsafe_code`; `missing_docs`, `missing_debug_implementations`, and `unreachable_pub` warn and the gate denies warnings; the root `clippy.toml` bans process-global installers outside binary entry points. Doc comments hold no compiled code blocks, and a `test-support` feature is enabled only from `[dev-dependencies]`. Source directories are flat: a subdirectory needs at least three files, otherwise modules sit beside the parent as `parent-label.rs` wired with `#[path]`. Every source file stays under the line ceiling `build_ceiling::check` enforces from each crate's `build.rs`. Engine, Harness, Host, and Plugin are capitalized defined terms, and Engine crates say "the caller" instead of naming the Host. Comments state only non-obvious constraints. Error messages are written for model consumption and name required versus actual. Behavior changes ship with tests in the same change. JSON reaching a recorder or replay round-trips exactly with sorted keys. No backward compatibility: signatures and record formats change outright. Text files are LF. Commit subjects are imperative sentence case, and a finished plan lands as a `Close plan: <slug>` commit.

</project-survey>

<execution-plan>

## Execution Instructions

Eighteen steps in nine components, one commit per step. Each component is one work item, named in bold below, so the references to work items elsewhere in this plan still hold.

Re-consolidation, measured at `594eb22a7` on a clean tree:

- Test lines: engine 35,714, lua 11,052, vfs 3,811, parser 2,366, types 2,282, model-client 727. Step 1 saves the real baseline.
- Banned calls in the Engine crates: the facade's `timestamp.rs` clock read; the `recv_timeout` in types' `cancel-tests.rs`; five `rand::random` sites in types' `untrusted/tests.rs`; two `Instant::now` in lua's `tests/cancellation.rs`; and in the engine, a `std::thread::sleep` in the plain `#[test]` `execute/run/tests.rs`, the `wait_timeout` in `execute/tests/scheduler/store_gate.rs`, four `Instant::now`, and 20 `tokio::time` sleeps and timeouts in `test_support/scripted-chat.rs`, `test_support/tokio_driver.rs`, and 14 test files. The `tokio::time` sites leave with tokio.
- Engine: 562 `#[tokio::test]` in 84 files (131 multi-threaded) beside 267 plain `#[test]`. 55 files call `TokioDriver::new`. Fixture tools are async (`TestTool::call` returns a boxed future), so the serial driver needs `FnTool` before it performs its first tool call.
- Resolved graphs: the facade reaches `fastrand` and `getrandom`; the engine `async-trait`, `fastrand`, `getrandom`, and `tokio`; types `getrandom`; vfs `fastrand` and `getrandom`. The other four Engine crates reach none.
- Corrections to the counts above: types and model-client hold 31 `json!` uses, not about 71. The hand-built 4096-window bindings are seven, not four. Inline engine frontmatter literals are about 220 lines in 53 files; the parser has about 83.
- Levels: the scheduler-directory files `walk/{core_rules,child_levels,jumps,driving}.rs`, `live_h1/*`, `fanout/*`, and `failures-script-tools.rs` inspect nothing below `Run`, so they move to `Scenario`. Tests keep their files when their driving changes.

Components, in dependency order:

1. **parser** (Step 1). First, because the **engine Run level** item imports its source builder, and the parser is the one crate both the parser's tests and the engine's see. One piece.
2. **types and model-client** (Step 2). Leaf crates. The **run seed** and **determinism policing** items need `Seeded` in types and `rand` gone. One piece, built jointly, because both crates get the same `round_trip` and `assert_json` shape.
3. **vfs and TempRoot** (Steps 3-4). A leaf crate, placed before the engine so the engine suite's `vfs.rs` already uses `TempRoot` when Step 8 rewrites it. Two pieces, sequential, because `TempRoot` must be its own commit: the vfs test backend, then `TempRoot` in three crates.
4. **lua** (Steps 5-6). Before the **run seed** item, so its seed callers sit in `TestVm`. Two pieces, sequential, because the tool-fixture tests build on the `TestVm` the section-VM piece lands.
5. **engine Run level** (Steps 7-12). After the parser's builder and `TempRoot`. Six pieces, sequential, each extending the utilities the last one left: foundations, then `Scenario` and `FnTool` over the offline suite, then the reply script over context-built runs, then tool loops, then chat and section-loop runs, then Run-level model tasks and scheduler-directory walks.
6. **engine scheduler level** (Steps 13-14). After the Run level, because the rig reuses `Scenario`'s stepper. Two pieces, sequential: the rig with the effect tap, round capture, and `RunState` internals, then held-and-released answers and seeded shuffles.
7. **tokio removal** (Step 15). After every engine test drives serially. One piece.
8. **run seed** (Step 16). After **lua**, **types and model-client**, and **tokio removal**, so its test callers go through `TestVm`, `Seeded`, the two `Scenario`s, and the rig. One piece in one commit, because the `RunContext::new` signature change breaks every caller at once.
9. **determinism policing** (Steps 17-18). Last, because its guards fail until the types, vfs, lua, and tokio-removal items have cleared their crates. It does not depend on **run seed**, so the two can swap if one has to wait. Two pieces, sequential, each its own bisectable commit: the dependency guard, then the clippy bans.

The rulebook-fix plan starts only after this plan's close commit. Every step runs its own checks and the test-name comparison; Step 18 runs the exit criteria.

<step-1>

### Step 1: Parser source builder and parse assertions [completed]

- Component: parser
- Tests:
  - `cargo nextest run --locked -p runvil-parser --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p runvil-parser --all-targets --all-features`
  - `cargo fmt --all --check`
  - `$s='../cabinet/_scratch/vibe-coder-internal-test-harnesses'; foreach ($p in 'runvil-parser') { cargo nextest list --locked -p $p --all-features | Out-File -Encoding utf8 "$s/$p-after.txt"; Compare-Object (Get-Content "$s/$p-before.txt") (Get-Content "$s/$p-after.txt") | ForEach-Object { "$p full $($_.SideIndicator) $($_.InputObject)" }; Compare-Object (Get-Content "$s/$p-before.txt" | ForEach-Object { ($_ -split '::')[-1] }) (Get-Content "$s/$p-after.txt" | ForEach-Object { ($_ -split '::')[-1] }) | ForEach-Object { "$p leaf $($_.SideIndicator) $($_.InputObject)" } }`
- Rules: PowerShell, run from the repository root, which every relative path here (`crates/...`, `../cabinet/...`) resolves against. Run cargo commands one at a time and never append `2>&1`; judge a cargo step by its result lines and `$LASTEXITCODE`. Read the root `AGENTS.md` and the nearest `AGENTS.md` above each crate you edit. A file leaves by `Move-Item` to `../cabinet/_trash/`. No test is removed or weakened, no dependency is added, and test helpers are `#[track_caller]` with failures that name what was being set up.
- Before any edit, save the baselines into `../cabinet/_scratch/vibe-coder-internal-test-harnesses/` (written `$s` below):
  - `New-Item -ItemType Directory -Force ../cabinet/_scratch/vibe-coder-internal-test-harnesses | Out-Null`
  - `git rev-parse HEAD | Out-File -Encoding utf8 ../cabinet/_scratch/vibe-coder-internal-test-harnesses/start-commit.txt`
  - The test lists, one package at a time: `foreach ($p in 'runvil-parser','runvil-types','runvil-model-client','runvil-vfs','runvil-lua','runvil-engine','runvil','build-xtask','harness-runner','harness','harness-gateway-client','workshop-run-log','workshop-agents') { cargo nextest list --locked -p $p --all-features | Out-File -Encoding utf8 "../cabinet/_scratch/vibe-coder-internal-test-harnesses/$p-before.txt" }`
  - The test line counts into `../cabinet/_scratch/vibe-coder-internal-test-harnesses/lines-before.txt`, from the repository root: `foreach ($c in 'engine','lua','vfs','parser','types','model-client') { $n = (Get-ChildItem "crates/runvil-internal/$c" -Recurse -Filter *.rs | Where-Object { $_.FullName -match '[\\/](tests|test_support)[\\/]|[\\/](tests|test_support)\.rs$|-tests\.rs$|[\\/]tests-[^\\/]+\.rs$' } | ForEach-Object { (Get-Content $_.FullName).Count } | Measure-Object -Sum).Sum; "$c $n" } | Out-File -Encoding utf8 ../cabinet/_scratch/vibe-coder-internal-test-harnesses/lines-before.txt`
  - If a baseline file is ever missing, rebuild it from a `git worktree` of the commit in `start-commit.txt`, placed in that scratch directory, never from an edited tree.
- In `crates/runvil-internal/parser/src/lib.rs`, widen the gate on `pub mod test_support` from `#[cfg(feature = "test-support")]` to `#[cfg(any(test, feature = "test-support"))]`, and update its doc. The engine already enables the feature from its dev-dependencies.
- In `crates/runvil-internal/parser/src/test_support.rs`, beside `synthetic_section` and `prose_block`, add the source builder in the facade harness's vocabulary:
  - `prompt(front: &str, sections: &[(&str, &str)]) -> String`: `---`, `name: t`, `description: d`, `runvil: 0`, the trimmed `front` lines, `---`, a blank line, `# Test prompt`, then per entry a heading and one `lua`-fenced body. A heading that starts with `#` is written as given, so H3 and H4 children can be expressed; any other heading gets `## `. Each body is trimmed as a whole and then line by line, as the facade suite's `push_trimmed` does, so sources can be indented raw strings.
  - `one_section(lua: &str) -> String`, which is `prompt("", &[("Only", lua)])`.
  - `PromptSource`, a small builder for shapes the two functions cannot express: `.name(..)`, `.title(..)`, `.front(..)`, `.h1(lua)`, `.section(heading, lua)`, `.prose(heading, text)`, `.build()`, with the same defaults and trimming.
  - `parse_ok(src) -> Prompt`, `parse_err(src) -> ParseError`, and `assert_parse_error(src, kind: ParseErrorKind, fragment: &str)`, each `#[track_caller]`, whose failures show the source's first lines and the actual outcome.
- Rewrite the parser's tests on them: the two `parse` copies (`src/tests.rs`, `src/contract/tests.rs`) and `prompt_src` in `src/tests.rs` go, and the inline frontmatter literals in `src/tests.rs`, `src/tests/{breaks,frontmatter,h1_fences,headings,lists,section_fences}.rs`, and `src/contract/tests.rs` and `tests-plugins.rs` become builder calls.
- Literal sources stay where layout is the subject: all of `src/tests/line_mapping.rs`, the CRLF, BOM, whitespace, and fence-shape cases, and any test that asserts a line number, column, or byte offset.
- Acceptance: the parser's tests pass with the same names; its rewritten test files get shorter, and its total test line count may rise by no more than the builder's lines in `src/test_support.rs`, which the engine reuses. The leaf comparison prints nothing marked `<=` and nothing marked `=>`; full-line differences that are not leaf differences are moves, each listed in the commit message.

</step-1>

<step-2>

### Step 2: Types and model-client serde helpers, Seeded, and no rand [completed]

- Component: types and model-client
- Tests:
  - `cargo nextest run --locked -p runvil-types --all-features`
  - `cargo nextest run --locked -p runvil-model-client --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p runvil-types --all-targets --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p runvil-model-client --all-targets --all-features`
  - `cargo fmt --all --check`
  - `rg -n -w "rand" crates/runvil-internal/types/Cargo.toml crates/runvil-internal/types/src`
  - `$s='../cabinet/_scratch/vibe-coder-internal-test-harnesses'; foreach ($p in 'runvil-types','runvil-model-client') { cargo nextest list --locked -p $p --all-features | Out-File -Encoding utf8 "$s/$p-after.txt"; Compare-Object (Get-Content "$s/$p-before.txt") (Get-Content "$s/$p-after.txt") | ForEach-Object { "$p full $($_.SideIndicator) $($_.InputObject)" }; Compare-Object (Get-Content "$s/$p-before.txt" | ForEach-Object { ($_ -split '::')[-1] }) (Get-Content "$s/$p-after.txt" | ForEach-Object { ($_ -split '::')[-1] }) | ForEach-Object { "$p leaf $($_.SideIndicator) $($_.InputObject)" } }`
- Rules: PowerShell, run from the repository root, which every relative path here (`crates/...`, `../cabinet/...`) resolves against. Run cargo commands one at a time and never append `2>&1`; judge a cargo step by its result lines and `$LASTEXITCODE`. Read the root `AGENTS.md` and `crates/runvil-internal/types/AGENTS.md`. A file leaves by `Move-Item` to `../cabinet/_trash/`. No test is removed or weakened, no dependency is added, and test helpers are `#[track_caller]`. Add only the test-support surface this step's tests use, since unused test code fails the gate as dead code.
- types: add `#[cfg(test)] mod test_support;` in `crates/runvil-internal/types/src/lib.rs` and `src/test_support.rs` with:
  - `Seeded`, a SplitMix64 generator: `Seeded::new(seed: u64)`, `seed()`, and the draws the property tests need (such as `next_u64`, `below(n)`, `pick(&[T])`). Its `#[track_caller]` assertion helpers put `seed <n>` in every failure message, so a failure can be replayed.
  - `round_trip(value) -> T` (serialize to JSON text and back) and `assert_json(value, json!(..))` (the value's JSON shape).
  - One `assert_send_sync::<T>()`, replacing the copies in `src/event-tests.rs`, `src/tools/tests.rs` (two), and `src/cancel-tests.rs`.
- In `src/untrusted/tests.rs`, `GuardNonce::from_seed(rand::random())` (line 9) and the four `rand::random::<u8>()` draws in the two property loops (about lines 231-234 and 391-394) take their values from `Seeded` with a fixed seed; each loop's failure names that seed. Any input that fails under `Seeded` is committed beside the loop as a fixed case.
- Remove `rand` and the comment above it from `[dev-dependencies]` in `crates/runvil-internal/types/Cargo.toml`, then refresh `Cargo.lock` once with `cargo metadata --format-version 1 | Out-Null`, without `--locked`. Its diff only drops `runvil-types`' `"rand",` edge.
- In `src/cancel-tests.rs`, the `.recv_timeout(std::time::Duration::from_secs(5))` (about line 149) becomes a plain `.recv()`. A hang then fails through nextest's timeout in `.config/nextest.toml`.
- The round-trip and JSON-shape checks in `src/event-tests.rs` (its `round_trips`), `ids-tests.rs`, `plugins-tests.rs`, `replay-tests.rs`, `timestamp-tests.rs`, `emitter-tests.rs`, `tools/tests.rs`, and the inline tests in `src/metrics.rs` (its `round_trips`) use the shared helpers.
- model-client: add `#[cfg(test)] mod test_support;` and `src/test_support.rs` with its own `round_trip` and `assert_json` and one wire decode helper for the decode-and-check sequence `src/client/tests.rs` repeats; `src/normalize-tests.rs` and the `src/model/*tests.rs` files use them where they round-trip or check JSON shape.
- New test, in types' test support: `seeded_failure_messages_name_the_seed`, which catches a failing `Seeded` assertion's panic and checks that its message contains the seed.
- Acceptance: both crates pass with the same names plus the new test; the `rg` prints nothing; the rewritten test files in both crates get shorter, and any rise in a crate's total comes only from its new `test_support.rs` (`Seeded`, the shared helpers, and the self-test). The leaf comparison prints nothing marked `<=`, and `=>` only `seeded_failure_messages_name_the_seed` under `runvil-types`. Full-line differences that are not leaf differences are moves, each listed in the commit message.

</step-2>

<step-3>

### Step 3: vfs configurable test backend and handle helpers [completed]

- Component: vfs and TempRoot
- Tests:
  - `cargo nextest run --locked -p runvil-vfs --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p runvil-vfs --all-targets --all-features`
  - `cargo fmt --all --check`
  - `$s='../cabinet/_scratch/vibe-coder-internal-test-harnesses'; foreach ($p in 'runvil-vfs') { cargo nextest list --locked -p $p --all-features | Out-File -Encoding utf8 "$s/$p-after.txt"; Compare-Object (Get-Content "$s/$p-before.txt") (Get-Content "$s/$p-after.txt") | ForEach-Object { "$p full $($_.SideIndicator) $($_.InputObject)" }; Compare-Object (Get-Content "$s/$p-before.txt" | ForEach-Object { ($_ -split '::')[-1] }) (Get-Content "$s/$p-after.txt" | ForEach-Object { ($_ -split '::')[-1] }) | ForEach-Object { "$p leaf $($_.SideIndicator) $($_.InputObject)" } }`
- Rules: PowerShell, run from the repository root, which every relative path here (`crates/...`, `../cabinet/...`) resolves against. Run cargo commands one at a time and never append `2>&1`; judge a cargo step by its result lines and `$LASTEXITCODE`. Read the root `AGENTS.md` and `crates/runvil-internal/vfs/AGENTS.md`. A file leaves by `Move-Item` to `../cabinet/_trash/`. No test is removed or weakened, no dependency is added, and test helpers are `#[track_caller]`. Add only the test-support surface this step's tests use. vfs stays std only.
- Add `#[cfg(test)] mod test_support;` in `crates/runvil-internal/vfs/src/lib.rs` and `src/test_support.rs` with one configurable test backend: a `Vfs` whose accesses default to a `MemoryBackend` and whose operations can each be overridden by a closure or recorded into a shared call log. It replaces the hand-written backends that each re-implement `acquire`, `release`, `read`, `write`, `append`, `remove`, `exists`, `glob`, `list`, `stat`, `mkdir`, `rename`, and `copy`: `StubFs` and `StubAccess` (`src/router-tests.rs`, `src/handle/tests.rs`), `StubBackend` (`src/traits-tests.rs`), `Refusing` and `ReadOnly` (`src/detail-tests.rs`), and `RefusingFs` and `RefuseSecond` (`src/handle/tests/refusals.rs`).
- In the same module: a handle-builder shortcut (a `VfsRef` with the store declared at `/` over a given backend) and an `access()` helper (acquire under a test origin) for the build-and-acquire sequences in `src/handle/tests/*.rs`, `src/router-tests.rs`, and `src/detail-tests.rs`.
- One definition each of `seeded` (`memory-tests.rs`, `router-tests.rs`, `handle/tests.rs`), `test_origin` (`router-tests.rs`, `handle/tests.rs`), `read_only` (`detail-tests.rs`, `router-tests.rs`), `files` (`router-tests.rs`, `handle/tests.rs`), `path` (`memory-tests.rs`, `traits-tests.rs`, `real/tests.rs`), and `context` (`memory-tests.rs`, `real/tests.rs`). Copies whose bodies genuinely differ get distinct names.
- Leave `src/real/tests.rs`'s `temp_dir` and `tempfile` for Step 4.
- Acceptance: vfs passes with the same names; no `Vfs` or `VfsAccess` implementation remains in a vfs test file outside `src/test_support.rs`, apart from ones whose behavior is the test's subject and that the closures cannot express (name each in the commit message); the test line count drops. The leaf comparison prints nothing marked `<=` and nothing marked `=>`; full-line differences that are not leaf differences are moves, each listed in the commit message.

</step-3>

<step-4>

### Step 4: TempRoot replaces tempfile in the facade, engine, and vfs [completed]

- Component: vfs and TempRoot
- Tests:
  - `cargo nextest run --locked -p runvil-vfs --all-features`
  - `cargo nextest run --locked -p runvil-engine --all-features`
  - `cargo nextest run --locked -p runvil --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p runvil-vfs --all-targets --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p runvil-engine --all-targets --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p runvil --all-targets --all-features`
  - `cargo fmt --all --check`
  - `rg -n "tempfile|SystemTime::now" crates/runvil/Cargo.toml crates/runvil-internal/engine/Cargo.toml crates/runvil-internal/vfs/Cargo.toml crates/runvil/tests crates/runvil-internal/engine/src crates/runvil-internal/vfs/src`
  - `foreach ($c in 'runvil','runvil-engine','runvil-vfs') { $hits = cargo tree --locked -q -p $c --all-features -e normal,build,dev --prefix none | ForEach-Object { ($_ -split ' ')[0] } | Where-Object { $_ -in 'tokio','tokio-util','async-trait','reqwest','getrandom','fastrand' } | Sort-Object -Unique; "{0}: {1}" -f $c, ($hits -join ', ') }`
  - `$s='../cabinet/_scratch/vibe-coder-internal-test-harnesses'; foreach ($p in 'runvil-vfs','runvil-engine','runvil') { cargo nextest list --locked -p $p --all-features | Out-File -Encoding utf8 "$s/$p-after.txt"; Compare-Object (Get-Content "$s/$p-before.txt") (Get-Content "$s/$p-after.txt") | ForEach-Object { "$p full $($_.SideIndicator) $($_.InputObject)" }; Compare-Object (Get-Content "$s/$p-before.txt" | ForEach-Object { ($_ -split '::')[-1] }) (Get-Content "$s/$p-after.txt" | ForEach-Object { ($_ -split '::')[-1] }) | ForEach-Object { "$p leaf $($_.SideIndicator) $($_.InputObject)" } }`
- Rules: PowerShell, run from the repository root, which every relative path here (`crates/...`, `../cabinet/...`) resolves against. Run cargo commands one at a time and never append `2>&1`; judge a cargo step by its result lines and `$LASTEXITCODE`. Read the root `AGENTS.md` and the `AGENTS.md` of the engine and vfs. A file leaves by `Move-Item` to `../cabinet/_trash/`. No test is removed or weakened, no dependency is added, and test helpers are `#[track_caller]`.
- Three std-only copies of `TempRoot`, one per crate's own test code: the facade suite's `crates/runvil/tests/suite/support.rs`; the engine's `#[cfg(test)]` test support, as `crates/runvil-internal/engine/src/test_support/temp_root.rs` wired from `test_support.rs`; and vfs's `src/real/tests.rs`, replacing `temp_dir()` and `TempDir` there and in `src/real/tests/{semantics,atomicity,links}.rs`.
- Each copy behaves the same way:
  - `TempRoot::new(label)` makes `std::env::temp_dir()/runvil-tests/<crate>/<label>-<pid>-<n>`, where `label` is the calling test's name, `pid` is `std::process::id()`, and `n` comes from one process-wide `AtomicU64`.
  - It removes anything already at that path, creates the folder, and removes it on drop. `path()` returns it. A failure to create or remove it names the path.
  - The process id keeps nextest's per-test processes apart, the counter keeps tests on threads in one process apart, and nothing reads a clock or a random generator.
- Call sites: the facade's `tests/suite/prepare.rs` (`two_runs_writing_the_same_real_file_through_the_shared_base_conflict`), the engine's `src/execute/tests/suite/vfs.rs` (its real-backend fanout run), and every vfs real-backend test.
- Remove `tempfile` and the comment above it from `[dev-dependencies]` in `crates/runvil/Cargo.toml`, `crates/runvil-internal/engine/Cargo.toml`, and `crates/runvil-internal/vfs/Cargo.toml`. vfs's `[dev-dependencies]` table goes, since `tempfile` is its only entry, and its zero-dependency comment stays true; its manifest test still allows dev-dependencies. Refresh `Cargo.lock` once with `cargo metadata --format-version 1 | Out-Null`, without `--locked`; its diff only drops the three `"tempfile",` edges.
- In the same commit, `crates/runvil/tests/suite/timestamp.rs` stamps a fixed `Timestamp` (such as `Timestamp::from_unix_millis(1_700_000_000_000)`) instead of reading `SystemTime::now`, drops its `std::time` import, and keeps both assertions.
- New tests, one in each copy, all named `temp_roots_with_one_label_get_distinct_paths_and_drop_removes_the_folder`: two roots made with one label get different paths, and dropping one removes its folder.
- Acceptance: the `rg` prints nothing; the graph loop prints an empty list for `runvil` and `runvil-vfs` and only `async-trait, tokio` for `runvil-engine`. The leaf comparison prints nothing marked `<=`, and `=>` only the new `TempRoot` test, once under each of `runvil-vfs`, `runvil-engine`, and `runvil`. Full-line differences that are not leaf differences are moves, each listed in the commit message.

</step-4>

<step-5>

### Step 5: lua TestVm and the section-VM tests [completed]

- Component: lua
- Tests:
  - `cargo nextest run --locked -p runvil-lua --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p runvil-lua --all-targets --all-features`
  - `cargo fmt --all --check`
  - `rg -n "Instant|elapsed|from_seed" crates/runvil-internal/lua/src/tests crates/runvil-internal/lua/src/tests.rs crates/runvil-internal/lua/src/globals-tests.rs crates/runvil-internal/lua/src/prelude-tests.rs crates/runvil-internal/lua/src/messages-tests.rs crates/runvil-internal/lua/src/models-tests.rs`
  - `$s='../cabinet/_scratch/vibe-coder-internal-test-harnesses'; foreach ($p in 'runvil-lua') { cargo nextest list --locked -p $p --all-features | Out-File -Encoding utf8 "$s/$p-after.txt"; Compare-Object (Get-Content "$s/$p-before.txt") (Get-Content "$s/$p-after.txt") | ForEach-Object { "$p full $($_.SideIndicator) $($_.InputObject)" }; Compare-Object (Get-Content "$s/$p-before.txt" | ForEach-Object { ($_ -split '::')[-1] }) (Get-Content "$s/$p-after.txt" | ForEach-Object { ($_ -split '::')[-1] }) | ForEach-Object { "$p leaf $($_.SideIndicator) $($_.InputObject)" } }`
- Rules: PowerShell, run from the repository root, which every relative path here (`crates/...`, `../cabinet/...`) resolves against. Run cargo commands one at a time and never append `2>&1`; judge a cargo step by its result lines and `$LASTEXITCODE`. Read the root `AGENTS.md` and `crates/runvil-internal/lua/AGENTS.md`. A file leaves by `Move-Item` to `../cabinet/_trash/`. No test is removed or weakened, no dependency is added, and test helpers are `#[track_caller]`. Add only the test-support surface this step's tests use.
- New `#[cfg(test)] pub(crate) mod test_vm;` at `crates/runvil-internal/lua/src/test_vm.rs`, separate from the `test-support` feature that serves the lua bench:
  - `TestVm::new()` with `.tools(..)`, `.plugins(..)`, `.models(..)`, `.argv(..)`, `.var(..)`, `.messages()`, and `.build()`, returning a struct with named handles (the VM or `Lua`, its runtime, its sets, its access) instead of tuples, plus `eval::<T>(src)` and `eval_err(src)`.
  - `TEST_SEED: u64`, the one nonce seed for test VMs. It replaces `test_nonce`'s `0x6c75_6174_6573` (`src/tests.rs`), `11` (`globals-tests.rs`), `7` (`prelude-tests.rs`), and `1` (`messages-tests.rs`, `models-tests.rs`); Step 6 replaces the rest. No lua test pins a nonce literal at `594eb22a7`.
  - One `fresh_access()`, replacing the copies in `src/tests.rs`, `messages-tests.rs`, and `prelude-tests.rs` (Step 6 takes `tools/tests.rs`'s).
- Convert on it: the `src/tests.rs` hub (`section_vm_with_set`, `section_vm_with_bindings`, `section_vm_after`, `shim_vm`, `test_nonce`); the construct-inject-install sequences (`SectionVm::new`, then `inject_values`, then `install_engine_globals`) in `src/tests/{argv,budgets,cancellation,diagnostics,logging,sandbox,store,store_errors,store_reports,sys_and_var,tool_scoping}.rs` and `src/tests/require*.rs`; `globals-tests.rs` (`section_vm`, `eval`); `prelude-tests.rs` (`section_vm`, `section_vm_with_var`, `eval`); `messages-tests.rs` (`section_vm`, `eval`); `messages-tests-list.rs` and `iteration-tests.rs` (`vm()`); and `models-tests.rs` (`models_vm`, `h2_vm`). `src/tests/section_vm.rs` keeps explicit construction where construction itself is under test.
- The two tests in `src/tests/cancellation.rs` drop `Instant::now` and their elapsed-time bound for a deterministic one. The hook returns before counting once the cancel flag is set (`budget_hook` in `src/hardening.rs`), so add a `#[cfg(test)]` reader of `InstructionBudget`'s `fired` counter, and a `#[cfg(test)]` reach to it through `SectionVm` for the second test, and assert that the count did not advance across the cancelled call: the first hook firing aborted it. The existing outcome and message assertions stay. A hang fails through nextest's timeout.
- `src/vm/run.rs`'s production `GuardNonce::from_seed(0)` is untouched; Step 16 changes it.
- Acceptance: the `rg` prints nothing; lua passes with the same names; its test line count drops. The leaf comparison prints nothing marked `<=` and nothing marked `=>`; full-line differences that are not leaf differences are moves, each listed in the commit message.

</step-5>

<step-6>

### Step 6: lua tool fixtures on TestVm [completed]

- Component: lua
- Tests:
  - `cargo nextest run --locked -p runvil-lua --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p runvil-lua --all-targets --all-features`
  - `cargo fmt --all --check`
  - `rg -n "SectionVm::new\(|GuardNonce::from_seed\(" crates/runvil-internal/lua/src -g '*test*' -g '**/tests/**'`
  - `$s='../cabinet/_scratch/vibe-coder-internal-test-harnesses'; foreach ($p in 'runvil-lua') { cargo nextest list --locked -p $p --all-features | Out-File -Encoding utf8 "$s/$p-after.txt"; Compare-Object (Get-Content "$s/$p-before.txt") (Get-Content "$s/$p-after.txt") | ForEach-Object { "$p full $($_.SideIndicator) $($_.InputObject)" }; Compare-Object (Get-Content "$s/$p-before.txt" | ForEach-Object { ($_ -split '::')[-1] }) (Get-Content "$s/$p-after.txt" | ForEach-Object { ($_ -split '::')[-1] }) | ForEach-Object { "$p leaf $($_.SideIndicator) $($_.InputObject)" } }`
- Rules: PowerShell, run from the repository root, which every relative path here (`crates/...`, `../cabinet/...`) resolves against. Run cargo commands one at a time and never append `2>&1`; judge a cargo step by its result lines and `$LASTEXITCODE`. Read the root `AGENTS.md` and `crates/runvil-internal/lua/AGENTS.md`. A file leaves by `Move-Item` to `../cabinet/_trash/`. No test is removed or weakened, no dependency is added, and test helpers are `#[track_caller]`. Add only the test-support surface this step's tests use.
- Convert the tool-facing tests onto `TestVm`: `plugins-tests.rs` (`lua_over`, `tool_at`, `scoped`), `tools/tests.rs` (`lua_over`, `lua_with_tools`, `lua_with_tools_and_runtime`, `tool_at`, `echo_tool`, `fresh_access`, its nonce seed `1`), `tools/tests-offering.rs` (`scoped`), and `dispatch-tests.rs` (`echo_tool`, its nonce seed `0xd15_9a7c`). One definition each of `lua_over`, `tool_at`, `scoped`, and `echo_tool` remains, in `test_vm.rs`.
- Then merge the remaining duplicated fixture builders, found with `rg -n "^fn \w+\(" crates/runvil-internal/lua/src -g '*test*' -g '**/tests/**'`: at `594eb22a7` they include `user`, `assistant`, `system` (`messages-tests-list.rs`, `projection-tests.rs`), `call` (`projection-tests.rs`, `models_loop/tests.rs`), `text` (`messages-tests-list.rs`, `messages-tests.rs`, `models_loop/machine-tests.rs`), `records` (`tests-recording.rs`, `models_loop/machine-tests.rs`), `run` (`messages-tests-list.rs`, `tests.rs`), `read` (`messages-tests-view.rs`, `tests.rs`), and `vm` (`iteration-tests.rs`, `messages-tests-list.rs`, if Step 5 left both), plus the `SECTION` constants (`dispatch-tests.rs`, `globals-tests.rs`, `prelude-tests.rs`, `tests/require.rs`), which become `TestVm`'s section name or get names that say how they differ. Merge where the bodies match; rename the ones that genuinely differ.
- Acceptance: the `rg` prints only `src/tests/section_vm.rs` and `src/test_vm.rs`; no free helper is defined in more than one lua test file unless renamed to say how it differs; lua passes with the same names; its test line count drops further. The leaf comparison prints nothing marked `<=` and nothing marked `=>`; full-line differences that are not leaf differences are moves, each listed in the commit message.

</step-6>

<step-7>

### Step 7: Engine Seeded and the synchronous ScriptedChat core [completed]

- Component: engine Run level
- Tests:
  - `cargo nextest run --locked -p runvil-engine --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p runvil-engine --all-targets --all-features`
  - `cargo bench --locked -p runvil-engine --features test-support --bench models_loop -- --test`
  - `cargo fmt --all --check`
  - `$s='../cabinet/_scratch/vibe-coder-internal-test-harnesses'; foreach ($p in 'runvil-engine') { cargo nextest list --locked -p $p --all-features | Out-File -Encoding utf8 "$s/$p-after.txt"; Compare-Object (Get-Content "$s/$p-before.txt") (Get-Content "$s/$p-after.txt") | ForEach-Object { "$p full $($_.SideIndicator) $($_.InputObject)" }; Compare-Object (Get-Content "$s/$p-before.txt" | ForEach-Object { ($_ -split '::')[-1] }) (Get-Content "$s/$p-after.txt" | ForEach-Object { ($_ -split '::')[-1] }) | ForEach-Object { "$p leaf $($_.SideIndicator) $($_.InputObject)" } }`
- Rules: PowerShell, run from the repository root, which every relative path here (`crates/...`, `../cabinet/...`) resolves against. Run cargo commands one at a time and never append `2>&1`; judge a cargo step by its result lines and `$LASTEXITCODE`. Read the root `AGENTS.md` and `crates/runvil-internal/engine/AGENTS.md`. A file leaves by `Move-Item` to `../cabinet/_trash/`. No test is removed or weakened, no dependency is added, and test helpers are `#[track_caller]`. Add only the test-support surface this step's tests use, since unused test code fails the gate as dead code.
- `crates/runvil-internal/engine/src/test_support/seeded.rs`, wired `#[cfg(test)]` from `test_support.rs`: the engine's copy of `Seeded`, the same shape as the types crate's (SplitMix64, `Seeded::new(seed: u64)`, `seed()`, the draws, and `#[track_caller]` assertion helpers whose failures name the seed). Its self-test, `seeded_failure_messages_name_the_seed`, catches a failing assertion's panic and checks that the message contains the seed, and exercises every method `Seeded` exposes, so none is dead code before Step 14 uses it.
- `test_support/scripted-chat.rs`: split `ScriptedChat::complete` into a synchronous core that records the round, counts it, and answers it, and keep the async `complete` as a thin wrapper over the core for the tokio driver's suites. In the core, a `ScriptedReply::Delayed` whose total delay exceeds the run's timeout answers the `Timeout` failure at once, and any other delay answers the inner reply at once. The async wrapper keeps today's record-then-wait order and its `tokio::time::timeout(timeout, tokio::time::sleep(delay))` wait until Step 15 deletes it. The bench still includes this file by `#[path]` until Step 15, so the file keeps naming only external crates.
- No test changes driver in this step.
- Acceptance: the engine passes with the same names plus the self-test; the bench runs once per benchmark. The leaf comparison prints nothing marked `<=`, and `=>` only `temp_roots_with_one_label_get_distinct_paths_and_drop_removes_the_folder` (from Step 4) and `seeded_failure_messages_name_the_seed` under `runvil-engine`.

</step-7>

<step-8>

### Step 8: Engine Scenario and FnTool over the offline fixture suite [completed]

- Component: engine Run level
- Tests:
  - `cargo nextest run --locked -p runvil-engine --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p runvil-engine --all-targets --all-features`
  - `cargo fmt --all --check`
  - `rg -n "tokio|RunFixture|run_with_fixture\b|run_prepared|macro_rules" crates/runvil-internal/engine/src/execute/tests/suite crates/runvil-internal/engine/src/execute/tests/suite.rs`
  - `$s='../cabinet/_scratch/vibe-coder-internal-test-harnesses'; foreach ($p in 'runvil-engine') { cargo nextest list --locked -p $p --all-features | Out-File -Encoding utf8 "$s/$p-after.txt"; Compare-Object (Get-Content "$s/$p-before.txt") (Get-Content "$s/$p-after.txt") | ForEach-Object { "$p full $($_.SideIndicator) $($_.InputObject)" }; Compare-Object (Get-Content "$s/$p-before.txt" | ForEach-Object { ($_ -split '::')[-1] }) (Get-Content "$s/$p-after.txt" | ForEach-Object { ($_ -split '::')[-1] }) | ForEach-Object { "$p leaf $($_.SideIndicator) $($_.InputObject)" } }`
- Rules: PowerShell, run from the repository root, which every relative path here (`crates/...`, `../cabinet/...`) resolves against. Run cargo commands one at a time and never append `2>&1`; judge a cargo step by its result lines and `$LASTEXITCODE`. Read the root `AGENTS.md` and `crates/runvil-internal/engine/AGENTS.md`. A file leaves by `Move-Item` to `../cabinet/_trash/`. No test is removed or weakened, no dependency is added, and test helpers are `#[track_caller]`. Add only the test-support surface this step's tests use. Every source file stays under 500 lines; split a file if it would cross.
- New `#[cfg(test)]` `crates/runvil-internal/engine/src/test_support/scenario.rs`:
  - `Scenario` holds setup as data: the source, built with the parser's `runvil_parser::test_support::{prompt, one_section, PromptSource}` or given literally; args; the run name, defaulting to `EXECUTION` (`"execute-test"`); `TEST_SEED: u64 = 1` and `TEST_STARTED_AT`, moved here from `execute/tests/context.rs`; a `.context(|ctx| ..)` hook; a store (`TestStore`, moved here) or a caller-built `VfsRef`; an observer; tools as `FnTool`s; and the prepare mode, either the implicit prepare `run_with_fixture` does today or a caller-supplied handle with no prepare, as `run_unprepared` does.
  - `.run() -> Trace` steps the run with `test_support::drive` on the calling thread. It answers a `Vfs` effect through `run_store_op`, a `Timer` at once, a `ToolCall` from the `FnTool` table (a miss is today's `refuse_tool_call` refusal), and a `Chat` with today's disabled-gateway refusal until Step 9 adds the scripted model.
  - `Trace` holds the `RunResult`, the events, the paired effect and answer records, the observations forwarded as `(execution, section, detail)` (parse-time events included, as `parse_execution_fixture` forwards them), and post-run store reads through fresh accesses. Its accessors, such as `text()` and `error()`, are `#[track_caller]` and name what was expected.
  - Move `perform_locally`, `text_of`, `infer_prompt`, `Batching`, and `drive_batched` (with `text_reply` and `tool_call_reply` until Step 9 replaces them) from `execute/tests/serial_driver.rs` into this module, and update the serial files that import them.
- `FnTool::new(id, schema, handler)` in `test_support/tools.rs`: a fixture tool from a closure `Fn(Value) -> Result<ToolOutput, ToolError>`, with `.description(..)` and `.structured(..)`. It implements `TestTool` with a ready future, so tokio-driven files can use it until Step 15, and `Scenario` calls it synchronously.
- Convert every test under `execute/tests/suite/` (about 160) to plain `#[test]` on `Scenario`:
  - From `suite/support.rs`, replace `context`, `RunOptions`, `prepare_run`, `drive`, `run`, `run_unprepared`, `run_fixture`, `FixtureRun`, `FixtureStore`, `Recorder`, `with_test_title`, and `parse_execution_fixture`; keep `Record` only if `Trace` reuses it. Nothing that replaces them keeps the dead `tools` or `_tools` parameter of `prepare_run`, `run`, and `suite/exec_flow.rs`'s `run`.
  - Replace the suite-local `run_offline` (`args_surface.rs`, `exec_flow.rs`, `lazy_prose.rs`, `require.rs`), `run_args` (`args_surface.rs`, `global_metatable.rs`), `EXECUTION` (`args_surface.rs`, `exec_flow.rs`, `lazy_prose.rs`, `global_metatable.rs`; `require.rs`'s `"fixture-require"` genuinely differs and gets its own name), `TestPrompt`, `TestStore`, `silent`, and `fixture` in `suite/exec_flow.rs`, `parse` in `suite/prepare.rs`, and `prelude` in `suite/require/failures.rs`, which `execute/tests/preludes.rs` shares.
  - Replace the macros `args_prompt!` (`args_surface.rs`, `global_metatable.rs`), `prose_prompt!` (`lazy_prose.rs`), `require_prompt!` (`require.rs`), `effect_prompt!` (`require/effects.rs`), and `flow_prompt!` (`suite/exec_flow.rs`), and the inline frontmatter literals in these files, with the parser's builder. Literals stay where layout is the subject, and fixture files under `crates/runvil/tests/prompts/` stay as they are.
  - `suite/exec_flow/run_setup.rs`'s `EchoTool` becomes an `FnTool`.
  - In `suite/execution.rs`, `concurrent_runs_keep_execution_ids_separate` interleaves its two runs step by step on the test thread instead of a tokio `Barrier` and two spawned tasks.
  - `suite/fanout.rs` and `suite/exec_flow/fanout_arms.rs` drop their `tokio::time::timeout` wrappers; a hang fails through nextest's timeout.
  - `suite/vfs.rs` keeps its `TempRoot` real-backend run through the caller-supplied-handle mode.
- Acceptance: the `rg` prints nothing; the engine passes with the same names. The leaf comparison prints nothing marked `<=`, and `=>` only `temp_roots_with_one_label_get_distinct_paths_and_drop_removes_the_folder` and `seeded_failure_messages_name_the_seed` under `runvil-engine`. Full-line differences that are not leaf differences are moves, each listed in the commit message.

</step-8>

<step-9>

### Step 9: Engine reply script and context-built runs [completed]

- Component: engine Run level
- Tests:
  - `cargo nextest run --locked -p runvil-engine --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p runvil-engine --all-targets --all-features`
  - `cargo fmt --all --check`
  - `rg -n "tokio::|resp_text|resp_tool_call|resp_failure|resp_delayed_text|macro_rules" crates/runvil-internal/engine/src/execute/tests.rs crates/runvil-internal/engine/src/execute/tests/exec_flow.rs crates/runvil-internal/engine/src/execute/tests/model_and_reply.rs crates/runvil-internal/engine/src/execute/tests/model_and_reply-globals.rs crates/runvil-internal/engine/src/execute/tests/model_and_reply-handles.rs crates/runvil-internal/engine/src/execute/tests/live_infer.rs crates/runvil-internal/engine/src/execute/tests/unified_pipeline.rs crates/runvil-internal/engine/src/execute/tests/debug_and_counts-infer-rounds.rs`
  - `$s='../cabinet/_scratch/vibe-coder-internal-test-harnesses'; foreach ($p in 'runvil-engine') { cargo nextest list --locked -p $p --all-features | Out-File -Encoding utf8 "$s/$p-after.txt"; Compare-Object (Get-Content "$s/$p-before.txt") (Get-Content "$s/$p-after.txt") | ForEach-Object { "$p full $($_.SideIndicator) $($_.InputObject)" }; Compare-Object (Get-Content "$s/$p-before.txt" | ForEach-Object { ($_ -split '::')[-1] }) (Get-Content "$s/$p-after.txt" | ForEach-Object { ($_ -split '::')[-1] }) | ForEach-Object { "$p leaf $($_.SideIndicator) $($_.InputObject)" } }`
- Rules: PowerShell, run from the repository root, which every relative path here (`crates/...`, `../cabinet/...`) resolves against. Run cargo commands one at a time and never append `2>&1`; judge a cargo step by its result lines and `$LASTEXITCODE`. Read the root `AGENTS.md` and `crates/runvil-internal/engine/AGENTS.md`. A file leaves by `Move-Item` to `../cabinet/_trash/`. No test is removed or weakened, no dependency is added, and test helpers are `#[track_caller]`. Add only the test-support surface this step's tests use. Every source file stays under 500 lines.
- The shared reply script, beside `ScriptedChat` in `test_support/scripted-chat.rs`: `text(..)`, `calls(..)`, and `fail(..)`, the facade harness's names, plus the variants the suites use (a text reply with a finish reason, a delayed reply), with `MOCK_MODEL` moved beside them. In one crate-wide sweep they replace `execute/tests/gateway.rs`'s `scripted_call`, `scripted_text`, `resp_tool_call`, `resp_two_tool_calls`, `resp_text`, `resp_delayed_text`, `resp_text_finish`, `resp_failure`, `echo_then_text_script`, and `aliased_tool_script` at every call site, tokio-driven files included. The canned `EffectAnswer` forms (`text_reply`, `tool_call_reply`, and `execute/run/tests-drops.rs`'s `text`) become answers built from the same constructors through the synchronous core. `gateway.rs` keeps only `OverflowClient` and `assert_model_failure` until Step 10.
- `Scenario` gains the scripted model (`.chat(ScriptedChat)`, performed through the synchronous core under the run's limits), the debug capture with `report_debug`, the bound-model path that `bound_for_model`, `bound_with_tools`, and `test_model_catalog` in `execute/tests/context.rs` set up, and a hold: `.hold(|effect| ..)` leaves matching effects unanswered while the run keeps stepping, a cancel can land between steps, and once the run decides, the driver answers every held effect `Dropped`.
- Convert to plain `#[test]` on `Scenario` (about 49 tests): `execute/tests.rs`, `exec_flow.rs` (its `flow_prompt!` macro becomes the builder, and its `ARM_FANOUT_PARENT` merges with the one in `suite/exec_flow/fanout_arms.rs`), `model_and_reply.rs`, `model_and_reply-globals.rs`, `model_and_reply-handles.rs` (`run_with_bindings`), `live_infer.rs`, `unified_pipeline.rs`, and `debug_and_counts-infer-rounds.rs`. In `live_infer.rs`, `cancelled_nested_infer_does_not_report_model_turn_failed` holds the 30-second reply's chat effect and cancels between steps, with no spawned task and no timeout.
- `execute/tests.rs`'s `untrusted_nonce_differs_across_runs_under_different_seeds` keeps its seeds 1 and 2 until Step 16.
- `execute/tests/context.rs`'s `run`, `run_offline`, `run_assembled`, `run_with_context`, `run_recorded`, `to_context`, `silent`, `gatewayed`, `gatewayed_with_debug`, `gateway_client`, `RunOptions`, and `Recorder` each leave in the step that converts their last user (this step or Steps 10-12).
- Acceptance: the `rg` prints nothing; the engine passes with the same names. The leaf comparison prints nothing marked `<=`, and `=>` only `temp_roots_with_one_label_get_distinct_paths_and_drop_removes_the_folder` and `seeded_failure_messages_name_the_seed` under `runvil-engine`. Full-line differences that are not leaf differences are moves, each listed in the commit message.

</step-9>

<step-10>

### Step 10: Engine tool loops on FnTool [completed]

- Component: engine Run level
- Tests:
  - `cargo nextest run --locked -p runvil-engine --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p runvil-engine --all-targets --all-features`
  - `cargo fmt --all --check`
  - `$t='crates/runvil-internal/engine/src/execute/tests'; rg -n "tokio::|TokioDriver|Instant::now" "$t/models_loop.rs" "$t/models_loop-arguments.rs" "$t/models_loop-author-shapes.rs" "$t/models_loop_compactors.rs" "$t/models_loop_contract.rs" "$t/models_loop_contract-rounds.rs" "$t/tool_loop.rs" "$t/tool_loop-scope-gate.rs" "$t/tool_scoping.rs" "$t/local_tools.rs" "$t/tool_call_arm.rs"`
  - `rg -n "impl TestTool for|impl crate::test_support::TestTool for" crates/runvil-internal/engine/src`
  - `$s='../cabinet/_scratch/vibe-coder-internal-test-harnesses'; foreach ($p in 'runvil-engine') { cargo nextest list --locked -p $p --all-features | Out-File -Encoding utf8 "$s/$p-after.txt"; Compare-Object (Get-Content "$s/$p-before.txt") (Get-Content "$s/$p-after.txt") | ForEach-Object { "$p full $($_.SideIndicator) $($_.InputObject)" }; Compare-Object (Get-Content "$s/$p-before.txt" | ForEach-Object { ($_ -split '::')[-1] }) (Get-Content "$s/$p-after.txt" | ForEach-Object { ($_ -split '::')[-1] }) | ForEach-Object { "$p leaf $($_.SideIndicator) $($_.InputObject)" } }`
- Rules: PowerShell, run from the repository root, which every relative path here (`crates/...`, `../cabinet/...`) resolves against. Run cargo commands one at a time and never append `2>&1`; judge a cargo step by its result lines and `$LASTEXITCODE`. Read the root `AGENTS.md` and `crates/runvil-internal/engine/AGENTS.md`. A file leaves by `Move-Item` to `../cabinet/_trash/`. No test is removed or weakened, no dependency is added, and test helpers are `#[track_caller]`. Add only the test-support surface this step's tests use. Every source file stays under 500 lines.
- `FnTool` one-liners replace the hand-written fixture tools crate-wide, with their `#[expect(clippy::unnecessary_literal_bound)]` blocks: `EchoTool`, `UntrustedEchoTool`, `StructuredFixtureTool`, `FailingTool`, and `ScopedFixtureTool` in `execute/tests/fixtures.rs` (`require_string_arg` stays as the handlers' argument check, and `ScopedFixtureTool`'s call counter becomes a counter the closure captures), `Unofferable` (`full_id_calls.rs`), `Offered` (`offering.rs`), and `FilesTool` (`tool_call_access.rs`). Because `FnTool` implements `TestTool`, the tokio-driven files that use these keep working. `SlowTool` (`execute/tests.rs`), `SignallingSlowTool` (`scheduler/failures-script-tools.rs`), and `DelayedTool` (`model_task_notices.rs`) never answer; they stay until their users convert in Steps 12-14.
- One test-model binding constructor in the engine test support (a `ModelBinding` from alias, description, and model id, with the 4096-token window) replaces the seven hand-built 4096-window bindings: `lua/tests.rs`, `lua/tests/globals.rs`, `execute/run/effect-tests.rs`, `execute/run/tests-drops.rs`, `execute/run/tests.rs`, `execute/tests/models_loop.rs` (`loop_models`), and `execute/tests/scheduler.rs` (`writer_models`).
- One `only_effect`, replacing the copies in `execute/run/tests.rs`, `models_loop_contract-rounds.rs`, and `parallel_tool_calls.rs`, and one `LOOP_TO_TEXT`, replacing the copies in `tool_loop.rs` and `tool_scoping.rs` (Step 11 points `exit_rules.rs` at it).
- `Scenario` gains the pre-filled model and tool sets that `loop_context` installs on a `RunState` (bypassing the live H1 pass), a chat closure for clients that are not scripts (`gateway.rs`'s `OverflowClient` becomes a counting closure, and `assert_model_failure` moves to the test support), and `cancel_when(signal)`: a canceller thread that waits untimed on a signal raised inside the run (the performer answering a given effect, or a counter it bumps) and then cancels; the scenario joins it before returning the trace.
- Convert to plain `#[test]` on `Scenario` (about 76 tests): `models_loop.rs`, `models_loop-arguments.rs`, `models_loop-author-shapes.rs`, `models_loop_compactors.rs`, `models_loop_contract.rs`, `models_loop_contract-rounds.rs`, `tool_loop.rs`, `tool_loop-scope-gate.rs`, `tool_scoping.rs`, `local_tools.rs`, and `tool_call_arm.rs`. With them go `loop_context_observed`, `loop_prompt`, `capped_loop_prompt` (`tool_loop.rs`), `jump_prompt` (`local_tools.rs`), `arm_prompt` and `tool_context` (`tool_call_arm.rs`), and `drive`, `drive_observed`, and `drive_overflowing` (`models_loop_contract.rs`), replaced by the parser's builder and `Scenario`. `loop_context`, `bound_tool`, `echo_tools`, and `loop_events` stay while unconverted files use them.
- Cancels that land mid-run:
  - `local_tools.rs`'s `cancel_during_a_looping_local_tool_handler_returns_promptly` and the looping-compactor cancel in `models_loop_compactors.rs`: Lua runs `while true do end` inside one step. `cancel_when` fires on the answer that hands control to the looping Lua; if the cancel lands before that step starts, the run still ends `Interrupted`, so the outcome does not depend on the race. The test joins the canceller, asserts `Interrupted`, and drops `Instant::now` and its elapsed bound; a hang fails through nextest's timeout.
  - `tool_loop.rs`'s in-flight tool cancel: the slow tool's call is held and the cancel lands between steps; assert `Interrupted`; drop `Instant::now` and the bound.
  - `models_loop_contract.rs`: the one-second canceller only guarded against a hang. Hold the slow tool, and cancel if the run ever has nothing but the held call outstanding, so a regression ends `Interrupted` and fails the existing assertion that the raise ended the call first.
  - `models_loop_contract-rounds.rs`: the cancel lands between steps once the first round is answered, where today's 100 ms timer aimed; assert `Interrupted` and the same loop events.
- Acceptance: the first `rg` prints nothing; the second prints only `FnTool`'s implementation and `SlowTool`, `SignallingSlowTool`, and `DelayedTool`; the engine passes with the same names. The leaf comparison prints nothing marked `<=`, and `=>` only `temp_roots_with_one_label_get_distinct_paths_and_drop_removes_the_folder` and `seeded_failure_messages_name_the_seed` under `runvil-engine`. Full-line differences that are not leaf differences are moves, each listed in the commit message.

</step-10>

<step-11>

### Step 11: Engine chat and section-loop runs [completed]

- Component: engine Run level
- Tests:
  - `cargo nextest run --locked -p runvil-engine --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p runvil-engine --all-targets --all-features`
  - `cargo fmt --all --check`
  - `$t='crates/runvil-internal/engine/src/execute/tests'; rg -n "tokio::|TokioDriver" "$t/batch_turn.rs" "$t/chat_arm.rs" "$t/chat_scope.rs" "$t/debug_and_counts.rs" "$t/exit_rules.rs" "$t/precheck_anchor.rs" "$t/observations.rs"`
  - `$s='../cabinet/_scratch/vibe-coder-internal-test-harnesses'; foreach ($p in 'runvil-engine') { cargo nextest list --locked -p $p --all-features | Out-File -Encoding utf8 "$s/$p-after.txt"; Compare-Object (Get-Content "$s/$p-before.txt") (Get-Content "$s/$p-after.txt") | ForEach-Object { "$p full $($_.SideIndicator) $($_.InputObject)" }; Compare-Object (Get-Content "$s/$p-before.txt" | ForEach-Object { ($_ -split '::')[-1] }) (Get-Content "$s/$p-after.txt" | ForEach-Object { ($_ -split '::')[-1] }) | ForEach-Object { "$p leaf $($_.SideIndicator) $($_.InputObject)" } }`
- Rules: PowerShell, run from the repository root, which every relative path here (`crates/...`, `../cabinet/...`) resolves against. Run cargo commands one at a time and never append `2>&1`; judge a cargo step by its result lines and `$LASTEXITCODE`. Read the root `AGENTS.md` and `crates/runvil-internal/engine/AGENTS.md`. A file leaves by `Move-Item` to `../cabinet/_trash/`. No test is removed or weakened, no dependency is added, and test helpers are `#[track_caller]`. Add only the test-support surface this step's tests use. Every source file stays under 500 lines.
- Convert to plain `#[test]` on `Scenario` (about 63 tests): `batch_turn.rs` (its local `drive`), `chat_arm.rs`, `chat_scope.rs`, `debug_and_counts.rs`, `exit_rules.rs` (`drive_loop`, its `run_offline` uses, and its copy of `LOOP_TO_TEXT`, which becomes Step 10's shared one), `precheck_anchor.rs`, and `observations.rs` (`drive_serially`, `run_recorded`, `run`, `run_with_context`).
- `Trace` gains the observation and debug-capture reads these files assert. The per-file `Observer` recorders (`lines`, `push`, and `on_*` in `batch_turn.rs` and `chat_arm.rs`) and `execute/tests.rs`'s `RecordingCapture` stay only where their record shape is the test's subject; otherwise the test reads observations from the trace.
- Delete each `execute/tests/context.rs` helper whose last user converts here.
- Acceptance: the `rg` prints nothing; the engine passes with the same names. The leaf comparison prints nothing marked `<=`, and `=>` only `temp_roots_with_one_label_get_distinct_paths_and_drop_removes_the_folder` and `seeded_failure_messages_name_the_seed` under `runvil-engine`. Full-line differences that are not leaf differences are moves, each listed in the commit message.

</step-11>

<step-12>

### Step 12: Engine Run-level model tasks and scheduler-directory walks [completed]

- Component: engine Run level
- Tests:
  - `cargo nextest run --locked -p runvil-engine --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p runvil-engine --all-targets --all-features`
  - `cargo fmt --all --check`
  - `$t='crates/runvil-internal/engine/src/execute/tests'; rg -n "tokio::|TokioDriver|Instant::now" "$t/model_task_answers.rs" "$t/model_task_notices-endings.rs" "$t/provenance.rs" "$t/scheduler/walk/core_rules.rs" "$t/scheduler/walk/child_levels.rs" "$t/scheduler/walk/jumps.rs" "$t/scheduler/walk/driving.rs" "$t/scheduler/live_h1.rs" "$t/scheduler/live_h1" "$t/scheduler/fanout.rs" "$t/scheduler/fanout/arms.rs" "$t/scheduler/fanout/failure_semantics.rs" "$t/scheduler/fanout/scheduling.rs" "$t/scheduler/failures-script-tools.rs"`
  - `$s='../cabinet/_scratch/vibe-coder-internal-test-harnesses'; foreach ($p in 'runvil-engine') { cargo nextest list --locked -p $p --all-features | Out-File -Encoding utf8 "$s/$p-after.txt"; Compare-Object (Get-Content "$s/$p-before.txt") (Get-Content "$s/$p-after.txt") | ForEach-Object { "$p full $($_.SideIndicator) $($_.InputObject)" }; Compare-Object (Get-Content "$s/$p-before.txt" | ForEach-Object { ($_ -split '::')[-1] }) (Get-Content "$s/$p-after.txt" | ForEach-Object { ($_ -split '::')[-1] }) | ForEach-Object { "$p leaf $($_.SideIndicator) $($_.InputObject)" } }`
- Rules: PowerShell, run from the repository root, which every relative path here (`crates/...`, `../cabinet/...`) resolves against. Run cargo commands one at a time and never append `2>&1`; judge a cargo step by its result lines and `$LASTEXITCODE`. Read the root `AGENTS.md` and `crates/runvil-internal/engine/AGENTS.md`. A file leaves by `Move-Item` to `../cabinet/_trash/`. No test is removed or weakened, no dependency is added, and test helpers are `#[track_caller]`. Add only the test-support surface this step's tests use. Every source file stays under 500 lines.
- These files inspect nothing below `Run`, so they move to `Scenario` as plain `#[test]` (about 92 tests):
  - `model_task_answers.rs`; `model_task_notices-endings.rs`, whose `SlowTool` children are held; `provenance.rs`.
  - `scheduler/walk/core_rules.rs`, `child_levels.rs`, `jumps.rs`, and `driving.rs`. In `driving.rs`, `cancellation_while_suspended_on_infer_interrupts_the_run` holds the 30-second reply and `cancel_when` fires once that round is issued, with no spawned task, poll loop, or timeout.
  - `scheduler/live_h1/control_flow.rs`, `pass.rs`, and `returns_and_prose.rs`, with `h1_context` and `h1_context_on` in `scheduler/live_h1.rs` replaced by a `Scenario` option that binds every declared role to the test model through `ctx.model_bindings`, leaving the shared model set empty for the live H1 pass.
  - `scheduler/fanout/arms.rs` (the pre-cancelled fanout cancels before the first step), `failure_semantics.rs`, and `scheduling.rs`, with `scheduler_context_with_limits` (`scheduler/fanout.rs`) replaced by `Scenario`'s limits.
  - `scheduler/failures-script-tools.rs`. In `cancellation_interrupts_a_slow_script_tools_call`, the `SignallingSlowTool` becomes a held tool whose dispatch raises the `started` signal; `cancel_when` waits on it untimed; the test asserts `Interrupted` and `started == 1` and drops `Instant::now`, the elapsed bound, and the poll loop. `SignallingSlowTool` leaves.
- The serial files that build runs through the old builders and inspect nothing below `Run` move to `Scenario` too: `preludes.rs` (`drive_recorded`), `chat_record_rebuild.rs` (`drive_rounds`), `parallel_tool_calls-withdrawn.rs`, `tool_call_access.rs` (`drive_files`), `model_task_trust.rs`, and `run_termination.rs`.
- Deduplicate in the same commit: one `task(id)` (`fanout_acceptance.rs`, `model_tasks.rs`, `provenance.rs`, `tasks.rs`, `waits.rs`); one `request_prompts` (`scheduler.rs`, `scheduler/fanout.rs`); one `chunk` (`model/tests-always.rs`, `model/tests-integration.rs`); one `fresh_access` (`model/tests.rs`, `execute/tests/context.rs`); one `SectionVmSetup` assembly (`lua/tests.rs`, `lua/tests/globals.rs`); one `EffectRecord` `round_trip` (`execute/run/tests.rs`, `execute/tests/effects.rs`), with `execute/run/effect-tests.rs`'s `AnswerRecord` version renamed. The genuinely different `completion` (`execute/run/effect-tests.rs`, `execute/support-tests.rs`), `request` (`execute/run/effect-tests.rs`, `execute/scheduler/chain_record-tests.rs`), `succeeded` (`preludes.rs`, `serial_driver.rs`), `test_context` (`execute/context-tests.rs`, `execute/tests/context.rs`), and `TREE` (`heading_address-tests.rs`, `execute/walk_target-tests.rs`) are merged where the bodies match and otherwise get distinct names.
- Acceptance: the `rg` prints nothing; the engine passes with the same names. The leaf comparison prints nothing marked `<=`, and `=>` only `temp_roots_with_one_label_get_distinct_paths_and_drop_removes_the_folder` and `seeded_failure_messages_name_the_seed` under `runvil-engine`. Full-line differences that are not leaf differences are moves, each listed in the commit message.

</step-12>

<step-13>

### Step 13: Engine scheduler rig with the effect tap, round capture, and RunState internals [completed]

- Component: engine scheduler level
- Tests:
  - `cargo nextest run --locked -p runvil-engine --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p runvil-engine --all-targets --all-features`
  - `cargo fmt --all --check`
  - `$t='crates/runvil-internal/engine/src/execute/tests'; rg -n "tokio::|TokioDriver" "$t/effects.rs" "$t/full_id_calls.rs" "$t/offering.rs" "$t/offering-plugins.rs" "$t/precheck_anchor-resend.rs" "$t/rounds.rs" "$t/tool_call_arm-local-handlers.rs" "$t/tasks.rs" "$t/tasks-chain-end.rs" "$t/scheduler/walk/chains.rs" "$t/model_tasks.rs"`
  - `$s='../cabinet/_scratch/vibe-coder-internal-test-harnesses'; foreach ($p in 'runvil-engine') { cargo nextest list --locked -p $p --all-features | Out-File -Encoding utf8 "$s/$p-after.txt"; Compare-Object (Get-Content "$s/$p-before.txt") (Get-Content "$s/$p-after.txt") | ForEach-Object { "$p full $($_.SideIndicator) $($_.InputObject)" }; Compare-Object (Get-Content "$s/$p-before.txt" | ForEach-Object { ($_ -split '::')[-1] }) (Get-Content "$s/$p-after.txt" | ForEach-Object { ($_ -split '::')[-1] }) | ForEach-Object { "$p leaf $($_.SideIndicator) $($_.InputObject)" } }`
- Rules: PowerShell, run from the repository root, which every relative path here (`crates/...`, `../cabinet/...`) resolves against. Run cargo commands one at a time and never append `2>&1`; judge a cargo step by its result lines and `$LASTEXITCODE`. Read the root `AGENTS.md` and `crates/runvil-internal/engine/AGENTS.md`. A file leaves by `Move-Item` to `../cabinet/_trash/`. No test is removed or weakened, no dependency is added, and test helpers are `#[track_caller]`. Add only the test-support surface this step's tests use. Every source file stays under 500 lines.
- New `#[cfg(test)]` `crates/runvil-internal/engine/src/test_support/rig.rs`, the scheduler rig, built on `Scenario`'s stepper:
  - It builds the `RunState` with the model set (the binding constructor's `writer` set, or the loop set) and the tool set already filled in, on a given `TestStore`, observer, and limits.
  - It exposes a serial step-and-answer loop, the effect tap (every issued `EffectRecord`, in issue order, performed or dropped at issue), round capture (each `Chat` effect's `Round`), and the `RunState` internals the suites reach today through `tokio_driver-test-hooks.rs`: `task_state_for_test`, `set_max_chains_for_test`, `leaf_requests_issued`, and the scheduler itself (`scheduler_for_test`, as `wedge_admission_for_test` needs).
- Convert to plain `#[test]` on the rig (about 62 tests, plus the serial files below): `effects.rs` (`effect_context`), `full_id_calls.rs` (`full_id_prompt`, `catalog_context`), `offering.rs` (`offering_context`, `offering_prompt`), `offering-plugins.rs`, `precheck_anchor-resend.rs` (its local `run_recorded`), `rounds.rs`, `tool_call_arm-local-handlers.rs`, `tasks.rs` (`TaskRecorder`), `tasks-chain-end.rs`, `scheduler/walk/chains.rs`, and `model_tasks.rs` (`model_task_context`, `model_task_context_with`, `owner_prompt`; its `SlowTool` child is held). The serial `parallel_tool_calls.rs` and `scheduler/concurrency-stepped.rs`, which reach the scheduler, build their runs through the rig too.
- `PARKED_CHILD` in `model_tasks.rs`, `tasks-chain-end.rs`, and `run_termination.rs` genuinely differ (a held tool call, a store write, a whole prompt); give each a name that says what it parks on.
- Acceptance: the `rg` prints nothing; the engine passes with the same names. The leaf comparison prints nothing marked `<=`, and `=>` only `temp_roots_with_one_label_get_distinct_paths_and_drop_removes_the_folder` and `seeded_failure_messages_name_the_seed` under `runvil-engine`. Full-line differences that are not leaf differences are moves, each listed in the commit message.

</step-13>

<step-14>

### Step 14: Engine held-and-released answers and seeded shuffles [completed]

- Component: engine scheduler level
- Tests:
  - `cargo nextest run --locked -p runvil-engine --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p runvil-engine --all-targets --all-features`
  - `cargo fmt --all --check`
  - `rg -n "tokio::|TokioDriver|drive_tokio|run_with_fixture\b|run_prepared|RunFixture|StoreGate|wait_timeout" crates/runvil-internal/engine/src/execute`
  - `rg -n "fn (effect_context|offering_context|catalog_context|tool_context|loop_context\w*|model_task_context\w*|ceiling_context|limited_context|scheduler_context\w*|h1_context\w*|to_context|run_fixture|run_offline|run_assembled|run_with_context|run_recorded|run_with_bindings|drive_serially|drive_recorded|drive_observed|drive_overflowing|drive_loop|drive_rounds|drive_files|drive_scripted|identity_run|loop_prompt|owner_prompt|arm_prompt|full_id_prompt|capped_loop_prompt|jump_prompt|offering_prompt|with_test_title)\b" crates/runvil-internal/engine/src`
  - `$s='../cabinet/_scratch/vibe-coder-internal-test-harnesses'; foreach ($p in 'runvil-engine') { cargo nextest list --locked -p $p --all-features | Out-File -Encoding utf8 "$s/$p-after.txt"; Compare-Object (Get-Content "$s/$p-before.txt") (Get-Content "$s/$p-after.txt") | ForEach-Object { "$p full $($_.SideIndicator) $($_.InputObject)" }; Compare-Object (Get-Content "$s/$p-before.txt" | ForEach-Object { ($_ -split '::')[-1] }) (Get-Content "$s/$p-after.txt" | ForEach-Object { ($_ -split '::')[-1] }) | ForEach-Object { "$p leaf $($_.SideIndicator) $($_.InputObject)" } }`
- Rules: PowerShell, run from the repository root, which every relative path here (`crates/...`, `../cabinet/...`) resolves against. Run cargo commands one at a time and never append `2>&1`; judge a cargo step by its result lines and `$LASTEXITCODE`. Read the root `AGENTS.md` and `crates/runvil-internal/engine/AGENTS.md`. A file leaves by `Move-Item` to `../cabinet/_trash/`. No test is removed or weakened, no dependency is added, and test helpers are `#[track_caller]`. Add only the test-support surface this step's tests use. Every source file stays under 500 lines.
- The rig gains held-and-released answers (hold matching effects, release them later in a chosen order, answer a timer while a reply is held) and seeded shuffles: each wave of answers delivered in `Seeded`'s permutation, replacing the tokio driver's xorshift `shuffle_batch` and `happens_before.rs`'s `TOKIO_SEEDS`. Every assertion inside a seeded loop names its seed through `Seeded`, and the serial batching loops print their batching, as `scheduler/concurrency-stepped.rs` does.
- Convert to plain `#[test]` on the rig (about 61 tests): `happens_before.rs` and `happens_before-concurrency.rs` (the seeded interleaving loops); `model_task_acceptance.rs` (`task_lifecycle`, `terminals_per_started_task`, `two_child_prompt`); `model_task_awaits.rs`; `model_task_ids_and_scope.rs`; `model_task_notices.rs` (`DelayedTool` leaves); `timeouts.rs`, where each `SLOW` reply that races a `tasks.join_any` or `tasks.join` timeout is held while the rig answers the timer, then released; `waits.rs` (`WaitRecorder`, its local `drive`); `fanout_acceptance.rs` (`PARKED`, `ceiling_context`, `identity_run`, `task_lifecycle`, `terminals_per_started_task`); `scheduler/failures.rs`; and `scheduler/concurrency.rs` (`limited_context`, `ceiling`). The serial `serial_driver.rs` builds its runs through the rig and keeps testing the driver and its batchings.
- `StoreGate`, `GateObserver`, and `gated_store` go: the rig holds the child's store-write effect until the `TaskCancelled` observation appears, then releases it. `scheduler/store_gate.rs`, with its 10-second `wait_timeout` backstop, leaves by `Move-Item` to `../cabinet/_trash/`, and `scheduler.rs` drops its re-export.
- One `task_lifecycle` and one `terminals_per_started_task`, from `fanout_acceptance.rs` and `model_task_acceptance.rs`.
- `SlowTool` and the remaining per-file builders leave with their last users: `scheduler_context`, `scheduler_context_on`, `scheduler_context_from`, `loop_context`, `arm_tools`, `fixture_tools`, `bound_tool`, `echo_tools`, and `drive_scripted`. `writer_models`, `loop_models`, `fixture_binding`, and `FixtureTools` survive only as internals of the rig or `Scenario`.
- Acceptance: both `rg` commands print nothing; no engine test names the tokio driver, and every engine test drives runs with the serial driver; the engine passes with the same names. The leaf comparison prints nothing marked `<=`, and `=>` only `temp_roots_with_one_label_get_distinct_paths_and_drop_removes_the_folder` and `seeded_failure_messages_name_the_seed` under `runvil-engine`. Full-line differences that are not leaf differences are moves, each listed in the commit message.

</step-14>

<step-15>

### Step 15: Remove tokio and async-trait from the engine [completed]

- Component: tokio removal
- Tests:
  - `cargo nextest run --locked -p runvil-engine --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p runvil-engine --all-targets --all-features`
  - `cargo bench --locked -p runvil-engine --features test-support --bench models_loop -- --test`
  - `cargo fmt --all --check`
  - `rg -n "tokio|async_trait|async-trait" crates/runvil-internal/engine`
  - `rg -n -g '*.rs' "thread::sleep|time::sleep|SystemTime::now|Instant::now|RandomState|wait_timeout|recv_timeout|park_timeout" crates/runvil crates/runvil-plugin crates/runvil-internal`
  - `foreach ($c in 'runvil-engine') { $hits = cargo tree --locked -q -p $c --all-features -e normal,build,dev --prefix none | ForEach-Object { ($_ -split ' ')[0] } | Where-Object { $_ -in 'tokio','tokio-util','async-trait','reqwest','getrandom','fastrand' } | Sort-Object -Unique; "{0}: {1}" -f $c, ($hits -join ', ') }`
  - `$s='../cabinet/_scratch/vibe-coder-internal-test-harnesses'; foreach ($p in 'runvil-engine') { cargo nextest list --locked -p $p --all-features | Out-File -Encoding utf8 "$s/$p-after.txt"; Compare-Object (Get-Content "$s/$p-before.txt") (Get-Content "$s/$p-after.txt") | ForEach-Object { "$p full $($_.SideIndicator) $($_.InputObject)" }; Compare-Object (Get-Content "$s/$p-before.txt" | ForEach-Object { ($_ -split '::')[-1] }) (Get-Content "$s/$p-after.txt" | ForEach-Object { ($_ -split '::')[-1] }) | ForEach-Object { "$p leaf $($_.SideIndicator) $($_.InputObject)" } }`
- Rules: PowerShell, run from the repository root, which every relative path here (`crates/...`, `../cabinet/...`) resolves against. Run cargo commands one at a time and never append `2>&1`; judge a cargo step by its result lines and `$LASTEXITCODE`. Read the root `AGENTS.md` and `crates/runvil-internal/engine/AGENTS.md`. A file leaves by `Move-Item` to `../cabinet/_trash/`. No test is removed or weakened, and no dependency is added.
- `test_support/tokio_driver.rs`, `tokio_driver-performers.rs`, and `tokio_driver-test-hooks.rs` leave by `Move-Item` to `../cabinet/_trash/`, and so does `test_support/fixture.rs` (`RunFixture`, `ChatClient`). From `test_support.rs` go `drive_tokio`, `run_with_fixture`, `run_prepared`, the `ChatClient for ScriptedChat` implementation, and the re-exports of `BoxFuture`, `Performer`, `Performers`, `RunFixture`, and `ChatClient`. The `TestTool` trait and its `FixtureFuture` go: the tool table holds `FnTool` directly. `ScriptedChat`'s async `complete` wrapper goes, leaving the synchronous core.
- The `test-support` feature becomes `test-support = []` and gates what the bench needs: the serial driver `drive`, `ScriptedChat` with the reply script, and the local effect performer. `scripted-chat.rs` moves under `#[cfg(any(test, feature = "test-support"))]`, and the module doc of `test_support.rs` names the bench as the feature's only consumer.
- `benches/models_loop.rs` prepares each run itself and drives it with the serial driver, answering rounds through the shared `ScriptedChat` from `runvil_engine::test_support`. It drops its `#[path]` include of `scripted-chat.rs`, that include's `#[expect(dead_code)]`, `BenchClient`, and both tokio runtimes.
- `crates/runvil-internal/engine/Cargo.toml`: drop the optional `tokio` and its comment, and `async-trait` and `tokio` from `[dev-dependencies]` with their comments; rewrite the feature and bench comments to describe the serial driver. Refresh `Cargo.lock` once with `cargo metadata --format-version 1 | Out-Null`, without `--locked`; its diff only drops the engine's `tokio` and `async-trait` edges. The workspace entries stay, because other crates use them.
- `execute/run/tests.rs`'s `a_child_cancel_handles_cancel_is_observed_by_the_instruction_hook`: the canceller thread cancels the parent with no sleep, and the test joins it after the step. The loop cannot end without the cancel, so `Cancelled` holds whichever lands first.
- Acceptance: the first two `rg` commands print nothing; the graph line for `runvil-engine` is empty; the bench runs once per benchmark; the engine passes with the same names. The leaf comparison prints nothing marked `<=`, and `=>` only `temp_roots_with_one_label_get_distinct_paths_and_drop_removes_the_folder` and `seeded_failure_messages_name_the_seed` under `runvil-engine`.

</step-15>

<step-16>

### Step 16: 256-bit run seed and run id [completed]

- Component: run seed
- Tests:
  - `foreach ($p in 'runvil-types','runvil-engine','runvil-lua','runvil','harness-runner','harness','harness-gateway-client','workshop-run-log','workshop-agents') { cargo nextest run --locked -p $p --all-features; "nextest $p exit $LASTEXITCODE" }`
  - `$env:CARGO_BUILD_WARNINGS='deny'; foreach ($p in 'runvil-types','runvil-engine','runvil-lua','runvil','harness-runner','harness','harness-gateway-client','workshop-run-log','workshop-agents') { cargo clippy -p $p --all-targets --all-features; "clippy $p exit $LASTEXITCODE" }`
  - `cargo bench --locked -p runvil-engine --features test-support --bench models_loop -- --test`
  - `cargo fmt --all --check`
  - `cargo +nightly-2026-09-05 xtask api --check`
  - `$s='../cabinet/_scratch/vibe-coder-internal-test-harnesses'; foreach ($p in 'runvil-types','runvil-engine','runvil-lua','runvil','harness-runner','harness','harness-gateway-client','workshop-run-log','workshop-agents') { cargo nextest list --locked -p $p --all-features | Out-File -Encoding utf8 "$s/$p-after.txt"; Compare-Object (Get-Content "$s/$p-before.txt") (Get-Content "$s/$p-after.txt") | ForEach-Object { "$p full $($_.SideIndicator) $($_.InputObject)" }; Compare-Object (Get-Content "$s/$p-before.txt" | ForEach-Object { ($_ -split '::')[-1] }) (Get-Content "$s/$p-after.txt" | ForEach-Object { ($_ -split '::')[-1] }) | ForEach-Object { "$p leaf $($_.SideIndicator) $($_.InputObject)" } }`
- Rules: PowerShell, run from the repository root, which every relative path here (`crates/...`, `../cabinet/...`) resolves against. Run cargo commands one at a time and never append `2>&1`; judge a cargo step by its result lines and `$LASTEXITCODE`. Read the root `AGENTS.md` and the nearest `AGENTS.md` above each crate you edit (types, lua, engine, `crates/harness-gateway-client/AGENTS.md`, and `crates/workshop/AGENTS.md` for the run log and `workshop-agents`). A file leaves by `Move-Item` to `../cabinet/_trash/`. No test is removed or weakened, no dependency is added, and there are no compatibility shims: signatures change outright and every caller changes in this commit.
- `crates/runvil-internal/types/src/seed.rs`, a new `pub mod seed`:
  - `RunSeed([u8; 32])` with `from_bytes([u8; 32])` and `as_bytes()`, `Clone`, `Copy`, `PartialEq`, and `Eq`. `Display` and serde use 64 lowercase hex characters, and deserializing anything else fails naming the required form and the actual length or character. `Debug` prints `RunSeed(..)` with no bytes.
  - `derive(domain: &[u8], seed: &RunSeed) -> [u8; 32]`: SHA-256 over the domain tag, then the seed's bytes. Every value derived from the seed goes through it with its own `runvil.<purpose>.v1` tag; `sha2` is already a normal dependency.
  - `RunUid([u8; 32])`, `derive(b"runvil.run-uid.v1", seed)`, with a hex `Display`.
- `GuardNonce::from_seed(&RunSeed)` in `types/src/untrusted.rs` keeps the `runvil.guard-nonce.v1` tag and its 32-hex-character form, taken from the first 16 bytes of `derive`; its doc drops the 64-bit wording, since the nonce's unpredictability is now its full 128 bits.
- Engine, `execute/config.rs`: `RunContext::new(name, seed: RunSeed, started_at)`, `seed() -> RunSeed`, and `uid() -> RunUid`; the hand-written `Debug` shows `uid` in place of `seed`; the docs on `RunContext` and `new` keep the rule that a live run's seed comes from a CSPRNG. `execute/context.rs` derives the nonce from the wide seed. The facade re-exports `RunSeed` and `RunUid` at its root beside `RunContext`; re-bless `crates/runvil/public-api.txt` with `cargo +nightly-2026-09-05 xtask api --bless`, then confirm with `--check`.
- Harness: `crates/harness-internal/runner/src/prepare.rs` draws `RunSeed::from_bytes(rand::random())` (the Harness keeps `rand`), and `RunMeta.seed` in `src/recorder.rs` becomes a `RunSeed`.
- Workshop run log: in `crates/workshop/run-log/src/schema.rs` the `CREATE TABLE` declares `seed TEXT NOT NULL`, holding the 64 hex characters, and `LAYOUT_VERSION` goes from 1 to 2, so the layout check sets aside a file written before this change. `append.rs` writes the seed's `Display`; `read.rs` decodes the 64 hex characters back to 32 bytes with a private helper in the run log and builds the seed with `RunSeed::from_bytes`, refusing a malformed value as a read error that names the column. The docs in `lib.rs` and `schema.rs` drop `seed` from the list of `u64` values stored as `i64`.
- Callers, re-found with `rg -n "RunContext::new\(|GuardNonce::from_seed\(|RunMeta \{|\.seed\b" crates`: the facade's `examples/greeter.rs` and `tests/suite/support/scenario.rs`; lua's `src/vm/run.rs` (its production direct-chunk path takes an all-zero `RunSeed`), `benches/surface.rs`, and `src/test_vm.rs`; the engine's test support, `execute/context-tests.rs`, `execute/run/tests.rs`, `execute/tests.rs`, `execute/tests/run_inputs.rs`, `execute/tests/model_task_trust.rs`, `execute/tests/tool_scoping.rs`, `lua/tests.rs`, `lua/tests/globals.rs`, `model/tests.rs`, and `benches/models_loop.rs`; types' `src/untrusted/tests.rs`; the Harness's `tests/it/{support,effect_loop-cap,effect_loop-broker,prepare,recorder,performers,effect_loop}.rs`; `crates/harness-gateway-client/src/wire/request-tests.rs`; the run log's `tests/it/{recorder,read,layout,fidelity,append}.rs`; and `crates/workshop/agents/src/tee-tests.rs`. `guide/` and the Workshop UI had no text calling the seed a number at `594eb22a7`; check again.
- Fixed seeds only: one `TEST_SEED: RunSeed` in each test support that builds runs or VMs (the facade `Scenario`, the engine test support behind `Scenario` and the rig, lua's `TestVm`, and the Harness's `tests/it/support.rs`). A test that needs several seeds draws them from `Seeded`: types' `untrusted/tests.rs` (today's seeds 0, 7, 8, and `u64::MAX`), and the engine's `untrusted_nonce_differs_across_runs_under_different_seeds` and `run_inputs.rs`.
- Changed expectations, and no others: the pinned nonce `"ab22feddfa5ea06b226ac2d588efa9eb"` in `types/src/untrusted/tests.rs` is re-pinned under the new derivation, and any run-log layout test that names 1 as the current layout follows `LAYOUT_VERSION` to 2.
- New tests:
  - types: `derive_matches_its_known_answer` (a known-answer vector for `derive`, computed outside the code under test); `the_nonce_and_run_id_differ_from_each_other_and_the_seed_and_repeat` (for one seed, the nonce and the run id differ from each other and from the seed's bytes, and repeat across calls); `a_run_seed_round_trips_through_serde_as_64_hex_characters`; `a_run_seeds_debug_shows_no_seed_bytes`.
  - engine: `a_run_contexts_debug_shows_the_run_id_and_not_the_seed`.
  - workshop-run-log: `a_32_byte_seed_survives_a_run_log_round_trip`.
- Acceptance: `xtask api --check` passes; every listed package passes, which also proves no caller still passes an integer seed. The leaf comparison prints nothing marked `<=`, and `=>` only: under `runvil-types`, `seeded_failure_messages_name_the_seed` and the four types tests above; under `runvil-engine`, `temp_roots_with_one_label_get_distinct_paths_and_drop_removes_the_folder`, `seeded_failure_messages_name_the_seed`, and `a_run_contexts_debug_shows_the_run_id_and_not_the_seed`; under `runvil`, `temp_roots_with_one_label_get_distinct_paths_and_drop_removes_the_folder`; under `workshop-run-log`, `a_32_byte_seed_survives_a_run_log_round_trip`. Full-line differences that are not leaf differences are moves, each listed in the commit message.

</step-16>

<step-17>

### Step 17: Total Engine dependency guard [completed]

- Component: determinism policing
- Tests:
  - `foreach ($c in 'runvil','runvil-plugin','runvil-engine','runvil-lua','runvil-parser','runvil-types','runvil-vfs','runvil-model-client') { $hits = cargo tree --locked -q -p $c --all-features -e normal,build,dev --prefix none | ForEach-Object { ($_ -split ' ')[0] } | Where-Object { $_ -in 'tokio','tokio-util','async-trait','reqwest','getrandom','fastrand' } | Sort-Object -Unique; "{0}: {1}" -f $c, ($hits -join ', ') }`
  - `cargo nextest run --locked -p build-xtask --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p build-xtask --all-targets --all-features`
  - `cargo fmt --all --check`
  - `rg -n "is_exempt|enabling_features|feature_lists|EXEMPTING_FEATURE" crates/build-xtask/src/engine_deps.rs`
  - `$s='../cabinet/_scratch/vibe-coder-internal-test-harnesses'; foreach ($p in 'build-xtask') { cargo nextest list --locked -p $p --all-features | Out-File -Encoding utf8 "$s/$p-after.txt"; Compare-Object (Get-Content "$s/$p-before.txt") (Get-Content "$s/$p-after.txt") | ForEach-Object { "$p full $($_.SideIndicator) $($_.InputObject)" }; Compare-Object (Get-Content "$s/$p-before.txt" | ForEach-Object { ($_ -split '::')[-1] }) (Get-Content "$s/$p-after.txt" | ForEach-Object { ($_ -split '::')[-1] }) | ForEach-Object { "$p leaf $($_.SideIndicator) $($_.InputObject)" } }`
- Rules: PowerShell, run from the repository root, which every relative path here (`crates/...`, `../cabinet/...`) resolves against. Run cargo commands one at a time and never append `2>&1`; judge a cargo step by its result lines and `$LASTEXITCODE`. Read the root `AGENTS.md`. A file leaves by `Move-Item` to `../cabinet/_trash/`. No dependency is added. Every source file stays under 500 lines.
- First run the graph loop above by hand in report-only mode. Every Engine crate must print an empty list; clear anything it finds before changing the guard. There is no exemption.
- `crates/build-xtask/src/engine_deps.rs` becomes total:
  - It scans every dependency table, direct and target-specific: `[dependencies]`, `[build-dependencies]`, and now `[dev-dependencies]`.
  - The exemption for an optional dependency that only `test-support` enables goes, with `is_optional`, `is_exempt`, `enabling_features`, and `feature_lists`. `EXEMPTING_FEATURE` moves to `test_support_leak.rs`, its one remaining user.
  - The forbidden list is `tokio`, `tokio-util`, `async-trait`, `reqwest`, `getrandom`, and `fastrand`.
  - A resolved-graph check: per Engine crate from `engine_guards::engine_crates`, run `cargo tree -p <crate> --all-features -e normal,build,dev --prefix none` with the cargo running the tests (the `CARGO` environment variable), and report every forbidden package reached, naming the Engine crate and the package. Per crate with all features, so optional dependencies are seen and workspace feature unification cannot flag a crate; `cargo tree`, because `cargo metadata` resolves features for the whole workspace at once. Wire it into `engine_guards::engine_guard_violations`, so `cargo test -p build-xtask` and CI run it.
  - Violation messages say what is required and what was found: the Engine crate or manifest, the table or the resolved graph, and the package, and that Engine crates never depend on these crates, directly or transitively.
  - Rewrite the module doc to match: every table, no exemption, the resolved graph, the full list.
- Fixture tests. Graph fixtures are small workspaces whose local path crates are named `tokio`, `getrandom`, `fastrand`, and `tempfile` (the `tempfile` fixture depending on the local `getrandom` and `fastrand`), resolved offline:
  - Renamed, per the Decision Record, each now asserting one violation: `a_forbidden_crate_in_dev_dependencies_only_passes` becomes `a_forbidden_crate_in_dev_dependencies_is_reported`; `an_optional_entry_enabled_only_by_test_support_passes` becomes `an_optional_entry_enabled_only_by_test_support_is_reported`; `a_dep_gated_entry_with_only_a_weak_feature_path_passes` becomes `a_dep_gated_entry_with_only_a_weak_feature_path_is_reported`.
  - `build_and_target_specific_tables_are_scanned_and_renames_resolved` now expects three violations, since its `cfg(unix)` dev `tokio` is reported.
  - The other existing tests keep their names and assertions.
  - New: `tokio_in_a_target_specific_dev_table_is_reported`; `a_forbidden_crate_reached_through_a_dev_dependency_is_reported` (an Engine crate dev-depends on a non-Engine crate that depends on `tokio`); `getrandom_reached_through_any_edge_is_reported` (through a normal, a build, and a dev edge); `a_tempfile_dev_dependency_is_reported_for_getrandom_and_fastrand`; `a_clean_engine_crate_graph_has_no_violations`.
- Acceptance: the graph loop prints an empty list for all eight Engine crates; the `rg` prints nothing; `build-xtask` passes, the real workspace included. The leaf comparison prints, marked `<=`, exactly the three old names above, and, marked `=>`, exactly the three new names and the five new tests. Full-line differences that are not leaf differences are moves, each listed in the commit message.

</step-17>

<step-18>

### Step 18: Clippy determinism bans, rule text, and exit criteria [completed]

- Component: determinism policing
- Tests:
  - `npm ci --prefix crates/workshop`
  - `npm ci --prefix crates/gateway/config-ui/ui`
  - `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`
  - `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo check -p gateway --no-default-features`
  - `cargo fmt --all --check`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc -p runvil --no-deps`
  - `cargo +nightly-2026-09-05 xtask api --check`
  - `node --test crates/workshop/ui/test/docs-claims.mjs`
  - `cargo deny --log-level error check`
  - `rg -n "tokio|async_trait|async-trait" crates/runvil-internal/engine`
  - `rg -n -g '*.rs' "thread::sleep|time::sleep|SystemTime::now|Instant::now|RandomState|wait_timeout|recv_timeout|park_timeout" crates/runvil crates/runvil-plugin crates/runvil-internal`
  - `foreach ($c in 'runvil','runvil-plugin','runvil-engine','runvil-lua','runvil-parser','runvil-types','runvil-vfs','runvil-model-client') { $hits = cargo tree --locked -q -p $c --all-features -e normal,build,dev --prefix none | ForEach-Object { ($_ -split ' ')[0] } | Where-Object { $_ -in 'tokio','tokio-util','async-trait','reqwest','getrandom','fastrand' } | Sort-Object -Unique; "{0}: {1}" -f $c, ($hits -join ', ') }`
  - `rg -n "tempfile" -g Cargo.toml crates/runvil crates/runvil-plugin crates/runvil-internal`
  - `$s='../cabinet/_scratch/vibe-coder-internal-test-harnesses'; foreach ($p in 'runvil-parser','runvil-types','runvil-model-client','runvil-vfs','runvil-lua','runvil-engine','runvil','build-xtask','harness-runner','harness','harness-gateway-client','workshop-run-log','workshop-agents') { cargo nextest list --locked -p $p --all-features | Out-File -Encoding utf8 "$s/$p-after.txt"; Compare-Object (Get-Content "$s/$p-before.txt") (Get-Content "$s/$p-after.txt") | ForEach-Object { "$p full $($_.SideIndicator) $($_.InputObject)" }; Compare-Object (Get-Content "$s/$p-before.txt" | ForEach-Object { ($_ -split '::')[-1] }) (Get-Content "$s/$p-after.txt" | ForEach-Object { ($_ -split '::')[-1] }) | ForEach-Object { "$p leaf $($_.SideIndicator) $($_.InputObject)" } }`
  - `$s='../cabinet/_scratch/vibe-coder-internal-test-harnesses'; foreach ($pair in ('engine','runvil-engine'),('lua','runvil-lua'),('vfs','runvil-vfs'),('parser','runvil-parser'),('types','runvil-types'),('model-client','runvil-model-client')) { $c=$pair[0]; $tests = Get-Content "$s/$($pair[1])-after.txt" | ForEach-Object { ($_ -split '::')[-1] }; Get-ChildItem "crates/runvil-internal/$c" -Recurse -Filter *.rs | Where-Object { $_.FullName -match 'test' } | ForEach-Object { $f=$_.FullName; rg -o --no-filename '^(pub(\([^)]*\))?\s+)?(const\s+|async\s+)?(fn|const|static|struct|enum|macro_rules!)\s+\w+' $f | ForEach-Object { [pscustomobject]@{ Name=($_ -split '\s+')[-1]; File=$f } } } | Where-Object { $_.Name -notin $tests } | Group-Object Name | Where-Object { ($_.Group.File | Sort-Object -Unique).Count -gt 1 } | ForEach-Object { "$c $($_.Name): $(($_.Group.File | Sort-Object -Unique) -join ', ')" } }`
  - `Get-Content ../cabinet/_scratch/vibe-coder-internal-test-harnesses/lines-before.txt; foreach ($c in 'engine','lua','vfs','parser','types','model-client') { $n = (Get-ChildItem "crates/runvil-internal/$c" -Recurse -Filter *.rs | Where-Object { $_.FullName -match '[\\/](tests|test_support)[\\/]|[\\/](tests|test_support)\.rs$|-tests\.rs$|[\\/]tests-[^\\/]+\.rs$' } | ForEach-Object { (Get-Content $_.FullName).Count } | Measure-Object -Sum).Sum; "$c $n" }`
- Rules: PowerShell, run from the repository root, which every relative path here (`crates/...`, `../cabinet/...`) resolves against. Run cargo commands one at a time and never append `2>&1`; judge a cargo step by its result lines and `$LASTEXITCODE`. Read the root `AGENTS.md` and the nearest `AGENTS.md` above each crate you edit. A file leaves by `Move-Item` to `../cabinet/_trash/`. No test is removed or weakened, and no dependency is added.
- Settle the clippy assumptions first, with one deliberate violation per ban in an Engine crate's test code, all removed before the commit: confirm that each banned path below is reported, `std::collections::hash_map::RandomState::new` through that re-export included, and that clippy's config lookup stops at the nearest `clippy.toml` without merging (a root-only entry left out of the nearer file is not enforced). If clippy behaves otherwise, the bans move to a `build-xtask` source scan of the same paths instead of the three files, and the commit message says so.
- Add `crates/runvil-internal/clippy.toml` (all six internal crates), `crates/runvil/clippy.toml`, and `crates/runvil-plugin/clippy.toml`. Each repeats the root `clippy.toml` settings (`allow-unwrap-in-tests`, `allow-expect-in-tests`, and the seven process-global `disallowed-methods` entries with their reasons), then adds `disallowed-methods` entries whose reasons name the replacement:
  - `std::thread::sleep`: a timer effect the test answers
  - `std::time::SystemTime::now` and `std::time::Instant::now`: a fixed `Timestamp`, or a count of what happened
  - `std::collections::hash_map::RandomState::new`, std's OS-seeded hasher: `Seeded`
  - `std::sync::Condvar::wait_timeout`, `std::sync::Condvar::wait_timeout_while`, `std::sync::mpsc::Receiver::recv_timeout`, and `std::thread::park_timeout`: an untimed wait, with nextest's timeout as the backstop
- Seeded generation stays allowed; threads stay allowed when a test joins them before asserting; real-filesystem tests stay allowed in the facade, the engine, and vfs on fresh `TempRoot` folders; platform-specific behavior is `cfg`-gated, never skipped at run time.
- A new `build-xtask` check beside `engine_deps.rs`, wired into the engine guards, fails in three cases: an Engine crate resolves (the nearest `clippy.toml` walking up from its manifest directory) to a `clippy.toml` other than these three; one of the three is missing a root entry or a determinism ban; any Engine source under `src`, `tests`, `benches`, or `examples` contains an `allow` or `expect` of `clippy::disallowed_methods`. Its messages name the file, what is required, and what was found.
- Its fixture tests: `an_engine_crate_resolving_to_another_clippy_toml_is_reported`, `a_clippy_toml_missing_a_root_entry_is_reported`, `a_clippy_toml_missing_a_determinism_ban_is_reported`, `an_engine_source_suppressing_disallowed_methods_is_reported`, and `the_engine_clippy_tomls_pass`.
- The root `AGENTS.md` gains, under "Structural Rules":
  - Engine crates, tests included, never depend on tokio, tokio-util, async-trait, reqwest, getrandom, or fastrand, directly or transitively.
  - No Engine test sleeps, reads the wall clock, waits with a timeout, or draws entropy from the OS.
  - Pseudo-random test input comes only from a fixed seed that the failure prints.
  - Temporary directory names come from the test's name, the process id, and a counter.
- Acceptance:
  - Every exit-criteria command passes, and the two `rg` checks, the `tempfile` `rg`, and the duplicate-helper search print nothing; the graph loop prints an empty list for all eight Engine crates.
  - The leaf comparison prints nothing marked `<=` except, under `build-xtask`, `a_forbidden_crate_in_dev_dependencies_only_passes`, `an_optional_entry_enabled_only_by_test_support_passes`, and `a_dep_gated_entry_with_only_a_weak_feature_path_passes`. It prints `=>` only for: under `runvil-types`, `seeded_failure_messages_name_the_seed`, `derive_matches_its_known_answer`, `the_nonce_and_run_id_differ_from_each_other_and_the_seed_and_repeat`, `a_run_seed_round_trips_through_serde_as_64_hex_characters`, and `a_run_seeds_debug_shows_no_seed_bytes`; under `runvil-engine`, `temp_roots_with_one_label_get_distinct_paths_and_drop_removes_the_folder`, `seeded_failure_messages_name_the_seed`, and `a_run_contexts_debug_shows_the_run_id_and_not_the_seed`; under `runvil-vfs` and `runvil`, `temp_roots_with_one_label_get_distinct_paths_and_drop_removes_the_folder`; under `workshop-run-log`, `a_32_byte_seed_survives_a_run_log_round_trip`; under `build-xtask`, the three renamed tests, Step 17's five new tests, and this step's five.
  - The engine's and lua's test line counts are lower than in `lines-before.txt`, against rough targets of 15 to 25 percent for the engine and 10 to 15 percent for lua, and the parser and engine together are lower. The four smaller crates may rise only by the utilities they host and those utilities' self-tests, as the Product Requirements allow. Report all six counts beside the baseline in the commit message.
- Result: test lines, baseline then final: engine 35,714 to 31,609 (-11.5%); lua 11,052 to 10,405 (-5.9%); vfs 3,811 to 3,535 (-7.2%); parser 2,366 to 2,387 (+0.9%, the cross-crate builder); types 2,282 to 2,429 (+6.4%, `Seeded`, the shared helpers, and the new run-seed tests); model-client 727 to 726. Parser and engine together 38,080 to 33,996. The engine and lua dropped but missed their rough targets. Two timing-flaky `gateway-stt-engine` tests failed one full run under load and passed the next; they are outside this plan and deferred.

</step-18>

</execution-plan>
