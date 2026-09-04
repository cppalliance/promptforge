---
name: Gateway sidecar decomposition
overview: Split the workshop desktop app into two processes - promptforge-workshop.exe (shell plus in-process workshop-server) and promptforge-gateway.exe (always a separate process, tray-resident) - with connection-file discovery so the workshop attaches to a running gateway or launches one, and closing the window never unloads the gateway. The installer presents Gateway, Workshop, and STT as independent optional components.
todos:
  - id: binary-rename
    content: "Step 1 - Rename binaries to promptforge-gateway / promptforge-workshop; rename shared crates to shared-loopback / shared-protocol (cargo bins, tauri.conf.json, installer refs, release packaging, dependent manifests)"
    status: completed
  - id: connection-file
    content: "Step 2 - New shared-sidecar crate: connection-file types, atomic owner-only write/removal, stale detection + cleanup, launch lock, health wait (from health.rs)"
    status: completed
  - id: gateway-first-run
    content: "Step 3 - Gateway boot self-provisions default config (port 0, no [workshop], STT gated on installer flag); shell stops generating config"
    status: completed
  - id: stt-feature
    content: "Step 4 - Re-gate STT as default-on stt feature (workshop feature survives as hosting-only until step 7); no-default-features stays green"
    status: completed
  - id: gateway-http-surface
    content: "Step 5 - Authenticated key probe, POST /shutdown, Host-header allowlist, one-time redirect URL for browser handoff"
    status: completed
  - id: connection-client
    content: "Step 6 - workshop-server resolves the gateway via connection file first (pid+health+key validation, stale cleanup), then explicit config"
    status: completed
  - id: shell-hosts-server
    content: "Step 7 - workshop.exe hosts workshop-server in-process; exact-port capability + CSP; GatewaySlot removed; gateway workshop feature and hosting removed; close stops the server only"
    status: completed
  - id: attach-launch
    content: "Step 8 - Attach-or-launch via std::process::Command detached; Workshop-only fallback; single-instance; Quit-everything posts /shutdown"
    status: completed
  - id: tray-abstraction-windows
    content: "Step 9 - Per-OS tray abstraction + Windows backend (hidden-window loop, menu idiom, double-click Settings, teardown order, polled-health icon)"
    status: completed
  - id: tray-macos
    content: "Step 10 - macOS backend: accessory policy, template icon, /usr/bin/open sibling launch, SMAppService login toggle"
    status: completed
  - id: tray-linux
    content: "Step 11 - Linux backend: ksni async-io, no-watcher fallback, XDG autostart toggle; plus --print-url and relaunch-opens-Settings CLI affordances"
    status: completed
  - id: installer-components
    content: "Step 12 - externalBin + NSIS components (Gateway/Workshop/STT), registry-persisted selection, updater stop/restart daemon, updater flags contract, uninstaller Run-key cleanup names PromptForgeGateway"
    status: completed
  - id: docs-ceilings
    content: "Step 13 - AGENTS.md/README updates, module ceilings with reasons, final full-suite Verify"
    status: completed
isProject: false
---

# Gateway Sidecar Decomposition

## Thesis

One gateway, one shape. Today the gateway ships in two forms - standalone lean and workshop-merged (`gateway` with the `workshop` feature, booted in-process by the shell) - and the merge carries its own bullshit on both sides. This plan deletes the merge: the gateway is always a separate process, the workshop shell hosts workshop-server in-process (the standalone `workshop-server` flow promoted to product flow), and the two always talk over HTTP - loopback, WSL, or LAN become one topology. Closing the workshop window stops meaning "unload 350GB of models."

This lands **before** the unified event system plan, which then gets one hub per process by construction - the never-cross rule becomes structural.

## The discovery answer (the port problem)

Connection-file pattern, Jupyter-style:

