---
name: Rust rulebook sweep
overview: Apply the Rust rulebook across the promptforge workspace on master, one commit per finding class, keeping every repo convention documented in AGENTS.md. Sized from five repo-wide scans of 611 production files.
todos:
  - id: step-1
    content: "Step 1 (ci-baseline): gate the child's store write in cancel_ends_a_parked_task_idempotently; 20 consecutive passes"
    status: pending
  - id: step-2
    content: "Step 2 (async-fs): spawn_blocking for workshop-workspace fs work and harness-sessions Runtime::launch; move UI_STATE_VALUE_CAP re-export"
    status: pending
  - id: step-3
    content: "Step 3 (emitter-flags): DebugMode and OutputTrust replace Emitter bool params"
    status: pending
  - id: step-4
    content: "Step 4 (error-model): strip {source} from 15 Display sites and rewire renderers; newtype 5 leaked foreign error types with manual From; 13 message fixes"
    status: pending
  - id: step-5
    content: "Step 5 (error-model): #[non_exhaustive] on 10 error enums, ~17 wire enums, events! macro, response/event structs"
    status: pending
  - id: step-6
    content: "Step 6 (error-model): attach causes at ~62 non-lua .map_err(|_|) sites; report sites left"
    status: pending
  - id: step-7
    content: "Step 7 (layout-lint-hygiene): retire 11 mod.rs files by pure moves, form per three-file rule"
    status: pending
  - id: step-8
    content: "Step 8 (layout-lint-hygiene): allow->expect with reason, 3 test-only re-exports, 2 doc unwraps, vm.rs fences, get_all rename, must_use, doubled cfg"
    status: pending
  - id: step-9
    content: "Step 9 (layout-lint-hygiene): hollow parser lib.rs into modules; shared-loopback if over 500; rename types.rs and wire/shared.rs"
    status: pending
  - id: step-10
    content: "Step 10 (paused-time): 5 sentinels to pending(), start_paused on ~14 in-process tests, Workspace clock injection for wait_past"
    status: pending
  - id: step-11
    content: "Step 11 (docs-prose): gateway family summaries to third person and //! lines"
    status: pending
  - id: step-12
    content: "Step 12 (docs-prose): promptforge and harness families summaries and //! lines"
    status: pending
  - id: step-13
    content: "Step 13 (docs-prose): workshop, shared, build summaries and //! lines; FULL verification block"
    status: pending
isProject: false
---

# Rust rulebook sweep of promptforge

<product-contract>

## Product Requirements

Bring the promptforge Rust workspace (`C:\Users\Vinnie\cursor\promptforge`, branch `master`) into line with the Rust rulebook at `tools-public/rulebooks/rust-rulebook.md`, applying every rule that makes sense for this repository and leaving untouched every convention the repository has deliberately chosen and documented in `AGENTS.md`. The sweep is sized from five read-only scans over 611 production Rust files and 107 commits from the last five days.

Requirements:

- R1. CI is green before any sweep commit lands. CI run 35511172403 on `9530340` failed one nondeterministic test; it must be made deterministic first.
- R2. No library `async fn` performs blocking filesystem work inline on the tokio executor.
- R3. No public `thiserror` type renders its `#[source]` in `Display` and also returns it from `source()`; no public error enum names a third-party error type in its variants; error messages follow the rulebook's lowercase, no-prefix, no-period style.
- R4. Public error enums and wire/protocol enums carry `#[non_exhaustive]`; wire structs that only the owning crate constructs carry it too. Internal state-machine enums stay exhaustive.
- R5. Discarded error causes (`.map_err(|_| ...)`) in non-lua crates attach `#[source]` where a slot exists or can be added without a new variant.
- R6. Lint suppressions carry a reason and use `#[expect]` where the lint fires; test-only re-exports live in test modules; doc examples use `?`, not `.unwrap()`; Rust shown in doc `text` fences compiles as a doctest where the API is reachable.
- R7. No `mod.rs` files except the two rulebook-sanctioned `tests/common/mod.rs`; module layout follows `AGENTS.md`'s three-file rule.
- R8. The engine emitter takes typed enums, not `bool` flags, for `debug` and `trusted`.
- R9. `promptforge/parser/src/lib.rs` is a facade (docs, `mod`, `pub use`) and under 500 lines; junk-drawer module names `types.rs` and `wire/shared.rs` are renamed to their concept.
- R10. Doc summary lines are third-person indicative; every module file opens with a `//!` line.
- R11. Deterministic in-process async tests use paused time; sentinel sleeps used as never-completing race arms become `std::future::pending()`.

Non-goals (repository conventions that win over the rulebook, unchanged by this plan):

- Manifestless family containers under `crates/` and short directory names (AGENTS.md Structure).
- The 500-line ceiling enforced only on `## Invariants` crates; `foo-bar.rs` `#[path]` siblings; `*-tests.rs` test files; the flat-directory rule.
- `clippy::pedantic = deny`; the four members (`workshop/shell`, `gateway/app`, `gateway-api-discovery`, `gateway/stt/whisper-ffi`) that mirror lints instead of `workspace = true`, because Cargo forbids combining `workspace = true` with an `unsafe_code` override and those crates hold `unsafe`.
- `workshop/shell` explicit WebView version pins (documented, mirror tauri-runtime-wry); the `turso = "=0.7.2"` workspace pin (documented pre-1.0 API churn; every consumer is `publish = false`).
- `anyhow` in `build-*` lib crates (build tooling consumed only by binaries) and `test-fixtures`-gated `Box<dyn Error>` returns.
- Model-facing message design; private `Result<_, String>` refusal-text helpers in the scheduler (data, not errors).
- `models` module names (domain concept: LLM models, not a junk drawer).
- `promptforge/parser` `-> impl Iterator` returns (family-private crate behind the door).
- `whisper-ffi` bool setters (mirror the C API); the `VfsAccess` 11-method trait (filesystem surface).
- Import grouping (repo is mixed 3/4 groups; reordering is churn with no reader benefit).
- Sleeps in tests that drive real subprocesses or loopback sockets (~45 sites: `gateway/app/tests/it`, `gateway-api-discovery/tests/it`, `workshop/server/tests/it`, `workshop/shell/gateway/tests`, `build-workshop/tests`); paused time auto-advances the clock when idle and would fire the server's own timeouts early. Blocking-pool fakes (`harness/runner/tests/it/support.rs:249`, `effect_loop.rs:51`, `performers.rs:146`, `api-runtime/run-tests.rs:373`) likewise stay on real time.
- The 46 `promptforge/lua` `.map_err(|_|` sites (mlua-to-kind-table conversions by design).
- The 57 production files over 500 lines outside the enforced crates (top: `gateway/local/src/runtime.rs` 2016, `gateway/config/src/config/accessors.rs` 1878, `gateway/logging/src/worker.rs` 1558); a follow-up.

## Functional Specification

Observable outcomes, per requirement:

