---
name: Workshop crates cleanup
overview: Cleanup of crates/workshop in the promptforge repository (branch master, after the engine consolidation). Retires "shell" for anything but terminal shells, closes a bind gap, fixes a save-timeout bug, hardens tests, standardizes registry docs and subsystem handles, moves the /prompts/contract route and shared helpers, applies the file layout convention, splits oversized code, and deletes the workshop's human docs. Runs directly on master; no rebase is needed.
todos:
  - id: baseline
    content: "Baseline: record test results on master before any change"
    status: pending
  - id: bind
    content: "Bind: refuse non-loopback addresses in reuse_bind, with a test"
    status: pending
  - id: dead-code
    content: "Dead code: delete WorkshopObserver and StatusBus helpers, drop unused server re-exports, fix gateway description and a stale comment"
    status: pending
  - id: shell-rename
    content: "Shell rename: gateway icon copies, shell/ to desktop/, tier and constant renames, prose, UI names (desk, view, placeholder, entry bundle), vocabulary in AGENTS.md"
    status: pending
  - id: config-ui
    content: "config-ui: views/ to pages/, page identifiers, shell to desk"
    status: pending
  - id: flaky-tests
    content: "Flaky tests: symlink tests fail under CI, replace fixed sleeps"
    status: pending
  - id: save-timeout
    content: "Save timeout: reproducing test, JSON 408 body on every deadline route, unknown-token state in the editor, audit of UI error consumers"
    status: pending
  - id: security-tests
    content: "Security tests: realtime relay refusals, jail edge cases"
    status: pending
  - id: wire-fixture
    content: "Wire fixture: shared /ws frame fixture for Rust and TypeScript, SelectModelFrame"
    status: pending
  - id: registry-docs
    content: "Registry docs: keep subsystem-named traits, reword claims, record runtime links in the registry crate docs"
    status: pending
  - id: code-docs
    content: "Code-level docs: fix code-comment drift; make the AGENTS.md import pointer explicit"
    status: pending
  - id: prompts-route
    content: "Prompts route: move /prompts/contract into workshop-server, drop workspace engine dependency"
    status: pending
  - id: helpers
    content: "Helpers: move render_message, JSON bucket validator, mock server helper, UI reconnect backoff into shared homes"
    status: pending
  - id: layout
    content: "Layout: directories for hyphenated groups of three or more, distinct ui_state names"
    status: pending
  - id: renames
    content: "Renames: remove server aliases, move /ws socket to a workshop_socket module, rename status relay and gateway SwitchOutcome to SwitchProfileBody, handles named structs, UI tokens into services"
    status: pending
  - id: split
    content: "Split: supervisor.rs, socket.rs framing, compose, heartbeat and progress run loops"
    status: pending
  - id: docs
    content: "Docs removal: delete workshop guide chapters, export, and READMEs; drop them from the guide build, doc tool, and docs-claims test"
    status: pending
isProject: false
---

# Workshop crates cleanup

<product-contract>

## Product Requirements

The workshop crates are well built line by line but hard for a human to explore and maintain. One word, "shell", names several unrelated things. Conventions are applied unevenly, docs contradict the code, some tests depend on timing or skip silently, and there is a bind gap and a likely save-timeout bug. This plan cleans that up inside the workshop crates and a few named exceptions, working directly on master now that the engine consolidation has landed. The workshop's human docs are deleted rather than fixed, and end-user behavior changes only where a fix requires it.

- Problem and users:
  - Users are the human maintainers and agents working on four things: the workshop desktop app, its in-process HTTP server, the subsystem crates, and the two TypeScript UIs (the workshop UI and the gateway config UI).
  - "shell" currently means all of these:
    - the Tauri desktop app (`crates/workshop/shell/`)
    - the build check's tier for the server (`crates/build-xtask/src/tidy.rs:31`, `const SHELL: &[&str] = &["workshop-server"]`, and `crates/workshop/server/src/lib.rs:24`, "Tier: shell")
    - the product-boundary rule's Tauri crate (`crates/build-xtask/src/product.rs:121`)
    - a shared UI component (`createStatusBarShell` in `crates/shared-ui/status-bar.ts`)
    - the workshop UI's main frame (`.ws-shell` in `crates/workshop/ui/src/parts/layout/zones.css:8`)
    - a lazy panel's loading stand-in ("lazy shell", about 28 occurrences)
    - the SPA entry bundle ("boot shell", `AGENTS.md:61`)
    - config-ui's post-login frame (`mountLiveShell` in `crates/gateway/config-ui/ui/src/main.ts:204`)
  - The workshop UI already stubs a Terminal menu (`crates/workshop/ui/src/parts/menu/stubs.contribution.ts:181-192`), where "shell" will mean a command shell.
- Goals:
  - Reserve "shell" for command shells in terminals, and give every other meaning its own word.
  - Close the loopback bind gap and fix the save-timeout behavior.
  - Make the test suite trustworthy before restructuring: no silent skips, fewer fixed sleeps, tested security surfaces, and `/ws` frames pinned across Rust and TypeScript.
  - Remove dead code and copied helpers. Make conventions uniform (subsystem handles, file layout, names).
  - Split the densest files and functions along their existing seams.
  - Delete the workshop's human docs (its user guide chapters, their export, and the workshop READMEs) and remove them from the guide build. Keep the guide's Gateway, Language, and Agent parts.
  - Keep the remaining code-level docs accurate where this plan touches them: `//!` crate docs, `AGENTS.md` rules, comments, and Cargo descriptions.
  - Keep every step fast, with just enough verification to show it works. Run the full gates only where they count.
- Non-goals:
  - No edits to `crates/promptforge*` or `crates/harness/*`, and none to `crates/gateway/*` beyond the named exceptions under Constraints.
  - No change to the Tauri package name `workshop` or the binary name `promptforge-workshop`.
  - No change to the protocol crate's engine dependency.
  - No edits to the pre-existing dated records under `vibe/`. The active plan's own repository copy and `vibe/ACTIVE` are the plan seed, and the steps edit them.
  - No rewrite of workshop user documentation before beta, and no content edits to the guide's Gateway, Language, or Agent pages.
- Success criteria:
  - Every work item in Execution Instructions is done.
  - Every baseline command is at least as green as its recorded baseline.
  - The retired-name checks in the Testing Plan exit criteria pass.
- Constraints:
  - **Repository.** The repository root is `C:\Users\Vinnie\cursor\promptforge`, on branch `master` at commit 1fd82c62 ("Close plan: debt removal api firewall"). All paths in this plan are relative to that root.
  - **Edit scope.** Edits are allowed in:
    - `crates/workshop/**` and `crates/build-xtask`
    - the workshop parts of the guide: `guide/src/workshop/`, the Workshop entries in `guide/src/SUMMARY.md`, and `guide/promptforge-workshop-guide.md`
    - `crates/build-user-guide/src/main.rs`: the `SETS` list, the doc comment that counts the sets, and the two unit tests that assert the workshop set
    - the link to the deleted Workshop part at `guide/src/introduction.md:27`
    - the hard-coded sidecar path `crates/workshop/shell/binaries` in `tools/stage-gateway-sidecar.mjs:37`, `tools/stage-gateway-sidecar.test.mjs:116`, and `crates/build-workshop/tests/interruption.rs:51-53`
    - vocabulary wording only, in `.cursor/rules/workshop-architecture.mdc` and `.cursor/rules/workshop-spa.mdc`
    - these repository-root files: `Cargo.toml` members, `.gitignore`, `.github/workflows/*`, `AGENTS.md`, `README.md`, and `tools/document.md`
    - the active plan's repository copy under `vibe/` and `vibe/ACTIVE`: the step marks, the baseline results, and the exit results
  - **Named exceptions outside that scope.** Each is small and confined to what is named here:
    - the gateway app's icon copies, and its icon and cross-reference comments (`crates/gateway/app`)
    - the shared status bar rename (`crates/shared-ui` and its consumer in `crates/gateway/config-ui`)
    - the config-ui page and desk renames (`crates/gateway/config-ui/ui`)
    - `crates/gateway/config-ui/ui/src/services/gateway-api.ts` and `panel-bridge.ts`, but only if the timeout audit finds that they render the new 408 badly
  - **History shape.** Every commit builds and passes its focused tests.
    - Moved files keep their content, except for the minimal import or path fixes needed to build.
    - Edits in other files that wire up a move (`mod` lines, `#[path]` attributes, imports, path strings) go in the same commit as the move.
    - Identifier renames and other content edits go in separate commits.
    - Git's rename detection works at this level of similarity, so `git blame --follow` still tracks the moves.
  - **Step size.** Keep steps few and fast. Merge small related edits into one step whenever one focused test set covers them. Mechanical steps (moves, renames, deletions with no behavior change) need no new tests; their check is that the touched packages still compile and their existing focused tests pass.
  - **File-size ceiling.** Files in crates that have the Invariants marker (every `workshop-*` crate) stay at or under 500 physical lines. `cargo test -p build-xtask` enforces this, and the desktop crate is exempt (`AGENTS.md:63`). Files already close to the limit are split before any edit that grows them. Five workshop files are within 20 lines of it: `crates/workshop/server/src/app.rs`, `crates/workshop/server/src/agents/socket.rs` (492 lines), `crates/workshop/server/tests/it/heartbeat_loop.rs`, `crates/workshop/workspace/src/workspace.rs`, and `crates/workshop/workspace/src/workspace_file.rs`. Every line count in this plan is a physical line count.
  - **Verification policy.** Steps and component ends run only the targeted checks listed under Testing Plan. The full canonical gates in `AGENTS.md`, as the Project Survey records them, run only twice: at the baseline and at the final step. Nothing builds the whole desktop app (`cargo workshop`) or runs a workspace-wide suite in between.
  - **Engine crate names.** New code names engine types as little as possible. Where it must, it goes through the `promptforge` facade, the only engine crate the workshop crates depend on (`crates/workshop/gateway/Cargo.toml`, `protocol/Cargo.toml`, `server/Cargo.toml`, and `workspace/Cargo.toml`). It never names a crate under `crates/promptforge-internal/`.
  - **Line numbers.** The citations in `AGENTS.md`, `crates/build-xtask`, and the workshop crates' `lib.rs`, `Cargo.toml`, and `AGENTS.md` files were re-verified on master at 1fd82c62. The rest were recorded on the earlier commit 75245481. Since then, master changed the workshop crates only by renaming engine imports and dependencies to `promptforge` and sweeping docs, so those lines are at most a few off. Always locate code by its content, since lines also shift as the work lands.
  - **Pre-move paths.** This plan cites paths under `crates/workshop/shell/`. They become `crates/workshop/desktop/` once the directory move lands.
- Open questions: None

## Functional Specification

Only four behaviors change for anyone outside the codebase: the standalone server refuses non-loopback binds, a request that hits the route deadline gets a JSON error body, the editor handles a timed-out save as an unknown state instead of a confusing failure, and `/prompts/contract` is served by the server with an unchanged wire contract. The published user guide also loses its Workshop part. Everything else is internal renaming, restructuring, testing, and doc deletion. The desktop app, installers, and release artifacts keep their names and behavior.

- Actors and workflows:
  - End users of the desktop app and of the gateway config UI see no workflow change.
  - Operators of the standalone `workshop-server` binary configure it through `workshop.toml`. After this plan, a non-loopback `server.bind` fails at startup.
  - Maintainers find code through the new vocabulary and a uniform layout.
  - Readers of the published user guide no longer see a Workshop part; the Gateway, Language, and Agent parts are unchanged. The guide is built by `.github/workflows/guide.yml:30` (`mdbook build guide`) and published to Pages. The workshop READMEs are also gone from the repository.
- Inputs and outputs:
  - **`/prompts/contract`.** Path, method, request body, response body, status codes, and wire error codes all stay the same. Only the crate that serves the route changes.
  - **HTTP 408 from the route deadline.**
    - Today the body is empty (`crates/workshop/support/src/deadline.rs:41`, `StatusCode::REQUEST_TIMEOUT.into_response()`).
    - After this plan, the body is JSON in the shape of `ErrorEnvelope`: `{"error":{"message":"...","code":"..."}}`, with a timeout-specific code (`crates/workshop/protocol/src/error.rs:45-69`).
  - **Which routes the 408 change affects.** It is a middleware change, so it covers every route wrapped by `with_deadline`, not only saves:
    - the workspace routes (`crates/workshop/workspace/src/handlers.rs:33`)
    - the user-state routes (`crates/workshop/user-state/src/handlers.rs:34`)
    - the server routes it wraps (`crates/workshop/server/src/app.rs:481-482`)
    - the gateway-config relay routes, under the 35-second relay deadline (`crates/workshop/server/src/routes/gateway_config.rs:31`)
    - the `/v1/models` relay (`crates/workshop/server/src/agents/state.rs:137`)
    - No existing test asserts an empty 408 body.
  - **UI code that reads status or error codes.** None of it handles 408 specially today.
    - Workshop UI: `crates/workshop/ui/src/services/json-request.ts:17-56`, `error-catalog.ts:17-46`, `workspace-api.ts:91-96,159-189`, `workspace-file-client.ts:104-114`, and `run-api.ts:263-267`, all under `crates/workshop/ui/src/services/`.
    - config-ui: 408s from the gateway-config relay reach it through `crates/gateway/config-ui/ui/src/services/panel-bridge.ts:206-220`, and `crates/gateway/config-ui/ui/src/services/gateway-api.ts:349-363,1069-1101` maps them by status and `error.code`.
- States and validation:
  - **Standalone server.** `reuse_bind` (`crates/workshop/server/src/serve.rs:327-340`) parses the configured address into a `SocketAddr` and binds it without checking what it is. After this plan, a parsed address whose IP is not loopback is refused with `std::io::ErrorKind::InvalidInput` before any socket is created. The default is `127.0.0.1:7910` (`crates/workshop/support/src/config.rs`).
  - **Desktop app.** It already forces `127.0.0.1:0` (`crates/workshop/shell/src/config.rs:23,91`), so the bind check doesn't affect it.
- Errors and recovery:
  - **Save timeout today:**
    - A slow `PUT /workspace/file` can exceed `DEFAULT_DEADLINE`, which is 10 seconds (`crates/workshop/support/src/deadline.rs:16`).
    - The client gets an empty 408, but the write runs on the blocking thread pool, can't be cancelled, and may still land on disk.
    - The UI reports that the server "returned a non-JSON answer" (`crates/workshop/ui/src/services/json-request.ts:47-55`), and the next save gets a 409 conflict.
  - **Save timeout after this plan:**
    - **The 408 has a JSON body.**
    - **The token becomes unknown.** When a save gets a 408, the editor's conflict token becomes "unknown". That state is new: today `crates/workshop/ui/src/parts/editor/editor-panel.ts:189-190` always sets `this.token = written.token`, and `crates/workshop/ui/src/services/workspace-api.ts:189` always sends `expected_token`.
    - **The user is told.** The editor says the save may or may not have landed, and it never sends a stale token.
    - **The next save re-reads first.** If the disk content matches what the editor tried to save, it adopts the returned token and saves. Otherwise it shows the existing conflict dialog.
  - **Remaining race:** the late write can still land after that re-read. The re-read narrows the race but does not remove it. The worst case is the existing conflict dialog, never a raw error.
  - **Status of the bug:** it was inferred from reading the code and has not been reproduced. The work item starts with a test that reproduces it.
- Security and privacy behavior:
  - **Bind.** Loopback-only binding is enforced in code. The server's own docs already promise it (`crates/workshop/server/AGENTS.md`), and the cross-site Host check does not stop a raw LAN client that forges a loopback Host header.
  - **Path jail.** Behavior is unchanged. New tests cover UNC and verbatim paths, case-only respellings, and Windows directory junctions.
  - **Realtime relay.** Behavior is unchanged. New unit tests pin its origin and subprotocol refusals.
- Acceptance criteria:
  - The standalone server refuses non-loopback IPv4 and IPv6 addresses, including `0.0.0.0` and `::`, and accepts `127.0.0.1` and `::1`.
  - Every 408 body produced by `with_deadline`, parsed as JSON, equals `serde_json::to_value(workshop_protocol::ErrorEnvelope::new(message, code))` for the timeout message and code.
  - After a save times out, the editor shows the unknown state and sends no stale token. A later save either succeeds or shows the existing conflict dialog.
  - The workshop UI and config-ui show the new timeout code as a readable message, and never as a JSON parse failure.
  - The `/prompts/contract` tests pass against the server with unchanged assertions.
  - The desktop app builds from `crates/workshop/desktop`, and all three workflows reference the new path.
  - `mdbook build guide` succeeds with no Workshop section. `cargo run -p build-user-guide` writes only the gateway, language, and agent exports and leaves them unchanged, and no workshop export remains.

</product-contract>
<implementation-contract>

## Technical Design

The design introduces one vocabulary across the Rust crates and both UIs, moves one directory and one HTTP route, and changes a small set of public APIs in the workshop crates. It adds shared helpers to `workshop-support`, renames one shared-ui export, and gives the gateway app its own icon copies. The tier graph is unchanged apart from renaming the server's tier from "shell" to "server". Nothing outside the edit scope depends on the changed items.

