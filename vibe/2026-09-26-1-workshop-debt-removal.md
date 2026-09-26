---
name: Workshop debt removal
overview: "Remove the three debts that the 43 workshop fixes and structure commits (3e31cd23..11da61d8) introduced or worsened: the tree listing following links outside the grants, the merged gateway readiness wait losing its test seam, and one stale agent-frame comment."
todos: []
isProject: false
---

# Workshop debt removal

<product-contract>

## Product Requirements

- Scope and target work:
  - Repository root `C:\Users\Vinnie\cursor\promptforge`, branch `master`. All paths are relative to that root.
  - Target: the 43 commits in `3e31cd23..11da61d8` (baseline `upstream/master` at `3e31cd23`, "Close plan: multi-product-docs-site"; endpoint `master` at `11da61d8`, "Close plan: workshop structure"). They carry two plans: `vibe/2026-09-25-2-workshop-fixes.md` (21 commits through `42edc053`) and `vibe/2026-09-25-3-workshop-structure.md` (22 commits after it). This is the content of [cppalliance/promptforge#75](https://github.com/cppalliance/promptforge/pull/75).
  - Disposition checked at `11da61d8`. Every accepted debt below is still present there.
- Cleanup goals:
  - The workspace tree listing never opens or reads a link's target outside the grants to decide how to list it, and never reports a target's size or modified time.
  - The desktop app's gateway readiness wait can be driven by tests without wall-clock time, and its tests stop relying on sleeping writer threads, elapsed-time assertions, and released ephemeral ports.
  - `crates/workshop/ui/src/services/protocol.ts` names the right Rust files for the agent frames.
- Non-goals:
  - No change to any wire frame shape, route, persisted format, or public API.
  - No fix for the pre-existing revoke race (see Exposed pre-existing debt).
  - No new structural checks or retired-string gates.
  - No edits outside `crates/workshop/**` and this plan's repository copy under `vibe/`.
- Success criteria:
  - Every work item under Execution Instructions is done, with its check passing.
  - The workshop exit commands in the Testing Plan are at least as green as the structure plan's recorded exit results.

## Functional Specification

### Debt Inventory

- **DEBT-FIX-01: the tree listing follows every link with a stat the grants never check (introduced).**
  - Evidence: `8ab90359` "List linked folders as directories" added the follow. `8272d3dc` moved it unchanged into `crates/workshop/workspace/src/workspace/tree.rs`. At `11da61d8`, `Workspace::directory_listing` (lines 61 to 112) calls `fs::metadata(entry.path())` for every entry where `own.is_symlink()` is true (lines 76 to 83), then takes `kind`, `size`, and `modified_ms` from the followed metadata. Only the listed directory goes through `confine_existing`. At the baseline the listing used only `entry.metadata()` and never touched a link's target.
  - Impact:
    - Availability: a stale link to a share that's down or VPN-only makes the followed open wait out the network connect. The whole listing then misses the route's 10 second deadline, so the parent folder answers 408 and none of its entries can be browsed. The tree refetches restored expanded folders on render (`crates/workshop/ui/src/parts/workspace/workshop-panel.ts`), so this happens without the user touching the link. On Unix, a link into a dead hard-mounted NFS path can pin a blocking-pool thread per attempt. This is inferred from the code and wasn't measured.
    - Credentials (Windows, conditional): a directory or file symlink with a UNC target inside a granted folder makes the listing open `\\host\share`, which sends the user's NTLM authentication to that host. Before `8ab90359` that only happened when the user opened the link itself. File links count too, because the `.filter(fs::Metadata::is_dir)` runs after the open.
    - Contract: it contradicts `crates/workshop/workspace/src/lib.rs` lines 19 to 20 ("symlink escapes, and UNC aliases cannot reach outside a grant").
  - Reversal cost: low. The change is local to `tree.rs` and `workspace/tests/jail.rs`, with no wire, persisted, or public change.
  - Target state: on Windows the listing classifies a link from the link's own enumeration attributes and never opens the target. On every platform, `size` and `modified_ms` come from the link itself.
- **DEBT-STRUCT-01: the merged readiness wait lost its only injectable pause, and its tests went back to the wall clock (worsened).**
  - Evidence: `4b8fec5a` "Merge boot's launch wait and poll past failed probes" deleted boot's `wait_for_launched_file_with<Resolve, Pause>`, the pause seam that `ad914a83` added, and routed boot through `wait_for_launched_file_cancellable_with<Health, Resolve>` in `crates/workshop/desktop/src/gateway/supervisor/launch.rs` (lines 107 to 185). That function reads `Instant::now()` for its deadline (lines 119, 126, 172) and sleeps in `cancellation.wait_timeout(RECOVERY_POLL_INTERVAL)` (line 181), and neither is injectable. In `crates/workshop/desktop/src/gateway/tests/boot.rs`:
    - `the_launch_wait_returns_once_the_file_appears_and_answers` (line 261) went back to a `std::thread::sleep(50ms)` writer thread and lost its `pauses == 1` assertion.
    - `the_launch_wait_fails_at_its_budget_with_the_last_probe_error` (line 346) asserts `started.elapsed() >= budget` and `probes >= 2` against a real 600 ms budget.
    - `dead_port()` (line 253) releases an ephemeral port before the test uses it.
  - Recurrence: the same cause has been corrected under five earlier plans: `df37f339`, `267713e2`, `84a234ac`, `6b23dd75`, and `26c3be6d` with `ad914a83`. `ad914a83` fixed this exact test.
  - Impact:
    - The budget test fails if the first probe returns after the deadline, which takes a stall of about 325 ms.
    - A parallel process can be handed `dead_port()`'s port, which breaks the exact `probes == 2` assertion in the replaced-file test.
    - No test proves the poll cadence anymore.
    - All of this is inferred from the code. No failure has been observed.
  - Reversal cost: low. Everything is `pub(in crate::gateway)` or private in one crate.
  - Target state: the wait takes one injected time seam covering its deadline reads and its pause, and the three affected tests run with no real sleeps, elapsed-time checks, or released ports.
- **DEBT-STRUCT-02: the `protocol.ts` agent-frame section still points at `workshop-protocol` (introduced, trivial).**
  - Evidence: `664f9e45` "Move the agent frames into the server" moved the session frames to `crates/workshop/server/src/agents/wire.rs`. It updated the file header and two type docs in `crates/workshop/ui/src/services/protocol.ts`, but not the section comment at lines 102 to 105, which still says "the frame structs in crates/workshop/protocol/src". Only `InputRequiredFrame` and `InputCancelledFrame` still live there, in `crates/workshop/protocol/src/input.rs`. `14789ea4`'s closing doc sweep missed it too.
  - Impact: the file contradicts itself and sends a maintainer to the wrong crate first. `agent-frames.json` still catches drift in the wire shape.
  - Reversal cost: one comment.
  - Target state: the comment names `wire.rs` for the session frames, `protocol/src/input.rs` for the input-wait frames, and `server/src/agents/socket.rs` for the routing.
- **Exposed pre-existing debt (reported separately, not counted, not in scope):**
  - DEBT-FIX-X01: revokes aren't serialized with workspace switches. `revoke_and_persist` in `crates/workshop/workspace/src/workspace/backing.rs` (line 97) never takes `switches`, and it reads `backing_file()` only after awaiting the blocking removal (line 106). So a revoke racing an open, Save As, or the shutdown close can mirror into the wrong workspace file or into none. A revoked folder can then come back on the next open.
  - `0ee7d67d` fixed the same race for grants and recorded this gap in its message. The function body is identical at the baseline, so the target didn't add it. The analyst classed it as exposed and the challenger as unrelated pre-existing. The code facts are confirmed.
- **Rejected candidates: 55.**
  - 22 residual-but-acceptable: seams, parameter clusters, and private mirrors that compile-time exhaustiveness or existing tests already protect.
  - 14 weak or speculative: no demonstrated consequence. Examples are the boot probe attempt budget dropping from 2 s to 100 ms, and non-finite metrics.
  - 14 false: disproved against the code, for example the `/ws` origin check, the chat filter, and the SSE CR handling.
  - 5 unrelated pre-existing.

</product-contract>
<implementation-contract>

## Technical Design

- **Tree listing (DEBT-FIX-01)**, in `Workspace::directory_listing` in `crates/workshop/workspace/src/workspace/tree.rs`:
  - Windows: when `own.is_symlink()`, classify with `std::os::windows::fs::FileTypeExt::is_symlink_dir()` on `own.file_type()`. That reads `FILE_ATTRIBUTE_DIRECTORY` from the enumeration data, which junctions and directory symlinks carry. Never call `fs::metadata` on the entry. A dangling directory link then lists as a directory, and opening it still fails confinement.
  - Unix: keep the followed `fs::metadata` only to decide `kind`. A Unix `stat` doesn't authenticate.
  - Every platform: `size` is 0 for any link, and `modified_ms` comes from `own`. That extends the existing file-link rule ("never exposes its target's size or timestamp") to folder links.
  - Update the `directory_listing` doc comment, and the confinement invariant in `crates/workshop/workspace/src/lib.rs` lines 16 to 20. They should say that listing may classify a link from its own attributes (Windows) or from a type-only stat (Unix), never opens it, and never reports its target's size or time. Opening still confines.
  - Keep `tree.rs` at or under 500 physical lines.
- **Readiness wait seam (DEBT-STRUCT-01)**, in `crates/workshop/desktop/src/gateway/supervisor/launch.rs`:
  - Add one private time seam to `wait_for_launched_file_cancellable_with`. It can be a small trait or struct with `now() -> Instant` and `pause(Duration, &CancellationToken) -> bool`, where the bool reports cancellation. Every `Instant::now()` in the wait and the `wait_timeout` at line 181 go through it.
  - The production wrapper `wait_for_launched_file_cancellable` passes the real implementation (`Instant::now` and `cancellation.wait_timeout`), so production behavior stays byte-for-byte the same.
  - Tests get a fake whose `now` reads a `Cell<Instant>` that `pause` advances, with an optional per-pause hook.
  - Visibility stays `pub(in crate::gateway)`. Update every caller: the production wrapper, `launch_wait_with` in `tests/boot.rs`, and any recovery test that calls the wait directly. Find them with `rg -n "wait_for_launched_file_cancellable_with" crates/workshop/desktop`.
  - The desktop crate is exempt from the 500-line limit.
- **Comment (DEBT-STRUCT-02):** replace lines 103 to 105 of `crates/workshop/ui/src/services/protocol.ts` with: "The Rust half of this family is the session frame structs in crates/workshop/server/src/agents/wire.rs, the input-wait frames in crates/workshop/protocol/src/input.rs, and the routing in crates/workshop/server/src/agents/socket.rs."
- No module, interface, data, protocol, or lifecycle change beyond these.

</implementation-contract>
<verification-contract>

## Testing Plan

- DEBT-FIX-01, in `crates/workshop/workspace/src/workspace/tests/jail.rs`, run with `cargo nextest run --locked -p workshop-workspace --all-features` on Windows and Linux:
  - `a_linked_folder_pointing_outside_the_grant_lists_as_a_directory_but_never_opens` also asserts that the entry's `modified_ms` equals `modified_ms(&fs::symlink_metadata(&link))`, and that `size` is 0.
  - `a_dangling_link_lists_as_its_own_entry` asserts `EntryKind::Directory` under `cfg(windows)` (own attributes of a junction) and `EntryKind::File` under `cfg(unix)`, and still asserts one entry, `size == 0`, and `exists`.
  - `a_linked_folder_inside_a_grant_lists_as_a_directory_and_opens`, `a_linked_file_pointing_outside_the_grant_lists_by_its_own_metadata`, and `a_junction_inside_a_grant_pointing_outside_is_rejected` pass unchanged.
  - Keep the existing `symlink_unavailable` CI guard on every link test.
- DEBT-STRUCT-01, in `crates/workshop/desktop/src/gateway/tests/boot.rs`, run with `cargo nextest run --locked -p workshop` and the staged sidecar, several times in a row:
  - `the_launch_wait_returns_once_the_file_appears_and_answers` writes the file inside the fake's first pause, with no thread, and asserts exactly one pause.
  - `the_launch_wait_fails_at_its_budget_with_the_last_probe_error` uses the fake time seam and a `health` closure that always returns a constructed `HealthError::Timeout { .. }`. It asserts an exact probe count derived from the budget and `RECOVERY_POLL_INTERVAL`, and that the message carries both the budget failure and the last probe error. It has no `Instant::now` or `elapsed`.
  - `the_launch_wait_completes_when_a_dead_port_file_is_replaced_by_a_live_one` fails the first probe by construction in its `health` closure instead of probing a released port, and keeps `probes == 2`.
  - `dead_port()` is deleted.
  - `rg -n "thread::sleep|elapsed\(\)|dead_port" crates/workshop/desktop/src/gateway/tests/boot.rs` finds no hit in those three tests.
- DEBT-STRUCT-02: `rg -n "frame structs in" crates/workshop/ui/src/services/protocol.ts` shows the new wording. `npm run typecheck` and `npm test` pass in `crates/workshop/ui`.
- Exit commands, each at least as green as the structure plan's exit record in `vibe/2026-09-25-3-workshop-structure.md` Step 21:
  - `cargo nextest run --locked -p workshop-workspace --all-features` (Windows)
  - `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`
  - `cargo nextest run --locked -p workshop-server --features headless`
  - `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`
  - `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings`
  - `cargo fmt --all --check`
  - `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`
  - `npm run build`, `npm test`, and `npm run typecheck` in `crates/workshop/ui`
  - Setup for the `workshop` package: `npm ci` in both UI packages, then `cargo build --locked -p gateway --no-default-features`, then `node tools/stage-gateway-sidecar.mjs stage --target x86_64-pc-windows-msvc --source target/debug/promptforge-gateway.exe`.

</verification-contract>
<decision-record>

## Decision Record

- Reversible decisions and consequences:
  - **DEBT-FIX-01: classify Windows links from their own attributes.**
    - Chosen because it's the only remedy that never touches the target, chains included. It needs no path parsing and uses a stable std API (Rust 1.64 and later).
    - Consequence: on Windows a dangling directory link lists as a folder that fails to open. That still matches the fixes plan's "a dangling link lists by its own metadata".
    - Consequence: an outside-grant folder link now reports its own modified time instead of the target's. That reverses one deliberate choice in `8ab90359`, to match the file-link rule.
  - **DEBT-FIX-01: keep the type-only stat on Unix.** A Unix link has no type flag, and a `stat` sends no credentials. The dead-NFS stall stays as residual risk.
  - **DEBT-STRUCT-01: one seam covering both the clock and the pause.** Chosen over a pause alone, because a pause alone leaves the budget test on the wall clock through `Instant::now()`. It's one private seam, not two more closure generics, so the parameter cluster doesn't grow.
  - **DEBT-STRUCT-02: reword the comment and add no gate.** A widened retired-string search wouldn't generalize to the next move.
- User-resolved architecture choices: none were needed. Every remedy is local, private, and reversible, with no public, persisted, wire, ownership, dependency-direction, or trust-boundary change.
- Rejected alternatives:
  - DEBT-FIX-01: refuse UNC-prefixed link targets after `fs::read_link`. Rejected because `read_link` returns only the first hop, so a relative link to an in-tree UNC link still gets followed. It would also have to recognize `\\?\UNC\`, `\??\UNC\`, `GLOBALROOT` device paths, and mapped drives.
  - DEBT-FIX-01: follow only when the target is lexically inside a grant. Rejected for the same chain problem, unless every hop is checked.
  - DEBT-STRUCT-01: fail the health probe by construction and change nothing else. It's cheaper, but it doesn't restore the poll-count proof or take the budget test off the clock.
- Assumptions and risks:
  - Rust's `DirEntry::metadata` and `file_type` on Windows report junctions as `is_symlink()` with the directory attribute. The current tests already rely on `is_symlink()` for junctions. If `is_symlink_dir()` turns out false for a junction, stop and report instead of falling back to following the target.
  - GitHub-hosted Windows runners can create junctions. The existing CI guard turns the job red if they can't, and that's by design.
  - The credential and stall consequences are inferred from Windows SMB and network behavior and weren't reproduced.

### Deferred and Out of Scope

- DEBT-FIX-X01, the revoke race with workspace switches. It's pre-existing, not added by the target. Revisit as its own fix:
  - Look up the literal request path as a stored grant key before canonicalizing.
  - Take `switches` for the whole of `revoke_and_persist`, the way `0ee7d67d` did for grants.
  - Add a race test beside `a_grant_racing_a_workspace_open_never_answers_success_and_then_loses_the_grant` in `crates/workshop/workspace/src/workspace/tests/switch.rs`.
- The Unix dead-NFS stall in the tree listing. Revisit if Workshop starts supporting network-mounted grants.
- Boot's per-attempt probe budget dropping from 2 s to 100 ms in `4b8fec5a` (weak, no demonstrated consequence). Revisit if a boot ever fails with `ProofInterrupted` against a live gateway. A fixture gateway whose `/v1/models` answers after 150 ms would settle it.
- A slow grant canonicalize holding up the shutdown close (weak). Revisit together with DEBT-FIX-X01, whose fuller remedy canonicalizes before taking the guard.
- The `models` UI guard rejecting a `null` description from non-PromptForge upstreams (weak). Revisit if a third-party catalog upstream is supported.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo workshop` (alias for `cargo run -p build-workshop --`; builds the gateway, stages the sidecar, builds the desktop app, then removes the staged copy). Gateway only: `cargo build --locked -p gateway` (the default member). Run `npm ci --prefix crates/workshop/ui` and `npm ci --prefix crates/gateway/config-ui/ui` once first, because both UIs are bundled by esbuild during the Cargo build.
- Focused test command pattern: `cargo nextest run --locked -p <crate> --all-features <test-name-filter>`, dropping `--all-features` for `workshop`, `workshop-server`, and `workshop-server-api`. Gateway integration test: `cargo test --locked -p gateway --no-default-features --features test-fixtures --test it <test_name>`. Workshop UI test file: `node --test test/<feature>.mjs` from `crates/workshop/ui`.
- Component test command pattern: `cargo nextest run --locked -p <crate> --all-features`, then `cargo test -p <crate> --all-features --doc`. Workshop crates: `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`, `cargo nextest run --locked -p workshop-server --features headless`, and `cargo nextest run --locked -p workshop-workspace --all-features`. UI packages: `npm run typecheck`, `npm run build`, and `npm test` in `crates/workshop/ui` or `crates/gateway/config-ui/ui`. Boundary and structural harness: `cargo test -p build-xtask`.
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api` and `cargo test --doc -p workshop -p workshop-server -p workshop-server-api`.
- Linter command: `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings` and `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`, plus the headless gate `cargo check -p gateway --no-default-features`. Never run a standalone `cargo check --workspace` beside the clippy runs. TypeScript: `npm run typecheck` in each UI package.
- Formatter check command: `cargo fmt --all --check` (Rust only; no TypeScript formatter is configured).
- Docs command: `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api` and, for the facade with default features, `cargo doc -p promptforge --no-deps`, both with `RUSTDOCFLAGS="-D warnings"` (in PowerShell set `$env:RUSTDOCFLAGS` for the call and clear it afterward). Also `cargo doc --locked --no-deps -p workshop-server --document-private-items`, the user guide via `cargo xtask site --books-only`, and the facade surface via `cargo +<nightly pinned in crates/build-xtask/src/api/toolchain.rs> xtask api --check`.
- Test placement and naming conventions: Rust unit tests live in a kebab sibling `<module>-tests.rs` wired at the end of the module with `#[cfg(test)] #[path = "<module>-tests.rs"] mod tests;`, or in `<dir>/tests.rs` when the module is already a directory. Integration tests live in the crate's top-level `tests/`, typically as one `it` binary (`tests/it/main.rs` plus one module per area such as `boot.rs` or `chat_gate.rs`, helpers in `support.rs` or `tests/common/`, data in `tests/fixtures/`). Criterion benches sit in `benches/` of the engine and lua crates. Test functions are snake_case behavior sentences, for example `a_direct_launch_recovers_the_lease_from_a_terminated_owner`. `unwrap` and `expect` are allowed only in tests (`clippy.toml`). Workshop UI tests are `crates/workshop/ui/test/<feature>.mjs` run by `node --test` with shared helpers in `test/helpers/`; gateway config UI tests are colocated `src/**/*.test.mjs`. Behavior changes ship with tests in the same change.
- Directory map:
  - `crates/`: every Rust crate plus the TypeScript UIs. Public crates sit at the root (`promptforge`, `harness-api`, `gateway-api-types`, `gateway-api-discovery`, `shared-error-source`, `shared-loopback`), beside meta tooling (`build-xtask`, `build-workshop`, `build-ui`, `build-user-guide`, `build-llama-cuda`), `workspace-hack` (cargo-hakari), `shared-ui` (shared TypeScript and CSS controls, not a Rust crate), and four manifestless private family containers: `promptforge-internal/`, `gateway/`, `harness/`, `workshop/`.
  - `crates/workshop/ui/`: the Workshop SPA in TypeScript, layered as `src/base`, `src/services`, `src/parts`, and `src/tokens`, with tests in `test/`.
  - `guide/`: mdBook user guide books, landing page, and site chrome.
  - `prompts/`: example prompt pipelines.
  - `tools/`: Node scripts for sidecar staging and live TTS checks, plus the repo's documentation tool.
  - `vibe/`: the architecture doc (`archdoc.md`), dated plan run logs, and `scratch/` step logs.
  - `.github/workflows/`: CI, nightly, release, installer smoke, site, and STT Miri workflows.
  - `.githooks/`: pre-commit format check; pre-push headless gateway check, clippy, and cargo deny.
  - `.cargo/`: the `cargo workshop` and `cargo xtask` aliases and the Windows rust-lld static-CRT linker setup.
  - `.config/`: nextest profiles (a heavy group for STT tests) and hakari config.
  - `images/`: README and site images.
  - `local/` (gitignored local gateway config, profiles, and fixtures), `target/` and `target-msrv/` (build output).
  - Root files: `Cargo.toml` (workspace members, default member `crates/gateway/app`, shared dependencies and lints), `AGENTS.md` (repo rules and verification gates), `rust-toolchain.toml` (stable), `clippy.toml`, `rustfmt.toml`, `deny.toml`, `dist-workspace.toml`.
- Component boundaries:
  - PromptForge: the `promptforge` facade is the family's only public crate. The private `promptforge-internal/` holds engine (sans-I/O executor), types (wire vocabulary only), vfs, lua, parser, store, and model-client. Depends only on shared crates; never on gateway, workshop, or harness crates.
  - Gateway: the public pair `gateway-api-types` and `gateway-api-discovery`. The private `gateway/` holds app (the server binary), routing, local, cloud-providers, config, config-ui, logging, progress, protocol, web-search, and the nested `stt/` subsystem, where only `gateway-stt` is visible to the family. Never depends on promptforge, workshop, or harness crates.
  - Harness: the public `harness-api`. The private `harness/` holds runner, models, capabilities, log (the Turso run log), sessions, web, webfetch, and web-search. May depend on `promptforge`, the gateway public pair, and shared crates; never on workshop or private gateway crates.
  - Workshop: the private `workshop/` holds the desktop app (package `workshop`), which depends on `workshop-server-api` and never on `workshop-server`. Server crates form four tiers, each depending only on tiers below it: server (`workshop-server`), features (`workshop-user-state`, `workshop-workspace`), services (`workshop-gateway`, `workshop-menu`, `workshop-status`), and vocabulary (`workshop-protocol`, `workshop-registry`, `workshop-support`). Reaches other products only through `harness-api`, `promptforge`, and the gateway public pair.
  - Shared (`shared-error-source`, `shared-loopback`): no product dependencies.
  - Enforcement: `cargo test -p build-xtask` checks the tier graph, container privacy, the product-boundary matrix, the mandatory `## Invariants` marker, and the 500-line ceiling. The rules bind normal, dev, build, and target-specific dependencies.
- Conventions summary:
  - Rust edition 2024 on the stable toolchain; `--locked` in CI; every dependency declared once in the root `[workspace.dependencies]` with a comment explaining its pin; cargo-hakari `workspace-hack` in every member.
  - Workspace lints: `unsafe_code` forbidden outside owned boundaries (each unsafe block documents its safety invariants), clippy `all` and `pedantic` denied, `unwrap_used` and `expect_used` denied outside tests, `missing_docs` warned, broken rustdoc links denied.
  - Flat source directories: one or two related files sit beside the parent as `parent-label.rs` wired with `#[path]`; three or more become a subdirectory, and groups convert in both directions when touched.
  - Every workshop-* and harness-* `lib.rs` opens with a `//!` doc holding a `## Invariants` section; files in those crates stay at or under 500 lines.
  - Comments explain only non-obvious constraints; platform or external-bug workarounds cite an upstream issue URL.
  - Error and status messages are written for model consumption: concise, factual, naming required versus actual.
  - Run-log JSON round-trips exactly: sorted keys, finite numbers, `float_roundtrip`, never `preserve_order`.
  - Cargo features gate real build constraints only; library and serve paths return errors instead of exiting or installing process-global state.
  - New structural checks need explicit user approval; plans may not introduce them on their own.
  - SPA: imports flow from parts to services to base; CSS sits beside its TypeScript and uses `--ws-*` tokens; no `localStorage` (state goes through `ui-storage` to the server); VS Code command ids and context keys are used verbatim; each feature registers through `registerAction` in an eager `*.contribution.ts`.

</project-survey>
<execution-plan>

## Execution Instructions

<step-1>

### Step 1: List links from their own attributes [completed]

- Component: Workspace tree listing
- Debt: DEBT-FIX-01.
- Component placement: first. It's the only debt with a user-facing consequence (a stalled listing, and on Windows the user's NTLM authentication sent to a link's UNC host). It touches only `workshop-workspace` and needs no staged sidecar.
- Pieces: the link classification in `tree.rs` with its docs, and the link tests in `jail.rs`. Built jointly, because the Windows dangling-link expectation flips from file to directory with the code change, so neither half is green without the other.
- Artifacts:
  - `crates/workshop/workspace/src/workspace/tree.rs`, `Workspace::directory_listing` (lines 61 to 112): replace the `own.is_symlink()` branch (lines 76 to 83).
    - Windows (`cfg(windows)`): take `kind` from `std::os::windows::fs::FileTypeExt::is_symlink_dir()` on `own.file_type()`. Never call `fs::metadata` on the entry.
    - Unix (`cfg(unix)`): keep the followed `fs::metadata(entry.path())`, but only to decide `kind`.
    - Every platform: `size` is 0 for any link, and `modified_ms` comes from `own`.
    - Rewrite the `directory_listing` doc comment to say that listing may classify a link from its own attributes (Windows) or from a type-only stat (Unix), never opens it, and never reports its target's size or time, while opening still confines.
    - Keep the file at or under 500 physical lines (113 at `11da61d8`).
  - `crates/workshop/workspace/src/lib.rs` lines 16 to 20: give the confinement invariant under `## Invariants` the same wording.
- Tests, in `crates/workshop/workspace/src/workspace/tests/jail.rs`:
  - `a_linked_folder_pointing_outside_the_grant_lists_as_a_directory_but_never_opens` also asserts that the entry's `modified_ms` equals `modified_ms(&fs::symlink_metadata(&link))` and that `size` is 0.
  - `a_dangling_link_lists_as_its_own_entry` asserts `EntryKind::Directory` under `cfg(windows)` and `EntryKind::File` under `cfg(unix)`, and still asserts one entry, `size == 0`, and `exists`.
  - `a_linked_folder_inside_a_grant_lists_as_a_directory_and_opens`, `a_linked_file_pointing_outside_the_grant_lists_by_its_own_metadata`, and `a_junction_inside_a_grant_pointing_outside_is_rejected` pass unchanged.
  - Every link test keeps its `symlink_unavailable` CI guard.
- Verify: `cargo nextest run --locked -p workshop-workspace --all-features` on Windows, and on Linux through CI.
- Stop condition: if `is_symlink_dir()` is false for a junction, stop and report. Never fall back to following the target.
- Dependencies: none.
- Commit: the listing change, the doc and invariant updates, and the jail tests.

</step-1>

<step-2>

### Step 2: Put the readiness wait behind a time seam [completed]

- Component: Desktop readiness wait
- Debt: DEBT-STRUCT-01.
- Component placement: second. It's independent of Step 1, but it's the first step that needs the staged sidecar, which then stays staged for Step 4.
- Pieces: the time seam in `launch.rs` with its production and test callers, and the three rewritten tests in `boot.rs`. Built jointly, because adding the seam parameter breaks every caller at compile time, and the rewritten tests are the seam's only proof.
- Artifacts:
  - `crates/workshop/desktop/src/gateway/supervisor/launch.rs`:
    - One time seam, a small trait or struct, with `now() -> Instant` and `pause(Duration, &CancellationToken) -> bool`, where `true` means cancelled. Add a real implementation backed by `Instant::now` and `cancellation.wait_timeout`.
    - `wait_for_launched_file_cancellable_with` (line 107) takes the seam. The `Instant::now()` reads at lines 119, 126, and 172 and the `cancellation.wait_timeout(RECOVERY_POLL_INTERVAL)` at line 181 all go through it. The wait stays `pub(in crate::gateway)`, and the seam is no wider than that.
    - `wait_for_launched_file_cancellable` (line 91) passes the real implementation, so production behavior stays byte-for-byte the same.
    - `RECOVERY_POLL_INTERVAL` (line 17) is private today. Widen it to `pub(in crate::gateway)` so the budget test can derive its probe count from it.
  - `crates/workshop/desktop/src/gateway/supervisor.rs` (line 20): re-export the seam, its real implementation, and `RECOVERY_POLL_INTERVAL` beside the wait's existing re-export.
  - `crates/workshop/desktop/src/gateway/tests/cancellation.rs`, `exit_joins_a_supervisor_blocked_in_health_wait_before_resolve` (direct call at line 107): pass the real implementation.
  - `crates/workshop/desktop/src/gateway/tests/boot.rs`:
    - A fake seam whose `now` reads a `Cell<Instant>` that `pause` advances by the requested duration. It counts pauses and takes an optional per-pause hook.
    - `launch_wait_with` (line 227) takes the seam. `launch_wait` (line 244) passes the real implementation, so `the_launch_wait_times_out_when_no_file_appears` and `the_launch_wait_rejects_a_key_the_live_process_does_not_accept` stay as they are.
    - Delete `dead_port()` (line 253) and any import it leaves unused.
  - The desktop crate is exempt from the 500-line limit.
- Tests, in `crates/workshop/desktop/src/gateway/tests/boot.rs`:
  - `the_launch_wait_returns_once_the_file_appears_and_answers` (line 261) uses the fake seam. Its per-pause hook writes the file during the first pause, with no thread, and the test asserts exactly one pause.
  - `the_launch_wait_fails_at_its_budget_with_the_last_probe_error` (line 346) uses the fake seam and a `health` closure that always returns a constructed `HealthError::Timeout { .. }`. It asserts an exact probe count derived from the budget and `RECOVERY_POLL_INTERVAL`, and that the message contains both "no validated gateway discovery file" and the last probe error. It has no `Instant::now` or `elapsed`.
  - `the_launch_wait_completes_when_a_dead_port_file_is_replaced_by_a_live_one` (line 314) uses the fake seam. Its first file names a fixed port, different from the live fixture's, that is never probed. The `health` closure fails the first call by construction and writes the live file, then probes the live gateway on the second call. It keeps `probes == 2` and `waited == live`.
- Verify:
  - Setup, once: `npm ci` in `crates/workshop/ui` and in `crates/gateway/config-ui/ui`, then `cargo build --locked -p gateway --no-default-features`, then `node tools/stage-gateway-sidecar.mjs stage --target x86_64-pc-windows-msvc --source target/debug/promptforge-gateway.exe`. The staged sidecar is gitignored. Leave it staged for Step 4.
  - `cargo nextest run --locked -p workshop`, at least three times in a row.
  - `rg -n "thread::sleep|elapsed\(\)|dead_port" crates/workshop/desktop/src/gateway/tests/boot.rs` finds no hit in the three tests above.
  - `rg -n "wait_for_launched_file_cancellable_with" crates/workshop/desktop` shows that every call passes a seam.
  - `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`.
- Dependencies: none.
- Commit: the seam, its callers, and the rewritten tests.

</step-2>

<step-3>

### Step 3: Point the agent-frame comment at the right files [completed]

- Component: UI agent-frame comment
- Debt: DEBT-STRUCT-02.
- Component placement: third. It's independent and trivial, and it gets its own commit so every step has exactly one.
- Pieces: one, the section comment in `protocol.ts`.
- Artifacts:
  - `crates/workshop/ui/src/services/protocol.ts`, under the `// --- Agent-session frames (/agents/ws)` header (line 102): replace the sentence on lines 103 to 105 that runs from "The Rust half of this family is the frame structs in" to "crates/workshop/server/src/agents/socket.rs." with the Technical Design wording, wrapped as `//` lines.
  - Line 105 also starts the next sentence, "Delivery classes mirror the Rust docs:", which continues on line 106. Keep it intact.
  - Keep "frame structs in" on one line so the check below matches.
- Tests: `rg -n "frame structs in" crates/workshop/ui/src/services/protocol.ts` shows the new wording.
- Verify: `npm run typecheck` and `npm test` in `crates/workshop/ui`.
- Dependencies: none.
- Commit: the comment edit alone.

</step-3>

<step-4>

### Step 4: Run and record the exit commands

- Component: Exit record
- Debt: DEBT-FIX-01, DEBT-STRUCT-01, and DEBT-STRUCT-02.
- Component placement: last, because the exit commands judge the finished tree against the structure plan's recorded exit results.
- Pieces: one, the exit run and its record.
- Artifacts:
  - This plan's repository copy, `vibe/2026-09-26-1-workshop-debt-removal.md`, which the run's seed commit creates before Step 1. Record into that copy; don't create another.
  - An `Exit results` list inside this step in that copy. Each line gives the command and its result, with the matching number from the `Exit results` list under Step 21 of `vibe/2026-09-25-3-workshop-structure.md` in brackets beside it.
- Tests (the Testing Plan's exit commands, verbatim):
  - Setup for the `workshop` package, if the sidecar isn't still staged from Step 2: `npm ci` in both UI packages, then `cargo build --locked -p gateway --no-default-features`, then `node tools/stage-gateway-sidecar.mjs stage --target x86_64-pc-windows-msvc --source target/debug/promptforge-gateway.exe`.
  - `cargo nextest run --locked -p workshop-workspace --all-features` (Windows)
  - `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`
  - `cargo nextest run --locked -p workshop-server --features headless`
  - `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`
  - `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings`
  - `cargo fmt --all --check`
  - `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`. In PowerShell, set `$env:RUSTDOCFLAGS` for the call and clear it afterward.
  - `npm run build`, `npm test`, and `npm run typecheck` in `crates/workshop/ui`
- Scope checks:
  - `git diff --name-only 11da61d8..HEAD` lists only paths under `crates/workshop/` and `vibe/`.
  - No wire frame shape, route, persisted format, or public API changed.
  - `crates/workshop/workspace/src/workspace/backing.rs` is untouched, because the revoke race (DEBT-FIX-X01) stays out of scope.
  - No structural check or retired-string gate was added.
- Verify: every exit command is at least as green as its bracketed Step 21 number, and every scope check holds.
- Dependencies: Steps 1, 2, and 3.
- Commit: the repository plan copy with the recorded exit results.

</step-4>

</execution-plan>
