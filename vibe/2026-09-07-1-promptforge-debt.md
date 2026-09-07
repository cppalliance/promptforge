---
name: collect-promptforge-debt
overview: Remove technical debt attributable to the 53 commits between upstream master and local master. The plan covers bounded logging, immediate legacy configuration removal, STT test infrastructure, Gateway and Workshop lifecycle simplification, strict Realtime decoding, and validated sidecar recovery.
todos:
  - id: logging-bounds
    content: Bound logging memory, disk, ordering, loss reporting, redaction, and stalls
    status: pending
  - id: config-and-ci
    content: Retire the legacy STT configuration shim and stabilize native test infrastructure
    status: pending
  - id: lifecycle-structure
    content: Extract Gateway and Workshop lifecycle state machines and ratchet their tests
    status: pending
  - id: sidecar-boundary
    content: Validate sidecar capabilities and unify replacement and shutdown ownership
    status: pending
  - id: verify-removal
    content: Run focused, architecture, native, UI, and release exit gates
    status: pending
isProject: false
---

# PromptForge Attributable Debt Removal

## Product Requirements

- Repository: [promptforge](C:/Users/Vinnie/cursor/promptforge).
- Baseline: live `upstream/master` at `d539a6d90c5f1054e0917ccd74251ab3a6df7461`.
- Endpoint: local `master` at `5c80bbd8685f12378eed012290727e6195abb842`.
- Target: the exact 53-commit range `d539a6d90..5c80bbd8`.
- Worktree inclusion: none. The worktree was clean.
- Evidence: complete target commit messages and diffs, current code at the endpoint, [vibe/archdoc.md](C:/Users/Vinnie/cursor/promptforge/vibe/archdoc.md), [vibe/archdoc-next.md](C:/Users/Vinnie/cursor/promptforge/vibe/archdoc-next.md), [the Realtime STT plan](C:/Users/Vinnie/cursor/promptforge/vibe/2026-09-05-2-generic-realtime-stt.md), [the final STT design](C:/Users/Vinnie/cursor/promptforge/design/generic-realtime-stt.md), and [acceptance evidence](C:/Users/Vinnie/cursor/promptforge/design/generic-realtime-stt-acceptance.md).
- Analysis limits: static read-only analysis only. No tests, fault injection, native fixtures, external runner configuration, or deployment census ran. Practical collision rates and deployed legacy-config counts remain unknown.
- Cleanup goals:
  - Bound logging memory, disk, producer latency, and shutdown time with explicit loss reporting.
  - Move redaction before text formatting and preserve post-format scanning as defense in depth.
  - Delete the legacy `[workshop.stt]` compatibility paths and duplicated fixture logic.
  - Replace temporal lifecycle meshes with explicit transaction or reducer state.
  - Make sidecar validation a type-level precondition and use one authoritative Gateway identity snapshot.
  - Split oversized integration suites and ratchet both production and test surfaces.
- Non-goals:
  - Unrelated pre-existing debt.
  - A fifth production STT crate, a new speech protocol, or changed installed STT behavior.
  - Per-process Gateway bearer rotation. `[server].api_key` remains a configured long-term credential.
  - Reassigning Gateway, Workshop, or relay component ownership beyond the validated sidecar capability selected below.
  - Reworking log message content unrelated to bounds, ordering, loss, redaction, or retention.
- Success criteria:
  - Every retained debt ID has a concrete target state and a regression or architecture gate.
  - Logging has explicit byte, time, and disk budgets with observable truncation or loss.
  - Configuration version 2 accepts canonical top-level `[stt]` only, and the local installed configuration remains valid without rewriting.
  - Feature-enabled fixture API size is measured, temporary dead-code allowances are gone, and native CI validates an exact toolchain contract.
  - Gateway profile switching, Workshop Realtime parsing, dictation ownership, agent supervision, and sidecar recovery have explicit bounded state owners.
  - Existing public wire behavior, config version 2, installed behavior, and release gates remain green.

## Debt Inventory

