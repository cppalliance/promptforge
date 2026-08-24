---
name: Voice UX fixes
overview: One commit redesigning the status bar and voice UX - full-width bar, descender fix, glow hovers, Activity enum rename (Thinking/Generating), always-visible REC badge, transcript auto-grow, socket auto-reconnect with status bar reset.
todos:
  - id: voice-ux
    content: "Single commit: all status bar, LED, REC, hover, transcript, and reconnect changes"
    status: completed
isProject: false
---

# Voice UX overhaul - single commit

One commit touching both `crates/promptforge-wb-server` (Rust + UI) and its tests. Follow the rust-rulebook and vibe-rulebook; run fmt, clippy, and targeted tests before committing.

Note: two fixes are already uncommitted in the working tree from the earlier debugging session (voice.ts 120s stop timeout, voice.rs `status.idle()` on session close). They fold into this commit.

## Files and changes

### Rust: `crates/promptforge-wb-server/src/status.rs`

Rename the `Activity` enum variants:
```rust
pub(crate) enum Activity {
    General,    // no LED pulse
    Thinking,   // amber: model turn in flight
    Generating, // green: output tokens arriving
}
```
`serde(rename_all = "lowercase")` stays - wire values become `"thinking"` and `"generating"`.

### Rust: all call sites

Every `Activity::Gateway` and `Activity::Voice` in the crate must be updated. The mapping:

- **`chat_ws.rs`:** "Submitting request..." -> `Thinking`; "Streaming response..." -> `Thinking`; per-delta debug pulse -> `Generating`; errors -> `General`
- **`heartbeat.rs`:** "Connected to gateway" / "Gateway unreachable" -> `General`
- **`app.rs`:** startup instrumentation, buffered chat, voice degrade verdicts -> `General`
- **`voice.rs`:** all `Activity::Voice` -> `General` (REC badge covers recording now)
- **`provision.rs`:** download progress, completion, errors -> `General`

Search with `Activity::Gateway` and `Activity::Voice` to catch every site. Do not leave any behind.

### Rust: tests

Every test asserting `activity == Activity::Gateway` or `Activity::Voice` must be updated to the new variant. Run `cargo test -p promptforge-wb-server` and fix until green.

### HTML: `ui/index.html`

1. Move `<footer class="status-bar">` out of `.dock-column` to a direct child of `<body>`, after `.shell`.
2. Restructure the status bar internals:
```html
<footer class="status-bar" role="status" aria-live="polite">
  <span class="status-bar__text">Ready</span>
  <span class="status-bar__right">
    <span class="status-bar__rec">REC</span>
    <span class="status-bar__slot">
      <progress class="status-bar__progress" value="0" max="100" aria-label="Task progress" hidden></progress>
      <span class="status-bar__led" aria-hidden="true"></span>
    </span>
  </span>
</footer>
```

### CSS: `ui/style.css`

**`:root` block additions:**
- `--rec-idle: #800000`
- `--rec-active: #ff0000`
- `--hover-glow: var(--accent, #5b9cf5)`

**Body layout** (status bar full-width):
```css
body {
  display: flex;
  flex-direction: column;
  height: 100vh;
  margin: 0;
}
```
Change `.shell` from `height: 100vh` to `flex: 1; min-height: 0;`.

**Status bar descender fix:**
- `height` -> `min-height`
- `line-height: 1` -> `line-height: 1.4`

**Status bar right group:**
```css
.status-bar__right {
  display: flex;
  align-items: center;
  gap: 0.5em;
}
```

**REC badge:**
```css
.status-bar__rec {
  font-size: 10px;
  font-weight: 700;
  letter-spacing: 0.05em;
  line-height: 1;
  padding: 1px 4px;
  border: 1px solid var(--rec-idle, #800000);
  border-radius: 2px;
  color: var(--rec-idle, #800000);
  transition: color 0.1s, border-color 0.1s, box-shadow 0.15s;
}
.status-bar__rec--active {
  color: var(--rec-active, #ff0000);
  border-color: var(--rec-active, #ff0000);
  box-shadow: 0 0 1px var(--rec-active, #ff0000);
}
```