- F1 (R1). `promptforge-api-runtime` `execute::tests::waits::cancel_ends_a_parked_task_idempotently_and_reports_task_cancelled_once` passes on every run because the child's store write is parked behind a gate until the cancel is observed; running it 20 times consecutively produces 20 passes.
- F2 (R2). `workshop-workspace` handlers (`tree`, `read_file`, `write_file`), `WorkspaceFile::{create, open, duplicate_to}`, the actor's `snapshot` and `close_database`, `workspace-backing.rs` (`grant_and_persist`, `revoke_and_persist`, `open_file`, `current`, `swap_backing`, `reload_current`), `workspace-pointer.rs` (`reopen_last`, `remember`), and `harness-sessions` `Runtime::launch` reach `std::fs` only through `tokio::task::spawn_blocking`. Behavior and all existing tests are unchanged. `current` uses one `spawn_blocking` for the whole per-grant loop.
- F3 (R3). The 15 variants listed in Technical Design render only their own message in `Display`; callers that showed the string to a person or model now render the `source()` chain. `LogError`, `WorkspaceFileError`, `SidecarError`, `LocalError`, `FetchError` name only crate-owned types in their public variants; `?` on the underlying `turso::Error` / `serde_json::Error` / `reqwest::Error` still compiles at every existing call site. The 13 flagged `#[error]` messages are lowercase-led (where the acronym is not the subject) with no `failed to` / `failed:` prefix.
- F4 (R4). The 10 error enums, ~17 wire enums, the `events!` macro output, and the response/event structs listed in Technical Design carry `#[non_exhaustive]`. `WaitError` and `FailureKind` remain exhaustive. Request-direction structs built by callers remain literal-constructible.
- F5 (R5). Each non-lua `.map_err(|_|` site either carries its cause or is listed in the step's return with the reason it was left.
- F6 (R6). Zero `#[allow(` without `reason` in `crates/`; the three `#[cfg(test)] pub(crate) use` re-exports in `promptforge-api-runtime` and the one in `workshop/workspace` live inside test modules; the two doc examples in `gateway/app` use `?`; the five `text` fences in `promptforge/lua/src/vm.rs` are compiled doctests or carry a one-line reason for staying text.
- F7 (R7). `rg --files -g mod.rs crates` lists only `gateway/stt/api/tests/common/mod.rs` and `workshop/server/tests/common/mod.rs`.
- F8 (R8). `Emitter::new`, `Emitter::root` take `DebugMode`; `Emitter::tool_result` takes `OutputTrust`; every call site in `promptforge-api-runtime` passes the enum.
- F9 (R9). `promptforge/parser/src/lib.rs` holds only crate docs, attributes, `mod`, and `pub use`, under 500 lines; `shared-vfs/src/types.rs` and `gateway/stt/api/src/realtime/wire/shared.rs` are renamed to the concept they hold.
- F10 (R10). The 254 imperative summary lines read in third person; 48 production and 105 test module files open with `//!`; `workshop/shell/src/main.rs` opens with `//!` before its release-build comment.
- F11 (R11). The 5 sentinel `sleep(30s)` sites use `std::future::pending()`; the listed in-process tests in `promptforge-api-runtime`, `harness/runner/tests/it`, `workshop/gateway`, `workshop/workspace` run under `start_paused = true`; `workspace-tests-grants.rs` no longer spins on the wall clock because `GrantMeta::added_at` takes its timestamp from an injectable clock.
- F12. `cargo fmt --all --check`, both clippy partitions with `-D warnings`, `cargo doc` with `-D warnings`, the full nextest suite, the doctest pass, and `cargo test -p build-xtask` all pass on the final commit.

</product-contract>

<implementation-contract>

## Technical Design

Target: `C:\Users\Vinnie\cursor\promptforge` on `master`. Steps run serially so each commit's content is what the final verification sees; parallelism is inside a step, with subagents split by crate over disjoint files. Dependencies: step 1, 2, 5 are mutually disjoint. 3 follows 2 (same error files). 2b follows 3. 4a and 4b follow 1 (the workspace re-export move lands in a file step 1 rewrites). 6 follows 3 (the parser `Error` enum lives in the `lib.rs` being hollowed). 8 follows 1 (workspace tests and `GrantMeta`). 7 is last because it touches every crate. Commit sequence: 0, 1, 2, 5, 3, 2b, 4a, 4b, 6, 8, 7 (7 as one commit per crate group).

### D0. Green baseline: the flaky wait test

CI run 35511172403 on `9530340` failed one test: `promptforge-api-runtime` `execute::tests::waits::cancel_ends_a_parked_task_idempotently_and_reports_task_cancelled_once` (`waits.rs` ~305-360). The child is meant to park on a store write, but the test uses ungated `TestStore::new()`, so whether the child completes (`return 'never'`) before `tasks.cancel(t)` lands is the blocking pool's choice; on CI the child won and the Lua assert at prologue line 20 fired with message `never`. Same class of race that `09f29994` fixed for the arm-conflict tests.

- Expose `StoreGate`, `GateObserver`, and `gated_store` from `execute/tests/scheduler.rs` (~2923-3000) as `pub(super)`.
- Rebuild the context in the wait test on `gated_store(&gate)` so the child's `store.write('child-park', ..)` parks with the gate held; open the gate once the cancel has been observed (via `GateObserver` on the child's `TaskCancelled`) so the run-end drain completes.

### D1. Blocking filesystem work off the executor (rulebook section 14)

`crates/workshop/workspace` has ~15 `async fn` doing `std::fs` / `Path::exists` / `canonicalize` inline and zero `spawn_blocking`; `harness/sessions/src/runtime.rs` `Runtime::launch` walks the agent directory inline. Pattern to copy: `workshop/user-state/src/store.rs` ~95 (`spawn_blocking` around `write_atomic`).

- `workspace_file.rs` `create` / `open` / `duplicate_to`: hoist the sync probes and `plan_siblings`/`copy_siblings` into a sync helper run via `tokio::task::spawn_blocking`.
- `workspace_file-actor.rs` `snapshot` (`fs::copy`) and `close_database` (`remove_empty_wal_sidecar`): `spawn_blocking` inside the actor, awaited so the actor's command ordering is unchanged.
- `workspace-backing.rs` `grant_and_persist` / `revoke_and_persist` / `open_file` / `current` / `swap_backing` / `reload_current`; `workspace-pointer.rs` `reopen_last` and `remember`: same. `current` stats one path per grant; batch the whole loop into one `spawn_blocking`.
- `handlers.rs` `tree` / `read_file` / `write_file`: wrap the `Workspace::*` sync calls. `spawn_blocking` needs `'static`: clone the `Arc<Workspace>` (or whatever the axum `State` holds) and move owned `PathBuf`s in. Keep the confinement `canonicalize` and the read/write inside the same closure so the check-then-use window does not widen.
- `harness/sessions/src/runtime.rs` `launch`: `spawn_blocking(discover)`.
- Also move `#[cfg(test)] pub(crate) use ui_state::UI_STATE_VALUE_CAP;` (`workspace_file.rs` ~34) into the two consuming test modules while the file is open.

### D2. Error type shapes (section 5)

Attributes and type definitions only; call-site work is D2b.

- P1 (15 sites): strip `{source}` / `{error}` / `{0}` from `#[error(...)]` where the field is `#[source]`/`#[from]`. Files: `harness/log/src/error.rs` ~11/~18/~26, `harness/runner/src/prepare.rs` ~116/~125/~140, `harness/sessions/src/environment.rs` ~347 and `session/run.rs` ~36/~40/~43, `promptforge-api-runtime/src/error.rs` ~265/~447, `gateway/cloud-providers/src/lib.rs` ~102, `gateway/app/src/dialect.rs` ~368, `workshop/server/src/app.rs` ~463. For each site, `rg` the enum name to find who renders it. Where the string reaches a person or a model through `{}` or `to_string()` (UI status frames in `workshop/server`, tool output in `harness/runner`), the renderer must walk `source()` instead; if no chain-rendering helper exists in that crate, add a small one (`fn display_chain(&dyn Error) -> String`). Where only `{:?}` / `anyhow` / `tracing` consume it, nothing else changes. Update tests asserting the full string.
- P2 (5 pub enums leaking third-party types): wrap each foreign error in a crate-owned `#[error(transparent)] pub struct XxxSource(turso::Error)` newtype with a private field so the public API names only crate types. `LogError` (turso, serde_json), `WorkspaceFileError` (turso), `SidecarError` (serde_json), `LocalError` (reqwest, serde_json), `FetchError` (reqwest). `?` applies a single `From`, so `#[from]` on the newtype alone does not make `turso::Error -> LogError` work; write `impl From<turso::Error> for LogError` by hand through the newtype. Existing `.map_err(LogError::Database)`-style call sites keep compiling if the variant shape stays the same.
- P6 (13 messages): lowercase `HTTP`/`STT`/`PCM16`-led messages where the acronym is not the subject (`harness/webfetch/src/error.rs` ~200, `gateway/stt/api/src/artifacts.rs` ~144/~152, `audio.rs` ~22); drop `failed to` / `failed:` prefixes (webfetch ~144, prepare ~125, environment ~347, api-runtime error ~434/~447, `workshop/server` session-menu ~182, cloud-providers ~102, `gateway/app` error ~130, api_error ~62).

