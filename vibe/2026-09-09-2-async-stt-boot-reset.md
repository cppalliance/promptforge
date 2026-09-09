---
name: async-stt-boot-reset
overview: Restart the STT hot-swap removal from the clean base while preserving the existing Gateway startup architecture, simplify the command queue to one authoritative pending deque, and rename the local discovery record to GatewayDiscoveryFile. The Gateway binds and serves first, then its existing queued boot profile load performs the one allowed STT load; later profile and configuration changes never reload it.
todos:
  - id: simplify-gateway-infrastructure
    content: Rename GatewayDiscoveryFile and replace the duplicated bounded command channel with one notified pending deque
    status: pending
  - id: one-time-speech
    content: Build the minimal one-time speech load while preserving existing status and retained ownership
    status: pending
  - id: bind-first-queue
    content: Load STT once through the existing post-readiness boot profile command and remove runtime reload paths
    status: pending
  - id: ui-restart
    content: Add the browser STT change predicate and successful-Apply restart toast
    status: pending
  - id: docs-qualification
    content: Reconcile documentation and complete full automated, package, and microphone qualification
    status: pending
isProject: false
---

# Remove STT Hot Swapping Without Blocking Gateway

<product-contract>

## Product Requirements

- Problem and users:
  - Runtime STT replacement adds generation publication, quiescence, rollback, reconstruction, and cancellation complexity to a transcription path that already works.
  - The prior plan incorrectly moved slow STT provisioning ahead of Gateway readiness. Gateway must open its port and serve its control plane before any model provisioning.
- Goals:
  - Preserve Workshop microphone capture, batch transcription, Realtime hypotheses and completion, Whisper decoding, authentication, request bounds, and native ownership.
  - Bind the Gateway listener, publish readiness, and serve status, configuration, and ready non-STT capabilities before STT provisioning begins.
  - Rename the Rust `ConnectionFile` discovery concept to `GatewayDiscoveryFile` without changing the on-disk `gateway.json` path, schema, permissions, or compatibility.
  - Make `CommandQueue` store pending commands in one authoritative deque and wake its single worker with `Notify`, with no arbitrary queue capacity or dead channel entries.
  - Preserve command debounce, FIFO order, progress, waiter attachment, active and pending status, cancellation, and bounded shutdown.
  - Load the boot-selected STT configuration exactly once through the existing queued boot `LoadProfile` command after readiness.
  - Reuse existing queue status, progress events, and speech `configured`, `ready`, and `gpu` fields instead of adding another public lifecycle field.
  - Keep the Gateway serving if STT provisioning or engine construction fails. Existing queue and progress reporting exposes the failed boot command, and speech remains unavailable until process restart.
  - Make later profile switches and configuration Apply persist desired STT state without inspecting, cancelling, retrying, or replacing the boot speech lifecycle.
  - Remove STT generation replacement, quiescence, cutover, rollback, reconstruction, and replacement-only cancellation behavior.
  - Show one browser-local restart toast after a successful Apply containing an STT-related change.
  - Correct living documentation and preserve working transcription behavior.
- Non-goals:
  - Add a new command type, provisioning lane, public speech lifecycle field, automatic STT retry, live reload, automatic Gateway restart, second speech engine, plugin system, or fingerprint comparison service.
  - Change audio capture, PCM handling, resampling, sessions, takes, segmentation, hypothesis agreement, finalization, Whisper policy, backend behavior, or FFI.
  - Repair native decode deadlines, retained-PCM overload recovery, exceptional finalization, or unrelated architecture findings.
  - Rename `gateway.json`, change its JSON schema, weaken discovery validation, or change launch-lock and stale-record behavior.
  - Change the separate dominion inference queue or remove `GatewayError::QueueFull` where it still represents inference admission.
  - Rewrite historical plans or acceptance records.
