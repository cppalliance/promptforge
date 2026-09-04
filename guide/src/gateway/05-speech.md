# Speech-to-Text

This chapter teaches you the gateway's transcription surface: how to declare speech models, how the interim and final roles work together, and what the /stt endpoints serve. Speech builds on local models, because speech models are provisioned and cached the same way.

## Declare speech models

A speech-to-text model is a `[[stt_model]]` entry:

````
[[stt_model]]
name = "whisper-base-en"
role = "interim"
source = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin"
sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
vram_gb = 1.0
````

Each entry has a `name`, a `role` of `interim` or `final`, a `source`, an optional `sha256` pin, a `vram_gb` estimate, and an optional `dominion` binding. The interim role transcribes while a take is still recording. The final role crystallizes completed audio.

A profile may select at most one interim and one final STT model. A final model requires an interim partner. Interim-only is a supported degraded mode. You can restore a built-in recommended pair at any time: whisper-base-en for interim and whisper-small-en for final, both carrying canonical whisper.cpp URLs and SHA-256 pins.

## Tune push-to-talk capture

Tune capture in the optional `[workshop.stt]` section:

````
[workshop.stt]
window_seconds = 15
interval_ms = 500
vocabulary = ["MCP", "GGUF", "Lua"]
````

The `window_seconds` key sets the seconds of trailing audio transcribed per pass (default 15), and `interval_ms` sets the milliseconds between passes (default 500). Each must be at least 1; a zero value fails startup. The `vocabulary` lists domain terms that bias both transcription workers toward those terms. An empty list disables biasing. A vocabulary that exceeds the model's prompt budget is truncated, and a warning is logged.

## The transcription endpoint

With the default-on `stt` feature the gateway serves OpenAI-compatible audio transcription at POST /v1/audio/transcriptions. The multipart form accepts `file`, `model`, `language`, `prompt`, `temperature`, `response_format`, and the repeated field `timestamp_granularities[]`.

Uploads are capped at 25 MiB; an over-limit upload is answered with "audio file exceeds the 25 MiB limit". Only 16 kHz mono WAV audio is accepted. Other sample rates or channel counts are rejected with a message naming what was received.

Two response shapes are offered. The `json` shape returns text only. The `verbose_json` shape returns task, language, duration, text, segments, and words. Segment timestamps are on by default, and word timestamps are always empty. A transcription request for a model not loaded in the active profile is rejected as an unknown model. A caller-supplied `temperature` must be a finite non-negative number. The `prompt` hint is accepted but ignored by the current English whisper workers.

## The runtime

Speech-to-text runs on a separately pinned whisper.cpp library bundle, b4938. A library that does not match the pinned layout fails to load, and only 64-bit targets are supported. Model artifacts and the runtime are downloaded and verified into the configured cache directory at startup, with progress reporting. Each model file is prewarmed and then loaded, with progress per model.

STT startup failures are named by stage: opening the artifact store, provisioning the whisper library, provisioning a named model, a missing interim partner, an unsupported role, or engine load. Library load failures name the failing path or symbol in the logs. You can query GET /stt/capability for a report of whether a speech engine is loaded and whether GPU transcription through CUDA or Metal is available.

## How a take is transcribed

A recorded take is split into speech segments at silence boundaries. A segment closes only after 2 seconds of trailing silence, so sentence-internal pauses survive. Speech bursts shorter than 250 ms are discarded as clicks. Audio quieter than -60 dBFS is treated as silence and never sent to the model, and fragments shorter than half a second are gated out.

With a final model configured, completed speech segments are re-transcribed in the background while the take still records, and each segment's text is reported as it finishes. Without a final model, the stop falls back to the interim model. Silent or very short fragments are skipped so the model does not invent text for them. Transcription is pinned to English, and translation is disabled.

## The streaming socket

The workshop listener serves a streaming speech-to-text WebSocket at /stt. The client drives it with the bare text messages `start` and `stop` and binary little-endian f32 PCM audio frames. The wire contract has a `stream` frame announcing each take, `interim` frames carrying committed and tentative transcripts, and a `final` frame with the transcript and frame count. Frames carry a per-connection generation counter, and committed text is append-only across interim frames.

The /stt socket refuses cross-site browser connections: the upgrade performs an Origin allowlist check and answers 403.

During a take the status bar shows "Listening...", then "Transcribing...", then "Finalizing transcript...", and failures appear as notices. A take that overruns the interim window without a final model is truncated; the warning names the window length and the dropped lead in seconds.

Switching the active profile provisions and loads the selected speech models. Switching away unloads the engine and releases the model memory.