### D2b. Discarded error causes (section 5)

`.map_err(|_|`: 108 sites; skip the 46 in `promptforge/lua`. For the remaining ~62 (dominant: `gateway/stt/api` 13, `whisper-ffi` 7, `gateway/local` 7, `promptforge-api-runtime` 6, `workshop/*` ~10): attach `#[source]` where the target variant already carries one or can take one without adding a variant; leave sites where the discarded error carries no information (integer conversions, `TryFrom` on constants, `Option`-like probes). Parallel by crate; each subagent reports the sites it left alone and why.

### D3. `#[non_exhaustive]` on error and wire types (sections 5, 6)

- 10 pub error enums: `ReplayError` (`promptforge-api-types/src/replay.rs` ~104), parser `Error` (`promptforge/parser/src/lib.rs` ~53), model-client `Error` (`error.rs` ~27), lua `Error` (`error.rs` ~59), `CurrentModelError` (`harness/sessions/src/environment.rs` ~345), `DriveError` (`harness/runner/src/effect_loop.rs` ~59), `PrepareError` (`prepare.rs` ~113), `LogError` (`harness/log/src/error.rs` ~9), `DialectResolveError` (`gateway/local/src/dialect.rs` ~40), `FetchError` (`gateway/cloud-providers/src/lib.rs` ~94). Skip `WaitError` and `FailureKind` (commented as deliberately exhaustive).
- ~17 serde wire enums: `gateway-api` `EnvRole`/`Tier`/`SliceStatus`; `gateway/protocol` `EmbeddingInput`/`SpeechVoice`/`SpeechResponseFormat`/`SpeechStreamFormat`; `gateway/config` `LlamaBackend`; `harness/sessions` `DeltaKind`; `workshop/protocol` `InputFrame`/`AgentEventKind`/`AgentDeltaKind`; `promptforge-api-types` `TaskOrigin`/`AbandonReason`; lua `StoreOp`; the `events!` macro in `promptforge-api-types/src/event.rs` so `Event` gets it.
- Wire structs: `#[non_exhaustive]` on a struct forbids struct-literal construction from every other crate, including `tests/it/`. Attribute only structs the owning crate constructs and others merely read: `gateway/protocol` `ChatResponse`/`ChatChunk`/`ChatChunkChoice`/`EmbeddingResponse`/`RerankResponse`/`ModelsResponse`, `harness/sessions` `SessionEvent`/`Delta`/`SessionFailure`, `harness/log` `StoredRecord`/`RunRow`, `workshop/protocol` `AgentEvent`/`Progress`/`StatusBarUpdate`/`CatalogPush`. Skip request-direction bags callers build by literal (`ChatRequest`, `EmbeddingRequest`, `SpeechRequest`, `RerankRequest`, `LaunchRequest`, `RunMeta`, `Record`, `SwitchProfileFrame`, `WorkbenchSnapshot`) unless `rg 'StructName \{'` shows no cross-crate literal.
- Variant-level attribute: only fix enums where some data variants already have it and siblings do not; not a blanket 122-variant pass.
- Add `_ =>` or `..` arms only where a downstream crate match now fails; each one gets a one-line comment naming the enum's owner.

### D4a. Module layout moves (section 7)

Pure `git mv` plus `mod` path fixes, no content edits.

- 11 `mod.rs` files: production `promptforge/lua/src/{tools,models}/mod.rs`, `promptforge-api-runtime/src/fanout/mod.rs`, `gateway/cloud-providers/src/providers/mod.rs`, `gateway/stt/api/src/realtime/mod.rs`; test `*/tests/mod.rs` under `promptforge-api-runtime/src/execute`, `promptforge-api-runtime/src/lua`, `harness/models/src/transport`, `promptforge/lua/src/protocol`, `gateway/app/src/cloud_models` and the remaining two found by `rg --files -g mod.rs crates`. Keep the two `tests/common/mod.rs`.
- Target form per AGENTS.md's three-file rule: if the directory holds three or more sibling files after `mod.rs` leaves, convert to `foo.rs` + `foo/`; if fewer, flatten to `foo.rs` + `foo-bar.rs` with `#[path]`. Count first, choose per directory.
- Check each moved parent for existing `#[path]` attributes that point into the directory and fix them.

### D4b. Lint hygiene and test plumbing (sections 11, 12)

- 6 `#[allow]` without reason -> `#[expect(lint, reason = "...")]`: `harness/runner/src/spawn.rs` ~31/~58/~79 (`clippy::disallowed_methods`), `gateway-api/src/lib.rs` ~119 (`struct_excessive_bools`), `gateway/cloud-providers/src/providers/cohere.rs` ~120 (cast lints), `bedrock-sigv4.rs` ~35 (`too_many_arguments`). Confirm `build-xtask` `harness_bans` does not grep the literal `allow`. Plain rustc skips expectations for `clippy::` tool lints, so `#[expect]` is safe outside clippy runs.
- 3 test-only re-exports moved into the consuming test modules: `promptforge-api-runtime/src/store.rs` ~24 `StoreExt`, `lua.rs` ~14 `ToolOutputKind`, `execute/scheduler.rs` ~100 `TaskState`.
- 2 doc examples using `.unwrap()` -> `?` with hidden `Ok` tail: `gateway/app/src/api_error.rs` ~24, `runner.rs` ~828.
- 5 ` ```text ` fences holding Rust in `promptforge/lua/src/vm.rs` ~51/~231/~433/~783/~996: convert to compiled doctests (`no_run` plus hidden setup) where the referenced API is reachable from the crate root; leave as `text` where it is not, with a one-line reason comment.
- `workshop/user-state/src/store.rs` `get_all` -> `all` (the two `get_state` are axum GET handlers; keep).
- Verify the 11 `Self`-returning constructors lacking `#[must_use]` (`shared-vfs/src/handle.rs` ~181/~189, `workshop/menu/src/menu.rs` ~184, `workshop/registry/src/traits.rs` x7, `build-llama-cuda/src/probe.rs` ~26); add where the type itself is not already `#[must_use]`.
- Verify `gateway/app/src/model_info.rs` no longer carries the doubled `#[cfg(feature = "local")]` left by `d77a8b48`; remove if present.

### D5. Bool parameters in the engine emitter (section 6)

`promptforge-api-types/src/emitter.rs` `Emitter::new` / `root` (`debug: bool`) and `tool_result` (`trusted: bool`). `OutputTrust` already exists in `promptforge_api_types::tools`; use it for `trusted`; add a two-variant `DebugMode { Off, On }` (or the nearest existing name) for `debug`. Update `promptforge-api-runtime` `execute/event_buffer.rs` and `RunContext::report_debug` call sites. Leave the other 24 single-bool pub fns.

### D6. Facade root and junk-drawer names (section 7)

- `promptforge/parser/src/lib.rs` (677 lines, 31 fns, 8 impls): move the logic into named modules (`build.rs` already exists; add siblings by concept), leave `lib.rs` as docs + `mod` + `pub use`. `tests.rs` (1317 lines) reaches private items through `use super::*`; moved items need `pub(crate)` visibility and the test module's imports updated, or the affected tests move beside the new module as `<module>-tests.rs`. Read `tests.rs` imports before choosing the split.
- `shared-loopback/src/lib.rs` (35 fns): same treatment if over 500 lines, else defer.
- Rename `shared-vfs/src/types.rs` and `gateway/stt/api/src/realtime/wire/shared.rs` to the concept they hold (read contents first).
- Any other file touched in D0-D5 that is over 500 lines gets split before edit per AGENTS.md.

### D7. Documentation prose (section 10)

Docs-only, no code motion, parallel by crate.

- 254 imperative summary lines -> third-person indicative (`Spawn` -> `Spawns`), verb form only, meaning untouched. Dominant: `gateway/cloud-providers` 69, `gateway/app` 47, `build-xtask` 25, `gateway/protocol` 25.
- 48 production files without a `//!` first line (37 in `gateway/stt`, 7 in `promptforge/lua`) and 105 test files: add one sentence each.
- `workshop/shell/src/main.rs` opens with a `//` comment: move the `//!` above it.

