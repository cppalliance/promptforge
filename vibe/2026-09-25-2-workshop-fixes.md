---
name: Workshop fixes plan
overview: Fix the user-facing bugs, the Windows CI gap, and the weakened tests in crates/workshop; delete the unused standalone workshop-server binary; make the docs that agents load match the code; and add a short human crate map. Module structure is left to a separate structure plan.
todos:
  - id: baseline
    content: Record the baseline of every exit command before any change
    status: pending
  - id: save-as
    content: Fix Save As onto an existing file and its timeout, and the reconcile-mismatch dialog wording, with UI tests (Overwrite timeout already fixed in 49d97995)
    status: pending
  - id: decode
    content: Remove the second percent-decode on workspace GET paths; %41 round-trip test; traversal still refused
    status: pending
  - id: windows-ci
    content: Run workshop-workspace tests with --all-features in the Windows check-workshop CI job
    status: pending
  - id: binary
    content: Delete the workshop-server binary, workbench.toml fallback, open dependency; drop the open_browser field (old configs still load); fix guide sentence
    status: pending
  - id: quit
    content: Stop the gateway supervisor before posting /shutdown on quit, with ordering and no-relaunch tests
    status: pending
  - id: menu-memory
    content: Make menu memory writes latest-wins with a sequence number
    status: pending
  - id: roots-signal
    content: Add a grant-set change signal to WorkspaceRoots so revokes reach running agent sessions
    status: pending
  - id: boot-order
    content: Validate loopback before composing state and bind before reopening the last workspace
    status: pending
  - id: workspace-fixes
    content: List linked folders as directories; reproduce and fix the grant-during-open race; fix the UNC test helper
    status: pending
  - id: switch-resync
    content: Resync the UI from GET /workspace/file/current after a workspace-switch 408
    status: pending
  - id: timing-tests
    content: Repair the chat quiet test; replace remaining real-clock windows (heartbeat convergence already repaired in 63ddccb5)
    status: pending
  - id: doc-sweep
    content: Rewrite the two .cursor/rules files and sweep every listed stale doc claim across crates, UI, and repo root
    status: pending
  - id: crate-map
    content: Add crates/workshop/AGENTS.md crate map and the new Vocabulary entries
    status: pending
  - id: exit
    content: Run the exit commands and retired-string checks; record results
    status: pending
isProject: false
---

# Workshop fixes and orientation

<product-contract>

## Product Requirements

The workshop crates came out of a large cleanup with a few user-facing bugs, a CI job that never runs the Windows path-confinement tests, two tests that no longer prove what they claim, and docs that contradict the code in exactly the files agents load most. There is also no document that tells a human what the eleven crates are. This plan fixes those, deletes the unused standalone server binary, and adds a short crate map. It changes behavior only where a fix requires it, and leaves module moves, renames, and API trimming to a separate structure plan.

- Problem and users:
  - Users are the maintainers and agents working on `crates/workshop/` (the desktop app, the in-process server, the subsystem crates, and the workshop UI), and the end users of the desktop app, who hit the Save As and quit bugs.
  - The defects are listed under Functional Specification.
- Goals:
  - Fix every defect in Functional Specification, each with a test in the same change.
  - Run the workspace crate's tests on Windows in CI.
  - Repair the weakened chat quiet test and replace the remaining real-clock windows.
  - Delete the standalone `workshop-server` binary and everything that exists only for it.
  - Make every doc claim listed under Execution Instructions match the code, and add a crate map and vocabulary entries.
- Non-goals:
  - No module moves, file-layout changes, renames, or public-surface trimming beyond what a fix needs.
  - No remote or browser access to the server.
  - No change to the desktop package name `workshop`.
  - No edits to `crates/promptforge*`, `crates/harness/*`, or `crates/gateway/*`.
  - No new structural checks.
- Success criteria:
  - Every work item under Execution Instructions is done.
  - Every exit command is at least as green as its recorded baseline.
  - The retired-string checks in the Testing Plan exit criteria pass.
- Constraints:
  - Repository root `C:\Users\Vinnie\cursor\promptforge`, branch `master`, at or after `890ae2f6` ("Close plan: vibe2 debt removal"). All paths are relative to that root. Cited line numbers are at `ce10a8eb`. Seven files under `crates/workshop` changed between `ce10a8eb` and `890ae2f6`, so their citations may be off by a few lines: `ui/src/parts/editor/editor-panel.ts`, `ui/test/editor-save-timeout.mjs`, `server/tests/it/heartbeat_loop/startup_convergence.rs`, `server/src/agents.rs`, `server/src/assets.rs`, `server/src/lib.rs`, and `workspace/src/workspace/tests/jail.rs`. Locate code by content.
  - Edit scope: `crates/workshop/**`; `Cargo.lock` (for the removed `open` dependency); `.github/workflows/ci.yml`; root `AGENTS.md`, `README.md`, and `tools/document.md`; `.cursor/rules/workshop-architecture.mdc` and `.cursor/rules/workshop-spa.mdc`; `guide/src/gateway/11-serving-and-observing.md` (one clause) plus the files `cargo run -p build-user-guide` regenerates from it; this plan's repository copy under `vibe/` and `vibe/ACTIVE`. For any edit outside the scope, stop and report.
  - Behavior changes ship with tests in the same change. Preserve existing product and behavior tests.
  - Files in `workshop-*` crates stay at or under 500 physical lines. The desktop crate (package `workshop`) is exempt. Near the limit: `crates/workshop/workspace/src/workspace.rs` (491), `crates/workshop/workspace/src/workspace_file.rs` (490), `crates/workshop/server/tests/it/heartbeat_loop.rs` (487), `crates/workshop/workspace/src/workspace/backing.rs` (469), `crates/workshop/server/tests/it/session/menu/restart.rs` (433), `crates/workshop/server/tests/it/session/menu.rs` (432). If an edit would pass 500, split the file first along an existing seam (for `workspace.rs`, move tree listing into `crates/workshop/workspace/src/workspace/tree.rs`).
  - A fix whose bug is marked inferred starts with a test that reproduces it. If no deterministic reproduction exists, record that in this plan's repository copy and skip the code change.
- Open questions: None

## Functional Specification

Each defect below has an observable before and after. Save As, the directory listing, and the quit gesture change for end users. Everything else changes internal state, CI, or tests.

- Actors and workflows:
  - A desktop user saves, saves as, and overwrites files in the editor, opens and switches workspaces, grants and revokes folders, and quits through the native menu or File > Exit.
  - A maintainer runs CI and the local gates.
  - An agent reads `.cursor/rules/workshop-*.mdc`, the `AGENTS.md` files, and crate docs before editing.
- Inputs and outputs:
  - **Save As onto an existing file.** Today `saveAs` writes with a `null` token (`crates/workshop/ui/src/parts/editor/editor-panel.ts:249`). The server refuses a null-token write to an existing file with 409 (`crates/workshop/workspace/src/workspace.rs:385-387`), and the conflict dialog then acts on the editor's old path (`editor-panel.ts:259-260`, `:361`, `:446-448`). After: the OS save dialog's replace confirmation counts as consent. On a 409, `saveAs` reads the target's current token and retries the write once with it, then retargets the panel. If the target can't be read (not UTF-8 text, too large, not a file), or the retry conflicts again, it shows an error naming the target and doesn't retarget. The conflict dialog never opens from Save As.
  - **Workspace GET paths are decoded once.** Today `decode_path_param` percent-decodes query paths that axum already decoded (`crates/workshop/workspace/src/handlers.rs:108-118`, used at `:128` and `:144`), while the PUT body path is not decoded (`:165`). A file named `a%41.txt` can be written but not read. After: `decode_path_param` is removed, and every route sees a path decoded exactly once.
  - **Retired `open_browser`.** A `workshop.toml` that sets `server.open_browser` still loads, and the key is ignored.
  - **Linked folders in the tree.** Today a symlinked or junctioned folder inside a grant lists as a 0-byte file, because `directory_listing` reads the link's own metadata (`crates/workshop/workspace/src/workspace.rs:450-457`). After: an entry whose link target is a directory lists as a directory. A dangling link lists by its own metadata. Opening an entry still goes through confinement.
- States and validation:
  - **Menu memory is latest-wins.** Today each selection's write is spawned on the blocking pool and never awaited (`crates/workshop/menu/src/menu-memory.rs:65-73`), so two quick selections can finish out of order and the older one can win on disk. After: the newest selection's snapshot is what persists, whatever order the writes finish in.
  - **Grant during a workspace open (inferred).** `grant_and_persist` doesn't take the switch guard (`crates/workshop/workspace/src/workspace/backing.rs:52-76`), so a grant answered 200 can be wiped by `replace_all` (`:158`). After: a grant either lands in the workspace that is open when it completes or fails, never a 200 followed by silent loss.
  - **Tree state after a timed-out workspace switch (inferred).** On a 408, Open Workspace and Save Workspace As report the error and keep the old tree (`crates/workshop/ui/src/parts/workspace-files/workspace-files.contribution.ts:216-219`, `:274-277`), even though the server may finish the switch late. After: after a 408 the UI re-reads the server's current workspace (`GET /workspace/file/current`) and rebuilds from it.