- Success criteria:
  - A deterministic blocked boot command proves complete HTTP responses from `/health`, `/admin/status`, `/admin/progress`, Config UI HTML, JavaScript, CSS, icons, configuration routes, and ready non-STT routes before speech becomes ready. Binding a socket without serving responses is insufficient.
  - Existing queue status and progress show the boot command while speech remains not ready.
  - Successful boot loading makes batch, Realtime, status, and speech model discovery available without restarting the process.
  - Failed boot loading leaves Gateway serving and keeps speech discovery empty until restart.
  - Apply and profile switching can persist speech B while the running process remains on boot speech A; a new process loads B.
  - Replacement-only production paths and tests are removed while retained behavior tests remain green.
  - The Config UI emits exactly one restart toast only after successful STT-related Apply.
  - `GatewayDiscoveryFile` reads and writes the existing `gateway.json` document byte-for-byte compatibly, and every gateway, Workshop, shared-sidecar, diagnostic, shutdown, and test caller uses the clearer name.
  - Command queue pending status exactly matches executable pending commands. Superseded or cancelled commands leave no channel tombstone, more than 32 legitimate commands are not rejected by an arbitrary transport bound, and one worker preserves existing order and cancellation behavior.
- Constraints:
  - Begin from the current clean worktree and accepted eight-invariant architecture document.
  - Slow provisioning stays behind readiness in accordance with the Gateway command-queue architecture.
  - Queue state under one mutex is the sole owner of pending commands. Wake notifications carry no command data and are never a second source of truth.
  - The initial queued `LoadProfile` command owns the only STT load attempt. Existing command cancellation and shutdown semantics apply.
  - Rust 1.89 and Node.js 22 remain supported.
  - Runtime paths never compile native dependencies or invoke build tools.
  - Unsafe code and ABI ownership remain confined to `gateway-whisper-ffi`.
- Open questions: None

## Functional Specification

- Actors and workflows:
  - Gateway reads and validates configuration, creates its empty serving shell, binds the listener, writes connection state, publishes readiness, mounts every route, and starts the existing command worker.
  - After binding, Gateway writes `GatewayDiscoveryFile` to the unchanged `gateway.json` path so Workshop and CLI can discover and validate the running process.
  - The Config UI HTML, JavaScript, CSS, icons, health, status, progress, configuration, profile, admin, and inference routes are mounted without waiting for model provisioning.
  - After readiness, Gateway enqueues its existing boot `LoadProfile` command. That command applies remote and local boot behavior first, then performs the process's one allowed STT load.
  - Later profile and Apply commands never contain an STT participant or another STT load attempt.
- Inputs and outputs:
  - No selected boot STT model leaves the existing speech service inactive.
  - A selected boot STT model remains not ready while the queue and progress surfaces report boot work, then becomes ready after successful loading.
  - `GET /v1/models` advertises physical speech names and `realtime-transcribe` only in `ready`.
  - Before successful STT loading, speech routes preserve their existing unavailable behavior while unrelated routes continue serving.
  - The Config UI snapshots its existing running-versus-pending STT predicate before Apply and emits `Restart the Gateway to apply speech-to-text changes.` only after matching success.
- States and validation:
  - Command enqueue applies debounce and replacement rules while holding queue state, stores each surviving command directly in the pending deque, and notifies the worker.
  - The worker atomically removes the oldest pending command, marks it active, executes it outside the lock, settles every attached waiter, and waits on `Notify` only when no executable command remains.
  - Pending cancellation and supersession remove the actual command. Shutdown marks the queue closed, cancels the active token, drains and settles pending commands, and wakes the worker to exit.
  - The speech facade starts empty and permits one initial load attempt from the boot profile command. No retry, reset, reopen, or replacement transition exists.
  - If the boot profile command is cancelled or superseded before STT loads, the process does not attempt STT again; restart is required.
- Errors and recovery:
  - Configuration and bind failures remain Gateway startup failures.
  - STT artifact, engine, timeout, or command cancellation failures fail the queued boot work, leave speech unavailable, do not shut down Gateway, and are not retried in-process.
  - Ordinary Gateway shutdown cancels pending or active command work and drops the final speech owners through existing joined worker shutdown.
  - Enqueue after shutdown settles as cancelled. Command enqueue no longer fails because an internal transport channel reached an arbitrary capacity.
