---
name: remove-range-debt
overview: Prune repository policy, remove unsupported structural ratchets, preserve proven runtime contracts, repair three concrete behavior defects, and verify the ordinary product from committed HEAD.
todos:
  - id: remove-structural-ratchets
    content: Prune policy guidance and remove unsupported structural ratchets
    status: pending
  - id: establish-gateway-ownership
    content: Preserve logging and establish Gateway ownership
    status: pending
  - id: repair-profile-and-registry
    content: Preserve profile behavior and generation identity
    status: pending
  - id: revoke-publication-and-shutdown
    content: Revoke publication and bound supervisor shutdown
    status: pending
isProject: false
---
# Remove Unsupported Ratchets and Repair Runtime Defects

## Product Requirements

- Problem and users:
  - PromptForge maintainers are paying recurring costs for structural tests that constrain repository shape without proving product behavior.
  - Gateway users can encounter duplicate-launch side effects before one process owns canonical state.
  - Workshop dictation can reject a server item identifier reused after reconnect.
  - Workshop shutdown can exceed its promised bound when the Gateway supervisor does not cooperate.
- Goals:
  - Remove unsupported source parsers, snapshots, counts, ceilings, topology checks, import walkers, and exact allowlists.
  - Reduce all 31 original `AGENTS.md` files to stable guidance with significant correctness, security, protocol, ABI, lifecycle, data-integrity, or release benefit.
  - Preserve proven logging, speech, profile-switch, sidecar, network, packaging, and release behavior.
  - Establish deterministic Gateway process ownership, connection-generation identity, publication revocation, and bounded supervisor shutdown.
  - Validate unsigned packages on every supported Workshop platform without publishing a release.
- Non-goals:
  - Rework `ValidatedConnection` revalidation.
  - Split logging modules, redesign queue ownership, add dependencies, rewrite registry collections for style, or consolidate unrelated fixtures.
  - Use net line deletion as proof of correctness.
  - Rewrite dated plans or historical acceptance logs.
- Success criteria:
  - Only four approved structural rules remain: Gateway cannot depend on Workshop, PromptForge cannot depend on Gateway or Workshop, Gateway cannot depend on PromptForge, and Workshop cannot depend on Gateway.
  - Product and behavior suites remain green after structural enforcement is removed.
  - The Gateway build script passes warnings-denied Clippy on non-Windows hosts without a lint exemption.
  - Direct-launch races produce one process owner and no losing-process canonical log mutation.
  - Reconnect accepts same-item-ID reuse while rejecting stale-generation work.
  - Supervisor shutdown returns `Joined`, `Panicked`, or `Detached` within one absolute deadline and prevents late publication.
  - The existing package matrices run in nonpublishing mode against the same verified commit as the owning CI jobs.
- Constraints:
  - Execution starts from a clean current branch tip, not a recreated historical tree.
  - The review anchor is `84b2c9261f96642bb3fa02836d4e98b13cde8208`; if it is not an ancestor, inspect both its direct diff and the merge-base diff.
  - Never use the divergent `624e324` line as an execution base.
  - Prefer compiler checks, types, behavior tests, and deterministic fault injection over repository-shape inspection.
  - Repository policy binds plans; plans cannot create structural enforcement without explicit user approval.
  - The native runner repository change is blocked until Rust 1.89.0 is verified under the actual service account.
- Open questions:
  - None.
## Functional Specification

- Actors and workflows:
  - Maintainers review plans and changes under one evidence hierarchy: types and compiler checks first, behavior tests and fault injection second, structural checks only by explicit exception.
  - A Gateway process acquires process-lifetime ownership before logging, stale cleanup, recovery, bind, or connection publication.
  - A losing Gateway waits briefly for the owner's validated connection record, attaches through the existing handoff path when possible, and otherwise exits with a console-only error.
  - Workshop transcription stamps every service event, connection state, error, wire result, user action, audio chunk, and capture completion with its originating connection generation.
  - Workshop shutdown permanently revokes Gateway publication, signals the supervisor, waits to one deadline, and detaches an uncooperative worker.
  - Runner administrators provision and validate one explicit Rust directory before repository preflight changes.
  - Release operators dispatch unsigned build and installer-test matrices without signing or publication.
- Inputs and outputs:
  - Cargo metadata supplies workspace package names and direct local dependency edges for the four approved product-boundary rules.
  - `PROMPTFORGE_RUST_1_89_0_BIN` supplies one absolute directory containing `cargo.exe` and `rustc.exe`.
  - Realtime service callbacks and client request results carry a typed connection generation.
  - Supervisor shutdown returns `Joined`, `Panicked`, or `Detached`.
  - Publication after revocation returns `GatewayPublicationError::PublicationClosed`.
- States and validation:
  - Gateway ownership is held by an operating-system lock handle for the process lifetime and is distinct from parent-side `LaunchLock` election.
  - `TakeRegistry.activeGeneration` advances only on newer readiness, treats duplicate readiness as idempotent, and rejects older or mismatched inputs.
  - Registry item bindings, pending requests, client correlations, commit expectations, and retired tombstones include generation.
  - `GatewayBinding` owns permanent publication state shared by every `GatewayUpdater` clone.
  - A recovery child is owned only when its validated connection identity matches the PID returned by spawn.
  - The runner preflight accepts only an absolute directory with two regular executables reporting exactly Rust 1.89.0.
- Errors and recovery:
  - Losing Gateway launches never initialize canonical logging or mutate shared state.
  - Closed connection generations roll back existing observable state and discard their wire identity while preserving only local ownership needed to settle already-started work.
  - Fatal indeterminate profile persistence cancels work, requests shutdown, prevents publication, and retains the originating cause.
  - Unpublished recovery children receive authenticated shutdown only when validated identity still matches the spawned PID; uncertain children are logged and left for ordinary stale resolution.
  - External runner or package-lane blockers remain incomplete and are never inferred passing.
- Security and privacy behavior:
  - Preserve structured credential redaction before formatting and bounded final-output redaction.
  - Preserve loopback authority, configured-LAN denial, same-socket validation, response limits, absolute network deadlines, slow-drip defenses, and malformed-length rejection.
  - Never expose credentials, prompts, transcripts, audio, request bodies, environment values, or full local model paths through logging.
  - Never kill a recovery process by PID alone or delete a connection record whose ownership is uncertain.