- Architecture:
  - **Vocabulary.** These words apply to identifiers, file and directory names, CSS classes, and prose:
    - "shell": a command shell in a terminal, and nothing else.
    - "desktop app": the Tauri crate (package `workshop`). Its directory becomes `crates/workshop/desktop/`.
    - "server": the build check's tier for `workshop-server`, formerly "shell", named after the only crate in it.
    - "desk": a UI's main frame. The word has no existing uses in `crates/workshop/ui/src`, `crates/gateway/config-ui/ui/src`, or `crates/shared-ui`.
      - In the workshop UI, `.ws-shell` becomes `.ws-desk`. It is the flex parent the dock column fills, with the status bar outside it (`crates/workshop/ui/src/parts/layout/zones.css:8`).
      - In config-ui, the post-login frame (the tab bar plus its pages) becomes the desk.
    - "workbench": keeps only its existing senses, and this plan adds none:
      - the Model menu snapshot frame on the `/ws` socket (`WorkbenchFrame`, `{"type":"workbench"}`, in `crates/workshop/protocol/src/workbench.rs` and `crates/workshop/menu/src/menu.rs`)
      - the VS Code-style UI architecture described in `crates/workshop/ui/AGENTS.md`
    - "workshop socket": the `/ws` socket. Its server module becomes `workshop_socket`, pairing with the UI client `crates/workshop/ui/src/services/workshop-socket.ts`, the same way `crates/workshop/server/src/agents/socket.rs` pairs with `crates/workshop/ui/src/services/agent-socket.ts`. Today the server's crate doc (`crates/workshop/server/src/lib.rs:14`) calls it "the /ws workbench socket"; that wording becomes "the /ws workshop socket". `crates/workshop/README.md:11` says the same, but that file is deleted.
    - "page": a routed screen behind a tab. This is what config-ui's `views/*-view.ts` files are today.
    - "view": a DOM component. shared-ui's status bar becomes `StatusBarView`.
    - "placeholder": what a lazy panel shows while its code chunk loads (`.ws-panel-lazy`). Today it is called a "lazy shell" or "empty shell", about 28 times, in `crates/workshop/ui/src/parts/layout/zones.css`, `crates/workshop/ui/src/parts/layout/panel-types.ts`, and `crates/workshop/ui/test/lazy-panel-sizing.mjs`. The local variable `shell` that holds the placeholder element in `crates/workshop/ui/test/lazy-panel-sizing.mjs:222` becomes `placeholder`. `zones.css` uses both senses: `.ws-shell` at line 8 is the desk, and the "lazy shell" prose at line 272 is the placeholder, so each occurrence in that file is classified by meaning.
    - "entry bundle": what `AGENTS.md:61` calls the "boot shell", which lazy panels must never import.
  - **Tier graph.** Unchanged except for the tier's name:
    - The desktop app depends only on `workshop-server-api`, and `workshop-server-api` re-exports `workshop-server`.
    - The server (server tier) may depend on the feature, service, and vocabulary crates.
    - The feature crates (user-state, workspace) and the service crates (gateway, menu, status) depend only on vocabulary.
    - Within vocabulary, `workshop-registry` depends on `workshop-protocol`.
    - `cargo test -p build-xtask` enforces this graph (`crates/build-xtask/src/tidy.rs`).
  - **Registry.** It keeps its subsystem-named traits: `MenuSink`, `CatalogSink`, `StatusSink`, and `WorkspaceRoots` in `crates/workshop/registry/src/traits.rs`, and `MenuPush` in `crates/workshop/registry/src/push.rs`. Its crate docs (`crates/workshop/registry/src/lib.rs`) and its Cargo description stop claiming it never names a subsystem. The crate docs record the real runtime links:
    - The gateway drives the menu through `MenuPush` (`crates/workshop/registry/src/push.rs:141-175`).
    - Publishing a model catalog forces a menu reconcile (`crates/workshop/registry/src/push.rs:99-106`).
    - Agent sessions read the workspace's granted roots through `WorkspaceRoots`.
- Modules and interfaces:
  - **Subsystem handles.**
    - Every subsystem crate (gateway, menu, status, user-state, workspace) has `src/handles.rs`. Its `register` function returns a named struct of registration guards instead of a tuple, and so does `register_tasks` where one exists.
    - user-state gains a `handles.rs`; its `register` lives in `crates/workshop/user-state/src/lib.rs:45` today.
    - The server's `compose` (`crates/workshop/server/src/app.rs`, around lines 338-424) reads the named fields instead of unpacking tuples by position.
  - **Shared helpers in `workshop-support`:**
    - **Error message rendering.** `render_message` and `LEAK_DETAIL` are copied word for word in `crates/workshop/workspace/src/error.rs`, `crates/workshop/server/src/error.rs`, and `crates/workshop/user-state/src/error.rs`, and `LEAK_DETAIL` also appears in `crates/workshop/server/src/agents/relay.rs`.
    - **The JSON state-bucket validator.** It checks the key against an allow list, caps the body at 1 MiB, and requires it to parse. It exists twice: in `crates/workshop/user-state/src/store.rs` and `handlers.rs`, and in `crates/workshop/workspace/src/workspace_file-ui-state.rs` and `handlers-file-state.rs`.
    - **A mock HTTP server test helper** behind support's `test-fixtures` feature. It binds a loopback port and runs `axum::serve`. About 11 near-copies exist across the gateway and server tests. The server's own copy is in `crates/workshop/server/src/app-fixtures.rs:73-84`, but the gateway can't depend upward on the server, so the helper belongs in support.
  - **Deadline body.**
    - `with_deadline` stays in `crates/workshop/support/src/deadline.rs`, because callers exist outside the server:
      - `crates/workshop/workspace/src/handlers.rs:33`
      - `crates/workshop/user-state/src/handlers.rs:34`
      - in the server: `crates/workshop/server/src/app.rs:481-482`, `crates/workshop/server/src/routes/gateway_config.rs:31`, and `crates/workshop/server/src/agents/state.rs:137`
    - Vocabulary crates may depend only from registry to protocol, so support can't depend on protocol and builds the JSON body itself.
    - To pin the shape, a server test parses the body as a JSON value and compares it with `serde_json::to_value(workshop_protocol::ErrorEnvelope::new(message, code))`.
    - `ErrorEnvelope` is public, has the public constructor `new(message, code)`, and derives only `Serialize`. Its inner `EnvelopeBody` and fields are private (`crates/workshop/protocol/src/error.rs:45-69`). So the test needs no `Deserialize` and no change to the protocol API.
    - The server, workspace, and user-state already build envelopes through `ErrorEnvelope::new`: `crates/workshop/server/src/error.rs:96`, `crates/workshop/workspace/src/error.rs:275`, and `crates/workshop/user-state/src/error.rs:94`.
  - **`/prompts/contract`.**
    - The route moves out of `crates/workshop/workspace/src/handlers-prompts.rs` and `handlers-prompts-tests.rs` into a server-owned route module under `crates/workshop/server/src/routes/`. It is mounted beside health, realtime, and gateway_config, under the default deadline.
    - Its error mapping moves from the workspace error type to the server's `AppError`, keeping the same status codes and wire error codes.
    - This route is workspace's only reason to depend on `promptforge` (`crates/workshop/workspace/Cargo.toml`), so workspace drops that dependency. The server already depends on `promptforge`, so nothing is added there. Both crates' Invariants blocks in `src/lib.rs` are updated to match.
  - **The `/ws` workshop socket.** `crates/workshop/server/src/agents/session.rs` and `session-menu.rs` move out of `agents/` into a `workshop_socket` module in the server: `crates/workshop/server/src/workshop_socket.rs`, with its menu child in `workshop_socket-menu.rs`. `SessionsState` still mounts the `/ws` route (`crates/workshop/server/src/agents/state.rs:141`).
  - **Protocol.** It gains a Rust type for the inbound `select_model` frame, next to `SwitchProfileFrame` in `crates/workshop/protocol/src/menu.rs`, plus a matching TypeScript interface in `crates/workshop/ui/src/services/protocol.ts`.
  - **Workshop UI service tokens.** These four tokens and their interface types move into `crates/workshop/ui/src/services/`. The implementations stay in `parts/` and register against the tokens:
    - `STATUS_BAR` (`crates/workshop/ui/src/parts/status/status-bar.ts:198`)
    - `CLOSED_EDITORS` (`crates/workshop/ui/src/parts/editor/closed-editors.ts:145`)
    - `EDITOR_SETTINGS_SERVICE` (`crates/workshop/ui/src/parts/editor/editor-settings-service.ts:165`)
    - `QUICK_INPUT_SERVICE` (`crates/workshop/ui/src/parts/quickinput/quick-input.ts:308`)
- File and public API changes:
  - **Directory move.** `crates/workshop/shell/` becomes `crates/workshop/desktop/`. Path strings change in:
    - root `Cargo.toml` (the `members` entry) and `.gitignore:22,25`
    - `.github/workflows/nightly.yml:204-210`, `.github/workflows/release-workshop.yml:167-191`, and `.github/workflows/workshop-installer-smoke.yml:8-9,43`
    - `crates/build-xtask/src/tidy.rs:84-86`, where the fallback directory `"shell"` becomes `"desktop"`, and `crates/build-xtask/src/tidy-tests.rs:223`
    - `README.md:84` and `AGENTS.md:27`. `tools/document.md:105` goes away with the workshop lens.
    - the sidecar staging path `crates/workshop/shell/binaries`, in `tools/stage-gateway-sidecar.mjs:37`, `tools/stage-gateway-sidecar.test.mjs:116`, and `crates/build-workshop/tests/interruption.rs:51-53`. `cargo workshop` (`crates/build-workshop/src/main.rs`) and CI (`.github/workflows/ci.yml:175,256` and `.github/workflows/workshop-installer-smoke.yml:38`) stage the sidecar through that script.
    - `git mv` leaves behind the gitignored build artifacts under the old path: the staged sidecar in `crates/workshop/shell/binaries/` and the Tauri output in `crates/workshop/shell/gen/`. Move them to the new path or delete them so no stale `crates/workshop/shell/` directory remains. `cargo workshop` re-stages the sidecar.
    - two gateway comments that point to `crates/workshop/shell/src/gateway.rs`: `crates/gateway/app/src/tray/windows.rs:596` and `crates/gateway/app/src/tray/macos.rs:459`
  - **Gateway icon source.**
    - Today the gateway app embeds `../../workshop/shell/icons/icon.ico` (`crates/gateway/app/build.rs:23`), and its test reads the same file (`crates/gateway/app/tests/it/icon.rs:42`).
    - It gets its own copies of `icon.ico`, `32x32.png`, and `64x64.png` in `crates/gateway/app/assets/`, next to the existing `tray-icon.rgba` and `tray-icon-template.rgba`.
    - The build, the test, and these comments point at the copies: `build.rs:7`, `tests/it/icon.rs:2`, `Cargo.toml:19`, `src/tray/windows.rs:49-51`, `src/tray/macos.rs:63-66`, and `src/tray/linux.rs:54`.
    - `crates/workshop/shell/icons/AGENTS.md` already requires config-ui's icon copies to stay in sync with the master icons. That rule is extended to cover the gateway app's copies.
  - **build-xtask.**
    - `crates/build-xtask/src/tidy.rs:31`: `SHELL` becomes `SERVER`, with tier name "server". The tier and fallback prose in the same file changes too (lines 27, 30, 84, and 86, including "Tier 3: the shell"). So does the "Tauri shell" wording at lines 13 and 252, which becomes "desktop app".
    - `crates/build-xtask/src/product.rs:121`: `SHELL` becomes `DESKTOP`. Its value stays `"workshop"`.
    - `crates/build-xtask/src/new_crate.rs:72`: the tier list in the new-crate template is updated.
    - Test names that mention "shell" in `crates/build-xtask/src/tidy-tests.rs` and `crates/build-xtask/src/product-tests.rs` are renamed to match.
  - **workshop-server.**
    - "Tier: shell" becomes "Tier: server" (`crates/workshop/server/src/lib.rs:24`).
    - The pre-decomposition module aliases are removed (`crates/workshop/server/src/lib.rs:60-67`). About 24 call sites switch to the real crate paths.
    - The unused re-exports `CacheEvent`, `CacheResponse`, and `SsePayloadStream` are removed (`crates/workshop/server/src/lib.rs:93-96`), along with the `observer` alias.
    - `workshop-server-api` re-exports none of these (`crates/workshop/server-api/src/lib.rs`).
  - **workshop-gateway.**
    - `WorkshopObserver` and its module are deleted (`crates/workshop/gateway/src/observer.rs` and `observer-tests.rs`). Nothing outside those two files uses them.
    - With it go the gateway's engine dependency (`promptforge`, which is used only there) and the words "the run event log" in its Cargo description (`crates/workshop/gateway/Cargo.toml:9`).
    - The public cache API (`cache_ensure`, `CacheEvent`, `CacheResponse`, `SsePayloadStream`) stays, for a planned caller.
    - The gateway's `SwitchOutcome` is the switch-profile JSON body. It is renamed `SwitchProfileBody` so it stops colliding with workshop-menu's unrelated `SwitchOutcome`.
  - **workshop-status.** `StatusBus::report`, `info`, `debug`, `error`, and `idle` are removed (`crates/workshop/status/src/status.rs:67-117`). Only their own tests call them; producers use `Push`.
  - **shared-ui.** `createStatusBarShell` becomes `createStatusBarView`, and `StatusBarShell` becomes `StatusBarView` (`crates/shared-ui/status-bar.ts`).
    - The code consumers are `crates/workshop/ui/src/parts/status/status-bar.ts`, `crates/workshop/ui/test/shared-status-bar.mjs`, and `crates/gateway/config-ui/ui/src/components/status-bar.ts`.
    - Comments change in `crates/shared-ui/status-bar.css:9`, the `crates/shared-ui/package.json` description, `crates/gateway/config-ui/ui/src/components/status-bar.test.mjs:3`, and `crates/gateway/config-ui/ui/src/styles/layout.css:1423`.
  - **config-ui** (paths relative to `crates/gateway/config-ui/ui/src`). Nothing in the workshop crates references these names.
    - **Files.** `views/` becomes `pages/`. The six `*-view.ts` and `*-view.test.mjs` pairs (cloud-models, discover, models, profiles, secrets, settings) become `*-page.*`. `apply-revert.test.mjs`, `model-detail.test.mjs`, and `settings-sections.test.mjs` move without being renamed. The six imports at `main.ts:35-40` follow.
    - **Page identifiers.** About 300 references change:
      - `createXView` becomes `createXPage`, and `XViewDeps` becomes `XPageDeps`.
      - `ViewId` becomes `PageId`, and `viewRoot` becomes `pageRoot`.
      - `setActiveView` becomes `setActivePage`, `tabByView` becomes `tabByPage`, and `defaultView` becomes `defaultPage`.
      - `PendingView` becomes `PendingPage`, and `disposeView` becomes `disposePage`.
      - The `.view-empty` class becomes `.page-empty`.
      - `review`, `viewport`, and `openReviewDiff` stay unchanged.
    - **The desk.** About 77 references across 22 files change, test descriptions included:
      - `mountLiveShell` (`main.ts:204`) becomes `mountLiveDesk`, and `showShell` (`main.ts:103`) becomes `showDesk`.
      - The inert panel-mode mount, documented at `main.ts:475`, is described as the inert desk.
      - `main.className = "shell"` (`main.ts:513`) and the `.shell` rules at `styles/layout.css:71,1438` become `.desk`.
      - Local variables named `shell` that hold the frame are renamed as well.
  - **Layout.**
    - A group of three or more hyphenated sibling files moves into a directory in standard module layout, and its `#[path]` attributes are dropped. The repository already states this convention at `AGENTS.md:64`. The groups:
      - `crates/workshop/workspace/src/workspace.rs` (8 path-wired children)
      - `crates/workshop/workspace/src/handlers.rs`
      - `crates/workshop/workspace/src/workspace_file.rs`
      - the gateway's `crates/workshop/gateway/src/gateway_progress-*` group
    - The four modules named `ui_state` get distinct names. They live in `workspace-ui-state.rs`, `workspace-tests-ui-state.rs`, `workspace_file-ui-state.rs`, and `workspace-file-tests-ui-state.rs`.
  - **Workshop docs removal.**
    - **Deleted from the guide:** `guide/src/workshop/` (the index and chapters 01 through 11), the Workshop entries in `guide/src/SUMMARY.md` (lines 4-17), and the export `guide/promptforge-workshop-guide.md`. The Gateway (`SUMMARY.md` lines 20-33), Language (35-47), and Agent (49-61) parts stay.
    - **The guide build.** `crates/build-user-guide/src/main.rs:17-22` lists four export sets (workshop, gateway, language, agent), and lines 79-81 write `promptforge-{set}-guide.md` for each. `workshop` is removed from that list. The doc comment then counts three sets, and the two unit tests that assert the workshop set (`summary_has_parts_in_audience_order` and `assembly_is_deterministic`) are pointed at the remaining parts. The crate generates `guide/src/SUMMARY.md` and each part's `index.md`, so those files are regenerated with `cargo run -p build-user-guide`, never edited by hand.
    - **The guide introduction.** Its link to the deleted Workshop part (`guide/src/introduction.md:27`) is removed. Nothing else on that page changes.
    - **Deleted from the crates:** `crates/workshop/README.md`, `crates/workshop/server/README.md`, `crates/workshop/shell/README.md`, `crates/workshop/user-state/README.md`, and `crates/workshop/workspace/README.md`. Any Cargo `readme` key, `include_str!`, or link that names one of them goes too.
    - **Kept:** the four agent-rule files (`crates/workshop/server/AGENTS.md`, `crates/workshop/shell/AGENTS.md`, `crates/workshop/shell/icons/AGENTS.md`, and `crates/workshop/ui/AGENTS.md`), the license notices in `crates/workshop/ui/THIRD_PARTY_NOTICES.md`, the `//!` crate docs with their mandatory Invariants blocks, and the Cargo descriptions.
    - **The docs-claims test.** `crates/workshop/ui/test/docs-claims.mjs` (lines 35-73) checks root `AGENTS.md`, every page under `guide/src`, and the workshop export for stale phrases. Its workshop-export check is removed; the other two checks stay.
    - **The doc tool.** `tools/document.md` loses its workshop lens, which writes `guide/src/workshop/` and targets the workshop crates at line 105, so the tool can't regenerate the deleted guide.
    - **Rustdoc is unchanged.** CI already leaves the three top workshop crates out of `cargo doc` (`.github/workflows/ci.yml:141`).
- Data, persistence, failure, security, and privacy constraints:
  - No persisted format changes: `.pfwork` workspace files, the user-state JSON file, and the `workshop.toml` schema all stay the same.
  - The only wire change is the 408 body. The `/ws` and `/agents/ws` frame shapes stay the same, and the new fixture pins them.
  - The icon copies stay byte-identical to the master icons until the brand changes.