- Security and privacy behavior:
  - Authentication, Origin validation, loopback restrictions, secret handling, audio bounds, and Workshop relay boundaries remain unchanged.
  - Existing queue progress and speech status expose no model paths, secrets, or backend error text.
- Acceptance criteria:
  - Listener binding, route mounting, and control-plane readiness precede all model provisioning.
  - One process publishes at most one speech runtime.
  - Runtime Apply and profile changes cannot alter the published runtime.
  - A restart loads the newly persisted selection.
  - Installed Workshop dictation demonstrates live hypothesis revision, authoritative completion, and a second take.

</product-contract>

<implementation-contract>

## Technical Design

- Architecture:
  - Preserve the production `spawn` sequence in [Gateway runner](C:\Users\Vinnie\cursor\promptforge\crates\gateway\src\runner.rs): cheap configuration work, listener bind, connection publication, readiness, router service, then queued boot provisioning.
  - Treat [archdoc A1](C:\Users\Vinnie\cursor\promptforge\vibe\archdoc.md) as a direct acceptance rule: any production `spawn` path that downloads a model, constructs a speech engine, or launches a model process before the Gateway serves HTTP is incorrect.
  - Keep eager `Gateway::from_config` assembly limited to its existing test and embedder use. Production `spawn`, `run`, tray, and packaged paths must use the empty shell plus queued provisioning.
  - Keep the existing serialized, observable, cancellable worker in [Gateway commands](C:\Users\Vinnie\cursor\promptforge\crates\gateway\src\commands.rs), but replace its bounded Tokio channel plus mirrored pending deque with one pending deque and `Notify`. Add no command, worker, lane, or detached provisioning task.
  - Mark the existing initial `LoadProfile` command as the only boot command allowed to load STT. Its remote and local work completes or publishes according to existing behavior before its isolated STT load attempt.
  - Evolve the speech facade only enough to publish one initial runtime into the already mounted route state and reject every later attempt.
- Modules and interfaces:
  - In [shared-sidecar file](C:\Users\Vinnie\cursor\promptforge\crates\shared-sidecar\src\file.rs), rename `ConnectionFile` to `GatewayDiscoveryFile`; update exports, validation, stale detection, launch locking, shutdown, diagnostics, Gateway, Workshop, documentation, examples, and tests without changing serialized fields or `gateway.json`.
  - In [Gateway commands](C:\Users\Vinnie\cursor\promptforge\crates\gateway\src\commands.rs), move each `Command` into its `PendingEntry`, remove `QueuedCommand`, the Tokio sender and receiver, `QUEUE_CAPACITY`, transport-level `try_send`, and command-queue `QueueFull` handling. Retain one shared `Notify` and a single-worker ownership flag.
  - `CommandQueue::enqueue` mutates only `QueueState.pending`; `spawn_worker` starts at most one worker; the worker repeatedly pops the next `PendingEntry` from shared state and sleeps with a lost-wakeup-safe check-and-notify loop.
  - Preserve `GatewayError::QueueFull` and its mappings for the separate dominion inference queue; remove only command-queue overflow production and tests.
  - In [gateway-stt service](C:\Users\Vinnie\cursor\promptforge\crates\gateway-stt\src\service.rs), replace production replacement APIs with empty facade construction, one guarded initial load, existing status, models, routes, and retained scripted construction.
  - In [gateway-stt generation state](C:\Users\Vinnie\cursor\promptforge\crates\gateway-stt\src\generation.rs), replace replaceable publication with one empty-to-ready publication guarded against a second attempt. Preserve only admission, session epoch, request, engine, and decode-job lifetime ownership needed by active work.
  - In [Gateway commands](C:\Users\Vinnie\cursor\promptforge\crates\gateway\src\commands.rs), identify the existing startup `LoadProfile` instance without adding another command variant. After its existing remote and local work, invoke the one guarded STT load with the command's progress tree and cancellation token.
  - In [Gateway runner](C:\Users\Vinnie\cursor\promptforge\crates\gateway\src\runner.rs), preserve the current bind-first sequence and enqueue only the existing boot profile command.
  - In [Gateway profile switching](C:\Users\Vinnie\cursor\promptforge\crates\gateway\src\profile_switch.rs) and [configuration Apply](C:\Users\Vinnie\cursor\promptforge\crates\gateway\src\config_apply.rs), remove speech preparation, staging, publication, rollback, reconstruction, fatal shutdown, and progress stages.
  - In [speech status](C:\Users\Vinnie\cursor\promptforge\crates\gateway-stt\src\status.rs) and Gateway status serialization, remove the replacement `generation` field in Step 4 alongside the other replacement removals, while retaining `configured`, `ready`, and `gpu`. Loading remains visible through existing queue and progress status.
  - Preserve the fully loaded speech injection seam for route, embedder, and scripted tests without adding a reload API.