- `PF-GWLOG-001` - introduced by `f303718e` in [gateway-logging/src/writer.rs](C:/Users/Vinnie/cursor/promptforge/crates/gateway-logging/src/writer.rs), [queue.rs](C:/Users/Vinnie/cursor/promptforge/crates/gateway-logging/src/queue.rs), and [redact.rs](C:/Users/Vinnie/cursor/promptforge/crates/gateway-logging/src/redact.rs). Record count is bounded but record and aggregate bytes are not. Impact: unbounded memory. Reversal cost: medium. Target: per-record and aggregate-byte limits with visible truncation or loss.
- `PF-GWLOG-002` - introduced by `f303718e` in [gateway-logging/src/queue.rs](C:/Users/Vinnie/cursor/promptforge/crates/gateway-logging/src/queue.rs). Sequence reservation happens before queue admission, so concurrent records can drain out of causal order. Impact: misleading chronology. Reversal cost: low. Target: assign sequence atomically with admission.
- `PF-GWLOG-003` - introduced by `f303718e` in [gateway-logging/src/queue.rs](C:/Users/Vinnie/cursor/promptforge/crates/gateway-logging/src/queue.rs) and [worker.rs](C:/Users/Vinnie/cursor/promptforge/crates/gateway-logging/src/worker.rs). Eviction summaries wait for a completely empty queue rather than the end of pressure. Impact: silent record loss. Reversal cost: low. Target: one summary per pressure episode after a defined low-water transition.
- `PF-GWLOG-004` - worsened by `1009b3f4` in [gateway-logging/src/config.rs](C:/Users/Vinnie/cursor/promptforge/crates/gateway-logging/src/config.rs) and [worker.rs](C:/Users/Vinnie/cursor/promptforge/crates/gateway-logging/src/worker.rs). Retention grew to five prior runs without a disk-byte bound. Impact: filesystem exhaustion. Reversal cost: medium. Target: fixed-size segments under one aggregate budget while preserving current diagnostic names.
- `PF-GWLOG-005` - introduced by `f303718e` across [queue.rs](C:/Users/Vinnie/cursor/promptforge/crates/gateway-logging/src/queue.rs), [runtime.rs](C:/Users/Vinnie/cursor/promptforge/crates/gateway-logging/src/runtime.rs), and [gateway/src/main.rs](C:/Users/Vinnie/cursor/promptforge/crates/gateway/src/main.rs). Protected-record producers, sink writes, and shutdown joins can block forever. Impact: frozen application threads or exit. Reversal cost: high. Target: finite waits followed by explicit loss, as selected by the operator.
- `PF-GWLOG-006` - worsened by `f303718e` and `1009b3f4` in [gateway-logging/src/redact.rs](C:/Users/Vinnie/cursor/promptforge/crates/gateway-logging/src/redact.rs). Text patterns do not cover all structured credentials, cookies, prompts, paths, payloads, or nested errors. Impact: persisted sensitive data. Reversal cost: medium to high. Target: typed field redaction before formatting plus adversarial post-format defense.
- `STT-CORE-001` - introduced by `3642d6dd` in [gateway-config/src/config/imp.rs](C:/Users/Vinnie/cursor/promptforge/crates/gateway-config/src/config/imp.rs) and [gateway-config-ui/ui/src/services/config-store.ts](C:/Users/Vinnie/cursor/promptforge/crates/gateway-config-ui/ui/src/services/config-store.ts). Rust and TypeScript indefinitely duplicate `[workshop.stt]` migration. Impact: compatibility drift and redundant parsing. Reversal cost: low for this pre-1.0 installation because the local file is already canonical. Target: config version 2 accepts only top-level `[stt]`; both migration shims are removed.
- `STT-CORE-002` - introduced by `c7c1c1f7` and expanded later across [gateway-stt-backend-whisper/src/prompt.rs](C:/Users/Vinnie/cursor/promptforge/crates/gateway-stt-backend-whisper/src/prompt.rs), [gateway-stt/src/test_fixtures/native.rs](C:/Users/Vinnie/cursor/promptforge/crates/gateway-stt/src/test_fixtures/native.rs), and native integration helpers. Five fixture resolvers can drift. Impact: inconsistent native gates. Reversal cost: low. Target: one feature-gated non-production resolver with explicit caller defaults.
- `STT-CORE-003` - introduced by `75f4cb30` and expanded through `25501883` in [gateway-stt-engine/src/test_fixtures.rs](C:/Users/Vinnie/cursor/promptforge/crates/gateway-stt-engine/src/test_fixtures.rs) and [gateway-stt/src/test_fixtures.rs](C:/Users/Vinnie/cursor/promptforge/crates/gateway-stt/src/test_fixtures.rs). The feature-gated fixture API grows outside public-root ratchets. Impact: quasi-public test contract constrains refactors. Reversal cost: medium. Target: feature-enabled public API accounting followed by scenario-level narrowing.
- `STT-CORE-004` - introduced by `4b490073` and `101dedac` in [gateway-stt/src/lib.rs](C:/Users/Vinnie/cursor/promptforge/crates/gateway-stt/src/lib.rs). Module-wide dead-code allowances outlived production wiring. Impact: obsolete code can accumulate silently. Reversal cost: low. Target: remove broad allowances and retain only justified item-level exceptions.
- `STT-CORE-005` - introduced by `b6021e4c` in [.github/workflows/stt-miri.yml](C:/Users/Vinnie/cursor/promptforge/.github/workflows/stt-miri.yml). Native CI depends on floating `stable` and a service-account Cargo layout. Impact: unreproducible runner failures. Reversal cost: medium. Target: exact toolchain and versioned runner provisioning contract.
- `PF-RTSTT-DC-001` - worsened by `467a2622`, `60165006`, and `1b50919d` in [gateway/src/lib.rs](C:/Users/Vinnie/cursor/promptforge/crates/gateway/src/lib.rs) and [config_write.rs](C:/Users/Vinnie/cursor/promptforge/crates/gateway/src/config_write.rs). Profile-switch phases and rollback state remain concentrated in the 5,000-line root module. Impact: temporal coupling across every runtime participant. Reversal cost: medium to high. Target: a private transaction module with explicit phase values.
- `PF-RTSTT-DC-002` - introduced by `467a2622` in [gateway/src/config_write.rs](C:/Users/Vinnie/cursor/promptforge/crates/gateway/src/config_write.rs). PID plus process-local sequence temporary names can collide with crash residue after PID reuse. Impact: valid profile switches can fail. Reversal cost: low. Target: high-entropy process nonce with bounded create-new retry.
- `PF-RTSTT-DC-003` - introduced by `7452751b` and worsened by `94357c39` in [realtime-transcription.ts](C:/Users/Vinnie/cursor/promptforge/crates/workshop-server/ui/src/services/realtime-transcription.ts). Production validates only fields it consumes while test fixtures enforce the full frozen event shape. Impact: production and canonical contract drift. Reversal cost: medium. Target: one exhaustive pure decoder used by production and fixture tests.
- `PF-RTSTT-DC-004` - introduced by `7452751b` and worsened by `aeec7b48` in [realtime-stt.ts](C:/Users/Vinnie/cursor/promptforge/crates/workshop-server/ui/src/ui/realtime-stt.ts). Five collections, lifecycle flags, capture state, and editor offsets are coordinated in one callback mesh. Impact: stale ownership and rollback defects. Reversal cost: medium. Target: a pure `TakeRegistry` reducer emitting editor and capture effects.
- `PF-RTSTT-DC-005` - introduced by `fb4e0bfe` and expanded by `13fb8eef` in [session_agents/supervisor.rs](C:/Users/Vinnie/cursor/promptforge/crates/workshop-server/src/session_agents/supervisor.rs) and [lifecycle.rs](C:/Users/Vinnie/cursor/promptforge/crates/workshop-server/src/session_agents/lifecycle.rs). Run completion, catalog replacement, Gateway replacement, cancellation, and accepted-turn settlement scale as branch interactions. Impact: exactly-once settlement risk. Reversal cost: medium. Target: explicit supervisor events and transition reducer.
- `PF-RTSTT-DC-006` - worsened across `1b50919d`, `06cba48a`, `6ce38729`, and `49441166` in [gateway/tests/it/realtime_stt.rs](C:/Users/Vinnie/cursor/promptforge/crates/gateway/tests/it/realtime_stt.rs), [realtime_relay.rs](C:/Users/Vinnie/cursor/promptforge/crates/workshop-server/tests/it/realtime_relay.rs), and [chat_gate.rs](C:/Users/Vinnie/cursor/promptforge/crates/workshop-server/tests/it/chat_gate.rs). Integration suites reached 1,778, 680, and 1,098 lines outside ratchets. Impact: coupled fixtures and hard-to-localize failures. Reversal cost: low. Target: concern-based files plus test-file ceilings.
- `DC-PF-P2-001` - introduced by `13fb8eef` in [workshop/src/gateway.rs](C:/Users/Vinnie/cursor/promptforge/crates/workshop/src/gateway.rs) and [main.rs](C:/Users/Vinnie/cursor/promptforge/crates/workshop/src/main.rs). Supervisor shutdown signals then abandons its thread and owned blocking work. Impact: post-teardown probing, launch, or publication. Reversal cost: medium. Target: cancellation-aware probes and a finite joined shutdown.
- `DC-PF-P2-002` - introduced by `13fb8eef` in [gateway_binding.rs](C:/Users/Vinnie/cursor/promptforge/crates/workshop-server/src/gateway_binding.rs) and [serve.rs](C:/Users/Vinnie/cursor/promptforge/crates/workshop-server/src/serve.rs). Public updater accepts a raw connection file while validation lives only in one caller. Impact: public trust-boundary bypass. Reversal cost: high. Target: updater accepts an unforgeable validated-connection capability.
- `DC-PF-P2-003` - worsened by `13fb8eef` across [gateway_binding.rs](C:/Users/Vinnie/cursor/promptforge/crates/workshop-server/src/gateway_binding.rs), [workshop/src/main.rs](C:/Users/Vinnie/cursor/promptforge/crates/workshop/src/main.rs), and [menu.rs](C:/Users/Vinnie/cursor/promptforge/crates/workshop/src/menu.rs). Client consumers and quit handling publish Gateway identity in two stores. Impact: quit can target a retired process and leave the replacement alive. Reversal cost: medium. Target: one validated authoritative snapshot for clients and shutdown.
- `DC-PF-P2-004` - worsened by `13fb8eef` in [workshop/src/gateway.rs](C:/Users/Vinnie/cursor/promptforge/crates/workshop/src/gateway.rs). Boot planning, launch, validation, identity, supervision, recovery, and tests occupy 854 lines. Impact: broad review and regression boundary. Reversal cost: low. Target: extract supervision and identity into private modules with ceilings.
- `DC-PF-P2-005` - worsened by `13fb8eef` across [ui/src/ui/stt.ts](C:/Users/Vinnie/cursor/promptforge/crates/workshop-server/ui/src/ui/stt.ts), [realtime-stt.ts](C:/Users/Vinnie/cursor/promptforge/crates/workshop-server/ui/src/ui/realtime-stt.ts), and [prompt-input.ts](C:/Users/Vinnie/cursor/promptforge/crates/workshop-server/ui/src/ui/prompt-input.ts). Every input adapter exposes document-end and plain-text details for composition policy. Impact: editor representation leaks into Realtime lifecycle code. Reversal cost: medium. Target: one target-owned insertion-context operation carrying anchor, original text, and required prefix.

## Technical Design

- Logging settlement for `PF-GWLOG-001` through `PF-GWLOG-006`:
  - Add one immutable limits object covering maximum formatted record bytes, aggregate queued bytes, producer wait, shutdown wait, segment bytes, and aggregate retained bytes.
  - Format into a bounded writer. Truncate at a valid text boundary with an explicit marker, or reject the record and increment the same observable loss episode.
  - Assign sequence under the queue mutex at successful admission. Track queued bytes with record counts and preserve lane priority within that one admission order.
  - End a pressure episode at a defined low-water transition, not only at empty, and enqueue exactly one summary containing dropped and truncated counts.
  - Apply the selected bounded-loss contract: protected producers wait only for the configured budget, then record loss through a preallocated counter path. Runtime shutdown waits only for its budget, records an emergency diagnostic when possible, and detaches without an unbounded join.
  - Redact structured tracing fields by classified field name and secret type before formatting. Keep the bounded textual scanner for dependency errors and unstructured messages.
  - Rotate fixed-size `gateway.log` segments under one aggregate byte budget while preserving `gateway.log` and numbered diagnostic names. Reserve enough segment space for a truncation marker and terminal record, and prune oldest segments before admitting a new one.
- Configuration settlement for `STT-CORE-001`:
  - Keep `config-version = 2`.
  - Delete Rust `migrate_legacy_stt` and TypeScript `canonicalizeStt`.
  - Reject `[workshop.stt]` through the existing unknown-field validation instead of rewriting it.
  - Keep canonical top-level `[stt]` parsing and UI serialization unchanged.
  - Verify `C:\Users\Vinnie\.promptforge\gateway.toml` contains no legacy section, perform no write to it, and leave it byte-for-byte unchanged.