- Errors and recovery:
  - **Save As timeout.** A 408 during `saveAs` shows the same timeout message `save()` shows and doesn't retarget. A retry then takes the 409 path above and converges. Overwrite's timeout handling is already correct: since `49d97995`, `save()` and `overwrite()` share the private `writeCurrent` helper in `editor-panel.ts`. Save As must not use `writeCurrent`, whose doc comment bars it, because a timeout there marks the open file's token unknown, not the target's.
  - **Conflict dialog wording.** When the unknown-token check finds that the disk doesn't match the last sent text (`editor-panel.ts:203-208`), the dialog says the file may hold an earlier timed-out save or an outside edit. It no longer blames only an outside edit.
  - **Quit ordering.** Today `quit_everything` posts the gateway's `/shutdown` and exits (`crates/workshop/desktop/src/quit.rs:36-45`), and the supervisor stops only later in the `RunEvent::Exit` handler (`crates/workshop/desktop/src/main.rs:152-158`). A supervisor probe in that gap relaunches a gateway the user asked to quit. After: `quit_everything` takes the supervisor out of its slot and shuts it down before posting `/shutdown`. The Exit handler then finds the slot empty, which `continue_teardown` already treats as done (`main.rs:180-192`).
  - **Bind failure leaves a sidecar file.** Today `serve_thread` reopens the last workspace before binding (`crates/workshop/server/src/serve.rs:262-264`), so a failed bind leaves the `.pfwork` file's `-wal` sidecar behind. The loopback refusal also only fires after full state composition. After: `spawn` rejects a non-loopback address before the server thread starts or any state is composed, and the listener binds before `reopen_last_workspace`. The readiness signal stays where it is.
- Security and privacy behavior:
  - **Revoked roots reach running agent sessions (inferred).** Today `bindings::forward` pushes roots to the harness only when the gateway binding, the chat catalog, or the menu changes (`crates/workshop/server/src/agents/bindings.rs:89-135`), so a revoke doesn't reach a running session. After: every grant, revoke, and workspace switch pushes fresh roots to the harness.
  - **Traversal stays refused.** With the second decode gone, a real `..` still fails the lexical check, and a literal `%2e%2e` segment is an ordinary filename that confinement resolves inside a grant or refuses. No request returns content outside a grant.
  - **The jail's Windows paths run in CI.** Today the workspace crate is tested only in the ubuntu job (`.github/workflows/ci.yml:96`). The windows job tests only `-p workshop -p workshop-server -p workshop-server-api` (`:189`), which doesn't run a dependency's own tests. So the Windows jail tests (`crates/workshop/workspace/src/workspace/tests/jail.rs:52-163`), the alternate-data-stream test (`crates/workshop/workspace/src/workspace/tests.rs:114`), and the Windows symlink branches run nowhere. After: they run in the windows job.
- Acceptance criteria:
  - Each bullet above has a test that fails before the fix and passes after, except the inferred items, which follow the reproduce-first constraint.
  - `crates/workshop/server/src/main.rs` and the `[[bin]]` table are gone, and no tracked file names the `workshop-server` binary.
  - The doc claims listed under Execution Instructions match the code.
  - `crates/workshop/AGENTS.md` exists, and the root Vocabulary section defines the added terms.

</product-contract>
<implementation-contract>

## Technical Design

No crate or dependency edge is added. The cross-module changes are one registry trait method, one config compatibility rule, one boot-order change, one lifecycle order change in the desktop app, and the removal of a binary target. Everything else is local to one module.

- Architecture:
  - The dependency graph and tiers are unchanged. The desktop app still reaches the server only through `workshop-server-api`.
- Modules and interfaces:
  - **`WorkspaceRoots` (`crates/workshop/registry/src/traits.rs:247-280`)** gains a change signal: a `subscribe` method returning a `tokio::sync::watch::Receiver<u64>` whose value is a grant-set generation. This is the same wake pattern `bindings::forward` already uses for the gateway binding and the chat catalog. `WorkspaceRootsAdapter` takes the receiver source beside its roots closure. The workspace bumps the generation on every grant, revoke, and workspace switch (`crates/workshop/workspace/src/handles.rs:43-47`). `bindings::forward` adds it as a fourth wake source.
  - **Menu memory.** `PendingWrite` (`crates/workshop/menu/src/menu-memory.rs:15-20`) gains a sequence number assigned under the menu state lock (`crates/workshop/menu/src/menu.rs:224-232`). `store_memory` serializes writes to the memory file and skips any write whose sequence is older than the last one written.
  - **Server boot order.** `spawn` validates that the bind address is loopback before composing state. `serve_thread` binds, then reopens the last workspace, then signals readiness (`crates/workshop/server/src/serve.rs:184-264`). `reuse_bind` keeps its own loopback refusal as a second line of defense.
  - **Desktop quit.** `quit_everything` stops the supervisor first, then requests the gateway's shutdown, then exits. Extract the ordering into a function the quit tests can drive without a Tauri runtime.
  - **Editor.** `saveAs` and `overwrite` in `crates/workshop/ui/src/parts/editor/editor-panel.ts` follow Functional Specification. `save()` is unchanged apart from the dialog wording.
- File and public API changes:
  - Delete `crates/workshop/server/src/main.rs` and the `[[bin]]` table at `crates/workshop/server/Cargo.toml:11-13`. That also removes the legacy `workbench.toml` fallback (`main.rs:25-34`). Remove the `open` dependency (`crates/workshop/server/Cargo.toml:23`) once nothing uses it.
  - Remove the `open_browser` field from the server config (`crates/workshop/support/src/config.rs:143,156`), plus every struct literal and forcing that sets it (`crates/workshop/desktop/src/config.rs:92,106`, and the tests that set `open_browser: false`). The `[server]` section (`ServerConfig`) doesn't deny unknown fields, so once the field is gone an existing key is silently ignored; no warning or special parse path is added.
  - `desktop/src/config.rs:192-221` becomes the test that an old config with the key still loads.
  - Remove "and the standalone `workshop-server` binary serves the UI for a browser" from `guide/src/gateway/11-serving-and-observing.md:29`, then regenerate the guide export with `cargo run -p build-user-guide`. Don't hand-edit generated files.
  - Remove `decode_path_param` (`crates/workshop/workspace/src/handlers.rs:108-118`) and its unit test (`crates/workshop/workspace/src/handlers/tests.rs:34`).
  - Add `cargo nextest run --locked -p workshop-workspace --all-features` to the `check-workshop` job on `windows-latest` (`.github/workflows/ci.yml:148-225`, beside the existing test command at `:189`).
  - Add `crates/workshop/AGENTS.md` (the crate map).
- Data, persistence, failure, security, and privacy constraints:
  - Menu memory stays a best-effort cache. A failed write is still logged and tolerated.
  - No persisted format changes. An existing `workshop.toml` that sets `server.open_browser` still loads.
  - Confinement, the lexical `..` rejection, the grant model, and the loopback-only bind are unchanged in strength.

</implementation-contract>
<verification-contract>

## Testing Plan

Each fix gets a focused test that fails before it and passes after. Timing-sensitive tests move to deterministic seams. A baseline of every exit command is recorded before any change, and the same commands close the plan. Building the `workshop` package needs the gateway sidecar staged: run `cargo build --locked -p gateway --no-default-features`, then `node tools/stage-gateway-sidecar.mjs stage --target x86_64-pc-windows-msvc --source target/debug/promptforge-gateway.exe`.

- Unit:
  - **Menu memory.** Two `PendingWrite`s run through `store_memory` newest-first leave the newest snapshot on disk.
  - **Double decode.** A file literally named `a%41.txt` in a granted root is written through PUT and read back through GET with the name URL-encoded once. The double-encoded traversal test (`crates/workshop/workspace/src/handlers/tests.rs:40-49`) now asserts that no content outside a grant is returned; the status code may change.
  - **Config.** A `workshop.toml` with `server.open_browser = true` still loads.
  - **Linked folders.** A Unix symlink to a directory and a Windows junction to a directory, both inside a grant, list as directories, and a dangling link lists as its own entry. Use the existing CI symlink guard in `crates/workshop/workspace/src/workspace/tests/jail.rs:17-20`.
  - **Test helper.** The UNC helper in `jail.rs:46-47` builds its path without assuming a drive-letter prefix.
  - **Quit.** The extracted quit ordering calls the supervisor stop before the shutdown request.
  - **Supervisor.** A supervisor that has been shut down launches nothing when its gateway later disappears (`crates/workshop/desktop/src/gateway/tests/shutdown.rs`).