### D8. Paused time in deterministic async tests (section 11)

In-process tests only; each conversion is read first, because `start_paused` turns "nothing arrives within 50ms" into "time jumps until something arrives".

- Sentinel `tokio::time::sleep(30s)` used as a never-completing race arm -> `std::future::pending()` (5): `harness/models/src/transport/tests/limits.rs` ~112/~149, `harness/web-search/src/web_search-tests.rs` ~395, `promptforge-api-runtime/src/execute/tests/mod.rs` ~1330, `execute/tests/scheduler.rs` ~3721.
- `promptforge-api-runtime` (~9): `#[tokio::test(flavor = "current_thread", start_paused = true)]` following the 17 precedents in `timeouts.rs` and `input.rs`. Fixed delays `tool_loop.rs` ~482, `input.rs` ~316; 10ms poll loops `scheduler.rs` ~150/~3412/~3751, `live_infer.rs` ~314; fake-latency `sleep(delay)` in `model_task_notices.rs` ~48, `tests/mod.rs` ~980. `test_support/tokio_driver.rs` ~357 is the timer performer itself and needs no edit. Where a test asserts a slow-vs-fast race, replace the sleep with an explicit `tokio::time::advance`.
- Harness and workshop in-process (~5): `harness/runner/tests/it/effect_loop.rs` ~205, `performers.rs` ~53; `workshop/gateway/src/gateway_progress-tests.rs` ~129/~140; `workshop/workspace/src/workspace-file-tests-mutations.rs` ~417.
- `workshop/workspace/src/workspace-tests-grants.rs` `wait_past` spins on the wall clock until the RFC 3339 second changes. Inject the clock: give `GrantMeta::added_at` its timestamp through a `now` function on `Workspace` (defaulting to `now_rfc3339`) so the test sets two distinct stamps directly. Small production change, ships with the test.

### Verification commands (PowerShell)

```
cargo fmt --all --check
cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings
cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings
$env:RUSTDOCFLAGS = "-D warnings"; cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api
cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features
cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api
cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc
cargo test -p build-xtask
```

</implementation-contract>

<verification-contract>

## Testing Plan

Tests are kept light. This sweep is overwhelmingly refactor and attribute work whose regression guard is the existing suite; new tests are written only where a step introduces a behavior a reader could break without noticing.

- Per step, the coder runs only the focused test command for the crates it touched (`cargo nextest run --locked -p <crate> --all-features`, plus `cargo test -p <crate> --doc` when a doc example changed). No formatter, linter, docs, or full-suite run per step.
- Full verification runs once, on the final step, using every command in the Technical Design verification block, in order.
- New tests, by step:
  - Step 0: none new; the existing test is made deterministic and run 20 times consecutively (`for ($i=0; $i -lt 20; $i++) { cargo nextest run --locked -p promptforge-api-runtime --all-features cancel_ends_a_parked_task }`), all passing.
  - D1: none new unless a sync helper is extracted with an edge case of its own; existing `workspace-tests-*.rs`, `workspace-file-tests-*.rs`, and `harness/sessions/tests/it` are the guard.
  - D2: for each renderer switched to walk `source()`, one assertion that the rendered string contains the cause text; for each `From<foreign> for Enum` impl, the existing `?` call sites compiling is the test.
  - D2b, D3, D4a, D4b, D6, D7: none new; compilation plus existing tests.
  - D5: one test that `Emitter::tool_result` with `OutputTrust::Untrusted` produces the same event the old `trusted: false` did, if no existing test already pins that event.
  - D8: the converted tests are the tests; each must pass under `start_paused` and, where a race is asserted, with the explicit `advance`. The clock injection ships with the rewritten `wait_past` test.
- A step whose focused tests fail is not marked complete.
- Review is one round per step, findings fixed in the same commit, per the vibe-coder cycle; verification (Verify dispatch) is skipped on every step except the last, where Scope is `FULL`.

</verification-contract>

<decision-record>

## Decision Record

- Checkout: `promptforge` on `master` (the audit ran here and the harness code lives here), not `promptforge3`/`vibe3`.
- Display and source: the rulebook wins. Models receive their own display string shaped for LLM consumption; the `source()` chain is for the Rust side. Strip `{source}` from `Display` wherever `#[source]`/`#[from]` is present and make human/model renderers walk the chain.
- `#[non_exhaustive]` scope: error enums and wire/protocol types only. Internal state-machine enums stay exhaustive so cross-crate matches keep breaking on new variants. Request-direction data bags stay literal-constructible.
- Foreign error types in public enums: hide behind crate-owned `#[error(transparent)]` newtypes rather than converting each enum to an opaque `struct Error(Repr)`; the newtype is the minimal change that removes the third-party name from the signature.
- Tests light: no new tests for refactor and attribute steps; the existing suite is the guard. Full verification once at the end, not per step.
- Serial steps, parallel within a step: so the final verification sees exactly the committed content and the per-step focused runs are attributable.
- Leave the `turso = "=0.7.2"` pin, the four lint-mirroring manifests, and the family-container layout: each is a documented, forced, or deliberate repo choice.
- Skip the 46 lua `.map_err(|_|` sites: the kind-table error design has no source slot by design.
- Skip converting real-process integration test sleeps: paused time with real sockets fires the system under test's own timeouts early.
- Step 0 uses the existing `StoreGate`/`GatedStore` machinery rather than a new fixture: it is the mechanism `09f29994` already established for this race class.

</decision-record>

