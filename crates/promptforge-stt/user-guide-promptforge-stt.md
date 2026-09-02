# Speech-to-Text User Guide

The PromptForge gateway has a built-in speech-to-text runtime. It gives you two things at once: a live dictation channel that streams interim transcripts while you speak, and an OpenAI-compatible file transcription endpoint that your existing client code can call without changes. Models are pinned by digest, provisioned automatically, and loaded only for the profile you select. This guide shows you how to configure the runtime, transcribe audio files, and run live dictation sessions. When you finish, you will have a transcription service you can configure, call, and observe.

## What This Is

The gateway owns and operates the speech-to-text runtime. You do not run a separate service. You select an STT profile in the gateway configuration, and the gateway provisions, verifies, and loads the models for that profile.

You interact with the runtime in two ways. You stream microphone audio over a WebSocket for live results. Or you upload a WAV file over HTTP and receive the transcript in the response.

## Endpoints and Transports

The gateway exposes three endpoints:

- `GET /stt` - a WebSocket endpoint for live, streaming speech-to-text. Workshop clients use this persistent socket for dictation. Any WebSocket client can use it.
- `POST /v1/audio/transcriptions` - an OpenAI-compatible multipart endpoint for file transcription. Existing OpenAI client tooling works against it without modification.
- `GET /stt/capability` - a JSON probe that reports whether GPU-accelerated transcription is available.

Use `/stt` when you speak to the gateway in real time. Use `/v1/audio/transcriptions` when you have an audio file.

## Configuration and Runtime Lifecycle

You configure the runtime through the shared gateway configuration. There is no separate STT config file. You declare model catalog entries with `[[stt_model]]` tables, and you attach models to a profile with `[[profile]]`.

A minimal configuration declares one interim model and selects it in a profile:

````toml
[[stt_model]]
name = "speech"
role = "interim"
source = "model.bin"
vram_gb = 1.0

[[profile]]
name = "voice"
models = ["speech"]
````

The `role` field is `interim` or `final`. An interim model produces live partial results during streaming. A final model is optional and produces higher-quality committed text. With only an interim model, the streaming endpoint keeps working with a degraded stop fallback.

When the runtime starts, the gateway provisions only the models the selected profile declares. Unused models are not downloaded. Downloaded artifacts are verified before use. Switching profiles loads the new engine on demand and unloads the previous engine automatically.

A fuller configuration adds a final model, pins an artifact by digest, and tunes capture behavior:

````toml
[[stt_model]]
name = "speech"
role = "interim"
source = "model.bin"
vram_gb = 1.0

[[stt_model]]
name = "speech-final"
role = "final"
source = "model-final.bin"
sha256 = "<64-hex-digit digest>"
vram_gb = 2.0

[workshop.stt]
window_seconds = 8
interval_ms = 400

[[profile]]
name = "voice"
models = ["speech", "speech-final"]
````

- Set `sha256` on a model to pin it to an exact digest. The gateway rejects a tampered or wrong artifact at provisioning time.
- Omit `sha256` and point `source` at a local file such as `model.bin` to use an unpinned model directly.
- Tune capture through `[workshop.stt]`: `vocabulary` biases recognition toward your terms, `window_seconds` sets the analysis window, and `interval_ms` sets the pass interval.

Three profile shapes are valid:

- Interim plus final: full quality pipeline.
- Interim only: one model loads; streaming still works.
- No STT models: the gateway starts cleanly with no STT.

Two configurations are rejected. A profile with a final model but no interim model fails validation with an error that names the profile and the fix: add one interim STT model or remove the final model. A headless gateway refuses an active profile that selects STT models.

## GPU Acceleration

GPU acceleration is selected at run time from the managed whisper.cpp bundle. Windows x86-64 uses the pinned CUDA build, Apple Silicon uses Metal, and other supported targets use the pinned CPU build. Cargo builds need no CUDA toolkit or whisper.cpp toolchain.

Before you start a dictation session, query the capability endpoint to check GPU availability:

````bash
curl "$GATEWAY/stt/capability"
````

The response reports GPU availability and whether an STT engine is provisioned and loaded in the active profile:

````json
{"gpu": true, "engine": true}
````

## File Transcription API

You transcribe a file with one multipart POST. Authenticate every request with the gateway bearer token. The simplest request uploads a WAV file and names a loaded model:

````bash
curl -X POST "$GATEWAY/v1/audio/transcriptions" \
  -H "Authorization: Bearer $TOKEN" \
  -F file=@meeting.wav \
  -F model=speech
````

The default response is compact JSON containing only the transcript:

````json
{"text": "hello"}
````

The `model` field selects which loaded model handles the request: the interim-role model or the final-role model. If the name matches no loaded model, the gateway returns HTTP 404 with a message naming the unknown model. A malformed request returns HTTP 400.

Uploaded audio must be 16 kHz mono WAV, at most 25 MiB. Integer WAV of any bit depth and 32-bit float WAV are both accepted; integer PCM is normalized to floating point automatically. Oversized files are rejected before any decoding work happens.

