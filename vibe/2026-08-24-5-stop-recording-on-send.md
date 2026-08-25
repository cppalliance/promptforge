---
name: Stop recording on send, append across takes
overview: When the user sends a chat message while voice recording is active, stop the recording, discard the pending take, and reset voice state so the next recording starts fresh. Dictation behaves like typing at the cursor (insert at insertion point, replace selection, readOnly during the take), so consecutive takes compose a message before sending.
todos:
  - id: discard-on-send
    content: "Single commit: onUserSubmit hook discards active recording, suppresses late frames, closes socket without stop; typing-model splice at cursor with readOnly during takes"
    status: pending
isProject: false
---

# Stop recording on send, append across takes

One commit, UI-only. No server changes.

## Bug 1: send while recording leaves the mic hot

Pressing send while recording leaves the voice session running: the mic stays hot, the server buffer keeps accumulating, and interim frames keep writing into the textarea after murm-ui clears it on submit. Worse, when the in-flight final pass resolves, its frame writes the finished transcript into the freshly cleared box.

## Bug 2: starting a new take wipes the composer

`startVoice` calls `clearInterim()` which sets `input.value = ""`. A user who records, stops, and records again loses the first take's text. Dictating a longer message across several takes should accumulate.

## The fix for bug 2: transcription is typing

The take behaves exactly like the user typing at the cursor (the Windows Win+H / macOS dictation model):

- **On record start:** capture `selectionStart`/`selectionEnd` as the insertion range. Set `input.readOnly = true` - the user cannot type or disturb the insertion geometry mid-take; programmatic writes still work. If a text selection exists, it is removed at take start and the take replaces it (standard dictation behavior).
- **During the take:** each frame writes `prefix + inserted + suffix` where prefix/suffix are the text outside the insertion range and `inserted` is `committed + tentative`. Cursor pinned to the end of the inserted region (`setSelectionRange`).
- **On stop (final):** the final transcript replaces the inserted region. `readOnly = false`. Cursor lands at the end of the insertion; the user edits freely and can record again at a new cursor position.
- **On discard (send mid-recording):** the inserted region is removed (prefix + suffix restored), `readOnly = false`.
- `clearInterim()` is removed entirely - nothing wipes the box.
- Append-to-end is just the special case "cursor at end," so multi-take composition works by default.

readOnly is what makes insert-at-cursor safe: without it the user could move the cursor mid-take and the next frame would splice at a stale offset. Add a subtle visual affordance (a `.mur-chat-input--recording` class, e.g. dimmed border via CSS variable) so the locked state is visible.

## The fix for bug 1

**Seam:** murm-ui's `ChatPlugin.onUserSubmit` hook (`ui/src/chat/core/types.ts:393`) - fires synchronously on every submit path, including Enter (which bypasses the DOM form submit event by calling `handleSubmit()` directly, per `components/input.ts:132-136`).

**`ui/src/main.ts`:** the voice plugin gains `onUserSubmit: () => voiceControls.discardIfRecording()`. `setupVoice` returns a small handle; the plugin stores it from `onInputMount`.

**`ui/src/voice.ts`:** `setupVoice` returns `{ discardIfRecording(): void }`. The discard path is deliberately different from `stopVoice()`:

1. If no active session, no-op.
2. Set a `suppressReplies` flag so `handleVoiceMessage` ignores any in-flight interim/final frames (the final pass arriving after send must not refill the cleared composer).
3. Release the audio half (existing `releaseAudio`).
4. Close the socket immediately - do NOT send `"stop"`. The tail's final-model decode is discarded by design: the user sent what they saw (committed + tentative), and waiting for a refinement that will be thrown away wastes a decode. The server's session loop breaks on close and drops the buffer, which is the "clear the recording buffer" requirement; the next take's `"start"` also fully resets server state via `begin_take`.
5. `setRecording(false)` on the mic button and `statusBar.setRecording(false)` (REC badge off).
6. Brief voice status note, e.g. "Recording discarded." (non-error).

`stopVoice()` (the normal mic-button path) is unchanged: it still sends `"stop"` and awaits the final transcript.