- Acceptance criteria:
  - Every removed structural check has a recorded disposition and no parser-only helper remains.
  - Every original `AGENTS.md` file is either reduced to materially useful current guidance or removed when fully redundant.
  - The retained Cargo metadata test checks only the four approved product dependency rules across every direct local dependency kind.
  - The non-Windows Gateway resource helper returns unit while `main` retains fallible environment handling and Windows resource errors.
  - Profile behavior is proven directly before profile source parsing is removed.
  - Real-process launch races, real-socket reconnect, publication races, uncooperative shutdown, native runner preflight, and unsigned installer workflows satisfy their stated outcomes.
## Technical Design

### Structural policy and enforcement

- Root policy in `AGENTS.md` owns the evidence hierarchy and applies it to plans:
  - Keep product boundaries, runtime safety, extension discipline, behavior-test integrity, and non-obvious workaround guidance.
  - Remove generic ceremony, command matrices, discoverable inventories, stale implementation detail, file-placement rules, test topology, and enforcement prescriptions.
  - Preserve product and behavior tests during refactors; explicitly permit approved structural-test deletion.
- All 30 nested `AGENTS.md` files contain crate-local material rules only:
  - Delete `crates/gateway-stt-backend-whisper/AGENTS.md` because its only material rule is already owned by workspace unsafe lints and `crates/gateway-whisper-ffi/AGENTS.md`.
  - Correct stale logging retention, Workshop asset-error, Lua markdown dependency, WebFetch resolver, and gateway-config file-placement guidance.
  - Keep UI layering and shared-package ownership as human guidance without walkers or source-text pinning.
- `crates/gateway-stt/tests/it/architecture.rs` retains one Cargo metadata test with four approved rules:
  - Gateway packages are `gateway` or names beginning with `gateway-`; they cannot directly depend on Workshop packages.
  - PromptForge packages are `promptforge` or names beginning with `promptforge-`; they cannot directly depend on Gateway or Workshop packages.
  - Gateway packages cannot directly depend on PromptForge packages.
  - Workshop packages are `workshop` and `workshop-server`; they cannot directly depend on Gateway packages.
  - Inspect normal, development, build, renamed, and target-specific local dependencies without enumerating allowed edges or package counts.
- Remove every other `architecture.rs` family:
  - Exact allowlists, legacy-symbol scans, reference-count vocabulary scans, registry and test-shape checks, speech-discovery scans, lint-manifest checks, ceilings, counts, and profile source parsing.
  - Replace unsafe-lint manifest inspection with compiler lanes for the safe STT crates and the explicit FFI lint policy.
- Remove remaining structural infrastructure:
  - Module-ceiling manifests and tests, internal public-API snapshots, root counts, integration-test ceilings, the JavaScript STT architecture driver, architecture-only dependencies, and CI tool installation.
  - UI layer walkers, build hooks, package scripts, esbuild plugins, and enforcement comments while preserving bundling, watching, static assets, TypeScript, and behavior tests.
- Remove structural tests omitted by the original inventory:
  - The gateway-logging manifest allowlist, installer source and workflow parser, shipped-prompt counts and symbol scans, Gateway cfg source checks, and config-store legacy-symbol scan.
  - CSS source-rule parsing and exact Realtime fixture-directory or duplicate case-list coupling while preserving shipped-artifact, wire, DOM, and runtime behavior checks.
  - Workflow-text extraction when the native runner preflight becomes a checked-in script.
- Current product and policy facts update `AGENTS.md` files and `design/generic-realtime-stt.md`; architecture observations enter `vibe/archdoc-next.md` only through the commit-message pipeline and reach `vibe/archdoc.md` only through the operator-owned queue drain.

### AGENTS dispositions

