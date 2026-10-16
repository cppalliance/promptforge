---
name: Rulebook fix commits
overview: Fix the Rust rulebook findings in the seven Engine crates (runvil facade plus engine, lua, model-client, parser, types, vfs) in 11 commits, one per audited slice of code, in crate dependency order. The internal-crate test harnesses plan, which took the audit's tokio, determinism, and test-setup findings, closed at b9126c55d, and every step here was re-checked against the repository at 852bb51b3. Every commit leaves the whole workspace building.
todos:
  - id: step-00
    content: "Step 0: after the harness plan's close commit, re-check every symbol, path, count, and recipe against the code; drop items whose code that plan removed; point type-change callers at its utilities (no commit; done 2026-10-15 against 852bb51b3)"
    status: completed
  - id: step-01
    content: "Step 1: types and model-client (Role enum, Charset kind, non_exhaustive + constructors, FromStr, derives, catalog by value)"
    status: completed
  - id: step-02
    content: "Step 2: vfs (typed refusals, io cause, exhaustive is_mutation, case-insensitive claims, Stat/Entry constructors, splits)"
    status: completed
  - id: step-03
    content: "Step 3: lua core A and the lua error model (error-chain convention, Internal routing, QuotaResource, lib.rs cleanup)"
    status: completed
  - id: step-04
    content: "Step 4: lua core B (quota by downcast, TaskPhase/Blocked/SpawnVia, SectionVm encapsulation, key formatter)"
    status: completed
  - id: step-05
    content: "Step 5: parser and facade (ParseErrorKind::Internal, span Range, named iterators, module renames)"
    status: completed
  - id: step-06
    content: "Step 6: engine core and typed Duration timer (infer kind, error-chain, facade non_exhaustive, AnswerRecord)"
    status: completed
  - id: step-07
    content: "Step 7: engine scheduler (ChainKind, ChainStep, TaskBuiltin, CallDepthExceeded, kept causes, exhaustive matches)"
    status: completed
  - id: step-08
    content: "Step 8: engine test support (Observer argument struct, track_caller and Panics docs, non_exhaustive fixes, doc summaries)"
    status: completed
  - id: step-09
    content: "Step 9: engine execute tests (failure output, error-kind assertions, live_infer copies, explicit imports, lows)"
    status: completed
  - id: step-10
    content: "Step 10: lua tests (raw-event payload tests, invariant expect messages, lows)"
    status: completed
  - id: step-11
    content: "Step 11: workspace (MSRV + CI job, pulldown-cmark features, new lints and fallout)"
    status: completed
isProject: false
---

# Rust Rulebook Fixes for the Engine Crates

<product-contract>

## Product Requirements

An audit of the seven Engine crates against the Rust rulebook found about 110 problems: 1 high, 28 medium, 81 low. Lints, suppressions, and `unsafe` hygiene were already good. The remaining problems are mostly repeated patterns: errors that misclassify or blame the wrong party, catch-all arms, stringly typed states, causes printed twice or dropped, missing `#[non_exhaustive]`, timing-dependent tests, and copied test setup. The internal-crate test harnesses plan ran first and fixed the timing-dependent tests and copied test setup, along with the other findings listed under "Plan order and handoff" in the Decision Record. This plan fixes the rest in 11 commits, one per audited slice of code. The commits are ordered so that each one builds, and each step carries enough detail for a fresh agent to execute it alone.

- Problem and users:
  - Hosts and models act on error kinds and messages that are sometimes wrong. For example, a backend protocol violation is reported as a non-retryable Lua author error.
  - Maintainers face facade types that can't grow without breaking callers. Engine tests that could flip on a slow machine were fixed by the harness plan, which now drives every engine test serially.
- Goals:
  - Every item in every step's work list is done.
  - The facade surface becomes extensible where intended, and closed on purpose where documented.
  - The test suites stay deterministic: no change here adds a sleep, a wall-clock read, a timed wait, or unseeded randomness, which the harness plan's guards also reject.
- Non-goals:
  - Rulebook rules the repo deliberately overrides, listed under Decision Record.
  - The language guide in the separate `runvil-docs` repository.
- Success criteria:
  - Each step's "Done when" checks pass.
  - Each commit passes the checks in Dependencies and verification.
  - Step 11 passes the full exit criteria.
- Constraints:
  - Baseline: the runvil repository (`c:\Users\Vinnie\cursor\runvil`) at `852bb51b3` on `master`. The internal-crate test harnesses plan (repository copy `vibe/2026-10-14-4-internal-test-harnesses.md`) closed at `b9126c55d`. Step 0's re-consolidation was done against `852bb51b3` on 2026-10-15, and "Re-consolidation record" in the Decision Record lists what it changed. Every `~Lnn` line hint is current at that commit. Earlier plans this one builds on:
    - The no-trace plan (`vibe/2026-10-14-1-no-trace-fence.md`) rewrote the parser's fence handling and the facade test suite into a shared harness, removed the retired-symbol lint, and added `runvil::json`.
    - The facade-suite-dedup plan (`vibe/2026-10-14-2-facade-suite-dedup.md`) made `tempfile` a dev-dependency of the facade, engine, and vfs, and loosened the vfs zero-dependency rule to allow dev-dependencies. The harness plan later replaced `tempfile` in test code with a std-only `TempRoot`, so vfs has no dev-dependencies at `852bb51b3`.
    - The sidecar-strict-fences plan (`vibe/2026-10-14-3-sidecar-strict-fences.md`) made every non-exact Lua fence a parse error.
    - The harness plan removed tokio and `async-trait` from the engine, drove every engine test with the serial driver, moved each internal crate's test setup into shared utilities (listed under Technical Design), widened the run seed to 256 bits (`RunSeed`, `RunUid`, `RunContext::uid`), and added dependency-graph and clippy determinism guards for the Engine crates.
    - The harness debt-removal plan (`vibe/2026-10-15-1-harness-debt-removal.md`, closed at `852bb51b3`) made these changes:
      - `ScriptedChat::complete_now` returns a `Result`, and the engine `Seeded::pick` is gone.
      - The `Scenario::cancel_when` doc, the lua `vm_tally` comment, and the `RandomState::new` ban reason in the three Engine `clippy.toml` files were rewritten.
      - Every happens-before sweep run gets its own `TestStore`.
      - The run log parses its seed through `RunSeed`'s serde form.
      - The Engine graph guard runs `cargo tree --target all`.
  - No backward compatibility: signatures and record formats change outright.
  - Run with the vibe coder (`../tools-public/planning/vibe-coder.md` from the repository root), with `rulebook-fixes` as the plan's vibe name words, so the close commit is `Close plan: rulebook-fixes`.
  - Before the run, confirm that `master` is at `852bb51b3`, or at later commits that touch only `vibe/`, with a clean tree and no `vibe/ACTIVE`. If `master` has moved further, symbols and search recipes stay authoritative, but re-check each `~Lnn` hint and recorded count against the code before relying on it.
  - The engineering rules every commit obeys are under Technical Design, "Rules every step follows", because the run's coding, review, and fix agents read Technical Design but not this section.
- Open questions: None

## Functional Specification

Callers see corrected error kinds and messages, a facade that grows without breaking them, typed timer durations, and a deterministic test suite. Run-log records keep typed outcomes.

- Actors and workflows: Hosts drive runs through the `runvil` facade, models read error messages, and maintainers run the suites.
- Inputs and outputs:
  - `Effect::Timer` carries a `std::time::Duration`, while its record keeps a numeric seconds wire form.
  - `Message::role` is a closed `Role` enum that serializes to the same lowercase strings.
  - `AnswerRecord` keeps outcome kinds and the trust level.
- States and validation:
  - Timer durations are validated once, where they enter.
  - Facade types other than `Effect`, `EffectAnswer`, and `Step` are `#[non_exhaustive]`, and structs get constructors. The three closed enums are documented as closed so that a new effect kind breaks every caller's build.
- Errors and recovery: the new variants and kinds are listed under Shared interfaces. Every error prints its cause exactly once, and quota errors are recognized by type, never by message text.
- Security and privacy behavior:
  - The vfs mode gate matches `Op` exhaustively.
  - Rooted-mount claims compare paths case-insensitively on Windows and macOS.
  - Non-UTF-8 real file names are refused with a typed error.
  - The reserved device-name check is extended.
- Acceptance criteria:
  - Every step's work list and "Done when" checks are complete.
  - The facade listing records the intended surface.

</product-contract>
<implementation-contract>

## Technical Design

A few workspace-wide conventions and shared types keep the 11 commits consistent. The crate that defines a type owns every finding about it and updates all its callers. A type that one step introduces and a later step uses is defined exactly as below, so independent agents agree.

- Rules every step follows. The run's coding, review, and fix agents read only this Technical Design, the Project Survey, and their own step, so every rule they need is here:
  - Run every command from the repository root, `c:\Users\Vinnie\cursor\runvil`, in PowerShell. Run cargo commands one at a time, because they share the target directory's lock, and never append `2>&1`. Judge a cargo command by its result lines and `$LASTEXITCODE`.
  - Before editing a crate, read the root `AGENTS.md` and the nearest `AGENTS.md` above the crate. The engine, lua, types, and vfs crates have their own, and so do `crates/workshop` and `crates/workshop/server`.
  - Engine crate text, the facade included, says "the caller". It never names the Host, a Host application, or a Harness crate or type, in prose, strings, or code names. `node --test crates/workshop/ui/test/docs-claims.mjs` rejects them.
  - A manifest change that alters `Cargo.lock` commits the refreshed lock. `--locked` commands refuse a stale lock until a cargo command without `--locked`, such as the clippy gate, rewrites it.
  - Every commit builds the whole workspace and passes both clippy commands in the Project Survey with warnings denied. A changed signature updates every caller in the workspace (Harness, Workshop, gateway, plugins, tests) in the same commit.
  - No backward compatibility: signatures and record formats change outright, with no shims.
  - vfs: add, never change, existing public signatures (`crates/runvil-internal/vfs/AGENTS.md`), and keep the library free of normal and build dependencies (`crates/runvil-internal/vfs/src/lib.rs` Invariants). Dev-dependencies are allowed.
  - The lua and parser internal `Error` enums and the lua protocol enums stay exhaustive (`crates/runvil-internal/lua/src/error.rs`, `crates/runvil-internal/parser/src/error.rs`, `crates/runvil-internal/lua/src/protocol.rs`).
  - Respect the per-file line ceiling (`build_ceiling::check` in each `build.rs`) and the sibling-file and directory rules in the root `AGENTS.md` Structural Rules. Split a file rather than exceed the ceiling.
  - A file leaves the repository by moving to the workspace's `cabinet/_trash/` (`../cabinet/_trash/` from the repository root), never by deletion.
  - Find each item by its path and symbol. A `~Lnn` line hint is current at `852bb51b3` and drifts after earlier steps. Fix every hit a search recipe finds, not just the examples.
  - Search recipes run under Windows PowerShell 5.1, which strips double quotes from native-command arguments, so a recipe writes a literal double quote as `\x22`.
  - A search that must return nothing passes only when it prints nothing. `rg` exits 1 when it finds nothing and 0 when it finds something, so judge such a search by its output, never by its exit code.
  - New and changed tests add no sleep, wall-clock read, timed wait, or unseeded randomness. The harness plan's guards reject them.
  - Don't fix an item another step owns. Report a new problem outside your step in your return instead.
- Naming rules for new public items, settled so that no step stops to ask:
  - A new `FromStr` reuses the type's existing parse error when it has one, as `GlobalName`, `PluginId`, and `ToolId` do. Otherwise its error is a public `Parse<Type>Error` struct holding the rejected text, whose `Display` lists the accepted values built from the variants: `ParseRoleError`, `ParseTaskOriginError`, `ParsePathReasonError`, `ParseErrorKindError`, and `ParseOverflowReasonError`. vfs writes `Display` and `Error` for its own by hand.
  - A struct that becomes `#[non_exhaustive]` gets `new`, taking the fields that have no natural default in declaration order, and a `with_<field>` setter that takes and returns `Self` for every other public field.
  - A named iterator is a public struct named for what it yields, carrying the borrow's lifetime: `ArgumentNames<'a>` for `ToolArguments::names`, `ArgsDeclIter<'a>` for `ArgsDecl::iter`, and `ModelRolesIter<'a>` for `ModelRoles::iter`.
  - Names of new items that stay off the facade and aren't fixed under Shared interfaces, such as the `ChatResult` outcome enum or the `MessageList::push` error enum, are the coder's choice. Return each as a decision.
- Architecture:
  - Error-chain convention (engine, lua, parser): `Display` holds the full self-contained message. The cause moves from `#[source]` to a plain field with a `cause()` accessor, so chain printers show it once. Each crate adds one test that walks an error chain and asserts that no segment repeats.
  - Commit ownership: a cross-crate finding lands in the step for the crate that defines the type, together with every caller it forces to change.
  - Order: types and model-client, then vfs, lua, parser and facade, engine core, scheduler, engine test support, engine tests, lua tests, and finally workspace config. Workspace config goes last because new lints flag code in every crate.
