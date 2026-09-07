# Voice Input

You can type prompts into the chat surface. This chapter teaches you to speak them instead. Dictation uses a push-to-talk microphone button beside the send button, and the transcript lands in the prompt exactly as if you had typed it. If voice is not available on your machine, this chapter also teaches you how to tell and why.

The desktop application keeps the microphone connection same-origin: its Workshop server relays `/v1/realtime` to the gateway's fixed `/v1/realtime?intent=transcription` target. The relay authenticates upstream but never parses speech payloads or owns speech state. The gateway credential stays in the server process and is never exposed to the webview.

## Dictating a prompt

To dictate into the chat input:

1. Click the microphone button beside the send button. Its tooltip reads "Push to talk".
2. Speak your message.
3. Click the microphone button again to stop. The tooltip now reads "Stop recording".

While you speak, you see one evolving transcript. Each revision replaces the previous hypothesis in the same editor range, so revised phrases do not accumulate. When you stop, the authoritative completion replaces the hypothesis and focus returns to the input. After you stop, the input stays locked until the final transcript arrives; other committed takes may finish independently.

Dictation splices the transcript into the current selection, behaving like typing at the cursor. Newlines in the transcript become line breaks. Dictating over a selection replaces the selection outright. Consecutive takes compose, because each take captures the cursor position fresh at record start. You never see stale transcription text from a previous take: takes are numbered per connection, and frames from a superseded take are discarded.

While a take records, the input locks against typing and shows a recording ring, so the insertion geometry cannot be disturbed. You can still press Enter to send what the box shows. Sending during a take sends the visible text, interim transcript included, and discards the take. Discarding a live take, for example by closing the tab or starting a new session, restores the pre-take text and unlocks the input. An empty take tells you no speech was detected, with the number of captured audio frames.

The status bar shows a red recording LED while the microphone is capturing, and the mic button shows a solid danger-colored fill with a matching ring while recording.

## When the mic does nothing

The mic stays visible and clickable in every state. Dictation is gated by the agent's pending input wait. Clicking it at another time names the blocker on the status bar: "The agent isn't asking for input; the mic opens when it does." The first eligible click may connect the Realtime session and ask you to try again in a moment; the session then reconnects with bounded backoff after a dropped connection.

Failures during dictation are named too. Microphone permission denial or capture failure is named on the status bar. A dropped dictation connection is reported on the status bar, including drops before the final transcript lands. A server error message during a take is shown verbatim on the status bar and ends the take. A browser without microphone, audio, or WebSocket support is told "Dictation is not available in this browser."

Under the hood, the Workshop serves a payload-opaque Realtime socket at `/v1/realtime`. Browser capture applies echo cancellation and noise suppression, resamples to 24 kHz, converts samples to signed little-endian PCM16, and sends canonical Base64 audio appends. Stop flushes the capture worklet before committing the input buffer, so the final short block is included.

## Microphone permission on each platform

Each platform handles the microphone grant differently:

- On Windows, the application grants the microphone permission automatically. You are never interrupted by a microphone permission prompt. Every other permission kind keeps the normal browser behavior.
- On Linux, the application turns on media capture in its webview and grants microphone and camera capture requests automatically. Other permission requests, such as notifications and geolocation, remain denied by default.
- On macOS, the application holds the audio-input entitlement that permits microphone capture for local dictation. The system permission prompt explains: "PromptForge uses the microphone you select for local voice dictation."

If microphone setup fails at startup, you can keep working in the application and only voice input stays unavailable.

## Voice configuration

Voice input comes pre-tuned with a 15-second transcription window and a 500 ms interval, set in the `[stt]` section of the gateway boot config:

````
[stt]
window_seconds = 15
interval_ms = 500
````

You can add a `vocabulary` list of domain terms to bias recognition:

````
vocabulary = ["MCP", "GGUF", "Lua"]
````

Legacy `[workshop.stt]` input is accepted only when `[stt]` is absent. The gateway saves only the canonical `[stt]` form.

First run provisions two recommended speech-to-text models: `whisper-base-en` for interim results and `whisper-small-en` for final results. They download from Hugging Face with pinned sha256 checksums and stated VRAM requirements of 1.0 GB and 2.0 GB. The generated configuration boots the gateway into a profile named `default` that activates both provisioned whisper models.

You can now speak or type your prompts. The next chapter teaches you to give the agent files to work on by granting folders to the workspace.