- File and public API changes:
  - Replace the public Rust type name `ConnectionFile` with `GatewayDiscoveryFile` and rename directly related helpers and prose. Add no compatibility alias; the on-disk format remains compatible.
  - Remove the command queue's private fixed-capacity transport and its overflow test. Add direct semantic queue tests instead of structural source assertions.
  - Delete replacement coordinator and snapshot machinery only after retained request and job ownership has moved into the smallest immutable runtime handle.
  - Remove public `PreparedSpeech`, `SpeechReplacement`, replacement methods, and explicit replacement shutdown behavior.
  - Add only the minimum one-time initial-load API required across the `gateway` and `gateway-stt` crate boundary.
  - Add the browser predicate and toast in [ConfigStore](C:\Users\Vinnie\cursor\promptforge\crates\gateway-config-ui\ui\src\services\config-store.ts) and [Config UI main](C:\Users\Vinnie\cursor\promptforge\crates\gateway-config-ui\ui\src\main.ts).
- Data, persistence, failure, security, and privacy constraints:
  - Desired persisted STT state may intentionally differ from the running boot snapshot until restart. Status and discovery describe the running lifecycle only.
  - No STT loading failure may request fatal Gateway shutdown.
  - A feature-disabled Gateway may still reject an active STT selection during cheap capability validation because that is not slow provisioning.
  - Correct stale doc comments in [Gateway runner](C:\Users\Vinnie\cursor\promptforge\crates\gateway\src\runner.rs) (the `spawn` and `run` doc comments) that claim provisioning runs before binding. Documentation must distinguish the eager test/embedder assembly seam from the bind-first production path.

</implementation-contract>

<verification-contract>

## Testing Plan

- Unit:
  - Prove `GatewayDiscoveryFile` reads existing `gateway.json` fixtures, preserves serialized fields and redaction, writes atomically with existing permissions, and retains validated attachment, stale cleanup, launch locking, and shutdown behavior.
  - Prove queue duplicates attach to one command, superseding removes the actual pending command, cancelled pending work never executes, FIFO order survives more than 32 legitimate commands, one worker starts, shutdown wakes an idle worker, and enqueue after shutdown settles as cancelled.
  - Prove pending and active status are derived from the same command objects the worker executes, with no hidden or dead transport entries.
  - Prove inactive facade behavior, one successful initial publication, duplicate-attempt rejection, load failure, cancellation, panic, and ordinary last-owner shutdown without adding a public lifecycle field.
  - Relocate retained admission, session epoch, request, and decode-job lifetime tests before deleting replacement tests.
  - Test the pure Config UI predicate for STT settings, model add/remove/edit, effective profile membership, active-profile pointer changes, and unrelated edits.