1. The sidecar gateway binds `127.0.0.1:0` - the OS assigns a free port; conflicts are impossible by construction. Fixed ports remain available for the standalone LAN-reachable server via explicit config.
2. After binding, the gateway writes a connection file `gateway.json` in a run directory under the state dir: `{port, api_key, pid, epoch, version, started_at}`.
3. Workshop startup: read the connection file, health-check `127.0.0.1:<port>/health`, then validate the file's api_key against an authenticated route; if both answer, attach. Missing or stale (dead pid, failed health, rejected key): launch `gateway.exe` detached, wait for the file/health, attach.
4. Launch races (two workshops, one gateway): a `gateway.json.lock` file beside the connection file in the run directory plus the health check settles it - loser attaches to the winner's gateway. (The `.locks` name is already taken by artifact-cache publish locks in gateway-local; the launch lock does not reuse it.)
5. "Configure the port when the gateway isn't running" is a non-paradox: the config is a file. First-run default generation moves into the gateway itself (from the shell's `discover.rs`), so both standalone and sidecar first-boots self-provision; live changes use the existing config-ui.

## Pieces

### 1. Connection file and discovery
- The whole seam lives in a new `shared-sidecar` crate shared by its three consumers (the gateway writes; workshop-server and the shell read): file format, atomic owner-only write, validation, stale detection, launch lock, health wait. One crate for this seam - not a catch-all shared crate, per the repo's tiny-per-seam precedent (`shared-loopback`, renamed from `gateway-loopback` in step 1) and the junk-drawer failure mode of `common`/`shared` crates.
- Gateway writes `gateway.json` after a successful bind (atomic write - the repo has `atomic.rs` patterns in workshop-server; gateway gets the same), removes it on clean shutdown.
- Sidecar default bind becomes `127.0.0.1:0` when no `[server] bind` is configured; the file carries the real port.
- Stale-file rules: pid alive + health answers + the file's api_key is accepted = attachable; anything else = relaunch. `GET /health` is unauthenticated today, so key validity needs an authenticated probe: reuse an existing key-gated route or add a trivial `GET /v1/whoami`.
- Jupyter-pattern hardening (the documented pitfalls of connection files): the file gets owner-only permissions (it carries the bearer key); stale detection verifies the pid is alive AND belongs to a promptforge-gateway binary, and the stale file is deleted on detection (SIGKILL orphans are Jupyter's phantom-server bug class); URLs normalize to literal `127.0.0.1`, never `localhost` (resolution ambiguity has burned both Jupyter and Syncthing); the gateway rejects requests whose `Host` header is not the bound loopback address (DNS-rebinding defense, same as Jupyter); the tray and shell hand the browser a one-time redirect URL that sets a cookie and redirects to a clean URL, so the api_key never sits in browser history.
- Tests: file written after bind, removed on shutdown, stale pid detected, wrong key detected, lock-race loser attaches.

### 2. Gateway first-run config generation
- Move the default `gateway.toml` + `profiles/default.toml` generation from [workshop/src/discover.rs](c:\Users\Vinnie\cursor\promptforge\crates\workshop\src\discover.rs) into the gateway's boot path, so a bare `gateway.exe` first run self-provisions exactly as the shell does today.
- The generated default changes shape at the same time: sidecar bind becomes `127.0.0.1:0`, the `[workshop]` section disappears, and the `[[stt_model]]` pair stays (STT is no longer workshop-gated, so the `STT_REQUIRES_WORKSHOP` startup refusal in runner.rs is removed). The discover.rs tests that assert port 8081 and the `[workshop]` section are rewritten as gateway boot tests, not moved verbatim.
- The shell stops generating config; it just launches the gateway.

### 3. The shell hosts workshop-server
- `workshop.exe` embeds the `workshop-server` library directly: serve the workshop UI on a loopback listener in-process (the standalone binary already does this), point the Tauri window at it.
- workshop-server's `workshop.toml` gains/uses a gateway base-url resolution order: connection file first, then explicit config.
- Webview-to-loopback hardening: the window's capability is built programmatically in `.setup()` with the exact bound port (`CapabilityBuilder` + `app.add_capability`) rather than a wildcard origin; workshop-server sets its own CSP headers including `connect-src ipc: http://ipc.localhost` (required for Tauri IPC from an External origin); the existing `on_navigation` same-origin guard stays.
- The gateway's `workshop` feature and its hosting of workshop-server routes are removed. STT is re-gated behind a new default-on `stt` feature (the `workshop` gate renamed): `gateway-stt` and `/v1/audio/transcriptions` ship in every default build and serve any client, while the lean `--no-default-features` build stubs STT exactly as it does today. The design record's endgame (voice lives in the workshop server) is noted as a later migration, not this plan.
- Tests: the existing workshop-server integration suite runs against the shell-hosted server unchanged.

### 4. Attach-or-launch lifecycle
- Shell boot: resolve connection file -> health check -> attach, or launch `promptforge-gateway.exe` detached from beside the shell's own executable -> wait for health (the `health.rs` wait logic moves into `shared-sidecar` and carries over) -> attach. On a Workshop-only install (no local gateway binary), launch is skipped and resolution falls through to explicit `workshop.toml` config - a LAN gateway - or a clear "no gateway configured" error.
- Closing the window shuts down workshop-server and the window only. The gateway stays. A "Quit everything" affordance (window menu) posts the gateway's shutdown route for the user who wants it.
- That route does not exist today - shutdown is `GatewayHandle::shutdown` / Ctrl-C only - so the gateway gains an authenticated `POST /shutdown` (bearer key required, since it kills the process; the connection file's key is what the shell posts).
- The shell's `GatewaySlot`/in-process `shutdown` machinery is replaced by process detach + connection-file attach.
- The gateway is spawned with `std::process::Command` (detached, `CREATE_BREAKAWAY_FROM_JOB` on Windows) - never via tauri-plugin-shell's sidecar API, which registers every spawned child and kills them all on `RunEvent::Exit` (verified in the plugin source; that behavior is the exact opposite of this design). The shell never reads `gateway.toml` - the connection file is the single source of truth for connection parameters (the SyncTrayzor lesson: the wrapper owns the connection, the daemon's config is an implementation detail). The shell's exit path never deletes `gateway.json` - the gateway owns its lifecycle. The shell adds tauri-plugin-single-instance so a second launch attaches/focuses instead of racing the first.

### 5. Gateway system tray
- The gateway has no window - its only UI is the config SPA on its configured port - so on an installed system the tray icon is its face. `promptforge-gateway` gains a tray icon behind a per-OS abstraction: on Windows a hidden-window message loop owns the main thread with tokio on worker threads; on macOS the `NSApplication` run loop owns the main thread; on Linux ksni runs on the tokio runtime directly (no GTK main thread to appease).
- Menu layout follows the tray-daemon idiom (Tailscale, Rancher Desktop, Oboto converge on it): a disabled status label on top (gateway state plus models loaded, e.g. "Running - 2 models, 4.1 GB"), then **Workshop**, **Settings**, separator, **Launch at Login** (check item whose state is read from the OS - SMAppService.status, the Run key, the XDG file - never from local config, since users can revoke it externally), separator, **Quit** last.
- The icon is a state machine - grayed/animated while starting, steady when running, distinct on error, tooltip carrying the text - driven by polled `/health`, never by one-shot startup events (Docker/Rancher's "starting forever" bugs come from event-driven icon state). Browser-handoff items stay disabled until health passes.
- Quit is fast and always works, even when the backend is wedged: models are dropped, not checkpointed; no confirmation dialog. Slow-quit spinners train users to force-kill, which orphans connection files (Docker's worst tray bug class).
- Login launches are headless: the autostart entry carries a `--login` flag so a login-triggered start never opens a browser or window (opening a browser at every login is Syncthing's most-documented annoyance). Relaunching the gateway while it runs opens the Settings SPA instead of a second instance. A CLI escape hatch prints the Settings URL for tray-less environments.
- Settings opens the config-ui URL in the default browser, and so does double-clicking the tray icon. The double-click handler is `#[cfg(target_os = "windows")]` - the crate only delivers `TrayIconEvent::DoubleClick` on Windows, and on macOS the menu opens on mouse-down so the gesture cannot exist (menu-only there, Settings first in the menu). Windows sets `with_menu_on_left_click(false)` so the double-click's two preceding Click events do not pop the menu on the way to Settings. The `open` crate - currently pulled in by the removed `workshop` feature - stays as a tray dependency for the browser launch.
- The Workshop item launches `promptforge-workshop.exe` detached (on macOS, via `/usr/bin/open <bundle> --args`, which is sandbox-immune where NSWorkspace argument passing is not). Its enabled state comes from a sibling check - the installer lays both executables in the same directory, so the gateway probes for the workshop exe beside `current_exe()`: present = enabled, absent (Gateway-only install) = disabled. muda delivers no menu-about-to-open event on any platform, so the state is maintained eagerly: probe at tray construction, re-probe on the right-click `Click` event before `show_menu()` (the crate's blessed refresh pattern since 0.22). Menu items are mutated in place via a retained `MenuItem` handle, never rebuilt while open - rebuilding a displayed menu is both a stale-menu UX bug (muda#129) and a fixed-but-recent use-after-free (muda#328/#361), so pin muda to a release containing the #361 fix. Because the workshop attaches through the connection file, this item is also the "reopen the window" path after the user closes it - the gateway and its loaded models are already running.
- Tray presence is default in installed/sidecar mode and opt-out (`--no-tray`) for servers.
- **Linux backend is ksni, not tray-icon's default.** tray-icon's Linux backend is GTK3 + libappindicator - deprecated upstream, removed from Debian 11+ except as the Ayatana fork, known to panic inside AppImages when the bundled GLib diverges from the host, and incapable of delivering icon click events. The tray therefore sits behind a small per-OS abstraction: `tray-icon` on Windows/macOS, `ksni` (pure StatusNotifierItem over D-Bus) on Linux, built with `default-features = false, features = ["async-io"]` to avoid the documented zbus/tokio runtime panic. This drops GTK3/libappindicator/libxdo from the Linux build and runtime closure and matches where Tauri upstream is heading (SNI-only in the GTK4 migration).
- **Stock GNOME has no tray** - no StatusNotifierWatcher runs without the AppIndicator shell extension. Linux degrades gracefully: register when a watcher is present (ksni re-registers automatically if one appears later), keep serving the SPA regardless, post one first-run notification with the Settings URL, and install a `.desktop` launcher whose re-launch opens the Settings SPA (single-instance handoff). Install docs point GNOME users at the extension.
- **Autostart is opt-in from the Settings SPA**, never forced by the installer, and a user-deleted entry is never resurrected: "Start on login" toggle writing `~/.config/autostart/promptforge-gateway.desktop` (XDG autostart) on Linux, `SMAppService.mainApp` via `objc2-service-management` on macOS (LaunchAgents surface badly in System Settings and can trigger TCC prompts), the HKCU Run key on Windows (the uninstaller already cleans that value).
- **Crate and icon hygiene.** Pin `tray-icon = "0.24"` (0.24.1+ re-registers on `TaskbarCreated` and preserves the icon across Explorer restarts - decisive for a long-lived daemon). Windows icon: multi-size ICO or 32x32 RGBA. macOS icon: monochrome 18pt template glyph (36px @2x RGBA), applied atomically with `set_icon_with_as_template` - separate `set_icon` + `set_icon_as_template` calls visibly flicker. Teardown order on Quit: drop every `TrayIcon` clone (the icon is reference-counted; a surviving clone leaks a ghost icon in the notification area), then `PostQuitMessage`, then tokio shutdown - never `process::exit` ahead of destructors. Tray and menu events forward into tokio via `set_event_handler` + mpsc with a `PostMessage` wake; the pump thread never blocks on HTTP work.
- **macOS process shape:** `LSUIElement` in Info.plist plus an explicit early `setActivationPolicy(.accessory)` before any framework init (Rust GUI frameworks have a documented history of overriding the plist), and the tray is created on the main thread only after the run loop is running. The config SPA binds `127.0.0.1` explicitly - loopback bypasses Local Network Privacy, but a bare `localhost` bind can land on `::1` only and break IPv4 clients.
- Tests: headless CI can't click a tray; gate tray construction behind the flag and test the non-tray path. The workshop-detection predicate (sibling exe probe) is pure path logic and unit-testable without a tray.

### 6. Installer, updater, and docs

Binary renames (land first, they ripple through everything below):
- `gateway.exe` -> `promptforge-gateway.exe` (gateway `[[bin]]` name), `workshop.exe` -> `promptforge-workshop.exe` (workshop bin + `tauri.conf.json` productName/mainBinaryName). The shell launches the gateway by the new name from beside its own executable.
- Release packaging, the `workshop-latest` updater endpoint artifacts, and CI dist wiring (the release profile that turns on the gateway `workshop` feature today) follow the new names.

Componentized installer (the template is already a fully custom [installer.nsi](c:\Users\Vinnie\cursor\promptforge\crates\workshop\installer.nsi), so this is ordinary NSIS section work):
- Add `MUI_PAGE_COMPONENTS` with three sections: **Gateway** (`promptforge-gateway.exe`, arrives as a Tauri `externalBin`), **Workshop** (`promptforge-workshop.exe`, the main binary), **STT** (no files - see below). All three are independently checkable: Gateway-only is the headless server install, Workshop-only is a client that attaches to a gateway over LAN via explicit `workshop.toml` config.
- The template's install/uninstall sections currently key everything off `MAINBINARYNAME`; they are restructured so each executable is installed, checked-at-install (`CheckIfAppIsRunning`), and deleted by its own section. Shortcuts, the finish-page Run checkbox, and the AppUserModelId follow the Workshop section (skipped when Workshop is unchecked).
- **STT is a config gate, not files.** The native runtime is already a separate runtime-loaded artifact: `whisper.dll` and the models download on demand via the artifact store (`provision_whisper_library`, gateway-local). The checkbox writes a registry value under the product key; the gateway's first-run config generation (piece 2) reads it and includes or omits the `[[stt_model]]` entries. Unchecked = STT never provisions, nothing downloads. Bare `gateway.exe` runs outside the installer default to STT on. The installer stays small.
- Component selection is written to the registry at install; update mode (`/UPDATE`, used by tauri-plugin-updater's passive flow) reads it back in `.onInit` and forces section states (`SectionSetFlags`), skipping the components page - passive updates show no UI, so without this an update resurrects declined components. Files of deselected components are explicitly deleted in update mode, since update mode installs over the top without uninstalling.
- The gateway ships via `externalBin` (build staging, target-triple suffix resolution, and code signing come free); the custom NSIS sections gate what actually installs. The custom template must faithfully re-implement the updater contract - `/P`, `/UPDATE`, `/R`, `/ARGS`, `/NS`, `/D` parsing - and track upstream template changes at each Tauri bump, or the updater breaks silently.
- Update mode stops the daemon before overwriting it: the updater's passive install only auto-kills the main binary, so a running `promptforge-gateway.exe` would file-lock the update and fail it. The template reads the pid from `gateway.json` (falling back to process-name lookup), stops the gateway in preinstall, and relaunches it postinstall.
- Linux packaging stays lean by construction: with ksni the .deb declares no appindicator/GTK/libxdo dependencies (session D-Bus only), and the AppImage avoids the libappindicator/GLib symbol-mismatch crash class entirely.
- Root and crate AGENTS.md updates: the product-boundary rule "boots the gateway in-process" becomes "attaches or launches the gateway as a separate process"; the `workshop` feature's removal is recorded; README quick-start changes (new binary names, component choices, headless-server and LAN-client install shapes).

## Steps

Thirteen steps in dependency order; each is the largest slice of behavior one set of tests covers, and each lands as one commit carrying its code and its tests. Component grouping and placement reasons:

- **A. Gateway foundations (steps 1-5)** first, because everything else attaches to or launches the gateway; each step ships independently behind the existing standalone flow.
- **B. The shell (steps 6-8)** next, because it consumes A's connection file and HTTP surface.
- **C. The tray (steps 9-11)** after A (it drives the shutdown/health surface) but independent of B; the three platform backends are separate steps because each has a distinct test surface.
- **D. Installer and docs (steps 12-13)** last, because they package everything above.

1. **binary-rename** - Rename both binaries to `promptforge-gateway` / `promptforge-workshop`: cargo `[[bin]]` names, `tauri.conf.json` productName/mainBinaryName, installer template references, release/CI packaging. The same mechanical sweep applies the shared-crate prefix convention retroactively: `gateway-loopback` -> `shared-loopback` and `gateway-protocol` -> `shared-protocol` (directory, package name, every dependent's Cargo.toml entry and `use` path). Tests: workspace build and `cargo test` stay green; the NSIS bundle builds. (Piece 6, rename bullet.)
2. **connection-file** - A new `shared-sidecar` crate gains the connection-file seam (the house pattern is tiny per-seam shared crates, like `gateway-loopback`; `gateway-protocol` is the OpenAI wire contract and stays out of this): the `gateway.json` types (serialize, deserialize, validate), atomic owner-only write and shutdown removal, stale detection (pid alive + binary identity + health + key) with stale-file cleanup, the `gateway.json.lock` launch lock, and the health wait/probe logic moved from the shell's `health.rs`. serde + thiserror only - no axum, reqwest, or tokio - so the gateway's writer side and the shell/server reader sides share one contract and the lean-build constraint is untouched. Tests: file lifecycle, stale pid, wrong key, lock-race loser attaches. (Piece 1.)
3. **gateway-first-run** - Move default config generation into gateway boot with the new shape (bind `127.0.0.1:0`, no `[workshop]`, `[[stt_model]]` gated on the installer's registry flag, default on elsewhere); the discover.rs tests are rewritten as gateway boot tests; the shell stops generating config. (Piece 2.)
4. **stt-feature** - Re-gate STT from `workshop` to a new default-on `stt` feature: `gateway-stt` and `/v1/audio/transcriptions` move behind `stt`, which joins `default`; `STT_RUNTIME_UNAVAILABLE` is re-keyed to the missing `stt` feature. The `workshop` feature SURVIVES this step as hosting-only (the shell still enables it and boots the merged gateway); removing it is step 7's job, because the shell does not stop consuming it until then - removing it here would break the workshop crate's compile. Tests: feature-matrix checks (`--no-default-features` green with STT stubbed, STT routes present under default), STT route availability. (Piece 3, gateway half.)
5. **gateway-http-surface** - Authenticated key probe (`/v1/whoami` or a reused key-gated route), bearer-authed `POST /shutdown`, Host-header loopback allowlist, one-time redirect URL that sets a cookie and redirects clean. Tests: key accept/reject, shutdown ordering, Host rejection, redirect sets the cookie and strips the key from the URL. (Pieces 1 and 4, gateway halves.)
6. **connection-client** - workshop-server resolves the gateway base-url: connection file first (validate pid + health + key, clean stale files), then explicit `workshop.toml` config. Tests: attach to a live file, stale-file cleanup, config fallback, wrong-key relaunch signal. (Piece 3, workshop-server half.)
7. **shell-hosts-server** - `promptforge-workshop` embeds workshop-server in-process; the window points at the in-process listener with a programmatic exact-port capability and server-set CSP (`connect-src ipc: http://ipc.localhost`); `GatewaySlot` and the in-process gateway boot are removed; window close stops workshop-server only. In the same commit, the gateway's `workshop` feature and its hosting of workshop-server routes are removed (the shell stops consuming them here, so this is the first commit where removal compiles), along with the feature's `open` dependency (the tray re-adds `open` in step 9). Tests: the existing workshop-server integration suite runs against the shell-hosted server unchanged. (Piece 3, shell half.)
8. **attach-launch** - Shell attach-or-launch: `std::process::Command` detached spawn with `CREATE_BREAKAWAY_FROM_JOB`, health wait, Workshop-only fallback to explicit config, tauri-plugin-single-instance, Quit-everything posts `/shutdown`. Tests: launch/attach/race against a fixture gateway; shutdown post; fallback error. (Piece 4.)
9. **tray-abstraction-windows** - Per-OS tray abstraction plus the Windows backend: hidden-window loop on the main thread with tokio on workers, the menu idiom (status label, Workshop, Settings, Launch at Login, Quit), double-click Settings with `with_menu_on_left_click(false)`, ICO, eager Workshop enable via sibling probe, ghost-icon-safe teardown order, polled-`/health` icon state machine. Tests: the sibling probe and state-machine transitions are pure logic, unit-tested; tray construction stays behind the flag for CI. (Piece 5.)
10. **tray-macos** - macOS backend: `LSUIElement` + early accessory activation policy, template icon via `set_icon_with_as_template`, `/usr/bin/open` sibling launch, `SMAppService.mainApp` login toggle read from OS status. Tests: predicates unit-tested; manual checklist on hardware. (Piece 5.)
11. **tray-linux** - Linux backend: ksni with `default-features = false, features = ["async-io"]`, no-watcher fallback (first-run notification + `.desktop` relaunch-opens-Settings), XDG autostart toggle. Also lands the two cross-platform affordances from Piece 5 that no other step covers: `--print-url` (print the Settings URL for tray-less environments) and relaunch-opens-Settings (a second gateway launch detects the live connection file and opens the Settings handoff URL instead of booting a duplicate). Tests: watcher detection, path logic, and the CLI affordances unit-tested. (Piece 5.)
12. **installer-components** - `externalBin` staging, NSIS components page (Gateway/Workshop/STT), per-section install/uninstall keyed off `MAINBINARYNAME` restructure, registry-persisted selection re-applied under `/UPDATE`, deselected-component file deletion in update mode, daemon stop/restart around update, faithful `/P /UPDATE /R /ARGS /NS /D` contract. The uninstaller's Run-key cleanup must name `PromptForgeGateway` (the step-9 value name), not `PromptForge`. Tests: the installer matrix in Verification. (Piece 6.)
13. **docs-ceilings** - AGENTS.md and README updates (product-boundary rule, `workshop` feature removal, new binary names, install shapes, and the recorded convention that every crate shared across products carries the `shared-` prefix, as `shared-progress`, `shared-loopback`, `shared-protocol`, and `shared-sidecar` now do), module ceilings updated with reasons. Final Verify runs the full workspace suite. (Piece 6, docs.)

## Execution

This plan runs on the Full path of [vibe-rulebook.md](c:\Users\Vinnie\cursor\tools-public\rulebooks\vibe-rulebook.md); all Rust work follows [rust-rulebook.md](c:\Users\Vinnie\cursor\tools-public\rulebooks\rust-rulebook.md). Executors read both before starting. This plan is self-contained: every decision made in chat during its design has been folded in (rule 5).

- **Preconditions:** the promptforge worktree is clean and pushed before step 1; a dirty tree stops the run with a report to the user.
- **Step 0 (no commit):** a survey subagent returns the rules manifest - the root AGENTS.md plus every nested AGENTS.md with the directory each governs. Every Coder and Review-and-Fix dispatch carries the manifest paths for the files its step touches, with the instruction to read them first.
- **Per step, in order:** Coder subagent (code + focused tests, then package-scoped `cargo test -p <touched>` and `cargo clippy -p <touched> --all-targets -- -D warnings` - never the workspace suite) -> main stages, Message subagent writes the commit message from the staged diff -> commit -> Review-and-Fix subagent against the diff (fix rounds capped at three, Critical first; it re-runs the package-scoped tests after any fix round, so a dirtied tree needs no separate Verify dispatch) -> amend if review dirtied the tree, re-messaging when the fix changed more than tests -> Verify subagent when scheduled. An open finding of any severity blocks the next step. All subagents are dispatched asynchronously, never synchronously.
- **Verify schedule (lightened):** workspace-wide build + test only at each component boundary (after steps 5, 8, 11), plus the full gate on step 13: `cargo test --workspace`, workspace clippy, `cargo check -p gateway --no-default-features`, and the NSIS bundle build. No every-3rd-step rule. A red scheduled Verify gates the next step; three failed fix rounds stop the run and report the failing signature.
- **Run state:** `vibe-ledger.md` (append-only: step, hash, Verify status, solo decisions with falsifiers) and `vibe-review.md` (open findings) live in `cabinet/_scratch/gateway-sidecar/` in the workspace, outside the promptforge repo so they never dirty its diffs. Frontmatter todos flip to `completed` as each step's commit lands.
- **Decisions:** reversible calls are made and recorded in this plan with falsifiers; hard-to-reverse choices surface to the user before execution (rule 2). Old bugs found mid-run are fixed in their own commits (rule 7).

## Constraints
- `cargo check -p gateway --no-default-features` stays green - STT moves to a default-on `stt` feature, so the lean build stubs it the way it stubs the `workshop` feature today. The gateway gets leaner, never richer.
- The gateway never depends on workshop-server after this plan; the dependency arrow points one way (workshop-server -> gateway client).
- Behavior changes ship with tests in the same commit; module ceilings updated with reasons; one commit per step.
- The standalone `workshop-server` binary keeps working throughout - it is the pattern being promoted.
- The installer stays small - STT remains runtime-downloaded artifacts; the checkbox gates config, not payload.
- New external dependencies are verified against docs.rs before entering any Cargo.toml (identity, release recency, tree depth): `tray-icon` 0.24.x, `muda` at a release containing the #361 fix, `ksni` (Linux only), `objc2-service-management` (macOS only), `open` (already in the tree). No others without a stated reason.
- `unsafe` appears only at the win32/cocoa FFI boundary of the tray backends, each block carrying a `// SAFETY:` invariant - `gateway-whisper-ffi` is the house model (crate-level `unsafe_code = "deny"` with documented opt-in blocks).
- Error design: concrete `thiserror` types in libraries (connection-file parse/validate in the new `shared-sidecar` crate), `anyhow` at binary edges; every new public item documented, with `# Errors` / `# Panics` / `# Safety` where they apply.
- Tests land with the code: unit tests in the file under test, integration tests in the crate's single `tests/it` binary; `cargo fmt --all --check` and package-scoped `cargo clippy -p <touched> --all-targets -- -D warnings` pass before every commit, with lint levels in `[lints]` tables; workspace-wide clippy runs at the component boundaries and the final gate.

## Explicitly out of scope
- Voice/STT migration into workshop-server (design-record endgame; STT stays in the gateway as an ungated capability for now)
- Gateway auto-update (the workshop updater covers the pair)
- The event system (the next plan; its pieces 3-4 assume this topology)

## Verification
- `cargo test` workspace, clippy, `cargo check -p gateway --no-default-features`.
- Manual script: launch workshop with no gateway -> gateway appears in tray, window opens, chat works; close the window -> gateway stays in tray, models stay loaded; relaunch workshop -> instant attach; "Quit everything" from the window menu -> gateway exits via `POST /shutdown`, connection file removed; quit from tray -> connection file removed, next launch cold-starts cleanly.
- Installer matrix: full install (all three checked) -> both exes present, STT config generated, tray Workshop item enabled and launches the shell; STT unchecked -> no `[[stt_model]]` in the generated config, nothing downloads; Gateway-only -> tray server with the Workshop item disabled, no shell, no shortcuts; Workshop-only -> shell attaches to a LAN gateway from explicit config; update over a partial install -> declined components stay declined; update while the gateway is running -> the installer stops the daemon, overwrites both exes, and relaunches it (models reload, connection file is rewritten).

---

## Recovered rationale

Recovered from the producing chat sessions by the plan ledger on 2026-09-04. Everything below this heading is derived annotation, not part of the original plan.

# Plan Enrichment: Gateway Sidecar Decomposition (d86d065f)

Source chats: creator chat c818ea38 (design plus run supervision); run chats faa9793e, e124abfb, 7600b3b0 (Review-and-Fix dispatches for steps 1, 3, 9).

## The why, in the user's words

The plan's shape came from four requirements the user stated verbatim in the creator chat:

- Installer components: "I want the installer to have these options: promptforge-gateway.exe # listed as Gateway / promptforge-worksop # listed as Workshop / gateway-stt # can we make this a separate DLL or separate installable or something?"
- Tray: "The gateway has no window it just has a config SPA on a configured port so when it is installed it needs to have a tray icon. and the tray menu should have Workshop as a menu item and that can be enabled or disabled depending on if the workshop is installed"
- Settings handoff: "double clicking the tray icon or popping the tray's context menu and choosing Settings should bring up the config ui in the default browser"
- Platform reach: "I also want this working smoothly on Linux and Mac."

The emotional core of the thesis (paraphrase): closing the workshop window must not unload hundreds of GB of models, so the gateway has to outlive the window as a tray-resident daemon.

## Discarded alternatives

1. **STT as a separate DLL.** The user's "separate DLL or separate installable or something?" was analyzed three ways: (A) whisper.cpp built as a shared library, runtime-loaded, installed by an NSIS component; (B) an NSIS component that only gates config and provisioning while the STT code stays compiled into gateway.exe; (C) a separate STT installer. The plan landed on B (paraphrase): the heavy part of STT is the models, which are already runtime-downloaded artifacts, so the checkbox gates config, not payload, and the installer stays small. The DLL route was discarded as a real architecture change (shared-lib build, libloading, capability detection) buying little size savings.
2. **One shared crate.** The user asked: "shouldn't we have just ONE crate which is the shared crate between gatway and workshop?" The answer (paraphrase): no - the repo's precedent is tiny-per-seam crates, and gateway-protocol's domain is the LLM upstream API, so stuffing process discovery there muddies it; the seam gets its own crate (shared-sidecar) serving exactly three consumers. But the user decreed the naming rule verbatim: "every shared crate should have "shared-" prefix" - hence the step-1 renames to shared-loopback / shared-protocol and the step-13 recorded convention.
3. **Binary rename reconsidered.** Mid-run the user asked "why are we renaming promptforge-gateway and promptforge-workshop?" The rationale (paraphrase): two generically named exes in one install dir identify nothing in Task Manager, firewall prompts, or logs; the shell launches the gateway by name, so a distinctive name cannot collide with some other "gateway.exe"; and the user had specced the names themselves in the installer message. The user accepted: "seems fine."
4. **First-run browser open.** Option B (auto-open on detected first run) was discarded because the shell's auto-launch of the gateway would produce both a workshop window and a browser tab, forcing a suppression-flag convention between components. Option A - an explicit `--open-settings` flag wired to the installer's finish page - was chosen by the user with "A". Underlying prior-art rule (paraphrase): a daemon never pops a browser on login-triggered starts (Syncthing's most-hated behavior), but "I just installed it" is an explicit user action.
5. **`--no-tray` naming and the service future.** The user asked "should it be called --headless, or --no-desktop instead?" The flag stayed `--no-tray` (paraphrase): it means "no desktop session exists" (CI, Docker, SSH), not "service mode". The user flagged the longer arc: "but I will want to make this an optional service eventually" - the sketched shape is the standard split-process pattern, a headless session-0 service plus a thin per-user tray client attaching over loopback.

## Execution-shape decisions

- Lightened verification, verbatim: "I want a lighter vibe run. We dont need to do whole repo compilation and testing at every step." and later "keep the vibe steps light I dont want to rebuild the world and test the world every time. I want this plan to run as quickly as possible." This is why the Verify schedule only gates at component boundaries (after steps 5, 8, 11) plus the step-13 full gate.
- Commit-message purpose, verbatim: "the purpose of the commit message is so that a human reviewer can tell if a decision was made that adds technical debt" - said while approving the STE100 commit format's tradeoff ("hmm thats actually exactly the right tradeoff").
- One plan, not two. When follow-up work was drafted as two plan files, the user ordered: "I wanted it in one plan not two dumbass."

## Design thinking that outgrew this plan (seeded the follow-up plans)

Late in the run the user articulated the gateway's target architecture, deferred past this plan. Verbatim, abridged: "I don't think there should be ... a boot up... we boot, we read the config file, we bring the whole thing into memory, we parse it... Then now we go to the main event loop. We have our servers running. We're responsive... And after we read the config file, we put a command into the command queue. And that command is load the models... if we switch profiles, that's a command in the command queue... Everything is commands in the command queue. So we always know what's active and what's pending. We can always cancel anything." Supporting constraints, verbatim: "we have to make sure we debounce commands I dont want multuple profile switch commands queued at once" and "design this so we can add user-cancelation of downloads later."

Adjacent deferred decisions:

- Full-async provisioning: "I'm thinking we should go full async though, no?" Adopted (paraphrase) as the structural fix for three observed bugs at once: quit-during-boot hangs, duplicating stdout progress bars, and multi-GB downloads that look hung with no cancel path.
- Shared UI: "I want the UI for the gateway to look identical to the UI for the workshop in terms of elements like buttons, status bars, LEDs, menus, and so on. can we factor out common html/css/typescript elements into a shared-ui crate?" The gateway status bar carries one LED per endpoint, and "the progress bar replaces the LEDs when a command is being processed. otherwise the LEDs are shown" - "Workshop already uses this pattern."
- Keyless loopback: "PROMPTFORGE_GATEWAY_API_KEY should not be required when the incoming connection is on the loopback adaptor." Flagged during planning (paraphrase) as a real trust-boundary tradeoff - any local process, including other OS users on shared machines, could reach admin routes that expose upstream keys - and scoped to the follow-up plan.
- CI cadence: "the llama-cuda build should be manually triggered" because "that binary is mostly for me."
- Icons: "so to be clear, gateway and workshop are both getting the same icon."

## Run chats (deviations check)

The three run chats are Review-and-Fix subagent dispatches (steps 1, 3, 9), not user conversations; they record no user deviations from the plan. What they add is operational intent the plan implies but does not spell out (all paraphrase): step 3's InstallSTT registry DWORD semantics (absent = STT on, zero = omit), first-run config generation must be create-new and never truncate an existing file or follow a planted symlink, and the generated api_key must stay CSPRNG-grade; step 9's ghost-icon-safe teardown order (drop TrayIcon, destroy the hidden window, then shut the gateway down; never process::exit), the `/auth?key=` handoff URL must never be logged or written to disk, muda MenuItems must be created and mutated only on the tray thread, and tray creation failure must degrade to headless serving without losing the connection file or the shutdown signal.