- `crates/gateway/AGENTS.md`: keep feature-off behavior, managed CUDA artifact ownership, subsystem boundaries, and legacy Workshop-section compatibility; remove root product-boundary duplication.
- `crates/gateway-config/AGENTS.md`: keep validate-before-export, declarative-only, and secret-safe diagnostics; remove the companion-file placement rule.
- `crates/gateway-logging/AGENTS.md`: keep caller-supplied state, binary-owned subscriber, redaction chokepoint, non-evictable warning and error records, disk-layout ownership, fallback, and shutdown order; remove exact dependencies, public-surface enumeration, stale constants, and test commands.
- `crates/gateway-local/AGENTS.md`: keep provisioning and child-lifecycle ownership apart from HTTP, routing, profile switching, and Gateway error types.
- `crates/gateway-routing/AGENTS.md`: keep shared routing vocabulary separate from HTTP, upstream construction, local inference, the routing table, and Gateway errors.
- `crates/gateway-web-search/AGENTS.md`: keep provider-service ownership, secret handling, and crate-local errors separate from Gateway routing, auth, and profile switching.
- `crates/shared-loopback/AGENTS.md`: keep the sole fail-closed peer and authority checks, distinct Gateway and Workshop origin policies, and the lean dependency boundary without naming an enforcement test.
- `crates/gateway-stt/AGENTS.md`: keep `Take` ownership, shared artifact-store use, and the pre-decode multipart limit.
- `crates/gateway-stt-engine/AGENTS.md`: keep stateless jobs, blocking decoder thread ownership, startup timeout semantics, and panic-reporting shutdown.
- `crates/gateway-stt-backend-whisper/AGENTS.md`: remove the file because its remaining unsafe boundary is fully owned by the FFI crate and compiler lints.
- `crates/gateway-whisper-ffi/AGENTS.md`: keep safety-comment requirements, Drop-owned raw pointers, safe-API confinement, and the packaged C ABI pin.
- `crates/workshop/AGENTS.md`: keep bridge unsafe confinement, boot failure behavior, degraded runtime handling, detached Gateway launch, shell ownership limits, quit semantics, and programmatic window capability.
- `crates/workshop/icons/AGENTS.md`: keep master-to-derived icon synchronization and protection of hand-crafted installer assets; remove regeneration recipes.
- `crates/workshop-server/AGENTS.md`: keep two-zone errors, embedding hygiene, loopback binding, opaque Realtime relay, socket ownership, durable and ephemeral delivery, disconnect cleanup, typed state, asset behavior, and bounded shutdown; remove ceilings, file-layout rules, test topology, and stale asset claims.
- `crates/workshop-server/ui/AGENTS.md`: keep one-way layer ownership, composition-root state injection, and the Workshop-specific Cursor surface target; remove walker enumeration, source pinning, test discovery, and harness instructions.
- `crates/shared-ui/AGENTS.md`: keep token ownership, base-layer direction, cross-product primitive criteria, component lifecycle, focus behavior, and third-party notices; remove package narration and enforcement references.
- `crates/shared-sidecar/AGENTS.md`: keep sole connection-file ownership, synchronous runtime independence, unsafe confinement, loopback probe authority, and stale-deletion ownership.
- `crates/shared-protocol/AGENTS.md`: keep OpenAI wire and upstream abstraction ownership apart from local inference, routing, handlers, and Gateway-local concepts.
- `crates/shared-progress/AGENTS.md`: keep bottom-of-graph placement, host-owned forwarding, producer and renderer separation, lossy intermediate versus terminal delivery, time-based weights, and additive serialization.
- `crates/promptforge/AGENTS.md`: keep the facade-only dependency and API vocabulary boundary.
- `crates/promptforge-core/AGENTS.md`: keep verbatim historical re-exports, provider ownership, private write scope, and dependency direction.
- `crates/promptforge-core-support/AGENTS.md`: keep report-only observation, explicit read-side history, bottom-of-graph placement, byte-identical run envelopes, and closed control-markup inventory.
- `crates/promptforge-agent/AGENTS.md`: keep sibling-executor independence, absent legacy globals, shared tool dispatch, and stable observer section labels.
- `crates/promptforge-model-client/AGENTS.md`: keep transport ownership, executor independence, canonical metrics vocabulary, and hidden executor seams.
- `crates/promptforge-parser/AGENTS.md`: keep prompt-document parsing, Lua dependency direction, and executor independence; remove feature and test-topology guidance.
- `crates/promptforge-store/AGENTS.md`: keep the virtual-filesystem boundary and hidden fanout seams without enumerating internal symbols.
- `crates/promptforge-tools/AGENTS.md`: keep the vocabulary-only dependency firewall and remove implementation inventories.
- `crates/promptforge-web-search/AGENTS.md`: keep provider ownership, cause-preserving errors, bounded requests, overflow rejection, and secret-free diagnostics.
- `crates/promptforge-webfetch/AGENTS.md`: keep caller-supplied URL scope and per-hop guarded resolver, redirect, and body-bound SSRF defenses.
- `crates/promptforge-lua/AGENTS.md`: keep the parser-cycle boundary, executor independence, single tool-dispatch body, and hidden executor seams; remove the unimplemented markdown dependency claim.

### Gateway ownership, logging, and runner contract
- `shared-sidecar` adds `GatewayInstanceLease` as an operating-system-backed, nonblocking lock on a dedicated run-directory file:
  - The owned handle is held for the Gateway process lifetime and released automatically after graceful exit, crash, or termination.
  - `LaunchLock` remains a separate parent-side spawn election; Gateway never acquires or waits on it.
- Gateway startup acquires the instance lease before canonical logging, stale-record cleanup, recovery, bind, or connection publication:
  - After lease acquisition, re-resolve and validate the connection record before any side effect.
  - A live validated Gateway causes lease release and ordinary handoff.
  - A lease loser waits a bounded interval for the winner's valid record and otherwise exits nonzero without canonical logging.
- Real-process fixtures use named synchronization files in unique temporary run directories:
  - Direct versus direct proves one owner and no losing-process canonical log mutation.
  - Workshop launch versus direct proves the child lease does not deadlock with the parent's `LaunchLock`.
  - Winner termination followed by relaunch proves operating-system dead-owner recovery.
- Logging changes are limited to `crates/gateway-logging/src/redact.rs` and duplicated fault support in `queue.rs`, `worker.rs`, and `writer.rs`:
  - Preserve pre-format field classification, final-output redaction, queue and shutdown accounting, bounded settlement, detached loss, stall handling, rename rotation, crash recovery, segment naming, pruning, and disk limits.
  - Any consolidated harness must retain direct write, flush, rename, sync, crash, recovery, and release injection.
- Native runner provisioning is external to the repository:
  - One checked-in PowerShell script validates only `PROMPTFORGE_RUST_1_89_0_BIN`; it performs no discovery or installation.
  - The workflow invokes the script before cache restoration.
  - Controlled fake executables test script behavior directly without extracting workflow text.

### Profile behavior and connection generation
- Direct profile behavior tests own four contracts before source parsing is removed:
  - Persistence is durable before readers observe the new runtime.
  - Failures after local or speech partial start reconstruct the prior runtime and persisted profile.
  - Indeterminate persistence cancels work, requests fatal shutdown, blocks publication, and retains its cause.
  - Dropped or aborted `SpeechReplacement` ownership rolls back without publication or leaked admission.
- `RealtimeTranscriptionService` assigns each created socket a monotonically increasing immutable generation:
  - Typed envelopes carry generation for decoded events, connection state, connection errors, and append, commit, or clear results.
  - Every socket callback retains its assigned generation and also keeps the current-socket identity guard.
- `setupStt` tracks the current ready generation and stamps every `TakeRegistryInput`:
  - User, audio, connection, error, server, wire, and local capture completions retain their originating generation.
  - Wire effects and returned request results preserve the actual socket generation rather than relabeling on completion.
- `TakeRegistry` stores `activeGeneration` and generation-scopes every connection-owned identity:
  - Newer readiness advances, duplicate readiness is idempotent, and older readiness is ignored.
  - All inputs except advancing readiness must match the active generation.
  - Pending requests, client correlations, commit expectations, item bindings, and retired tombstones match on generation and identifier.
  - Connection loss performs existing rollback, clears closed-generation wire identity, and leaves only local ownership needed to settle already-started work.
- A new generation can reuse any prior item identifier immediately; stale server events, wire results, errors, and capture completions cannot mutate the new generation.

### Publication revocation and bounded supervision
- `GatewayBinding` replaces `Arc<Mutex<()>>` with shared `Arc<Mutex<PublicationState>>`:
  - Candidate construction occurs before locking.
  - Publication and permanent revocation linearize through the same mutex.
  - Closed publication does not increment generation, store a snapshot, or notify consumers.
  - Every updater clone observes closure and returns `GatewayPublicationError::PublicationClosed`.