<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build --locked -p gateway` (the workspace `default-members` is `crates/gateway/app`, so bare `cargo build` builds only the gateway; the desktop app is `cargo build --locked -p workshop`, which first needs `npm ci --prefix crates/workshop/ui` and `npm ci --prefix crates/gateway/config-ui/ui` because the crates' build scripts bundle the UIs into `OUT_DIR`). Toolchain: `rust-toolchain.toml` pins `stable`; edition 2024; resolver 3. On Windows `.cargo/config.toml` links with `rust-lld` and `+crt-static`.
- Focused test command pattern: `cargo nextest run --locked -p <crate> --all-features [<test-name-substring>]`; doctests are separate under nextest: `cargo test -p <crate> --doc`. Workshop crates (`workshop`, `workshop-server`, `workshop-server-api`) drop `--all-features`; `workshop-server` also has `--features headless`. Plain `cargo test --locked -p <crate> --test it <name>` is used in CI for single integration tests. `.config/nextest.toml` sets a 60s slow-timeout (terminate after 3 periods), a 250ms leak-timeout, and a `heavy` test group (max 8 threads, 4 required per test) for `gateway-stt` and `gateway-stt-backend-whisper`.
- Component test command pattern: the repo tests in two partitions. Workspace partition: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`. Workshop partition: `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`, then `cargo nextest run --locked -p workshop-server --features headless`, then `cargo test --doc -p workshop -p workshop-server -p workshop-server-api`. A family (harness, gateway, promptforge) is tested by listing its crates with repeated `-p`, e.g. `cargo nextest run --locked -p harness-runner -p harness-sessions -p harness-log --all-features`. Structural harness: `cargo test -p build-xtask`. Product-boundary matrix from cargo metadata: `cargo test -p gateway-stt --test it architecture`.
- Full-suite test command (in CI order): `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`; `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`; `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`; `cargo nextest run --locked -p workshop-server --features headless`; `cargo test --doc -p workshop -p workshop-server -p workshop-server-api`; `cargo test -p build-xtask`. CI additionally runs `cargo check -p gateway --no-default-features` and a clean-tree check (`git status --porcelain` must be empty after the build).
- Linter command: `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings`, then `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`. Never run standalone `cargo check --workspace` beside these (AGENTS.md); the one sanctioned check is `cargo check -p gateway --no-default-features`. Workspace lints in `Cargo.toml`: `clippy::all` and `clippy::pedantic` deny, `unwrap_used`/`expect_used` deny (`clippy.toml` allows both in tests), `doc_markdown` allow, `unsafe_code` forbid, `missing_docs`/`missing_debug_implementations`/`unreachable_pub` warn, `rustdoc::broken_intra_doc_links`/`private_intra_doc_links` deny. Supply chain: `cargo deny check` (`deny.toml`) and `cargo audit`. Pre-push hook runs the headless check, the workspace clippy partition, and cargo deny.
- Formatter check command: `cargo fmt --all --check` (`rustfmt.toml`: `style_edition = "2024"`). The pre-commit hook in `.githooks/` runs it.
- Docs command: PowerShell `$env:RUSTDOCFLAGS = "-D warnings"; cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`. User guide: `mdbook build guide` (`guide/book.toml`), assembled by `build-user-guide`.
- Test placement and naming conventions: three shapes coexist. (1) Inline `#[cfg(test)] mod tests` at the bottom of the source file (~322 files). (2) Sibling test file `<stem>-tests.rs` next to `<stem>.rs`, wired with `#[path = "<stem>-tests.rs"] mod tests;` (108 files; ~100 `#[path]` attributes); large groups split further as `<stem>-tests-<label>.rs` (e.g. `workspace-tests-grants.rs`, `workspace-file-tests-mutations.rs`, `state-alignment-tests.rs`). (3) Cargo integration tests under `tests/it/` with a `main.rs` entry and one module per concern plus a `support.rs` helper (15 crates: `build-ui`, `gateway/app`, `gateway/stt/api`, `gateway-api-discovery`, `harness/{capabilities,log,models,runner,sessions}`, `harness-api`, `workshop/{protocol,registry,server,support,workspace}`); `tests/common/mod.rs` exists in `gateway/stt/api` and `workshop/server`. In-source `tests/mod.rs` subdirectories exist in 7 places (`promptforge-api-runtime/src/{execute,lua,model}/tests`, `promptforge/lua/src/protocol/tests`, `harness/models/src/transport/tests`, `gateway/app/src/cloud_models/tests`, `gateway/config/src/config/tests`); these plus 4 production `mod.rs` files are the D4a targets. Test functions are long snake_case sentences stating the behavior (`cancel_ends_a_parked_task_idempotently_and_reports_task_cancelled_once`, `a_direct_launch_recovers_the_lease_from_a_terminated_owner`). Cross-crate fixtures are gated behind a `test-fixtures` Cargo feature (`gateway-api-discovery`, `workshop/{menu,server,workspace,gateway}`); `promptforge-api-runtime` has an in-crate `test_support/` module. Async tests use `#[tokio::test]`, with `flavor = "current_thread", start_paused = true` already in use in `timeouts.rs` and `input.rs`. UI tests: `npm test` in `crates/workshop/ui` and `crates/gateway/config-ui/ui` (`node --test`), with `npm run typecheck` (`tsc --noEmit`) and `npm run build` (esbuild).
- Directory map: `Cargo.toml` (workspace root, explicit member list, workspace deps and lints), `Cargo.lock`, `rust-toolchain.toml`, `rustfmt.toml`, `clippy.toml`, `deny.toml`, `dist-workspace.toml` (cargo-dist), `AGENTS.md` (repo policy), `README.md`, `.cargo/config.toml` (Windows linker, `cargo workshop` and `cargo xtask` aliases), `.config/{hakari.toml,nextest.toml}`, `.githooks/{pre-commit,pre-push}`, `.github/workflows/` (`ci.yml` plus release, nightly, guide, whisper, miri, installer-smoke workflows), `crates/` (all Rust code; see `crates/README.md`), `guide/` (mdbook user guide), `prompts/` (PromptForge prompt files), `tools/` (Node scripts: `stage-gateway-sidecar.mjs`, `gateway-tts-live.mjs`), `vibe/archdoc.md` (architecture identity and invariants A1-A9), `images/`, `local/`, `target/` and `target-msrv/` (build output). Under `crates/`: root-level public crates `gateway-api`, `gateway-api-discovery`, `harness-api`, `promptforge-api-runtime`, `promptforge-api-types`, `shared-loopback`, `shared-progress`, `shared-vfs`, `workspace-hack`; build tooling `build-llama-cuda`, `build-ui`, `build-user-guide`, `build-workshop`, `build-xtask`; `shared-ui` (TypeScript+CSS package, not a crate); and four manifestless family containers: `promptforge/{lua,parser,store,vfs,model-client}`, `gateway/{app,cloud-providers,config,config-ui,local,logging,protocol,routing,web-search,stt/{api,engine,backend-whisper,whisper-ffi}}`, `workshop/{shell,server,server-api,gateway,menu,protocol,registry,status,support,user-state,workspace,ui}`, `harness/{runner,models,capabilities,log,sessions,web,webfetch,web-search}`.
- Component boundaries: dependencies flow one way, shell -> features -> services -> vocabulary; never lower to higher. Family containers are private: a crate inside `crates/<family>/` may depend only on `crates/` root crates and its own siblings. PromptForge's one door is `promptforge-api-runtime` + `promptforge-api-types` (executor: sans-I/O deterministic state machine, no clock, no host trait objects; depends on store, Lua VM boundary, shared substrate). Harness's one door is `harness-api`; `harness-*` may depend on `promptforge-api-runtime`, `promptforge-api-types`, `gateway-api`, `gateway-api-discovery`, `shared-*`; never on workshop or private gateway crates. Gateway's public pair is `gateway-api` + `gateway-api-discovery`; gateway crates must not depend on promptforge, workshop, or harness crates; the nested `stt/` subsystem exposes only `gateway-stt` to the family. Workshop crates (`workshop-*`) may name the gateway public pair, the promptforge door, and `harness-api` only; the shell (`workshop`) depends on `workshop-server-api`, never `workshop-server`. `shared-*` crates depend on no product crates; `shared-vfs` is std-only. `build-*` crates are exempt from container privacy. The rules bind normal, dev, build, and target-specific dependencies and are enforced by `cargo test -p build-xtask` and `cargo test -p gateway-stt --test it architecture`. Crate name to path: private family crates keep the family prefix (`gateway-local` at `gateway/local`, `harness-runner` at `harness/runner`, `promptforge-lua` at `promptforge/lua`); `gateway` is `gateway/app`, `workshop` is `workshop/shell`, `gateway-stt` is `gateway/stt/api`, `gateway-whisper-ffi` is `gateway/stt/whisper-ffi`.
- Conventions summary: every `workshop-*` and `harness-*` `lib.rs` opens with a `//!` doc carrying a `## Invariants` marker; the 500-line file ceiling is enforced by `build-xtask` on those marked crates (split before edit). Source directories are flat: a subdirectory needs three or more files, otherwise siblings are `foo-bar.rs` wired by `#[path = "foo-bar.rs"] mod bar;`, and the two forms convert in both directions when a group crosses the line. Error types use `thiserror` 2; `anyhow` appears in `build-*` tooling; error and status messages are written for model consumption (concise, factual, required vs actual). Async runtime is tokio with `axum` 0.8, `reqwest` 0.13 on rustls/aws-lc-rs (`ring` must stay out of the gateway closure). Long-running work reports through `shared-progress`. Unsafe code is forbidden workspace-wide except in the explicitly owned FFI crates (`gateway-whisper-ffi` and three STT crates that mirror lints instead of `workspace = true`). Comments explain constraints and cite upstream issue URLs for workarounds. Cargo features gate real constraints only. No build step writes into the repository (UI bundles go to `OUT_DIR`). `workspace-hack` (cargo-hakari) is a dependency of nearly every member. Pinned versions carry a comment naming the reason (`turso = "=0.7.2"`, `tauri = "~2.11"`, `tokio-tungstenite = "0.29"`). Behavior changes ship with tests in the same change; structural enforcement needs explicit user approval. Branch: `master`, HEAD `95303402`. UI code (`crates/workshop/ui`, `crates/gateway/config-ui/ui`, `crates/shared-ui`) is TypeScript bundled by esbuild with CSS beside its TS, `--ws-*` tokens, and no `localStorage`.

</project-survey>

<execution-plan>

## Execution Instructions

