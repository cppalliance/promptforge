# crates/gateway/stt/

`crates/gateway/stt/` is the speech-to-text subsystem, private to the gateway family - only `gateway-stt` is visible one level up.

## gateway-stt

The gateway-owned speech-to-text runtime and HTTP endpoints (at `api/`): the SpeechService covering artifacts, batch, and Realtime transcription. The gateway mounts its routes in stt builds. Depends on gateway-config, gateway-local, gateway-progress, gateway-stt-engine, and gateway-stt-backend-whisper.

## gateway-stt-engine

Backend-neutral speech decoding: the SttEngine with interim and final workers and the ModelFactory/Decoder traits. The api crate drives it and backends implement it. No workspace dependencies.

## gateway-stt-backend-whisper

The safe Whisper decoder backend for the engine. The api crate selects it as the production backend. Depends on gateway-progress, gateway-stt-engine, and gateway-whisper-ffi.

## gateway-whisper-ffi

Runtime-loaded safe bindings for the pinned whisper.cpp C API. Only the Whisper backend uses it. No workspace dependencies; libloading performs the dynamic loading.