- Integration and end-to-end:
  - **Revoke.** A revoke during a running agent session pushes roots without the revoked folder to the harness (under `crates/workshop/server/tests/it/`).
  - **Non-loopback bind.** `spawn` with a non-loopback address fails before any workspace file is opened or any temp sweep runs, and the existing reopen-before-readiness tests still pass (`crates/workshop/server/src/serve-tests.rs`).
  - **Grant race.** A grant racing a workspace open never answers 200 and then loses the grant. Reproduce first.
  - **Chat quiet check.** `assert_chat_quiet` (`crates/workshop/server/tests/it/chat_gate.rs:264-269`) stops using `Duration::ZERO` with a sync frame from a different socket. It syncs on a reply from the same `/agents/ws` socket, so socket ordering puts any premature frame first. If that socket has no request that gets an in-order reply, restore a bounded window and say why in a comment.
  - **Real-clock windows.** Replace these with event-driven waits or paused time wherever the code under test uses `tokio::time` or the desktop's injected clock: `crates/workshop/server/tests/it/heartbeat_loop.rs:171,317`, `realtime_relay.rs:262`, `realtime_relay/authentication.rs:61`, `agents/replacement.rs:255`, `crates/workshop/desktop/src/gateway/tests/shutdown.rs:29,54,63,71,88`, `tests/boot.rs:237`, `tests/recovery.rs:186,232,314,337,360`, and `crates/workshop/desktop/src/quit-tests.rs:13`. Where a real socket makes that impossible, keep the window and say why in a comment.
  - **UI.** Under `crates/workshop/ui/test/`:
    - Save As onto an existing file writes the target, retargets the panel, and leaves the old file untouched.
    - A Save As whose target read fails shows an error and doesn't retarget.
    - A Save As 408 shows the timeout message and doesn't retarget.
    - The reconcile-mismatch dialog uses the new wording.
    - The six existing cases in `crates/workshop/ui/test/editor-save-timeout.mjs`, including the two Overwrite timeout cases, still pass.
    - After a workspace-switch 408, the UI rebuilds from `GET /workspace/file/current`.
- Regression, security, and performance:
  - All existing traversal, confinement, CSP, cross-site, and bind-refusal tests pass unchanged, apart from the status code of the double-encoded traversal test noted above.
  - Run the new Windows command locally on Windows before relying on CI.
- Exit criteria:
  - **Commands.** Each of these is at least as green as its baseline:
    - `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`
    - `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`
    - `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`
    - `cargo nextest run --locked -p workshop-server --features headless`
    - `cargo nextest run --locked -p workshop-workspace --all-features` (on Windows)
    - `cargo test --doc -p workshop -p workshop-server -p workshop-server-api`
    - `cargo doc --locked --no-deps -p workshop-server --document-private-items` with `RUSTDOCFLAGS=-D warnings` (the CI step `Docs (workshop-server, private items)`, `.github/workflows/ci.yml:202-205`)
    - `cargo test -p build-xtask`
    - both clippy partitions with `-D warnings`
    - `cargo check -p gateway --no-default-features`
    - `cargo fmt --all --check`
    - the docs gate with `RUSTDOCFLAGS=-D warnings`, and the facade docs gate
    - `cargo +<pinned nightly> xtask api --check`
    - `mdbook build guide`
    - `cargo run -p build-user-guide`, changing only the regenerated gateway export
    - `npm run build`, `npm test`, and `npm run typecheck` in `crates/workshop/ui`
    - `cargo workshop`
  - **Retired strings.** Each of these finds nothing outside `vibe/`:
    - `rg -n "workshop-sessions" .cursor/rules`
    - `rg -n "session_agents|decode_path_param|workbench\.toml" crates/workshop`
    - `rg -n "supervisor\.rs" crates/workshop/desktop/AGENTS.md`
    - `rg -n "workshop-server\b" guide/src`
  - **Retired key.** `rg -n "open_browser" crates/workshop` finds only the two tests proving an old config with the key still loads.

</verification-contract>
<decision-record>

## Decision Record

- Decisions:
  - **Delete the standalone binary.** Nothing tests or ships it, and it prints nothing at startup. The library's `spawn(config)` is the part worth keeping, and it stays. The user chose "Delete it now (main.rs, [[bin]], workbench.toml fallback, open_browser, the open dependency, the guide sentence); design remote browser access later on the finished desktop app", after saying "it would probably be better to just finish the Tauri desktop application, polish it, and then port the server to work remotely".
  - **Two plans.** This one holds fixes and orientation, and a separate structure plan holds the moves. The user chose "Two plans: fixes first (bugs, CI gap, weakened tests, doc sweep, crate map), then a structure plan".
  - **Add a crate map without dependency lists.** Dependency lists are what drifted in the deleted READMEs, and each `Cargo.toml` and Invariants block already owns that information. The user selected "Add one short crate map (crates/workshop/AGENTS.md: 11 crates, tiers, one-line roles, the three runtime links, no dependency lists)".
  - **Drop `open_browser` with no warning or shim.** Only the top-level `Config` denies unknown fields. The `[server]` section doesn't, so an existing `workshop.toml` that sets the key keeps loading once the field is gone. The desktop app already forced the key off, so nothing a user sees changes, and a warning would add a parse path for no behavior.
  - **Save As trusts the OS replace confirmation.** It retries once with the target's token instead of opening a conflict dialog aimed at the old file. Pointing the dialog at the picked path would add panel state for a replace the user already confirmed.
  - **Remove the second decode.** After axum's single decode, a surviving `%2e%2e` is a literal filename, not a path step, so the second decode protects nothing. It also makes names containing `%XX` unreadable.
  - **Revoke propagation reuses the existing watch-generation pattern.** That is how `bindings::forward` already wakes on the gateway binding and the chat catalog, so no new mechanism is added.
  - **Menu memory uses a sequence number, not a writer task.** It's the smallest change that makes the newest selection win, and it keeps the cache best-effort.
  - **Define "entry bundle" once.** It is the eagerly loaded composition: `crates/workshop/ui/src/main.ts` and the `*.contribution.ts` modules it imports. Lazy panels never import a module inside it, directly or through another import. Everything else that eager and lazy code both import, such as `services/`, `base/`, and shared parts modules like `parts/layout/zones.ts`, is shared code. An earlier wording counted every module `main.ts` imports statically, and Step 15 found it false: lazy panels import `parts/workspace/workspace-drops.ts`, `parts/layout/zones.ts`, and `parts/layout/workshop-panel.ts`, all of which `main.ts` reaches eagerly. The user chose this definition: "The entry bundle is main.ts plus the *.contribution.ts modules it imports; lazy panels never import those. Everything else both sides use (services/, base/, and shared parts such as zones.ts) is shared code. Verify first, and stop again if a lazy panel imports a contribution module." Use this wording in root `AGENTS.md:33,74`, `.cursor/rules/workshop-spa.mdc:14`, `crates/workshop/ui/AGENTS.md:17`, and `crates/workshop/ui/build.mjs:51-52`.
  - **Reword the `index.ts` rule to match the code.** A part that the panel registry loads lazily has an `index.ts` as its chunk entry (`crates/workshop/ui/src/services/panel-registry.ts:194-222`), and other parts need none. This replaces "every directory has an `index.ts`" (`.cursor/rules/workshop-spa.mdc:13`) and the `index.ts` clause of root `AGENTS.md:80`.
  - **The Windows CI line is a separate command with `--all-features`.** The existing Windows command for the three workshop packages never takes that flag.
  - **Keep the heartbeat convergence test's real-time quiet window.** `63ddccb5` replaced its paused-clock window, which could not drive real loopback probes and so proved nothing, with a real sleep of four test intervals. It now fails when retries continue, so this plan leaves it alone and doesn't add a counter seam.
- Rejected alternatives:
  - **Keep the binary as an SSH-tunnel stopgap.** Rejected by the user. Revisit when remote access is designed.
  - **One plan with fixes first.** Rejected by the user.
  - **A single writer task for menu memory.** It needs more machinery than a sequence check. Revisit if menu memory ever needs ordered writes of more than one file.
  - **A refresh-attempt counter for the heartbeat convergence test.** It adds a production test seam to replace a window that already works. Revisit if the four-interval window proves flaky.
- Assumptions, risks, and notes:
  - GitHub-hosted Windows runners can create symlinks. If they can't, the CI symlink guard turns the new job red by design. Stop and report instead of weakening the guard.
  - If a lazy panel turns out to import a module inside the entry bundle as defined above, stop and report instead of choosing another definition.
  - The grant race, the revoke propagation, and the workspace-switch resync are inferred from reading the code, not reproduced.
  - The quit race's timing is inferred, but its ordering is verified wrong, so the ordering fix and its test are unconditional.
  - Line numbers are at `ce10a8eb`. The files that changed since are listed under Constraints.
  - The server's `/ws` docs in `crates/workshop/server/src/lib.rs` and `crates/workshop/server/src/agents.rs:1-5` were corrected by `eca41e57`, which also added the private-items docs CI step. Any server doc edit here must keep that step green.

### Deferred and Out of Scope