- Test infrastructure and CI settlement for `STT-CORE-002` through `STT-CORE-005` and `PF-RTSTT-DC-006`:
  - Centralize native fixture resolution behind the existing feature-gated STT test infrastructure rather than adding a production crate. Preserve caller-specific fallback roots as explicit parameters.
  - Generate and ratchet feature-enabled public API snapshots for both STT fixture surfaces. Narrow low-level synchronization controls to scenario-level operations only after current consumers are inventoried.
  - Remove module-wide dead-code allowances and fix or annotate only genuinely configuration-specific items.
  - Pin an exact native Rust toolchain in the workflow and validate a versioned self-hosted runner contract before cache or test work.
  - Split Gateway Realtime, Workshop relay, and chat integration suites by authentication, protocol, lifecycle, recovery, overload, and canonical sequence. Add physical-line ceilings and preserve discovered test counts.
- Gateway lifecycle settlement for `PF-RTSTT-DC-001` and `PF-RTSTT-DC-002`:
  - Extract a private `profile_switch` transaction that owns target profile, cancellation token, prepared persistence, old runtime snapshot, staged routing and speech replacements, and terminal outcome.
  - Represent prepared, cutover, staged, and committed phases as values so invalid rollback or publication order cannot be called.
  - Keep existing locks and external behavior while moving persistence and rollback helpers out of the root module.
  - Name preparation files with a process-random nonce and bounded `create_new` retry. Never delete residue unless ownership is proven.
- Workshop protocol and state settlement for `PF-RTSTT-DC-003` through `PF-RTSTT-DC-005` and `DC-PF-P2-005`:
  - Extract a pure exhaustive Realtime event decoder returning a discriminated union. Validate exact required fields, nullable fields, IDs, content index, revision, transcript partition, audio spans, completion usage, and unsupported event types. Drive both production and canonical fixture mutation tests through it.
  - Replace the callback-owned dictation maps and flags with a pure `TakeRegistry` transition reducer. Inputs are typed service events and user actions; outputs are editor, capture, status, and wire effects.
  - Move insertion policy into `SttInputTarget::insertionContext`, returning the selected range, original text, and immutable composition prefix. The registry never reads document structure directly.
  - Model agent supervision with explicit events for run completion, catalog generation, Gateway generation, operator cancellation, accepted input, and terminal settlement. A pure transition function decides wait, cancel, preserve, relaunch, or close effects.
- Sidecar settlement for `DC-PF-P2-001` through `DC-PF-P2-004`:
  - Add a public but unforgeable `ValidatedConnection` capability in `shared-sidecar`. Constructors remain private; validation proves process image, boot identity, health, and bearer acceptance. Expose redacted accessors needed to build a consumer snapshot.
  - Change `GatewayUpdater` to accept only `ValidatedConnection`. Remove raw `ConnectionFile` publication from the public Workshop server API.
  - Store the validated connection identity in the same immutable `GatewayBinding` snapshot as HTTP and model clients. Route quit through the current authoritative snapshot and remove the separate `GatewaySlot`.
  - Make resolve, validation, wait, and launch loops cancellation-aware. `GatewaySupervisor` owns and joins its thread under a finite shutdown budget, and publication is impossible after cancellation.
  - Split boot planning and one-shot launch from continuous supervision, identity, and recovery tests. Ratchet each resulting module.

## Testing Plan

- `PF-GWLOG-001`, `PF-GWLOG-003`, and `PF-GWLOG-005`: inject oversized records, variable-size concurrent pressure, a permanently stalled sink, and shutdown during saturation. Assert strict peak bytes, bounded producer and exit latency, one summary per episode, and explicit loss or truncation.
- `PF-GWLOG-002`: pause one producer before admission and prove successful admission sequence is global write order.
- `PF-GWLOG-004`: exceed segment and aggregate budgets across active and retained logs. Assert current diagnostic names, oldest-first pruning, terminal-record preservation, and total bytes at or below budget.
- `PF-GWLOG-006`: adversarial structured and textual credentials, Basic and Bearer authorization, cookies, URLs, multiline errors, prompts, paths, request bodies, and nested chains must persist no protected values.
- `STT-CORE-001`: version 2 canonical `[stt]` parsing, legacy `[workshop.stt]` rejection, mixed-form rejection, canonical UI round-trip, absence of browser canonicalization, and byte-for-byte local configuration preservation.
- `STT-CORE-002` and `STT-CORE-003`: every native test target must resolve identical explicit fixtures and retain caller fallbacks; feature-enabled public API snapshots must fail on unreviewed growth; default builds must expose no fixture symbols.
- `STT-CORE-004`: default, all-feature, test, Miri, and featureless lint configurations pass with dead-code diagnostics active.
- `STT-CORE-005`: native preflight accepts only the pinned toolchain and versioned runner layout, rejects wrong or missing versions before cache use, and runs all native Whisper jobs on the self-hosted runner.
- `PF-RTSTT-DC-001` and `PF-RTSTT-DC-002`: retain every profile-switch cancellation, rollback, indeterminate persistence, atomic publication, and featureless test; add deterministic temporary-name collisions and crash residue.
- `PF-RTSTT-DC-003`: mutate each required and forbidden Realtime field in production decoding, then replay every canonical sequence through the same decoder.
- `PF-RTSTT-DC-004` and `DC-PF-P2-005`: reducer invariants cover overlap, tombstones, precommit binding, rollback, reconnect, sequential spacing, completion authority, selection replacement, textarea, and ProseMirror.
- `PF-RTSTT-DC-005`: transition tables cover delayed catalog, profile and Gateway replacement during accepted input, operator cancel, retained history, close, and exactly-once settlement.
- `PF-RTSTT-DC-006` and `DC-PF-P2-004`: test count before and after every split is identical; new source and test ceilings pass.
- `DC-PF-P2-001`: block each sidecar resolve, validation, launch, and health phase, request Workshop exit, and prove joined termination within budget with no later publication.
- `DC-PF-P2-002` and `DC-PF-P2-003`: raw connection files cannot publish; wrong image, boot identity, health, or bearer cannot create a capability; same-port and same-key replacement works; configured-key replacement is atomic; replacement raced with quit targets one current generation.
- Exit checks: repository formatting, warnings-denied workspace lint, workspace tests, documentation tests, architecture gates, feature-enabled API snapshots, native Whisper, both Miri targets, both UI suites, guide generation cleanliness, unsigned local package recovery, and existing signed release CI.

## Decision Record

- Scope correction: the tracked branch is `origin/master`, but the requested upstream baseline is the separate `upstream/master` remote at `d539a6d90`. The live remote was verified without changing local refs.
- Logging stall policy: bounded producer and shutdown waits with explicit loss. Rejected indefinite protected-record retention because it can freeze arbitrary threads and process exit. Rejected an emergency spool because it creates another sink and budget lifecycle.
- Sidecar trust boundary: an unforgeable validated-connection capability. Rejected caller-only validation because the public updater remains forgeable. Rejected moving all supervision into `workshop-server` because it changes component ownership more broadly.
- Legacy configuration: keep version 2 and remove `[workshop.stt]` support immediately. The repository is pre-1.0 and the local installation is already canonical, so no migration mechanism or new schema version is justified. Rejected automatic migration, an operator command, and a deprecation window because each preserves compatibility machinery that this installation does not need.
- Log retention: fixed-size segments under an aggregate byte budget while retaining current names. Rejected per-run discard because late terminal diagnostics could be lost. Rejected prune-only run rotation because the active file remains unbounded.
- Reversible decisions:
  - Define queue chronology as successful admission order.
  - Use typed structural redaction first and bounded text scanning second.
  - Centralize native fixtures in existing feature-gated test infrastructure, not a new production crate.
  - Extract internal transaction and reducer modules without changing wire or installed behavior.
  - Split tests before adding ceilings so counts prove semantic preservation.
- Assumptions and risks:
  - Other unpublished installations using `[workshop.stt]` will fail validation after removal. That break is intentional for the selected pre-1.0 scope.
  - Bounded logging deliberately permits loss during permanent sink stalls; summaries and emergency diagnostics are part of the contract.
  - A validated capability expands `shared-sidecar` public API but narrows Workshop mutation authority.
  - External runner provisioning may already pin Rust; repository checks must match the actual service image before enforcement.

## Project survey

- Build commands:
  - Prerequisite: Rust 1.89 (pinned in `rust-toolchain.toml` and workspace `rust-version`) and Node.js 22; run `npm ci` once per checkout in `crates/workshop-server/ui` and `crates/gateway-config-ui/ui`.
  - `cargo build` builds the default workspace member `gateway`, including default-on `config-ui`, `local`, `web-search`, and `stt` features.
  - `cargo build -p workshop` builds the Tauri desktop product and its in-process `workshop-server`.
  - UI bundles build independently with `npm run build` in each UI directory; crate `build.rs` scripts invoke esbuild and place bundles in `OUT_DIR` (nothing UI-built is checked in).
  - `cargo run -p build-user-guide` regenerates `guide/src/SUMMARY.md`, per-part landing pages, and the four single-file guide exports.
