---
name: Workshop UI overhaul
overview: Remove the layout-shifting voice-status line, upgrade the agent window (thinking block, streaming markdown, tool activity, external links), replace the Windows system title bar with custom PromptForge chrome and functional menus, and add a three-zone workspace with file tree and CodeMirror 6 editor.
todos:
  - id: step-01
    content: Remove the voice-status line; route voice errors to StatusBar.showLocal
    status: pending
  - id: step-02
    content: Open chat links externally (target/rel in sanitizer, shell navigation intercept)
    status: pending
  - id: step-03
    content: Block-memoized streaming markdown with unterminated-construct repair
    status: pending
  - id: step-04
    content: Three-state thinking block (Planning next moves / Thinking, 4-line preview)
    status: pending
  - id: step-05
    content: Tool activity block (one-line autoscroll collapsed, full log expanded)
    status: pending
  - id: step-06
    content: Title-bar markup and window-chrome.ts browser-side wiring
    status: pending
  - id: step-07
    content: Accessible File, Edit, Window, Help menus
    status: pending
  - id: step-08
    content: Title-bar and menu CSS in the :root skin system
    status: pending
  - id: step-09
    content: Decorationless Windows shell with typed wry IPC commands
    status: pending
  - id: step-10
    content: Install the cold medallion as the program icon
    status: pending
  - id: step-11
    content: Confined workspace list/read/write APIs in workspace.rs
    status: pending
  - id: step-12
    content: Native Windows path drops through the shell
    status: pending
  - id: step-13
    content: Zone registry, panel-type registry, and Workshop file-tree panel
    status: pending
  - id: step-14
    content: EditorSurface interface and CodeMirror 6 editor panel
    status: pending
  - id: step-15
    content: Layout boot, lock, persistence, and keyboard shortcuts
    status: pending
  - id: verify
    content: Final full verification (build, tests, manual Windows pass)
    status: pending
isProject: false
---

# PromptForge Workshop UI overhaul

All paths below are relative to `promptforge/` (the repo at `c:\Users\Vinnie\cursor\promptforge`). The UI lives in `crates/promptforge-ws-server/ui/`; the desktop shell is `crates/promptforge-ws/`; the in-process server is `crates/promptforge-ws-server/src/`.

## House rules (binding on every step)

TypeScript (per the workspace TypeScript rulebook):

- Verify `ui/tsconfig.json` carries `strict` and `verbatimModuleSyntax` before writing new modules; add what is missing. Decision (step 3): `noUncheckedIndexedAccess` stays off - enabling it produces dozens of pre-existing errors in untouched files, an unrelated refactor. Falsifier: if a future step touches those files anyway, enable the flag in that step's commit. New modules must be clean under the flag in isolation.
- No `any`. External data (workspace API JSON, native drop events, wry IPC payloads) arrives as `unknown` and is validated or parsed into a narrow type before use. Never `as` on external data.
- No `enum`; use `as const` objects with derived union types.
- No barrel files: `ui/src/workshop/` must not grow an `index.ts`. Import from source files directly.
- Explicit return types on every exported function. No floating promises (`void`-detach with `.catch` or await).
- One concept per file; split past ~300 lines.

Rust (per the workspace Rust rulebook; the workspace already sets `unsafe_code = "forbid"`, `unwrap_used`/`expect_used = "deny"` in libraries, clippy `all` deny + `pedantic` warn):

- New modules are one domain concept per file with a `//!` first line; items default to `pub(crate)`.
- Fallible public functions return a concrete `thiserror` error enum marked `#[non_exhaustive]`, variants named for the operation, messages as lowercase noun phrases with no trailing period. Document with `# Errors`.
- Unit tests live in `#[cfg(test)] mod tests` in the file under test.
- Every change passes `cargo fmt --all --check` and `cargo clippy --all-targets --all-features -- -D warnings` before commit.

HTML/CSS (per the workspace HTML/CSS rulebook):

