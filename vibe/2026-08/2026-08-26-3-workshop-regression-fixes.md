---
name: Workshop regression fixes
overview: "Fix the seven reported regressions: missing thinking UI (server drops reasoning tokens plus a prefill render race), broken panel dragging (wry's native drop handler kills HTML5 drag-and-drop in WebView2), missing/odd-looking tabs, the closable-and-unrecoverable Workspace panel, the duplicate New Chat command, and the hidden mic (non-CUDA build)."
todos:
  - id: reasoning-relay
    content: "Forward reasoning tokens: server reasoning frames, socket callback, provider reasoning_delta"
    status: completed
  - id: prefill-race
    content: Fix Planning next moves prefill race and target the generating message
    status: completed
  - id: drag-fix
    content: Replace wry drop handler with delegating IDropTarget to restore Dockview drag
    status: completed
  - id: tabs-workspace
    content: Normal chip tabs, non-closable tree tab, boot ensureTree, Window menu item
    status: completed
  - id: remove-new-chat
    content: Remove File > New Chat and AgentController.newChat
    status: completed
  - id: cuda-default
    content: Make cuda a default feature and skip voice init on CPU builds
    status: completed
  - id: verify
    content: Run UI tests, typecheck, fmt, clippy, workspace tests, and manual smoke
    status: completed
isProject: false
---

# Workshop Regression Fixes

## Root causes found

- **No "Planning next moves" / "Thinking"**: two independent bugs. (a) [crates/promptforge-ws-server/src/chat_ws.rs](crates/promptforge-ws-server/src/chat_ws.rs) relays only `choices[0].delta.content`; `reasoning_content` tokens from the gateway are dropped, so [workshop-provider.ts](crates/promptforge-ws-server/ui/src/workshop-provider.ts) never emits `reasoning_delta` and the Thinking block never exists. (b) The prefill row in [thinking-plugin.ts](crates/promptforge-ws-server/ui/src/chat/plugins/thinking/thinking-plugin.ts) fires on a selector `onChange` which the store notifies before the hot render creates `.mur-message-assistant`, so "Planning next moves" silently never attaches on a first reply.
- **Nothing is draggable**: [crates/promptforge-ws/src/window.rs](crates/promptforge-ws/src/window.rs) uses wry's `with_drag_drop_handler`. On Windows, wry implements this by calling `RevokeDragDrop` on the WebView2 child window and registering its own `IDropTarget` (verified in wry 0.56.1 source), which disables all HTML5 drag-and-drop inside the page. Dockview tabs use HTML5 drag-and-drop, so panel dragging is dead.
- **No rendered tab / wrong-looking titles**: `singleTabMode: "fullwidth"` in [main.ts](crates/promptforge-ws-server/ui/src/main.ts) stretches a lone tab into a title-bar-like strip; a closed tree zone leaves a bare empty header.
- **Workspace panel closable and unrecoverable**: the tree tab has a default close X, boot only re-ensures Agent panels (`agents.ensureAgent()`), and no menu item reopens the tree.
- **Mic hidden**: the running binary was built without `--features cuda`, so `/voice/capability` reports gpu=false and the mic hides by design. The server still wastefully loads ~4.7 GB of whisper models on CPU and shows "Voice ready".

## Step 1: Forward reasoning tokens end to end

- `chat_ws.rs`: extract `delta.reasoning_content` (plus `reasoning`/`thinking` synonyms, mirroring `extract_reasoning` in [crates/promptforge-core/src/normalize.rs](crates/promptforge-core/src/normalize.rs)) and emit a new `{"type":"reasoning","content":...}` frame alongside `delta`.
- [workshop-socket.ts](crates/promptforge-ws-server/ui/src/workshop-socket.ts): surface reasoning frames to a per-request callback.
- `workshop-provider.ts`: emit `reasoning_delta` stream events with a dedicated block id; the existing `ThinkingPlugin.onBlockRender` and stream reducer already turn those into the collapsible Thinking block.
- Rust + UI tests for the new frame.

## Step 2: Fix the prefill race

- In `thinking-plugin.ts`, attach the "Planning next moves" row after the feed's hot render has created the assistant message element (defer via `requestAnimationFrame`/microtask with a retry, or subscribe hot) and target the message element for `generatingMessageId` specifically, not "last assistant".
- Extend [ui/test/thinking-block.mjs](crates/promptforge-ws-server/ui/test/thinking-block.mjs): prefill must appear on a first-ever reply and must attach to the generating message, not a previous turn.

## Step 3: Restore Dockview dragging (desktop shell)

- Replace wry's `with_drag_drop_handler` in `window.rs` with a delegating `IDropTarget` registered by promptforge-ws itself: enumerate the WebView2 child HWNDs, capture the existing drop target (OLE stores it in the `OleDropTargetInterface` window property), revoke, then register a wrapper that forwards every `DragEnter`/`DragOver`/`DragLeave`/`Drop` call to the original target (keeping HTML5 drag-and-drop alive) while also extracting `CF_HDROP` paths on `Drop` and sending `ShellEvent::FileDrop` as today.
- If the original target cannot be captured, fall back to registering nothing (drag works, Explorer drops degrade gracefully) and log it.
- Adds a direct `windows` crate dependency to promptforge-ws. Manual verification: drag an Explorer folder (grant still lands) and drag a Dockview tab between zones.

## Step 4: Real tabs and Workspace panel protection

- Remove `singleTabMode: "fullwidth"` from the dock options in `main.ts` so every panel gets a normal chip tab (fixes "no rendered tab" and the odd title strip).
- Give the tree panel a custom `tabComponent` (dockview `ITabRenderer`) with no close button; register it in [panel-types.ts](crates/promptforge-ws-server/ui/src/workshop/panel-types.ts).
- Boot: after `restoreLayout`, ensure the tree panel exists (mirror of `agents.ensureAgent()`), so a stale persisted layout can never boot without the Workspace panel.
- Add a Window menu item "Workshop Panel" (Ctrl+B) in [window-menu.ts](crates/promptforge-ws-server/ui/src/window-menu.ts) that toggles/focuses the tree via the existing `toggleWorkshopPanel` command from [shortcuts.ts](crates/promptforge-ws-server/ui/src/workshop/shortcuts.ts).
- Update zones/layout/menu tests.

## Step 5: Remove New Chat

- Delete the File > New Chat item from `window-menu.ts` and `AgentController.newChat()` from [agent-controller.ts](crates/promptforge-ws-server/ui/src/workshop/agent-controller.ts). New Agent is the only way to start a fresh conversation.
- Update `window-menu.mjs` and `agent-controller.mjs` tests.

## Step 6: CUDA by default and honest voice state

- [crates/promptforge-ws/Cargo.toml](crates/promptforge-ws/Cargo.toml): `default = ["cuda"]` (already forwards to `promptforge-ws-server/cuda`). Note: building the desktop app now requires the NVIDIA CUDA toolkit; other crates and CI paths that build the workspace default features will too - I will verify `cargo test --workspace` still passes and flag if the toolkit is missing.
- Server: when `gpu_transcription_available()` is false, skip whisper model loading entirely and suppress the "Voice ready" status and REC indicator, so a CPU build never half-initializes voice.

## Step 7: Verification gates

- `npm test` + `tsc --noEmit` in the UI, `cargo fmt --check`, `cargo clippy`, `cargo test --locked --workspace`, then a release build with the new defaults and a manual smoke of: submit a message (prefill then Thinking appears and persists), drag the Agent tab, close attempt on Workspace tab (no X), mic present.