- Focused test command patterns:
  - Rust unit or named test: `cargo test -p <crate> <test-name>`.
  - Rust integration harness: `cargo test -p <crate> --test it <test-name>`. Gateway, `gateway-stt`, and `workshop-server` use `tests/it/main.rs` as the harness with responsibility-named modules below it.
  - STT architecture gates: `node tools/check-stt-architecture.test.mjs`, `node tools/check-stt-architecture.mjs`, and `cargo test -p gateway-stt --test it architecture`.
  - STT feature-gated fixtures: `cargo test -p gateway-stt -F test-fixtures`, `cargo test -p gateway-stt-engine -F test-fixtures`, `cargo test -p gateway-stt-backend-whisper -F test-fixtures`.
  - Native Whisper tests are `#[ignore]` by default: same package command with `-- --ignored --test-threads=1`; require `PROMPTFORGE_WHISPER_LIBRARY` (or model/audio overrides) plus gitignored fixtures under `local/stt-fixtures/` or caller-specific fallback roots.
  - Miri (pure STT ownership): `cargo +nightly-2026-09-05 miri test -p gateway-stt-engine -F test-fixtures miri_` and the same for `gateway-stt`.
  - Workshop UI focused tests run from `crates/workshop-server/ui`, for example `node --test test/stt-stream.mjs`; package discovery is `npm test`.
  - Config UI focused tests run from `crates/gateway-config-ui/ui` with `node --test src/<area>.test.mjs`; `npm test` runs the full discovered suite after `pretest` runs `check-layers.mjs`.
  - Node repository tools: `node --test tools/check-stt-architecture.test.mjs`, `node tools/check-stt-native-workflow.test.mjs`, and `node tools/stage-gateway-sidecar.test.mjs`.
  - Gateway-logging latency budget: `cargo test -p gateway-logging --release -- --ignored`.
- Full-suite test commands:
  - Rust workspace (CI Linux split): `cargo test --locked --workspace --exclude workshop --exclude workshop-server --all-features`, then `cargo test --locked -p workshop -p workshop-server` on Windows after staging the gateway sidecar with `node tools/stage-gateway-sidecar.mjs`.
  - Workshop UI: from `crates/workshop-server/ui`, run `npm run typecheck`, `npm run build`, then `npm test` as separate commands.
  - Config UI: from `crates/gateway-config-ui/ui`, run `npm run typecheck`, `npm run build`, then `npm test` (tests import built `dist/app.js`, so build precedes test).
  - MSRV job: `cargo build --locked --workspace --exclude workshop --exclude workshop-server --all-features` and `cargo test --locked --workspace --exclude workshop --exclude workshop-server --all-features` on Rust 1.89.0.
- Linter and formatter commands:
  - Rust formatting: `cargo fmt --all --check`.
  - Rust linting: `cargo clippy --workspace --all-targets --all-features -- -D warnings`; CI excludes `workshop` and `workshop-server` in the Linux job and lints those two packages separately on Windows.
  - Documentation gates: `cargo test --workspace --all-features --doc` and `RUSTDOCFLAGS=-D warnings cargo doc --workspace --no-deps --all-features`.
  - Feature boundary gate: `cargo check -p gateway --no-default-features`.
  - Supply chain: `cargo deny check` and `cargo audit`.
  - Workshop UI layering and types: `npm run typecheck` (`tsc --noEmit` plus `check-layers.mjs`). Config UI runs `check-layers.mjs` through `npm test`. Neither UI package defines a standalone formatter command.
- Test placement and naming:
  - Rust unit tests are colocated in source modules under `#[cfg(test)]`; async tests use `#[tokio::test]`.
  - Cross-module and socket tests live under `tests/it/`, with shared fixtures in `tests/common/`. Test function names are lower snake case behavior statements.
  - Native, Miri-filtered, and large-download tests are explicitly `#[ignore]` or gated with `#[cfg(not(miri))]` and name their required fixture or live dependency.
  - Workshop UI tests are either `ui/test/**/*.mjs` or colocated `ui/src/**/*.test.mjs`. Names are plain English behavior statements; disposable-owning tests use `test/helpers/leak-check.mjs`.
  - Node repository tool tests colocate as `tools/*.test.mjs` beside their drivers.
  - Module size ratchets: `module-ceilings.toml` in `gateway-stt`, `gateway-stt-engine`, `gateway-stt-backend-whisper`, `gateway-whisper-ffi`, and `workshop-server`, enforced by crate integration tests (for example `cargo test -p workshop-server --test it ratchet`).
- Directory map:
  - `.cargo/` holds repository Cargo configuration; `.github/` holds CI, release, nightly, STT/Miri, and guide workflows plus reusable actions.
  - `crates/` is the product and library workspace. Gateway speech code lives in `gateway-stt`, `gateway-stt-engine`, `gateway-stt-backend-whisper`, and `gateway-whisper-ffi`.
  - Gateway product crates: `gateway`, `gateway-config`, `gateway-config-ui`, `gateway-local`, `gateway-logging`, `gateway-routing`, `gateway-stt`, `gateway-web-search`, `gateway-whisper-ffi`.
  - Workshop product crates: `workshop`, `workshop-server`; the browser application is `crates/workshop-server/ui`; config UI sources are `crates/gateway-config-ui/ui`.
  - Cross-product substrate: `shared-loopback`, `shared-progress`, `shared-protocol`, `shared-sidecar`, and the non-Rust `shared-ui` package.
  - PromptForge library crates use the `promptforge-*` prefix; `build-*` crates (`build-ui`, `build-user-guide`, `build-llama-cuda`) are compile-time or CI tooling linked into no deliverable.
  - `design/` holds design material; `guide/` holds mdBook documentation; `local/` holds gitignored developer fixtures; `prompts/` holds prompt programs; `tools/` holds repository Node gates; `vibe/` holds execution plans, `vibe/archdoc.md`, and the architecture queue.
- Component boundaries (from `vibe/archdoc.md` and current manifests):
  - executor (`promptforge`, `promptforge-core`, parser, store, Lua, agent, tools): depends on gateway protocol, store, shared substrate.
  - gateway (`gateway`, `gateway-config`, `gateway-local`, `gateway-logging`, `gateway-routing`, `gateway-stt`, `gateway-web-search`): independent server process; sole holder of vendor credentials (A2); depends on shared substrate only among cross-product crates.
  - workshop UI (`workshop`, `workshop-server`, `workshop-server/ui`): desktop shell hosts `workshop-server` in-process and attaches to gateway through `shared-sidecar`; depends on executor support crates and shared substrate, not on `gateway-stt`.
  - STT stack: `gateway-stt` orchestrates HTTP/WebSocket speech routes and session state; depends on `gateway-stt-engine` (backend-neutral workers) and `gateway-stt-backend-whisper` (Whisper policy), which depends on `gateway-whisper-ffi` (runtime-loaded ABI leaf). No STT crate depends on `workshop-server`.
  - Realtime paths at endpoint: `gateway/tests/it/realtime_stt.rs`, `gateway-stt/tests/it/realtime_session.rs` and `realtime_fixtures.rs`, `workshop-server/src/routes/realtime.rs`, `workshop-server/tests/it/realtime_relay.rs` and `chat_gate.rs`, `workshop-server/ui/src/services/realtime-transcription.ts`, `workshop-server/ui/src/ui/realtime-stt.ts`.
  - Sidecar seam: `shared-sidecar` is the sole connection-file implementation; gateway writes, workshop and workshop-server read.
  - Logging: `gateway-logging` is consumed only by `gateway`; queue, rotation, redaction, and worker thread stay inside that crate.
  - Config: `gateway-config` owns validated TOML; version 2 uses top-level `[stt]` with a legacy `[workshop.stt]` migration shim still present in Rust and the config UI (debt target for this plan).
- Visible conventions:
  - Crate prefixes encode product membership (`gateway*`, `promptforge*`, bare `workshop*`, `shared-*`, `build-*`). Shared dependencies must live in `shared-*`; build-only tooling in `build-*`.
  - Cargo features gate real constraints only. Gateway `local`, `web-search`, `config-ui`, and `stt` features are additive and default on; `cargo check -p gateway --no-default-features` must stay green.
  - Rust modules are private by default with deliberate crate-root re-exports. Every public item requires rustdoc; libraries use typed errors; behavior changes ship with tests in the same change.
  - Runtime paths never compile native code. Whisper loads from packaged runtime artifacts; unsafe, ABI layouts, and raw pointers stay in `gateway-whisper-ffi`.
  - Workshop server route groups expose `fn routes(state) -> Router`; `app.rs` composes them. One task owns each ordinary socket; request and session errors are values; in-process tests use `Router::oneshot` or spawn fixtures.
  - Workshop UI imports flow `ui -> services -> base`; `main.ts` is the composition root. The rule is enforced by `check-layers.mjs` during build, typecheck, and Cargo bundling.
  - Generated UI bundles and architecture-owned guide files are never checked in. STT public surfaces and module sizes are ratcheted through `tools/check-stt-architecture.mjs` and per-crate `module-ceilings.toml`.
  - Plans and nested `AGENTS.md` files bind sub-agents; root `AGENTS.md` states workspace-wide rules that nested files do not restate.
