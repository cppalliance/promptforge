---
name: Desktop shell review follow-up
overview: File the completed code review of promptforge-desktop-shell as an output report, then apply the three worthwhile fixes (origin-pinned navigation, maximized-dispatch dedup, proxy consolidation) with tests.
todos:
  - id: file-report
    content: Write review report to cabinet/_output/review-promptforge-desktop-shell.md
    status: completed
  - id: pin-origin
    content: Pin in-place navigation to the server origin with updated tests
    status: completed
  - id: dedup-maximized
    content: Dispatch promptforge:maximized only on state transitions
    status: completed
  - id: consolidate-proxies
    content: Replace the three EventLoopProxy instances with clones of one
    status: completed
  - id: verify
    content: Run cargo test + clippy; manual edge-resize check
    status: completed
isProject: false
---

# Desktop Shell Review Follow-Up

## Context

Code review of [promptforge-desktop-shell](promptforge/crates/promptforge-desktop-shell) is complete (findings delivered in chat). The crate is in good shape; three findings are worth acting on.

## Step 1: File the review report

Write the full review (strengths, findings 1-5, verdict) to `cabinet/_output/review-promptforge-desktop-shell.md`. No YAML frontmatter; date/time/model line in italics at the bottom per cabinet convention. Check for existing `review-promptforge-desktop-shell*` files first and use a numeric suffix if present.

## Step 2: Pin in-place navigation to the server origin (Finding 1, low security)

In [window.rs](promptforge/crates/promptforge-desktop-shell/src/window.rs):

- Parse the `url` argument of `run` once into a `(scheme, host, port)` origin tuple.
- Change `classify_navigation` to take the allowed origin and return `Allow` only for an exact scheme+host+port match (keeping the loopback classification as a fallback only if origin parsing fails, or fail closed - decide at implementation time; failing closed is safer since `run` already requires a valid URL from the caller).
- Update the navigation handler closure to capture the origin.
- Update existing tests (`loopback_urls_load_in_the_webview`, `external_urls_open_in_the_system_browser`) and add: same host different port is denied, same origin is allowed, `localhost.evil.example` stays denied.

## Step 3: Dispatch `promptforge:maximized` only on transitions (Finding 2)

In the `run_return` closure in [window.rs](promptforge/crates/promptforge-desktop-shell/src/window.rs), keep a `mut last_maximized: Option<bool>` and call `dispatch_maximized` only when `window.is_maximized()` differs from the last dispatched value.

## Step 4: Consolidate the three event-loop proxies (Finding 4)

In `run` in [window.rs](promptforge/crates/promptforge-desktop-shell/src/window.rs), `proxy`, `navigation_proxy`, and the cfg-gated `drop_proxy` are three independent `EventLoopProxy` handles to the same loop. Create one `proxy` after the event loop is built and hand out `proxy.clone()` to the navigation handler and to each cfg branch's drop handler. Behavior is identical (a proxy is a cheap clonable handle); this removes the implication that the three channels differ.

## Step 5: Verify

- `cargo test -p promptforge-desktop-shell`
- `cargo clippy -p promptforge-desktop-shell --all-targets` (crate denies `unwrap_used`/`expect_used` and warns pedantic)
- Manual check on Windows: window edge-resize works with decorations off (Finding 5).

## Explicitly out of scope

- The `with_capacity` nit, `Volume{GUID}` spelling, and `eprintln` logging - documented in the report, not worth churn.
- No restructuring of `file_drop.rs` per the crate's AGENTS.md.


---

## Recovered rationale

Recovered from the producing chat sessions by the plan ledger on 2026-09-04. Everything below this heading is derived annotation, not part of the original plan.

# Enrichment: Desktop shell review follow-up

Source: creator chat "code review promptforge-desktop-shell" (Aug 31, 2026).

## Why this plan exists