Repository `C:\Users\Vinnie\cursor\promptforge`, branch `master`, HEAD `95303402`. Steps run serially in the order below; each step is exactly one commit holding its code and its tests. Parallelism is allowed inside a step only, with workers split by crate over disjoint files. Per step, run only the focused test command for the crates touched (`cargo nextest run --locked -p <crate> --all-features`; `workshop`, `workshop-server`, `workshop-server-api` drop `--all-features`; add `cargo test -p <crate> --doc` when a doc example changed). Formatter, clippy, docs, and the full suite run once, in Step 13. A step whose focused tests fail is not complete. A completed step adds ` [completed]` to its heading; the tag lines never change.

Enforced-crate rule for every step: `workshop-*` and `harness-*` crates carry a 500-line file ceiling checked by `cargo test -p build-xtask`. When an edit would push one of their files past 500 lines, split that file first, in the same commit, using the AGENTS.md three-file rule (three or more siblings: `foo.rs` + `foo/`; fewer: `foo.rs` + `foo-bar.rs` with `#[path]`).

Components in dependency order:

1. `ci-baseline` (Step 1) - R1 requires green CI before any sweep commit; nothing else can land first.
2. `async-fs` (Step 2) - disjoint from every other component, but Steps 7, 8, and 10 rewrite files it edits (`workspace_file.rs`, `workspace-tests-grants.rs`, `GrantMeta`), so it lands before them.
3. `emitter-flags` (Step 3) - disjoint from everything; placed ahead of `error-model` so it never rebases over error-file churn. This swaps the Technical Design's "2, 5" order; both are stated disjoint, so the swap changes no content.
4. `error-model` (Steps 4-6) - shapes first (variant text and newtypes), then `#[non_exhaustive]` on the same files, then call-site causes once every variant and source slot is final.
5. `layout-lint-hygiene` (Steps 7-9) - `mod.rs` moves and the api-runtime re-export moves follow `async-fs` (the workspace re-export move already landed in Step 2's file); the parser facade follows `error-model` because the parser `Error` enum lives in the `lib.rs` being hollowed.
6. `paused-time` (Step 10) - follows `async-fs` because it rewrites `workspace-tests-grants.rs` and adds the clock to `Workspace`.
7. `docs-prose` (Steps 11-13) - last, because it touches every crate and must see final file names and module paths.

<step-1>

### Step 1: Deterministic cancel-wait test [completed]

- Component: `ci-baseline`
- Piece: flaky wait test (D0)
- Covers: R1, F1
- Depends on: none

Changes:

- `crates/promptforge-api-runtime/src/execute/tests/scheduler.rs` (~2923-3000): widen `StoreGate`, `GateObserver`, and `gated_store` to `pub(super)`.
- `crates/promptforge-api-runtime/src/execute/tests/waits.rs` (~305-360), test `cancel_ends_a_parked_task_idempotently_and_reports_task_cancelled_once`: replace the ungated `TestStore::new()` context with one built on `gated_store(&gate)` so the child's `store.write('child-park', ..)` parks while the gate is held; attach a `GateObserver` that opens the gate when the child's `TaskCancelled` event is observed so the run-end drain completes. Mirror the shape `09f29994` used for the arm-conflict tests.

Tests: no new test. Run `for ($i=0; $i -lt 20; $i++) { cargo nextest run --locked -p promptforge-api-runtime --all-features cancel_ends_a_parked_task }`; all 20 must pass.

Commit: one commit naming the test and the gate.

</step-1>

<step-2>

### Step 2: Blocking filesystem work off the tokio executor [completed]

- Component: `async-fs`
- Piece: workshop-workspace and harness-sessions (D1)
- Covers: R2, F2
- Depends on: Step 1

Pattern to copy: `crates/workshop/user-state/src/store.rs` ~95 (`tokio::task::spawn_blocking` around `write_atomic`). `spawn_blocking` closures need `'static`: clone the `Arc<Workspace>` (or whatever the axum `State` holds) and move owned `PathBuf`s in.

Changes in `crates/workshop/workspace/src/`:

- `workspace_file.rs` `WorkspaceFile::create` / `open` / `duplicate_to`: hoist the sync probes and `plan_siblings` / `copy_siblings` into a sync helper run via `spawn_blocking`.
- `workspace_file-actor.rs` `snapshot` (`fs::copy`) and `close_database` (`remove_empty_wal_sidecar`): `spawn_blocking` inside the actor, awaited so command ordering is unchanged.
- `workspace-backing.rs` `grant_and_persist` / `revoke_and_persist` / `open_file` / `current` / `swap_backing` / `reload_current`: same; `current` batches its whole per-grant stat loop into one `spawn_blocking`.
- `workspace-pointer.rs` `reopen_last` / `remember`: same.
- `handlers.rs` `tree` / `read_file` / `write_file`: wrap the `Workspace::*` sync calls; keep the confinement `canonicalize` and the read or write inside the same closure so the check-then-use window does not widen.
- `workspace_file.rs` ~34: move `#[cfg(test)] pub(crate) use ui_state::UI_STATE_VALUE_CAP;` into the two consuming test modules.

Changes in `crates/harness/sessions/src/runtime.rs`: `Runtime::launch` runs its agent-directory walk as `spawn_blocking(discover)`.

Tests: none new unless an extracted sync helper has an edge case of its own. Guard: `workspace-tests-*.rs`, `workspace-file-tests-*.rs`, `crates/harness/sessions/tests/it`. Focused: `cargo nextest run --locked -p workshop-workspace -p harness-sessions --all-features`. Apply the enforced-crate rule to every file above.

Commit: one commit.

</step-2>

<step-3>

### Step 3: Typed enums for the emitter's debug and trusted flags [completed]

- Component: `emitter-flags`
- Piece: engine emitter (D5)
- Covers: R8, F8
- Depends on: Step 1

Changes:

- `crates/promptforge-api-types/src/emitter.rs`: `Emitter::new` and `Emitter::root` take `DebugMode` instead of `debug: bool`; `Emitter::tool_result` takes `promptforge_api_types::tools::OutputTrust` instead of `trusted: bool`. Add `pub enum DebugMode { Off, On }` beside `Emitter` unless an equivalent two-variant enum already exists in the crate, in which case reuse it.
- `crates/promptforge-api-runtime/src/execute/event_buffer.rs` and `RunContext::report_debug`: pass the enums at every call site. The other 24 single-bool pub fns in the workspace stay as they are.

Tests: one test that `Emitter::tool_result` with `OutputTrust::Untrusted` produces the same event the old `trusted: false` did, unless an existing test already pins that event. Focused: `cargo nextest run --locked -p promptforge-api-types -p promptforge-api-runtime --all-features`.

Commit: one commit.

</step-3>

<step-4>

### Step 4: Error shapes - Display, foreign types, message style [completed]

- Component: `error-model`
- Piece: type definitions and renderers (D2)
- Covers: R3, F3
- Depends on: Step 3

Attributes, type definitions, and renderers only; call-site `.map_err` work is Step 6.

- P1, 15 doubled `Display` + `source()` sites: strip `{source}` / `{error}` / `{0}` from `#[error(...)]` where the field is `#[source]` or `#[from]`. Files: `crates/harness/log/src/error.rs` ~11/~18/~26; `crates/harness/runner/src/prepare.rs` ~116/~125/~140; `crates/harness/sessions/src/environment.rs` ~347 and `session/run.rs` ~36/~40/~43; `crates/promptforge-api-runtime/src/error.rs` ~265/~447; `crates/gateway/cloud-providers/src/lib.rs` ~102; `crates/gateway/app/src/dialect.rs` ~368; `crates/workshop/server/src/app.rs` ~463. For each enum, `rg` its name to find renderers. Where the string reaches a person or model via `{}` or `to_string()` (UI status frames in `workshop/server`, tool output in `harness/runner`), the renderer walks `source()`; add `fn display_chain(&dyn Error) -> String` in that crate if no chain renderer exists. Where only `{:?}`, `anyhow`, or `tracing` consume it, nothing else changes. Update tests asserting the full string.
- P2, 5 public enums naming third-party types: add crate-owned `#[error(transparent)] pub struct XxxSource(inner)` newtypes with a private field for `LogError` (turso, serde_json), `WorkspaceFileError` (turso), `SidecarError` (serde_json), `LocalError` (reqwest, serde_json), `FetchError` (reqwest). Write `impl From<turso::Error> for LogError` and the analogous impls by hand through the newtype so every existing `?` site still compiles; keep variant shapes so `.map_err(LogError::Database)`-style sites compile.
- P6, 13 messages: lowercase `HTTP` / `STT` / `PCM16`-led messages where the acronym is not the subject (`crates/harness/webfetch/src/error.rs` ~200, `crates/gateway/stt/api/src/artifacts.rs` ~144/~152, `audio.rs` ~22); drop `failed to` / `failed:` prefixes (webfetch ~144, prepare ~125, environment ~347, api-runtime `error.rs` ~434/~447, `workshop/server` session-menu ~182, cloud-providers ~102, `gateway/app` `error.rs` ~130, `api_error.rs` ~62).

Tests: for each renderer switched to walk `source()`, one assertion that the rendered string contains the cause text; each hand-written `From` impl is tested by the existing `?` sites compiling. Focused nextest on every crate touched (`harness-log`, `harness-runner`, `harness-sessions`, `harness-webfetch`, `promptforge-api-runtime`, `gateway-cloud-providers`, `gateway`, `gateway-local`, `gateway-stt`, `workshop-workspace`, `workshop-server` without `--all-features`). Apply the enforced-crate rule.

Commit: one commit.

</step-4>

<step-5>

### Step 5: `#[non_exhaustive]` on error and wire types [completed]

- Component: `error-model`
- Piece: exhaustiveness attributes (D3)
- Covers: R4, F4
- Depends on: Step 4

Changes:

- 10 public error enums: `ReplayError` (`crates/promptforge-api-types/src/replay.rs` ~104), parser `Error` (`crates/promptforge/parser/src/lib.rs` ~53), model-client `Error` (`crates/promptforge/model-client/src/error.rs` ~27), lua `Error` (`crates/promptforge/lua/src/error.rs` ~59), `CurrentModelError` (`crates/harness/sessions/src/environment.rs` ~345), `DriveError` (`crates/harness/runner/src/effect_loop.rs` ~59), `PrepareError` (`prepare.rs` ~113), `LogError` (`crates/harness/log/src/error.rs` ~9), `DialectResolveError` (`crates/gateway/local/src/dialect.rs` ~40), `FetchError` (`crates/gateway/cloud-providers/src/lib.rs` ~94). `WaitError` and `FailureKind` stay exhaustive.
- ~17 serde wire enums: `gateway-api` `EnvRole` / `Tier` / `SliceStatus`; `gateway/protocol` `EmbeddingInput` / `SpeechVoice` / `SpeechResponseFormat` / `SpeechStreamFormat`; `gateway/config` `LlamaBackend`; `harness/sessions` `DeltaKind`; `workshop/protocol` `InputFrame` / `AgentEventKind` / `AgentDeltaKind`; `promptforge-api-types` `TaskOrigin` / `AbandonReason`; lua `StoreOp`; the `events!` macro in `crates/promptforge-api-types/src/event.rs` so `Event` receives it.
- Wire structs the owning crate alone constructs: `gateway/protocol` `ChatResponse` / `ChatChunk` / `ChatChunkChoice` / `EmbeddingResponse` / `RerankResponse` / `ModelsResponse`; `harness/sessions` `SessionEvent` / `Delta` / `SessionFailure`; `harness/log` `StoredRecord` / `RunRow`; `workshop/protocol` `AgentEvent` / `Progress` / `StatusBarUpdate` / `CatalogPush`. Skip request-direction bags (`ChatRequest`, `EmbeddingRequest`, `SpeechRequest`, `RerankRequest`, `LaunchRequest`, `RunMeta`, `Record`, `SwitchProfileFrame`, `WorkbenchSnapshot`) unless `rg 'StructName \{'` shows no cross-crate literal, including under `tests/it/`.
- Variant-level attribute only where an enum already has it on some data variants and not on siblings.
- Add `_ =>` or `..` arms only where a downstream match now fails; each new arm gets a one-line comment naming the enum's owner.

Tests: none new; compilation plus existing tests. Focused nextest on every crate touched and every downstream crate that gained a wildcard arm.

Commit: one commit.

</step-5>

<step-6>

### Step 6: Attach discarded error causes [completed]

- Component: `error-model`
- Piece: call sites (D2b)
- Covers: R5, F5
- Depends on: Step 5

Changes: of the 108 `.map_err(|_|` sites, skip the 46 in `crates/promptforge/lua`. For the remaining ~62 (`gateway/stt/api` 13, `gateway/stt/whisper-ffi` 7, `gateway/local` 7, `promptforge-api-runtime` 6, `workshop/*` ~10, remainder found by `rg '\.map_err\(\|_\|' crates`), attach the discarded error as `#[source]` where the target variant already carries a source or can take one without adding a variant. Leave sites whose discarded error carries no information (integer conversions, `TryFrom` on constants, `Option`-like probes). Workers split by crate; each reports the sites it left and why, and the step's return lists them.

Tests: none new; compilation plus existing tests. Focused nextest per crate touched.

Commit: one commit.

</step-6>

<step-7>

### Step 7: Retire `mod.rs` files [completed]

- Component: `layout-lint-hygiene`
- Piece: module layout moves (D4a)
- Covers: R7, F7
- Depends on: Step 6

Pure `git mv` plus `mod` and `#[path]` fixes; no content edits.

- Targets: production `crates/promptforge/lua/src/tools/mod.rs`, `crates/promptforge/lua/src/models/mod.rs`, `crates/promptforge-api-runtime/src/fanout/mod.rs`, `crates/gateway/cloud-providers/src/providers/mod.rs`, `crates/gateway/stt/api/src/realtime/mod.rs`; test `tests/mod.rs` under `promptforge-api-runtime/src/execute`, `promptforge-api-runtime/src/lua`, `promptforge-api-runtime/src/model`, `harness/models/src/transport`, `promptforge/lua/src/protocol`, `gateway/app/src/cloud_models`, `gateway/config/src/config`. Confirm the list with `rg --files -g mod.rs crates`; keep `gateway/stt/api/tests/common/mod.rs` and `workshop/server/tests/common/mod.rs`.
- Form per directory (count siblings first): three or more sibling files after `mod.rs` leaves -> `foo.rs` + `foo/`; fewer -> flatten to `foo.rs` + `foo-bar.rs` with `#[path]`.
- Fix any existing `#[path]` attribute in the moved parent that points into the directory.

Tests: none new. `rg --files -g mod.rs crates` lists exactly the two `tests/common/mod.rs` files. Focused nextest per crate touched.

Commit: one commit.

</step-7>

<step-8>

### Step 8: Lint suppressions, test plumbing, doc examples [completed]

- Component: `layout-lint-hygiene`
- Piece: lint hygiene (D4b)
- Covers: R6 (with Step 2's re-export move), F6
- Depends on: Step 7

Changes:

- 6 `#[allow]` without reason -> `#[expect(lint, reason = "...")]`: `crates/harness/runner/src/spawn.rs` ~31/~58/~79 (`clippy::disallowed_methods`), `crates/gateway-api/src/lib.rs` ~119 (`clippy::struct_excessive_bools`), `crates/gateway/cloud-providers/src/providers/cohere.rs` ~120 (cast lints), `bedrock-sigv4.rs` ~35 (`clippy::too_many_arguments`). Confirm `build-xtask`'s `harness_bans` does not grep the literal `allow`.
- 3 test-only re-exports into their consuming test modules: `crates/promptforge-api-runtime/src/store.rs` ~24 `StoreExt`, `lua.rs` ~14 `ToolOutputKind`, `execute/scheduler.rs` ~100 `TaskState`.
- 2 doc examples `.unwrap()` -> `?` with a hidden `Ok(())` tail: `crates/gateway/app/src/api_error.rs` ~24, `runner.rs` ~828.
- 5 ` ```text ` fences holding Rust in `crates/promptforge/lua/src/vm.rs` ~51/~231/~433/~783/~996: compiled doctests (`no_run` plus hidden setup) where the API is reachable from the crate root; otherwise stay `text` with a one-line reason comment.
- `crates/workshop/user-state/src/store.rs`: rename `get_all` -> `all` (the two `get_state` axum GET handlers keep their names).
- `#[must_use]` on `Self`-returning constructors lacking it, where the type is not already `#[must_use]`: `crates/shared-vfs/src/handle.rs` ~181/~189, `crates/workshop/menu/src/menu.rs` ~184, `crates/workshop/registry/src/traits.rs` (7 sites), `crates/build-llama-cuda/src/probe.rs` ~26.
- `crates/gateway/app/src/model_info.rs`: remove the doubled `#[cfg(feature = "local")]` from `d77a8b48` if still present.

Tests: none new. `rg '#\[allow\(' crates` shows only attributes carrying `reason`. Focused nextest per crate touched plus `cargo test -p gateway --doc` and `cargo test -p promptforge-lua --doc`.

Commit: one commit.

</step-8>

<step-9>

### Step 9: Parser facade root and junk-drawer renames

- Component: `layout-lint-hygiene`
- Piece: facade and names (D6)
- Covers: R9, F9
- Depends on: Step 8

Changes:

- `crates/promptforge/parser/src/lib.rs` (677 lines, 31 fns, 8 impls): read `tests.rs` (1317 lines, reaches private items through `use super::*`) first, then move the logic into sibling modules named by concept (`build.rs` exists; add siblings), leaving `lib.rs` as crate docs, attributes, `mod`, and `pub use` under 500 lines. Moved items gain `pub(crate)` and `tests.rs` imports are updated, or the affected tests move beside their module as `<module>-tests.rs`. The `Error` enum attributed in Step 5 moves with its module.
- `crates/shared-loopback/src/lib.rs` (35 fns): same treatment if over 500 lines; otherwise defer and note it in the return.
- Rename `crates/shared-vfs/src/types.rs` and `crates/gateway/stt/api/src/realtime/wire/shared.rs` to the concept they hold (read contents first); fix `mod` lines and `#[path]` attributes.
- Any enforced-crate file grown past 500 lines by Steps 1-8 that the enforced-crate rule missed: split it here.

Tests: none new; compilation plus existing tests. Focused: `cargo nextest run --locked -p promptforge-parser -p shared-loopback -p shared-vfs -p gateway-stt --all-features`.

Commit: one commit.

</step-9>

<step-10>

### Step 10: Paused time in deterministic async tests

- Component: `paused-time`
- Piece: in-process tests and the workspace clock (D8)
- Covers: R11, F11
- Depends on: Step 9

Read each test before converting: `start_paused` turns "nothing arrives within 50ms" into "time jumps until something arrives".

- 5 sentinel `tokio::time::sleep(30s)` race arms -> `std::future::pending()`: `crates/harness/models/src/transport/tests/limits.rs` ~112/~149 (path as moved in Step 7), `crates/harness/web-search/src/web_search-tests.rs` ~395, `crates/promptforge-api-runtime/src/execute/tests/mod.rs` ~1330 (as moved), `execute/tests/scheduler.rs` ~3721.
- `promptforge-api-runtime` (~9): `#[tokio::test(flavor = "current_thread", start_paused = true)]` following the 17 precedents in `timeouts.rs` and `input.rs`. Fixed delays `tool_loop.rs` ~482, `input.rs` ~316; 10ms poll loops `scheduler.rs` ~150/~3412/~3751, `live_infer.rs` ~314; fake-latency `sleep(delay)` in `model_task_notices.rs` ~48 and `tests/mod.rs` ~980. `test_support/tokio_driver.rs` ~357 is the timer performer and stays. Where a test asserts a slow-vs-fast race, replace the sleep with an explicit `tokio::time::advance`.
- Harness and workshop in-process (~5): `crates/harness/runner/tests/it/effect_loop.rs` ~205, `performers.rs` ~53; `crates/workshop/gateway/src/gateway_progress-tests.rs` ~129/~140; `crates/workshop/workspace/src/workspace-file-tests-mutations.rs` ~417.
- Clock injection: give `Workspace` a `now` function (defaulting to `now_rfc3339`) that `GrantMeta::added_at` reads in `workspace-backing.rs` `grant_and_persist`; rewrite `wait_past` in `crates/workshop/workspace/src/workspace-tests-grants.rs` to set two distinct stamps directly instead of spinning on the wall clock.

Out of scope: the ~45 real-process or loopback-socket sleeps and the four blocking-pool fakes listed in Non-goals stay on real time.

Tests: the converted tests are the tests; each passes under `start_paused`, with explicit `advance` where a race is asserted. Focused: `cargo nextest run --locked -p promptforge-api-runtime -p harness-models -p harness-web-search -p harness-runner -p workshop-gateway -p workshop-workspace --all-features`. Apply the enforced-crate rule.

Commit: one commit (production clock change ships with the rewritten test).

</step-10>

<step-11>

### Step 11: Documentation prose - gateway family

- Component: `docs-prose`
- Piece: gateway crates (D7, group 1)
- Covers: R10, F10 (gateway share)
- Depends on: Step 10

Docs-only, no code motion; workers parallel by crate over `crates/gateway/**`, `crates/gateway-api`, `crates/gateway-api-discovery`.

- Imperative summary lines -> third-person indicative (`Spawn` -> `Spawns`), verb form only, meaning untouched: `gateway/cloud-providers` 69, `gateway/app` 47, `gateway/protocol` 25, plus the remainder in the family found by `rg '^\s*///\s+[A-Z][a-z]+ ' crates/gateway`.
- `//!` first line on the 37 `gateway/stt` production files lacking one and on every gateway-family test module file lacking one; one sentence each.

Tests: none new. `cargo doc -p <crate> --no-deps` with `-D warnings` on each crate touched, plus focused nextest to confirm no doctest fence changed meaning.

Commit: one commit.

</step-11>

<step-12>

### Step 12: Documentation prose - promptforge and harness families

- Component: `docs-prose`
- Piece: promptforge and harness crates (D7, group 2)
- Covers: R10, F10 (promptforge and harness share)
- Depends on: Step 11

Docs-only; workers parallel by crate over `crates/promptforge/**`, `crates/promptforge-api-runtime`, `crates/promptforge-api-types`, `crates/harness/**`, `crates/harness-api`.

- Imperative summary lines -> third-person indicative across both families.
- `//!` first line on the 7 `promptforge/lua` production files lacking one, on any other production file in these families lacking one, and on every test module file (`*-tests.rs`, `tests/*.rs`, `tests/it/*.rs`) lacking one.

Tests: none new. `cargo doc -p <crate> --no-deps` with `-D warnings` on each crate touched, plus focused nextest.

Commit: one commit.

</step-12>

<step-13>

### Step 13: Documentation prose - workshop, shared, build; full verification

- Component: `docs-prose`
- Piece: remaining crates (D7, group 3) and the F12 gate
- Covers: R10, F10 (remaining share), F12
- Depends on: Step 12

Docs-only; workers parallel by crate over `crates/workshop/**`, `crates/shared-*`, `crates/build-*`, `crates/workspace-hack` (if any doc lines).

- Imperative summary lines -> third-person indicative (`build-xtask` 25 and the rest).
- `//!` first line on every production and test module file in these crates lacking one.
- `crates/workshop/shell/src/main.rs`: move the `//!` line above the leading `//` release-build comment.

After the docs land, run the full verification block from Technical Design in order: `cargo fmt --all --check`; both clippy partitions with `-D warnings`; `$env:RUSTDOCFLAGS = "-D warnings"; cargo doc ...`; both nextest partitions; the workspace doctest pass; `cargo test -p build-xtask`. Any failure is fixed inside this step's commit before it lands, and the fix is named in the return. Verify dispatch runs on this step with Scope `FULL`.

Tests: none new beyond the verification block. The 254 summary lines, 48 production files, and 105 test files from Technical Design D7 are all covered across Steps 11-13; re-run the D7 counting greps at the end and report any residue.

Commit: one commit.

</step-13>

</execution-plan>