- Rules manifest:
  - `AGENTS.md` governs the repository root.
  - `crates/gateway/AGENTS.md` governs `crates/gateway/`.
  - `crates/gateway-config/AGENTS.md` governs `crates/gateway-config/`.
  - `crates/gateway-local/AGENTS.md` governs `crates/gateway-local/`.
  - `crates/gateway-logging/AGENTS.md` governs `crates/gateway-logging/`.
  - `crates/gateway-routing/AGENTS.md` governs `crates/gateway-routing/`.
  - `crates/gateway-stt/AGENTS.md` governs `crates/gateway-stt/`.
  - `crates/gateway-stt-engine/AGENTS.md` governs `crates/gateway-stt-engine/`.
  - `crates/gateway-stt-backend-whisper/AGENTS.md` governs `crates/gateway-stt-backend-whisper/`.
  - `crates/gateway-web-search/AGENTS.md` governs `crates/gateway-web-search/`.
  - `crates/gateway-whisper-ffi/AGENTS.md` governs `crates/gateway-whisper-ffi/`.
  - `crates/promptforge/AGENTS.md` governs `crates/promptforge/`.
  - `crates/promptforge-agent/AGENTS.md` governs `crates/promptforge-agent/`.
  - `crates/promptforge-core/AGENTS.md` governs `crates/promptforge-core/`.
  - `crates/promptforge-core-support/AGENTS.md` governs `crates/promptforge-core-support/`.
  - `crates/promptforge-lua/AGENTS.md` governs `crates/promptforge-lua/`.
  - `crates/promptforge-model-client/AGENTS.md` governs `crates/promptforge-model-client/`.
  - `crates/promptforge-parser/AGENTS.md` governs `crates/promptforge-parser/`.
  - `crates/promptforge-store/AGENTS.md` governs `crates/promptforge-store/`.
  - `crates/promptforge-tools/AGENTS.md` governs `crates/promptforge-tools/`.
  - `crates/promptforge-web-search/AGENTS.md` governs `crates/promptforge-web-search/`.
  - `crates/promptforge-webfetch/AGENTS.md` governs `crates/promptforge-webfetch/`.
  - `crates/shared-loopback/AGENTS.md` governs `crates/shared-loopback/`.
  - `crates/shared-progress/AGENTS.md` governs `crates/shared-progress/`.
  - `crates/shared-protocol/AGENTS.md` governs `crates/shared-protocol/`.
  - `crates/shared-sidecar/AGENTS.md` governs `crates/shared-sidecar/`.
  - `crates/shared-ui/AGENTS.md` governs `crates/shared-ui/`.
  - `crates/workshop/AGENTS.md` governs `crates/workshop/`.
  - `crates/workshop/icons/AGENTS.md` additionally governs `crates/workshop/icons/`.
  - `crates/workshop-server/AGENTS.md` governs `crates/workshop-server/`.
  - `crates/workshop-server/ui/AGENTS.md` additionally governs `crates/workshop-server/ui/`.

## Execution Instructions

### Step 1: Split Gateway Realtime integration coverage [completed]

- Component and piece: Component 1 of 8, regression boundaries; first split the Gateway Realtime suite by authentication, protocol, lifecycle, recovery, overload, and canonical sequence while preserving every discovered test.
- Dependency: starts from the plan seed because later Gateway and Realtime refactors need stable concern-level test homes and a recorded pre-refactor test count.
- Debt IDs: `PF-RTSTT-DC-006`.
- Artifacts: `crates/gateway/tests/it/realtime_stt.rs`, `crates/gateway/tests/it/realtime_stt/*.rs`, `crates/gateway/tests/it/main.rs`, and focused support extracted only when shared by the new files.
- Scope: move tests without changing assertions, fixtures, ignored status, or production behavior; verify the discovered test count before and after the split without adding a persistent ratchet yet.
- Exclusions: no profile-switch, decoder, fixture-resolution, or production changes; unrelated defects are recorded separately.
- Focused verification: from the repository root run `cargo test -p gateway`; compare the ratchet's recorded count with the passing discovered suite.

### Step 2: Split Workshop relay integration coverage [completed]

- Component and piece: Component 1 of 8, regression boundaries; split Workshop `realtime_relay` and `chat_gate` coverage by authentication, protocol, lifecycle, recovery, overload, and canonical sequence while preserving every discovered test.
- Dependency: depends on Step 1 only for one consistent count-preserving split convention; it must precede Workshop decoder, reducer, supervisor, and sidecar changes so moved assertions retain stable ownership.
- Debt IDs: `PF-RTSTT-DC-006`.
- Artifacts: `crates/workshop-server/tests/it/realtime_relay.rs`, `crates/workshop-server/tests/it/realtime_relay/*.rs`, `crates/workshop-server/tests/it/chat_gate.rs`, `crates/workshop-server/tests/it/chat_gate/*.rs`, and `crates/workshop-server/tests/it/main.rs`.
- Scope: move tests and narrowly shared fixtures without semantic edits; verify exact before and after counts for both source suites without adding persistent ratchets yet.
- Exclusions: no production relay, session-agent, UI, Gateway binding, or sidecar behavior changes.
- Focused verification: from the repository root run `cargo test -p workshop-server`.

### Step 3: Enforce integration test file ceilings [completed]

- Component and piece: Component 1 of 8, regression boundaries; add one repository gate for physical-line ceilings and exact test-count records for the three split suites.
- Dependency: depends on Steps 1 and 2 because the selected decision is to split first, prove count preservation, and only then freeze the resulting concern boundaries.
- Debt IDs: `PF-RTSTT-DC-006`.
- Artifacts: create `tools/check-integration-test-ceilings.mjs`, `tools/check-integration-test-ceilings.test.mjs`, and `tools/integration-test-ceilings.json` covering `crates/gateway/tests/it/realtime_stt/`, `crates/workshop-server/tests/it/realtime_relay/`, and `crates/workshop-server/tests/it/chat_gate/`; wire the gate in `.github/workflows/ci.yml`.
- Scope: enforce physical-line ceilings, exact manifest coverage, and recorded test totals with path-normalized tests.
- Exclusions: do not impose ceilings on unrelated suites or alter any production module ceiling.
- Focused verification: from the repository root run `node tools/check-integration-test-ceilings.test.mjs`, `node tools/check-integration-test-ceilings.mjs`, `cargo test -p gateway`, and `cargo test -p workshop-server`.
- Component boundary: ends Component 1; review cumulative Steps 1 through 3 against the pre-Step-1 base.

### Step 4: Bound formatted logging records [completed]

- Component and piece: Component 2 of 8, `gateway-logging`; establish one immutable limits object and bounded record formatting with a valid-text truncation marker.
- Dependency: depends on Step 3 only as the completed regression foundation; within logging it is first because queue, wait, shutdown, and segment budgets consume the same limits object.
- Debt IDs: `PF-GWLOG-001`, with contract input for `PF-GWLOG-004` and `PF-GWLOG-005`.
- Artifacts: `crates/gateway-logging/src/config.rs`, `writer.rs`, `queue.rs`, `lib.rs`, and their unit tests.
- Scope: define maximum formatted record bytes, aggregate queued bytes, producer wait, shutdown wait, segment bytes, and aggregate retained bytes; bound formatting and count rejected or truncated records in the same observable loss episode.
- Exclusions: no queue-order change, producer timeout, disk rotation, or redaction expansion yet; log message content otherwise stays unchanged.
- Focused verification: from the repository root run `cargo test -p gateway-logging`.

### Step 5: Order and account the logging queue [completed]

- Component and piece: Component 2 of 8, `gateway-logging`; make queue admission enforce aggregate bytes, assign sequence under the mutex, and close one pressure episode at a defined low-water transition.
- Dependency: depends on Step 4 because admission must use the shared record and aggregate byte limits and its loss accounting; it precedes timeout work because wait outcomes need final admission semantics.
- Debt IDs: `PF-GWLOG-001`, `PF-GWLOG-002`, `PF-GWLOG-003`.
- Artifacts: `crates/gateway-logging/src/queue.rs`, `writer.rs`, and queue concurrency tests.
- Scope: preserve lane priority inside one successful-admission order, enforce strict peak queued bytes, and emit exactly one summary with dropped and truncated counts per pressure episode.
- Exclusions: no indefinite retention guarantee, sink implementation change, segment rotation, or redaction work.
- Focused verification: from the repository root run `cargo test -p gateway-logging`.