**LED class rename:**
- `.status-bar__led--gateway` -> `.status-bar__led--generating` (green, same colors)
- `.status-bar__led--voice` -> `.status-bar__led--thinking` (amber, same colors)

**Glow hover** (after the murm-ui bridge block):
```css
.mur-form-icon-btn:hover:not(:disabled),
.sidebar__picker:hover,
.voice-mic:hover {
  background-color: transparent;
  box-shadow: 0 0 0 1px var(--hover-glow, #5b9cf5),
              0 0 4px color-mix(in oklab, var(--hover-glow, #5b9cf5) 40%, transparent);
}
```

**Recording mic** (replaces the current `.voice-mic--recording` block; drop the `mic-pulse` keyframe):
```css
.voice-mic--recording,
.voice-mic--recording:hover {
  color: var(--on-danger, #ffffff);
  background: var(--danger, #b0606a);
  border-radius: 50%;
  box-shadow: 0 0 0 1px var(--danger, #b0606a),
              0 0 6px color-mix(in oklab, var(--danger, #b0606a) 55%, transparent);
}
```

### TS: `ui/src/status-bar.ts`

- Query `.status-bar__rec` in constructor.
- Add `setRecording(on: boolean)` that toggles `status-bar__rec--active`.
- Rename `PulseActivity` type: `"gateway" | "voice"` -> `"thinking" | "generating"`.
- `applyLed()`: `--thinking` gets amber, `--generating` gets green; generating wins on collision.
- Add `reset()` method: sets text to "Reconnecting...", clears tooltip, clears error class, calls `renderSlot(null)`.

### TS: `ui/src/voice.ts`

- Accept `statusBar: StatusBar` (add to `VoiceElements` or as a separate parameter).
- `startVoice()`: call `statusBar.setRecording(true)` after `setRecording(true)`.
- `stopVoice()`: call `statusBar.setRecording(false)` alongside `setRecording(false)`.
- `ws.onclose` handler: call `statusBar.setRecording(false)`.
- `handleVoiceMessage` final branch: after `input.value = text` and the `input` event dispatch, add `input.style.height = "auto"; input.style.height = input.scrollHeight + "px";`.
- The 120s stop timeout and `status.idle()` on session close are already in the working tree; they fold into this commit.

### TS: `ui/src/main.ts`

- Pass `statusBar` to `setupVoice`.
- Wire `workbenchSocket.onDisconnect(() => statusBar.reset())`.

### TS: `ui/src/workbench-socket.ts`

- Add auto-reconnect on `socket.onclose`: exponential backoff (1s, 2s, 4s, capped at 30s), reset to 1s on success.
- Add `onDisconnect(handler)` registration, called from `socket.onclose` before scheduling reconnect.

### Tests: `ui/test/smoke.mjs`

- Update status frame assertions: `"gateway"` -> `"generating"` or `"thinking"` as appropriate.
- Add a REC element assertion (`.status-bar__rec` present, class toggle works).

## Build and verify

```
cargo fmt --all
cargo clippy -p promptforge-wb-server --all-targets --all-features -- -D warnings
cargo test -p promptforge-wb-server
npm run build && npm run typecheck && npm test   (in crates/promptforge-wb-server/ui)
```

All must pass before committing. Single commit message: "Redesign the status bar and voice UX".


---

## Recovered rationale

Recovered from the producing chat sessions by the plan ledger on 2026-09-04. Everything below this heading is derived annotation, not part of the original plan.

# Enrichment: voice_ux_fixes_8f6de74e

## Why