- Modules and interfaces (shared interfaces):
  - `Role` (model-client, re-exported in `runvil::model`):
    - Variants `System`, `User`, `Assistant`, `Tool`, serialized as `rename_all = "lowercase"`.
    - Has `as_str()`, and `FromStr` whose error lists the accepted roles built from the variants.
    - Replaces `Message.role: String` and lua's `MessageRole`.
  - `GlobalNameErrorKind::Charset` (types): covers uppercase, `@`, and non-ASCII. `Control` keeps only control bytes. `PluginIdErrorKind` and `ToolIdErrorKind` gain a matching `Charset`, mapped one to one.
  - `CompletionErrorKind::InvalidParts` (model-client): non-retryable. Returned when `ToolCall::from_parts` or `Completion::from_result` rejects caller-built parts.
  - `VfsError` new variants (vfs):
    - `ApprovalRequired { path, reason }`: policy `Ask`.
    - `ScopeEnded { path }`: the owning tool call or run ended.
    - `ReadOnly { path }`: a read-only mount or backend.
    - `OutsideRoot { path, detail }`: an escape from the root or a dangling link.
    - `RootProtected { path }`: removing or renaming the root.
    - `NoStore`: the access declares no store.
    - `CrossMount { path, detail }`: the operation spans mounts.
    - `Io { path, kind: std::io::ErrorKind, raw_os_error: Option<i32>, message }`.
    - The existing `NotUtf8 { path }` becomes `NotUtf8 { path, valid_up_to: usize }`, the byte offset `Utf8Error::valid_up_to` reports. This changes a variant's shape, not a method signature, so the vfs signature rule allows it. Every constructor and matcher changes in the same commit.
    - `PermissionDenied` keeps only policy `Deny`, `Unsupported` keeps only "backend lacks this operation", and `Backend` keeps only non-I/O backend diagnoses.
    - New method `VfsError::tag(&self) -> &'static str` returns a stable tag per variant. Existing variants keep the tags authors already branch on in `err.reason`: `not_found`, `already_exists`, `not_a_directory`, `is_a_directory`, `directory_not_empty`, `not_utf8`, `invalid_path`, `invalid_range`, `anchor`, `permission_denied`, `unsupported`, `conflict`, and `backend`. The new variants are `approval_required`, `scope_ended`, `read_only`, `outside_root`, `root_protected`, `no_store`, `cross_mount`, and `io`.
    - A store error value carries `path` for each new variant that has one, and nothing else, as `PermissionDenied` does today. `NotUtf8` also carries `valid_up_to` as an integer field, as `Anchor` carries `count`.
    - The engine's `raised_store_error` (`crates/runvil-internal/engine/src/error/convert.rs` ~L201) rebuilds every tag. It degrades the fields a table doesn't carry the way its existing arms do, so `Io` comes back with `kind: std::io::ErrorKind::Other` and no `raw_os_error`.
    - Enum-taking siblings, all additive. Each enum is `#[non_exhaustive]` like every facade type:
      - `Depth { One, Recursive }` for `VfsAccess::remove_with` and `VfsAccess::mkdir_with`, which are provided methods delegating to the `bool` forms so implementors keep compiling, and for `Access::remove_with` and `Access::mkdir_with`.
      - `GlobKind { FilesOnly, DirsOnly }` for `VfsAccess::glob_with`, both delegating to `glob_kind`, which keeps files when `dirs_only` is false.
      - `Writability { ReadWrite, ReadOnly }` for `RealBackend::with_writability`, beside `with_read_only`.
  - Lua protocol enums (lua):
    - `TaskPhase { Running, Done, Cancelled, Abandoned }`, with tags `running`, `done`, `cancelled`, `abandoned`. The engine's `TaskState::Delivered` reports `Done`.
    - `Blocked { Chat, ToolCall, Store, Tasks, Call, Queued }`, with tags `chat`, `tool_call`, `store`, `tasks`, `call`, `queued`. There's no timer variant, because `Request::Timer` doesn't park a chain, so docs stop listing `timer`.
    - `SpawnVia { Spawn, Fanout }` replaces `Spawn::fanout: bool`.
  - Lua quotas:
    - `QuotaResource { LogEvent, LogByte, Instruction }`, displayed as `log event`, `log byte`, `instruction`.
    - `LuaQuota { resource: QuotaResource }` is used in both lua and engine.
    - Quota refusals are raised as the typed external value `QuotaExceeded(QuotaResource)` and recognized by downcast. The `lua_quota` constants stay only as the message text.
  - `Error::CallDepthExceeded { cap: usize, via: DepthVia }` (engine):
    - `DepthVia { Call, Fanout }` renders `call` or `tasks.fanout`. A `tasks.spawn` trip maps to `Call`, as today.
    - The `Display` text stays `{via} recursion exceeded cap of {cap}`.
    - It maps to `RunErrorKind::Quota`, whose doc adds the call-depth cap.
  - `TaskBuiltin { Task, TaskCancel, TaskStatus, AwaitTasks }` (engine scheduler): has `ALL` and `name()` (`task`, `task_cancel`, `task_status`, `await_tasks`). Replaces `TASK_BUILTINS` and `RESERVED_TOOL_NAMES`.
  - `ParseErrorKind::Internal` (parser): for parser invariant breaks and Lua VM setup failures.
  - Internal errors and remaps:
    - Engine `Error::Internal` and `Error::RequirementsUnmet` each gain `cause: Option<BoxedSource>`, using the crate's existing alias for `Box<dyn std::error::Error + Send + Sync>` (`crates/runvil-internal/engine/src/error.rs` ~L18), with a `cause()` accessor. Per the error-chain convention, `Display` appends the cause's text once when there is one.
    - Step 6 adds `Error::internal_with_cause(message: &'static str, cause: impl Into<BoxedSource>)`, `#[track_caller]` like `Error::internal`, and Step 7 calls it.
    - Lua `Error::Internal` widens from `&'static str` to `Cow<'static, str>` in Step 3, because some messages it takes over name a value, such as the tool name at `crates/runvil-internal/lua/src/scope.rs` ~L52.
    - The engine's `Error::Internal` `message` field (`crates/runvil-internal/engine/src/error.rs` ~L313, `&'static str` today) widens to `Cow<'static, str>` in the same step, because the engine's conversion passes a Lua internal message through whole (`crates/runvil-internal/engine/src/error/convert.rs` ~L172, `LuaError::Internal(message) => Error::internal(message)`). `Error::internal` and `Error::internal_with_cause` keep taking `&'static str`.
  - Timer types:
    - Lua `Request::Timer` and the await timeout carry `Duration`.
    - `Effect::Timer { duration: Duration }`.
    - `EffectRecord::Timer { seconds: f64 }` is unchanged.
    - The Harness's `Timer::sleep` (`crates/harness-internal/runner/src/performers.rs`) takes `Duration` instead of `seconds: f64`.
  - `AnswerRecord`:
    - A failure records `FailureRecord { kind: String, message: String }`, with public fields, `#[non_exhaustive]`, and `new(kind, message)`. It serializes as `{"kind": .., "message": ..}`. `AnswerRecord::Chat`, `ToolCall`, and `Vfs` hold `Result<_, FailureRecord>` in place of `Result<_, String>`.
    - `kind` is `CompletionErrorKind::tag()`, `ToolErrorKind::tag()`, or `VfsError::tag()`, and `message` is the error's `Display` text. Neither kind enum has a tag today, so Step 6 adds both methods, each returning its variant's name in snake_case, such as `malformed_response` or `invalid_arguments`.
    - `ToolAnswerRecord.trusted: bool` becomes `trust: OutputTrust`. `OutputTrust` has no serde derives today, so Step 6 adds them with `rename_all = "snake_case"`, giving `trusted` and `untrusted`.
  - Every `match` on a workspace-owned enum is exhaustive. A wildcard arm is allowed only for a foreign or `#[non_exhaustive]` enum.
  - Invariant panics are either `Error::internal` returns, or `.expect(..)` under `#[expect(clippy::expect_used, reason = "..")]`.
- Shared test utilities from the harness plan. New and changed tests use these, never a new private setup helper:
  - Engine (under `crates/runvil-internal/engine/src/`):
    - The serial driver is `drive` and `perform_locally` in `test_support.rs`.
    - `Scenario` is in `test_support/scenario.rs`, and its stepping is in `test_support/scenario-driving.rs`.
    - `test_support/scenario-serial.rs` holds the serial helpers, including `only_effect` and `text_of`.
    - The scheduler-level rig is `test_support/rig.rs`.
    - The rest are `ScriptedChat` (`test_support/scripted-chat.rs`), `FnTool` (`test_support/tools.rs`), `Seeded` (`test_support/seeded.rs`), and `TempRoot` (`test_support/temp_root.rs`).
  - Facade suite: `Scenario` in `crates/runvil/tests/suite/support/scenario.rs`.
  - Other crates:
    - types: `Seeded` in `crates/runvil-internal/types/src/test_support.rs`.
    - vfs: `TestFs` in `crates/runvil-internal/vfs/src/test_support.rs`.
    - lua: `TestVm` in `crates/runvil-internal/lua/src/test_vm.rs`.
    - parser: `PromptSource`, the prompt source builder, in `crates/runvil-internal/parser/src/test_support.rs`.
- File and public API changes:
  - Each step that changes the facade surface re-blesses `crates/runvil/public-api.txt` with `cargo +nightly-2026-09-05 xtask api --bless`, then confirms it with `--check`. The nightly is pinned in `crates/build-xtask/src/api/toolchain.rs`, and the command refuses any other toolchain.
  - Every type a facade item mentions, including a `FromStr` impl's error type and a method's iterator type, must be a facade re-export, or `--bless` refuses with a closure finding (`crates/build-xtask/src/api/items.rs`, `api.rs` ~L142). Re-export each new one as a single-item `pub use` in the `crates/runvil/src/lib.rs` module that already re-exports its parent type:
    - `model`: `Role`, `ParseRoleError`, and `ArgumentNames` (Step 1).
    - `ids`: `ParseTaskOriginError` (Step 1).
    - `vfs`: `Depth`, `GlobKind`, `Writability`, and `ParsePathReasonError` (Step 2).
    - `prompt`: `ArgsDeclIter` and `ModelRolesIter` (Step 5).
    - `effect`: `FailureRecord` (Step 6).
  - Doc text on facade items names no internal crate, because the API check's doc-text rule rejects it.
- Data, persistence, failure, security, and privacy constraints:
  - The `AnswerRecord` JSON format changes, and run-log readers in the workspace are updated in the same commit.

</implementation-contract>
<verification-contract>

## Testing Plan

Each step adds or adjusts tests for its own fixes and keeps the workspace green, and Step 11 runs the whole workspace exit set. New and changed tests go through the harness plan's shared utilities and pass its determinism guards.

- Unit:
  - A chain-repeat test in the engine, lua, and parser.
  - A test for each new variant or kind under Shared interfaces.
  - The duplicate-tool message renders `web/fetch`, and non-string Lua keys render as `1`.
  - An author's `error("lua log event budget exceeded")` stays a Lua error with its mapped line.
- Integration and end-to-end:
  - Facade tests use constructors for every newly `#[non_exhaustive]` struct.
  - A cross-thread vfs claims test, plus vfs link tests that fail when a link can't be made.
- Regression, security, and performance:
  - Tests for the exhaustive `is_mutation`, case-variant conflicts on Windows and macOS rooted mounts, and refused non-UTF-8 names.
  - Payload tests assert on serialized raw events.
- Per-commit gate, run on every step. Decomposition writes these commands into every step's Tests line in PowerShell form, because Verify runs Tests-line commands as written, and at FOCUSED scope it otherwise runs no clippy, formatter, or docs command:
  - Both clippy commands in the Project Survey, with warnings denied, so a caller missed in a crate the step didn't touch still fails.
  - `cargo fmt --all --check`.
  - `node --test crates/workshop/ui/test/docs-claims.mjs`, the wording scan, which rejects Host and Harness names in Engine crate text.
  - Steps 1 to 8: `$env:RUSTDOCFLAGS='-D warnings'` as its own statement, then `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api` and `cargo doc --locked --no-deps --all-features -p runvil-engine --document-private-items`. The workspace denies `broken_intra_doc_links` and `private_intra_doc_links` (root `Cargo.toml` ~L274 to ~L276), which clippy never checks. At `852bb51b3`, doc links already point at Step 1's `model_id_from_validated` (`crates/runvil-internal/types/src/models.rs` ~L61) and Step 7's `TASK_BUILTINS` (`crates/runvil-internal/engine/src/execute/scheduler/builtins.rs` ~L160). Step 11's FULL scope runs every docs command in the Project Survey.
  - The nextest suite of every crate the step touches.
  - `cargo +nightly-2026-09-05 xtask api --check` after every step that changes non-test code, Steps 1 to 7. Steps 1, 2, 4, 5, and 6 re-bless the listing first. Steps 3 and 7 should leave it unchanged, because the engine's internal `Error` isn't on the facade.
  - The step's "Done when" searches. Each passes only when it prints nothing. `rg` exits 1 in that case, so judge these by their output, never by their exit code.
- Exit criteria:
  - `cargo nextest` passes for the workspace, including `build-xtask` with the harness plan's guards.
  - Workspace clippy passes with warnings denied, including the harness plan's determinism bans.
  - `cargo xtask tidy` and `cargo +nightly-2026-09-05 xtask api --check` pass.
  - `cargo fmt --all --check` and the wording scan `node --test crates/workshop/ui/test/docs-claims.mjs` pass.
  - Every docs command in the Project Survey passes with `RUSTDOCFLAGS='-D warnings'`, including the new `unescaped_backticks` lint.
  - The seven crates build locally at the declared `rust-version` (`cargo +<floor> build --locked` over them), and the new MSRV CI job builds them at that version.
  - Every step's "Done when" check still holds.

</verification-contract>
<decision-record>

## Decision Record

The user chose one commit per audited slice of code, the full-message error convention, closed effect enums, and inline per-step work lists. Smaller calls follow the audit's recommended fixes and are recorded as defaults.