### Step 6: Bound logging stalls and shutdown [completed]

- Component and piece: Component 2 of 8, `gateway-logging`; apply the selected bounded-loss policy to protected producers, sink stalls, and runtime shutdown.
- Dependency: depends on Step 5 because finite waits must terminate in the queue's explicit loss-accounting path and preserve successful-admission order.
- Debt IDs: `PF-GWLOG-005`, plus `PF-GWLOG-003` loss observability.
- Artifacts: `crates/gateway-logging/src/queue.rs`, `worker.rs`, `runtime.rs`, `crates/gateway/src/main.rs`, and stalled-sink and saturated-shutdown tests.
- Scope: protected producers wait only for the configured budget and then record loss through a preallocated path; shutdown waits only for its budget, attempts an emergency diagnostic, and detaches rather than joining forever.
- Exclusions: no emergency spool, no unbounded protected-record retention, no unrelated Gateway shutdown redesign, and no claim of lossless logging during a permanent stall.
- Focused verification: from the repository root run `cargo test -p gateway-logging` and `cargo test -p gateway`.

### Step 7: Rotate fixed-size log segments [completed]

- Component and piece: Component 2 of 8, `gateway-logging`; replace run-count-only retention with fixed-size segments under one aggregate disk-byte budget.
- Dependency: depends on Step 4 for segment and aggregate budgets and on Step 5 for bounded terminal records; it is independent of Step 6 behavior but follows it to avoid overlapping worker and runtime edits.
- Debt IDs: `PF-GWLOG-004`.
- Artifacts: `crates/gateway-logging/src/config.rs`, `worker.rs`, `runtime.rs`, and rotation tests.
- Scope: retain `gateway.log` and numbered diagnostic names, reserve marker and terminal-record space, prune oldest segments before admission, and prove active plus retained bytes stay within budget.
- Exclusions: no per-run discard, no prune-only active-file strategy, and no rename of diagnostic files.
- Focused verification: from the repository root run `cargo test -p gateway-logging`.

### Step 8: Redact structured logging fields [completed]

- Component and piece: Component 2 of 8, `gateway-logging`; classify and redact structured fields and secret types before formatting, with the bounded textual scanner retained as defense in depth.
- Dependency: depends on Step 4 because pre-format output must honor the bounded writer and on Step 5 because rejected or truncated records share loss accounting; it follows Steps 6 and 7 to minimize conflicting edits.
- Debt IDs: `PF-GWLOG-006`.
- Artifacts: `crates/gateway-logging/src/redact.rs`, `writer.rs`, `lib.rs`, and their adversarial redaction tests.
- Scope: cover Basic and Bearer authorization, cookies, URLs, prompts, paths, payloads, multiline and nested errors, and classified credential fields without persisting protected values.
- Exclusions: no unrelated log wording changes and no unbounded scanner or second sink.
- Focused verification: from the repository root run `cargo test -p gateway-logging`, `cargo test -p gateway`, and `cargo test -p gateway-logging --release -- --ignored` for the existing latency budget.
- Component boundary: ends Component 2; review cumulative Steps 4 through 8 against the Step 3 commit.

### Step 9: Remove both legacy STT config shims

- Component and piece: Component 3 of 8, version-2 configuration; delete both compatibility paths in one atomic behavior change.
- Dependency: depends only on the regression foundation ending at Step 3 and is intentionally independent of logging; Rust and TypeScript must land together so no layer continues accepting `[workshop.stt]`.
- Debt IDs: `STT-CORE-001`.
- Artifacts: `crates/gateway-config/src/config/imp.rs`, `config/accessors.rs`, `config/tests/schema.rs`, `config/tests/serialize.rs`, `config/tests/validation.rs`, `crates/gateway-config-ui/ui/src/services/config-store.ts`, `src/services/config-store.test.mjs`, and `src/views/settings-sections.test.mjs`.
- Scope: keep `config-version = 2`, delete `migrate_legacy_stt` and `canonicalizeStt` immediately, reject legacy and mixed forms through unknown-field validation, and leave canonical `[stt]` parsing and UI serialization unchanged. Verify `C:\Users\Vinnie\.promptforge\gateway.toml` has no legacy section, record its SHA-256, perform no write to it, and prove the already-canonical file is byte-for-byte identical afterward.
- Exclusions: no migration command, schema version 3, deprecation window, automatic rewrite, or repair of the canonical local file.
- Focused verification: from the repository root run `cargo test -p gateway-config`; from `crates/gateway-config-ui/ui` run `npm run typecheck`, `npm run build`, and `npm test`; in PowerShell compare `Get-FileHash C:\Users\Vinnie\.promptforge\gateway.toml -Algorithm SHA256` before and after the read-only local check.
- Component boundary: ends Component 3; review Step 9 against the Step 8 commit, including the paired Rust and TypeScript deletion and the local hash evidence.

### Step 10: Centralize native STT fixture resolution

- Component and piece: Component 4 of 8, STT test infrastructure; replace the five resolver copies with the existing feature-gated `gateway-stt-engine` fixture boundary and explicit caller fallback roots.
- Dependency: depends on Step 3's stable test layout; it precedes API snapshots because the canonical resolver surface must exist before its feature-enabled contract is recorded.
- Debt IDs: `STT-CORE-002`.
- Artifacts: create `crates/gateway-stt-engine/src/test_fixtures/native.rs`; update `crates/gateway-stt/tests/common/mod.rs`, `crates/gateway-stt-backend-whisper/src/prompt.rs`, `crates/gateway-stt-backend-whisper/tests/native_whisper.rs`, `crates/gateway/tests/it/realtime_stt/`, and affected `Cargo.toml` feature wiring.
- Scope: expose one non-production resolver, preserve `PROMPTFORGE_WHISPER_LIBRARY`, `PROMPTFORGE_WHISPER_MODEL`, and `PROMPTFORGE_WHISPER_AUDIO`, and require each caller to pass its fallback root explicitly.
- Exclusions: no fifth production STT crate, no installed speech behavior change, and no native fixture download redesign.
- Focused verification: from the repository root run `cargo test -p gateway-stt -F test-fixtures`, `cargo test -p gateway-stt-backend-whisper -F test-fixtures`, and `cargo test -p gateway`.

### Step 11: Ratchet feature-enabled fixture APIs

- Component and piece: Component 4 of 8, STT test infrastructure; measure and freeze the feature-enabled public surfaces before narrowing them.
- Dependency: depends on Step 10 because snapshots must describe the centralized API, and it must precede Step 12 so narrowing has an explicit reviewed baseline.
- Debt IDs: `STT-CORE-003`.
- Artifacts: `tools/check-stt-architecture.mjs`, `tools/check-stt-architecture.test.mjs`, `crates/gateway-stt/public-api-test-fixtures.txt`, `crates/gateway-stt-engine/public-api-test-fixtures.txt`, and both crates' `module-ceilings.toml` records.
- Scope: make unreviewed fixture API growth fail while proving default builds expose no fixture symbols.
- Exclusions: no production public API expansion and no low-level fixture removal in this baseline step.
- Focused verification: from the repository root run `node tools/check-stt-architecture.test.mjs`, `node tools/check-stt-architecture.mjs`, `cargo test -p gateway-stt -F test-fixtures`, and `cargo test -p gateway-stt-engine -F test-fixtures`.

### Step 12: Narrow fixture APIs to scenarios

- Component and piece: Component 4 of 8, STT test infrastructure; replace consumer-visible synchronization controls with scenario-level fixture operations.
- Dependency: depends on Step 11 because every current consumer and feature-enabled symbol must be inventoried and snapshotted before contraction.
- Debt IDs: `STT-CORE-003`.
- Artifacts: `crates/gateway-stt/src/test_fixtures.rs`, `crates/gateway-stt-engine/src/test_fixtures.rs`, their consumer tests, `crates/gateway-stt/public-api-test-fixtures.txt`, `crates/gateway-stt-engine/public-api-test-fixtures.txt`, and both crates' `module-ceilings.toml`.
- Scope: preserve all tested scenarios while reducing the quasi-public control surface and updating exact snapshots downward.
- Exclusions: no production behavior changes, no new feature, and no weakened Miri ownership or queue coverage.
- Focused verification: from the repository root run `cargo test -p gateway-stt -F test-fixtures`, `cargo test -p gateway-stt-engine -F test-fixtures`, `cargo +nightly-2026-09-05 miri test -p gateway-stt -F test-fixtures`, `cargo +nightly-2026-09-05 miri test -p gateway-stt-engine -F test-fixtures`, and `node tools/check-stt-architecture.mjs`.