The status bar itself originated earlier the same day (Aug 24, 2026) from the user's demand for ambient visibility into the workbench: "that bar should always show the user like what the fuck the workbench is doing. Like at all times", delivered via an observer object threaded through the program - explicitly "I don't want a global variable, but I want something threaded through the entire program". The reconnect requirement also predates this plan: "the workbench must tolerate losing the connection to the gateway, and reconnecting".

This plan is the same-day correction pass: the user ran the UI, watched it, and reported defects in real time (with screenshots) - clipped descenders, hover styling clobbering the recording state, LED colors that did not match his mental model, and transcribed text landing outside the edit box. The plan bundles those fixes plus the already-uncommitted debugging-session fixes (120s stop timeout, status.idle() on session close) because the user wanted "just one commit".

## Decisive user statements (verbatim)

- Full-width bar: "The status bar should be the full width of the window at all times, at the bottom, and it is not a child of anything else it is a top level element."
- LED semantics (the reason Gateway/Voice became Thinking/Generating): "Change it. I want amber to mean that a model turn is processing. And green when there is a spurt of output tokens" - later refined: "Amber as long as there is a completion processing, and green during the completion if tokens are coming out".
- REC design: "yes and the REC will be just the word REC in maroon, and put a maroon rectangle around that, and there is no more red LED. REC will ALWAYS be present to the left of the LED when the LED is showing. And when recording is on, REC and the rectangle around it should have a red color (ff,00,00) and have a 1-pixel glow".
- Layout: "to be clear I want REC and the LED to be right justified, dont put a huge space between those two. just a normal space like the width of a SP character (or two)"; after seeing the first run: "too much space between REC and the LED. also, the inactive REC is too red. make it like 0x552222".
- Hover glow: "when the record button is enabled and I hover, it looks like it's not enabled anymore... I think I want the hover to be to outline the control with the glow. For like everything should be that way".
- Transcript auto-grow: "I want it, in the fucking edit box, in a way that the user can select it and edit it afterwards and make sure that all the text is visible (up to some verttical height limit)".
- LED aesthetic (from the morning design talk): "I want that LED to be beautiful with a soft glow like a real LED"; and the slot rule: "the progress bar and the LED are mutually exclusive, the progress bar takes priority".

## Discarded alternatives

- Original LED semantics: amber = voice activity, green = any gateway traffic. Discarded because the user's mental model was model-turn state (processing vs tokens flowing), not which transport endpoint was active; voice recording got the REC badge instead.
- REC as a "red glow LED" and as a new StatusBar enum state that hides the green/amber LEDs while recording (the user's first sketch) - replaced within minutes by the persistent maroon text badge sitting beside the LED.
- A separate interim-transcript element above the edit box - built, then rejected on sight: "Remove that element." Transcribed text goes directly into the editable input instead.
- `Option<Activity>` - the user chose adding a `General` variant to the enum instead.
- SSE for status updates - the user chose two two-way websocket connections.
- Two separate LEDs (green plus amber) considered in the morning design talk; merged into one color-changing LED with a ~200-250ms pulse timer.

## Run deviations (coder run d63483b7, commit 7871355 "Redesign the status bar and voice UX", 19 files +367/-130)

1. `reopen()` nulls `onclose` before the intentional close - otherwise a chat abort would flash "Reconnecting..." and stack a redundant backoff timer.
2. The smoke.mjs REC test required FakeWebSocket `addEventListener`, non-JSON send tolerance, and getUserMedia/AudioContext stubs; `mediaDevices` goes on Node's global `navigator`; explicit `process.exit(0)` because the voice-status 8s timer outlives the assertions.
3. A provision.rs test asserted the wire string `"voice"` - fixed to `"general"`; the plan's `Activity::` grep missed string literals.
4. One filtered re-run showed a native CUDA teardown access violation after all tests passed; it did not recur in the full suite and was judged unrelated (enum rename only).

The second supplied run chat (59952dc1) is not a plan run - it is a transcript-mining session that only catalogs this plan - and holds no deviations.