- Semantic elements: `<header>` for the title bar, real `<button>` for every control, `<nav>`-appropriate menu semantics. No `<div onclick>`.
- Contrast: body text stays at or above 4.5:1. The dim thinking/activity text uses the existing `--text-muted` token (#8b90a0 on #0d0e12, measured 6.0:1) - do not invent a dimmer gray.
- Every `<img>` has `alt` (empty for the decorative title-bar icon) plus `width`/`height`.
- Focus: never `outline: none` without a `:focus-visible` replacement; icon-only buttons carry `aria-label`.
- Animations are compositor-safe: `transform`, `opacity`, or `background-position` only. No layout-property animation. Non-essential motion is wrapped in `@media (prefers-reduced-motion: reduce)`.
- All themed values live as custom properties in the existing `:root` skin block of `ui/style.css` with `var(--name, fallback)` fallbacks.

Execution (per the vibe rulebook): one step = one commit carrying code, test, and docs. Work steps in order; each step dispatches a coder subagent then a review-and-fix subagent. Run Verify on every 3rd step, at the end of each workstream, and on the final step. Keep main context clean; pass findings through scratch files.

## Visual specifications (self-contained)

Custom title bar: a compact near-black (#0d0e12) strip, roughly 40px high, with a one-pixel lower divider in `--border` (#262a33) and a restrained purple accent (`--accent` #7c7fd4) used only for focus/selection states - no glow, no gradient. Left to right: the PromptForge program icon (decorative, noninteractive), then text menu buttons `File`, `Edit`, `Window`, `Help`. The center is empty draggable space. The far-right cluster is exactly three equal-width buttons in Windows order: Minimize (short horizontal stroke), Maximize/Restore (one outlined square normally, two overlapping outlined squares while maximized), Close (thin X at the extreme right). Glyphs are thin muted-gray SVG strokes (`--text-muted`); Minimize and Maximize/Restore get a neutral `--bg-hover` wash on hover; Close gets the standard red hover (`--danger` #b0606a background, white glyph). The browser-hosted Workshop keeps normal browser chrome; the custom bar appears only in the Windows desktop shell.

Thinking block: a quiet single row when collapsed - chevron-right plus the label in `--text-muted` italic. While reasoning streams, the label reads "Planning next moves..." with a shimmer that blends only gray tones (a soft light-gray highlight sweeping across the dim base text via a `background-position` animation; static text under `prefers-reduced-motion`). When open, the thinking text renders as italic `--text-muted` prose inside a subtle container with a soft left edge (a 2px `--border` left border with small left padding), sitting between the label row and the response.

Tool activity block: collapsed, a one-line autoscrolling status window in `--text-muted`; expanded, a static list of single-line rows, each with a status icon (spinner while running, green check `--led-green` #4caf7d when done, red X `--danger-text` #cf7f88 on error), a one-line human summary, and a per-row chevron for arguments/result.

Program icon: `crates/promptforge-ws/assets/icons/promptforge-icon-1.png` (cold medallion) is the static program icon in the title bar and Windows shell. Frames 2-5 (`promptforge-icon-2.png` through `promptforge-icon-5.png`, same directory) are matching heat stages reserved for a future activity animation; this plan installs only the cold frame.

## Workstream A: voice-status removal

Problem: a `div.voice-status` sits below the chat form (created in `ui/src/main.ts` lines 62-66). `showVoiceStatus()` in `ui/src/voice.ts` fills it with messages like "Recording - press the mic button again to stop." The CSS in `ui/style.css` lines 461-478 expands it from `max-height: 0` to 40px plus padding on every appearance, shifting the composer up. Recording state is already indicated by the red mic button (`.voice-mic--recording`) and the status-bar REC badge, so the text line is redundant.

### Step 1: remove the voice-status line

- `ui/src/status-bar.ts`: add a public method so voice errors can paint the bar without an observer frame:

```ts
/** Shows a locally-originated message (e.g. voice capture errors). The next observer frame overwrites it. */
showLocal(label: string, severity: "info" | "error"): void {
  this.text.textContent = label;
  this.root.title = "";
  this.text.classList.toggle("status-bar__text--error", severity === "error");
}
```

- `ui/src/voice.ts`: remove `status` from the `VoiceElements` interface; delete `showVoiceStatus()` and `voiceStatusTimer`. Delete these messages outright: "Recording - press the mic button again to stop." (line 241), "Transcript ready - edit, then send." (line 142), "Recording discarded." (line 288). Route the rest to `statusBar.showLocal(...)`: "No speech detected (N PCM frames captured)." as info; the verbatim server message (line 152), "Voice capture is not available in this browser.", mic permission denied/unavailable (line 188), "The voice connection dropped." (line 229), and "Voice capture failed: ..." (line 252) as error.
- `ui/src/main.ts`: delete the status div creation (lines 62-66), drop `status` from the `setupVoice` call, update the comment on line 43.
- `ui/style.css`: delete `.voice-status`, `.voice-status--visible`, `.voice-status--error` (lines 461-478).
- `ui/test/smoke.mjs`: update the closing comment (lines 791-792) that references the voice-status auto-hide timer. No assertions touch `.voice-status`.
- Test: the existing smoke test must still pass; add an assertion that no `.voice-status` element exists after the voice plugin mounts.

## Workstream B: agent window

Context: the vendored murm-ui (`ui/src/chat/`) already has typed `reasoning` blocks with `reasoning_delta` events, the OpenAI provider mapping `reasoning_content`/`reasoning`/`reasoning_text`, a chevron-collapsible `ThinkingPlugin`, a `ToolsPlugin`, and GFM markdown via `marked`. None of the plugins are registered in `main.ts` (only `voicePlugin` is). This workstream wires them up and upgrades them.

### Step 2: external links

- `ui/src/chat/utils/html.ts`: sanitized anchors gain `target="_blank"` and `rel="noopener"`.
- `crates/promptforge-ws/src/window.rs`: intercept webview navigation to external (non-loopback) URLs and open them in the system browser via the existing `open` crate, so a clicked link never navigates the app away from itself.
- Test: sanitized anchors carry `target`/`rel`; a unit test covers the navigation classifier (loopback allowed, external intercepted).

### Step 3: streaming markdown

- `ui/src/chat/components/message-node.ts`: replace the full-document re-parse every 70ms with block memoization, staying on `marked`. Split incoming text into top-level blocks; cache rendered HTML per completed block; re-parse only the tail block while streaming. Repair unterminated markdown on the tail (open bold/italic, unclosed code fence, incomplete table or link) before parsing so partial constructs never flash broken markup. Keep the existing `renderSafeHTML` sanitizer as the final pass on every block.
- Test: an unclosed code fence and open bold mid-stream render healed; completed blocks are not re-parsed (instrument the parse count).

### Step 4: thinking block

- Register `ThinkingPlugin` in `main.ts` and upgrade `ui/src/chat/plugins/thinking/thinking-plugin.ts` to a three-state block per the visual spec above. The thinking never dumps its full contents into the chat feed by default.
- States: **collapsed** (chevron + "Thinking" label row), **preview** (default while streaming: a fixed-height region capped at roughly four lines; beyond that it scrolls internally, pinned to the newest line, so the feed footprint never grows; user scroll-up inside the preview disengages auto-pin until they scroll back to the bottom), **expanded** (full thinking via the chevron toggle).
- Lifecycle: auto-opens into preview on the first reasoning delta; auto-collapses once to the label row on the first content token; a manual toggle is sticky and overrides auto behavior for that message. Scroll-lock during the collapse animation so the feed does not jump. Grace-period loader: when streaming starts but no block has arrived, show the synthetic "Planning next moves..." row only after ~500ms to prevent flicker on fast responses.
- Accessibility: real `<button>` with `aria-expanded`/`aria-controls`; announce state transitions only, never per-token text.
- Test: grace loader timing, preview opens on first delta, four-line cap with internal scroll, auto-pin disengage/re-engage, auto-collapse on first content token, sticky manual toggle, full expansion.

### Step 5: tool activity block

- Register `ToolsPlugin` in `main.ts` and restyle `ui/src/chat/plugins/tools/tools-plugin.ts` per the visual spec. Two states: **collapsed while working** (the one-line autoscrolling status window: each new activity line appears in the single row and the previous line scrolls up and out via a compositor-safe transform animation; height never changes; on completion the animation stops and a summary line such as "5 actions completed" rests in the row) and **expanded** (the complete preserved activity log as static single-line rows with status icons and per-row chevrons).
- Consecutive activity rows belong to one collapsible block per agent run, so a run reads as a single unit in the feed. Collapsing hides history; it never discards it.
- Test: while working, the collapsed row shows each new line and scrolls the previous out at constant height; expanding reveals the full log; completion rests the summary; three consecutive calls fold into one block per run.

## Workstream C: custom Windows title bar

### Step 6: title-bar markup and browser wiring

- `ui/index.html`: add a semantic `<header class="window-titlebar">` before `.shell`, hidden by default, containing the decorative program icon `<img>` (with `alt=""`, `width`, `height`), the four menu buttons, the draggable center region, and the three window controls - every control a real `<button>` with an accessible label and inline SVG (no icon dependency).
- New module `ui/src/window-chrome.ts`: reveals the bar only when the wry initialization flag `window.__PROMPTFORGE_DESKTOP__` is present; sends only typed window commands through `window.ipc.postMessage(...)` for drag, minimize, maximize/restore, and close; starts native dragging on primary-button `pointerdown` in the empty center; toggles maximize on double-click; listens for a native `promptforge:maximized` event so the maximize glyph and `aria-label` switch to Restore after button maximize, double-click, Windows Snap, or restore. IPC payloads are parsed and validated into a narrow command type, never cast.
- Test: smoke test asserts the icon, four menu buttons, and three controls exist, stay hidden in browser mode, and do not require `window.ipc` when inactive.

### Step 7: application menus

- New module `ui/src/window-menu.ts`: accessible HTML popovers inside the title bar, not native menus. Each top-level button opens on click; Left/Right moves between menus; Up/Down moves through commands; Enter activates; Escape and outside-click dismiss. Opening a second menu replaces the first; disabled commands are announced and cannot run; popovers position below the bar without changing layout.
- Commands: **File** - New Chat (`chat.engine.sessions.create()`), separator, Close Window (typed Close IPC). **Edit** - Undo, Redo, Cut, Copy, Paste, Select All, preserving the previously focused editable target and disabling commands with no valid target. **Window** - Minimize, Maximize/Restore, reusing the same command functions as the visible controls. **Help** - About PromptForge, a small themed modal (product name, application version, license) with focus trapping and Escape dismissal.
- Expose the initialized `ChatUI` instance to the menu setup from `ui/src/main.ts` rather than querying internal chat DOM. All command dispatch lives in this one module so keyboard shortcuts and future menu items call the same actions.
- Test: menu opening, one-menu-at-a-time, keyboard navigation/dismissal, New Chat dispatch, shared Window commands, About-dialog focus/close.

### Step 8: title-bar CSS

- `ui/style.css`: add title-bar variables (`--titlebar-height`, control width, foreground, hover, divider, purple accent) to the `:root` skin block and implement the visual spec. Buttons and drag space use `user-select: none`; only the empty center initiates drag. Menu popovers are slightly raised near-black surfaces (`--bg-raised`) with compact rows, subtle `--border` borders, shortcut columns, and purple focus/selection accents. The existing column flex layout absorbs the fixed-height row without overlap. Window control hit areas are at least 24px high (the ~40px bar satisfies this).
- Test: visual/contract assertions for bar visibility gating and control order (Minimize, Maximize/Restore, Close). Revision (landed in step 8's commit): the assertions live in `ui/test/titlebar-style.mjs`, a dedicated contract test wired into `npm test`, not the smoke test - the style contract scans the built `dist/style.css` rule by rule, a separate concern from the jsdom smoke flow, and smoke.mjs already carries the step-6 markup gating/order assertions.

### Step 9: decorationless Windows shell and IPC

- `crates/promptforge-ws/src/window.rs`: on Windows, create the tao window with `.with_decorations(false)`; macOS and Linux keep the native frame. Configure wry with an initialization script setting `window.__PROMPTFORGE_DESKTOP__ = true`, then install `with_ipc_handler` to parse a narrow JSON command enum. The IPC callback does not own the tao `Window`, so it forwards commands through `EventLoopProxy<WindowCommand>` to the tao event loop:

```text
HTML button -> window.ipc.postMessage -> validated WindowCommand
            -> EventLoopProxy -> tao event loop -> Window method
```

- The event loop handles `Drag` with `window.drag_window()`, `Minimize` with `window.set_minimized(true)`, `ToggleMaximize` with `window.set_maximized(!window.is_maximized())`, and `Close` by exiting the loop so the existing server shutdown path still runs. On resize/state changes it calls `webview.evaluate_script(...)` to dispatch the maximized-state event. Unknown or malformed IPC is ignored, never treated as a window command.
- Test: unit coverage (in-file) for the command parser - malformed JSON and unknown actions cannot invoke native operations. Manual: native drag, double-click maximize/restore, minimize, close, Windows Snap, edge resizing, glyph synchronization, focus rings, and that Close still returns from `window::run()` and shuts down the in-process server.

### Step 10: program icon

- Install `crates/promptforge-ws/assets/icons/promptforge-icon-1.png` as the title-bar icon (referenced from the markup in step 6) and as the tao window icon via `WindowBuilder::with_window_icon` (decode the PNG once at startup; on failure, log and continue without an icon rather than failing startup). Revision (landed in step 6's commit): the web-served half of this install shipped early - step 6's markup references the icon, so that commit copied the cold frame to `crates/promptforge-ws-server/ui/icons/promptforge-icon-1.png`, serves it at `/icons/promptforge-icon-1.png` from `app.rs`, and mirrors it in both static-copy lists. What remains here is only the tao window icon.
- Test: smoke assertion that the title-bar img resolves; Rust unit test that the icon decoder accepts the bundled asset.

## Workstream D: workspace, file tree, and editor

Product shape: three fixed named zones - **left** (workspace/file tree), **main** (document editors), **right** (agent/chat panels). Each zone is a Dockview group (a tabbed bank). Panels drag freely between zones, but each panel type has a declared affinity: new documents open in `main`, new chats in `right`, the tree lives in `left`. Reserve the `bottom` zone name for later. Placement resolution for a new panel: persisted per-panel override, then type default. Moving a panel writes an override; moving it back to its default zone deletes the override. A single layout lock freezes user rearrangement but never blocks app placement. Dockview 8 (the incumbent) provides every primitive: groups as zones, `addPanel({ position: { referenceGroup } })` for affinity, `group.locked` for locking, `toJSON()`/`fromJSON()` for persistence.

### Step 11: confined workspace APIs

- New module `crates/promptforge-ws-server/src/workspace.rs`, rooted only at paths explicitly granted through Windows drag/drop. A dropped folder becomes an allowed root; a dropped file grants its parent directory for that session. Grants live in memory for the running process; profile persistence is a separate future consent decision.
- Routes beside the existing ones in `app.rs`: `GET /workspace/tree?path=...` (one level, directories before files, stable ordering, tree metadata), `GET /workspace/file?path=...` (UTF-8 text, size, modified time; rejects binary or oversized files), `PUT /workspace/file` (writes only after validating path, size, and expected modified-time token).
- All paths are canonicalized and confined to granted roots before any filesystem operation. Reject `..`, symlink escapes, UNC tricks, alternate data streams, and paths outside every grant. Pattern the jail on the gateway's existing `confine.rs` approach. Errors use a concrete `thiserror` enum per the house rules.
- Test (in-file): canonical path confinement, symlink escape rejection, binary rejection, size limits, modified-time conflict handling, and successful read/write inside a granted root, using `tempfile::TempDir`.

### Step 12: native path drops

- `crates/promptforge-ws/src/window.rs`: use wry's file-drop handling so dragging files or folders from Explorer delivers real OS paths. Translate native drop events into a typed browser event containing normalized paths; the page then calls the workspace HTTP APIs. The browser fallback can still receive file contents through normal HTML drag/drop, but desktop mode prefers trusted paths and never reads file bytes merely because a file was dragged.
- Test: unit-test the path normalization (Windows backslashes, spaces, Unicode); smoke-test that a synthesized drop event reaches the page handler with paths only.

### Step 13: zone registry and Workshop tree panel

- New directory `ui/src/workshop/`, one file per concern, no barrel:
  - `zones.ts` - the zone registry: zone name -> group id, `openInZone(type, params)`, affinity table, override map. The only module that talks to Dockview placement APIs. Rebuilds a zone group if the user closes every panel in it.
  - `panel-types.ts` - the static registry: `{ type, defaultZone, title, factory }` per panel kind (`tree`, `editor`, `chat`) as an `as const` structure.
  - `workshop-panel.ts` - the file tree: loads one directory at a time, folders before files, preserves expansion state per session, opens a file on activation via `openInZone("editor", ...)`. API responses are validated at the boundary.
- Register real Dockview component names instead of the current factory that always returns `ChatPanel`.
- Test: smoke assertions that Chat and Workshop mount, the tree requests paths rather than file contents, and `openInZone` places panels by affinity and honors overrides.

### Step 14: CodeMirror 6 editor panel

- `ui/src/workshop/editor-surface.ts` - the internal `EditorSurface` interface (`open`, `save`, `isDirty`, `focus`) plus the CodeMirror 6 implementation. Nothing else in the app imports `@codemirror/*` directly. Initial extension set: `basic-setup`, a dark theme built from the PromptForge palette tokens, default keymap plus history, `@codemirror/search` for in-file find/replace, and language modes loaded lazily by file extension (first-party packs: JavaScript/TypeScript, Python, Rust, JSON, Markdown, YAML; TOML via legacy-modes). Dirty tracking is an `updateListener` comparing against the saved document. Revision (found in step 14's review): the language modes are not truly lazy - the single-file esbuild bundle (`ui/build.mjs`, no code splitting) inlines the dynamic `import()`s, so every first-party pack ships in the bundle; the `import()` structure only keeps the load boundary explicit. Enabling real laziness means turning on esbuild code splitting, a build change deferred out of this plan.
- `ui/src/workshop/editor-panel.ts` - one open document per panel, written against `EditorSurface`; panel, zone, and save logic never touch the concrete editor. Dirty state shows in the panel title; save runs through the workspace API and refuses to silently overwrite a file whose modified-time token changed on disk (conflict dialog: reload or overwrite).
- Test: opening a file creates an editor panel, typing sets dirty state, save clears it, and a stale modified-time token surfaces the conflict dialog.

### Step 15: layout boot, lock, persistence, shortcuts

- `ui/src/main.ts` and `ui/style.css`: replace the hardcoded single-panel setup with the component registry and a default three-zone layout (tree left, chat right, `main` empty until a document opens). Keep `locked: true` initially; the Window menu and a lock control on each zone header call `dock.updateOptions({ locked })`. Unlocking reveals the tab/drop affordances currently hidden by CSS.
- The status bar is not part of the zone system. It is a direct child of `body` (outside `.shell` and outside the Dockview dock), a permanent full-width footer pinned by the body's flex column (`.shell` takes `flex: 1`, the footer is `flex: none`). It must never become a Dockview panel, never participate in layout serialization, and never be dragged into a zone. The body column order is: title bar, `.shell` (dock), status bar.
- Persist `{ version, locked, zones, overrides }` via `onDidLayoutChange` -> `toJSON()` into localStorage, debounced, with a schema version from day one. On boot, register all factories first, then `fromJSON()`; on any restore failure, fall back to the known-good default layout. Store identity, not DOM - panels re-create through their factories on load.
- Keyboard shortcuts: one app-level keydown listener dispatching to the shared command functions - **Ctrl+S** save active editor, **Ctrl+W** close active editor panel (prompt on dirty), **Ctrl+B** toggle the Workshop panel, **Ctrl+Tab / Ctrl+Shift+Tab** cycle editor panels, **Ctrl+Shift+F** focus the Workshop tree. No keybinding customization, no chords, no conflicts UI. Typing, selection, clipboard, undo/redo, and in-file find/replace come from CodeMirror's built-ins; workspace-wide search stays deferred.
- Test: lock starts engaged, unlocking enables movement, layout survives a reload, restore failure falls back to defaults, and each shortcut dispatches its command.

## Data flow and sequencing review

The four workstreams are independent until verification: voice errors flow into `StatusBar`; agent-window work touches only `ui/src/chat/`; title-bar actions flow from DOM controls through a strict IPC enum into tao; workspace work flows from native drops through confined HTTP APIs into Dockview panels. Within workstream C the order is markup, menus, CSS, then shell IPC because each supplies the contract the next consumes. Within workstream D the order is server API, drops, zones/tree, editor, then layout boot, for the same reason. Steps 2-5 (workstream B) can run in parallel with A, C, and D. No step combines unrelated responsibilities, and the existing server lifecycle remains the single owner of shutdown.

## Verification schedule

- Verify after steps 3, 6, 9, 12, and 15 (every 3rd step), at the end of each workstream, and as the final gate.
- Final gate: `cargo build -p promptforge-ws-server` (build.rs rebuilds `ui/dist/` via esbuild), `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --locked --workspace`, `node ui/test/smoke.mjs`, and a manual Windows pass: mic recording shifts nothing, links open externally, thinking preview caps at four lines, activity row autoscrolls, title bar drags/minimizes/maximizes/closes, menus navigate by keyboard, a dropped folder browses and edits, and Close shuts the server down cleanly.


---

## Recovered rationale

Recovered from the producing chat sessions by the plan ledger on 2026-09-04. Everything below this heading is derived annotation, not part of the original plan.

# Enrichment: remove_voice_status_line_3a505587 (Workshop UI overhaul)

## Origin and rationale (creator chat, Aug 26-27 2026)

The plan grew out of a single irritation and expanded by accretion. The seed: "there's a little piece of text below the edit box that shows like when the mic is recording... I wanna remove it, it's super annoying, 'cause when it appears, it shifts everything up." The layout shift, not the text itself, was the offense; the plan's redundancy argument (red mic button, REC badge) was the assistant's later justification for deleting rather than restyling.

The custom title bar was an aesthetic demand, non-negotiable: "I don't want the system menu bar. That shit's ugly as fuck. I want a nice looking menu bar." The user supplied reference images and asked the agent to describe them back, explicitly "to prove that you know how the fuck to do it" - the visual-spec section of the plan exists because the user wanted evidence of competence before commitment, not because the design needed recording.

Discarded alternative in the chrome design: early mockups included browser-style navigation. Rejected verbatim: "No back/forward buttons. Just program icon File Edit ... and then the proper icons on the top right." Windows ordering of the three controls was dictated by the user with a reference image.

## Program icon

The user art-directed multiple rounds of generated concept sheets (20 icons per round). Brief: "I want forge imagery. hammer. anvil. furnace. something like that. make it legible... at small icon size." Themes explored and discarded: hammer-and-anvil (a 5-frame hammer-strike animation with sparks was designed then dropped), furnace, letter-P logos, a chevron mark. Survivor: "I want Medallion Heat to be the icon for the program" - a P-in-circle medallion rendered cold-to-molten in 5 frames. The decision to ship only the cold frame and reserve frames 2-5 for a future activity animation is the user's scoping, not a technical limitation.

## Workspace, zones, and the lock reversal

The three-zone layout is the user's own architecture, dictated nearly in final form: "the workspace zone on the left... the editor zone in the middle... the agent zone on the right. And each zone is a bank of tabbed windows, and you can freely move windows from one place to another, but each zone has a preference." Affinity ("whenever a new document is created, it goes to the zone where it has affinity") is the user's word and concept.

Notably, the lock was also the user's idea ("there should be a little lock icon... if you unlock it, then it becomes movable"), and the user later reversed it after using the build: "Completely remove the lock/unlock feature, including the icon and the code to make the dividers lockable. They should always behave unlocked." The plan file still specifies a lock in step 15; the chat supersedes it. Same for the model selector: "Model should be a top level menu in the menu bar instead of a drop down in its own panel" - the plan's File/Edit/Window/Help set predates this.

Component strategy was buy-not-build, user-directed: "Wouldn't it be easier for us to just download an existing component... Present me with several options, preferably projects that I can actually try out, and I'll tell you what feels right, and then we'll just adopt it." Surveys of editor components and agentic harnesses were commissioned; Dockview (already vendored) and CodeMirror 6 won. The modularity demand is a standing fear of generated-code sprawl: "I want each piece of code to be very well isolated... I don't want AI code bloat."

Drag-and-drop paths-only semantics are a user security instinct, stated before any design existed: "when you drag, it should deliver the path to the UI, not the file itself."

## Agent window details

Thinking block: the four-line cap is because "nobody wants a default where all the thinking just scrolls the window endlessly." Labels were dictated: "I want it to say Planning next moves... and then Thinking >", later refined: "Just 'Planning next moves' during prefill and then 'Thinking' while thinking tokens are streaming in", and "'Planning next moves' should not have elipses at the end." Two post-plan corrections the file does not reflect: retention - "the thinking disappears after its done. I want it to stay just like cursor so it can be audited" - and typography - "no italics even for thinking prose" (the plan specifies italic thinking prose; the chat overrides it).

Tool activity block: modeled explicitly on Cursor's collapsed behavior, user-described from screenshots: "In essence a 1-line window that autoscrolls."

Status bar placement was a one-line user ruling during plan review: "the status bar stays fixed to the bottom though and it has no parent" - the source of step 15's never-a-Dockview-panel rule.

## Run chat deviations (step 13 execution only)

The run transcript covers only the step-13 coder subagent. Deviations from the plan as written:

- `ChatPanel` was moved out of `main.ts` into a new `ui/src/workshop/chat-panel.ts`; the plan named only three workshop files and did not say where the chat panel lives.
- A fourth module, `workspace-api.ts` (validated fetch boundary), was added; the plan listed `zones.ts`, `panel-types.ts`, `workshop-panel.ts` only.
- The placeholder `editor-panel.ts` was created a step early, behind the registry, so step 14 could slot in (sanctioned by the dispatch prompt).
- Tree expansion state was kept at module level (app session) rather than per panel instance, so closing and reopening the Workshop tab preserves expansion.
- A deferred-use ESM import cycle (zones -> panel-types -> workshop-panel -> zones) was accepted deliberately after analysis showed all cross-uses are runtime-deferred; the `ZoneName` type was homed in `panel-types.ts` to eliminate the type-level cycle.
- `createComponent` returns a placeholder renderer for unknown panel names instead of throwing, to avoid breaking Dockview on a bad name.

Process note: the subagent's shell session wedged on a piped command mid-task; verification was completed in fresh shell sessions. No plan content impact.