- Deferred: module moves, file-layout rule application, public-surface trimming, the `/ws` module split, the gateway cache API deletion, moving the agent frames out of `workshop-protocol`, the UI parts restructure, the desktop dependency build check, and the remaining test-coverage gaps. Revisit after this plan's exit gates pass, in the structure plan.
- Deferred: the check-then-use window between confinement and file access (acknowledged at `crates/workshop/workspace/src/handlers.rs:121-123`). Revisit when remote access is designed, since today exploiting it needs local write access inside a granted root.
- Out of scope: remote or browser access to the server.
- Out of scope: renaming the desktop package `workshop`.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build` builds only the gateway (the sole default member, `crates/gateway/app`); `cargo workshop` (alias for `run -p build-workshop --`) is the one-command Workshop build that builds the gateway, stages the Tauri sidecar, builds the desktop app, and removes the staged copy. `cargo build -p workshop` is a low-level build that needs a pre-staged sidecar at `crates/workshop/desktop/binaries/promptforge-gateway-<triple>`. The UI bundles build through crate build scripts (`build-ui`), so run `npm ci --prefix crates/workshop/ui` and `npm ci --prefix crates/gateway/config-ui/ui` first. Headless shape: `cargo check -p gateway --no-default-features`.
- Focused test command pattern: non-workshop crates `cargo nextest run --locked -p <crate> --all-features <test-name-filter>` (integration binary: add `--test it`); workshop trio `cargo nextest run --locked -p <workshop|workshop-server|workshop-server-api> <test-name-filter>` without `--all-features`; doctests `cargo test -p <crate> --doc`; Workshop UI `node --test test/<feature>.mjs` from `crates/workshop/ui`; gateway config UI `node --test src/<path>.test.mjs` from `crates/gateway/config-ui/ui`.
- Component test command pattern: `cargo nextest run --locked -p <crate> --all-features` for non-workshop crates (including other workshop-* crates such as `workshop-workspace`); `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api` for the workshop trio, plus `cargo nextest run --locked -p workshop-server --features headless`; UI packages `npm test` (and `npm run typecheck`) in `crates/workshop/ui` or `crates/gateway/config-ui/ui`.
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`, `cargo nextest run --locked -p workshop-server --features headless`, and `cargo test --doc -p workshop -p workshop-server -p workshop-server-api`; UI suites `npm test` in both UI packages. Structural harness `cargo test -p build-xtask` is covered by the workspace run; its nightly-only fixtures run as `cargo nextest run --locked -p build-xtask --run-ignored only` on the pinned nightly.
- Linter command: `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings` and `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`; TypeScript `npm run typecheck` (`tsc --noEmit`) in each UI package; supply chain `cargo deny check`; facade surface `cargo +<pinned nightly> xtask api --check` (nightly named in `crates/build-xtask/src/api/toolchain.rs`). Never run a standalone `cargo check --workspace` beside clippy.
- Formatter check command: `cargo fmt --all --check` (`rustfmt.toml` sets `style_edition = "2024"`). No formatter is configured for the TypeScript packages.
- Docs command: `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`; facade `RUSTDOCFLAGS="-D warnings" cargo doc -p promptforge --no-deps` (default features); workshop-server `cargo doc --locked --no-deps -p workshop-server --document-private-items`; user guide `mdbook build guide`.
- Test placement and naming conventions: unit tests sit in `#[cfg(test)]` modules, either inline or in sibling `<stem>-tests.rs` files wired with `#[path = "<stem>-tests.rs"] mod tests;` (split further as `<stem>-tests-<label>.rs`); a group of three or more moves into a `src/.../tests/` subdirectory (for example `engine/src/execute/tests/`). Each crate has one integration binary at `tests/it/main.rs` with topic modules `tests/it/<topic>.rs` and `<topic>/` subdirectories (the facade uses `tests/suite/main.rs` plus `tests/prompts/` fixtures; `workshop-server` adds `tests/common/`). Test functions are descriptive snake_case sentences such as `a_direct_launch_recovers_the_lease_from_a_terminated_owner`. The Workshop UI keeps Node `node:test` files in `crates/workshop/ui/test/<feature>.mjs`; the gateway config UI and `tools/` keep `<name>.test.mjs` beside the source. Nextest caps the STT crates in a `heavy` test group. Behavior changes ship with tests in the same change.
- Directory map:
  - `crates/` root: the public layer (`promptforge` facade, `harness-api`, `gateway-api-types`, `gateway-api-discovery`, `shared-error-source`, `shared-loopback`), build tooling (`build-xtask`, `build-workshop`, `build-ui`, `build-user-guide`, `build-llama-cuda`), `workspace-hack` (cargo-hakari), and `shared-ui` (a TypeScript and CSS package, not a Rust crate).
  - `crates/promptforge-internal/`: private engine family - `engine`, `types`, `vfs`, `lua`, `parser`, `store`, `model-client`.
  - `crates/gateway/`: private gateway family - `app` (package `gateway`), `cloud-providers`, `config`, `config-ui` (with its `ui/` TypeScript package), `local`, `logging`, `progress`, `protocol`, `routing`, `web-search`, and `stt/` (`api`, `engine`, `backend-whisper`, `whisper-ffi`).
  - `crates/workshop/`: private Workshop family - `desktop` (Tauri package `workshop`), `server`, `server-api`, `gateway`, `menu`, `protocol`, `registry`, `status`, `support`, `user-state`, `workspace`, and `ui/` (the TypeScript SPA with `src/base`, `src/services`, `src/parts`, and `test/`).
  - `crates/harness/`: private harness family - `runner`, `models`, `capabilities`, `log`, `sessions`, `web`, `webfetch`, `web-search`.
  - `guide/`: mdBook user guide (`book.toml`, `src/`) plus the language, agent, and gateway guides and `CONTRIBUTING.md`.
  - `prompts/`: example prompt pipelines. `tools/`: Node scripts for sidecar staging and a live TTS check, with tests.
  - `.github/workflows/`: `ci.yml` (fmt, clippy, test, docs, Windows and Linux workshop checks, UI, supply chain, api-surface, `ci-green` gate) plus guide, release, nightly, and specialty workflows. `.githooks/`: pre-commit fmt, pre-push headless check, clippy, and cargo deny.
  - `.cargo/config.toml` (rust-lld and static CRT on Windows, `xtask` and `workshop` aliases), `.config/` (nextest and hakari), root `Cargo.toml`, `deny.toml`, `clippy.toml`, `rustfmt.toml`, `rust-toolchain.toml` (stable), `dist-workspace.toml`, `AGENTS.md`.
  - `vibe/`: plans, `archdoc.md`, and run scratch. `local/`: developer-local gateway and profile config and fixtures. `images/`: README art. `target/`, `target-msrv/`: build output.
- Component boundaries:
  - executor (`promptforge-engine` behind the `promptforge` facade): sans-I/O state machine; depends on store, the Lua VM boundary, and shared substrate. Outside crates depend only on `promptforge`.
  - harness (behind `harness-api`): depends on `promptforge`, `gateway-api-types`, `gateway-api-discovery`, and shared-*; never on workshop or private gateway crates. Workshop reaches harness only through `harness-api`.
  - gateway (behind `gateway-api-types` and `gateway-api-discovery`): depends on shared substrate only; never on promptforge, workshop, or harness crates.
  - Workshop: depends on `harness-api`, `promptforge`, the gateway public pair, and shared-*; the desktop app depends on `workshop-server-api`, never `workshop-server`. Internal tiers flow server, then features, then services, then vocabulary. The SPA flows `parts` to `services` to `base`, and lazy panels never import the entry bundle.
  - store depends on the VFS layer; the VFS layer and shared-* depend on no product crate.
  - Family containers are private: a container crate may depend only on `crates/` root crates and its own siblings; build-* crates are exempt. `cargo test -p build-xtask` enforces the matrix, and `cargo xtask api --check` enforces the facade surface against `crates/promptforge/public-api.txt`.
- Conventions summary: Rust 2024 edition on stable with resolver 3; every dependency is declared in `[workspace.dependencies]` with comments justifying version pins, and members inherit `workspace-hack`. Workspace lints forbid `unsafe_code` outside owned boundaries, deny clippy `all` and `pedantic` plus `unwrap_used` and `expect_used`, and warn on `missing_docs` and `unreachable_pub`. Errors use `thiserror` with third-party causes wrapped through `shared-error-source`, and messages are written for model consumption. Source directories are flat, with one- or two-file groups as kebab siblings (`foo-bar.rs` plus `#[path]`) and three or more as a subdirectory. workshop-* and harness-* `lib.rs` files open with a `//! ## Invariants` marker, and marker crates keep files at or under 500 lines. Comments explain only non-obvious constraints, and workarounds cite upstream issue URLs. Cargo features gate real constraints only (`headless`, `test-fixtures`). Run-log JSON round-trips exactly (`float_roundtrip`, sorted keys, never `preserve_order`). CI commands pass `--locked`. The UI is TypeScript bundled with esbuild (`build.mjs`), keeps CSS beside its TypeScript, uses `--ws-*` tokens, never uses `localStorage`, and follows VS Code command, menu, and keybinding mechanics.

</project-survey>
<execution-plan>

## Execution Instructions

<step-1>

### Step 1: Record the exit-command baseline [completed]

- Component: Baseline

- Component order: first, because every exit command must end at least as green as a baseline recorded before any code change.
- Piece: gate baseline, a single step with no construction choice.
- Artifacts: a baseline section in this plan's repository copy under `vibe/`.
- Work:
  - Stage the gateway sidecar that building the `workshop` package needs: `cargo build --locked -p gateway --no-default-features`, then `node tools/stage-gateway-sidecar.mjs stage --target x86_64-pc-windows-msvc --source target/debug/promptforge-gateway.exe`. Run `npm ci --prefix crates/workshop/ui` and `npm ci --prefix crates/gateway/config-ui/ui`.
  - Run every command under Testing Plan > Exit criteria > Commands, the four retired-string checks, and the `open_browser` check. Record pass or fail for each, with the failing test names where a command fails. The retired-string checks are expected to find hits today.
  - Record the branch head, which must be `master` at or after `890ae2f6`.