- `GatewaySupervisor` uses a private `StopSignal` separate from `CancellationToken`:
  - The stop signal is shared atomic state plus a condition-variable wake and never waits while signaling.
  - A completion guard signals a one-shot receiver on return or panic unwind.
  - Shutdown revokes publication, signals stop, computes one deadline, and joins only after completion is observed.
  - Timeout drops the thread handle and returns `Detached`; successful completion returns `Joined` or `Panicked`.
  - Drop uses the same bounded path, and server teardown continues for every outcome.
- Recovery distinguishes an independently published winner from `RecoveryCandidate { child_pid, validated }`:
  - A launched candidate is accepted only when validated identity names the exact spawned PID.
  - Stop or publication closure after spawn prevents publication.
  - Unpublished owned children receive authenticated shutdown under a separate one-second deadline.
  - PID mismatch, failed validation, or cleanup timeout leaves the connection record intact and never kills by PID.
- `CancellationToken` remains for effect admission and bounded I/O, not supervisor completion proof:
  - Keep network deadlines, response head and body limits, slow-drip defenses, malformed-length rejection, same-socket authority checks, and cancellation before destructive cleanup.
  - Narrow duplicate cancellable wrappers only where the new lifecycle makes them redundant without weakening these bounds.

### Package validation

- `Release Workshop / build` and `Release Workshop / test` gain a manually dispatched nonpublishing mode:
  - Read the expected version from the workspace.
  - Build unsigned packages without signing credentials or updater signatures.
  - Upload temporary test artifacts and run the existing installer-test matrices for Windows, both macOS architectures, Linux x64, and Linux ARM.
  - Make the publish job unreachable from this mode.
## Testing Plan

- Unit:
  - Preserve logging redaction, ordering, byte accounting, exact loss, bounded settlement, stall, rotation, recovery, disk-budget, and pruning tests.
  - Prove profile persistence ordering, local and speech rollback reconstruction, fatal indeterminate handling, and uncommitted `SpeechReplacement` disposal before deleting source parsing.
  - Prove generation-aware reducer behavior for same-ID reuse and every stale input class.
  - Prove publication and revocation ordering, permanent closure across updater clones, and `Joined`, `Panicked`, and `Detached` supervisor outcomes.
  - Prove recovery-child PID matching, cleanup timeout, retained records, and `LaunchLock` release.
- Integration and end-to-end:
  - Run real direct-versus-direct, Workshop-versus-direct, and dead-owner Gateway process races on Windows and Linux.
  - Run the real two-socket reconnect path through `RealtimeTranscriptionService`, `setupStt`, and `TakeRegistry`.
  - Preserve FIFO acknowledgment, overlapping takes, rollback, sequential spacing, textarea, ProseMirror, network-limit, configured-LAN, same-socket, and validated-publication behavior.
  - Run the checked-in PowerShell preflight against valid, wrong-version, missing, mixed, malformed, relative, nonexistent, and unset directory fixtures.
  - Run unsigned package build and clean-machine installer tests on Windows, both macOS architectures, Linux x64, and Linux ARM.
- Regression, security, and performance:
  - Compile the three safe STT crates with unsafe code denied and the FFI crate under its explicit lint policy.
  - Run warnings-denied Clippy for the Gateway build script on Linux and preserve Windows resource embedding coverage.
  - Run TypeScript, production bundle, and UI behavior suites after walker removal.
  - Use a one-time final search for orphaned structural tooling and stale policy claims as review evidence, never as retained CI.
  - Preserve absolute deadlines, framing limits, slow-drip defenses, malformed-length rejection, credential redaction, and bounded shutdown.
  - Record exact commands, execution commit, CI URLs, runner identity, fixture identity, artifact hashes, and external failures.
- Exit criteria:
  - `CI / check` owns formatting, warnings-denied lint, non-Workshop Rust tests, doctests, documentation, featureless Gateway, and clean-tree verification.
  - `CI / ui` owns both UI packages; `CI / check-workshop` owns Windows Workshop tests and process races; `CI / check-workshop-linux` owns the Linux build and Unix process races.
  - `CI / msrv`, `CI / supply-chain`, `STT Miri / pure-stt-state`, and `STT Miri / native-whisper` each run their existing matrix once against the execution commit.
  - The nonpublishing Workshop package matrix runs against that same commit and cannot enter publication.
  - Generated guides and UI bundles leave the worktree clean.
  - The final diff contains no structural gate beyond the four approved product-dependency rules and no unrelated refactor or historical-record rewrite.
## Decision Record

- Decisions:
  - Reject structural ratchets by default because history showed recurring refactor coupling without demonstrated product-defect prevention.
  - Retain exactly four narrow Cargo metadata rules. The user said, "promptforge* crates must never depend on gateway or workshop crates," "Gateway product crates cannot depend on PromptForge product crates," and "Workshop product crates cannot depend on Gateway product crates."
  - Prune all repository guidance under a material-benefit test. The user said, "I would like to cut from them whatever is not of significant material benefit."
  - Preserve valuable behavior selectively rather than reverting mixed commits wholesale.
  - Keep `LaunchLock` for parent election and add a separate process-lifetime `GatewayInstanceLease`.
  - Require external runner provisioning before repository preflight changes because repository discovery cannot prove the service account's environment.
  - Scope Realtime identity and tombstones to a connection generation because item identifiers are not globally unique across reconnects.
  - Put permanent publication closure in `GatewayBinding` so every updater clone and detached worker observes it.
  - Let shutdown bounds outrank worker completion; detach after the deadline rather than force-stop or wait indefinitely.
  - Authenticate late-child cleanup with validated PID identity and a separate one-second deadline.
  - Complete package validation without signing or publication.
  - Treat deletion estimates as information, never acceptance evidence.