- Decisions:
  - Eleven commits, one per audited slice. The user said: "one commit per file (11 commits)".
  - `Display` holds the full message, and the cause sits in a plain field with an accessor. The user picked "Keep the full self-contained message in Display; hold the cause in a plain field with an accessor, not #[source]".
  - `Effect`, `EffectAnswer`, and `Step` stay closed on purpose, and every other facade type becomes `#[non_exhaustive]`, including `RunResult`. The user picked "Keep them closed on purpose and document why".
  - Steps carry symbol-anchored work lists, search recipes, shared interfaces, and "Done when" checks. The user picked "Expand the plan inline".
  - No compatibility shims. The user's standing rule: "I do not want ANY backward compatibility".
  - Plan order and handoff. This plan runs after the internal-crate test harnesses plan, which closed at `b9126c55d`, and the check against `852bb51b3` found every handed-over item below done. Otherwise Steps 8 to 10 would harden and convert tokio test code that plan deletes, and deduplicate the same helpers in a different shape. Steps 1 to 7 also get cheaper afterward, because their type changes reach a few shared utilities instead of about 70 test files. The user accepted this order: "Okay, update these two plans to reflect that they will follow your advice". The harness plan's "Taken over from the rulebook-fix plan" list is the full record. In short:
    - Owned by the harness plan, and done:
      - Step 1: the seeded rewrite of `types/src/untrusted/tests.rs`, done with its `Seeded` in place of `StdRng`.
      - Step 8:
        - The `TestTool` reduction, done as `FnTool`, including the 10 `unnecessary_literal_bound` expectations.
        - `ScriptedChat` sharing with the bench, and the `test-support` docs.
        - The test-model binding constructor.
        - The `chunk`, `SectionVmSetup`, and engine `fresh_access` dedup.
      - Step 9:
        - All determinism and seed work: serial driving, the 12 seed loops printing their seed through `Seeded`, and the `store_gate.rs` backstop.
        - One definition each of `task`, `run_offline`, `TestStore`, `PARKED_CHILD`, `EXECUTION`, `Recorder`, and `task_lifecycle`, with no dead `tools` or `_tools` parameter.
        - The prompt shell, done as the parser's source builder.
      - Step 10:
        - The cancellation-test timing, and the nonce seed.
        - The lua test hub, `TestVm`, with its `fresh_access`, `lua_over`, setup-chain, and construct-inject-install dedup.
        - The duplicated fixture builders such as `tool_at` and `scoped`.
        - `h2_vm`.
    - Gone with the engine's tokio driver: the `JoinSet` panic re-raise and its test, the test-hook `cfg` cleanup and its tap and shuffle field, the `drive_tokio`, `Performer`, and `Performers` visibility, the `tokio_driver.rs` `# Panics` docs, and `#[non_exhaustive]` on `Performers` (Step 8).
    - Gone with hakari, which the remove-hakari plan (`vibe/2026-10-13-3-remove-hakari.md`) deleted: the `[final-excludes]` tokio entry and the `workspace-hack` regeneration (Step 11), and `cargo hakari verify`. Also dropped from Step 11: the minimal workspace `tokio` entry, whose reason was the engine's optional tokio.
    - Already done: `tempfile` in the engine suite's `suite/vfs.rs` (facade-suite-dedup), and the fence classification mismatch as an internal error, though still under `Structure` (sidecar-strict-fences; Step 5 moves it to `Internal`).
  - Defaults, which the user may override:
    - MSRV is set to the verified floor, at least 1.88.
    - Rooted-mount claims compare paths case-insensitively on Windows and macOS. A false conflict fails safe.
    - `CallDepthExceeded` maps to `RunErrorKind::Quota`.
    - `Blocked` gets no timer variant.
    - `Chain.blocked` switches to `Blocked` in Step 4, because the `TaskStatus` type change forces it.
    - `PluginIdErrorKind` and `ToolIdErrorKind` each gain a `Charset` kind beside `GlobalNameErrorKind::Charset`, mapped one to one, so the same misclassification doesn't survive in the id kinds.
    - The Harness's `Timer::sleep` takes `Duration`, matching `Effect::Timer`, and its implementations change in Step 6.
    - `VfsError::NotUtf8` gains `valid_up_to: usize` in Step 2.
    - `DebugMode` and `LibraryLoad` are enums, so Step 1 gives them `#[non_exhaustive]` and no constructors.
    - Settled in the run-readiness review on 2026-10-15, each because a run would otherwise stop on an unsettled public or persisted choice:
      - The new `VfsError` tags, the store error-value fields of the new variants, and how `raised_store_error` degrades them (Shared interfaces).
      - The vfs siblings' enums and names: `Depth`, `GlobKind`, `Writability`, `remove_with`, `mkdir_with`, `glob_with`, and `with_writability`.
      - `FailureRecord`, the snake_case `tag()` on `CompletionErrorKind` and `ToolErrorKind`, and `OutputTrust`'s snake_case serde form.
      - Lua `Error::Internal` widens to `Cow<'static, str>`.
      - Engine `Error::Internal` and `RequirementsUnmet` hold `Option<BoxedSource>`, and `Error::internal_with_cause` builds the first.
      - The naming rules for new `FromStr` errors, constructors, and named iterators (Technical Design).
      - `From<ModelSetError>` maps to `Internal` in lua and the engine. `ModelSetError` is a unit struct for a poisoned lock, so there's no cause to keep, and a poisoned lock is Engine misuse.
      - `MessageList::records` becomes `to_records`.
      - `crates/runvil-internal/engine/src/tools-tests.rs` retires with the re-exports it tests.
    - Settled in the second run-readiness pass on 2026-10-15:
      - The engine's `Error::Internal` message widens to `Cow<'static, str>` in Step 3, beside lua's, so no Lua internal message loses its text.
      - The facade homes of the ten new facade-reachable types (Technical Design, File and public API changes).
      - `ModelSet` keeps `from_parts` as its constructor and gains `push` and `set_default_alias` for its two in-place writers.
      - Names of new items that stay off the facade are the coder's choice.
      - The vibe name words are `rulebook-fixes`.
    - Settled during Step 2 on 2026-10-15: `GlobKind` is `{ FilesOnly, DirsOnly }`, not `{ Any, DirsOnly }`. `glob_kind(pattern, false)` keeps files only, so an `Any` variant couldn't delegate to it, and `glob` already returns every match. The user chose the exact two-case replacement for the `bool`.
- Rejected alternatives:
  - Commits grouped by theme: the user chose one commit per slice.
  - The `#[source]`-chain convention: it doesn't fit self-contained model-facing messages.
  - `#[non_exhaustive]` on the effect enums: Hosts must handle every effect.
  - Keeping per-finding detail in the audit files: the plan must stand alone.
  - Running this plan before the harness plan: Steps 8 to 10 would be deleted or redone there.
  - Running only Steps 1 to 7 before the harness plan: every signature change would edit the tokio test files that plan is about to rewrite.
- Assumptions, risks, and notes:
  - Risk: Step 6 is the largest commit, because the `Duration` timer touches lua, the engine, the Harness, Workshop, and the facade together. Inside the engine, its test-side performers are now the serial driver (`perform_locally` in `test_support.rs`) and the harness plan's `Scenario` (`test_support/scenario-driving.rs`).
  - Note: Steps 3, 4, and 6 are the largest, and the user kept 11 commits on 2026-10-15. The internal-test-harnesses run landed 18 steps that each changed 18 to 74 files and about 1,000 to 7,600 lines, every one within one fix round, so these fit. They bundle more separate items, which costs review attention rather than risking a stall. If one does stall at the vibe coder's effort limit, split it then.
  - Risk: Step 7's scheduler state types reach the harness plan's scheduler-level test rig, `crates/runvil-internal/engine/src/test_support/rig.rs`. Update the rig in the same commit.
  - Risk: new variants can push files past the line ceiling, so split files as the Structural Rules allow. The ceiling counts test files too. These files were already near the 500-line ceiling at `852bb51b3`; split one before growing it:
    - types: `event-tests.rs` (478).
    - vfs: `router.rs` (493) and `memory.rs` (462).
    - lua: `coro.rs` (486), `error-value.rs` (469), `protocol/parse.rs` (459), and `protocol/request.rs` (452), plus the test files `tests/require.rs` (497), `projection-tests.rs` (475), and `tests/section_vm.rs` (466).
    - engine: `execute/scheduler/tool_call.rs` (472), `execute/config.rs` (468), `test_support/scenario-driving.rs` (492), and `test_support/scenario.rs` (456), plus the test files `execute/run/tests.rs` (490) and `execute/tests/happens_before.rs` (475).
    - Harness: `crates/harness-internal/runner/src/effect_loop.rs` (499).
    - Workshop: `crates/workshop/server/src/agents/wire-tests.rs` (494), if Step 5 adds a wire-code test there.
  - Note: line numbers written as `~Lnn` are current at `852bb51b3` and drift after earlier steps. Symbols and search recipes are authoritative.
  - Note: Step 0 re-checked the test-file paths, helper names, and counts in Steps 8 to 10 against `852bb51b3`, after the harness plan moved test setup into shared utilities and split files.
- Re-consolidation record. Step 0 was done on 2026-10-15 against `852bb51b3`, and each step's text below already reflects it. Path prefixes such as `V/`, `P/`, `E/`, and `X/` are the ones the steps define.
  - Dropped as already done:
    - Step 1: the seeded rewrite of `types/src/untrusted/tests.rs`.
    - Step 2: `MemoryAccess::rename` already moves the file, `V/handle.rs` already uses `VfsPath::root()`, and `Origin` is already `#[non_exhaustive]`.
    - Step 3: the "the the" typo in `dispatch.rs`.
    - Step 5: the `kind()` assertions in `P/tests/frontmatter.rs` and `P/tests/headings.rs`.
    - Step 8: `#[track_caller]` on `only_effect` and on the `ScriptedChat` helpers, and `# Panics` on `recording/forward.rs`.
    - Step 9: the `Arc::clone(&recorder)` item (no `recorder.clone()` site remains), and the `live_infer.rs` label.
  - Dropped as gone:
    - `TestToolTable` and `E/test_support/fixture.rs`, deleted with the tokio driver in `0e5614a6a`.
    - `pub(super) use self::tools::*;` in `X/context.rs`.
  - Settled under Defaults, because a fresh run would otherwise stop on them as unsettled public-interface choices: `Charset` in the plugin and tool id kinds, `Timer::sleep` taking `Duration`, and the `NotUtf8` offset field.
  - Corrected:
    - Step 1: `DebugMode` and `LibraryLoad` are enums, and the allocation behind `ToolId::plugin()` is now in `GlobalName::first`.
    - Step 4: the lua test hub is `lua/src/test_vm.rs`.
    - Step 6: the `Effect::Timer` performer list.
    - Step 7: the `drive.rs` `unreachable!`.
    - Steps 8 and 9: helpers that moved into `test_support/scenario-serial.rs`.
    - Everywhere: the API listing command, and every recipe count and `~Lnn` hint.
  - Note: the facade suite and example are the versions rewritten by the no-trace plan and trimmed by facade-suite-dedup.
- Run-readiness review. Done on 2026-10-15 against `852bb51b3`, still the `master` HEAD with a clean tree. It checked the plan against the vibe coder's contract and data flow, and changed:
  - The rules every step follows moved from Product Requirements and Execution Instructions into Technical Design, and the per-commit gate moved into the Testing Plan, because the run's agents read only those sections and their own step.
  - "Note any new problem in the commit body" became "report it in your return", because the commit body is written from the diff alone.
  - The `render_status` item moved from Step 7 to Step 4. Step 4's `TaskPhase` change breaks its `status.state == "done"` comparison, so Step 4 must fix it.
  - Step 6's and Step 7's scope lines now name the files outside their slice that they must change.
  - Step 1's role recipe and Step 10's `expect` recipe use `\x22`. As typed, the role recipe returned 4,939 hits under Windows PowerShell 5.1 instead of 153.
  - `ModelSet` joined Step 1's struct-literal recipe, the Harness JSON fixture joined Step 6's record recipe, and Step 6 now quotes the no-store message's current text.
  - The choices listed under Defaults were settled.
  - Checked and left alone: the Harness matches on `VfsOutcome` already have catch-all arms, Step 11's lint fallout is small (3 `Formatter` signatures without `<'_>`, no `todo!`), and the pinned nightly is installed.
- Second run-readiness pass. Done on 2026-10-15 against `852bb51b3`, still the `master` HEAD with a clean tree and no `vibe/ACTIVE`. It changed:
  - Step 3 widens the engine's `Error::Internal` message with lua's. As written, the engine's `&'static str` field couldn't take a widened Lua message at `error/convert.rs` ~L172, so Step 3 wouldn't compile.
  - The facade homes of ten new types are named, because `xtask api --bless` refuses while a facade item mentions a type the facade doesn't re-export.
  - The per-commit gate gained the wording scan and, for Steps 1 to 8, two docs builds. Clippy checks neither doc links nor wording, so a break would first show at Step 11's FULL scope, in the wrong commit.
  - Step 6's "breaks every Host" became "breaks every caller's build", because the wording scan rejects "host" in Engine crate text.
  - Step 1's `ModelSet` items stopped contradicting each other.
  - Done-when searches are judged by output. In the internal-test-harnesses run, Verify passed a search that printed 5 hits.
  - Technical Design gained the repository path, the PowerShell cargo rules, the `AGENTS.md` rule, the caller-wording rule, the `Cargo.lock` rule, and the off-facade naming rule.
  - Step 11 names `pulldown-cmark`'s features (none), how to find `unescaped_backticks` fallout, and the local MSRV build. Steps 2, 6, and 9 got a test hint and corrected counts.
  - The exit criteria gained the formatter, the wording scan, every docs command, and the local MSRV build. Steps 1, 2, 3, 5, and 6 gained Done-when checks for their new items, and the Step 3 and Step 6 scope lines name the extra files those items touch.
  - Checked and left alone: every recipe count matches or is off by one, the Lua-visible task and `err.reason` strings already match the new tags, no test outside a step's own crates breaks at runtime, `cargo test -p build-xtask` isn't affected, and the API listing carries no doc text, so Step 7 needs no re-bless.
- Run start. On 2026-10-15 the user kept the eleven steps as written and asked for no new steps, so the run skipped re-decomposition. It only wrapped each step in its tag pair and gave each a Component line and a Tests line that writes out the per-commit gate. The components follow the audited crates and keep their steps contiguous: `types-model-client` (Step 1), `vfs` (Step 2), `lua-core` (Steps 3 and 4), `parser-facade` (Step 5), `engine` (Steps 6 to 9), `lua-tests` (Step 10), and `workspace` (Step 11).

### Deferred and Out of Scope

- Deferred: a privileged scheduled CI job that runs the Windows file-link tests. Revisit once a runner with the symlink privilege exists.
- Out of scope: rulebook rules the repo overrides on purpose:
  - the sans-IO rule as `crates/build-xtask/src/engine_deps.rs` enforces it, which the harness plan extends to dev-dependencies and the resolved graph
  - the doctest ban
  - kebab sibling test files
  - the build-time line ceiling
  - `unsafe_code = "deny"` with an allowlist
  - denied `pedantic`
  - denied `unwrap_used` and `expect_used` in library code
  - the `doc(hidden)` ban and `detail` modules
  - model-facing sentence messages
  - exhaustive internal error and protocol enums
  - no `serde` feature gate
  - serde types allowlisted on the facade
  - a vfs library with no normal or build dependencies
  - the vfs mode gate in `lib.rs`
  - frozen vfs signatures
  - `Arc` under mlua `send`
  - a std-only `Timestamp`
  - a floating `stable` toolchain
  - container directories
  - written exact pins
  - one facade test binary