</implementation-contract>
<verification-contract>

## Testing Plan

The full canonical gates run twice: once before any change, as the baseline, and once at the final step. In between, each step runs only its own focused tests, and each component end runs the suites and lints of just the packages that component touched, so no step rebuilds the world. New tests pin the bind refusal, the 408 body and the editor's recovery from it, the realtime relay's refusals, the jail's edge cases, and the shapes of the `/ws` frames. Timing-based tests move to event-driven waits. Exit also requires a clean guide build and a grep showing the retired names are gone within scope.

- Unit:
  - **Bind refusal**, in `crates/workshop/server/src/serve-tests.rs`: non-loopback IPv4 and IPv6 addresses are refused, and `127.0.0.1` and `::1` are accepted.
  - **Realtime relay**, in a new `crates/workshop/server/src/routes/realtime-tests.rs`: origin refusal and subprotocol refusal. `crates/workshop/server/src/routes/realtime.rs` has no unit tests today.
  - **Jail edge cases**, in `crates/workshop/workspace/src/workspace-tests.rs`: UNC and verbatim `\\?\` paths, case-only respellings of a granted root, and a Windows directory junction.
  - **Socket framing helpers.** They get table-driven tests when they are split out of `crates/workshop/server/src/agents/socket.rs`, which has 2 unit tests today.
  - **408 body shape.** A server test parses the body that `with_deadline` produces as a JSON value, and compares it with `serde_json::to_value(workshop_protocol::ErrorEnvelope::new(message, code))`.
  - **`/prompts/contract`.** Its tests move to the server with their assertions unchanged.
- Integration and end-to-end:
  - **Save timeout.**
    - A server test reproduces today's behavior: a write that runs past the deadline gets an empty 408, and the write can still land on disk. It must fail against the unfixed code and pass after the fix.
    - UI tests in `crates/workshop/ui/test/` cover how the editor handles a 408 on save:
      - the token becomes unknown, and no stale token is sent
      - a disk match adopts the new token and saves
      - a mismatch shows the conflict dialog
      - a late write that lands after the re-read leads to the conflict dialog, not a raw error
  - **`/ws` frame fixture.** A new `crates/workshop/protocol/tests/fixtures/workshop-frames.json` covers the status, models, workbench, error, and switch_profile frames. Both `crates/workshop/protocol/tests/it/` and a new `crates/workshop/ui/test/workshop-wire-fixtures.mjs` assert it. This mirrors the existing `crates/workshop/protocol/tests/fixtures/agent-frames.json` and `crates/workshop/ui/test/agent-wire-fixtures.mjs`.
  - **Gateway icon embedding.** `cargo nextest run --locked -p gateway` runs it. `gateway` is the package name of `crates/gateway/app`.
  - **Both UIs.** At component ends that touch a UI, and at the final step, run `npm test`, `npm run typecheck` (`tsc --noEmit`), and `npm run build` (`node build.mjs`) in the touched UI: `crates/workshop/ui`, `crates/gateway/config-ui/ui`, or both (the scripts are at `package.json:11-14` in each). `npm test` runs `node --test` over `.mjs` files and does not typecheck the `.ts` sources, which is why typecheck is a separate step.
- Regression, security, and performance:
  - **Silent skips.** Some workspace symlink tests print a message and return when they can't create a symlink. Find them by grepping for `eprintln` in `crates/workshop/workspace/src/*-tests*.rs`. They must panic instead when the `CI` environment variable is set.
  - **Fixed sleeps.** Replace these with event-driven waits, or with `tokio::time::pause` where the code under test uses tokio timers:
    - `crates/workshop/server/tests/it/realtime_relay/overload.rs:19` (750 ms)
    - `crates/workshop/server/tests/it/chat_gate/lifecycle.rs:79` (a 150 ms quiet window)
    - `crates/workshop/server/tests/it/heartbeat_loop/startup_convergence.rs:148` (`TEST_INTERVAL * 4`)
    - `crates/workshop/shell/src/gateway/tests/recovery.rs:274` (a 5-second hang fixture)
  - Paused time already works in this codebase: `crates/workshop/support/src/deadline.rs` uses `start_paused`.
  - **Structural checks.** `cargo test -p build-xtask` checks the tier graph, the Invariants marker, the file-size ceiling, and the product boundaries. It builds quickly. Run it after any change to manifests, crate names, or file layout.
  - **Docs.** `docs-claims.mjs` keeps guarding root `AGENTS.md` and the remaining guide pages. `mdbook build guide` and `cargo run -p build-user-guide` confirm that the guide builds without its Workshop part.
- Exit criteria:
  - **Canonical gates**, the full verification. They run at the baseline and again at the final step, and nowhere else. Each must end at least as green as its baseline.
    - They are the canonical gates in `AGENTS.md`, exactly as the Project Survey records them: the full-suite test, linter, formatter-check, and docs commands, including the facade gates master added.
    - They also include `cargo test -p build-xtask`, `mdbook build guide`, and the three npm scripts in `crates/workshop/ui` and in `crates/gateway/config-ui/ui`.
    - At the final step, add `cargo run -p build-user-guide` (only the three remaining exports are written, unchanged) and `cargo workshop` (the full desktop build).
  - **Per-step checks** (minimal):
    - The step's own tests only: the survey's focused test pattern with a test-name filter, or the single `node --test` file.
    - `npm run typecheck` in a UI whose `.ts` files the step changed.
    - `cargo test -p build-xtask` when the step changes manifests, crate names, or file layout.
    - Mechanical steps with no behavior change add no tests. Their check is that the touched packages still compile and their existing focused tests pass.
    - No clippy, no full-crate suites, no `cargo workshop`, and no workspace-wide run at a step. The pre-commit hook runs `cargo fmt`.
  - **Component-end checks.** These cover the packages that component touched, and nothing wider:
    - the survey's component test commands
    - `cargo clippy --all-targets -- -D warnings` with one `-p` flag per touched package
    - `cargo fmt --all --check`
    - `npm test`, `npm run typecheck`, and `npm run build` in any UI the component touched
    - for a component that includes the directory move, also `cargo test -p build-xtask` and `cargo nextest run --locked -p gateway`, which covers the icon test
  - **Retired names.** A grep over the edit scope, excluding `vibe/`, returns nothing for any of these: `workshop/shell`, `Tier: shell`, `StatusBarShell`, `createStatusBarShell`, `ws-shell`, `mountLiveShell`, `showShell`, `WorkshopObserver`, "workbench socket", "lazy shell", "empty shell", "boot shell". It also finds no identifier `SHELL` in `crates/build-xtask/src`.
  - **Identifiers named `shell`.** No variable, parameter, or field named `shell` remains in `.rs`, `.ts`, or `.mjs` files in scope.
  - **Remaining "shell" hits.** Any left in `crates/workshop`, `crates/build-xtask`, `crates/shared-ui`, `crates/gateway/config-ui/ui/src`, or the root docs must mean a terminal command shell.

</verification-contract>
<decision-record>

## Decision Record

- Decisions:
  - **Retire "shell" for everything except terminal command shells.**
    - Rationale: the word has at least eight meanings across the workshop crates and UIs, and the stubbed Terminal menu will make it mean bash or PowerShell.
    - User: "maybe 'shell' should be terminology for terminals and we should use something else for Tauri".
  - **Call the Tauri crate "the desktop app", in `crates/workshop/desktop/`.**
    - Rationale: its Cargo description already says "desktop app", and prose already says "desktop shell". Every alternative collides with an existing name (see Rejected alternatives).
  - **Keep the package name `workshop` and the binary name `promptforge-workshop`.**
    - Rationale: these are user-facing release identifiers, and they were already renamed once, in commit 21493cb5 on 2026-09-02.
    - The user chose "Keep both names; rename only the directory and the prose".
  - **Rename the server's tier to "server".**
    - Rationale: the tier holds only `workshop-server`, and "root" is already overloaded. `AGENTS.md:29` uses "root" for the `crates/` public layer, and "composition root" and "repository root" are both in use.
    - The user chose "server: the tier holds only workshop-server, so name it after the crate".
  - **Adopt the UI vocabulary: desk, page, view, placeholder, and entry bundle. "workbench" keeps only its existing senses.**
    - Rationale for "desk": it has no existing uses in the two UI source trees or shared-ui.
    - "workbench" was rejected because it already means three things: `WorkbenchFrame` and `{"type":"workbench"}` on the wire, the `/ws` "workbench socket", and the VS Code mechanics described in `crates/workshop/ui/AGENTS.md`. Using it for the main frame would add senses to a word that is already overloaded.
    - Using "view" for the main frame would invert the hierarchy, since a view would then contain pages.
    - User: "how about settings-page, discover-page ?" After the review showed the collision, the user chose "desk" for the main frame.
  - **Name the `/ws` server module `workshop_socket`.**
    - Rationale: it pairs with the UI's client for that socket, `crates/workshop/ui/src/services/workshop-socket.ts`, the same way the agent socket pairs server `agents/socket.rs` with UI `agent-socket.ts`. It also avoids "workbench".
  - **Handle the save timeout in the client, as an unknown-token state, with an honest success criterion.**
    - Rationale: the blocking write can't be cancelled and may land after any re-read. No client-side re-fetch can guarantee that the next save won't conflict. The criterion is therefore that the editor surfaces the unknown state, never sends a stale token, and that a later save either succeeds or shows the existing conflict dialog.
    - The user chose the client-only design.
  - **Rename the gateway's `SwitchOutcome` to `SwitchProfileBody`.**
    - Rationale: the name describes the wire body, and it stops colliding with workshop-menu's `SwitchOutcome`.
  - **Verify minimally per step, and fully only at the baseline and the final step.**
    - Each step runs only its own focused tests. Component ends run the suites and lints of the touched packages. The full canonical gates (`AGENTS.md:51-57`) plus `cargo workshop` run only at the baseline and the final step.
    - Rationale: the full gates rebuild the whole workspace and the desktop app. Running them per step makes each step slow and adds little over focused tests plus per-component checks.
    - User: "47 steps is quite a lot. I want each step to go fast. minimal verification. just enough to make sure it works, I dont want a huge rebuilding or global test run. do a full verify where it counts".
  - **Delete the workshop's human docs and remove them from the guide build.**
    - Scope: the guide's Workshop chapters, their SUMMARY entries, the workshop export and its `build-user-guide` set, and the five workshop READMEs. The `AGENTS.md` agent rules, the third-party license notices, the `//!` crate docs that the build check requires, and the Cargo descriptions all stay. The docs-claims test drops only its workshop-export check.
    - Rationale: before beta, prose about a fast-moving product goes stale faster than anyone can maintain it.
    - User: "let's just delete all the workshop docs and remove them from the docs build. they are going to go stale very fast and keeping them up to date while the product is pre-beta is nothing but a tax on development. keep promptforge, gateway, and harness docs."
    - The user chose "Human docs only" (keep the `AGENTS.md` files).
  - **Remove the workshop lens from `tools/document.md`.**
    - Rationale: the tool would otherwise regenerate the deleted guide.
    - The user chose to remove it.
  - **Approve four small scope widenings that the steps need.** Each is confined to the named lines.
    - The hard-coded sidecar path in `tools/stage-gateway-sidecar.mjs`, its test, and `crates/build-workshop/tests/interruption.rs`. Without it, `cargo workshop` and CI can't find the sidecar after the directory move.
    - The two unit tests and the doc comment in `crates/build-user-guide/src/main.rs` that break when the workshop set leaves `SETS`.
    - The dead link to the Workshop part at `guide/src/introduction.md:27`.
    - Vocabulary wording in `.cursor/rules/workshop-architecture.mdc` and `.cursor/rules/workshop-spa.mdc`, which still say "shell" and "boot shell".
    - The user approved all four.
  - **Allow a conditional edit to config-ui's `gateway-api.ts` and `panel-bridge.ts`.** The edit is made only if the timeout audit finds they render the new 408 badly.
    - Rationale: the edit is small and made only if needed. `refusalDetail` already reads the envelope, so no edit is expected.
    - The user chose to add the exception.
  - **Plan the full cleanup, not just the rename.**
    - The user chose "The full cleanup sequence (all phases), with the rename as a step in phase 1".
  - **Keep promptforge, harness, and gateway out of scope, except for named exceptions.**
    - User: "this plan should also not touch promptforge, harness, or gateway".
    - Then: "break gateway's dependency on workshop/shell by just making copies of the icons and putting them in a gateway crate".
    - Then: "you can reanme createStatusBarShell , the blast radius in gateway would be quite minimal and master isn't touching gateway so its very safe".
    - Then: "I want config-ui's change in the plan".
  - **Run the plan directly on master in the promptforge repository.**
    - Rationale: master finished the engine consolidation (1fd82c62, "Close plan: debt removal api firewall"), so the plan runs on top of it. That removes the rebase and every conflict it would have caused.
    - User: "change the plan to @promptforge repo, and survey based on that".
    - This supersedes the earlier target, `vibe2` in the promptforge2 worktree with a rebase afterward (user: "when this plan finishes executing I plan to just rebase vibe2 on top of the completed master").
  - **Keep the registry's subsystem-named traits and fix its docs.**
    - Rationale: moving the traits into the crates that own them would create service-to-service dependencies, which the tier check forbids. A new interfaces crate would add a crate for nothing more than a rename.
    - The user chose "Keep the traits in workshop-registry; reword the 'never names a subsystem' claims and write down the real runtime links".
  - **Standardize subsystem handles on a named struct.**
    - Rationale: `handles.rs` currently takes four different shapes across five crates (user-state has none), and callers unpack unnamed tuples of guards by position (`crates/workshop/server/src/app.rs:344,381`).
    - The user chose "Every subsystem gets a handles.rs whose register returns a named struct of registration guards".
  - **Move `/prompts/contract` into the server.**
    - Rationale: it's a pure prompt parse that has nothing to do with the filesystem jail, and it's the workspace crate's only reason to depend on the engine runtime.
    - The user chose "Move it into workshop-server as a server-owned route; workspace drops the engine dependency".
  - **Keep the gateway client's cache API and drop only the server's re-exports.**
    - The user chose "Keep it for a planned caller; only drop the server's re-exports".
  - **Move the workshop UI service tokens into `services/`.**
    - Rationale: it makes the rule that parts depend on services literally true.
    - The user chose "Move the tokens and their interface types into ui/src/services; implementations stay in parts".
  - **Build the 408 body inside support, and pin its shape with a server test that compares JSON values against a serialized `ErrorEnvelope::new(message, code)`.**
    - Rationale: the workspace and user-state crates call `with_deadline`, and support can't depend on protocol. Comparing JSON values avoids adding `Deserialize` to a public protocol type.
    - The body change applies to every deadline-wrapped route, so every UI consumer of status or error codes is audited, not just the editor.
  - **Name the route-deadline wire code `deadline_elapsed`.**
    - The 408 body is `{"error":{"message":"...","code":"deadline_elapsed"}}` with `content-type: application/json`. The message names the elapsed deadline in seconds and says the operation may still complete, for example "the request did not finish within its 10s deadline; the operation may still complete".
    - `workshop-support` exports the code as a constant and the message builder, so the server's shape test uses the same source as the middleware.
    - Rationale: existing wire codes are lowercase snake_case names of the failure (`modified_conflict`, `gateway_unreachable`). This one matches `with_deadline` and its "request deadline elapsed" log line, and it says the server abandoned the response, which HTTP's "Request Timeout" (a slow client) does not. Both UIs key on the string, so it is a wire contract.
    - Added during step decomposition, where the plan had named only "a timeout-specific code". The user confirmed "Keep deadline_elapsed".
  - **Make the `AGENTS.md` pointer explicit.**
    - Each workshop `src/lib.rs` (for example `crates/workshop/support/src/lib.rs:10`) and the new-crate template (`crates/build-xtask/src/new_crate.rs:73`) say "Read `AGENTS.md` before adding an import." without saying which file. The sentence will name the repository-root `AGENTS.md`, plus the crate's own file for crates that have one.
    - Rationale: crate-level `AGENTS.md` files exist only in `crates/workshop/ui`, `crates/workshop/shell`, `crates/workshop/server`, and `crates/workshop/shell/icons`.
  - **Order the work through dependencies:**
    - Tests are made reliable before code is restructured.
    - The settled decisions come before the moves they shape.
    - The workshop docs are deleted early, before the vocabulary renames, so no step edits a file that is about to be deleted.
    - The remaining code-level doc fixes come last.
    - Rationale: a restructure needs a suite you can trust, and text that describes composition goes stale fastest, because the composition root changes most often: `crates/workshop/server/src/app.rs` had 29 commits and `crates/workshop/server/README.md` had 28 between 2026-09-02 and 2026-09-24, following renames.
  - **Every commit builds.** Moved files keep their content, apart from the minimal import or path fixes needed to build. Edits that wire up a move go in the same commit as the move. Identifier renames and other content edits go in separate commits.
    - Rationale: execution runs focused tests, a review, and periodic verification at every step, so a commit that doesn't build fails all three. Git's rename detection still works at high similarity, so `git blame --follow` keeps tracking the moves.
    - This applies to `views/` to `pages/` (moves plus identifier renames) and to `shell/` to `desktop/` (a move plus identifier and prose edits).
    - The user chose "Every commit builds", replacing the earlier pure-rename rule.
- Rejected alternatives:
  - **Running on `vibe2` in the promptforge2 worktree and rebasing onto master afterward.** This was superseded once master's consolidation finished, because running on master removes the rebase and its conflicts. Revisit: none.
  - **Renaming the Tauri package or binary.** It would churn release identifiers. Revisit if matching the other `workshop-*` package names becomes important.
  - **"app", "host", "window", or "launcher" for the Tauri crate.**
    - "app" collides with the server's `app.rs` and `AppState`, the gateway's `app` crate, and config-ui's `#app` document root.
    - "host" collides with `HostSnapshot` and with "embedding host" in the server docs.
    - "window" and "launcher" undersell the crate, which also supervises the gateway and runs the updater.
    - Revisit: none.
  - **"frame" for UI pieces.** It already means wire frames (`StatusFrame`, `WorkbenchFrame`) and the iframe that hosts config-ui. Revisit: none.
  - **"view", "layout", "screen", or "app" for config-ui's post-login frame.**
    - "view" inverts the hierarchy.
    - "layout" undersells a function that also starts data flows.
    - "screen" produces awkward names like `mountLiveScreen`.
    - "app" is already the document root (`#app`, `app.js`, `app.css`).
    - Revisit: none.
  - **Deferring the directory move because the gateway embeds its icon from the workshop directory.** Giving the gateway its own icon copies replaced this. Revisit: none.
  - **Deferring the shared-ui status bar rename.** Only two gateway files use the name, so the change is small. Revisit: none.
  - **Moving the registry's subsystem-named traits into their owning crates, or into a new interfaces crate.** The first is forbidden by the tier check. The second adds a crate just to rename. Revisit if the registry has to become a pure type map for some other reason.
  - **Only documenting the current `handles.rs` shapes.** Callers would still unpack tuples by position. Revisit: none.
  - **Putting `/prompts/contract` in a new crate, or keeping it in workspace.** The server already owns several routes, and a new crate adds overhead for a single route. Revisit if more prompt-related routes appear.
  - **Deleting the gateway cache API.** A caller is planned. Revisit if that caller is dropped.
  - **Only updating `crates/workshop/ui/AGENTS.md` to allow tokens under `parts/`.** The layering rule would stay aspirational. Revisit: none.
  - **Moving `with_deadline` into the server.** The workspace and user-state crates call it. Revisit: none.
  - **"workbench" for the UI main frame.** It was chosen at first, then dropped because the word already has three senses. Revisit: none.
  - **"scaffold" for the UI main frame.** It reads as code scaffolding. Revisit: none.
  - **"layout", "main", "frame", "chrome", or "console" for the UI main frame.** Each already has many uses across the UI trees and shared-ui: 122, 153, 165, 46, and 35 whole-word occurrences respectively. They mean dock arrangement, the main zone and `main.ts`, wire frames and iframes, window chrome, and the browser or OS console. Revisit: none.
  - **"root" or "composition" for the server's tier.** "root" is overloaded, and "composition" is less direct than naming the tier after its only crate. Revisit: none.
  - **A per-path lock in workshop-workspace, so that reads wait for in-flight writes.** It would remove the save race, but it adds concurrency machinery to the jail crate. Revisit if conflict dialogs after save timeouts turn out to be common.
  - **Adding `Deserialize` to `ErrorEnvelope`.** It would be a public protocol API change that the JSON-value comparison makes unnecessary. Revisit if a Rust client ever needs to parse envelopes.
  - **Adding crate-level `AGENTS.md` files everywhere, or deleting the pointer sentence.** An explicit reference is cheaper and removes the ambiguity. Revisit: none.
  - **Strictly pure-rename commits that don't build.** They would fail the focused tests, the review test runs, and periodic verification that execution runs at every step. Revisit: none.
  - **Keeping the workshop docs and fixing their drift.** Before beta, maintaining them costs more than they return. Revisit at beta.
  - **Deleting the workshop `AGENTS.md` files as well.** They hold rules that the code and this plan rely on, such as the icon sync rule and the UI layering rules. Revisit: none.
  - **Keeping the workshop lens in `tools/document.md`.** It would regenerate the deleted guide. Revisit at beta, together with the docs.
  - **Running the full gates at every step or component end.** They are slow and add little beyond focused tests and per-component checks. Revisit if a component end misses a regression that a full run would have caught.
- Assumptions, risks, and notes:
  - **Repository state.**
    - Master is at 1fd82c62, just after the engine consolidation. No plan is active (`vibe/ACTIVE` is absent).
    - The workshop crates depend on the `promptforge` facade.
    - The build-xtask `SHELL` constants are unchanged: `&["workshop-server"]` in `tidy.rs` and `"workshop"` in `product.rs`.
    - `crates/build-user-guide`, `crates/build-workshop`, and `.cursor/rules` are unchanged since commit 75245481.
  - **The bind gap** affects only the standalone `workshop-server` binary.
  - **The save-timeout bug** was inferred from the code: a task on tokio's blocking pool can't be cancelled. It has not been reproduced.
  - **Residual race, by design:** after a save times out, the late write can land after the editor re-reads the file. The worst case is the existing conflict dialog.
  - **Test-coverage statements** in this plan are static estimates. No coverage tool was run.
  - **Risk: busy files.** The restructure touches the most-changed files (`crates/workshop/server/src/app.rs` and `crates/workshop/server/src/lib.rs`). No other plan is active, so no concurrent workshop work competes for them.
  - **Risk: broad renames.** The mechanical renames are large: about 300 page references and 77 desk references in config-ui (a measurement found 287 "view" occurrences), and about 130 "shell" occurrences in the workshop UI (a measurement found 108). They can catch unrelated words. The exclusions are listed (`review`, `viewport`, `openReviewDiff`), and the Testing Plan's retired-name and identifier greps catch misses.
  - **Risk: duplicated icons.** The gateway's icon copies duplicate brand assets. The sync rule in `crates/workshop/shell/icons/AGENTS.md` contains this.
  - **Risk: no workshop user guide.** The published guide has no Workshop part until the docs are rewritten.
  - **Risk: late failures.** With full gates only at the baseline and the final step, a cross-package break can surface late. Component-end checks on the touched packages contain most of this, and the final step's full gates catch the rest.

### Deferred and Out of Scope

- **Deferred: the protocol crate's dependency on the engine.** It depends on the full `promptforge` facade, which was a deliberate design decision. Revisit if build times or drift in the wire format caused by engine types become a problem.
- **Deferred: cross-part imports in the workshop UI.** `STATUS_BAR` and `openInZone` act as hubs (for example `crates/workshop/ui/src/parts/run/run-panel.ts:28-31`). Revisit after the service tokens move into `services/`.
- **Deferred: workshop user documentation**, meaning the guide chapters and READMEs, and the `tools/document.md` workshop lens. Revisit at beta.
- **Out of scope:** edits to `crates/promptforge*`, to `crates/harness/*`, and to `crates/gateway/*` beyond the named exceptions.
- **Out of scope:** content edits to the guide's Gateway, Language, and Agent pages.
- **Out of scope:** renaming the Tauri package or binary.
- **Out of scope:** dated files under `vibe/`.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: None for focused and component verification, because their test commands compile only the touched packages. For full verification only, `cargo workshop` (alias for `run -p build-workshop --`, accepting `--release` and `--target <triple>`) builds the gateway, stages it as the Tauri sidecar, builds the desktop app, and removes the staged copy. Plain `cargo build` builds only the default member, the gateway (`crates/gateway/app`). `cargo build -p workshop` alone needs a pre-staged sidecar; CI stages one with `node tools/stage-gateway-sidecar.mjs stage --target <triple> --source target/debug/promptforge-gateway` (`.exe` on Windows) and removes it with `node tools/stage-gateway-sidecar.mjs remove --target <triple>`.
- Focused test command pattern: `cargo nextest run --locked -p <package> --all-features <test-name-filter>` for any main-partition package. For `workshop`, `workshop-server`, and `workshop-server-api`, drop `--all-features` to match CI (on `workshop-server` it would turn on `headless`): `cargo nextest run --locked -p <package> <test-name-filter>`. Workshop UI: `node --test crates/workshop/ui/test/<file>.mjs`. Gateway config UI: `node --test crates/gateway/config-ui/ui/src/<path>.test.mjs`. UI tests need `npm ci --prefix <ui dir>` first.
- Component test command pattern: main partition, `cargo nextest run --locked -p <package> --all-features` then `cargo test --locked -p <package> --all-features --doc`. Workshop partition, `cargo nextest run --locked -p <package>` then `cargo test --doc -p <package>`, plus `cargo nextest run --locked -p workshop-server --features headless` when touching `workshop-server`. When touching `crates/promptforge/` or anything it re-exports, also run the facade docs and surface gates listed under Docs command. UI: `npm test --prefix crates/workshop/ui` or `npm test --prefix crates/gateway/config-ui/ui`.
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`, then `cargo nextest run --locked -p workshop-server --features headless`, then `cargo test --doc -p workshop -p workshop-server -p workshop-server-api`, then `npm test --prefix crates/workshop/ui` and `npm test --prefix crates/gateway/config-ui/ui`. The boundary and structural harness `cargo test -p build-xtask` runs inside the workspace nextest pass and can be run alone. Its nightly-only fixtures are `#[ignore]`d and run only in CI's api-surface job: `cargo +<pinned nightly> nextest run --locked -p build-xtask --run-ignored only`.
- Linter command: `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings` and `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`, plus the headless gate `cargo check -p gateway --no-default-features`. UI typecheck: `npm run typecheck --prefix crates/workshop/ui` and `npm run typecheck --prefix crates/gateway/config-ui/ui`. Supply chain (CI, and pre-push when installed): `cargo deny check`; CI also runs `cargo audit` and a check that `ring` stays out of the gateway's normal dependency closure. Never run a standalone `cargo check --workspace` beside clippy. `clippy.toml` allows `unwrap` and `expect` in tests.
- Formatter check command: `cargo fmt --all --check` (rustfmt `style_edition = "2024"`; the pre-commit hook runs it). No TypeScript formatter is configured.
- Docs command: `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api` with `RUSTDOCFLAGS` set to `-D warnings` (PowerShell: `$env:RUSTDOCFLAGS="-D warnings"`), then the facade docs with default features, `cargo doc -p promptforge --no-deps`, under the same flags, and `mdbook build guide` for the user guide. Facade surface gate: `cargo +nightly-2026-09-05 xtask api --check` (the nightly is pinned in `crates/build-xtask/src/api/toolchain.rs`; the committed listing is `crates/promptforge/public-api.txt`). The CI docs job does not cover the three workshop-partition packages.
- Test placement and naming conventions:
  - Rust unit tests take two forms. The workshop, harness, promptforge-internal, and build-xtask crates mostly use a sibling file `<stem>-tests.rs`, wired at the bottom of the parent module with `#[cfg(test)]` and `#[path = "<stem>-tests.rs"] mod tests;` (about 150 such files). Further splits are `<stem>-tests-<label>.rs` (for example `workspace-tests-close.rs`), and a large group becomes a directory, `<module>/tests.rs` plus `<module>/tests/<seam>.rs` (for example `crates/workshop/gateway/src/gateway/tests/`). The gateway family, the VFS, the shared crates, and a scattering of workshop files use an inline `#[cfg(test)] mod tests { }` instead (about 185 files).
  - Integration tests compile into one binary per crate at `tests/it/main.rs`, one module per concern, with larger concerns split into `tests/it/<concern>/<seam>.rs`. Shared helpers live in `tests/common/mod.rs`, pulled in by `#[path = "../common/mod.rs"] mod common;` (workshop-server, gateway-stt), and data sits under `tests/fixtures/`. Exceptions: the `promptforge` facade uses `tests/suite/main.rs` with prompt fixtures in `tests/prompts/`, and build-workshop, gateway-cloud-providers, gateway-stt-engine, and gateway-stt-backend-whisper keep loose top-level `tests/*.rs` files.
  - Cross-crate test seams sit behind a `test-fixtures` feature (workshop and gateway families, gateway-api-discovery), `test-support` (promptforge-engine, promptforge-lua, promptforge-parser, harness-runner), or `test-helpers` (gateway-routing). The feature is usually switched on through a self dev-dependency, for example `workshop-server = { path = ".", features = ["test-fixtures"] }`.
  - Test function names are snake_case behavior sentences, for example `a_status_update_reaches_the_sink_at_info_severity`.
  - Workshop UI tests are `node --test` files in `crates/workshop/ui/test/<kebab-feature>.mjs` (96 files) with helpers and Tauri stubs in `test/helpers/`. Its npm script also globs `src/**/*.test.mjs`, but no such file exists today. The gateway config UI uses only colocated `src/**/*.test.mjs`.
  - Test files count toward the 500-line ceiling in marker crates, because the ceiling check reads every Rust file under the crate directory.
- Directory map:
  - `crates/` root: the public layer. `promptforge` (the facade: single-item re-exports grouped into documented role modules, with topic docs as `src/*.md` and the committed surface listing `public-api.txt`), `gateway-api-types`, `gateway-api-discovery`, `harness-api`, `shared-loopback`, `shared-error-source`, `shared-ui` (a TypeScript and CSS package, not a Rust crate), `workspace-hack` (cargo-hakari), and the `build-*` tooling crates (`build-xtask` boundary harness and `cargo xtask api` surface checker, `build-workshop` behind `cargo workshop`, `build-ui` esbuild helper, `build-user-guide`, `build-llama-cuda`).
  - `crates/promptforge-internal/`: private PromptForge family (`engine` is `promptforge-engine`, the sans-I/O executor; `types` is `promptforge-types`, the wire vocabulary; plus `lua`, `parser`, `store`, `vfs`, `model-client`).
  - `crates/gateway/`: private gateway family (`app` is package `gateway` with binary `promptforge-gateway`; `cloud-providers`, `config`, `config-ui` with its `ui/` TypeScript package, `local`, `logging`, `progress`, `protocol`, `routing`, `web-search`, and `stt/` holding `api` (package `gateway-stt`), `engine`, `backend-whisper`, `whisper-ffi`).
  - `crates/harness/`: private harness family (`runner`, `models`, `capabilities`, `log`, `sessions`, `web`, `webfetch`, `web-search`).
  - `crates/workshop/`: private workshop family (`shell` is package `workshop`, the Tauri desktop app with binary `promptforge-workshop`; `server`, `server-api`, `gateway`, `menu`, `protocol`, `registry`, `status`, `support`, `user-state`, `workspace`; and `ui/`, the SPA TypeScript package the server bundles, with `src/` split into `base/`, `services/`, `parts/`, `tokens/`, and `main.ts`).
  - `guide/`: mdbook user guides (`guide/book/` is ignored build output). `prompts/`: example PromptForge Markdown prompts. `images/`: README art.
  - `tools/`: Node scripts for gateway sidecar staging and a live TTS check, with `.test.mjs` files beside them that CI does not run.
  - `vibe/`: dated plan and design records, a few undated design notes, plus `archdoc.md` and `dependency-surface.md`.
  - `.github/workflows/`: CI (`ci.yml`), plus release, nightly, guide, installer smoke, STT Miri, whisper library, and llama CUDA pipelines. `.githooks/`: pre-commit runs fmt; pre-push runs the headless gateway check, main-partition clippy, and cargo-deny when installed. `.config/`: nextest profiles (a `heavy` test group throttles the STT suites) and hakari config. `.cargo/config.toml`: the `cargo workshop` and `cargo xtask` aliases, plus rust-lld and static CRT on Windows MSVC.
  - Root files: `AGENTS.md` (the repository policy), `rust-toolchain.toml` (stable), `rustfmt.toml`, `clippy.toml`, `deny.toml`, `dist-workspace.toml` (cargo-dist release config), and `gateway.local.example.toml`.
  - `target/`, `target-msrv/`, and `local/` (personal gateway config, profiles, and scratch prompts): ignored.
- Component boundaries:
  - Four product families (PromptForge, Gateway, Harness, Workshop) plus a shared layer. A crate inside a family container may depend only on `crates/` root crates and its own siblings. Nothing outside may depend into a container, except `promptforge` into `crates/promptforge-internal/` and `harness-api` into `crates/harness/`; inside the gateway, only `gateway-stt` is visible from outside `crates/gateway/stt/`. `build-*` crates are exempt from container privacy. The rules bind normal, dev, build, and target-specific dependencies, and `cargo test -p build-xtask` enforces them. One sanctioned exception: promptforge-internal crates may list `promptforge` under `[dev-dependencies]` only so doc examples compile against facade paths (engine, types, parser, store, and model-client do today).
  - PromptForge: promptforge-* depends on no gateway, workshop, or harness crate, and outsiders see only `promptforge`. Inside, `promptforge-engine` depends on lua, model-client, parser, store, types, and vfs; `promptforge-lua` on model-client, store, and types; `promptforge-parser` on lua and types; `promptforge-model-client` on types; `promptforge-store` on vfs; `promptforge-types` and `promptforge-vfs` on no family crate. The facade depends on all seven.
  - Gateway: gateway-* depends on no promptforge, workshop, or harness crate. Its public pair is `gateway-api-types` and `gateway-api-discovery` (which depends on `shared-error-source`).
  - Harness: harness-* may depend on `promptforge`, the gateway public pair, and shared-*, never on workshop crates or private gateway crates. Today every harness crate names `promptforge`, `harness-log` also names `shared-error-source`, and none names a gateway crate (the gateway binding arrives as data). Inside, `harness-api` depends on runner and sessions; `harness-sessions` on capabilities, log, models, runner, and web; `harness-runner` on capabilities and log; `harness-models` on runner; `harness-web` on capabilities, webfetch, and web-search; `harness-webfetch` and `harness-web-search` on capabilities.
  - Workshop: workshop-* may depend on the gateway public pair, `promptforge`, and `harness-api`, never on private gateway or harness crates.
  - shared-* depends on no product crate.
  - Inside workshop, dependencies flow shell -> features -> services -> vocabulary: `workshop` (shell) -> `workshop-server-api` (re-exports only) -> `workshop-server` -> the subsystems. The shell depends on `workshop-server-api` and `gateway-api-discovery`, never on `workshop-server` directly. `workshop-server` depends on all eight siblings plus `harness-api`, `promptforge`, `shared-loopback`, and `gateway-api-discovery`, with `build-ui` as a build dependency. `workshop-gateway` depends on protocol, registry, support, `promptforge`, and the gateway public pair. `workshop-menu` and `workshop-status` depend on protocol, registry, and support. `workshop-user-state` depends on those three plus `shared-error-source`, and `workshop-workspace` on those three plus `shared-error-source` and `promptforge`. `workshop-registry` depends on protocol. `workshop-protocol` depends on `promptforge`. `workshop-support` has no workspace dependencies.
  - Workshop UI imports flow `parts/` -> `services/` -> `base/`; `main.ts` is the composition root; `shared-ui` is consumed by both UIs.
  - The archdoc lists a CLI component, but no CLI crate or binary exists in the workspace.
- Conventions summary:
  - Rust 2024 edition, resolver 3, stable toolchain. Members set `[lints] workspace = true` and depend on `workspace-hack`; `promptforge-vfs` has no dependencies at all. Workspace lints forbid `unsafe_code`, warn on `missing_docs`, `missing_debug_implementations`, and `unreachable_pub`, deny broken and private intra-doc links, and deny clippy `all`, `pedantic`, `unwrap_used`, and `expect_used` (`doc_markdown` allowed). The four crates that own unsafe code (`workshop` shell, `gateway` app, `gateway-api-discovery`, `gateway-whisper-ffi`) copy the table instead, with `unsafe_code = "deny"` so one module can `#[expect(unsafe_code)]`, and clippy `pedantic` at warn (still fatal under `-D warnings`).
  - Third-party dependencies are declared once in root `[workspace.dependencies]`, each pin or feature choice explained by a comment; members use `.workspace = true`. Every workshop crate sets `publish = false`; `workshop` and `workshop-server` inherit 0.3.0, the other nine sit at 0.0.0, and the rest of the workspace inherits 0.3.0.
  - Every workshop-* and harness-* `lib.rs`, plus `harness-api`, opens with a `//!` doc holding a `## Invariants` section naming what the crate may and may not depend on. Rust files in those crates stay at or under 500 lines (the `workshop` shell is exempt from both). Several workshop files already sit within 20 lines of the ceiling (`server/src/app.rs`, `server/src/agents/socket.rs`, `server/tests/it/heartbeat_loop.rs`, `workspace/src/workspace.rs`, `workspace/src/workspace_file.rs`).
  - Source directories are flat: groups under three files become kebab siblings `parent-label.rs` wired with `#[path = "..."]`; groups of three or more become a standard subdirectory. Convert whichever way applies when touching a group. `workshop-workspace` does not follow this today: its `src/` is flat, with sibling groups of three or more (`workspace-*.rs`, `handlers-*.rs`, `workspace_file-*.rs`).
  - Features gate only real constraints or test seams (`test-fixtures`, `test-support`, `test-helpers`, `headless`, gateway `local`/`stt`/`web-search`/`config-ui`).
  - Library and serve paths return errors instead of exiting or installing process-global state. Unsafe stays in owned boundaries with a `// SAFETY:` comment on the preceding line; in workshop only `crates/workshop/shell/src/bridge.rs` has unsafe code, admitted by an `#[expect(unsafe_code, reason = ...)]` on its module declaration in `main.rs`.
  - Error and status messages target model consumption: concise, self-contained, naming required versus actual.
  - Comments explain only non-obvious constraints; platform and external-bug workarounds cite the upstream issue URL.
  - No new structural enforcement (source parsers, allowlists, counts, ceilings, topology checks) without explicit user approval. Behavior changes ship with tests in the same change, and refactors preserve behavior tests.
  - Run-log JSON round-trips exactly: sorted keys, finite numbers, `float_roundtrip`, never `preserve_order`.
  - Workshop server subsystems register handles into `workshop-registry` at boot and never receive another subsystem's handles through a constructor; missing optional contributions degrade to no-ops, never per-request panics.
  - SPA CSS sits beside its TypeScript in self-contained feature directories, uses only `--ws-*` tokens, and never touches `localStorage`; persisted values go through the `ui-storage` adapter to `/user/state` or `/workspace/file/state`. Commands and context keys reuse VS Code ids verbatim and register through `registerAction`.
  - Commit subjects are short imperative sentences, one change per commit (for example "Split src/execute/tests.rs into seam modules"). A finished plan ends with a `Close plan: <plan name>` commit.

</project-survey>
<execution-plan>

## Execution Instructions

Rules for every step:

- Each step is one commit with the subject given in its Commit line, holding only that step's code, tests, and plan marks.
  - Step 1 changes no code. Its baseline results and completion mark go into the active plan and fold into the plan seed commit.
  - Step 34 changes code only if an exit check needs a fix. Otherwise its commit holds the exit results and its completion mark.
- **History shape.** Every commit builds and passes its focused tests. A step that moves files uses `git mv`, and in the same commit makes only the edits that wire up the move (`mod` lines, `#[path]` attributes, imports, path strings) plus the minimal import or path fixes inside the moved files. Identifier renames and other content edits get their own steps. Before committing a move, `git diff --cached -M --name-status` must list every moved file as an `R` entry, never as a delete-and-add pair.
- **Checks.** A step runs only its Tests line. The last step of a component also runs its Component end line. No clippy, no full-crate suite, no `cargo workshop`, and no workspace-wide run at a step; the pre-commit hook runs `cargo fmt`. A step is done when its Tests line passes and nothing is worse than Step 1's baseline. `cargo nextest run --no-run` is the compile check for a mechanical step. Main-partition packages take `--all-features` unless a step's command says otherwise; `workshop`, `workshop-server`, and `workshop-server-api` never take it (Project Survey).
- **Sidecar.** Building the `workshop` package needs the gateway sidecar that Step 1 stages. It stays staged, and gitignored, until Step 34.
- **Ceiling guard.** Before editing a Rust file in a `workshop-*` crate, count its physical lines, and split it first if the edit would take it past 500. The files near the limit on master: `crates/workshop/server/src/agents/socket.rs` (492, relieved by Step 21), `crates/workshop/server/src/app.rs` (489, relieved by Step 20), `crates/workshop/workspace/src/workspace-tests.rs` (475), `crates/workshop/workspace/src/workspace.rs` (493), `crates/workshop/workspace/src/workspace_file.rs` (494), and `crates/workshop/server/tests/it/heartbeat_loop.rs` (495).
- **Layout convention.** A new file that gives a module a third hyphenated sibling turns the group into a directory in standard layout (`AGENTS.md:64`).
- **Scope.** Stay inside the Constraints' edit scope and named exceptions. For any edit outside them, stop and report.
- **Line numbers.** See Constraints, "Line numbers", for which citations were re-verified on master. Locate code by its content. Paths under `crates/workshop/shell/` become `crates/workshop/desktop/` from Step 13 on.
- When a step is done, append ` [completed]` to its heading and leave its tags unchanged.

Components, in dependency order:

1. **Baseline** (Step 1). Every later check compares against it.
2. **Workshop docs removal** (Step 2). It depends only on the baseline. It goes first so no later step edits a file that is about to be deleted, which is why the Decision Record deletes the docs before the vocabulary renames. One piece.
3. **Trustworthy tests** (Steps 3-6): flaky-tests, security-tests, wire-fixture. Before any code change, so every later "no worse than baseline" check runs on a suite with no silent skips or timing races, and the `/ws` frames are pinned before the socket module moves. The pieces are joint: they touch different files, and none consumes another's output. Step 3 joins the workspace halves of flaky-tests and security-tests, because the junction case uses the CI skip helper and one test set covers both.
4. **Behavior fixes** (Steps 7-10): bind, save-timeout. These are the only user-visible changes. They need only the trusted suite, and landing them before the mass renames means the renames include the fixed code. The two pieces are joint (different files). Inside save-timeout the steps are sequential: the UI rendering reads the server's new body, and the editor reacts to the UI's `deadline_elapsed` error.
5. **Dead code** (Step 11). Before the vocabulary work, so nothing about to be deleted gets renamed. One step covers the gateway, status, and server deletions, because one compile-and-test check covers all three.
6. **Shell vocabulary** (Steps 12-18): shell-rename, config-ui. It settles the names and the desktop path before the restructure moves files. The pieces:
   - icon copies come before the directory move, because the gateway build reads the icon from the old directory
   - the directory move comes before every rename piece, so the renames edit files at their final paths
   - Rust and workflow names and UI names are joint: they touch different files
   - UI names come before config-ui, because config-ui's `components/status-bar.ts` uses "shell" for both the status bar and the frame
   - inside config-ui, the file move comes before the identifier renames (History shape)
   - docs come last, so they describe settled names
7. **Structural consolidation** (Steps 19-32): split, prompts-route, helpers, layout, renames, and workshop UI structure. It needs the trusted suite and the settled names. The pieces are sequential:
   - split comes first. The Constraints require splitting `app.rs` and `socket.rs` before the edits that grow them (the prompts route, the alias removal, and the named handles), and none of the split's new files moves in a later step. Its steps are joint, except that the compose extraction follows the app directory move.
   - prompts-route comes before layout, so `handlers-prompts.rs` moves once.
   - helpers come before layout, so the layout moves include the final content.
   - layout comes before renames, so the renames edit files at their final paths and names.
   - Inside renames, the alias removal comes before the named handles (both edit `app/compose.rs`), and the socket move comes before the step that rewords the socket.
   - workshop UI structure touches only UI files and depends on no Rust piece. It comes last only to keep the Rust steps contiguous.
8. **Code-level docs** (Step 33): registry-docs and the rest of docs. Last, because text that describes composition goes stale fastest (Decision Record).
9. **Exit** (Step 34). The full verification, after every change.

<step-1>

### Step 1: Record the baseline [completed]

- Component: Baseline
- Piece: baseline
- Confirm three things: the repository is `C:\Users\Vinnie\cursor\promptforge` on branch `master`; 1fd82c62 is HEAD or an ancestor of it (the plan seed commit may sit on top); and `git status` is clean.
- Seed the plan: copy this plan file (frontmatter included) to `vibe/2026-09-24-2-workshop-crates-cleanup.md` (the execution date's next free dated-record name), and write that path into `vibe/ACTIVE`. Both join this step's commit.
- Run `cargo workshop` first. It builds the gateway, stages its own sidecar, builds the desktop app, and removes the staged copy.
- Stage the sidecar for the plan's `workshop` package runs, the way `.github/workflows/ci.yml:171-175` does: `cargo build --locked -p gateway --no-default-features`, then `node tools/stage-gateway-sidecar.mjs stage --target x86_64-pc-windows-msvc --source target/debug/promptforge-gateway.exe`. Leave it staged until Step 34.
- Run every canonical gate from the Testing Plan exit criteria, in order, plus the survey's two workshop-partition extras: `cargo nextest run --locked -p workshop-server --features headless` and `cargo test --doc -p workshop -p workshop-server -p workshop-server-api`.
- Run `cargo run -p build-user-guide` and record whether it changed any tracked file under `guide/`. If it did, list the files, then restore them with `git checkout -- guide` so the tree stays unchanged.
- Rerun each failing command once. A test that fails on only one of the two runs is intermittent.
- Record the results in a `Baseline results` list inside this step of the active plan (the repository copy under `vibe/`). Use one line per command with pass or fail, plus the names of the failing and intermittent tests.
- Baseline results (all pass; no failing or intermittent tests):
  - `cargo workshop`: pass (builds the gateway, stages its sidecar, builds the desktop app, removes the staged copy)
  - `cargo build --locked -p gateway --no-default-features` then `node tools/stage-gateway-sidecar.mjs stage --target x86_64-pc-windows-msvc --source target/debug/promptforge-gateway.exe`: pass (sidecar left staged)
  - `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`: pass (3899 passed, 54 skipped, 1 leaky)
  - `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`: pass
  - `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`: pass (234 passed, 4 skipped)
  - `cargo nextest run --locked -p workshop-server --features headless`: pass (130 passed, 2 skipped)
  - `cargo test --doc -p workshop -p workshop-server -p workshop-server-api`: pass
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
  - workshop UI `npm run build`, `npm test` (136 passed), `npm run typecheck`: pass
  - gateway config UI `npm run build`, `npm test` (178 passed), `npm run typecheck`: pass
  - Note: the workshop UI `npm test` boots the workbench from `dist/`, so it needs `npm run build` first; the recorded order is build then test.
- Tests: none added. The recorded list is the comparison point for every later step.
- Commit: "Seed the workshop crates cleanup plan", holding the `vibe/` plan copy, `vibe/ACTIVE`, and the recorded baseline results.

</step-1>

<step-2>

### Step 2: Delete the workshop's human docs [completed]

- Component: Workshop docs removal
- Piece: docs removal
- Remove with `git rm`: `guide/src/workshop/` (the index and chapters 01 through 11), the export `guide/promptforge-workshop-guide.md`, and the five READMEs `crates/workshop/README.md`, `crates/workshop/server/README.md`, `crates/workshop/shell/README.md`, `crates/workshop/user-state/README.md`, and `crates/workshop/workspace/README.md`.
- Keep `crates/workshop/server/AGENTS.md`, `crates/workshop/shell/AGENTS.md`, `crates/workshop/shell/icons/AGENTS.md`, `crates/workshop/ui/AGENTS.md`, `crates/workshop/ui/THIRD_PARTY_NOTICES.md`, every `//!` crate doc, and every Cargo description.
- Remove any Cargo `readme` key, `include_str!`, or link that names a deleted file. A planning-time grep found no `readme` key or `include_str!`. Check the remaining `AGENTS.md` files and root docs for links with `rg -n "README|guide/src/workshop|workshop-guide" crates/workshop AGENTS.md README.md tools/document.md`.
- In `crates/build-user-guide/src/main.rs`, remove `("workshop", "The Workshop")` from `SETS`, and make its doc comment say three sets instead of four. Two unit tests in the same file assert the removed set: `summary_has_parts_in_audience_order` looks for "# The Workshop", and `assembly_is_deterministic` reads `promptforge-workshop-guide.md`. Point them at the gateway part and `promptforge-gateway-guide.md`, and keep the audience-order check over the three remaining parts. These two edits are the least the `SETS` change needs to keep the crate's tests green.
- Regenerate with `cargo run -p build-user-guide`. The crate owns `guide/src/SUMMARY.md` and each part's `index.md`, so never hand-edit them. The new `SUMMARY.md` loses only the Workshop part (lines 4-17 today). If the run changes any gateway, language, or agent file, stop and report instead of committing it.
- In `crates/workshop/ui/test/docs-claims.mjs`, delete the test "the tracked guide export matches the sources on the stale claims" (the workshop export check, around lines 62-73), and reword the comment at line 48 that says one list guards both the sources and the export. The root `AGENTS.md` and `guide/src` checks stay.
- In `tools/document.md`, remove the workshop lens (the part that writes `guide/src/workshop/` and targets the workshop crates at line 105) and any list entry that names the workshop set, so the tool can't regenerate the deleted guide.
- In `guide/src/introduction.md`, remove the link to the deleted `workshop/index.md` at line 27, and the wording that introduces the Workshop part. Change nothing else on the page. This edit is approved in the Decision Record.
- Tests: `cargo nextest run --locked -p build-user-guide --all-features`, `node --test crates/workshop/ui/test/docs-claims.mjs`, and `mdbook build guide` pass. `git status` shows no change to the gateway, language, or agent exports or index files. Over the edit scope, excluding `vibe/`, `rg "promptforge-workshop-guide|guide/src/workshop"` finds nothing.
- Component end: `cargo clippy -p build-user-guide --all-targets --all-features -- -D warnings` (the crate is a binary, so it has no doc tests), `cargo fmt --all --check`, and `npm test`, `npm run typecheck`, and `npm run build` in `crates/workshop/ui`.
- Commit: "Delete the workshop's human docs and drop them from the guide build"

</step-2>

<step-3>

### Step 3: Fail the symlink tests under CI and cover the jail's edge cases [completed]

- Component: Trustworthy tests
- Piece: flaky-tests and security-tests, the workspace half of each
- Check recent CI logs for the skip message "skipping: symlink creation failed" (`gh run list`, then `gh run view <id> --log`). If a CI runner already hits the skip, this step would turn that job red: stop and report.
- Add `crates/workshop/workspace/src/workspace-tests-jail.rs`, declared in `workspace-tests.rs` beside `backing`, `grants`, `pointer`, and `ui_state` as `#[path = "workspace-tests-jail.rs"] mod jail;`. `workspace-tests.rs` is at 475 lines, so the new code goes in the new file.
- In it, add `symlink_unavailable(ci: bool, reason: &str)`. It panics when `ci` is true, and otherwise prints the reason so the caller can return. The two skip sites in `workspace-tests.rs` (lines 143 and 167, `eprintln!("skipping: symlink creation failed")`) call it with `std::env::var_os("CI").is_some()`. Passing the flag keeps the tests free of `std::env::set_var`, which is `unsafe` in Rust 2024 and forbidden here.
- Also in it, pin today's behavior of the confinement code (`crates/workshop/workspace/src/workspace-confine.rs`) for:
  - UNC (`\\server\share\...`) and verbatim (`\\?\C:\...`) spellings of paths inside and outside a granted root
  - case-only respellings of a granted root
  - a Windows directory junction inside a granted root that points outside it, created with `cmd /C mklink /J` (no new dependency)
- Gate the Windows-only cases with `#[cfg(windows)]`. A junction that can't be created goes through `symlink_unavailable`.
- If a case shows a path escaping the jail, stop and report it. The plan keeps jail behavior unchanged, so an escape is a security finding for the user, not something to pin.
- Tests: a `#[should_panic]` test for the helper with the flag set, a test that it returns normally without the flag, and the jail cases. `cargo nextest run --locked -p workshop-workspace --all-features workspace::tests` passes, and `rg eprintln crates/workshop/workspace/src -g "*-tests*.rs"` finds only the helper.
- Commit: "Fail the symlink tests under CI and cover the jail's edge cases"

</step-3>

<step-4>

### Step 4: Replace fixed sleeps with event-driven waits [completed]

- Component: Trustworthy tests
- Piece: flaky-tests
- Replace each fixed wait with a wait on the event it stands in for, or with `tokio::time::pause` or `#[tokio::test(start_paused = true)]` where the code under test uses tokio timers (`crates/workshop/support/src/deadline.rs` shows the pattern):
  - `crates/workshop/server/tests/it/realtime_relay/overload.rs:19` (750 ms)
  - `crates/workshop/server/tests/it/chat_gate/lifecycle.rs:79` (a 150 ms quiet window)
  - `crates/workshop/server/tests/it/heartbeat_loop/startup_convergence.rs:148` (`TEST_INTERVAL * 4`)
  - `crates/workshop/shell/src/gateway/tests/recovery.rs:274` (a 5-second hang fixture). The supervisor runs on threads with std time, so use a gate the test releases instead of paused time.
- A quiet-window assertion ("nothing arrives") keeps a bound, but the bound becomes a paused-time advance or an explicit end-of-stream signal, not a wall-clock sleep.
- Tests: the four tests assert what they asserted before and pass five runs in a row: `cargo nextest run --locked -p workshop-server realtime_relay::overload chat_gate::lifecycle heartbeat_loop::startup_convergence` and `cargo nextest run --locked -p workshop gateway::tests::recovery`.
- Commit: "Replace fixed sleeps in workshop tests with event-driven waits"

</step-4>

<step-5>

### Step 5: Pin the realtime relay refusals [completed]

- Component: Trustworthy tests
- Piece: security-tests
- Add `crates/workshop/server/src/routes/realtime-tests.rs`, wired at the bottom of `crates/workshop/server/src/routes/realtime.rs` with `#[cfg(test)]` and `#[path = "realtime-tests.rs"] mod tests;`.
- Pin today's behavior with no production change: an upgrade whose `Origin` is outside the allowed loopback origins is refused, and an upgrade without the required subprotocol is refused. Assert the status and body that each refusal answers today.
- Tests: `cargo nextest run --locked -p workshop-server routes::realtime` passes.
- Commit: "Add unit tests for the realtime relay's refusals"

</step-5>

<step-6>

### Step 6: Pin the /ws frames in a shared fixture [completed]

- Component: Trustworthy tests
- Piece: wire-fixture
- Add `SelectModelFrame`, the inbound `{"type":"select_model","model":"..."}` frame, to `crates/workshop/protocol/src/menu.rs` beside `SwitchProfileFrame`, with the same derives, and re-export it from `crates/workshop/protocol/src/lib.rs` next to `SwitchProfileFrame`.
- In `crates/workshop/server/src/agents/session-menu.rs`, parse `select_model` through `SelectModelFrame`, the way `switch_profile` parses through `SwitchProfileFrame`. Keep the refusal text ("select_model needs a \"model\" string") so `crates/workshop/server/tests/it/session/menu.rs` passes unchanged.
- Add a matching `SelectModelFrame` interface to `crates/workshop/ui/src/services/protocol.ts`, and type the frame sent at `crates/workshop/ui/src/services/workshop-socket.ts:184` with it.
- Add `crates/workshop/protocol/tests/fixtures/workshop-frames.json`, shaped like `agent-frames.json`. It covers the status, models, workbench, error, and switch_profile frames, plus select_model so the new type is pinned too.
- Assert the fixture on both sides: a new `workshop_frames` module in `crates/workshop/protocol/tests/it/` (outbound frames serialize equal to the fixture, inbound frames deserialize from it), and a new `crates/workshop/ui/test/workshop-wire-fixtures.mjs` modeled on `agent-wire-fixtures.mjs`.
- Tests: `cargo nextest run --locked -p workshop-protocol --all-features workshop_frames`, `cargo nextest run --locked -p workshop-server session::menu`, `node --test crates/workshop/ui/test/workshop-wire-fixtures.mjs`, and `npm run typecheck --prefix crates/workshop/ui` pass.
- Component end, for the packages Steps 3-6 touched:
  - `cargo nextest run --locked -p workshop-workspace -p workshop-protocol --all-features`, then `cargo test --locked -p workshop-workspace -p workshop-protocol --all-features --doc`
  - `cargo nextest run --locked -p workshop -p workshop-server`, `cargo nextest run --locked -p workshop-server --features headless`, and `cargo test --doc -p workshop -p workshop-server`
  - `cargo clippy -p workshop-workspace -p workshop-protocol --all-targets --all-features -- -D warnings` and `cargo clippy -p workshop -p workshop-server --all-targets -- -D warnings`
  - `cargo fmt --all --check`
  - `npm test`, `npm run typecheck`, and `npm run build` in `crates/workshop/ui`
- Commit: "Pin the /ws frames in a fixture shared by Rust and TypeScript"

</step-6>

<step-7>

### Step 7: Refuse non-loopback binds [completed]

- Component: Behavior fixes
- Piece: bind
- In `crates/workshop/server/src/serve.rs`, `reuse_bind` returns `std::io::Error::new(std::io::ErrorKind::InvalidInput, ...)` when the parsed `SocketAddr` has `!addr.ip().is_loopback()`, before it creates any socket. The message names the refused address and the loopback requirement.
- Tests: in `crates/workshop/server/src/serve-tests.rs`, `0.0.0.0:0`, `[::]:0`, and a LAN address such as `192.168.1.10:0` are refused with `InvalidInput`; `127.0.0.1:0` binds; `[::1]:0` is not refused with `InvalidInput` (a runner without IPv6 may fail that bind for another reason). `cargo nextest run --locked -p workshop-server serve::tests` passes.
- Commit: "Refuse non-loopback addresses in reuse_bind"

</step-7>

<step-8>

### Step 8: Answer the route deadline with a JSON 408 [completed]

- Component: Behavior fixes
- Piece: save-timeout
- Write the reproducing test first, in a new `save_timeout` module under `crates/workshop/server/tests/it/`: a `PUT /workspace/file` whose write outlasts the route deadline gets a 408 whose body parses as the JSON envelope, and after the stall is released the write still lands on disk. Run it against the unfixed code and confirm it fails before changing `deadline.rs`.
  - Stall the write deterministically through a seam behind the workspace crate's `test-fixtures` feature, released by the test. Add the feature if it's missing, and enable it in the server's dev-dependency on `workshop-workspace`.
  - Keep the test off the 10-second wall clock with a test-only deadline or paused time. Add no production knob.
- In `crates/workshop/support/src/deadline.rs`, answer an elapsed deadline with status 408, `content-type: application/json`, and the body `{"error":{"message":"...","code":"deadline_elapsed"}}`, built with `serde_json` (add it to support's dependencies if it's missing) because support can't depend on protocol. Export the code as a constant and the message builder (which takes the `Duration`) from `workshop-support`. The Decision Record fixes the code and the message.
- Add the shape test to the `save_timeout` module: parse the body as a `serde_json::Value` and compare it with `serde_json::to_value(workshop_protocol::ErrorEnvelope::new(message, code))`, using the support exports.
- Extend `a_stalled_route_answers_408_at_its_deadline` in `deadline.rs` to check the content type and the parsed body.
- Tests: `cargo nextest run --locked -p workshop-support --all-features deadline` and `cargo nextest run --locked -p workshop-server save_timeout` pass. If the workspace crate gained a seam, `cargo nextest run --locked -p workshop-workspace --all-features handlers` passes too. If a manifest changed, `cargo test -p build-xtask` passes.
- Commit: "Answer the route deadline with a JSON error envelope"

</step-8>

<step-9>

### Step 9: Render route timeouts readably in the UIs [completed]

- Component: Behavior fixes
- Piece: save-timeout
- In `crates/workshop/ui/src/services/json-request.ts` (lines 17-56), a 408 with the JSON envelope yields the envelope's message and the `deadline_elapsed` code, and a 408 with an empty or non-JSON body yields a readable timeout error. Neither path reports that the server "returned a non-JSON answer".
- In `crates/workshop/ui/src/services/error-catalog.ts` (lines 17-46), add `deadline_elapsed` if the catalog maps codes to messages.
- Audit the other status and code readers under `crates/workshop/ui/src/services/`: `workspace-api.ts:91-96,159-189`, `workspace-file-client.ts:104-114`, and `run-api.ts:263-267`. Fix any that would mishandle the new body.
- config-ui: `refusalDetail` in `crates/gateway/config-ui/ui/src/services/gateway-api.ts` (lines 349-363 and 1069-1101) already reads the envelope's `message` and `code`, and `panel-bridge.ts:206-220` passes relay answers through. Confirm that a 408 envelope from the gateway-config relay shows its message. These two files are a named exception only for this case: edit them only if the audit finds that they render the 408 badly, and then add a config-ui test for the fix.
- Tests: a new `crates/workshop/ui/test/json-request-timeout.mjs` covers the JSON 408 and the empty 408. It and `npm run typecheck --prefix crates/workshop/ui` pass. If config-ui changed, its new test and `npm run typecheck --prefix crates/gateway/config-ui/ui` pass.
- Commit: "Render route timeouts as readable errors in the UIs"

</step-9>

<step-10>

### Step 10: Track an unknown save token in the editor [completed]

- Component: Behavior fixes
- Piece: save-timeout
- In `crates/workshop/ui/src/parts/editor/editor-panel.ts`, add an "unknown" token state beside the known token that lines 189-190 set from `written.token`:
  - A 408 on save (the `deadline_elapsed` error from Step 9) sets the token to unknown and tells the user the save may or may not have landed.
  - While the token is unknown, the next save first re-reads the file. If the disk content matches what was last sent, it adopts the returned token and saves with it. Otherwise it shows the existing conflict dialog.
  - The editor never sends a stale token: `crates/workshop/ui/src/services/workspace-api.ts:189` only ever receives a token the editor currently knows.
- Tests: a new `crates/workshop/ui/test/editor-save-timeout.mjs` covers four cases. A 408 leaves the token unknown and sends no stale token. A disk match adopts the new token and saves. A mismatch shows the conflict dialog. A late write that lands after the re-read ends in the conflict dialog, not a raw error. It and `npm run typecheck --prefix crates/workshop/ui` pass.
- Component end, for the packages Steps 7-10 touched:
  - `cargo nextest run --locked -p workshop-support --all-features` and `cargo test --locked -p workshop-support --all-features --doc`, plus the same pair for `workshop-workspace` if Step 8 gave it a seam
  - `cargo nextest run --locked -p workshop-server`, `cargo nextest run --locked -p workshop-server --features headless`, and `cargo test --doc -p workshop-server`
  - `cargo clippy -p workshop-support --all-targets --all-features -- -D warnings` (with `-p workshop-workspace` if Step 8 touched it) and `cargo clippy -p workshop-server --all-targets -- -D warnings`
  - `cargo fmt --all --check`
  - `npm test`, `npm run typecheck`, and `npm run build` in `crates/workshop/ui`, and in `crates/gateway/config-ui/ui` if Step 9 changed it
- Commit: "Recover from a timed-out save through an unknown token state"

</step-10>

<step-11>

### Step 11: Delete the dead gateway, status, and server code

- Component: Dead code
- Piece: dead code
- Gateway: remove `crates/workshop/gateway/src/observer.rs` and `observer-tests.rs` with `git rm`, and remove `pub mod observer` and the `WorkshopObserver` re-export from `crates/workshop/gateway/src/lib.rs`. Confirm with `rg -w promptforge crates/workshop/gateway/src` that only `observer.rs` and `observer-tests.rs` name the engine crate. Then remove `promptforge` from `crates/workshop/gateway/Cargo.toml` (line 21), drop "the run event log" from its `description` (line 9), and drop the engine crate from the gateway's `## Invariants` block if it names it. The public cache API (`cache_ensure`, `CacheEvent`, `CacheResponse`, `SsePayloadStream`) stays for a planned caller.
- Server: in `crates/workshop/server/src/lib.rs`, remove `observer` from the alias list, and remove `CacheEvent`, `CacheResponse`, and `SsePayloadStream` from the `gateway` re-export (lines 93-96). Confirm that `crates/workshop/server-api/src/lib.rs` re-exports none of them.
- Status: confirm with `rg` that only their own tests call `StatusBus::report`, `info`, `debug`, `error`, and `idle` (`crates/workshop/status/src/status.rs:67-117`); producers use `Push`. Delete the five methods and those tests.
- Stale comment: in the module doc at `crates/workshop/server/src/agents/status.rs:1-4`, drop the history about the deleted sessions crate. Leave the "shell" and "relay" wording for Steps 14 and 30.
- Tests: `cargo nextest run --locked -p workshop-gateway -p workshop-status --all-features --no-run`, `cargo nextest run --locked -p workshop-status --all-features status`, `cargo nextest run --locked -p workshop-server -p workshop-server-api --no-run`, and `cargo test -p build-xtask` pass. `rg WorkshopObserver crates/workshop` finds nothing.
- Component end:
  - `cargo nextest run --locked -p workshop-gateway -p workshop-status --all-features`, then `cargo test --locked -p workshop-gateway -p workshop-status --all-features --doc`
  - `cargo nextest run --locked -p workshop-server -p workshop-server-api`, `cargo nextest run --locked -p workshop-server --features headless`, and `cargo test --doc -p workshop-server -p workshop-server-api`
  - `cargo clippy -p workshop-gateway -p workshop-status --all-targets --all-features -- -D warnings` and `cargo clippy -p workshop-server -p workshop-server-api --all-targets -- -D warnings`
  - `cargo fmt --all --check`
- Commit: "Delete WorkshopObserver, unused StatusBus helpers, and stale server re-exports"

</step-11>

<step-12>

### Step 12: Give the gateway app its own icon copies

- Component: Shell vocabulary
- Piece: icon copies
- Copy `icon.ico`, `32x32.png`, and `64x64.png` from `crates/workshop/shell/icons/` into `crates/gateway/app/assets/`, next to `tray-icon.rgba` and `tray-icon-template.rgba`. The copies must be byte-identical (compare with `Get-FileHash`).
- Point the build, the test, and the comments at the copies, all under `crates/gateway/app`: `build.rs:7,23`, `tests/it/icon.rs:2,42`, `Cargo.toml:19`, `src/tray/windows.rs:49-51`, `src/tray/macos.rs:63-66`, and `src/tray/linux.rs:54`.
- Extend the sync rule in `crates/workshop/shell/icons/AGENTS.md` to cover the gateway app's copies beside config-ui's.
- Tests: `cargo nextest run --locked -p gateway icon` (the icon embedding test) and `cargo check -p gateway --no-default-features` pass. `rg "workshop/shell/icons" crates/gateway/app` finds nothing.
- Commit: "Give the gateway app its own copies of the icons"

</step-12>

<step-13>

### Step 13: Move the desktop app to crates/workshop/desktop

- Component: Shell vocabulary
- Piece: directory move
- Approved scope widening (recorded in the Decision Record). Three files outside the Constraints' edit scope hard-code the old sidecar path: `tools/stage-gateway-sidecar.mjs:37` and `tools/stage-gateway-sidecar.test.mjs:116` build `crates/workshop/shell/binaries`, and `crates/build-workshop/tests/interruption.rs:51-53` checks it. `cargo workshop` (`crates/build-workshop/src/main.rs`) and CI (`.github/workflows/ci.yml:175,256` and `workshop-installer-smoke.yml:38`) stage through that script, so without these edits the desktop app can't find its sidecar after the move. The user approved editing the three files.
- `git mv crates/workshop/shell crates/workshop/desktop`.
- In the same commit, update the path strings that wire up the move:
  - root `Cargo.toml` (the `members` entry) and `.gitignore:22,25`
  - `.github/workflows/nightly.yml:204-210`, `.github/workflows/release-workshop.yml:167-191`, and `.github/workflows/workshop-installer-smoke.yml:8-9,43`
  - `crates/build-xtask/src/tidy.rs:84-86`, where the fallback directory `"shell"` becomes `"desktop"`, and `crates/build-xtask/src/tidy-tests.rs:223`
  - the path in `README.md:84` and in `AGENTS.md:27` (only the path; Step 18 rewrites the prose)
  - the comments at `crates/gateway/app/src/tray/windows.rs:596` and `crates/gateway/app/src/tray/macos.rs:459`
  - the `"shell"` path segment in the three approved files
- Catch the rest with `rg 'workshop[/\\]shell'` over the edit scope and the three approved files, excluding `vibe/`.
- `git mv` leaves the gitignored build outputs behind: the staged sidecar in `crates/workshop/shell/binaries/` and the Tauri output in `crates/workshop/shell/gen/`. Move both under `crates/workshop/desktop/`, so no `crates/workshop/shell/` directory remains and the sidecar from Step 1 keeps the `workshop` package building.
- Tests: `git diff --cached -M --name-status` lists every moved file as an `R` entry. `cargo test -p build-xtask`, `node --test tools/stage-gateway-sidecar.test.mjs`, `cargo nextest run --locked -p build-workshop --all-features --test interruption`, and `cargo nextest run --locked -p workshop gateway::` (which builds the desktop app from its new path) pass. `Test-Path crates/workshop/shell` is false, and the `rg` above finds nothing.
- Commit: "Move the desktop app to crates/workshop/desktop"

</step-13>

<step-14>

### Step 14: Retire "shell" in the Rust crates, the build check, and the workflows

- Component: Shell vocabulary
- Piece: Rust and workflow names
- build-xtask:
  - `crates/build-xtask/src/tidy.rs:31`: `SHELL` becomes `SERVER`, with the tier name "server". Update the shell prose and the fallback literal in the same file at lines 13, 27, 30, 84, and 86, plus the "Tauri shell" wording at line 252 ("Tier 3: the shell" is line 30).
  - `crates/build-xtask/src/product.rs:121`: `SHELL` becomes `DESKTOP`, and its value stays `"workshop"`. The shell-boundary prose and comments in the same file name the desktop app.
  - `crates/build-xtask/src/new_crate.rs:72`: the tier list in the new-crate template becomes `vocabulary | services | features | server`.
  - Rename the tests that mention "shell" in `crates/build-xtask/src/tidy-tests.rs` and `product-tests.rs`.
- `crates/workshop/server/src/lib.rs`: "Tier: shell" (line 24) becomes "Tier: server" in this same commit, so the tier name and the crate's Invariants agree, and "thin shell" (line 4) becomes "thin entry point".
- Every other non-Markdown file under `crates/workshop/**` outside `crates/workshop/ui` (Rust sources and tests, Cargo manifests with their comments and descriptions, build scripts, Tauri and installer config): classify each "shell" by meaning.
  - the Tauri app becomes "desktop app"
  - the server or its tier becomes "server"
  - any other non-terminal sense gets its own word
  - third-party names (for example Tauri's shell plugin or an NSIS keyword) keep their names
  - This covers rustdoc such as "The shell maps its per-crate error types" in `crates/workshop/protocol/src/error.rs`, the "shell" wording in `crates/workshop/server/src/agents/status.rs`, and test names. Rust variables, parameters, and fields named `shell` are renamed to match.
- In `.github/workflows/*.yml`, comments that use "shell" for the desktop app or the server (for example `ci.yml:177` and the "beside the shell" comments in `release-workshop.yml`) get the same words. `shell:` step keys are terminal shells and stay.
- Touch nothing under `crates/gateway` or `crates/workshop/ui`, and no Markdown file; Steps 15-18 cover those.
- Tests: `cargo test -p build-xtask` passes. Every other touched package compiles with `cargo nextest run --locked -p <package> --no-run` (with `--all-features` in the main partition), and its renamed tests pass by name. If a rustdoc intra-doc link names a renamed item, `cargo doc --no-deps -p <package>` passes with `RUSTDOCFLAGS="-D warnings"`. `rg -w SHELL crates/build-xtask/src` and `rg "Tier: shell" crates` find nothing, and `rg -i -w shell` over `crates/build-xtask/src`, `.github/workflows`, and `crates/workshop` (leaving out `crates/workshop/ui` and Markdown files) shows only third-party or terminal senses.
- Commit: "Retire shell for the desktop app and server in Rust and the workflows"

</step-14>

<step-15>

### Step 15: Retire "shell" in the shared status bar and the workshop UI

- Component: Shell vocabulary
- Piece: UI names
- shared-ui: in `crates/shared-ui/status-bar.ts`, `createStatusBarShell` becomes `createStatusBarView` and `StatusBarShell` becomes `StatusBarView`. Update the comment at `crates/shared-ui/status-bar.css:9` and the `crates/shared-ui/package.json` description.
- Status bar consumers: `crates/workshop/ui/src/parts/status/status-bar.ts`, `crates/workshop/ui/test/shared-status-bar.mjs`, and `crates/gateway/config-ui/ui/src/components/status-bar.ts`, plus the comments at `crates/gateway/config-ui/ui/src/components/status-bar.test.mjs:3` and `crates/gateway/config-ui/ui/src/styles/layout.css:1423`. Local `shell` variables that hold the status bar become `view`. In config-ui's `components/status-bar.ts`, "shell" means both the status bar and the frame: rename only the status bar references here, and leave the frame for Step 17.
- Workshop UI desk: `.ws-shell` becomes `.ws-desk` in `crates/workshop/ui/src/parts/layout/zones.css:8`, `crates/workshop/ui/index.html:41`, `crates/workshop/ui/test/workshop-layout.mjs:350`, and every other TypeScript and test reference.
- Workshop UI placeholder: "lazy shell" and "empty shell" become "placeholder" in `crates/workshop/ui/src/parts/layout/zones.css` (for example line 272), `crates/workshop/ui/src/parts/layout/panel-types.ts`, and `crates/workshop/ui/test/lazy-panel-sizing.mjs`. The local `shell` at `lazy-panel-sizing.mjs:222` becomes `placeholder`. Classify each `zones.css` occurrence by meaning: line 8 is the desk, and line 272 is the placeholder.
- Every other "shell" in `crates/workshop/ui` outside Markdown (TypeScript, tests, CSS, HTML, comments, and test descriptions): "boot shell" becomes "entry bundle", the Tauri app becomes "desktop app", and the server becomes "server". Terminal senses, such as the stubbed Terminal menu, stay.
- Tests: `node --test` passes for `crates/workshop/ui/test/shared-status-bar.mjs`, `crates/workshop/ui/test/lazy-panel-sizing.mjs`, `crates/gateway/config-ui/ui/src/components/status-bar.test.mjs`, and every other test file this step edits. `npm run typecheck` passes in both UIs. `rg "StatusBarShell|createStatusBarShell" crates` and `rg "ws-shell|lazy shell|empty shell|boot shell" crates/workshop/ui crates/shared-ui` find nothing.
- Commit: "Retire shell in the shared status bar and the workshop UI"

</step-15>

<step-16>

### Step 16: Move config-ui's views to pages

- Component: Shell vocabulary
- Piece: config-ui
- With paths relative to `crates/gateway/config-ui/ui/src`: `git mv views pages`. The six pairs for cloud-models, discover, models, profiles, secrets, and settings move from `*-view.ts` and `*-view.test.mjs` to `*-page.ts` and `*-page.test.mjs`. `apply-revert.test.mjs`, `model-detail.test.mjs`, and `settings-sections.test.mjs` move without a new name.
- In the same commit, fix only the import paths: the six imports at `main.ts:35-40`, and the relative imports inside the moved files and their tests. Identifiers keep their names until Step 17.
- Run `rg "views/" crates/gateway/config-ui` for references outside `ui/src`. If a build script, Rust asset list, or any other file outside the named exception names the old path, stop and report.
- Tests: `git diff --cached -M --name-status` lists every moved file as an `R` entry, and `views/` no longer exists. `npm run typecheck --prefix crates/gateway/config-ui/ui` and `node --test` over the moved test files pass.
- Commit: "Move config-ui's views to pages"

</step-16>

<step-17>

### Step 17: Rename config-ui's page identifiers and its frame to desk

- Component: Shell vocabulary
- Piece: config-ui
- With paths relative to `crates/gateway/config-ui/ui/src`, rename the page identifiers (about 300 references): `createXView` becomes `createXPage`, `XViewDeps` becomes `XPageDeps`, `ViewId` becomes `PageId`, `viewRoot` becomes `pageRoot`, `setActiveView` becomes `setActivePage`, `tabByView` becomes `tabByPage`, `defaultView` becomes `defaultPage`, `PendingView` becomes `PendingPage`, `disposeView` becomes `disposePage`, and the `.view-empty` class becomes `.page-empty`. Leave `review`, `viewport`, `openReviewDiff`, and `createStatusBarView` alone.
- Rename the frame to the desk (about 77 references across 22 files, test descriptions included):
  - `mountLiveShell` (`main.ts:204`) becomes `mountLiveDesk`, and `showShell` (`main.ts:103`) becomes `showDesk`.
  - The inert panel-mode mount documented at `main.ts:475` is described as the inert desk.
  - `main.className = "shell"` (`main.ts:513`) and the `.shell` rules at `styles/layout.css:71,1438` become `desk`.
  - Local variables named `shell` that hold the frame become `desk`, including the remaining frame references in `components/status-bar.ts`.
- Tests: `npm run typecheck --prefix crates/gateway/config-ui/ui` and `node --test` over every test file this step edits pass. In `crates/gateway/config-ui/ui/src`, `rg -w "ViewId|viewRoot|setActiveView|tabByView|defaultView|PendingView|disposeView|view-empty"` finds nothing, `rg "create\w+View\b|\w+ViewDeps"` finds only `createStatusBarView`, and `rg -w shell` shows only terminal senses. `rg "mountLiveShell|showShell" crates/gateway/config-ui` finds nothing.
- Commit: "Rename config-ui's views to pages and its shell to desk"

</step-17>

<step-18>

### Step 18: Retire "shell" in the docs and record the vocabulary

- Component: Shell vocabulary
- Piece: docs
- Root `AGENTS.md`: at line 27 "the shell" becomes "the desktop app"; at line 32 "the Workshop shell" becomes "the desktop app"; at lines 61-63 the tier chain becomes "server -> features -> services -> vocabulary", "boot shell" becomes "entry bundle", and "the Tauri shell" becomes "the desktop app". Fix any other non-terminal "shell" in root `AGENTS.md` and `README.md`.
- Crate rules: classify "shell" the same way in `crates/workshop/server/AGENTS.md`, `crates/workshop/desktop/AGENTS.md`, `crates/workshop/desktop/icons/AGENTS.md`, and `crates/workshop/ui/AGENTS.md`.
- Cursor rules: apply the same vocabulary, and nothing else, in `.cursor/rules/workshop-architecture.mdc` and `.cursor/rules/workshop-spa.mdc`: desktop app, server tier, entry bundle, and desk. This edit is approved in the Decision Record.
- Wrong claim: "the shell constructs the Harness" (`crates/workshop/server/AGENTS.md:13`). The server builds it: `compose` in `crates/workshop/server/src/app.rs` calls `harness_for` in `crates/workshop/server/src/agents.rs:55`.
- Add a Vocabulary section to root `AGENTS.md` with the words from Technical Design, "Architecture": shell, desktop app, server, desk, workbench, workshop socket, page, view, placeholder, and entry bundle. State each word's current meaning without quoting a retired phrase, since the retired-name grep and `docs-claims.mjs` both scan this file. Leave out the `workshop_socket` module name: Step 29 creates the module and Step 30 adds the name.
- Tests: `node --test crates/workshop/ui/test/docs-claims.mjs` passes. Over the edit scope, excluding `vibe/`, `rg "workshop/shell|Tier: shell|StatusBarShell|createStatusBarShell|ws-shell|mountLiveShell|showShell|WorkshopObserver|lazy shell|empty shell|boot shell"` finds nothing. "workbench socket" waits for Step 30.
- Component end, for the packages Steps 12-18 touched:
  - `cargo test -p build-xtask` and `cargo nextest run --locked -p gateway`, which covers the icon test
  - for each other main-partition package touched (at least `build-workshop` and the workshop crates Step 14 edited): `cargo nextest run --locked -p <package> --all-features`, then `cargo test --locked -p <package> --all-features --doc` where the package has a library
  - `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`, `cargo nextest run --locked -p workshop-server --features headless`, and `cargo test --doc -p workshop -p workshop-server -p workshop-server-api`
  - `cargo clippy --all-targets -- -D warnings` with one `-p` per touched package: the main-partition packages with `--all-features`, and `workshop`, `workshop-server`, and `workshop-server-api` in a separate invocation without it
  - `cargo fmt --all --check` and `node --test tools/stage-gateway-sidecar.test.mjs`
  - `npm test`, `npm run typecheck`, and `npm run build` in `crates/workshop/ui` and in `crates/gateway/config-ui/ui`
- Commit: "Retire shell in the workshop docs and record the vocabulary"

</step-18>

<step-19>

### Step 19: Move app.rs's children into an app directory

- Component: Structural consolidation
- Piece: split
- `git mv crates/workshop/server/src/app-fixtures.rs crates/workshop/server/src/app/fixtures.rs` and `git mv crates/workshop/server/src/app-tests.rs crates/workshop/server/src/app/tests.rs`. In the same commit, drop their `#[path]` attributes in `app.rs` (lines 13 and 16) so standard layout finds them.
- Step 20 adds `app/compose.rs` as the third child. The layout convention turns a group of three into a directory, and the History shape keeps this move apart from the compose extraction.
- Tests: `git diff --cached -M --name-status` lists both files as `R` entries. `cargo nextest run --locked -p workshop-server app::` and `cargo test -p build-xtask` pass.
- Commit: "Move app.rs's children into an app directory"

</step-19>

<step-20>

### Step 20: Break compose into per-subsystem register helpers

- Component: Structural consolidation
- Piece: split
- Move `compose` (about 115 lines, starting near `crates/workshop/server/src/app.rs:316`) into a new `crates/workshop/server/src/app/compose.rs`, declared from `app.rs`, and break it into one register helper per subsystem: gateway, menu, status, user-state, workspace, and the harness. The helpers still unpack today's tuples; Step 31 switches them to named fields.
- The composition order, and so the registration order, stays the same.
- Tests: `cargo nextest run --locked -p workshop-server app::` passes with the app tests unchanged, and `cargo test -p build-xtask` passes. `app.rs` ends well under 500 lines.
- Commit: "Break compose into per-subsystem register helpers"

</step-20>

<step-21>

### Step 21: Split out the agent socket's framing helpers

- Component: Structural consolidation
- Piece: split
- Move `input_frame`, `delta_frame`, `frame_entry`, and `drain_events` from `crates/workshop/server/src/agents/socket.rs` (492 lines) into a new sibling module `crates/workshop/server/src/agents/socket_frames.rs`, declared in `crates/workshop/server/src/agents.rs`, with table-driven tests in `agents/socket_frames-tests.rs` wired by `#[path]`. As a sibling module it needs no move of `socket-tests.rs`, and no later step moves it.
- Tests: `cargo nextest run --locked -p workshop-server agents::socket` (which matches both `socket` and `socket_frames`) passes with the existing socket tests unchanged, and `cargo test -p build-xtask` passes.
- Commit: "Split the agent socket's framing helpers into their own module"

</step-21>

<step-22>

### Step 22: Split the desktop supervisor

- Component: Structural consolidation
- Piece: split
- Split `crates/workshop/desktop/src/gateway/supervisor.rs` (751 lines) into new children under `crates/workshop/desktop/src/gateway/supervisor/`, one per seam: recovery-candidate ownership, stop and completion signals, thread lifecycle with the 3-second shutdown budget, and cancellable launch and wait.
- `supervisor.rs` keeps `run_supervision` and the injection APIs, so the tests in `crates/workshop/desktop/src/gateway/tests/` neither move nor change. The desktop crate is exempt from the file-size ceiling; this split is for readability.
- Tests: `cargo nextest run --locked -p workshop gateway::` passes with the gateway tests unchanged.
- Commit: "Split the desktop supervisor along its seams"

</step-22>

<step-23>

### Step 23: Name the phases of the heartbeat and progress loops

- Component: Structural consolidation
- Piece: split
- Extract named phase helpers from `heartbeat::run` (about 122 lines, `crates/workshop/gateway/src/heartbeat.rs:213`) and `gateway_progress::run` (about 102 lines, `crates/workshop/gateway/src/gateway_progress.rs:134`). Keep the helpers in the same files: `heartbeat.rs` already has two hyphenated siblings, and a third would trigger the directory rule. Both files stay under 500 lines (354 and 268 today).
- The `select!` semantics stay the same: the same branches, branch order, `biased` setting, and cancellation points.
- Tests: `cargo nextest run --locked -p workshop-gateway --all-features heartbeat gateway_progress` passes with the heartbeat and progress tests unchanged.
- Commit: "Name the phases of the heartbeat and progress run loops"

</step-23>

<step-24>

### Step 24: Serve /prompts/contract from the server

- Component: Structural consolidation
- Piece: prompts-route
- `git mv crates/workshop/workspace/src/handlers-prompts.rs crates/workshop/server/src/routes/prompts.rs` and `git mv crates/workshop/workspace/src/handlers-prompts-tests.rs crates/workshop/server/src/routes/prompts-tests.rs`. The test file keeps its `#[path = "prompts-tests.rs"]` wiring, like `routes/gateway_config-tests.rs`.
- In the same commit, the wiring:
  - Remove the `prompts` module (lines 24-25) and its mount from `crates/workshop/workspace/src/handlers.rs`.
  - Declare `prompts` in `crates/workshop/server/src/routes.rs` beside `assets`, `gateway_config`, `health`, and `realtime`, and mount it beside health, realtime, and gateway_config under the default deadline.
  - Map its errors to `AppError` in `crates/workshop/server/src/error.rs`, with the same status codes and wire error codes the workspace error type used, and remove the variants only this route used from `crates/workshop/workspace/src/error.rs`.
  - Drop `promptforge` from `crates/workshop/workspace/Cargo.toml` (line 23); the server already depends on it, so nothing is added there. Update the `## Invariants` blocks in both crates' `src/lib.rs`.
- Inside the moved files, change only what the server needs to build them: import paths, the error type, and the test setup that reaches the server's router. The test assertions stay the same. Put the error mapping in `error.rs` rather than in the moved file, so git still detects both files as renames.
- Tests: `git diff --cached -M --name-status` lists both files as `R` entries. `cargo nextest run --locked -p workshop-server routes::prompts`, `cargo nextest run --locked -p workshop-workspace --all-features handlers`, and `cargo test -p build-xtask` pass. `rg -w promptforge crates/workshop/workspace/src` finds nothing.
- Commit: "Serve the prompts contract route from the server"

</step-24>

<step-25>

### Step 25: Share the error rendering and the state-bucket validator

- Component: Structural consolidation
- Piece: helpers
- Error rendering: move `render_message` and `LEAK_DETAIL` into a new `crates/workshop/support/src/error_message.rs`, exported from `crates/workshop/support/src/lib.rs`, with unit tests in `error_message-tests.rs`. Delete the copies in `crates/workshop/workspace/src/error.rs`, `crates/workshop/server/src/error.rs`, and `crates/workshop/user-state/src/error.rs`, and point `crates/workshop/server/src/agents/relay.rs` at the shared `LEAK_DETAIL`. Rendered messages stay byte-for-byte the same.
- State-bucket validator: add it in a new `crates/workshop/support/src/state_bucket.rs`, with tests in `state_bucket-tests.rs`. It checks the key against an allow list the caller passes, caps the body at 1 MiB, and requires the body to parse as JSON, returning a support-level error with one variant per refusal. Switch both copies to it: `crates/workshop/user-state/src/store.rs` and `handlers.rs`, and `crates/workshop/workspace/src/workspace_file-ui-state.rs` and `handlers-file-state.rs`. Each crate maps the support error onto its existing variants, so the wire codes (`user_state_key`, `user_state_too_large`, `user_state_not_json`, `ui_state_key`, `ui_state_too_large`, `ui_state_not_json`) and the messages don't change.
- Tests: the new support tests cover each refusal, an accepted body, and the rendered messages. The existing tests pass unchanged: `cargo nextest run --locked -p workshop-support --all-features error_message state_bucket`, `cargo nextest run --locked -p workshop-user-state --all-features store handlers error`, `cargo nextest run --locked -p workshop-workspace --all-features error ui_state file_state`, and `cargo nextest run --locked -p workshop-server error relay`.
- Commit: "Share error rendering and the state-bucket validator through workshop-support"

</step-25>

<step-26>

### Step 26: Share the mock HTTP server test helper

- Component: Structural consolidation
- Piece: helpers
- Add a helper behind support's `test-fixtures` feature (add the feature if it's missing), in a new `crates/workshop/support/src/fixtures.rs`. It binds a loopback port, runs `axum::serve` on the caller's router in a task, and returns the bound address, plus the task handle if callers stop it.
- Switch the near-copies to it. Find them with `rg "axum::serve" crates/workshop/gateway crates/workshop/server`, skipping the production server in `serve.rs`. They include `crates/workshop/server/src/app/fixtures.rs`, `crates/workshop/server/src/agents/relay-tests.rs`, `crates/workshop/gateway/src/gateway/tests.rs`, `crates/workshop/gateway/src/gateway_progress-tests.rs`, and the server integration tests `session.rs`, `session/menu/restart.rs`, `heartbeat_loop.rs`, `chat_gate.rs`, and `agents.rs` under `tests/it/`.
- Enable support's `test-fixtures` feature in the gateway's and the server's dev-dependencies where it isn't enabled already.
- Tests: `cargo nextest run --locked -p workshop-support --all-features fixtures`, `cargo nextest run --locked -p workshop-gateway --all-features gateway::tests gateway_progress`, `cargo nextest run --locked -p workshop-server app:: relay session heartbeat_loop chat_gate agents`, and `cargo test -p build-xtask` pass.
- Commit: "Share the mock HTTP server test helper through workshop-support"

</step-26>

<step-27>

### Step 27: Move the hyphenated groups into directories

- Component: Structural consolidation
- Piece: layout
- In one commit, `git mv` each group of three or more hyphenated siblings into standard module layout, drop the moved files' `#[path]` attributes, declare the modules in standard layout, and fix the imports. Each file lands where Rust's standard layout looks for it from the module that declares it today. The parent files (`workspace.rs`, `handlers.rs`, `workspace_file.rs`, and `gateway_progress.rs`) stay where they are.
- The children of `crates/workshop/workspace/src/workspace.rs`:
  - `workspace-backing.rs`, `workspace-confine.rs`, `workspace-pointer.rs`, and `workspace-token.rs` become `workspace/backing.rs`, `workspace/confine.rs`, `workspace/pointer.rs`, and `workspace/token.rs`.
  - `workspace-tests.rs` becomes `workspace/tests.rs`. `workspace-tests-close.rs`, `workspace-tests-reopen.rs`, and `workspace-tests-switch.rs`, which `workspace.rs` declares, become `workspace/tests_close.rs`, `workspace/tests_reopen.rs`, and `workspace/tests_switch.rs`.
  - The test children `workspace-tests-backing.rs`, `workspace-tests-grants.rs`, `workspace-tests-pointer.rs`, and Step 3's `workspace-tests-jail.rs` become `workspace/tests/backing.rs`, `workspace/tests/grants.rs`, `workspace/tests/pointer.rs`, and `workspace/tests/jail.rs`.
- The children of `crates/workshop/workspace/src/handlers.rs`: `handlers-file.rs` becomes `handlers/file.rs` and its `handlers-file-tests.rs` becomes `handlers/file/tests.rs`; `handlers-file-state.rs` becomes `handlers/file_state.rs` and its `handlers-file-state-tests.rs` becomes `handlers/file_state/tests.rs`; `handlers-tests.rs` becomes `handlers/tests.rs`.
- The children of `crates/workshop/workspace/src/workspace_file.rs`: `workspace_file-actor.rs` and `workspace_file-siblings.rs` become `workspace_file/actor.rs` and `workspace_file/siblings.rs`. `workspace-file-tests.rs` becomes `workspace_file/tests.rs`, and its `workspace-file-tests-mutations.rs` becomes `workspace_file/tests/mutations.rs`.
- The four `ui_state` modules take distinct names in the same move, because standard layout ties a module's name to its file name:
  - `workspace-ui-state.rs` (the in-memory map on the backing) becomes `workspace/backing/ui_state_memory.rs`, module `ui_state_memory`
  - `workspace-tests-ui-state.rs` becomes `workspace/tests/ui_state_memory_tests.rs`, module `ui_state_memory_tests`
  - `workspace_file-ui-state.rs` (the values in the file's `kv` table) becomes `workspace_file/ui_state_kv.rs`, module `ui_state_kv`
  - `workspace-file-tests-ui-state.rs` becomes `workspace_file/tests/ui_state_kv_tests.rs`, module `ui_state_kv_tests`
- The children of `crates/workshop/gateway/src/gateway_progress.rs`: `gateway_progress-presenter.rs` becomes `gateway_progress/presenter.rs`, `gateway_progress-tests.rs` becomes `gateway_progress/tests.rs`, and its `gateway_progress-tests-presenter.rs` and `gateway_progress-tests-recovery.rs` become `gateway_progress/tests/presenter.rs` and `gateway_progress/tests/recovery.rs`.
- Tests: `git diff --cached -M --name-status` lists every moved file as an `R` entry. `cargo nextest run --locked -p workshop-workspace --all-features workspace handlers`, `cargo nextest run --locked -p workshop-gateway --all-features gateway_progress`, and `cargo test -p build-xtask` pass. `rg -n "#\[path" crates/workshop/workspace/src crates/workshop/gateway/src` shows only groups under three files, such as `error-tests.rs`, `resolve-tests.rs`, the two `heartbeat-*.rs` files, and `test_gateway-process.rs`.
- Commit: "Move the workspace and gateway progress groups into directories"

</step-27>

<step-28>

### Step 28: Remove the server's module aliases

- Component: Structural consolidation
- Piece: renames
- Remove the pre-decomposition aliases in `crates/workshop/server/src/lib.rs` (lines 60-67: `gateway`, `gateway_binding`, `gateway_progress`, `heartbeat`, `resolve`, `catalog`, `menu`, and `status`), and point their call sites at the real crates (`workshop_gateway::gateway::...` and so on). The named public re-exports below them (`GatewayClient`, `GatewayUpdater`, `ResolvedGateway`, and the rest) name the real paths.
- The call sites, about 24: `serve.rs`, `fixtures.rs`, `error.rs`, `app.rs`, `app/compose.rs`, `app/tests.rs`, `app/fixtures.rs`, and `routes/realtime.rs` in the server; the server's integration tests; and `boot.rs`, `identity.rs`, `recovery.rs`, and `shutdown.rs` in `crates/workshop/desktop/src/gateway/tests/`.
- The desktop app may depend only on `workshop-server-api`, so its call sites switch to named re-exports. If an item it needs has none, add a named `pub use` to the server's `lib.rs`, not an alias module.
- Tests: `cargo nextest run --locked -p workshop-server -p workshop-server-api --no-run`, `cargo nextest run --locked -p workshop-server app::`, `cargo nextest run --locked -p workshop gateway::`, and `cargo test -p build-xtask` pass. No path in the server, its tests, or the desktop app goes through a removed alias.
- Commit: "Remove the server's pre-decomposition module aliases"

</step-28>

<step-29>

### Step 29: Move the /ws socket into a workshop_socket module

- Component: Structural consolidation
- Piece: renames
- `git mv crates/workshop/server/src/agents/session.rs crates/workshop/server/src/workshop_socket.rs` and `git mv crates/workshop/server/src/agents/session-menu.rs crates/workshop/server/src/workshop_socket-menu.rs`.
- In the same commit: declare `mod workshop_socket;` in `crates/workshop/server/src/lib.rs`, remove `session` from `crates/workshop/server/src/agents.rs`, point the menu child's attribute at `#[path = "workshop_socket-menu.rs"]`, and fix the paths that were relative to `agents`. `SessionsState` still mounts `/ws` (`crates/workshop/server/src/agents/state.rs:141`), now from `crate::workshop_socket`.
- Tests: `git diff --cached -M --name-status` lists both files as `R` entries. `cargo nextest run --locked -p workshop-server session workshop_socket` passes, including the `/ws` tests in `tests/it/session/`.
- Commit: "Move the /ws socket into a workshop_socket module"

</step-29>

<step-30>

### Step 30: Rename the status relay, the gateway's SwitchOutcome, and the socket wording

- Component: Structural consolidation
- Piece: renames
- In `crates/workshop/server/src/agents/status.rs`, `spawn_relay` becomes `spawn_reporter`, `relay` becomes `report`, and the module doc calls the task the status reporter, so "relay" only means the model-catalog passthrough in `agents/relay.rs`. Update the caller in `crates/workshop/server/src/agents.rs` and the names in `status-tests.rs`.
- The gateway's `SwitchOutcome` becomes `SwitchProfileBody` in `crates/workshop/gateway/src/gateway/events.rs`, `gateway.rs`, `lib.rs`, and `gateway/tests/switch.rs`, and in the server's re-export in `crates/workshop/server/src/lib.rs`. workshop-menu's `SwitchOutcome` keeps its name.
- "the /ws workbench socket" becomes "the /ws workshop socket" in the crate doc of `crates/workshop/server/src/lib.rs` (line 14), in `crates/workshop/server/src/agents.rs:1`, and in `crates/workshop/server/src/agents/socket.rs:46`. Add the `workshop_socket` module name to the workshop socket entry of the root `AGENTS.md` Vocabulary section.
- Tests: `cargo nextest run --locked -p workshop-server agents::status` and `cargo nextest run --locked -p workshop-gateway --all-features switch` pass. `rg -w relay crates/workshop/server/src/agents/status.rs crates/workshop/server/src/agents/status-tests.rs`, `rg -w SwitchOutcome crates/workshop/gateway`, and `rg "workbench socket"` over the edit scope, excluding `vibe/`, find nothing.
- Commit: "Rename the status relay, the gateway's SwitchOutcome, and the /ws socket wording"

</step-30>

<step-31>

### Step 31: Return named registration structs from every subsystem

- Component: Structural consolidation
- Piece: renames
- Add `crates/workshop/user-state/src/handles.rs`, and move `register` into it from `crates/workshop/user-state/src/lib.rs:45`, keeping its public path through a re-export.
- In the `handles.rs` of gateway, menu, status, user-state, and workspace, `register` returns a named struct of registration guards instead of a tuple (for example `WorkspaceRegistrations`, with one field per guard), and so does `register_tasks` where one exists.
- The register helpers in `crates/workshop/server/src/app/compose.rs` read the named fields instead of unpacking by position.
- Tests: `cargo nextest run --locked -p workshop-gateway -p workshop-menu -p workshop-status -p workshop-user-state -p workshop-workspace --all-features --no-run` passes, and so do the subsystem tests that call `register` or `register_tasks`, run by name, and `cargo nextest run --locked -p workshop-server app::`.
- Commit: "Return named registration structs from every subsystem"

</step-31>

<step-32>

### Step 32: Move the workshop UI's shared backoff and service tokens into services

- Component: Structural consolidation
- Piece: workshop UI structure
- Reconnect backoff: merge the two implementations (`crates/workshop/ui/src/services/workshop-socket.ts:21` and `crates/workshop/ui/src/services/agent-socket.ts:48`) into a new `crates/workshop/ui/src/services/reconnect-backoff.ts`. If their delays or caps differ, the module takes them as options, and each socket keeps its current values.
- Service tokens: move these tokens and their interface types into new modules under `crates/workshop/ui/src/services/`, one per service. The implementations stay in `parts/` and register against the tokens:
  - `STATUS_BAR` (`crates/workshop/ui/src/parts/status/status-bar.ts:198`)
  - `CLOSED_EDITORS` (`crates/workshop/ui/src/parts/editor/closed-editors.ts:145`)
  - `EDITOR_SETTINGS_SERVICE` (`crates/workshop/ui/src/parts/editor/editor-settings-service.ts:165`)
  - `QUICK_INPUT_SERVICE` (`crates/workshop/ui/src/parts/quickinput/quick-input.ts:308`)
- Switch every consumer to import the tokens from `services/`, and update the imports-flow layering rule at `crates/workshop/ui/AGENTS.md:5` (and the service-token list it describes).
- Tests: a new `crates/workshop/ui/test/reconnect-backoff.mjs` covers growth, the cap, and reset. It, the socket test files, `crates/workshop/ui/test/lazy-panel-sizing.mjs`, and every other test file this step edits pass under `node --test`, and `npm run typecheck --prefix crates/workshop/ui` passes. No file imports one of the four tokens from `parts/`.
- Component end, for the packages Steps 19-32 touched:
  - `cargo nextest run --locked -p workshop-support -p workshop-workspace -p workshop-user-state -p workshop-gateway -p workshop-menu -p workshop-status --all-features`, then `cargo test --locked` over the same packages with `--all-features --doc`
  - `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`, `cargo nextest run --locked -p workshop-server --features headless`, and `cargo test --doc -p workshop -p workshop-server -p workshop-server-api`
  - `cargo clippy --all-targets --all-features -- -D warnings` with one `-p` for each of the six main-partition packages above, and `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`
  - `cargo fmt --all --check` and `cargo test -p build-xtask`
  - `npm test`, `npm run typecheck`, and `npm run build` in `crates/workshop/ui`
- Commit: "Move the workshop UI's shared backoff and service tokens into services"

</step-32>

<step-33>

### Step 33: Correct the code-level docs

- Component: Code-level docs
- Piece: docs
- Registry: reword the claims that the registry never names a subsystem, in `crates/workshop/registry/src/lib.rs:19-21` and the `description` in `crates/workshop/registry/Cargo.toml:9`. The traits keep their subsystem names (`MenuSink`, `CatalogSink`, `StatusSink`, `WorkspaceRoots`, and `MenuPush`). Record the three runtime links in the registry's crate docs: the gateway drives the menu through `MenuPush` (`crates/workshop/registry/src/push.rs:141-175`), publishing a model catalog forces a menu reconcile (`push.rs:99-106`), and agent sessions read the workspace's granted roots through `WorkspaceRoots`.
- Code-comment drift (the README items went with Step 2):
  - `crates/workshop/desktop/Cargo.toml:50-51`: `src/linux_media.rs` handles the Linux microphone permission, not `src/bridge.rs`.
  - `crates/workshop/desktop/Cargo.toml:69-73`: clippy `pedantic` is lowered too, not only `unsafe_code` (compare the workspace lints in root `Cargo.toml`, around line 274).
  - `crates/workshop/protocol/src/lib.rs:83-85`: confirm the sentence about the session loops reads in the present tense; master's earlier doc sweep already made it so, so expect no edit here.
  - `crates/workshop/support/src/atomic.rs:1-2`: `write_atomic` is also used by `crates/workshop/user-state/src/store.rs:95` and by the workspace pointer module (`crates/workshop/workspace/src/workspace/pointer.rs` since Step 27).
  - `crates/workshop/ui/src/services/protocol.ts:66-67`: point the citation at `crates/workshop/server/src/agents/socket.rs`.
- Import pointer: in every workshop crate's `src/lib.rs` (for example `crates/workshop/support/src/lib.rs:10`) and in the new-crate template at `crates/build-xtask/src/new_crate.rs:73`, "Read `AGENTS.md` before adding an import." names the repository-root `AGENTS.md`. The server's sentence also names `crates/workshop/server/AGENTS.md`, and the desktop app's names `crates/workshop/desktop/AGENTS.md` if its `lib.rs` has the sentence. Update any build-xtask test that pins the template text.
- Sweep: re-read these against the code and fix what drifted: the workshop rules and the Vocabulary section in root `AGENTS.md`; `crates/workshop/server/AGENTS.md`, `crates/workshop/desktop/AGENTS.md`, `crates/workshop/desktop/icons/AGENTS.md`, and `crates/workshop/ui/AGENTS.md`; each workshop crate's `//!` crate doc and `## Invariants` block; and each workshop crate's Cargo `description`.
- Tests: `cargo test -p build-xtask` passes. `cargo doc --no-deps` with `RUSTDOCFLAGS="-D warnings"` passes for each touched main-partition library crate; CI leaves the three workshop-partition crates out of `cargo doc`, so they get `cargo nextest run --locked -p <package> --no-run` instead. `npm run typecheck --prefix crates/workshop/ui` and `node --test crates/workshop/ui/test/docs-claims.mjs` pass. `rg "Read .AGENTS\.md. before" crates/workshop crates/build-xtask/src` finds nothing.
- Component end: Step 34 runs next, and its full gates cover every package this step touched, so this component adds no separate checks.
- Commit: "Correct the workshop's code-level docs"

</step-33>

<step-34>

### Step 34: Run the exit gates

- Component: Exit
- Piece: exit
- With the sidecar from Step 1 still staged, run every canonical gate from the Testing Plan exit criteria, plus `cargo nextest run --locked -p workshop-server --features headless` and `cargo test --doc -p workshop -p workshop-server -p workshop-server-api`. Each must end at least as green as its line in Step 1's `Baseline results`.
- Remove the staged sidecar with `node tools/stage-gateway-sidecar.mjs remove --target x86_64-pc-windows-msvc`, then run `cargo workshop`. It must build the desktop app from `crates/workshop/desktop`.
- Run `cargo run -p build-user-guide`. It writes only the gateway, language, and agent exports, `git status` shows them unchanged, and no workshop export exists.
- Retired names: over the edit scope, excluding `vibe/`, a grep for `workshop/shell`, `Tier: shell`, `StatusBarShell`, `createStatusBarShell`, `ws-shell`, `mountLiveShell`, `showShell`, `WorkshopObserver`, "workbench socket", "lazy shell", "empty shell", and "boot shell" finds nothing, and `rg -w SHELL crates/build-xtask/src` finds nothing.
- No variable, parameter, or field named `shell` remains in `.rs`, `.ts`, or `.mjs` files in scope.
- Every remaining "shell" in `crates/workshop`, `crates/build-xtask`, `crates/shared-ui`, `crates/gateway/config-ui/ui/src`, and the root docs means a terminal command shell.
- Record the exit results beside the baseline in Step 1.
- Tests: every exit check passes. If one fails, fix it within this step's commit and rerun that check.
- Commit: "Record the exit gate results", holding the exit results, the completion mark, and any fix an exit check needed.

</step-34>

</execution-plan>