- Rejected alternatives:
  - Reverting the reviewed commits wholesale is rejected because logging, cfg ownership, network limits, profile behavior, and other runtime value must remain.
  - Replacing removed ratchets with new parsers, snapshots, allowlists, graphs, counts, or policy linters is rejected; revisit only for an explicitly approved stable product or security boundary with no ordinary equivalent.
  - A supervisor-local publication flag is rejected because other updater clones and detached work could still publish.
  - Cooperative cancellation plus unconditional join is rejected because an uncooperative worker can exceed the shutdown bound.
  - PID-only termination or connection-record deletion is rejected because process identity can change or remain uncertain.
  - PATH, rustup-home, profile, or drive discovery is rejected because it cannot establish one service-account contract.
  - Signed release publication as a completion gate is rejected because unsigned install behavior can be validated independently.
  - `ValidatedConnection` revalidation cleanup is deferred until a separate API-scope decision.
- Assumptions, risks, and notes:
  - The review anchor and execution branch have diverged; execution must inspect both direct and merge-base comparisons.
  - Structural enforcement entered across several historical changes, and root or nested guidance later described parts of it as mandatory.
  - The current tree already contains valuable sidecar, logging, profile, Realtime, and arbitrary-duration speech work that removal must not disturb.
  - Profile source parsing cannot be removed until four direct behavior contracts are green.
  - Runner validation and native Whisper remain externally blockable; blocked evidence stays incomplete.
  - Real-process race tests are platform-sensitive and require bounded fixture cleanup.
  - Confidence is high that the design separates proven runtime defects from unsupported repository-shape preferences.
## Project survey

- Build command:
  - Prerequisites are Rust 1.89 and Node.js 22, followed once per checkout by `npm ci` in `crates/workshop-server/ui` and `crates/gateway-config-ui/ui`.
  - `cargo build` builds the default workspace member, `gateway`.
  - `cargo workshop` is the normal full desktop build alias and invokes `build-workshop`; `cargo build -p workshop` is the lower-level desktop package build and requires a staged Gateway sidecar.
  - Each UI builds independently with `npm run build` from its own `ui` directory. Generated UI bundles go to Cargo `OUT_DIR`, not the repository.
- Focused test command pattern:
  - Rust: `cargo test -p <crate> <test-name-filter>`. Integration suites are named by their crate test target and expose module and test-name filters, so a focused plan step should name both the package and behavior filter.
  - JavaScript and TypeScript: run `node <path-to-test.mjs>` for one test module, or `npm test` from the owning UI directory for that UI's complete Node test set.
- Full-suite test command:
  - No single repository command covers every supported product and platform. `cargo test -p '*'` is the full Rust workspace test command on a host with the Workshop prerequisites and staged sidecar, while each UI additionally requires `npm run typecheck`, `npm run build`, and `npm test` in its own directory.
  - The authoritative full suite is the CI job matrix: `check`, `ui`, `check-workshop`, `check-workshop-linux`, `msrv`, `supply-chain`, and the STT Miri and package lanes named in this plan. Root `cargo test` alone covers only the default Gateway member.
- Linter and formatter commands:
  - Rust formatting uses `cargo fmt`; CI and the pre-commit hook check all workspace files.
  - Rust linting uses `cargo clippy`; CI splits non-Workshop workspace targets from `workshop` and `workshop-server` and treats warnings as errors. Workspace lints deny Clippy `all`, `unwrap_used`, `expect_used`, unsafe operations in unsafe functions, and broken Rustdoc links.
  - UI static checks use `npm run typecheck`. The current Workshop UI typecheck also runs its layer checker, while the config UI runs that checker before tests. No separate JavaScript or TypeScript formatter command is configured.
- Test placement and naming conventions:
  - Rust unit tests live beside production code in `src`, commonly in `tests.rs`, a `tests` module directory, or an inline conditional test module.
  - Rust integration tests live under `crates/<crate>/tests`. Larger crates use one harness such as `tests/it/main.rs` or `tests/suite/main.rs`, with behavior-area modules beneath it; smaller crates use descriptive top-level files such as `engine_contract.rs` and `startup_cleanup.rs`.
  - Shared integration fixtures live in `tests/common`; opt-in native and hardware proofs use ignored tests or feature and platform gates.
  - UI tests are Node ESM modules. Workshop UI tests are primarily `ui/test/*.mjs` with some source-adjacent `*.test.mjs`; config UI tests are source-adjacent `ui/src/**/*.test.mjs`.
- Directory map:
  - `.cargo/`: target configuration and the `cargo workshop` alias.
  - `.githooks/`: optional local formatting and pre-push validation hooks.
  - `.github/`: CI, release, package, Miri, and reusable action definitions.
  - `crates/`: all Rust workspace packages plus the two embedded UI trees and the shared TypeScript and CSS package.
  - `design/`: current product and protocol design contracts.
  - `guide/`: mdBook sources, generated guides, and the checked-in rendered book.
  - `images/`: repository and guide artwork.
  - `prompts/`: shipped PromptForge prompt programs.
  - `tools/`: Node and PowerShell build, staging, architecture, and verification utilities.
  - `vibe/`: architecture records and active planning material. `vibe/archdoc.md` exists and is the architecture anchor for this survey.
  - Root manifests and policy files: Cargo workspace and lock files, Rust toolchain and lint configuration, repository policy, licensing, distribution configuration, and the example Gateway configuration.
- Component boundaries:
  - Shared substrate: `shared-progress`, `shared-loopback`, `shared-protocol`, and `shared-sidecar` provide bottom-of-graph facilities used by multiple products.
  - Store: `promptforge-store` owns the run-scoped virtual filesystem boundary and has no product dependency.
  - Executor and library: the `promptforge-*` crates parse Markdown, run sandboxed Lua and model turns, dispatch tools, observe runs, and expose the `promptforge` facade. They consume store and shared abstractions and remain independent of Gateway and Workshop product crates.
  - Gateway: the `gateway*` crates form a separate server process that owns credentials, configuration, routing, remote providers, local inference, logging, speech, and Gateway UI serving. Gateway product crates do not depend on PromptForge or Workshop product crates.
  - Workshop: `workshop` is the Tauri shell, `workshop-server` hosts the in-process API, and `crates/workshop-server/ui` is the authoring UI. Workshop attaches to the Gateway through shared protocol and sidecar facilities; Workshop product crates do not depend on Gateway product crates.
  - UI substrate: `crates/shared-ui` supplies TypeScript and CSS primitives to both embedded UIs without becoming a Rust workspace member.
  - Build tooling: `build-*` crates and `tools/` support compilation, guide generation, sidecar staging, packaging, and CI and are linked into no shipped runtime except during build.
  - Dependency direction is therefore build tooling into build outputs, product crates into shared substrate, and product-specific shells into their internal components, with the four cross-product prohibitions defined by this plan.