### Step 13: Restore dead-code diagnostics

- Component and piece: Component 4 of 8, STT test infrastructure; remove broad dead-code allowances and resolve only actual configuration-specific exceptions.
- Dependency: depends on Step 12 because narrowing fixture symbols first prevents allowances from masking obsolete controls.
- Debt IDs: `STT-CORE-004`.
- Artifacts: `crates/gateway-stt/src/lib.rs`, affected feature-gated modules, focused tests, and item-level annotations only where configuration evidence requires them.
- Scope: keep dead-code diagnostics active under default, all-feature, test, Miri, and featureless configurations.
- Exclusions: no module-wide allowance, speculative use site, or unrelated warning cleanup.
- Focused verification: from the repository root run `cargo test -p gateway-stt`, `cargo test -p gateway-stt -F test-fixtures`, `cargo clippy -p gateway-stt --all-targets --all-features -- -D warnings`, and `cargo check -p gateway --no-default-features`.

### Step 14: Pin the native STT runner contract

- Component and piece: Component 4 of 8, STT test infrastructure; enforce one exact Rust toolchain and versioned self-hosted runner layout before cache or native work.
- Dependency: depends on Step 10 for the final native fixture contract and follows Steps 11 through 13 so the workflow validates the settled test surface.
- Debt IDs: `STT-CORE-005`.
- Artifacts: `.github/workflows/stt-miri.yml` and `tools/check-stt-native-workflow.test.mjs`.
- Scope: set `RUSTUP_TOOLCHAIN` to `1.89`, resolve `rustup`, `cargo`, and `rustc` from the provisioned runner `PATH`, require exact Rust `1.89.0` before cache use, and keep the hosted Miri nightly pinned and all native Whisper jobs on the Windows CUDA runner.
- Exclusions: no floating `stable`, `$USERPROFILE\.cargo\bin` assumption, runner reprovisioning from CI, or change to native fixture hashes.
- Focused verification: from the repository root run `node tools/check-stt-native-workflow.test.mjs`, `cargo test -p gateway-stt`, and `cargo test -p gateway-stt-backend-whisper`.
- Component boundary: ends Component 4; review cumulative Steps 10 through 14 against the Step 9 commit.

### Step 15: Make preparation names collision-resistant

- Component and piece: Component 5 of 8, Gateway profile switching; harden prepared persistence names before moving transaction ownership.
- Dependency: depends on Step 1's split Gateway coverage and is the first profile-switch piece because the transaction must inherit settled temporary-file ownership semantics.
- Debt IDs: `PF-RTSTT-DC-002`.
- Artifacts: `crates/gateway/src/config_write.rs`, its `PreparedFile` tests, and relevant profile-switch integration tests under `crates/gateway/tests/it/profiles.rs`.
- Scope: add one process-random nonce and bounded `create_new` retry, test deterministic collisions and crash residue, and delete residue only when ownership is proven.
- Exclusions: no broad temporary-file cleanup, rollback redesign, config format change, or deletion of unproven residue.
- Focused verification: from the repository root run `cargo test -p gateway`.

### Step 16: Extract profile preparation phases

- Component and piece: Component 5 of 8, Gateway profile switching; create a private transaction module for target, cancellation, prepared persistence, prior runtime snapshot, and prepared and cutover phase values.
- Dependency: depends on Step 15 because moved preparation must use the final collision and ownership contract; it precedes terminal phases so tests can pin preparation and cutover independently.
- Debt IDs: `PF-RTSTT-DC-001`.
- Artifacts: create `crates/gateway/src/profile_switch.rs`; move `PreparedPersistence`, `CutoverState`, `prepare_cutover`, persistence helpers, and their tests from `crates/gateway/src/lib.rs` and `config_write.rs`.
- Scope: preserve locks, cancellation points, persistence ordering, old-runtime capture, and external behavior while making invalid preparation and cutover order unrepresentable.
- Exclusions: no wire change, installed behavior change, new lock, terminal commit rewrite, or unrelated reduction of the root module.
- Focused verification: from the repository root run `cargo test -p gateway`.

### Step 17: Complete the profile-switch transaction

- Component and piece: Component 5 of 8, Gateway profile switching; represent staged, committed, rolled-back, indeterminate, and terminal outcomes as values and delegate root orchestration to the transaction.
- Dependency: depends on Step 16 because terminal transitions consume the prepared and cutover phase values and their owned rollback state.
- Debt IDs: `PF-RTSTT-DC-001`.
- Artifacts: `crates/gateway/src/profile_switch.rs`, `crates/gateway/src/lib.rs`, `config_write.rs`, `config_apply.rs`, and profile-switch unit and integration tests.
- Scope: preserve every cancellation, rollback, indeterminate-persistence, atomic-publication, speech-replacement, and featureless path; move only helpers owned by this transaction.
- Exclusions: no Gateway API change, no altered timeout policy, no profile schema change, and no cleanup outside the extracted responsibility.
- Focused verification: from the repository root run `cargo test -p gateway`, `cargo check -p gateway --no-default-features`, and `cargo clippy -p gateway --all-targets --all-features`.
- Component boundary: ends Component 5; review cumulative Steps 15 through 17 against the Step 14 commit and update architecture records only for transaction facts now present.

### Step 18: Decode Realtime events exhaustively

- Component and piece: Component 6 of 8, Workshop Realtime UI; introduce one pure exhaustive decoder used by production and canonical fixture mutation tests.
- Dependency: depends on Step 2's stable Workshop integration boundaries and precedes reducer work because the reducer may accept only typed trusted events.
- Debt IDs: `PF-RTSTT-DC-003`.
- Artifacts: `crates/workshop-server/ui/src/services/realtime-transcription.ts`, create `src/services/realtime-event-decoder.ts`, and update `test/realtime-wire-fixtures.mjs` and `test/stt-stream.mjs`.
- Scope: return a discriminated union after exact validation of required and nullable fields, IDs, content index, revision, transcript partition, audio spans, completion usage, and unsupported event types; production and canonical sequence tests call the same decoder.
- Exclusions: no speech protocol change, relay change, reconnect policy change, or dictation ownership refactor.
- Focused verification: from `crates/workshop-server/ui` run `npm run typecheck`, `npm run build`, and `npm test`.

### Step 19: Move insertion policy into input targets

- Component and piece: Component 6 of 8, Workshop Realtime UI; give each `SttInputTarget` one insertion-context operation.
- Dependency: depends on Step 18 only for settled typed service inputs and precedes the registry because composition policy must leave lifecycle state before reducer extraction.
- Debt IDs: `DC-PF-P2-005`.
- Artifacts: `crates/workshop-server/ui/src/ui/stt.ts`, `prompt-input.ts`, textarea target code, `test/prompt-input.mjs`, and `test/stt-stream.mjs`.
- Scope: `insertionContext` returns the selected range, original text, and immutable required prefix for textarea and ProseMirror targets.
- Exclusions: no editor replacement, document-wide read in the registry, transcript reducer, or visual behavior change.
- Focused verification: from `crates/workshop-server/ui` run `npm run typecheck`, `npm run build`, and `npm test`.

### Step 20: Build the pure TakeRegistry reducer

- Component and piece: Component 6 of 8, Workshop Realtime UI; extract pure take state and transitions before production wiring.
- Dependency: depends on Step 18 for typed events and Step 19 for target-owned insertion context, which together define all reducer inputs.
- Debt IDs: `PF-RTSTT-DC-004`, `DC-PF-P2-005`.
- Artifacts: create `crates/workshop-server/ui/src/ui/take-registry.ts` and `test/take-registry.mjs`; use types from `realtime-event-decoder.ts` and `stt.ts`.
- Scope: model overlap, tombstones, precommit binding, rollback, reconnect, sequential spacing, completion authority, and selection replacement; emit editor, capture, status, and wire effects without performing them.
- Exclusions: no DOM, socket, capture-service, status-service, or document-structure access inside the reducer and no production wiring yet.
- Focused verification: from `crates/workshop-server/ui` run `npm run typecheck`, `npm run build`, and `npm test`.

### Step 21: Wire production through TakeRegistry

- Component and piece: Component 6 of 8, Workshop Realtime UI; make `setupStt` interpret reducer effects and remove the callback-owned maps, sets, flags, and editor offsets.
- Dependency: depends on Step 20 because production wiring must consume a fully tested pure transition surface rather than define state transitions in callbacks.
- Debt IDs: `PF-RTSTT-DC-004`, `DC-PF-P2-005`.
- Artifacts: `crates/workshop-server/ui/src/ui/realtime-stt.ts`, `take-registry.ts`, `test/stt-stream.mjs`, `test/prompt-input.mjs`, and affected UI boot tests.
- Scope: preserve capture, wire, status, textarea, ProseMirror, rollback, reconnect, and spacing behavior while giving the registry exclusive take ownership.
- Exclusions: no protocol decoder change after Step 18, no editor internals in lifecycle code, and no unrelated UI cleanup.
- Focused verification: from `crates/workshop-server/ui` run `npm run typecheck`, `npm run build`, and `npm test`.
- Component boundary: ends Component 6; review cumulative Steps 18 through 21 against the Step 17 commit and update architecture records only for decoder and reducer facts now present.