- Baseline results (recorded 2026-09-25; every command passes; no failing, flaky, leaky, or slow tests):
  - Branch head: `master` at `890ae2f6` ("Close plan: vibe2 debt removal") plus the commit that adds this plan, which changes only files under `vibe/`; worktree clean.
  - Setup: `npm ci` in both UI packages, `cargo build --locked -p gateway --no-default-features`, then `node tools/stage-gateway-sidecar.mjs stage --target x86_64-pc-windows-msvc --source target/debug/promptforge-gateway.exe`: pass (sidecar left staged). `cargo workshop` ran first because it removes the staged copy when it finishes.
  - Nextest and cargo test runs added `--no-fail-fast` so a failure would list every failing test; pass or fail is unchanged by the flag.
  - `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`: pass (3926 passed, 54 skipped)
  - `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`: pass
  - `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`: pass (247 passed, 4 skipped)
  - `cargo nextest run --locked -p workshop-server --features headless`: pass (143 passed, 2 skipped)
  - `cargo nextest run --locked -p workshop-workspace --all-features` (Windows): pass (142 passed, 0 skipped)
  - `cargo test --doc -p workshop -p workshop-server -p workshop-server-api`: pass
  - `cargo doc --locked --no-deps -p workshop-server --document-private-items` with `RUSTDOCFLAGS=-D warnings`: pass
  - `cargo test -p build-xtask`: pass (169 passed, 18 ignored)
  - `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings`: pass
  - `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`: pass
  - `cargo check -p gateway --no-default-features`: pass
  - `cargo fmt --all --check`: pass
  - `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api` with `RUSTDOCFLAGS=-D warnings`: pass
  - `cargo doc -p promptforge --no-deps` with `RUSTDOCFLAGS=-D warnings`: pass
  - `cargo +nightly-2026-09-05 xtask api --check`: pass (0 violations; the listing matches public-api.txt)
  - `mdbook build guide`: pass
  - `cargo run -p build-user-guide`: pass (no tracked file under `guide/` changed)
  - workshop UI `npm run build`, `npm test` (139 passed), `npm run typecheck`: pass
  - `cargo workshop`: pass
  - Retired strings (hits expected today):
    - `rg -n "workshop-sessions" .cursor/rules`: 1 hit, `.cursor/rules/workshop-architecture.mdc:12`
    - `rg -n "session_agents|decode_path_param|workbench\.toml" crates/workshop`: 8 hits - `protocol/tests/it/fixture.rs:188` (`session_agents`), `server/src/main.rs:25` (`workbench.toml`), `workspace/src/handlers.rs:114,128,144` and `workspace/src/handlers/tests.rs:34,35,37` (`decode_path_param`)
    - `rg -n "supervisor\.rs" crates/workshop/desktop/AGENTS.md`: 1 hit, line 11
    - `rg -n "workshop-server\b" guide/src`: 1 hit, `guide/src/gateway/11-serving-and-observing.md:29`
  - Retired key: `rg -n "open_browser" crates/workshop`: 20 hits in 10 files - `support/src/config.rs:143,156`; `support/tests/it/config.rs:114,121,129,132`; `desktop/src/config.rs:7,92,106,192,202,221`; `server/src/main.rs:36,38`; and the `open_browser: false` literals in `desktop/src/quit-tests.rs:44`, `desktop/src/gateway/tests/shutdown.rs:104`, `recovery.rs:61`, `boot.rs:101`, `server/tests/common/mod.rs:57`, and `server/src/serve-tests.rs:16`.
- Tests: none added. The recorded results are what the Exit step compares against.
- Commit: the baseline record in the plan's repository copy.

</step-1>

<step-2>

### Step 2: Fix Save As onto an existing file and the reconcile wording [completed]

- Component: Editor fixes

- Component order: second, because it touches only the TypeScript SPA, needs no Rust change, and no later component depends on it.
- Piece: editor panel, built before the workspace-switch piece. Sequential only to keep each commit small; the two pieces share no code.
- Artifacts: `saveAs` and the reconcile-mismatch dialog text in `crates/workshop/ui/src/parts/editor/editor-panel.ts`; new `crates/workshop/ui/test/editor-save-as.mjs`; `crates/workshop/ui/test/editor-save-timeout.mjs`.
- Work:
  - The OS save dialog's replace confirmation counts as consent. On a 409 from the null-token write, `saveAs` reads the target's current token, retries the write once with it, then retargets the panel.
  - When the target can't be read (not UTF-8 text, too large, not a file) or the retry conflicts again, show an error naming the target and don't retarget. The conflict dialog never opens from Save As.
  - A 408 during `saveAs` shows the same timeout message `save()` shows and doesn't retarget. A later retry takes the 409 path and converges.
  - `saveAs` must not call the private `writeCurrent` helper, whose doc comment bars it, because a timeout there marks the open file's token unknown instead of the target's. `save()` and `overwrite()` keep sharing `writeCurrent` as they have since `49d97995`; Overwrite's timeout handling is already correct.
  - When the unknown-token check finds that the disk doesn't match the last sent text, the dialog says the file may hold an earlier timed-out save or an outside edit, instead of blaming only an outside edit. `save()` is otherwise unchanged.
- Tests (from `crates/workshop/ui`: `node --test test/editor-save-as.mjs test/editor-save-timeout.mjs`, then `npm test` and `npm run typecheck`):
  - Save As onto an existing file writes the target, retargets the panel, and leaves the old file untouched.
  - A Save As whose target read fails shows an error and doesn't retarget.
  - A Save As 408 shows the timeout message and doesn't retarget.
  - The reconcile-mismatch dialog uses the new wording, tested in `editor-save-timeout.mjs` beside its unknown-token cases.
  - The six existing cases in `editor-save-timeout.mjs`, including the two Overwrite timeout cases, still pass.
- Commit: the `editor-panel.ts` change and its UI tests.

</step-2>

<step-3>

### Step 3: Resync the tree after a workspace-switch timeout [completed]

- Component: Editor fixes

- Piece: workspace-files contribution, after the editor panel. Sequential and kept separate so the reproduce-first rule applies to this inferred item alone.
- Artifacts: the 408 branches of Open Workspace and Save Workspace As in `crates/workshop/ui/src/parts/workspace-files/workspace-files.contribution.ts`; `crates/workshop/ui/test/workspace-switch.mjs`.
- Work:
  - This bug is inferred. First write the UI test showing that after a 408 the tree keeps the old workspace while the server finished the switch late. If no deterministic reproduction exists, record that in this plan's repository copy and skip the code change.
  - After a 408 from either gesture, report the error as today, then re-read `GET /workspace/file/current` and rebuild the tree from it.
- Tests (from `crates/workshop/ui`: `node --test test/workspace-switch.mjs`, then `npm test` and `npm run typecheck`): after a workspace-switch 408, the UI rebuilds from `GET /workspace/file/current`.
- Commit: the contribution change and its test, or the recorded non-reproduction alone.

</step-3>

<step-4>

### Step 4: Decode workspace GET paths once [completed]

- Component: Workspace confinement

- Component order: third, after the UI fixes and before state propagation, because the grant-race fix settles the shape of `Workspace::grant_and_persist` that the roots-signal step later bumps a generation inside.
- Piece: route handlers, first in the component. Sequential; it is the smallest piece and independent of the listing and grant pieces.
- Artifacts: `decode_path_param` and its two GET callers in `crates/workshop/workspace/src/handlers.rs`; `crates/workshop/workspace/src/handlers/tests.rs`.
- Work:
  - Remove `decode_path_param` and use the path axum already decoded, so every route sees a path decoded exactly once, matching the undecoded PUT body path.
  - Remove the `decode_path_param` unit test.
  - After axum's single decode, a surviving `%2e%2e` is a literal filename that confinement resolves inside a grant or refuses, and a real `..` still fails the lexical check.
- Tests (`cargo nextest run --locked -p workshop-workspace --all-features`):
  - A file literally named `a%41.txt` in a granted root is written through PUT and read back through GET with the name URL-encoded once.
  - The double-encoded traversal test now asserts that no content outside a grant is returned. Its status code may change.
  - Every other traversal and confinement test passes unchanged.
- Commit: the handler change and its tests.

</step-4>

<step-5>

### Step 5: Run the jail's Windows tests in CI [completed]

- Component: Workspace confinement

- Piece: Windows coverage, after the decode fix. Sequential, because the new job must run against a UNC helper that no longer assumes a drive letter.
- Artifacts: the UNC path helper in `crates/workshop/workspace/src/workspace/tests/jail.rs` (cited at `:46-47`); the `check-workshop` job on `windows-latest` in `.github/workflows/ci.yml`.
- Work:
  - Fix the UNC helper so it builds its path without assuming a drive-letter prefix.
  - Add a separate step running `cargo nextest run --locked -p workshop-workspace --all-features` beside the existing `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api` step. It stays a separate command because the trio command never takes `--all-features`.
  - Run the new command locally on Windows before relying on CI.
  - If GitHub-hosted Windows runners can't create symlinks, the CI symlink guard near the top of `jail.rs` turns the job red by design. Stop and report instead of weakening the guard.
- Tests: `cargo nextest run --locked -p workshop-workspace --all-features` on Windows runs the Windows jail tests in `jail.rs`, the alternate-data-stream test in `crates/workshop/workspace/src/workspace/tests.rs`, and the Windows symlink branches.
- Commit: the helper fix and the CI step.

</step-5>

<step-6>

### Step 6: List linked folders as directories [completed]

- Component: Workspace confinement