- Conventions summary:
  - The workspace uses Rust 2024, Cargo resolver 3, pinned MSRV 1.89, Node.js 22, and locked dependency resolution in CI.
  - Crate prefixes declare ownership: `gateway*`, `promptforge*`, bare `workshop*`, `shared-*`, and `build-*`.
  - Public Rust items require documentation; behavior changes carry tests; production library and serve paths return errors rather than exiting or installing process-global state.
  - Unsafe Rust is forbidden workspace-wide except at the explicitly owned FFI boundary, and external or platform workarounds require an upstream issue URL in the explanatory comment.
  - Long-running operations report through `shared-progress`; model and tool loops are explicitly bounded; security-sensitive network destinations and file paths are revalidated at their owning boundaries.
  - UI sources are TypeScript ESM bundled by esbuild. Bundles are generated into `OUT_DIR`; source, tests, static assets, and shared UI primitives remain checked in.
  - `vibe/archdoc.md` names the stable component map and invariants. Architecture observations flow through the queue described in this plan rather than direct implementation edits to the architecture records.
- Rules manifest:
  - `AGENTS.md` governs the repository root and all descendants, together with the nearest nested file.
  - `crates/gateway/AGENTS.md` governs `crates/gateway/`.
  - `crates/gateway-config/AGENTS.md` governs `crates/gateway-config/`.
  - `crates/gateway-local/AGENTS.md` governs `crates/gateway-local/`.
  - `crates/gateway-logging/AGENTS.md` governs `crates/gateway-logging/`.
  - `crates/gateway-routing/AGENTS.md` governs `crates/gateway-routing/`.
  - `crates/gateway-stt/AGENTS.md` governs `crates/gateway-stt/`.
  - `crates/gateway-stt-backend-whisper/AGENTS.md` governs `crates/gateway-stt-backend-whisper/`.
  - `crates/gateway-stt-engine/AGENTS.md` governs `crates/gateway-stt-engine/`.
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
  - `crates/workshop/icons/AGENTS.md` governs `crates/workshop/icons/`.
  - `crates/workshop-server/AGENTS.md` governs `crates/workshop-server/`.
  - `crates/workshop-server/ui/AGENTS.md` governs `crates/workshop-server/ui/`.
## Execution Instructions

### Component order and construction

1. **Cleanup foundation, Steps 1 through 4.** Establish binding policy and a clean baseline first, repair the isolated build-script defect second, then remove Rust and non-Rust structural enforcement in separate commits because their focused test owners differ. Step 3 proves profile behavior before deleting its structural proxy. Step 4 removes the remaining snapshots, tools, and CI consumers only after their Rust producers are gone.
2. **Gateway runtime, Steps 5 and 6.** Characterize and simplify logging first so process-race failures have trustworthy diagnostics. Then add the lease, startup ordering, and real-process proof together because the lease has no product value until the Gateway holds it and the race suite covers that complete behavior slice.
3. **Workshop runtime, Steps 7 and 8.** Connection-generation identity and Gateway lifecycle hardening are independent behavior slices and may be developed in parallel after Component 2. They remain separate commits because UI reconnect tests and Rust lifecycle tests have different owners.
4. **External verification, Steps 9 and 10.** Migrate the native runner only after local product work, so an unavailable service account cannot block Steps 1 through 8. Package validation and the complete suite follow against one converged commit.

Every step runs its listed focused tests. Steps 4, 6, 8, and 10 also run their component-boundary verification. Only Step 10 runs the complete repository and external matrix. No implementation step directly edits `vibe/archdoc.md` or `vibe/archdoc-next.md`; architecture observations flow through commit messages into the operator-owned queue drained in Step 10.

Preserve the scope exclusions throughout: no `ValidatedConnection` revalidation redesign, logging module split, queue redesign, logger API change, new dependency, registry collection rewrite, unrelated fixture-framework consolidation, signed publication, structural enforcement beyond the four approved Cargo rules, or deletion-driven refactor.

### Step 1: Establish the execution baseline and repository policy [completed]

- Before any edit, require an empty `git status`. Stop if dirty.
- Record the current branch, HEAD, review anchor `84b2c9261f96642bb3fa02836d4e98b13cde8208`, ancestry result, and merge base. If the anchor is not an ancestor, inspect both its direct diff and its merge-base diff. Never restore files from `624e324`.
- Run focused ordinary behavior suites for the areas touched below and record exact commands and outcomes against HEAD.
- Rewrite `AGENTS.md` and the 30 nested `crates/**/AGENTS.md` files according to the complete disposition list above. Remove only `crates/gateway-stt-backend-whisper/AGENTS.md`. Keep material correctness, security, protocol, ABI, lifecycle, data-integrity, and release guidance, while removing inventories, ceremony, topology, source-shape rules, and structural enforcement prescriptions.
- Validate that all 31 original files have one recorded keep, rewrite, or remove disposition and that the policy permits the structural deletions in this plan without weakening behavior tests.
- Commit the policy changes only after whitespace validation and the baseline record are complete.

### Step 2: Correct the cross-platform Gateway build helper [completed]

- In `crates/gateway/build.rs`, change non-Windows `embed_resources` to return unit. Keep the Windows implementation fallible, and keep `main` responsible for propagating Windows resource failures before returning success.
- Do not add a lint exemption; correct the signatures and call sites directly.
- Run focused warnings-denied Clippy for the Gateway build script on a non-Windows target and the existing Windows resource embedding test. Commit this isolated defect and its platform coverage.

### Step 3: Replace Rust structural proxies with direct behavior evidence [completed]