- Integration and end-to-end:
  - Use a deterministic blocked boot `LoadProfile` command to prove connection publication and complete HTTP responses for health, status, progress, Config UI HTML, JavaScript, CSS, icons, configuration routes, and non-STT routes before speech completes.
  - Release the STT portion of the boot command and prove eventual status, discovery, batch, and Realtime readiness.
  - Force artifact or engine failure and prove Gateway remains serving with speech not ready and no discovered speech models.
  - Prove one boot attempt from snapshot A, persist or select B through Apply and profile switching, and show the running process remains on A with no speech progress stages; a new process must load B.
  - Preserve authentication, Origin, overload, wire fixtures, Workshop relay, UI stream, native Whisper equivalence, and feature-disabled Gateway coverage.
  - When replacement fixtures are deleted, remove the stale `engine_replacement` lookup from [Workshop Realtime relay](C:\Users\Vinnie\cursor\promptforge\crates\workshop-server\tests\it\realtime_relay.rs) in the same change and run the exact relay integration target.
  - When speech-only Gateway seams are removed, update [feature-disabled logging fixtures](C:\Users\Vinnie\cursor\promptforge\crates\gateway\src\main\logging_tests.rs) in the same change and run default plus feature-disabled Gateway tests before component verification.
  - Test the Config UI toast for qualifying success, unrelated success, failure, cancellation, duplicate suppression, and pre-refresh capture.
- Regression, security, and performance:
  - Run portable workspace tests only while the Workshop sidecar is absent and with Workshop crates excluded. Then build the target-matched Gateway, stage it with `tools/stage-gateway-sidecar.mjs`, run Workshop and Workshop Server tests, and remove staging in guaranteed cleanup.
  - `cargo workshop` is package construction, not Workshop test setup: it stages and then removes its temporary sidecar. Never rely on it to leave the test prerequisite behind.
  - Treat missing sidecars, unreadable logs, active builds, unavailable native fixtures, and other environment setup failures as setup work. Never change production code or unrelated tests to make an environment gate pass.
  - Delete only tests whose asserted behavior is replacement, generation switching, quiescence, rollback, reconstruction, or replacement-only close signaling.
  - Run repository formatting, warnings-denied linting, portable Rust, staged Workshop Rust, documentation, both UI suites, feature-disabled Gateway, retained STT Miri, native Whisper, guide generation, package construction, and installed identity checks.
  - Stage the Workshop Gateway sidecar with the repository helper only for Workshop tests and always remove it afterward.
  - Run complete `shared-sidecar` and Gateway command tests after the discovery rename and queue replacement; preserve command debounce, cancellation, progress, and shutdown assertions unchanged where behavior is retained.
- Exit criteria:
  - Gateway binds and serves its Config UI, static assets, health, status, progress, configuration, and admin routes before STT provisioning completes.
  - Speech reaches one terminal boot state without later replacement.
  - All retained transcription behavior and automated gates pass without weakened assertions.
  - Current documentation states bind-first, one-time STT loading and restart-only configuration changes.
  - Installed physical dictation passes live hypothesis revision, authoritative completion, and a second take, and no review finding remains open.

</verification-contract>

<decision-record>

## Decision Record

- Decisions:
  - Listener readiness precedes every slow model provisioning operation. User direction: "the gateway is supposed to open the port right away so that everyone can talk to it right away."
  - `GatewayDiscoveryFile` names the local process-discovery document accurately; `gateway.json` remains the stable physical file and wire format.
  - One mutex-protected pending deque owns queued commands. `Notify` wakes the worker but carries no payload or queue state.
  - The command queue has no arbitrary item capacity. Semantic debounce and replacement control pending work, and actual enqueues determine memory use.
  - Preserve the existing production sequence and command queue rather than adding a speech command, task, worker, or lane.
  - The initial queued `LoadProfile` command performs existing remote and local work, then makes the process's one guarded STT load attempt. Every later `LoadProfile` and `ApplyConfig` omits STT.
  - Existing queue status, progress events, and speech `configured`, `ready`, and `gpu` fields report boot work without a new public status field.
  - STT boot failure leaves speech unavailable and requires restart; it never prevents the Gateway listener from serving.
  - Persisted desired STT may differ from the boot snapshot by design, while later runtime transactions never compare or synchronize them.