The user asked for a code review of `promptforge-desktop-shell`, nothing more. The review (delivered in chat) found the crate strong overall, with five findings. The plan packages the follow-through: file the review as a durable report, fix the three findings judged worth acting on, and verify. The plan is deliberately proportional - the crate was in good shape, so the follow-up is three surgical fixes, not a rework.

## Why these three fixes and not the others

- **Finding 1 (origin pinning)** was the only security item. The reviewer verified that `classify_navigation` allowed any loopback origin on any port to load in the webview, and that a page loaded that way inherits the wry IPC bridge (window drag/minimize/maximize/close), the `workspace-pick-folder` channel (can prompt the user and receive the chosen path back), and the `__PROMPTFORGE_DESKTOP__` flag. Severity was rated low because the attack surface is local-only and the shell never reads file bytes, but the fix was cheap, so it made the cut. The plan's "fail closed vs. loopback fallback" choice was explicitly deferred to implementation time, with the reviewer noting failing closed is safer because `run` already requires a valid URL from the caller.
- **Finding 2 (maximized dedup)** was a minor performance item: `dispatch_maximized` evaluated a script in the webview on every resize event during a drag-resize, though the flag almost never changes.
- **Finding 4 (proxy consolidation)** was originally classified as a nit and left out of scope. The user pulled it in with the decisive sentence: "we are touching the code anyway and three of something duplicated sounds like waste". The fix is behavior-identical (a proxy is a cheap clonable handle); the point is removing the false impression that the three handles are different channels.

The remaining findings were consciously excluded as "not worth churn": the tao `EventLoopBuilder::build()` panic-vs-error doc nit (verified against the tao 0.36 registry source; practically unreachable), the `with_capacity` nit (bounded by real attachments), the `\\?\Volume{GUID}\` spelling nit (rare), and the `eprintln` logging note (only matters if the binary ever flips to `windows_subsystem = "windows"`). Restructuring `file_drop.rs` was excluded per the crate's own AGENTS.md.

## Why Step 1 (the review report) is in the plan

The user challenged it directly: "why are we getting a review report". The answer: the workspace's cabinet convention classifies a code review as an output artifact (it draws conclusions from evidence), so it gets filed to stand alone for a reader later. The reviewer noted it was not a requirement since the review was already delivered in chat. The user chose to keep the step.

## Finding 5 and the Groupy discovery (context for the verify step)

Finding 5 was a low-confidence verification item: whether edge-resize works on Windows with decorations off depends on tao 0.36's borderless hit-testing, which could not be checked statically. It proved prescient. Later in the same chat the user reported: "its like impossible to resize from the top edge of the window because, not sure why but I think its because of this app I have installed called Groupy on Windows". Investigation found the root cause: raw tao `with_decorations(false)` sets `WS_POPUP` (truly frameless, userland `WM_NCHITTEST`), while Groupy hooks the same top-edge strip and wins the hit-test race. Unsloth Studio (a Tauri v2 app) manages it correctly because Tauri uses `DwmExtendFrameIntoClientArea` - the window looks frameless but the DWM still owns the edges, so Groupy coexists with the OS resize grips.

## Discarded alternatives

Three options were weighed for the resize conflict: (1) exclude PromptForge from Groupy - zero code, the practical immediate answer; (2) call `DwmExtendFrameIntoClientArea` manually on the tao HWND - moderate effort, Windows-only cfg code; (3) migrate the shell to Tauri - gets DWM frame integration for free plus window-state persistence, auto-updater, single-instance, deep linking, notifications, tray, and installer bundling, at the cost of deleting the hand-built shell (event loop, JSON IPC protocol, navigation classifier, evaluate_script dispatch, PNG icon decoding, the 265-line unsafe COM file-drop bridge). No resize code fix appears in this plan because the user ultimately chose option 3: the Tauri migration was planned and executed as a separate, larger effort later in the same chat, superseding any patch to the shell's windowing. The fixes in this plan (navigation pinning, dedup, proxy consolidation) were still worth landing because they are independent of the windowing layer and the migration's fate was not yet decided when this plan was written.