- Out of scope: the `runvil-docs` language guide.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build --locked -p gateway` (the workspace's only default member). Crate build scripts bundle the UIs with esbuild, so run `npm ci --prefix crates/workshop` and `npm ci --prefix crates/gateway/config-ui/ui` first, as CI does. The desktop app is a separate, explicit build: `cargo build --locked -p workshop`, or `cargo workshop` (the `.cargo/config.toml` alias for `run -p build-workshop --`).
- Focused test command pattern: `cargo nextest run --locked -p <package> --all-features <test-name-filter>`, or `-E 'test(<name>)'` for a nextest filterset; drop `--all-features` when the package is `workshop`, `workshop-server`, or `workshop-server-api`. Integration test binaries are named `it` in most crates and `suite` in `runvil` and `harness`, so `-E 'binary(suite) & test(<name>)'` narrows to one. The two criterion benches run each benchmark once with `cargo bench --locked -p runvil-engine --features test-support --bench models_loop -- --test` and `cargo bench --locked -p runvil-lua --features test-support --bench surface -- --test`.
- Component test command pattern: `cargo nextest run --locked -p <package> --all-features` (again without `--all-features` for `workshop`, `workshop-server`, and `workshop-server-api`).
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`. CI also runs `npm test --workspaces --if-present` in `crates/workshop`, `npm test` in `crates/gateway/config-ui/ui`, and, on the pinned nightly, `cargo nextest run --locked -p build-xtask --run-ignored only`.
- Linter command: `CARGO_BUILD_WARNINGS=deny cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features` and `CARGO_BUILD_WARNINGS=deny cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`. In Windows PowerShell 5.1, run `$env:CARGO_BUILD_WARNINGS='deny'` as its own statement first, then each command without the prefix. Companion gates: `cargo check -p gateway --no-default-features` (headless build shape), `cargo test -p build-xtask` (boundary and structural checks: product and container boundaries, the Engine dependency guard `engine_deps.rs`, the Engine clippy-config guard `engine_clippy.rs`, `engine_guards.rs`, `harness_bans.rs`, the doctest ban, the `test-support` leak guard, the unsafe allowlist, tidy, and the facade shape check), `node --test crates/workshop/ui/test/docs-claims.mjs` (wording and rulebook scan), `cargo +nightly-2026-09-05 xtask api --check` (facade surface against `crates/runvil/public-api.txt`; `--bless` rewrites the listing; the nightly is pinned in `crates/build-xtask/src/api/toolchain.rs`), and `cargo deny check` plus `cargo audit` (CI supply-chain job). Never run a standalone `cargo check --workspace` beside clippy.
- Formatter check command: `cargo fmt --all --check` (rustfmt `style_edition = "2024"`; the pre-commit hook runs it).
- Docs command: `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api` (in Windows PowerShell 5.1, run `$env:RUSTDOCFLAGS='-D warnings'` first), plus, under the same `RUSTDOCFLAGS`, the default-feature facade builds `cargo doc -p runvil --no-deps` and `cargo doc -p harness --no-deps` and the private-item build `cargo doc --locked --no-deps --all-features -p runvil-engine --document-private-items`. The Windows CI job also runs `cargo doc --locked --no-deps -p workshop-server --document-private-items`.
- Test placement and naming conventions: Unit tests take three forms, all common: a sibling file `<module>-tests.rs` wired at the end of the module with `#[cfg(test)] #[path = "<module>-tests.rs"] mod tests;` (a large one splits into `tests-<label>.rs` siblings); an inline `#[cfg(test)] mod tests { }`; or a `src/**/tests/` directory, as the engine's `src/execute/tests/` and `src/lua/tests/`, `runvil-lua`'s and `runvil-parser`'s `src/tests/`, and several gateway crates use. Integration tests compile as one binary per crate at `tests/it/main.rs` (most crates) or `tests/suite/main.rs` (`runvil`, `harness`), with one `mod` file per area plus a `support` module. Engine prompt fixtures live in `crates/runvil/tests/prompts/`. Test functions are snake_case sentences that state the behavior, such as `one_parent_cancel_ends_every_run_holding_one_of_its_children`; Miri-run tests carry a `miri_` prefix. Engine test support lives in `crates/runvil-internal/engine/src/test_support.rs` and `src/test_support/` (the serial `Scenario` driver, `ScriptedChat`, `Seeded`, `FnTool`, recording, the rig, and temp roots), behind `#[cfg(any(test, feature = "test-support"))]`; `runvil-parser` gates its `test_support` the same way, and `runvil-types`, `runvil-vfs`, and `runvil-model-client` keep theirs `#[cfg(test)]`-only. The `test-support` features of `runvil-engine`, `runvil-lua`, `runvil-parser`, and `runvil-plugin` are enabled only from `[dev-dependencies]` or bench `required-features`. Temporary directory names come from the test's name, the process id, and a counter. Some crates compile their test fixtures only under a crate feature (for example `gateway-stt-engine`'s `test-fixtures`), so keep `--all-features` on their test commands or nextest selects no tests.
- Directory map: `crates/` holds all code. Its top level is the public and shared layer: `runvil` (the Engine's public API facade), `runvil-plugin` (the Plugin API), `harness` and `harness-gateway-client`, the linked Plugins `plugin-web`, `plugin-mcp`, and `plugin-user-input`, `gateway-api-types` and `gateway-api-discovery`, `shared-error-source` and `shared-loopback`, `shared-ui` (a TypeScript and CSS package, excluded from the Cargo workspace), and the `build-*` tooling (`build-xtask` behind `cargo xtask`, `build-workshop` behind `cargo workshop`, plus `build-ui`, `build-user-guide`, `build-llama-cuda`, and `build-ceiling`). Four manifestless containers hold each family's private crates: `crates/runvil-internal/` (Engine: `types`, `parser`, `lua`, `engine`, `vfs`, `model-client`), `crates/harness-internal/` (`runner`, package `harness-runner`), `crates/gateway/` (the `app` binary and its family crates, with STT nested under `stt/`), and `crates/workshop/` (the Tauri `desktop` app, `server`, `server-api`, the tier crates such as `run-log` and `agents`, and the npm workspace members `ui`, `look`, and `platform`). The root `Cargo.toml` is a virtual workspace manifest, so a file directly under a manifestless container, such as `crates/runvil-internal/clippy.toml`, belongs to no package. Outside `crates/`: `guide/` is the user guide source; `prompts/` holds sample prompt programs; `vibe/` holds dated implementation plans (current ones at its top level, older ones in monthly subfolders, plus `scratch/`); `tools/` holds Node scripts with `.test.mjs` siblings; `.github/workflows/` holds CI; `.githooks/` holds pre-commit (fmt) and pre-push (wording scan, headless gateway check, clippy, cargo deny); `.cargo/config.toml` sets the `xtask` and `workshop` aliases and the Windows `rust-lld` static-CRT link; `.config/nextest.toml` sets timeouts and the heavy STT test group; `.cursor/rules/` holds two Workshop rulebooks; root `deny.toml`, `clippy.toml`, and `rustfmt.toml` configure the gates; `local/` is gitignored operator config, and `images/` holds README art.
- Component boundaries: the `runvil` facade depends on all six `runvil-internal` crates; `runvil-engine` depends on the other five; `runvil-parser` depends on `runvil-lua` and `runvil-types`; `runvil-lua` on `runvil-model-client`, `runvil-types`, and `runvil-vfs`; `runvil-model-client` on `runvil-types`; `runvil-types` and `runvil-vfs` are leaves. `runvil-plugin` depends on `runvil-types` and `runvil-vfs`, and each linked Plugin depends only on `runvil-plugin`. `harness-runner` depends on `runvil` and `runvil-plugin`; `harness` on `harness-runner`, `runvil`, and `runvil-plugin`; `harness-gateway-client` on `harness`, `plugin-web`, and `runvil`. Workshop's `run-log` and `agents` depend on `harness`, and `workshop-server` is the Workshop composition root, depending on the Harness crates, every linked Plugin, and every Workshop tier crate; `workshop-server-api` re-exports `workshop-server` for the `workshop` desktop crate. The gateway family depends only on `gateway-api-types`, `gateway-api-discovery`, `shared-*`, and its own crates, never on the Engine or Harness. `cargo test -p build-xtask` enforces the product and container boundaries, the Workshop tier graph, and lint inheritance.
- Conventions summary: Rust edition 2024 on the stable toolchain with resolver 3 and every dependency declared once in `[workspace.dependencies]`, each non-obvious choice explained by a comment. Workspace lints deny clippy `all` and `pedantic`, `unwrap_used` and `expect_used` (allowed in tests by `clippy.toml`), bare `allow` attributes (use `#[expect(..., reason = "...")]`), and `unsafe_code`; `missing_docs`, `missing_debug_implementations`, and `unreachable_pub` warn and the gate denies warnings. The root `clippy.toml` bans process-global installers outside binary entry points; `crates/runvil/`, `crates/runvil-internal/`, and `crates/runvil-plugin/` each hold an identical `clippy.toml` that repeats those entries and adds the Engine determinism bans (no sleep, wall clock, timed wait, or OS-seeded `RandomState`), and Engine crates never depend on `tokio`, `tokio-util`, `async-trait`, `reqwest`, `getrandom`, or `fastrand`, tests included. Doc comments hold no compiled code blocks. Source directories are flat: a subdirectory needs at least three files, otherwise modules sit beside the parent as `parent-label.rs` wired with `#[path]`. Every source file stays under the 500-line ceiling `build_ceiling::check` enforces from each crate's `build.rs`. Engine, Harness, Host, and Plugin are capitalized defined terms, and Engine crates say "the caller" instead of naming the Host. Comments state only non-obvious constraints. Error messages are written for model consumption and name required versus actual. Behavior changes ship with tests in the same change. JSON reaching a recorder or replay round-trips exactly with sorted keys. Text files are LF. Commit subjects are imperative sentence case, and a finished plan lands as a `Close plan: <slug>` commit.

</project-survey>
<execution-plan>

## Execution Instructions

Eleven steps, one commit each, in order. Step 0's re-consolidation is done: every path, symbol, count, and recipe below was checked against `852bb51b3` on 2026-10-15, and the Decision Record's "Re-consolidation record" lists what changed. Step 0 makes no commit and is not part of the run. The run-readiness review and a second pass the same day settled the remaining open choices, and their records sit beside the re-consolidation record.

The run's coding, review, and fix agents read only Technical Design, the Project Survey, and their own step. So the rules every step follows, including the shared test utilities and the search-recipe quoting, are under Technical Design, and the per-commit gate is in the Testing Plan. Each step ends with its "Done when" checks.

<step-1>

### Step 1: types and model-client [completed]

- Component: `types-model-client`
- Tests: Verify runs these from the repository root in PowerShell, in this order. A search passes only when it prints nothing.
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`
  - `cargo fmt --all --check`
  - `node --test crates/workshop/ui/test/docs-claims.mjs`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --locked --no-deps --all-features -p runvil-engine --document-private-items`
  - `Remove-Item Env:CARGO_BUILD_WARNINGS -ErrorAction SilentlyContinue; cargo +nightly-2026-09-05 xtask api --check`, because the nightly cargo turns its unused-workspace-dependency warning into an error while that variable is set
  - `rg -n "model_id_from_validated|thiserror" crates/runvil-internal/types`
  - `cargo nextest run --locked -p <package> --all-features` for every package the step touches, without `--all-features` for `workshop`, `workshop-server`, and `workshop-server-api`

Paths starting `T/` are under `crates/runvil-internal/types/src/`, and paths starting `M/` are under `crates/runvil-internal/model-client/src/`.

- Error kinds and messages:
  - `T/names.rs` `GlobalNameErrorKind`: add `Charset` per Shared interfaces. Add the matching `Charset` to `PluginIdErrorKind` and `ToolIdErrorKind`, and map it one to one in `T/plugins.rs` `from_global_name_kind` (~L155) and in `T/tools/ids.rs` (~L133).
  - `M/client/wire-canned.rs` `ToolCall::from_parts` and `Completion::from_result` (through `M/normalize.rs`): rejected caller-built parts return `CompletionErrorKind::InvalidParts` instead of the retryable `MalformedResponse`.
  - `T/tools/registry.rs` duplicate-identity error: change `{id:?}` to `` `{id}` ``, and add a test that pins the rendered message.
- `Role` per Shared interfaces:
  - `M/client/wire.rs` `Message.role` and the unchecked `impl Into<String>` entry in `M/detail.rs` take `Role`.
  - Replace lua `MessageRole` (`crates/runvil-internal/lua/src/protocol/request.rs` ~L313) there and in `protocol/parse/chat.rs` and `projection.rs`. Also replace every literal role comparison (recipe: `rg -n '\x22(system|user|assistant|tool)\x22' crates/runvil-internal crates/harness crates/harness-internal crates/harness-gateway-client`). The recipe had 153 hits at `852bb51b3`, many of them tool names or other strings that aren't roles. Change only role comparisons.
- `T/emitter.rs` `DebugMode` (~L47) and `LibraryLoad` (~L59) are enums. Mark them `#[non_exhaustive]` only, with no constructors.
- `#[non_exhaustive]` plus a `new` constructor and `with_*` setters for public-field structs:
  - `T/ids.rs` `Provenance`.
  - The six structs in `T/metrics.rs`.
  - `T/tools/origin.rs` `ToolCallOrigin`.
  - `M/model/options.rs` `ModelInvocation`. `ModelSet` is handled under Derives and encapsulation instead.
  - Rewrite every struct literal in the workspace (recipe: `rg -n "(CallMetrics|Usage|ClientTiming|LlamaTimings|VllmMetrics|ToolCallEvent|ToolCallOrigin|Provenance|ModelInvocation|ModelSet) \{" crates`). The four `ModelSet` literals become `ModelSet::from_parts` calls once its fields go private. At `852bb51b3` they are in `crates/runvil-internal/engine/src/`: `lua/tests.rs` (~L43), `lua/tests/globals.rs` (~L33), `execute/tests/models_loop.rs` (~L28), and `execute/tests/scheduler.rs` (~L20).
- API shape:
  - `T/tools/registry.rs` `ToolCatalog::new` takes `impl IntoIterator<Item = ToolDescriptor>`, and every caller moves its vector in (recipe: `rg -n "ToolCatalog::new" crates`).
  - Implement `FromStr` for `GlobalName`, `PluginId`, and `ToolId`, delegating to the existing `parse`. Replace `TaskOrigin::from_tag` with `FromStr` and a small error.
  - `M/client/wire.rs` `ToolArguments::names` returns a named iterator type.
  - Re-export `Role`, `ParseRoleError`, and `ArgumentNames` in the facade's `model` module, and `ParseTaskOriginError` in its `ids` module (`crates/runvil/src/lib.rs`), per File and public API changes.
- Derives and encapsulation:
  - `M/model/options.rs`: give `Temperature` an `Eq` impl (its finite invariant makes `==` an equivalence), then derive `Eq` on `ModelBinding`, `ModelInvocation`, and `ModelSet`. Delete the two forged-NaN tests in `M/model/options-tests.rs` (~L39, ~L53) and the stale comments.
  - `T/metrics.rs` `CallMetrics: Default`, and `T/models.rs` `ModelCatalog: Eq`.
  - `M/model/options.rs` `ModelSet` (~L307): make `bindings` and `default` private. It keeps `from_parts` (~L321) as its constructor, so it gets no `#[non_exhaustive]`, `new`, or `with_*` setters. Keep the `bindings()` and `binding()` accessors and add `default_alias()`. Two writers change it in place today, so add a mutator for each:
    - `push(&mut self, binding: ModelBinding)` for the engine's fill in `crates/runvil-internal/engine/src/execute/context-bound.rs` (~L146, `set.bindings.push(binding)`).
    - `set_default_alias(&mut self, alias: String)` for lua's `models.default` in `crates/runvil-internal/lua/src/models.rs` (~L304, `set.default = Some(label)`), which keeps its cannot-change-mid-run check and reads through `default_alias()`.
    - Rewrite the `default` field doc (~L310), which tells readers to use the field.
- Test plumbing:
  - Delete `T/detail.rs` `model_id_from_validated`. Its callers use `ModelId::gateway("test-model")` plus an invariant `expect` (recipe: `rg -n model_id_from_validated crates`). It had 10 hits at `852bb51b3`, including the engine `Scenario` in `crates/runvil-internal/engine/src/test_support/scenario.rs` (~L22, ~L89).
  - `T/untrusted/inventory.rs` has `pub(super) use table::{CONTROL_MARKUP, Shape};`, widened only so that `T/untrusted/tests.rs` can name `inventory::CONTROL_MARKUP` and `inventory::Shape` (~L257, ~L379). `inventory.rs` itself also uses both names, so keep a private import for non-test builds. For example, pair a `#[cfg(not(test))] use` with a `#[cfg(test)] pub(super) use`, or let the tests reach `table` directly.
- Lows:
  - Add variant-level `#[non_exhaustive]` to the data-holding error variants (`T/replay.rs` `ReplayError` ~L97 and ~L104, `M/client/wire.rs` ~L148 and ~L156), and narrow `ReplayError` to its crate.
  - `ToolId::plugin()` (`T/tools/ids.rs` ~L59) still allocates on every call, because `T/names.rs` `GlobalName::first` (~L68) clones a one-element `Vec<String>`. Build the `PluginId` from the borrowed first segment instead, which also helps `PluginId::contains`.
  - Delete the three `M/detail.rs` functions (~L99, ~L105, ~L126) that duplicate the `M/client/wire.rs` getters, and switch callers to the getters.
  - The `Display` and `Serialize` impls in `T/names.rs`, `T/timestamp.rs`, `T/plugins.rs`, and `T/tools/ids.rs` write directly instead of building a `String`.
  - `T/tools/output.rs`: add `ToolError::new`, and make `with_source` a builder as on `CompletionError`.
  - Remove `thiserror` from `crates/runvil-internal/types/Cargo.toml`, hand-writing `Display` and `Error` for its eight error types. Keep the serde derive. The `runvil-types` entry in `Cargo.lock` lists `thiserror`, so commit the refreshed lock.
  - Rewrite about 22 doc summaries as one short third-person sentence (examples: `T/ids.rs` ~L212, `T/replay.rs` ~L12 and ~L90, `T/timestamp.rs` ~L46).
- Done when:
  - `rg -n "model_id_from_validated|thiserror" crates/runvil-internal/types` returns nothing.
  - Tests cover `Charset`, the `Role` serde round trip, and the catalog message.
  - `ModelSet` has no public field, and its two in-place writers use `push` and `set_default_alias`.
  - The API listing is re-blessed and lists `Role`, `ParseRoleError`, and `ArgumentNames` under `runvil::model`, and `ParseTaskOriginError` under `runvil::ids`.

</step-1>
<step-2>

### Step 2: vfs [completed]

- Component: `vfs`
- Tests: Verify runs these from the repository root in PowerShell, in this order. A search passes only when it prints nothing.
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`
  - `cargo fmt --all --check`
  - `node --test crates/workshop/ui/test/docs-claims.mjs`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --locked --no-deps --all-features -p runvil-engine --document-private-items`
  - `Remove-Item Env:CARGO_BUILD_WARNINGS -ErrorAction SilentlyContinue; cargo +nightly-2026-09-05 xtask api --check`, because the nightly cargo turns its unused-workspace-dependency warning into an error while that variable is set
  - `rg -n "to_string_lossy" crates/runvil-internal/vfs/src`
  - `cargo nextest run --locked -p <package> --all-features` for every package the step touches, without `--all-features` for `workshop`, `workshop-server`, and `workshop-server-api`

Paths starting `V/` are under `crates/runvil-internal/vfs/src/`.

- Typed refusals per Shared interfaces:
  - `V/handle/access.rs`: policy `Ask` becomes `ApprovalRequired`, and `Deny` stays `PermissionDenied`.
  - `V/handle/scope.rs`: an ended call or run becomes `ScopeEnded`.
  - `V/router.rs` and `V/real.rs`: read-only becomes `ReadOnly`.
  - `V/real/resolve.rs`: escapes and dangling links become `OutsideRoot`.
  - The root-removal and root-rename refusals in `V/real.rs` and `V/memory.rs` become `RootProtected`.
  - `V/handle/store_view.rs`: no store becomes `NoStore`.
  - `V/router.rs`: across mounts becomes `CrossMount`.
  - `V/real.rs` `map_io`: every `io::Error`, OS `EACCES` included, becomes `Io` with `kind` and `raw_os_error`.
  - `NotUtf8` gains `valid_up_to: usize` per Shared interfaces (`V/error.rs` ~L152). Today the decoders in `V/handle/operations.rs` (~L33) and `V/traits.rs` (~L290) discard the `Utf8Error`.
  - Add `VfsError::tag()` with the tags under Shared interfaces.
  - Update these callers:
    - the engine conversion `raised_store_error` (`crates/runvil-internal/engine/src/error/convert.rs` ~L201 to ~L251), which gains an arm per new tag per Shared interfaces
    - the lua store messages (`crates/runvil-internal/lua/src/engine_globals-store.rs` ~L97)
    - the lua error-value tag map `store_error_reason` (`crates/runvil-internal/lua/src/error-value-store.rs` ~L9). It should call `VfsError::tag()`, and its `_ => "backend"` arm goes. Its sibling `store_error_fields` (~L36) carries the new variants' fields per Shared interfaces.
    - every matcher (recipe: `rg -n "PermissionDenied|Unsupported \{|VfsError::Backend" crates`). The recipe had 114 hits at `852bb51b3` and also matches `std::io::ErrorKind::PermissionDenied`, so edit only the `VfsError` sites.
  - Tests match variants instead of substrings (`V/lib.rs` tests, `V/router-tests.rs`, `V/real/tests/links.rs`, `V/detail-tests.rs`). The shared `TestFs` (`V/test_support.rs`) owns the read-only and seeding helpers that `V/router-tests.rs` uses.
- Mode gate:
  - `V/lib.rs` `is_mutation` becomes an exhaustive `match` over `Op`.
  - Mark `Op` and `Verdict` (`V/traits.rs`) and `Mode` (`V/lib.rs`) `#[non_exhaustive]`, and give downstream `Policy` impls a denying wildcard arm (recipe: `rg -n "impl .*Policy for" crates`). At `852bb51b3` the impls are `DeskFiles` (`crates/harness/tests/suite/vfs.rs`), `AllowAll` (`V/traits.rs`), `ModePolicy` (`V/lib.rs`), and `FlipPolicy` (`V/handle/tests.rs`). Only `DeskFiles` is downstream.
- Path safety:
  - `V/handle/claims.rs`: on Windows and macOS, rooted real-mount claim keys compare case-insensitively. Document this on `RealBackend` and in `V/path.rs`, and add a conflict test.
  - `V/real.rs` and `V/real/resolve.rs`: replace `to_string_lossy` with a typed refusal of non-UTF-8 names. Test it with a real non-UTF-8 name: an unpaired surrogate built with `std::os::windows::ffi::OsStringExt::from_wide` on Windows, and raw bytes through `std::os::unix::ffi::OsStrExt::from_bytes` on Linux. Skip macOS, whose file systems refuse such names. CI runs only Linux and Windows.
  - `V/handle/store_view.rs` reserved names: add `COM0`, `LPT0`, the superscript-digit `COM` and `LPT` names, and refuse `:`.
- API (additive, signatures frozen):
  - `V/stat.rs`: `Stat::new(file_type, size)` with `with_mode`, `with_modified`, and `with_created`, and `Entry::new(name, stat)` with `with_description`.
  - Add the enum-taking siblings under Shared interfaces, with the enums `Depth`, `GlobKind`, and `Writability`: `VfsAccess::remove_with`, `mkdir_with`, and `glob_with` (`V/traits.rs`), `Access::remove_with` and `mkdir_with` (`V/handle/operations.rs`), and `RealBackend::with_writability` (`V/real.rs`).
  - Add `FromStr` for `PathReason` (`V/error.rs`), wrapping the existing `PathReason::from_tag` (~L54 to ~L88).
  - Re-export `Depth`, `GlobKind`, `Writability`, and `ParsePathReasonError` in the facade's `vfs` module (`crates/runvil/src/lib.rs`), per File and public API changes.
  - Add the valid derives to `Stat` and `Entry` (`V/stat.rs`), `Origin` (`V/observe.rs`), and `AllowAll` (`V/traits.rs`).
  - Add `#[track_caller]` on `overlay` (`V/handle.rs`), `mount`, and `store` (`V/router.rs`).
- Structure:
  - Replace the `unwrap_or_else(|err| panic!(..))` sites on canonical-path slices with a crate-private infallible constructor guarded by `debug_assert!`. The sites are in `V/handle/region.rs` (~L15, ~L33), `V/handle/scope.rs` (~L230, ~L304), and `V/handle.rs` (~L146, ~L156). `V/handle.rs` already uses `VfsPath::root()`.
  - Split `V/traits.rs` into `backend`, `policy`, and `identity` behind the same `pub use` list.
  - Move the platform arms of `V/real/resolve.rs` and `V/real/files.rs` into `real/unix.rs` and `real/windows.rs`, each gated once, with a fallback.
  - Use `Arc::clone(&x)` in `V/handle.rs` and `V/handle/access.rs`.
  - Rewrite the `V/lib.rs` crate summary as one short sentence.
- Lows:
  - Add `close() -> Result` on the handle and router (`V/handle.rs`, `V/router.rs`), so callers can see `release` errors that `Drop` swallows. `V/router.rs` was 493 lines at `852bb51b3`, so split it before it gains `close()`, `ReadOnly`, and `CrossMount`. `V/error.rs` (382) will pass 450 with the new variants. `V/memory.rs` (462) gains the `RootProtected` refusal, so watch it too.
  - Give scope ids and clock values newtypes (`V/handle/scope.rs`, `V/handle/claims.rs`, `V/handle/prune.rs`).
- Tests:
  - Add a two-thread claims test.
  - `V/real/tests/links.rs`, file links: `make_file_link` (~L31) already fails on every error except the Windows refusal for want of the symlink privilege (OS error 1314). That refusal prints an `eprintln!`, returns `Ok(false)`, and lets the test pass silently. Give the privilege-dependent file-link tests `#[cfg_attr(windows, ignore = "needs the symlink privilege")]`, and make them fail on any link error.
  - `V/real/tests/links.rs`, directory links: the Unix `make_dir_link` (~L23) returns `false` on any error, so make a failure there fail the test too.
- Done when:
  - `rg -n "to_string_lossy" crates/runvil-internal/vfs/src` returns nothing (11 hits at `852bb51b3`).
  - `is_mutation` has no `matches!`.
  - Each new variant has a test.
  - The API listing is re-blessed and lists `Depth`, `GlobKind`, `Writability`, and `ParsePathReasonError` under `runvil::vfs`.

</step-2>
<step-3>

### Step 3: lua core part A and the lua error model [completed]

- Component: `lua-core`
- Tests: Verify runs these from the repository root in PowerShell, in this order.
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`
  - `cargo fmt --all --check`
  - `node --test crates/workshop/ui/test/docs-claims.mjs`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --locked --no-deps --all-features -p runvil-engine --document-private-items`
  - `Remove-Item Env:CARGO_BUILD_WARNINGS -ErrorAction SilentlyContinue; cargo +nightly-2026-09-05 xtask api --check`, because the nightly cargo turns its unused-workspace-dependency warning into an error while that variable is set
  - `cargo nextest run --locked -p <package> --all-features` for every package the step touches, without `--all-features` for `workshop`, `workshop-server`, and `workshop-server-api`

Paths starting `L/` are under `crates/runvil-internal/lua/src/`.

- Scope: these files, plus every crate-wide site of the error-type changes below, plus `crates/runvil-internal/engine/src/error.rs` and `crates/runvil-internal/engine/src/error/convert.rs` for the engine side of those changes.
  - `alias.rs`, `argv.rs`, `collection-order.rs`, `collection.rs`, `compactors.rs`, `coro.rs`, `detail.rs`, `dispatch.rs`
  - `engine_globals-store.rs`, `engine_globals.rs`, `error-value-store.rs`, `error-value.rs`, `error.rs`, `globals.rs`, `handles.rs`, `hardening.rs`
  - `iteration.rs`, `lib.rs`, `messages-view.rs`, `messages.rs`, `models-userdata.rs`, `models.rs`, `models_loop.rs`, and `models_loop/`
- Error model:
  - `L/error.rs` `LuaRuntime`, `LuaCompile`, `Tool`, and `Store` follow the error-chain convention. This covers their constructors (about 240 `Error::lua` uses, nearly all `.map_err(Error::lua)`, keep compiling), the direct builds in `L/program.rs` and `L/prelude.rs`, and the engine conversion in `crates/runvil-internal/engine/src/error/convert.rs`. Add the chain-repeat test.
  - Reclassify `Error::Lua(String)` (recipe: `rg -n "Error::Lua\(" crates/runvil-internal/lua/src -g '!*test*'`, 79 sites at `852bb51b3`):
    - Keep `Lua` for any condition the prompt author caused and can fix in Lua.
    - Use `Error::Internal` for Engine misuse that no author can cause: double injection in `L/vm/install.rs`, lifecycle order in `L/vm/state.rs`, and poisoned locks such as `L/scope.rs`. `Internal` widens to `Cow<'static, str>` per Shared interfaces, because some of these messages name a value.
  - The engine's `Error::Internal` `message` field (`crates/runvil-internal/engine/src/error.rs` ~L313) widens to `Cow<'static, str>` with lua's, per Shared interfaces, so `From<runvil_lua::Error>` (`crates/runvil-internal/engine/src/error/convert.rs` ~L172) passes a Lua internal message through whole. `Error::internal` keeps taking `&'static str`.
  - `L/error.rs` `From<ModelSetError>` (~L223) maps to `Error::Internal` instead of `Lua(error.to_string())`. `ModelSetError` is a unit struct for a poisoned model-set lock, so its message is the whole cause, and a poisoned lock is Engine misuse by the rule above. The engine mirror in `crates/runvil-internal/engine/src/error/convert.rs` (~L92), which also builds `Error::Lua` today, maps to `Error::internal` the same way.
  - `LuaQuota { resource: QuotaResource }` per Shared interfaces, with the engine mirror in `crates/runvil-internal/engine/src/error.rs`. Downcast detection is Step 4's.
  - `L/messages.rs` `MessageList::push` and its three helpers return a small error enum instead of `String`.
  - `L/error-value.rs` `ErrorValue::fields` becomes exhaustive.
- API and types:
  - Add `FromStr` for `ErrorKind` (`L/error-value.rs`) and `OverflowReason` (`L/compactors.rs`), and implement `std::error::Error` for `OverflowReason`.
  - `L/models-userdata.rs` `LuaModelHandle::from_binding` takes `ModelBinding` by value. Its four callers in `L/models.rs` (~L275, ~L306, ~L315, ~L324) and its test callers in `L/models_loop/tests.rs` (~L59) and `L/protocol/tests.rs` (~L44) move theirs in.
  - Replace the bool parameters with enums: `numbered` (`L/engine_globals.rs`) and `raw_ids` (`L/models.rs`). `succeeded` (`L/engine_globals-store.rs`) takes the `Result` itself.
  - `L/handles.rs` `ToolBinding` and `ToolSet` already have accessors (~L85, ~L203), but their fields (~L37 to ~L47, ~L190 to ~L197) are still `pub`. Make the fields private, and have `L/handles.rs` and `L/dispatch.rs` read through the accessors.
  - `L/alias.rs`: validated aliases travel as an `Alias` newtype through `L/models.rs` and `L/tools.rs`.
- Structure:
  - `L/lib.rs` keeps only crate docs, `mod`, and `pub use`. Each module imports std and external names itself and reaches crate items through `crate::` paths.
  - Move `HOOK_*` to `L/hardening.rs`, `LUA_LOG_CHARACTER_LIMIT` to `L/engine_globals.rs`, and the `DEFAULT_*` constants and `log_byte_budget` to the VM module. Delete the private `use tools::LuaToolHandle;` in `L/lib.rs` (~L137).
  - Move the scalar-return functions in `L/hardening.rs` beside their caller in `L/vm/run.rs`.
  - Store-conflict slot: on a poisoned lock, recover with `PoisonError::into_inner` instead of dropping the conflict, on the write side (`L/engine_globals-store.rs`) and in `take_store_conflict` (`L/vm/install.rs`).
- Lows:
  - Replace the `L/engine_globals-store.rs` macro with a generic function shared by `glob` and `exists`, and drop the `too_many_lines` expectation if it no longer applies.
  - Narrow the public items in `L/detail.rs` and `L/error-value-store.rs` that have no outside consumer, along with their `lib.rs` re-exports.
  - Fix the docs in `L/dispatch.rs` and `L/engine_globals.rs`. In `L/dispatch.rs`, the doc at ~L169 names the alias where the code keys on the id (~L107, ~L118). In `L/engine_globals.rs`, a summary was copied from elsewhere.
  - Rename `MessageList::records()` to `to_records()`, because it clones every record into a new `Vec` (`L/messages.rs` ~L80).
  - Preallocate in `L/iteration.rs` and `L/collection.rs`.
  - Rewrite this step's doc summaries as one third-person sentence and wrap module lines over 100 columns (`L/handles.rs`, `L/engine_globals.rs`, `L/hardening.rs`).
- Line ceiling: `L/coro.rs` was 486 lines and `L/error-value.rs` 469 at `852bb51b3`. Split either one before it grows.
- Done when:
  - The four variants carry no `#[source]`, and the chain test passes.
  - Every remaining `Error::Lua(` site is author-caused.
  - `L/lib.rs` holds no `use` lines other than `pub use`.
  - Lua's `Error::Internal` and the engine's `Error::Internal` `message` field both hold `Cow<'static, str>`, and a test shows an owned Lua internal message reaching the engine error whole.

</step-3>
<step-4>

### Step 4: lua core part B [completed]

- Component: `lua-core`
- Tests: Verify runs these from the repository root in PowerShell, in this order. A search passes only when it prints nothing.
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`
  - `cargo fmt --all --check`
  - `node --test crates/workshop/ui/test/docs-claims.mjs`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --locked --no-deps --all-features -p runvil-engine --document-private-items`
  - `Remove-Item Env:CARGO_BUILD_WARNINGS -ErrorAction SilentlyContinue; cargo +nightly-2026-09-05 xtask api --check`, because the nightly cargo turns its unused-workspace-dependency warning into an error while that variable is set
  - `rg -n "fanout: bool" crates/runvil-internal`
  - `rg -n '[^:]Integer\(1\)' crates/runvil-internal/lua/src`
  - `cargo nextest run --locked -p <package> --all-features` for every package the step touches, without `--all-features` for `workshop`, `workshop-server`, and `workshop-server-api`

Paths starting `L/` are under `crates/runvil-internal/lua/src/`.

- Scope:
  - These files and directories in `crates/runvil-internal/lua/src/`: `plugins.rs`, `prelude.rs`, `program.rs`, `projection.rs`, `prose.rs`, `protocol.rs` and `protocol/`, `proxy.rs`, `require.rs`, `scope.rs`, `sys.rs`, `tools.rs` and `tools/`, `vm.rs` and `vm/`.
  - `test_vm.rs`, the lua test hub that the harness plan added.
  - `argv.rs`, for the key formatter only.
  - The lua `Cargo.toml`, `build.rs`, and benches.
  - The embedded Lua shims (`__impl_*.lua`) change only if a request's wire shape changes. Keep the existing table fields when the Rust types change.
- Errors:
  - `L/program.rs`: raise the quota refusals as `mlua::Error::external(QuotaExceeded(..))` and classify them by downcast instead of `raw.contains(..)` (~L237 to ~L241). Add a test where an author raises the quota text.
  - `L/vm/run.rs`: recognize a retained typed error by identity (a per-answer token), not by comparing first `Display` lines. `block_failure` must keep the mapped prompt line and traceback.
  - Append the cause to the author message at `L/protocol/parse/tasks.rs` (task id), `L/plugins.rs` (`PluginId::parse`), and `L/protocol/parse.rs` (serde conversion of `item` and `args`).
  - Make the arms in `L/projection.rs` (now `Role`) and `L/vm/run.rs` exhaustive.
  - The `L/program.rs` `LuaProgram::compile` `# Errors` text (~L87) says a VM-creation failure is `Error::LuaRuntime`. Today it says `Error::Lua`.
- Protocol types:
  - In `L/protocol/answer.rs`, `TaskStatus.state` becomes `TaskPhase` and `TaskStatus.blocked` becomes `Option<Blocked>`, per Shared interfaces. Render their tags in `L/protocol/render.rs`. The `blocked` docs (~L178) still list `timer`.
  - Update the engine producers in `crates/runvil-internal/engine/src/execute/scheduler/`:
    - `waits.rs`.
    - `dispatch.rs` `blocked_on` (~L80). It returns `Option<&'static str>` today and already returns `None` for `Request::Timer`.
    - The `queued` write in `tasks.rs` (~L248).
    - `Chain.blocked` in `chain_record.rs` (~L132), which becomes `Option<Blocked>`.
    - `render_status` in `builtins.rs` (~L120), which compares `status.state == "done"` today. The type change breaks that comparison, so match `TaskPhase::Done` instead.
  - `SpawnVia` replaces three things:
    - `Spawn::fanout` (`L/protocol/request.rs` ~L75).
    - The engine's `dispatch_spawn` and `prepare_spawn` `fanout: bool` (`scheduler/tasks.rs` ~L128, ~L169).
    - The bare `false` in `scheduler/builtins.rs` (~L276).
  - `L/protocol/request.rs` was 452 lines and `L/protocol/parse.rs` 459 at `852bb51b3`. Split before the tighter types grow them.
  - Mark `VfsOutcome` `#[non_exhaustive]` (`L/protocol/answer.rs`).
  - `ChatResult` (`L/protocol/answer.rs`): one outcome enum replaces `overflow: bool` beside `overflow_reason`, and `reply` beside `tool_calls`.
  - Tighten parsed types:
    - the task `limit` becomes `NonZeroU64` (`L/protocol/parse/tasks.rs`, `L/protocol/request.rs`)
    - the awaited task set becomes a non-empty type
    - `tool_call_id` becomes a required `String` (`request.rs`, `L/projection.rs`)
    - `MessageContent::Parts` becomes non-empty
- VM:
  - `L/vm.rs` `SectionVm`: make `tool_runtime` and `model_runtime` (~L67 to ~L69) private behind `&Arc<Mutex<..>>` accessors, and switch the engine's readers of those fields to the accessors in the same commit. `L/vm/state.rs` `lua()` narrows to what the scheduler uses, or its doc is corrected.
  - Replace the `values_injected` flag (`L/vm.rs`, `L/vm/install.rs`, `L/vm/state.rs`, `L/vm/run.rs`) with typestate.
  - `L/require.rs`: hold the `loading` guard only for the final push.
  - Gather the 17 `test-support` gates (recipe: `rg -n '#\[cfg\([^\]]*test-support' crates/runvil-internal/lua/src`; a bare `test-support` search also matches doc comments) into one gated `pub mod test_support` with one predicate.
  - Move the `L/vm.rs` re-exports `run_chunk` and `LuaOutcome` (~L27) into the lua test hub, `L/test_vm.rs`.
  - Drop `L/tools.rs`'s `LuaToolHandle` re-export (~L38).
  - Make `L/vm/install.rs` `sys_live_handle` (~L367) `#[cfg(test)]`, replacing its `cfg_attr(not(test), expect(dead_code, ..))`.
- Messages: `L/sys.rs`, `L/prelude.rs`, and `L/argv.rs` render non-string keys through one crate formatter: an integer as `1`, anything else by `type_name()`, matching `child_path` (`L/prelude.rs` ~L147). Today the refusals at `L/argv.rs` ~L101 and `L/sys.rs` ~L179 print `{other:?}`. Update the four tests that pin `Integer(1)`: `L/prelude-tests.rs`, `L/prelude-tests-environment.rs`, `L/tests/sys_and_var.rs`, `L/tests/argv.rs`.
- Lows:
  - `L/tools.rs` `install_lua_tool_calls` takes `Vec<String>`.
  - Use `crate::` instead of `super::super` in `L/protocol/parse/chat.rs`, `tasks.rs`, and `store.rs`.
  - Rewrite combinator chains past two links as `match` or `let else` (`L/tools.rs`, `L/protocol/parse.rs`, `L/protocol/parse/tasks.rs`, `L/program.rs`).
  - Delete the doc comment in `L/program.rs` (~L260) that cites an audit finding.
  - Rewrite this step's doc summaries.
- Done when:
  - `rg -n "fanout: bool" crates/runvil-internal` returns nothing.
  - `rg -n '[^:]Integer\(1\)' crates/runvil-internal/lua/src` returns nothing. At `852bb51b3` its 4 hits were the four pinned messages. `Value::Integer(1)` fixtures may stay.
  - `L/program.rs` classifies no error by message text.
  - The API listing is re-blessed.

</step-4>
<step-5>

### Step 5: parser and facade [completed]

- Component: `parser-facade`
- Tests: Verify runs these from the repository root in PowerShell, in this order.
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`
  - `cargo fmt --all --check`
  - `node --test crates/workshop/ui/test/docs-claims.mjs`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --locked --no-deps --all-features -p runvil-engine --document-private-items`
  - `Remove-Item Env:CARGO_BUILD_WARNINGS -ErrorAction SilentlyContinue; cargo +nightly-2026-09-05 xtask api --check`, because the nightly cargo turns its unused-workspace-dependency warning into an error while that variable is set
  - `cargo nextest run --locked -p <package> --all-features` for every package the step touches, without `--all-features` for `workshop`, `workshop-server`, and `workshop-server-api`

Paths starting `P/` are under `crates/runvil-internal/parser/src/`.

- Scope: `crates/runvil-internal/parser` and `crates/runvil`.
- Errors:
  - Add `ParseErrorKind::Internal`. The parser's internal `Error::Internal(&'static str)` already exists (`P/error.rs` ~L65) and `P/build.rs` builds it, but today `classify_parse_error` maps it to `Structure` through its wildcard. Give it an explicit `Internal` arm.
  - `P/error.rs` `classify_parse_error` (~L220, with its `_ =>` wildcard at ~L253) and the `other => other` catch-all in `with_prompt_context` (~L136) become exhaustive, with a wildcard only inside the foreign lua `Error` arm.
  - The fence classification mismatch in `P/fence.rs` (`split_section_blocks`, ~L343), which the sidecar-strict-fences plan made an internal `ParseErrorKind::Structure` error, moves to `ParseErrorKind::Internal`.
  - Map the new kind in Workshop's wire codes (`crates/workshop/server/src/error.rs` ~L76 to ~L83). `ParseErrorKind` is `#[non_exhaustive]`, so without an explicit arm its `_ => "parse_error"` would absorb `Internal`.
  - `ParseFrontmatter` (`P/error.rs` ~L27, built in `P/parse.rs`) follows the error-chain convention. Add the chain-repeat test.
  - `P/contract.rs` `check_distinct_plugins` (~L118) returns a small error enum instead of `String`, classified at its call site in `P/parse.rs`.
  - Messages state the actual value: in `P/contract/args.rs` (~L122), the default's actual type, and in `P/parse.rs` (~L124 to ~L130), the H1 count and where they are.
- API (facade-visible):
  - `ParseError::span` returns `Range<usize>`, and `line()` and `column()` return `Option<NonZeroU32>` (`P/error.rs` ~L273 to ~L302). Update every caller.
  - `P/contract/args.rs` `ArgsDecl::iter` (~L171) and `P/contract/models.rs` `ModelRoles::iter` (~L105) return named iterator types instead of `impl Iterator`, and add `impl IntoIterator` for `&ArgsDecl` and `&ModelRoles`. Re-export `ArgsDeclIter` and `ModelRolesIter` in the facade's `prompt` module (`crates/runvil/src/lib.rs`), per File and public API changes.
  - Each `ArgType` variant doc names the YAML values it accepts.
- Structure:
  - Move the frontmatter model out of `P/build.rs` and `P/build-frontmatter.rs` into `P/frontmatter.rs`, and rename the rest of `build` to `sections`. Update the `pub use` of `Frontmatter` and its siblings in `P/lib.rs` (~L41 to ~L43).
  - Regroup imports in `P/build.rs`, `P/fence.rs`, `P/list.rs`, `P/contract/args.rs`, `P/contract/models.rs`, and `P/parse.rs` into three blocks, using `crate::` paths and module paths for free functions.
  - The heading walk in `P/build.rs` consumes its vector instead of indexing and cloning.
  - `P/contract.rs` uses the entry API instead of `contains_key` then `insert`.
  - Correct the `# Errors` text in `P/build.rs` (~L322) that names a nonexistent `Error::LuaCompile`.
- Docs: rewrite the multi-sentence doc summaries as one short third-person sentence (examples: `P/body_text.rs`, `P/prompt.rs`, `P/list.rs`, `P/fence.rs`, `P/contract.rs`). The audit counted 7, so recount before starting.
- Done when:
  - The only `_ =>` in `P/error.rs` is inside the lua arm.
  - Tests cover `Internal` and the frontmatter chain.
  - The API listing is re-blessed and lists `ArgsDeclIter` and `ModelRolesIter` under `runvil::prompt`.

</step-5>
<step-6>

### Step 6: engine core and the typed timer [completed]

- Component: `engine`
- Tests: Verify runs these from the repository root in PowerShell, in this order.
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`
  - `cargo fmt --all --check`
  - `node --test crates/workshop/ui/test/docs-claims.mjs`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --locked --no-deps --all-features -p runvil-engine --document-private-items`
  - `Remove-Item Env:CARGO_BUILD_WARNINGS -ErrorAction SilentlyContinue; cargo +nightly-2026-09-05 xtask api --check`, because the nightly cargo turns its unused-workspace-dependency warning into an error while that variable is set
  - `cargo nextest run --locked -p <package> --all-features` for every package the step touches, without `--all-features` for `workshop`, `workshop-server`, and `workshop-server-api`

Paths starting `E/` are under `crates/runvil-internal/engine/src/`.

- Scope: every non-test file in `E/` outside `E/execute/scheduler/`, plus the engine `Cargo.toml`, `build.rs`, and benches. Three changes reach further, and each lists its sites below: the typed timer (two scheduler files, the lua protocol, the Harness, Workshop, and the facade example and suite), the `AnswerRecord` format (`tag()` in model-client and types, `OutputTrust`'s serde derives, the `FailureRecord` re-export in `crates/runvil/src/lib.rs`, and the record's readers and fixtures), and the no-store message (one lua file).
- Errors:
  - Both infer arms in `E/execute/tools.rs` (renamed below) return `Error::Completion(CompletionError::new(CompletionErrorKind::MalformedResponse, ..))`. Test both the kind and `is_retryable()`.
  - `E/error.rs` `LuaRuntime`, `LuaCompile`, `Tool`, and `Store` follow the error-chain convention, built in `E/error/convert.rs` (including the test-only constructor). Add the chain-repeat test.
  - `Error::Internal` and `Error::RequirementsUnmet` gain their cause fields per Shared interfaces. Add `Error::internal_with_cause`, which Step 7 uses.
  - The no-store failure says what's missing. Today the probe wrapper `Error::store` (`E/error/convert.rs` ~L53) and the store-view setup in `crates/runvil-internal/lua/src/engine_globals-store.rs` (~L188) both render the fixed text `store operation failed`, even for `VfsError::NoStore`. For `NoStore`, both render a message saying the run declares no store. Other failures keep the fixed text.
  - `E/execute/walk_target.rs` keeps its cause.
  - Make `E/error/value.rs` and `E/execute/error.rs` `is_retryable` and `location` exhaustive.
- Typed timer per Shared interfaces:
  - `crates/runvil-internal/lua/src/protocol/parse/tasks.rs` already validates the seconds with `Duration::try_from_secs_f64` (~L56 to ~L61), but `Request::Timer` still carries `seconds: f64` (`request.rs` ~L83 to ~L87). Store the `Duration` there, and carry the await timeout the same way.
  - Drop the re-checks in `E/execute/scheduler/await_tasks.rs` (~L51 to ~L63) and `E/execute/scheduler/timer.rs` (~L52 to ~L54).
  - `E/execute/run/effect.rs` `Effect::Timer { duration }` keeps a seconds record.
  - Update every `Effect::Timer` match (recipe: `rg -n "Effect::Timer" crates`, which had 14 hits at `852bb51b3`, `public-api.txt` included):
    - the serial driver, `perform_locally` in `E/test_support.rs` (~L80)
    - `Scenario`'s stepping in `E/test_support/scenario-driving.rs` (~L89)
    - `crates/runvil/examples/greeter.rs` (~L52)
    - the facade suite's `Scenario` in `crates/runvil/tests/suite/support/scenario.rs` (~L301)
    - the Harness effect loop in `crates/harness-internal/runner/src/effect_loop.rs` (~L368)
  - The Harness's `Timer::sleep(&self, seconds: f64)` (`crates/harness-internal/runner/src/performers.rs` ~L115) takes `Duration` per Shared interfaces. Update every implementation:
    - `crates/harness/examples/run-prompt.rs` (~L54)
    - `crates/harness/tests/suite/support.rs` (~L38)
    - `crates/workshop/agents/src/timer.rs` (~L16)
    - the runner tests in `crates/harness-internal/runner/tests/it/`: `performers.rs` (~L33), `support.rs` (~L154, ~L366), and `scripted.rs` (~L187)
  - `effect_loop.rs` was 499 lines and `scenario-driving.rs` 492 at `852bb51b3`. The timer change must not grow them, so split first if it would.
- Facade surface:
  - Mark these `#[non_exhaustive]` and give the structs constructors: `RunResult` (`E/execute.rs`), `Round`, `EffectRecord`, `AnswerRecord`, `ChatAnswerRecord`, and `ToolAnswerRecord` (`E/execute/run/effect.rs`), and `SourceLocation` (`E/execute/error.rs`).
  - `RunContext` is already `#[non_exhaustive]`, and `RunSeed` and `RunUid` keep private fields, so none of them needs this change.
  - Document `Effect` and `EffectAnswer` (`effect.rs`) and `Step` (`E/execute/run.rs`) as closed on purpose, so a new effect kind breaks every caller's build. Engine text says "the caller", never Host, because the wording scan rejects it.
  - `AnswerRecord` follows Shared interfaces:
    - Add `FailureRecord`, `CompletionErrorKind::tag()` (`crates/runvil-internal/model-client/src/model/error.rs`), `ToolErrorKind::tag()`, and `OutputTrust`'s serde derives (both in `crates/runvil-internal/types/src/tools/output.rs`). Re-export `FailureRecord` in the facade's `effect` module (`crates/runvil/src/lib.rs`), per File and public API changes.
    - Update every reader and builder of the record (recipe: `rg -n "AnswerRecord|ToolAnswerRecord|ChatAnswerRecord" crates`, 17 files at `852bb51b3`) and every JSON fixture of it (recipe: `rg -n '\x22trusted\x22' crates -g '*.rs'`). The fixture in `crates/harness-internal/runner/tests/it/effect_loop.rs` (~L176) changes. The `tool_result` event in `crates/runvil-internal/types/src/event-tests.rs` (~L387) keeps its own `trusted` field.
    - Add a test that round-trips a failure record of each kind through JSON.
  - `E/execute/config-limits.rs` `RunLimits` derives `PartialEq`, `Eq`, and `Hash`.
- Ownership:
  - `E/execute/run.rs` `Run::new` moves `ui`, `preludes`, and `roster` out of its owned `RunContext` instead of deep-copying them from a borrow, and drops the `needless_pass_by_value` expectation. The copies are at `E/execute/context.rs` ~L162 to ~L163 and `E/execute/context-bound.rs` ~L25. `Run::new` is at `E/execute/run.rs` ~L109 to ~L114, and `prepare_state` (~L269) still takes `&RunContext`.
  - Remove the never-read `SectionContext::var` (`E/execute/section_context.rs`, `section_context-construct.rs`).
  - The prose renderer shares `args` and `argv` through `Arc` instead of copying them per block.
  - `Run`'s two `Option`s become one live, stillborn, or spent enum.
  - `E/execute/requirements.rs` writes each unmet line directly.
  - `E/execute/context.rs` uses `Arc::clone(&model_set)`.
- Structure:
  - Rename `E/execute/tools.rs` to `infer`, and split `E/execute/support.rs` by concept.
  - Delete the test-only re-exports in `E/lua.rs` (`LuaProgram`) and `E/tools.rs` (`OutputTrust`, `ToolError`, `ToolErrorKind`, `ToolId`, `ToolOutput`). Seven test files import those re-exports directly: `walk_target-tests.rs`, `execute/tests.rs`, `model/tests.rs`, `lua/tests.rs`, `lua/tests/errors.rs`, `lua/tests/coroutine.rs`, and `tools-tests.rs`. They import from `runvil_lua` and `runvil_types::tools` instead. The `execute/tests/` children reach them only through the `execute/tests.rs` hub. Retire `E/tools-tests.rs`: all three of its tests check the re-exports this item deletes. Move it to the trash and drop its wiring in `E/tools.rs`.
  - Regroup imports in `E/execute/` (examples: `run/effect.rs`, `context.rs`, `config.rs`, `scope.rs`, `section_vm.rs`), reaching free functions through their module.
- Done when:
  - `Effect` carries no `f64`.
  - The infer test passes.
  - No Engine crate text names the Host, and the wording scan passes.
  - The API listing is re-blessed and lists `FailureRecord` under `runvil::effect`.

</step-6>
<step-7>

### Step 7: engine scheduler [completed]

- Component: `engine`
- Tests: Verify runs these from the repository root in PowerShell, in this order. A search passes only when it prints nothing.
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`
  - `cargo fmt --all --check`
  - `node --test crates/workshop/ui/test/docs-claims.mjs`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --locked --no-deps --all-features -p runvil-engine --document-private-items`
  - `Remove-Item Env:CARGO_BUILD_WARNINGS -ErrorAction SilentlyContinue; cargo +nightly-2026-09-05 xtask api --check`, because the nightly cargo turns its unused-workspace-dependency warning into an error while that variable is set
  - `rg -n "unreachable!|root_result|TASK_BUILTINS|RESERVED_TOOL_NAMES" crates/runvil-internal/engine/src/execute/scheduler`
  - `cargo nextest run --locked -p <package> --all-features` for every package the step touches, without `--all-features` for `workshop`, `workshop-server`, and `workshop-server-api`

Paths starting `S/` are under `crates/runvil-internal/engine/src/execute/scheduler/`.

- Scope: every non-test file in `S/`, plus the files outside it that this step's items name: `crates/runvil-internal/engine/src/error.rs` and `crates/runvil-internal/engine/src/execute/error.rs` for `CallDepthExceeded`, its `RunErrorKind::Quota` mapping, and that kind's doc; `crates/runvil-internal/engine/src/execute/context-bound.rs` for `sort_unstable`; and the scheduler-level test rig `crates/runvil-internal/engine/src/test_support/rig.rs`, which the new state types reach.
- State types:
  - `S/chain_record.rs` `Chain`: a `ChainKind { Root, H1, Call { parent }, Task { owner, admission } }` replaces `owner`, `parent`, `h1`, `slots_used`, `holding`, `released_holder`, `admitted`, and `pending_spawn`. Every hand decoder matches on it (examples: `S/chain.rs`, `S/admission.rs`, `S/step.rs`).
  - Replace `root_result: &mut Option<Result<String>>` with a returned `ChainStep { Continue, RunEnded(Result<String>) }` across `S/step.rs`, `S/chain.rs`, `S/h1.rs`, `S/walk.rs`, and the owner in `S/drive.rs`.
  - `TaskBuiltin` per Shared interfaces replaces `TASK_BUILTINS` (`S/builtins.rs`), `RESERVED_TOOL_NAMES` (`S/tool_call.rs`), and the literals in `S/builtins-schemas.rs`. The router matches exhaustively, and the always-true filter in `S/tool_call.rs` goes.
  - `S/tasks.rs` `TaskSlot.state` and `ok` become one enum, and in `S/await_tasks.rs` the `AwaitTasks` timer field and its timeout field, a `Duration` since Step 6, become one optional struct.
  - Add the newtypes `SectionIndex`, `BlockIndex`, and `CallDepth` (`S/chain_record.rs`), and keep `NonZero` concurrency without `.get()` (`S/h1.rs`, `S/walk.rs`, `S/waits.rs`). `start_chain` (`S/chain.rs`) takes an argument struct.
- Errors:
  - `CallDepthExceeded` per Shared interfaces at `S/dispatch.rs` and `S/tasks.rs`. Other scheduler `Error::Lua(String)` sites are author-caused and stay.
  - The H1 remap in `S/step.rs` moves the original error into `RequirementsUnmet`'s cause.
  - Keep causes in `S/builtins-schemas.rs` and `S/chat.rs` (as `Internal` with a cause, through `Error::internal_with_cause`), and in `S/builtins.rs`, `S/timer.rs`, and `S/chain.rs`.
  - `S/admission.rs` `admit_chain` returns `Result`, replacing its three `unreachable!` sites (~L145, ~L149, ~L164).
  - `S/drive.rs` (~L63) holds a fourth site, `unreachable!("the phase was matched as ending")`, which the Done-when recipe also catches. Restructure that match so the arm can't occur, or return `Error::internal`.
  - Propagate the `let _ =` in `S/await_tasks.rs` as `Internal`.
  - Make exhaustive: `S/apply.rs` (`Continuation`, and the `(Continuation, EffectAnswer)` pair), `S/chat.rs` (`DispatchTarget`), and `S/tool_call.rs` (`Answer`).
- Ownership and hygiene:
  - `S/dispatch.rs` `classify_vfs_failure` takes `VfsError` by value from `S/apply.rs`.
  - `S/chain.rs` matches the outcome by value with an `e @ Error::Determinism(_)` binding.
  - One `Chain` accessor replaces the 12 hand checks of the live-frame invariant (examples: `S/step.rs`, `S/chat.rs`, `S/tool_call.rs`).
  - Remove the throwaway vectors (`S/task_end.rs`, `S/drive.rs`), and turn side effects inside `map` into loops (`S/notices.rs`, `S/apply.rs`).
  - Use `sort_unstable` on unique keys (`S/task_end.rs`, `S/waits.rs`, `crates/runvil-internal/engine/src/execute/context-bound.rs`).
  - Derive `Debug` on the 16 plain-data types: `S/chain_record.rs` (~L43 and `Chain`), `S/pending.rs` (`Continuation`, `CallSite`, ~L47, ~L59, ~L112), `S/builtins.rs` (`BuiltinAnswer`, ~L99), `S/chat.rs` (`ChatDispatch` ~L78, `ArrivedRound` ~L287), `S/notices.rs` (~L35), `S/step.rs` (~L35), `S/tool_call.rs` (`ToolRoute`, ~L78), and `S/walk.rs` (`ChainTarget`).
  - Regroup imports in `S/builtins.rs`, `builtins-schemas.rs`, `chat.rs`, `dispatch.rs`, `drive.rs`, `pending.rs`, `step.rs`, `tool_call.rs`, `waits.rs`, and `walk.rs`. Call `access_id`, `end_access`, and `access_spawn` as `detail::..` (`S/apply.rs`, `S/tasks.rs`, `S/tool_call.rs`).
- Line ceiling: `S/tool_call.rs` was 472 lines at `852bb51b3`, and this step adds the `TaskBuiltin` routing, an exhaustive `Answer` match, and `Debug` derives to it. Split it first if they would push it past 500.
- Done when:
  - `rg -n "unreachable!|root_result|TASK_BUILTINS|RESERVED_TOOL_NAMES" crates/runvil-internal/engine/src/execute/scheduler` returns nothing.
  - The three files above have no `_ =>` on owned enums.

</step-7>
<step-8>

### Step 8: engine test support and the remaining engine tests [completed]

- Component: `engine`
- Tests: Verify runs these from the repository root in PowerShell, in this order.
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`
  - `cargo fmt --all --check`
  - `node --test crates/workshop/ui/test/docs-claims.mjs`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --locked --no-deps --all-features -p runvil-engine --document-private-items`
  - `cargo nextest run --locked -p <package> --all-features` for every package the step touches, without `--all-features` for `workshop`, `workshop-server`, and `workshop-server-api`

Paths starting `E/` are under `crates/runvil-internal/engine/src/`.

- Scope: `E/test_support.rs` and `E/test_support/`, plus every engine test file outside `E/execute/tests/`.
- Already done by the internal-crate test harnesses plan, or gone with the engine's tokio driver, so not part of this step: the tokio driver's `JoinSet` panic re-raise and test-hook cleanup, `ScriptedChat` sharing with the bench, the `test-support` docs and the `drive_tokio`, `Performer`, and `Performers` visibility, the `TestTool` reduction (`FnTool`), the test-model binding constructor, and the `chunk`, `SectionVmSetup`, and `fresh_access` dedup.
- Already done or gone at `852bb51b3`, so not part of this step:
  - `TestToolTable` and `E/test_support/fixture.rs` went with the tokio driver in `0e5614a6a`.
  - `only_effect` now lives in `E/test_support/scenario-serial.rs` with `#[track_caller]` (~L27).
  - The `ScriptedChat` helpers have `#[track_caller]` (`scripted-chat.rs` ~L63).
  - `recording/forward.rs` `forward_one` has its `# Panics` section (~L147).
  - The debt-removal plan made `ScriptedChat::complete_now` return `Result`, with an `# Errors` section.
- Fixtures: `E/test_support/recording.rs` `Observer` hooks (~L40 to ~L90) take one argument struct, with no constant placeholder coordinates and `trust: OutputTrust` instead of `trusted: bool`. Update the producer in `recording/forward.rs` (~L344 to ~L355).
- Lows:
  - Add `#[track_caller]` wherever it's missing on the panicking helpers in `E/execute/run/tests-drops.rs`, `E/lua/tests.rs`, and `E/error/tests.rs`.
  - Add `# Panics` docs on the public panicking functions in `E/test_support.rs`, such as `drive` (~L98), which has none.
  - Drop the inert `#[non_exhaustive]` on the private `NullObserver` and `DebugEvent` (`recording.rs` ~L138 to ~L171), and add it to the public `Observation` (`recording/observation.rs` ~L93).
  - Rewrite the public doc summaries in `E/test_support/` that run past one short sentence. The audit counted 9, so recount before starting.
- Done when:
  - `Observer` hooks take one argument struct and carry `trust: OutputTrust`.
  - `recording.rs` has no `#[non_exhaustive]` on a private item.

</step-8>
<step-9>

### Step 9: engine execute tests [completed]

- Component: `engine`
- Tests: Verify runs these from the repository root in PowerShell, in this order. A search passes only when it prints nothing.
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`
  - `cargo fmt --all --check`
  - `node --test crates/workshop/ui/test/docs-claims.mjs`
  - `rg -n -U "else \{\s*panic!\(" crates/runvil-internal/engine/src/execute/tests`
  - `rg -n -F -e "use self::context::*;" -e "use self::fixtures::*;" -e "use self::gateway::*;" crates/runvil-internal/engine/src/execute`
  - `cargo nextest run --locked -p <package> --all-features` for every package the step touches, without `--all-features` for `workshop`, `workshop-server`, and `workshop-server-api`

Paths starting `X/` are under `crates/runvil-internal/engine/src/execute/tests/`.

- Scope: `X/` and `crates/runvil-internal/engine/src/execute/tests.rs`.
- Already done by the internal-crate test harnesses plan, so not part of this step:
  - All determinism work: paused time, current-thread runtimes, `multi_thread` reasons, the seed loops' seed printing, and the `X/scheduler/store_gate.rs` backstop.
  - One definition each of the shared helpers `task`, `run_offline`, `TestStore`, `PARKED_CHILD`, `EXECUTION`, `Recorder`, and `task_lifecycle`, with the dead `tools` and `_tools` parameters removed.
  - The prompt shell that replaces the inline frontmatter literals and the `*_prompt!` macros.
- Other changes since the audit:
  - `tempfile` in `X/suite/vfs.rs` was swapped by facade-suite-dedup, and `TempRoot` has since replaced it.
  - The engine `fresh_access` dedup was Step 8's.
  - The debt-removal plan restructured `X/happens_before.rs` so every sweep run gets its own `TestStore`, through the new helper `assert_every_interleaving_leaves`.
- Failure output:
  - Rewrite the `let ... else { panic!(..) }` sites as a `match` with an `other => panic!("..: {other:?}")` arm, or use `text_of`, which now lives in `E/test_support/scenario-serial.rs` (~L64). The recipe is `rg -n -U "else \{\s*panic!\(" crates/runvil-internal/engine/src/execute/tests`, with 34 sites at `852bb51b3`.
  - Give each bare `assert!(matches!(..))` that has no message one that prints the value. There were 9 without a message at `852bb51b3`, of 12 in all, for example `X/serial_driver.rs` ~L67. Find them with `rg -n 'assert!\(matches!\(' crates/runvil-internal/engine/src/execute/tests` and keep only the ones with no message.
  - The six `format!("{:?}", trace.error())` plus `.contains(..)` sites assert `error.kind()` and `error.to_string()` instead. They are in `X/suite/exec_flow/globals.rs` (~L149, ~L195), `fanout_arms.rs` (~L197, ~L252, ~L285), and `chains.rs` (~L354).
- Duplicate tests: delete the three bodies in `X/live_infer.rs` (~L25, ~L177, ~L367) that copy `X/scheduler/live_h1/pass.rs` (~L10, ~L29, ~L67).
- Imports:
  - Replace the glob chains `use self::context::*;`, `use self::fixtures::*;`, and `use self::gateway::*;` (`execute/tests.rs` ~L35 to ~L37) with explicit imports in each file that relies on them. The `pub(super) use self::tools::*;` in `X/context.rs` is already gone.
  - Write `crate::` paths instead of the `super::super` lines (15 at `852bb51b3`).
- Lows:
  - `X/tool_loop.rs`: assert `RunLimits::new().tool_iterations()` and delete the mirror constant `DEFAULT_MAX_TOOL_ITERATIONS` in `X/context.rs` (~L94 to ~L96).
  - Use `detail::` constants for the hard-coded observation labels that remain: the `"Run succeeded"` literal in `X/suite/execution.rs` (~L172), and the label string constants such as `TASK_STARTED` in `X/scheduler.rs` (~L80 to ~L84) that `X/scheduler/failures.rs` uses. `X/live_infer.rs` already uses `detail::`.
- Done when:
  - The `let ... else { panic!(..) }` recipe returns nothing.
  - The glob chains are gone.

</step-9>
<step-10>

### Step 10: lua tests [completed]

- Component: `lua-tests`
- Tests: Verify runs these from the repository root in PowerShell, in this order. A search passes only when it prints nothing.
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`
  - `cargo fmt --all --check`
  - `node --test crates/workshop/ui/test/docs-claims.mjs`
  - `rg -n '\.expect\(\x22[^\x22 ]+( [^\x22 ]+)?\x22\)' crates/runvil-internal/lua/src -g '*test*' -g '**/tests/**'`
  - `cargo nextest run --locked -p <package> --all-features` for every package the step touches, without `--all-features` for `workshop`, `workshop-server`, and `workshop-server-api`

Paths starting `L/` are under `crates/runvil-internal/lua/src/`.

- Scope: every lua test file: `tests` paths, `-tests.rs`, `tests-*.rs`, and `tests-recording.rs`.
- Correctness:
  - The "payload-free" tests assert on serialized raw drained `Event`s that hold only identity fields, and drop their `format!("{:?}")` searches. They are in `L/tests/section_vm.rs` (~L332), `L/tests/tool_scoping.rs` (~L174), `L/tests/diagnostics.rs` (~L355), and `L/tests/store_reports.rs` (~L94).
  - `L/models_loop/machine-tests.rs` `show` (~L38) prints the `ErrorField::String` text, and its three assertions are updated.
  - The 32-state loop in `L/models-tests.rs` (~L232) prints the iteration and checks key order on the sorted option keys.
  - `L/tests-recording.rs` (~L112) panics with a message on a serialization failure instead of recording `Observation::Other("")`.
- Already done by the internal-crate test harnesses plan, so not part of this step:
  - The `L/tests/cancellation.rs` elapsed-time bound, and the one nonce seed.
  - The shared test hub, `TestVm` in `L/test_vm.rs`, with its `fresh_access`, `lua_over`, setup-chain, and construct-inject-install dedup.
  - The duplicated fixture builders such as `tool_at` and `scoped`.
  - `h2_vm`, which `TestVm` replaces.
- Lows:
  - Rewrite the short `expect` messages to name the invariant, using `lua_table` instead of hand-built `raw_set` chains (examples: `L/protocol/tests/parse_tasks.rs`, `L/protocol/tests/parse.rs`). The recipe is `rg -n '\.expect\(\x22[^\x22 ]+( [^\x22 ]+)?\x22\)' crates/runvil-internal/lua/src -g '*test*' -g '**/tests/**'`. The second glob catches test files whose names lack `test`, such as `L/protocol/tests/parse.rs`. At `852bb51b3` it found about 133, mostly under `L/protocol/tests/`.
  - Regroup imports in `L/models-tests.rs`, `L/tests.rs`, `L/tools/tests.rs`, and `L/tests/library_load.rs`, and write `crate::` instead of `super::super` in `L/models_loop/machine-tests.rs`.
  - Drop the `D-` tags (`L/iteration-tests.rs` ~L188, ~L304, ~L325).
  - Use plain ownership instead of `Arc<Recorder>` in single-threaded tests where no Lua callback captures it (examples: `L/tests/logging.rs`, `L/tests/section_vm.rs`).
  - Rename the test module `recording::detail` (`L/tests-recording.rs`) and update its imports in `L/tests.rs` (~L19) and `L/dispatch-tests.rs` (~L9).
- Done when:
  - The `expect` recipe returns nothing.
  - No test searches `Debug` text for payloads.

</step-10>
<step-11>

### Step 11: workspace manifests and lints [completed]

- Component: `workspace`
- Tests: Verify runs these from the repository root in PowerShell, in this order, beside the FULL-scope commands in the Project Survey. A search passes only when it prints nothing.
  - `node --test crates/workshop/ui/test/docs-claims.mjs`
  - `cargo xtask tidy`
  - `Remove-Item Env:CARGO_BUILD_WARNINGS -ErrorAction SilentlyContinue; cargo +nightly-2026-09-05 xtask api --check`, because the nightly cargo turns its unused-workspace-dependency warning into an error while that variable is set
  - `cargo +<floor> build --locked -p runvil -p runvil-engine -p runvil-lua -p runvil-parser -p runvil-types -p runvil-vfs -p runvil-model-client`, where `<floor>` is the `rust-version` this step sets in the root `Cargo.toml` `[workspace.package]`
  - `rg -n "model_id_from_validated|thiserror" crates/runvil-internal/types`
  - `rg -n "to_string_lossy" crates/runvil-internal/vfs/src`
  - `rg -n "fanout: bool" crates/runvil-internal`
  - `rg -n '[^:]Integer\(1\)' crates/runvil-internal/lua/src`
  - `rg -n "unreachable!|root_result|TASK_BUILTINS|RESERVED_TOOL_NAMES" crates/runvil-internal/engine/src/execute/scheduler`
  - `rg -n -U "else \{\s*panic!\(" crates/runvil-internal/engine/src/execute/tests`
  - `rg -n -F -e "use self::context::*;" -e "use self::fixtures::*;" -e "use self::gateway::*;" crates/runvil-internal/engine/src/execute`
  - `rg -n '\.expect\(\x22[^\x22 ]+( [^\x22 ]+)?\x22\)' crates/runvil-internal/lua/src -g '*test*' -g '**/tests/**'`

- Scope: the root `Cargo.toml`, the seven crate manifests, `deny.toml`, and `.github/workflows/ci.yml`. The hakari items and the minimal workspace `tokio` entry are not part of this step: the remove-hakari plan deleted hakari, and the engine no longer has an optional tokio dependency. The workspace `tokio` entry itself (root `Cargo.toml` ~L100) stays for the crates outside the Engine.
- Toolchain:
  - Add `rust-version` to `[workspace.package]` at the verified floor: at least 1.88 for let chains, confirmed by building the seven crates at the candidate version. At `852bb51b3` this machine has 1.89 installed but not 1.88, so trying 1.88 first takes `rustup toolchain install 1.88.0`.
  - Each of the seven crates sets `rust-version.workspace = true`.
  - Verify confirms the floor locally with `cargo +<floor> build --locked -p runvil -p runvil-engine -p runvil-lua -p runvil-parser -p runvil-types -p runvil-vfs -p runvil-model-client`.
  - Add one CI job that builds them at that version. `.github/workflows/ci.yml` has only the stable jobs and the API nightly job today. The declared toolchain is `rust-toolchain.toml` `channel = "stable"`.
- Dependencies:
  - Set `pulldown-cmark` to `default-features = false` with no features (root `Cargo.toml` ~L75, `pulldown-cmark = "0.12"` with default features today).
  - Neither of its two users needs a default feature. `runvil-parser` (`crates/runvil-internal/parser/src/fence.rs` ~L15, and the module Step 5 renamed from `build` to `sections`) and `crates/build-xtask/src/no_doctests.rs` (~L20) use only the core parser API, never `html` or `getopts`. The change drops `pulldown-cmark-escape` from `Cargo.lock`, so commit the refreshed lock.
- Lints:
  - Add `rust_2018_idioms` (as a group with priority -1), `clippy::todo = "deny"`, and rustdoc `unescaped_backticks = "deny"` to the workspace lint tables (root `Cargo.toml` ~L266 to ~L298).
  - Fix their fallout in every crate in this commit. `unescaped_backticks` is a rustdoc lint, so clippy never reports it: find its fallout with the Project Survey's docs commands, including the `workshop-server` private-items build. At `852bb51b3` about 20 doc lines in the Engine crates alone have an odd number of backticks, though many are valid multi-line code spans.
- Docs:
  - Comment the parser's `test-support` feature (`crates/runvil-internal/parser/Cargo.toml` ~L21).
  - Correct the `deny.toml` "all targets" comment (~L5) to "all features".
- Done when: the full exit criteria in the Testing Plan pass.

</step-11>

### Dependencies and verification

- Steps 1 through 11 run in order. Step 0 is done (see "Re-consolidation record" in the Decision Record).
- Every commit passes the per-commit gate in the Testing Plan.
- Steps 1, 2, 4, 5, and 6 re-bless the API listing with `cargo +nightly-2026-09-05 xtask api --bless` before the gate's `--check`.
- Step 11 runs the full exit criteria.

</execution-plan>