- Rejected alternatives:
  - Keep both the bounded Tokio channel and pending deque: rejected because they duplicate ownership and leave cancelled or superseded channel tombstones.
  - Replace 32 with another fixed capacity: rejected because no supported workload establishes a correct number.
  - Use an unbounded Tokio command channel alongside the pending deque: rejected because it preserves duplicate state and dead entries.
  - Rename the physical `gateway.json` file: rejected because the clearer Rust name does not require an on-disk migration.
  - Pre-bind STT construction: rejected because it blocks the control plane and violates the Gateway readiness architecture.
  - A dedicated `BootSpeech` command: rejected because the existing boot `LoadProfile` already owns post-readiness progress, cancellation, and shutdown.
  - Loading STT inside every `LoadProfile`: rejected because runtime profile changes must never reload it.
  - An independent untracked task: rejected because it loses queue status, cancellation, and shutdown ownership.
  - Automatic retry or reload: rejected because it recreates runtime lifecycle and coherency machinery.
  - A new parallel provisioning lane: rejected as unnecessary for restoring bind-first readiness; revisit only if measured serial queue delay is unacceptable.
- Assumptions, risks, and notes:
  - Current wired command producers are semantically coalesced. Future non-debounced producers must justify their own admission policy instead of reviving an arbitrary global transport bound.
  - The notification loop must prevent lost wakeups when enqueue or shutdown races a worker preparing to sleep.
  - The command queue is serial. STT runs after the boot command's remote and local work, while the listener and control plane remain available.
  - Speech routes preserve existing unavailable behavior before a successful initial load.
  - Native decode can remain non-preemptible and is outside this plan.

</decision-record>