- Rewrite `crates/gateway-stt/tests/it/architecture.rs` around Cargo metadata. Retain exactly the four product rules from the Technical Design across normal, development, build, renamed, and target-specific direct local dependencies. Remove exact allowlists, counts, ceilings, source scans, reference-count scans, ownership-shape checks, and parser-only helpers.
- Before deleting the profile parser, add deterministic direct tests around `crates/gateway/src/profile_switch.rs`, `crates/gateway/tests/it/profiles.rs`, and `crates/gateway-stt/src/generation.rs`. Prove durable persistence before publication, local and speech rollback reconstruction, fatal indeterminate handling with cause retention, and dropped or aborted `SpeechReplacement` rollback without leaked admission.
- Run those profile tests green in the worktree, then delete the profile source parser. The final `architecture.rs` contains only the four Cargo metadata rules.
- Remove the Rust module-ceiling tests and manifests under `crates/gateway-stt*`, `crates/workshop`, and `crates/workshop-server`; structural cases in `crates/workshop-server/tests/it/ratchet.rs`; the manifest parser in `crates/gateway-logging/tests/it/main.rs`; Gateway cfg source assertions in `crates/gateway/src/main.rs`; and exact counts and symbol scans from `crates/promptforge-core/tests/suite/shipped.rs` while preserving shipped-prompt parsing.
- Replace unsafe-lint manifest inspection with compiler checks that deny unsafe code in the three safe STT crates and retain the explicit `gateway-whisper-ffi` policy. Remove Rust parser dependencies, test registrations, and CI consumers with their final use.
- Run focused Cargo metadata adversarial fixtures, profile and STT generation tests, safe-crate compiler lanes, Workshop structural-test replacements, Gateway logging behavior tests, Gateway tests, and PromptForge shipped-prompt tests. Commit all Rust cleanup and direct replacement evidence together.

### Step 4: Remove the remaining structural tools and CI wiring [completed]

- Remove the internal public API snapshot files under `crates/gateway-stt*`, `tools/check-stt-architecture.mjs`, its test, the integration-test ceiling tool, test, and JSON file, and their remaining manifest or `.github/workflows/ci.yml` consumers. Remove obsolete `cargo-modules` and `cargo-public-api` installation.
- Delete `tools/check-workshop-installer-theme.test.mjs` and its CI consumer while preserving `crates/workshop/installer.nsi` and the real unsigned build in `.github/workflows/workshop-installer-smoke.yml`.
- Remove `check-layers.mjs`, source walkers, structural esbuild hooks, enforcement comments, and package-script consumers from both UI packages. Preserve TypeScript, bundling, watching, static assets, production imports, and human layering guidance in `AGENTS.md`.
- Delete the structural-only `crates/gateway-config-ui/ui/src/services/config-store.test.mjs`. Remove CSS source-rule assertions from `titlebar-style.mjs` and `agent-toolbar.mjs`, and remove exact fixture-directory and duplicate case-list coupling from `realtime-wire-fixtures.mjs`, while retaining built-artifact, DOM, focus, decoder, mutation, sequence, and runtime behavior.
- Preserve `tools/check-stt-native-workflow.test.mjs` until Step 9 replaces its workflow extraction with direct PowerShell execution.
- Run focused tests for both UI packages, production bundles, retained Realtime fixtures, installer smoke behavior, JavaScript tools, and clean-tree generation.
- Component boundary: run the cleanup-focused Rust compiler and behavior lanes plus both UI builds and tests. Do not run the complete repository suite. Commit the remaining snapshots, tools, package scripts, and CI cleanup together.

### Step 5: Preserve logging while removing internal duplication

- Limit production edits to `crates/gateway-logging/src/redact.rs` and shared fault support used by `queue.rs`, `worker.rs`, and `writer.rs`. Do not split modules, redesign queue ownership, change the public API, or add dependencies.
- Keep structured credential classification before formatting and the bounded final-output pass. Preserve queue ordering, byte accounting, exact loss, bounded settlement, detached loss, stall handling, rotation rename and sync behavior, crash recovery, segment naming, pruning, and disk limits.
- Consolidate only duplicated test fault injection when the resulting harness still reaches direct write, flush, rename, sync, crash, recovery, and release boundaries.
- Run focused `gateway-logging` redaction, queue, worker, writer, rotation, recovery, disk-budget, and shutdown tests, including every retained fault boundary. Commit the characterized simplification without unrelated cleanup.

### Step 6: Establish process-lifetime Gateway ownership

- Add `GatewayInstanceLease` in `crates/shared-sidecar/src/lock.rs`, with its dedicated run-directory path in `paths.rs` and export in `lib.rs`. The nonblocking operating-system lock is owned by its file handle and releases on graceful exit, panic, crash, or termination. Keep parent-side `LaunchLock` unchanged.
- Integrate the lease through `crates/gateway/src/main.rs` and `relaunch.rs`. Acquire it before canonical logging, stale cleanup, recovery, bind, or publication, then re-resolve the connection record. A validated owner triggers ordinary handoff; an absent or stale record lets the lease holder boot.
- Make a lease loser wait only a bounded interval for the owner's validated record, then hand off or exit nonzero with a console-only error. It never initializes canonical logging or mutates shared state.
- Extend `crates/shared-sidecar/src/lock.rs`, `crates/shared-sidecar/tests/it/main.rs`, `crates/gateway/src/main/logging_tests.rs`, `crates/gateway/tests/it/support.rs`, `crates/gateway/tests/it/boot.rs`, and fixtures under `crates/workshop-server/src/test_gateway`.
- Add deterministic real-process cases for direct versus direct, Workshop launch versus direct, dead-owner relaunch, and bounded cleanup. Prove one owner, no losing-process log mutation, no `LaunchLock` deadlock, and operating-system dead-owner recovery on Windows and Linux.
- Run focused shared-sidecar, Gateway boot, relaunch, handoff, diagnostics, logging, and process-race tests.
- Component boundary: run the Gateway and shared-sidecar focused suites on Windows and a Unix host, including all real-process races. Do not run the complete repository suite. Commit the lease API, startup integration, fixtures, and race proof together without directly editing either architecture record.

### Step 7: Carry connection generation through Workshop dictation