- Piece: tree listing, after Windows coverage. Sequential, so its Windows junction test runs in the job the previous step added.
- Artifacts: `Workspace::directory_listing` in `crates/workshop/workspace/src/workspace.rs` (491 physical lines); new `crates/workshop/workspace/src/workspace/tree.rs` if the split is needed; linked-folder tests in `crates/workshop/workspace/src/workspace/tests/jail.rs`, or a sibling module under `workspace/tests/` that reuses its symlink guard if `jail.rs` would pass 500 lines.
- Work:
  - If the edit would take `workspace.rs` past 500 physical lines, first move tree listing into `workspace/tree.rs` within this step.
  - An entry whose link target is a directory lists as a directory. A dangling link lists by its own metadata. Opening an entry still goes through confinement.
- Tests (`cargo nextest run --locked -p workshop-workspace --all-features` on Windows and Linux):
  - A Unix symlink to a directory and a Windows junction to a directory, both inside a grant, list as directories.
  - A dangling link lists as its own entry.
  - Both use the existing CI symlink guard in `jail.rs`.
- Commit: the listing change, the split if made, and the tests.

</step-6>

<step-7>

### Step 7: Close the grant-during-open race [completed]

- Component: Workspace confinement

- Piece: grant persistence, last in the component. Sequential, because the roots-signal step later edits the same function.
- Artifacts: `Workspace::grant_and_persist`, the `switches` guard, and `replace_all` in `crates/workshop/workspace/src/workspace/backing.rs` (469 physical lines); the `hold_switches_for_test` seam; race tests in `crates/workshop/workspace/src/workspace/tests_switch.rs`.
- Work:
  - This bug is inferred. First write a test where a grant races a workspace open, answers 200, and is then wiped by `replace_all`. If no deterministic reproduction exists, record that in this plan's repository copy and skip the code change.
  - If it reproduces, take the switch guard in `grant_and_persist`, so a grant either lands in the workspace that is open when it completes or fails.
  - Keep `backing.rs` at or under 500 lines; split along an existing seam first if needed.
- Tests (`cargo nextest run --locked -p workshop-workspace --all-features`): a grant racing a workspace open never answers 200 and then loses the grant.
- Commit: the guard and its test, or the recorded non-reproduction alone.

</step-7>

<step-8>

### Step 8: Delete the standalone server binary and drop `open_browser` [completed]

- Component: Binary removal

- Component order: fourth, before lifecycle ordering, because the quit and boot-order tests build `Config` literals that this step strips of `open_browser`.
- Piece: binary and config, one joint change. The binary is the only reader of `open_browser`, so deleting the binary and dropping the field are one compatibility decision.
- Artifacts:
  - Delete `crates/workshop/server/src/main.rs`, which also removes the legacy `workbench.toml` fallback, plus the `[[bin]]` table and the `open` dependency in `crates/workshop/server/Cargo.toml`.
  - `ServerConfig`, its `Default`, and `Config::parse` in `crates/workshop/support/src/config.rs`; `open_browser_defaults_to_false_and_parses_when_set` in `crates/workshop/support/tests/it/config.rs`.
  - `shape_for_desktop`, `default_config`, the module doc, and the `open_browser` test (cited at `:192-221`) in `crates/workshop/desktop/src/config.rs`.
  - Every other literal that sets the field: `crates/workshop/server/tests/common/mod.rs`, `crates/workshop/server/src/serve-tests.rs`, `crates/workshop/desktop/src/quit-tests.rs`, `crates/workshop/desktop/src/gateway/tests/boot.rs`, `gateway/tests/recovery.rs`, and `gateway/tests/shutdown.rs`.
  - Binary wording: `crates/workshop/desktop/src/main.rs:13-14`, the "or the server binary itself" clause at `crates/workshop/server/src/serve.rs:5`, and the `src/main.rs` clause at `crates/workshop/server/src/lib.rs:4`.
  - `guide/src/gateway/11-serving-and-observing.md:29` and the gateway export that `cargo run -p build-user-guide` regenerates from it.
- Work:
  - Remove the `open_browser` field. Only the top-level `Config` denies unknown fields; `ServerConfig` doesn't, so serde ignores `server.open_browser` once the field is gone. Add no warning and no special parse path. An existing `workshop.toml` that sets the key still loads, and no persisted format changes.
  - Remove "and the standalone `workshop-server` binary serves the UI for a browser" from the guide sentence, then run `cargo run -p build-user-guide`. Don't hand-edit generated files.
  - Removing `open` rewrites the `workshop-server` entry in `Cargo.lock`, which is in the edit scope. The gateway app still uses `open`, so the root `[workspace.dependencies]` entry and `crates/workspace-hack` should not change; if either would, stop and report.
- Tests:
  - A `workshop.toml` with `server.open_browser = true` still loads, in the rewritten desktop config test.
  - The support-crate test becomes a test that the key still parses and is ignored. These two are the only `open_browser` mentions the exit check allows.
  - `cargo nextest run --locked -p workshop-support --all-features`, `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api` with the staged sidecar, `cargo nextest run --locked -p workshop-server --features headless`, the workshop clippy partition, and `mdbook build guide`.
  - `rg -n "workbench\.toml" crates/workshop` and `rg -n "workshop-server\b" guide/src` find nothing, and `rg -n "open_browser" crates/workshop` finds only the two old-config tests.
- Commit: the binary deletion, the field removal, the literal updates, `Cargo.lock`, the guide clause, and the regenerated export together.

</step-8>

<step-9>

### Step 9: Stop the supervisor before quitting the gateway [completed]

- Component: Lifecycle ordering

- Component order: fifth, after binary removal so its tests use the final `Config`, and before test determinism, which rewrites clock waits in the same quit and supervisor test files.
- Piece: desktop quit, before server boot. Sequential only to keep each commit small; the two pieces share no code.
- Artifacts: `quit_everything` and `request_gateway_shutdown` in `crates/workshop/desktop/src/quit.rs`; `GatewaySupervisorSlot`, the `RunEvent::Exit` handler, and `continue_teardown` in `crates/workshop/desktop/src/main.rs`; `GatewaySupervisor::shutdown` in `crates/workshop/desktop/src/gateway/supervisor/lifecycle.rs`; `crates/workshop/desktop/src/quit-tests.rs`; `crates/workshop/desktop/src/gateway/tests/shutdown.rs`.
- Work:
  - Extract the quit ordering into a function the tests can drive without a Tauri runtime, taking the supervisor stop and the shutdown request as inputs.
  - `quit_everything` takes the supervisor out of `GatewaySupervisorSlot` and shuts it down, then requests the gateway's `/shutdown`, then exits. The Exit handler then finds the slot empty, which `continue_teardown` already treats as done.
  - The race's timing is inferred but its ordering is verified wrong, so this fix and its tests are unconditional.
  - Update the `quit.rs` module and function docs to the new order.
- Tests (`cargo nextest run --locked -p workshop` with the staged sidecar):
  - The extracted ordering calls the supervisor stop before the shutdown request.
  - A supervisor that has been shut down launches nothing when its gateway later disappears, in `gateway/tests/shutdown.rs`.
- Commit: the quit ordering and its tests.

</step-9>

<step-10>

### Step 10: Validate loopback first and bind before reopening the workspace [completed]

- Component: Lifecycle ordering

- Piece: server boot, after desktop quit. Sequential and independent.
- Artifacts: `spawn`, `spawn_inner`, `serve_thread`, and `reuse_bind` in `crates/workshop/server/src/serve.rs`; `AppState::reopen_last_workspace` in `crates/workshop/server/src/app.rs`; `crates/workshop/server/src/serve-tests.rs`.
- Work:
  - `spawn` rejects a non-loopback bind address before the server thread starts or any state is composed.
  - `serve_thread` binds the listener, then calls `reopen_last_workspace`, then signals readiness. The readiness signal stays where it is. A failed bind no longer leaves the `.pfwork` file's `-wal` sidecar behind.
  - `reuse_bind` keeps its own loopback refusal as a second line of defense. The loopback-only bind is unchanged in strength.
- Tests (`cargo nextest run --locked -p workshop-server` and `cargo nextest run --locked -p workshop-server --features headless`):
  - `spawn` with a non-loopback address fails before any workspace file is opened or any temp sweep runs.
  - The existing reopen-before-readiness and bind-refusal tests still pass.
- Commit: the boot-order change and its tests.

</step-10>

<step-11>

### Step 11: Make menu memory writes latest-wins [completed]

- Component: State propagation

- Component order: sixth, after the workspace fixes so the roots signal bumps inside the final `grant_and_persist`, and before the docs, which describe the new wake source.
- Piece: menu memory, before the roots signal. Sequential; it is independent and smaller, and landing it first leaves the registry trait change in a commit of its own.
- Artifacts: `PendingWrite` and `store_memory` in `crates/workshop/menu/src/menu-memory.rs`; the selection path under the menu state lock in `crates/workshop/menu/src/menu.rs` (cited at `:224-232`); `crates/workshop/menu/src/menu-tests-memory.rs`.
- Work:
  - `PendingWrite` gains a sequence number assigned under the menu state lock.
  - `store_memory` serializes writes to the memory file and skips any write whose sequence is older than the last one written.
  - Menu memory stays a best-effort cache. A failed write is still logged and tolerated.
- Tests (`cargo nextest run --locked -p workshop-menu --all-features`): two `PendingWrite`s run through `store_memory` newest-first leave the newest snapshot on disk.
- Commit: the sequence check and its test.