<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build` builds the default `gateway` member; `cargo workshop` builds the Gateway, stages the target-matched sidecar, builds the desktop Workshop, and removes the staging artifact.
- Focused test command pattern: Rust uses `cargo test -p <crate> <test-filter>` for unit tests and `cargo test -p <crate> --test it <test-filter>` for responsibility-named integration suites. UI tests use `node --test <path-to-test.mjs>` after that UI's build when the test imports bundled output.
- Component test command pattern: use `cargo test -p <crate>` for a Rust crate. For either UI, run `npm run typecheck --prefix <ui-directory>`, `npm run build --prefix <ui-directory>`, then `npm test --prefix <ui-directory>`.
- Full-suite test command:
  - Run `cargo test --locked --workspace --exclude workshop --exclude workshop-server --all-features`, followed by the same workspace selection with `--doc`, and `cargo check -p gateway --no-default-features`.
  - Typecheck, build, and test `crates/gateway-config-ui/ui` and `crates/workshop-server/ui`.
  - Build the target-matched Gateway, stage it with `node tools/stage-gateway-sidecar.mjs stage`, run `cargo test --locked -p workshop -p workshop-server`, and remove the sidecar in guaranteed cleanup.
  - Retain the dedicated STT lanes: Miri filters in `gateway-stt-engine` and `gateway-stt`, plus ignored native Whisper backend, FFI, facade unit, and facade integration tests with pinned runtime, model, and audio fixtures.
- Linter command: `cargo clippy --workspace --exclude workshop --exclude workshop-server --all-targets --all-features -- -D warnings`; after staging the Gateway sidecar, lint desktop crates with `cargo clippy -p workshop -p workshop-server --all-targets -- -D warnings`.
- Formatter check command: `cargo fmt --all --check`.
- Test placement and naming conventions: Rust unit tests live beside source in `tests` modules or `src/**/tests/`; integration tests live under `tests/`, commonly as a `tests/it/main.rs` harness with responsibility-named modules. Config UI tests are colocated as `src/**/*.test.mjs`; Workshop UI tests primarily live in `ui/test/*.mjs`, with shared fixtures under `ui/test/helpers`.
- Directory map: `.cargo/` holds target and Cargo alias configuration; `.github/` holds CI, release workflows, and reusable actions; `crates/` holds Rust product, shared, backend, build-helper, and integration-test crates plus the two TypeScript UIs and shared UI package; `design/` holds design reports and specifications; `guide/` is the mdBook source; `prompts/` contains example Markdown programs; `tools/` contains Node and PowerShell build, staging, and validation scripts; `vibe/` records architecture and implementation history; `images/` contains documentation assets.
- Component boundaries: shared protocol, progress, loopback, sidecar, and store crates are dependency leaves. The PromptForge facade depends on its pipeline core and agent runtime, which depend on parser, Lua, model-client, tool, and store layers. Gateway depends on config, routing, local inference, web search, logging, STT, and shared substrate, never on PromptForge or Workshop product crates. The STT facade depends downward on the backend-neutral engine and safe Whisper backend, with `gateway-whisper-ffi` as the native ABI leaf. Workshop embeds Workshop Server; Workshop Server uses PromptForge agents and reaches Gateway only through discovery and protocol clients. Both browser UIs consume `shared-ui`.
- Conventions summary: Rust uses edition 2024 with Rust 1.89 as the minimum, workspace lints, warnings-denied CI, rustfmt 2024 style, typed errors, Tokio/Axum async boundaries, and unsafe code confined to the documented FFI boundary. Behavior changes carry nearby tests; deterministic fault injection is preferred to structural source checks. Product dependency directions are enforced, optional Cargo features represent real build constraints, serve paths do not compile or install dependencies, long work reports through `shared-progress`, and comments explain constraints or cited external workarounds. UI code is TypeScript ESM bundled with esbuild on Node 22 and tested with the Node test runner.

</project-survey>

<execution-plan>

## Execution Instructions

- Run control:
  - Require a clean worktree, use one shared readable scratch directory, and complete one reviewed commit per step.
  - Coding and repair workers leave changes uncommitted. The run controller owns staging, provisional commits, amendments, and final messages.
  - Workers never edit `vibe/archdoc.md`, this plan, or unrelated files. Repair environment setup outside product code and never discard a provisional commit while a worker is active.

<step-1>

### Step 1: Rename Gateway discovery [completed]

- Component: Gateway infrastructure
- Change: Rename public `ConnectionFile` and directly related helper names to `GatewayDiscoveryFile` everywhere, with no alias and no change to `gateway.json`, serialized fields, validation, permissions, atomic writes, launch locking, stale cleanup, diagnostics, or shutdown.
- Artifacts: `shared-sidecar` file, exports, validation, stale detection, lock, shutdown, README, and tests; matched Gateway, Workshop, Workshop Server, examples, diagnostics, and documentation callers.
- Tests: Run complete `shared-sidecar` tests plus focused Gateway, Workshop, and Workshop Server discovery, attachment, recovery, and shutdown tests.

</step-1>

<step-2>

### Step 2: Make the pending deque authoritative

- Component: Gateway infrastructure
- Change: In `crates/gateway/src/commands.rs`, make `QueueState.pending` the sole command owner. Store each `Command` in `PendingEntry`, wake one worker with payload-free `Notify`, and remove `QueuedCommand`, the Tokio command channel, `QUEUE_CAPACITY`, transport overflow, and dead channel entries. Preserve debounce, FIFO order, progress, waiter attachment, cancellation, single-worker ownership, and bounded shutdown.
- Artifacts: `CommandQueue`, `QueueState`, `PendingEntry`, `enqueue`, `spawn_worker`, `worker_loop`, status accessors, cancellation, shutdown, and queue tests. Preserve `GatewayError::QueueFull` only for the separate dominion inference queue.
- Tests: Prove existing debounce and Apply priority, FIFO beyond 32 commands, complete pending cancellation, exact pending status, one worker, closed enqueue, idle shutdown, and enqueue or shutdown races around worker sleep.

</step-2>

<step-3>

### Step 3: Build one-time speech publication

- Component: STT lifecycle
- Change: Refactor `gateway-stt` to one empty facade with one guarded initial publication. Move admission, session, engine, request, and decode-job ownership into the smallest immutable runtime handle. Preserve existing status fields, scripted injection, batch, Realtime, overload, cancellation, and ordinary last-owner shutdown. Keep temporary compatibility only until Step 4 removes replacement callers.
- Artifacts: `gateway-stt` service, generation, lease, snapshot, replacement, artifacts, status, routes, sessions, takes, and retained generation, service, batch, and Realtime tests. Do not touch the cross-crate `engine_replacement` fixture or its Workshop relay consumer in this step; that removal belongs to Step 4.
- Tests: Prove complete one-time publication, duplicate rejection, failure and cancellation cleanup, panic in the load path leaving speech unavailable without poisoning later state, request and worker-job lifetime, inactive and ready status, model discovery, batch and Realtime behavior, and final-owner worker join.

</step-3>

<step-4>

### Step 4: Move STT to boot only and delete replacement

- Component: STT lifecycle
- Change: Give the startup `LoadProfile` an internal `boot: bool` field set only by the runner's boot enqueue; debounce attach and supersession preserve it, and only a command carrying it may call the guarded STT initial load after its remote and local work. In the same commit, remove STT from later profile and Apply transactions and delete replacement APIs, coordinator, snapshots, quiescence, rollback, reconstruction, replacement events, and replacement-only tests. Keep the listener, routes, Config UI, health, status, and progress serving before provisioning. Add no command, worker, task, lane, retry, or public status field.
- Artifacts: Gateway commands, runner, profile switch, config Apply, status, model discovery, test support, boot, surface, progress, profile, and Realtime tests; remaining `gateway-stt` replacement files and fixtures; Workshop relay fixture list; feature-disabled Gateway logging fixtures.
- Tests: Block the STT portion and prove complete HTTP responses from UI assets, health, status, progress, config, profile, and ready non-STT routes. Then prove eventual batch and Realtime readiness, isolated STT failure, cancellation and shutdown, boot A remaining live after B persists, B loading after restart, no replacement fixture lookup, exact Workshop relay behavior, and default plus feature-disabled Gateway builds.

</step-4>

<step-5>

### Step 5: Notify the browser when STT needs restart

- Component: Config UI
- Change: Add one pure pending-STT predicate over settings, models, effective profile membership, and active-profile pointer. Capture it before Apply refresh and show exactly one restart toast only after qualifying success. When one Apply changes both a process-owned section and STT, the existing backend `restart_required` banner already communicates restart; the STT toast still shows once, and the two messages coexist without suppression or duplication.
- Artifacts: ConfigStore, Config UI main, Apply tests, and focused model, profile, and settings tests.
- Tests: Cover all qualifying changes, unchanged and unrelated edits, success, failure, cancellation, refresh ordering, duplicate suppression, and the combined process-owned plus STT Apply showing both the existing banner and one STT toast; run Config UI typecheck, build, and complete suite.

</step-5>

<step-6>

### Step 6: Reconcile documentation and qualify

- Component: Documentation and release
- Change: Document bind-first serving, one queued boot STT load, restart-only later STT changes, queue progress, the authoritative unbounded command deque, and `GatewayDiscoveryFile` writing unchanged `gateway.json`. Keep eager `Gateway::from_config` limited to tests and embedders. Regenerate current guide outputs. Do not edit `vibe/archdoc.md`, historical plans, or historical acceptance records. If the Step 4 commit grows beyond a reviewable size, split it into two commits: first rewire boot loading and strip switch/Apply speech participation, then delete the dormant replacement machinery; both must pass the Step 4 tests.
- Artifacts: Current root, Gateway, shared-sidecar, and STT documentation; Gateway guide source chapters and generated outputs. Do not create or edit `design/generic-realtime-stt.md`; that file does not exist at this base.
- Tests: Run one final qualification: formatting, warnings-denied linting, portable Rust and documentation tests, feature-disabled Gateway, both UI suites, retained Miri, native Whisper, guide generation, mdBook, explicitly staged Workshop tests with guaranteed cleanup, package construction, installed identity and HTTP checks, then live hypothesis revision, authoritative completion, and a second microphone take.

</step-6>

</execution-plan>