- In `crates/workshop-server/ui/src/services/realtime-transcription.ts`, assign every WebSocket a monotonically increasing immutable generation. Typed envelopes for decoded events, connection state, errors, and append, commit, or clear results retain the originating socket generation and the existing current-socket guard.
- Add `activeGeneration` and generation-bearing pending requests, client correlations, commit expectations, item bindings, and retired tombstones in `ui/src/ui/take-registry-types.ts`, `take-registry-state.ts`, `take-registry-events.ts`, and `take-registry.ts`.
- Make newer readiness advance, duplicate readiness remain idempotent, and older readiness or mismatched inputs no-op. Connection loss performs existing rollback, clears closed-generation wire identity, and keeps only local ownership required to settle started work.
- Update `ui/src/ui/realtime-stt.ts` so `setupStt` stamps user, audio, connection, error, server, wire, and capture inputs with their origin. Preserve that generation through interpreted effects and asynchronous request results.
- Extend `ui/test/stt-stream.mjs`, `take-registry.mjs`, `take-registry-regressions.mjs`, and `agent-stt.mjs` with service, reducer, integration, and real two-socket reconnect cases. Prove immediate same-item-ID reuse and rejection of every stale callback, event, error, wire result, user action, audio input, and capture completion class.
- Run focused Workshop UI typecheck, production bundle, Realtime service tests, reducer regressions, and two-socket tests. Update only `design/generic-realtime-stt.md` with the tested contract. Commit the typed service, reducer, integration, tests, and current design text together.

### Step 8: Close publication and bound the Gateway lifecycle

- Replace the publication mutex in `crates/workshop-server/src/gateway_binding.rs` and `gateway_binding/publication.rs` with binding-owned shared `Arc<Mutex<PublicationState>>`. Construct candidates before locking, linearize publish and permanent close together, and return `GatewayPublicationError::PublicationClosed` from every updater clone without advancing generation, storing, or notifying.
- In `crates/workshop/src/gateway/supervisor.rs`, add nonblocking `StopSignal`, a panic-safe completion guard, and typed `Joined`, `Panicked`, or `Detached` shutdown outcomes. Revoke publication, signal stop, and use one absolute deadline; join only after completion and drop the handle at timeout. Make Drop use the same bounded path.
- Represent launched recovery in `crates/workshop/src/gateway/supervisor.rs` and `crates/workshop/src/gateway/boot.rs` as `RecoveryCandidate { child_pid, validated }`. Claim ownership only when validation names the spawned PID. Recheck stop and closure before publication, and send authenticated shutdown to unpublished owned children under a separate one-second deadline. Never kill by PID or remove an uncertain record.
- Wire revocation and bounded outcomes through `crates/workshop/src/gateway.rs`, `crates/workshop/src/main.rs`, and `crates/workshop-server/src/app.rs`. Teardown always continues after `Joined`, `Panicked`, or `Detached`, and late workers cannot publish.
- Extend `gateway_binding/tests/publication.rs`, `atomic.rs`, and `shutdown.rs`, plus `crates/workshop/src/gateway/tests/shutdown.rs`, `recovery.rs`, and boot fixtures. Cover publish-close races, clone permanence, all supervisor outcomes, spurious wakes, exact and mismatched PID, validation failure, cleanup timeout, retained records, late publication, and teardown continuation.
- Preserve network deadlines, framing and body limits, slow-drip defenses, same-socket authority, cancellation before destructive cleanup, credential secrecy, and `LaunchLock` release.
- Run focused Workshop server binding, Workshop supervisor, recovery, boot, teardown, and shared-sidecar shutdown tests.
- Component boundary: run the Workshop UI generation slice from Step 7 and the Workshop shell and server lifecycle slices from this step. Do not run the complete repository suite. Commit publication closure, supervision, recovery authentication, teardown wiring, and tests together without directly editing either architecture record.

### Step 9: Migrate the native runner to one PowerShell contract

- External gate: under the actual native Whisper service account, verify that `PROMPTFORGE_RUST_1_89_0_BIN` is an absolute directory containing regular `cargo.exe` and `rustc.exe` files and that both report exactly Rust 1.89.0. If unavailable, leave this step incomplete with the account and failure boundary. Do not infer success or block the completed local Steps 1 through 8.
- Add `tools/validate-rust-1.89.0.ps1`. It reads only that environment contract, performs no discovery or installation, and rejects unset, relative, nonexistent, mixed, malformed, missing, or wrong-version inputs.
- Rewrite `tools/check-stt-native-workflow.test.mjs` to execute the script with controlled fake executables, removing all YAML extraction and workflow-text assertions.
- Update the `native-whisper` job in `.github/workflows/stt-miri.yml` to run the script before cache restoration and use only the validated directory. Preserve the Miri matrix and explicit native FFI lint policy.
- Run the focused PowerShell fixture matrix for every accepted and rejected input class, plus the native-job preflight where the service account is available. Commit the script, direct tests, and workflow migration together.

### Step 10: Validate packages, converge verification, and drain architecture observations

- Add a manual nonpublishing mode to `.github/workflows/release-workshop.yml`. Read the expected Workshop version from the workspace, build without signing credentials or updater signatures, upload temporary artifacts, and run the existing installer tests for Windows, macOS ARM, macOS Intel, Linux x64, and Linux ARM.
- Make `publish` unreachable in nonpublishing mode with an explicit job condition while preserving ordinary tag-triggered release behavior. Ensure every build and test job reports and checks out the same execution SHA.
- Commit the workflow change, then run the complete verification only against that commit: `CI / check`, `CI / ui`, `CI / check-workshop`, `CI / check-workshop-linux`, `CI / msrv`, `CI / supply-chain`, `STT Miri / pure-stt-state`, `STT Miri / native-whisper`, and the nonpublishing Workshop package matrix.
- Record exact commands, execution SHA, CI URLs, runner and fixture identity, artifact names and hashes, and external failures. A blocked runner or package lane remains incomplete.
- Run one review-only search for orphaned structural tools, snapshots, obsolete dependencies, stale policy claims, and structural gates. Do not retain a parser, policy linter, source search, or CI search. Require no structural enforcement beyond the four Cargo rules, no unrelated refactor, no generated-tree dirt, and no dated-plan or historical-log rewrite.
- Component boundary: this complete verification is the sole full-suite run. Regenerate guides and UI bundles and require a clean worktree.
- Collect architecture observations emitted by the ten commit messages, then stop for the operator-owned `vibe/archdoc-next.md` queue drain. The implementation executor never edits or commits `vibe/archdoc.md` or `vibe/archdoc-next.md`. Resume closure only after the operator states that every observation was promoted, rejected, or intentionally left open.