</step-11>

<step-12>

### Step 12: Push fresh roots on every grant, revoke, and switch [completed]

- Component: State propagation

- Piece: roots signal, one joint change, because the registry trait, the workspace adapter, and the server wake loop don't compile apart.
- Artifacts: `WorkspaceRoots` and `WorkspaceRootsAdapter` in `crates/workshop/registry/src/traits.rs`; `register` in `crates/workshop/workspace/src/handles.rs`; `Workspace::grant` and `Workspace::revoke` in `crates/workshop/workspace/src/workspace.rs` and the switch paths in `crates/workshop/workspace/src/workspace/backing.rs`; `forward` in `crates/workshop/server/src/agents/bindings.rs`; `crates/workshop/registry/tests/it/main.rs`; `crates/workshop/server/src/agents/bindings-tests.rs`; a new revoke test module under `crates/workshop/server/tests/it/agents/`.
- Work:
  - This bug is inferred. First write the integration test showing that a revoke during a running agent session doesn't reach the harness. If no deterministic reproduction exists, record that in this plan's repository copy and skip the code change.
  - `WorkspaceRoots` gains `subscribe`, returning a `tokio::sync::watch::Receiver<u64>` whose value is a grant-set generation. `WorkspaceRootsAdapter::new` takes the receiver source beside its roots closure.
  - `Workspace` owns the generation sender and bumps it on every grant, revoke, and workspace switch. `register` passes the receiver source to the adapter.
  - `bindings::forward` adds the roots receiver as a fourth wake source beside the gateway binding, chat catalog, and menu watches, and its module doc lists all four.
  - Keep `workspace.rs` and `backing.rs` at or under 500 physical lines. If `workspace.rs` would pass and Step 6 didn't split it, move tree listing into `workspace/tree.rs` first.
- Tests:
  - A revoke during a running agent session pushes roots without the revoked folder to the harness.
  - `cargo nextest run --locked -p workshop-registry --all-features`, `cargo nextest run --locked -p workshop-workspace --all-features`, `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`, and `cargo test -p build-xtask`, which must stay green.
- Commit: the trait change, every call site, and the tests.

</step-12>

<step-13>

### Step 13: Make the server integration tests deterministic [completed]

- Component: Test determinism

- Component order: seventh, after the lifecycle and state fixes that edit the same test files, and before the docs.
- Piece: server tests, before desktop tests. Sequential; separate crates with separate test commands.
- Artifacts: `assert_chat_quiet` in `crates/workshop/server/tests/it/chat_gate.rs` (cited at `:264-269`); the real-clock windows in `crates/workshop/server/tests/it/heartbeat_loop.rs` (`:171`, `:317`; 487 physical lines), `realtime_relay.rs:262`, `realtime_relay/authentication.rs:61`, and `agents/replacement.rs:255`.
- Work:
  - `assert_chat_quiet` stops using `Duration::ZERO` with a sync frame from a different socket. It syncs on a reply from the same `/agents/ws` socket, so socket ordering puts any premature frame first. If that socket has no request that gets an in-order reply, restore a bounded window and say why in a comment.
  - Replace each listed window with an event-driven wait or paused time wherever the code under test uses `tokio::time`. Where a real socket makes that impossible, keep the window and say why in a comment.
  - Leave the heartbeat convergence test in `heartbeat_loop/startup_convergence.rs` and its four-interval real-time window alone.
  - Keep `heartbeat_loop.rs` at or under 500 physical lines; split along an existing seam first if needed.
- Tests: `cargo nextest run --locked -p workshop-server` and `cargo nextest run --locked -p workshop-server --features headless`, each run several times to confirm stability.
- Commit: the test changes.

</step-13>

<step-14>

### Step 14: Make the desktop supervisor and quit tests deterministic [completed]

- Component: Test determinism

- Piece: desktop tests, after the server piece. Sequential.
- Artifacts: the real-clock windows in `crates/workshop/desktop/src/gateway/tests/shutdown.rs` (`:29`, `:54`, `:63`, `:71`, `:88`), `gateway/tests/boot.rs:237`, `gateway/tests/recovery.rs` (`:186`, `:232`, `:314`, `:337`, `:360`), and `crates/workshop/desktop/src/quit-tests.rs:13`.
- Work: replace each listed window with an event-driven wait, paused time, or the desktop's injected clock. Where a real socket makes that impossible, keep the window and say why in a comment. The desktop crate is exempt from the 500-line limit.
- Tests: `cargo nextest run --locked -p workshop` with the staged sidecar, run several times to confirm stability.
- Commit: the test changes.

</step-14>

<step-15>

### Step 15: Rewrite the agent rules and markdown docs [completed]

- Component: Documentation

- Component order: eighth, after every code fix, so the docs describe the code as it ends up.
- Piece: agent-facing markdown, first in the component. Sequential, because it fixes the entry-bundle and `index.ts` wording that the UI comment piece repeats.
- Settled wording (copied verbatim from the Decision Record; use it exactly):
  - Entry-bundle definition: "the eagerly loaded composition: `crates/workshop/ui/src/main.ts` and the `*.contribution.ts` modules it imports. Lazy panels never import a module inside it, directly or through another import. Everything else that eager and lazy code both import, such as `services/`, `base/`, and shared parts modules like `parts/layout/zones.ts`, is shared code."
  - `index.ts` rule: "A part that the panel registry loads lazily has an `index.ts` as its chunk entry (`crates/workshop/ui/src/services/panel-registry.ts:194-222`), and other parts need none." It replaces "every directory has an `index.ts`" and the `index.ts` clause of root `AGENTS.md:80`.
- Artifacts and edits (cited lines are at `ce10a8eb`; locate by content):
  - `.cursor/rules/workshop-architecture.mdc`:
    - `:12`: list the real crates per tier from `crates/build-xtask/src/tidy.rs:22-31`. There's no `workshop-sessions`, and `workshop-user-state` is a feature crate.
    - `:13`: state the one allowed same-tier edge, `workshop-registry` to `workshop-protocol` (`tidy.rs:65-68`).
    - `:19`: the composition root calls each subsystem's `register` by name and requires its concrete handle, and subsystems reach each other only through the registry.
  - `.cursor/rules/workshop-spa.mdc`:
    - `:12`: add the missing parts chatbox, quickinput, run, and workspace-files.
    - `:13`: the `index.ts` rule above.
    - `:14`: the entry-bundle definition above.
  - Root `AGENTS.md:33,74,80`: the entry-bundle definition and the `index.ts` clause.
  - `crates/workshop/ui/AGENTS.md`:
    - `:5`: admit the panel registry's deliberate dynamic imports of parts.
    - `:13`: each service token lives in its own service module.
    - `:17`: the entry-bundle definition.
    - Replace any "services are DOM-free" claim with "services own no views", since `crates/workshop/ui/src/services/text-control-service.ts:106-107` attaches document listeners.
  - `crates/workshop/ui/build.mjs:51-52`: the entry-bundle definition.
  - `crates/workshop/desktop/AGENTS.md:11`: the supervisor is `src/gateway/supervisor/`.
  - `README.md:107`: the guide's documentation sets. `tools/document.md:123`: the nonexistent sessions path.
- Work: before writing the entry-bundle definition, confirm from the esbuild import graph that no lazy chunk includes `main.ts` or any `*.contribution.ts` module, directly or through another import. If one does, stop and report instead of choosing another definition. Lazy panels importing shared parts modules such as `zones.ts`, `workspace-drops.ts`, or `workshop-panel.ts` is expected and allowed.
- Tests: `rg -n "workshop-sessions" .cursor/rules` and `rg -n "supervisor\.rs" crates/workshop/desktop/AGENTS.md` find nothing; `npm run build` and `npm test` in `crates/workshop/ui` pass, including `docs-claims.mjs` and `lazy-css-entry-bundle.mjs`.
- Commit: the rule and markdown rewrites.

</step-15>

<step-16>

### Step 16: Correct the subsystem crate docs [completed]

- Component: Documentation

- Piece: subsystem crate docs, after the markdown piece. Sequential; one docs gate covers every crate here.
- Artifacts and edits (cited lines are at `ce10a8eb`; Steps 8 through 12 moved some, so locate by content):
  - Registry:
    - `crates/workshop/registry/Cargo.toml:9`: the composition root requires concrete handle types.
    - `crates/workshop/registry/src/lib.rs:25-29`: `MenuPush` is a struct; list `StatusChannel`; `Push::menu()` is a per-subsystem accessor; a new subsystem also needs a compose helper and a `require`.
    - Each trait doc in `crates/workshop/registry/src/traits.rs` names the `handles.rs` that registers its adapter.
  - Gateway, all under `crates/workshop/gateway/src/`:
    - `heartbeat.rs:16`: "status subscribers", not "observer".
    - `heartbeat.rs:119-122`: the cache calls have no production caller.
    - `heartbeat.rs:197-205`: remove the orphaned run-loop doc.
    - `gateway_progress.rs:282`: the senders live in the registry's `GatewayHandles`.
    - `gateway/progress.rs:5`: switch is a single JSON document.
    - `gateway_binding.rs:22,24,49`: one client.
    - `gateway_binding.rs:235,244`: "desktop app", not "desktop host".
    - `gateway.rs:7-8`: progress decodes into `ProgressStream`.
    - `lib.rs:17-18`: the gateway drives the menu through `MenuPush`.
    - Capitalize "Gateway" and "Workshop" only when naming the product.
  - Other crates:
    - `crates/workshop/support/Cargo.toml:9,13-14` and `crates/workshop/support/src/bus.rs:13-15`.
    - `crates/workshop/workspace/Cargo.toml:9,13-14`, `crates/workshop/workspace/src/workspace.rs:2`, and `crates/workshop/workspace/src/workspace/token.rs:3-4`.
    - `crates/workshop/workspace/src/handles.rs:3-4,27,34-35`: the reader of the roots is the server, not a same-tier subsystem.
    - `crates/workshop/protocol/src/lib.rs:87-89` and `crates/workshop/protocol/tests/it/fixture.rs:188`.
    - `crates/workshop/status/src/lib.rs:19-20` and `crates/workshop/status/src/status.rs:1` ("status bus").
    - `crates/workshop/menu/src/handles.rs:1-4`: registration and handles.
    - `crates/workshop/menu/src/menu-memory.rs:23`: layout state doesn't live in localStorage.