A maximal request chooses the verbose response shape, hints the language, and requests timestamp granularity:

````bash
curl -X POST "$GATEWAY/v1/audio/transcriptions" \
  -H "Authorization: Bearer $TOKEN" \
  -F file=@meeting.wav \
  -F model=speech-final \
  -F response_format=verbose_json \
  -F language=en \
  -F "timestamp_granularities[]=segment" \
  -F prompt="quarterly planning meeting" \
  -F temperature=0.0
````

- `response_format` is `json` (default) or `verbose_json`.
- `language` is a hint. It defaults to `en` and is echoed back in the verbose response.
- `timestamp_granularities[]` accepts `segment` (the default) and `word`. Word-level timestamps can be requested, but the `words` array is currently empty because the engine has no word alignment.
- `prompt` and `temperature` are accepted without errors for OpenAI compatibility, but the current transcription workers ignore them. `temperature` must be a finite number greater than or equal to 0.0.

The verbose response adds duration, language, task name, and segment timestamps:

````json
{
  "task": "transcribe",
  "language": "en",
  "duration": 12.0,
  "text": "hello world",
  "segments": [
    {"id": 0, "start": 0.0, "end": 12.0, "text": "hello world"}
  ],
  "words": []
}
````

Errors are distinguishable by cause. Each error message names the cause. Two examples:

````text
audio file exceeds the 25 MiB limit
````

````text
audio must be 16 kHz mono, got 44100 Hz and 2 channels
````

Missing fields, invalid field values, unsupported response formats, bad WAV data, and inference failures each produce a distinct, identifiable error.

## Dictation Session Basics

A live dictation session runs over the `/stt` WebSocket. You open one connection, then run one or more push-to-talk takes on it.

The flow for one take:

1. Open a WebSocket connection to `/stt`.
2. Send the text message `start` to begin a take.
3. Receive a `stream` announcement frame. It carries a generation number that identifies the take.
4. Send audio as binary WebSocket frames: 16 kHz mono little-endian 32-bit float PCM, 4 bytes per sample.
5. Receive `interim` frames while you speak. Each frame splits the transcript into a stable `committed` prefix and a still-changing `tentative` suffix.
6. Send the text message `stop` to end the take.
7. Receive a `final` frame with the complete transcript.

Control messages are bare words, not JSON. Committed text is append-only: once words appear in `committed` they are never revised, so you can render them permanently.

You can run multiple takes on one connection without reconnecting. State resets between takes. Send `start` again mid-connection to restart with an incremented generation. Generation counters are per-connection: a new connection starts numbering at 1.

## Dictation Wire Protocol

The wire contract, by example. You send bare control words and binary audio:

````text
start
<binary PCM frames>
stop
````

The server answers with JSON frames. The `stream` announcement arrives immediately after `start`, before any other frame:

````json
{"type": "stream", "generation": 1}
````

While audio streams, `interim` frames carry the live partial result:

````json
{"type": "interim", "committed": "we hold these truths", "tentative": "to be self", "generation": 1}
````

On `stop`, the `final` frame carries the full transcript, the count of complete audio samples received, and the take generation:

````json
{"type": "final", "text": "we hold these truths to be self evident", "frames": 192, "generation": 1}
````

Rules to rely on:

- Every frame carries the same `generation` counter. Correlate any frame with its take and discard stale frames.
- A partial trailing sample in a binary frame is dropped. Only complete 4-byte samples are counted in `frames`.
- Unknown text messages are ignored. They do not break the take or disturb transcription.
- Standard WebSocket ping/pong keepalive is handled transparently.

## Transcription Quality Pipeline

During a take, the runtime refines the transcript in the background. You observe this through the frames.

When a final-role model is configured, `committed` text grows segment by segment as the final-pass model finishes closed speech segments. If the final pass fails, or no final model is configured, the runtime falls back to the interim model automatically. The interim-only fallback decodes the entire take on stop, so no speech is lost.

The final transcript is the committed prefix plus a freshly transcribed tail, joined by a single space. If you stop exactly at a segment boundary, the final transcript is just the committed prefix, with no redundant tail transcription.

Three suppression behaviors keep the stream clean:

- Silent or too-short audio windows are never sent to the transcription engine. Silent audio produces no interim frames and an empty final transcript.
- Interim frames are sent only when the transcript changed. Duplicates are suppressed.
- A slow-reading client always sees the freshest interim result. Stale interims are dropped in favor of the newest.

If the engine is swapped mid-stream, for example by a profile switch, the current take resets cleanly instead of corrupting the transcript.

## Session Observability and Security

During a take, the gateway pushes live status updates to the workshop activity feed: "Listening...", "Transcribing...", and "Finalizing transcript...". Transcription failures produce visible failure notifications ("Transcription failed") in the feed rather than silent drops.

The `/stt` endpoint accepts connections only from native clients (no `Origin` header) or allowed loopback origins. Cross-site requests are rejected. A foreign origin is refused with HTTP 403 at upgrade time, before any session starts.