**State reset:** `suppressReplies` is per-session - set on the discarded session, and `startVoice` always begins with a fresh flag, so the next recording is unaffected.

## Test

Extend `ui/test/smoke.mjs` voice section:
1. Start recording via mic click (FakeWebSocket stubs already exist).
2. Submit a chat through the engine (the smoke test already drives a chat round-trip - trigger it while recording).
3. Assert: REC badge cleared, the voice socket was closed, and a late `{"type":"final","text":"..."}` delivered to the closed socket's message handler does not write into the textarea.
4. Typing-model: with "Hello." in the textarea and cursor at end, start a take, receive an interim `{committed:"", tentative:"world"}`; assert the box shows "Hello. world" (separator space inserted when the prefix is non-empty and doesn't end in whitespace). Receive the final; assert "Hello. world". Assert no leading space when the prefix is empty.
5. Insert-at-cursor: with "ab" in the textarea and the cursor between a and b, record an interim "X"; assert "aXb" and the cursor sits after X.
6. Selection replacement: with "ab" fully selected, record an interim "X"; assert the box shows "X".
7. readOnly: assert `input.readOnly` is true during the take and false after the final arrives and after a discard.

## Bug 3: hover feedback is broken/inconsistent

The mic button while recording has no hover feedback (the glow-hover rule from the earlier commit doesn't apply to the recording state). The send button hover also appears inconsistent. All interactive controls must have the same hover behavior regardless of state.

**Root cause (from earlier analysis):** the `.mur-form-icon-btn:hover:not(:disabled)` glow rule and the `.voice-mic--recording:hover` rule have competing specificity. The review fix raised `.voice-mic--recording:hover:not(:disabled)` but the recording state's `box-shadow` (the danger-colored glow) replaces the hover glow entirely - there's no visible hover change.

**Fix:** recording-state hover should brighten the existing glow. Two visual states:

- **Recording, not hovered:** solid danger background, subtle danger glow (current).
- **Recording, hovered:** same background, *brighter* danger glow (wider/more opaque box-shadow). The CSS transition already in place handles the fade.

```css
.voice-mic--recording:hover:not(:disabled) {
  box-shadow: 0 0 0 1px var(--danger, #b0606a),
              0 0 8px color-mix(in oklab, var(--danger, #b0606a) 70%, transparent);
}
```

Also verify the send button hover: it should show the same accent glow in both normal and generating (stop) states. Check `.mur-action-btn:hover` vs `.mur-action-btn.mur-generating:hover` - both should get the glow, not the background wash.

## Bug 4: growing composer overlaps the chat history

The composer's `.mur-chat-form-container` is `position: absolute` (overlaying the scroll area), not in the flex flow. The chat history reserves a fixed `7rem` bottom padding for it (`base.css:289`). When the composer grows past that (multiline voice transcripts), it covers the messages.

**Fix in `style.css` (workbench override block):** make the form container participate in the column flex flow so the scroll area shrinks to accommodate it. Override only for the embedded app (our workbench), not murm-ui's standalone mode:

```css
.mur-app-embedded .mur-chat-form-container {
  position: relative;
}
.mur-app-embedded .mur-chat-history {
  padding-bottom: 1rem;
}
```

`position: relative` puts the form in the flex flow; `flex: 1; min-height: 0` on `.mur-chat-scroll-area` (already set in base.css:274-276) makes the scroll area shrink. The 7rem bottom padding is replaced with 1rem (no gap needed since the form is no longer floating). The scroll area auto-scrolls to the bottom when the form grows (murm-ui's auto-scroll behavior already covers this for new messages; the form growth may need a manual `scrollTop` nudge - test and add if needed).

## Verify

`npm run build; npm run typecheck; npm test` in `crates/promptforge-wb-server/ui`, plus `cargo fmt --all --check`, `cargo clippy -p promptforge-wb-server --all-targets --all-features -- -D warnings`, `cargo test -p promptforge-wb-server` (should be untouched - UI-only change, but build.rs rebundles).

Commit message: "Discard the voice take when a message is sent mid-recording". Design log entry for the close-without-stop decision.


---

## Recovered rationale

Recovered from the producing chat sessions by the plan ledger on 2026-09-04. Everything below this heading is derived annotation, not part of the original plan.

# Enrichment: stop_recording_on_send_721052f2

## Origin: four bugs from hands-on dogfooding

All four bugs were reported by the user on 2026-08-24 while using the workbench voice UI live. Verbatim:

- Bug 1 (send leaves mic hot): "If they press, if, if the recording is on and then they press the send button, it should turn recording off. Right now it leaves recording on and then it continues appending, and then when, and, and then when, like, when you press send, it has to clear the recording buffer so that it can start fresh."
- Bug 2 (new take wipes composer): "another problem. if you press record, talk, stop reecord. then press record, talk, - it clears the previous text, even if you did not press send. it should be possible to add to existing text without clearing it, before sending"
- Bug 4 (composer overlap): "When the edit box grows vertically, the bottom of the chat box stays below the top of the edit box, overlapping the text"
- Bug 3 (hover consistency): "a hover should be the same no matter what state it's in. We should stroke the outline in white and make it glow, like two or three pixels, and the same for the, for the, for the send button."

Underlying requirement stated earlier the same day: "I want it, in the fucking edit box, in a way that the user can select it and edit it afterwards" - transcription must land in the editable composer, not a separate display element (the user had ordered the separate interim-text element removed: "Remove that element."). The volatility complaints that day ("The text in the edit box should only grow (mostly) while recording. Not see words disappear.") supply the stability bar the typing model is meant to clear.

## The typing model came from the user

The readOnly-during-dictation and insert-at-cursor design was the user's own proposal: "Oh, I think during dictation, editing should be disabled. We should disable editing, so like the moment you press record, now that edit box becomes read-only. We, we can put, we can put stuff in it, but the user can't. And then once the recording goes off, okay, now it's editable again. Does this work? And then when they press the record button, we, we insert at the insertion point? How does that work? Or do we only append? What if they select text and then they press record and they start talking, does that text then get replaced? In other words, is the model that the transcription is just like the user typing?"

The assistant confirmed this matches OS dictation (Windows Win+H, macOS, Dragon) and adopted it wholesale; readOnly is what makes splice-at-cursor safe against stale offsets.

## Discarded alternatives

1. Base-text append (superseded). The first design for bug 2 captured the composer content as baseText at take start and rendered base + " " + transcript. Dropped when the user proposed the typing model: splice-at-cursor is strictly more general (append becomes the cursor-at-end special case) and handles selection replacement. The base-text approach also carried a documented limitation - text typed mid-take was silently overwritten by the next interim write - which readOnly eliminates.

2. Send-then-wait-for-final (rejected). For bug 1, the alternative of sending "stop", awaiting the final transcript, then sending the message was rejected as slower and confusing UX (paraphrase). The user sends exactly what they saw; the pending final pass is discarded.

3. Sending "stop" before closing the socket (rejected). Considered for cleanliness; the server session loop breaks on close and drops the buffer, so "stop" would only buy a final decode whose result is thrown away. This is the plan's close-without-stop decision, which the plan flags for a design log entry.

4. Hover as uniform white outline glow (refined). The user asked for the same white glow outline on hover for every control in every state. The plan instead brightens the existing state glow (danger-colored while recording) so the recording state stays visually distinct on hover (paraphrase; the chat does not record explicit user approval of this refinement).

## Constraints

- One commit, UI-only. The user repeatedly demanded single tight commits that session ("do what you think is best. get it done. one commit.").
- Enter must be covered: murm-ui's Enter path calls handleSubmit() directly, bypassing the DOM form submit event, which is why the seam is the plugin onUserSubmit hook rather than a form listener.

## Run-chat deviations

None. The supplied run chat does not execute this plan; it only references the plan file in a transcript inventory. The plan was executed inside the creator chat itself. One deviation from the plan text: the prescribed commit message was "Discard the voice take when a message is sent mid-recording", but the actual commit f2bd202 landed as "Handle voice recording as cursor-position typing with send-discard". Three review findings (dead field, empty-state CSS interaction, multi-take test) were fixed and amended in before the commit.