- Tests: `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`; `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`; the non-workshop clippy partition; `cargo fmt --all --check`; `rg -n "session_agents" crates/workshop` finds nothing.
- Commit: the subsystem doc corrections.

</step-16>

<step-17>

### Step 17: Correct the server and desktop docs [completed]

- Component: Documentation

- Piece: server and desktop docs, after the subsystem piece. Sequential; the private-items docs gate and the workshop doctests cover these crates, not the workspace docs gate.
- Artifacts and edits (cited lines are at `ce10a8eb`; locate by content):
  - `crates/workshop/server/src/lib.rs`, the line reading "The server's WebSocket origin policy is applied to every upgrade": `/v1/realtime` uses a stricter same-origin check.
  - `crates/workshop/server/Cargo.toml:24-26` (shifted by Step 8): the status reporter, plus the prompts route as an engine user.
  - `crates/workshop/server/build.rs:6`: name the Node.js and `npm ci` requirement directly.
  - `crates/workshop/server/src/cross_site.rs:13-14`: three upgrade handlers.
  - `crates/workshop/server/src/routes.rs:3`: add `/user/state`.
  - `crates/workshop/server/src/routes/gateway_config.rs:25-29`: the config-asset routes also have no deadline.
  - `crates/workshop/server/tests/it/main.rs:1-2`, `crates/workshop/server/src/agents/bindings.rs:83`, and `crates/workshop/server/src/workshop_socket.rs:3`.
  - `crates/workshop/desktop/src/gateway.rs:3-4`: describe the real imports between `boot`, `identity`, and `supervisor`.
  - `crates/workshop/desktop/src/main.rs:285`: drop "as today".
- Work: keep the `/ws` docs that `eca41e57` corrected in `crates/workshop/server/src/lib.rs` and `crates/workshop/server/src/agents.rs:1-5` accurate, so the private-items docs step stays green.
- Tests: `RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps -p workshop-server --document-private-items`; `cargo test --doc -p workshop -p workshop-server -p workshop-server-api`; the workshop clippy partition; `cargo fmt --all --check`.
- Commit: the server and desktop doc corrections.

</step-17>

<step-18>

### Step 18: Replace plan-step comments and fix the UI guard tests [completed]

- Component: Documentation

- Piece: UI source comments, after the Rust doc pieces. Sequential, because it repeats the entry-bundle wording Step 15 settled.
- Artifacts and edits:
  - Replace the "plan step N" comments under `crates/workshop/ui/src` (23 at `890ae2f6`, for example `workspace-files.contribution.ts:25`, `editor.contribution.ts:2`, and `add-folder.ts:3`) with plain statements of the constraint.
  - Add header comments to `index.ts`, `take-registry.ts`, `take-registry-events.ts`, `take-registry-state.ts`, and `take-registry-types.ts` in `crates/workshop/ui/src/parts/take/`.
  - Fix the header of `crates/workshop/ui/test/lazy-css-entry-bundle.mjs:1-2` to the entry-bundle definition: "the eagerly loaded composition: `crates/workshop/ui/src/main.ts` and the `*.contribution.ts` modules it imports. Lazy panels never import a module inside it, directly or through another import. Everything else that eager and lazy code both import, such as `services/`, `base/`, and shared parts modules like `parts/layout/zones.ts`, is shared code."
  - Retire the guide-phrase half of `crates/workshop/ui/test/docs-claims.mjs:43-59`: `STALE_GUIDE_PHRASES` and its test.
  - `crates/workshop/ui/src/services/protocol.ts:17`.
- Tests: `rg -n -i "plan step" crates/workshop/ui/src` finds nothing; `npm run build`, `npm test`, and `npm run typecheck` in `crates/workshop/ui`.
- Commit: the UI comment and guard-test changes.

</step-18>

<step-19>

### Step 19: Add the crate map and vocabulary entries [completed]

- Component: Documentation

- Piece: crate map and vocabulary, last in the component. Sequential, because it summarizes the corrected docs and edits root `AGENTS.md` after Step 15.
- Artifacts: new `crates/workshop/AGENTS.md`; the Vocabulary section of root `AGENTS.md`; the crate or module docs that cite where each new term is defined.
- Work:
  - Write `crates/workshop/AGENTS.md` for humans:
    - One line per crate with its tier from `crates/build-xtask/src/tidy.rs:22-31`:
      - `desktop` (package `workshop`): the Tauri desktop app. It spawns the server in-process through `workshop-server-api` and supervises the gateway sidecar.
      - `server`: the Axum server and composition root. It holds the agent sessions, the `/ws` workshop socket, and the asset, gateway-config, prompts, realtime, and health routes.
      - `server-api`: the desktop app's only view of the server.
      - `gateway`: the gateway client, binding publication, heartbeat, and progress feed.
      - `workspace`: the jailed filesystem with granted roots, plus the `.pfwork` workspace file.
      - `menu`: Model menu state and the chat catalog.
      - `user-state`: per-account UI state.
      - `status`: the status bus.
      - `protocol`: wire frame types.
      - `registry`: self-registration slots and the `Push` facade.
      - `support`: shared primitives and test fixtures.
      - `ui`: the TypeScript SPA.
    - The three runtime links from `crates/workshop/registry/src/lib.rs:9-13`.
    - The rule that the desktop app depends only on `workshop-server-api`.
    - No dependency lists.
  - Add take, zone, chip, contribution, status bus, sidecar, and publication to the root `AGENTS.md` Vocabulary section. Define each from the code where it is used, and cite that file in the crate or module doc, not in the vocabulary line.
- Tests: each crate line matches its tier in `tidy.rs` and each vocabulary term matches its code; the workspace docs gate and the private-items docs gate if a Rust doc changed; `npm run typecheck` in `crates/workshop/ui` if a TypeScript doc changed; `cargo fmt --all --check`.
- Commit: the crate map, the vocabulary entries, and their doc citations.

</step-19>

<step-20>

### Step 20: Run the exit gates

- Component: Exit

- Component order: last, because the exit criteria run once, after every item.
- Piece: exit gates, a single step with no construction choice.
- Artifacts: an exit section in this plan's repository copy under `vibe/`, beside the Step 1 baseline.
- Work:
  - Confirm that every work item from Steps 2 through 19 is done, or that an inferred item's non-reproduction is recorded.
  - Run each of these exit commands with the staged sidecar (copied verbatim from the Testing Plan exit criteria), and compare each with its Step 1 baseline. Each must be at least as green:
    - `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`
    - `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`
    - `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`
    - `cargo nextest run --locked -p workshop-server --features headless`
    - `cargo nextest run --locked -p workshop-workspace --all-features` (on Windows)
    - `cargo test --doc -p workshop -p workshop-server -p workshop-server-api`
    - `cargo doc --locked --no-deps -p workshop-server --document-private-items` with `RUSTDOCFLAGS=-D warnings`
    - `cargo test -p build-xtask`
    - both clippy partitions with `-D warnings`: `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings` and `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`
    - `cargo check -p gateway --no-default-features`
    - `cargo fmt --all --check`
    - `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api` and `cargo doc -p promptforge --no-deps`, each with `RUSTDOCFLAGS=-D warnings`
    - `cargo +<pinned nightly> xtask api --check`, with the nightly named in `crates/build-xtask/src/api/toolchain.rs`
    - `mdbook build guide`
    - `cargo run -p build-user-guide`, changing no tracked file under `guide/`
    - `npm run build`, `npm test`, and `npm run typecheck` in `crates/workshop/ui`
    - `cargo workshop`
  - Run the retired-string checks, which must find nothing outside `vibe/`: `rg -n "workshop-sessions" .cursor/rules`, `rg -n "session_agents|decode_path_param|workbench\.toml" crates/workshop`, `rg -n "supervisor\.rs" crates/workshop/desktop/AGENTS.md`, and `rg -n "workshop-server\b" guide/src`.
  - Run `rg -n "open_browser" crates/workshop`, which must find only the two tests proving an old config with the key still loads.
  - Confirm that `crates/workshop/server/src/main.rs` and the `[[bin]]` table are gone and no tracked file names the `workshop-server` binary.
- Tests: the exit commands and checks above.
- Commit: the recorded exit results in the plan's repository copy.

</step-20>

</execution-plan>