### Step 22: Define agent-supervisor transitions

- Component and piece: Component 7 of 8, Workshop agent supervision; build a pure event and transition model before changing the async loop.
- Dependency: depends on Step 21 only for prior component closure; it deliberately retains the existing `GatewayBinding` generation interface, which later sidecar publication changes must preserve.
- Debt IDs: `PF-RTSTT-DC-005`.
- Artifacts: create `crates/workshop-server/src/session_agents/supervisor/transition.rs`; update `session_agents/lifecycle.rs` and focused transition-table tests.
- Scope: model run completion, catalog generation, Gateway generation, operator cancellation, accepted input, and terminal settlement; decide wait, cancel, preserve, relaunch, or close effects.
- Exclusions: no async orchestration rewrite, model client change, catalog semantics change, or sidecar publication change in this step.
- Focused verification: from the repository root run `cargo test -p workshop-server`.

### Step 23: Wire agent supervision through transitions

- Component and piece: Component 7 of 8, Workshop agent supervision; reduce the async supervisor loop to event collection and effect execution.
- Dependency: depends on Step 22 because run and accepted-turn ownership must be decided by the tested transition table before branch interactions are removed.
- Debt IDs: `PF-RTSTT-DC-005`.
- Artifacts: `crates/workshop-server/src/session_agents/supervisor.rs`, `supervisor/catalog.rs`, `supervisor/transition.rs`, `lifecycle.rs`, and agent integration coverage under `crates/workshop-server/tests/it/agents.rs`.
- Scope: preserve delayed catalog handling, replacement during accepted input, operator cancel, retained history, close behavior, and exactly-once settlement.
- Exclusions: no Gateway binding representation change, catalog filtering change, protocol change, or unrelated session cleanup.
- Focused verification: from the repository root run `cargo test -p workshop-server`.
- Component boundary: ends Component 7; review cumulative Steps 22 and 23 against the Step 21 commit and update architecture records only for supervisor facts now present.

### Step 24: Introduce ValidatedConnection

- Component and piece: Component 8 of 8, sidecar trust and lifecycle; make successful validation produce a public but unforgeable capability.
- Dependency: depends on stable existing sidecar resolution tests and precedes all publication changes because raw files must become incapable of crossing the Workshop mutation boundary.
- Debt IDs: `DC-PF-P2-002`.
- Artifacts: create `crates/shared-sidecar/src/validated.rs`; update `lib.rs`, `stale.rs`, `file.rs`, `health.rs`, and their capability tests and public documentation.
- Scope: keep constructors private; prove process image, boot identity, health, and bearer acceptance; expose only redacted accessors and internal data needed to build a consumer snapshot.
- Exclusions: no caller-only validation, no public constructor, no secret-bearing debug output, and no shift of supervision ownership into `workshop-server`.
- Focused verification: from the repository root run `cargo test -p shared-sidecar` and `cargo doc -p shared-sidecar --no-deps`.

### Step 25: Require capability-based Gateway publication

- Component and piece: Component 8 of 8, sidecar trust and lifecycle; narrow the public updater and place validated identity in the immutable binding snapshot.
- Dependency: depends on Step 24 because `GatewayUpdater` must accept the unforgeable capability rather than revalidate or trust a raw `ConnectionFile`; it also supplies the authoritative identity consumed by Steps 26 and 27.
- Debt IDs: `DC-PF-P2-002`, `DC-PF-P2-003`.
- Artifacts: `crates/workshop-server/src/gateway_binding.rs`, `serve.rs`, `app.rs`, `lib.rs`, tests, and `crates/workshop/src/gateway.rs`.
- Scope: make `GatewayUpdater::replace_sidecar` accept only `ValidatedConnection`, remove raw-file publication from the public Workshop server API, and atomically publish clients plus validated identity in one `GatewayBinding` snapshot.
- Exclusions: no separate identity store, no per-process bearer rotation, no LAN Gateway shutdown authority, and no supervision move across components.
- Focused verification: from the repository root run `cargo test -p shared-sidecar`, `cargo test -p workshop-server`, and `cargo test -p workshop`.

### Step 26: Route quit through the authoritative snapshot

- Component and piece: Component 8 of 8, sidecar trust and lifecycle; remove duplicate Gateway identity ownership from the desktop shell.
- Dependency: depends on Step 25 because quit must read the same validated snapshot that current HTTP and model clients use, including after replacement.
- Debt IDs: `DC-PF-P2-003`.
- Artifacts: `crates/workshop/src/main.rs`, `menu.rs`, `gateway.rs`, `crates/workshop-server/src/gateway_binding.rs`, and replacement-raced-with-quit tests.
- Scope: remove `GatewaySlot`, target exactly one current validated local generation, and preserve explicit LAN Gateway behavior; prove same-port, same-key, and configured-key replacement remain atomic.
- Exclusions: no shutdown of configured LAN Gateways, no second identity cache, no credential rotation, and no menu redesign.
- Focused verification: from the repository root run `cargo test -p workshop-server` and `cargo test -p workshop`.

### Step 27: Join cancellation-aware sidecar shutdown

- Component and piece: Component 8 of 8, sidecar trust and lifecycle; make resolve, validation, wait, launch, supervision, and publication cancellation-aware and finitely joined.
- Dependency: depends on Steps 25 and 26 because cancellation must prevent publication into the authoritative snapshot and quit must target that same snapshot.
- Debt IDs: `DC-PF-P2-001`.
- Artifacts: `crates/workshop/src/gateway.rs`, `main.rs`, `crates/shared-sidecar/src/stale.rs`, `health.rs`, `lock.rs`, and blocking-phase tests in those modules.
- Scope: `GatewaySupervisor` owns and joins its thread under a finite shutdown budget; tests block each phase, request Workshop exit, and prove bounded termination with no later launch, probe, or publication.
- Exclusions: no abandoned supervisor thread, unbounded join, process kill, emergency supervisor, or change to separate Gateway process ownership.
- Focused verification: from the repository root run `cargo test -p shared-sidecar`, `cargo test -p workshop-server`, and `cargo test -p workshop`.

### Step 28: Split and ratchet sidecar lifecycle ownership

- Component and piece: Component 8 of 8, sidecar trust and lifecycle; separate boot planning and one-shot launch from continuous supervision, validated identity, and recovery tests, then freeze the new boundaries.
- Dependency: depends on Step 27 because the selected order is to settle cancellation and joined ownership before extracting modules and recording their final ceilings; it is last because full exit gates may run only after every debt ID is closed.
- Debt IDs: `DC-PF-P2-004`, with closure verification for every debt ID in this plan.
- Artifacts: `crates/workshop/src/gateway.rs`; create `crates/workshop/src/gateway/boot.rs`, `supervisor.rs`, `identity.rs`, `tests/boot.rs`, `tests/recovery.rs`, and `tests/identity.rs`; add `crates/workshop/module-ceilings.toml` and `crates/workshop/tests/module_ceiling.rs`; append settled implementation facts only to `vibe/archdoc-next.md`, and update public documentation only for facts settled by Steps 15 through 28. Never edit `vibe/archdoc.md` during the run.
- Scope: preserve boot, launch, validation, recovery, publication, and shutdown behavior; record ceilings for every resulting module; run focused tests first, then formatting, workspace lint, workspace tests, documentation, architecture, feature-enabled API, native Whisper, both Miri, both UI, guide-generation, unsigned-package recovery, and signed-release gates.
- Exclusions: no pre-documentation of planned APIs, unrelated debt cleanup, component ownership reassignment, speech behavior change, or expansion beyond defects introduced, worsened, or exposed by this plan.
- Focused verification: from the repository root run `cargo test -p shared-sidecar -p workshop-server -p workshop`, `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo test --workspace --locked`, `cargo doc --workspace --no-deps`, `node tools/check-stt-architecture.test.mjs`, `node tools/check-stt-architecture.mjs`, and `cargo run -p build-user-guide`; run `npm run typecheck`, `npm run build`, and `npm test` in both UI directories; then require the native Whisper, both Miri, unsigned-package recovery, and signed-release workflow jobs to pass.
- Component boundary: ends Component 8 and the plan; review cumulative Steps 24 through 28 against the Step 23 commit, then run the complete exit gates from a clean tree.