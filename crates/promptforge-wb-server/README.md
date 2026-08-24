# promptforge-wb-server

[![License](https://img.shields.io/badge/license-BSL--1.0-blue.svg)](LICENSE)

The PromptForge Workbench HTTP server. It serves a local chat UI and API on loopback: an OpenAI-shaped model catalog and chat relay in front of a PromptForge gateway (with streaming over a WebSocket), a JSONL session tape recording every exchange, and a WebSocket voice endpoint that transcribes push-to-talk microphone audio on-device with whisper.cpp. The desktop shell (`promptforge-wb`) embeds it in-process; run standalone it is the browser-tab frame of the same workbench.

## Quick start

Create a `workbench.toml` in the current directory (see [`workbench.example.toml`](../../workbench.example.toml) at the repository root for a commented version of every field):

```toml
[gateway]
base_url = "http://127.0.0.1:8081"
api_key = "${PROMPTFORGE_GATEWAY_API_KEY}"
```

Then run:

```bash
cargo run -p promptforge-wb-server
```

The server binds `127.0.0.1:7910` by default and serves the chat UI at `http://127.0.0.1:7910/`. Set `server.open_browser = true` to have it open your system browser once it is serving.

The desktop shell (`promptforge-wb`) is the zero-config path: it searches beside its executable, then the current directory, then `~/.promptforge/`, and on first run writes a default `workbench.toml` into `~/.promptforge/` and loads that. The server binary does not generate one - it reads `workbench.toml` from the current directory only.

String values support `${VAR}` environment interpolation; `$$` is a literal `$`, and an unset variable interpolates to the empty string.

## Configuration

Every field of `workbench.toml`:

| Field | Default | Description |
| --- | --- | --- |
| `gateway.base_url` | `http://127.0.0.1:8081` when empty | Base URL of the PromptForge gateway; an empty value (for example an unset `${PROMPTFORGE_GATEWAY_URL}`) falls back to the default |
| `gateway.api_key` | (empty) | Bearer key for the gateway API; supports `${VAR}` interpolation; empty sends no `Authorization` header |
| `tape.path` | `tape.jsonl` | Path of the JSONL session tape; one event per chat exchange |
| `server.bind` | `127.0.0.1:7910` | Address the workbench server binds to |
| `server.open_browser` | `false` | When true, the server binary opens the system browser at its address once serving; the desktop shell ignores it |
| `voice.interim_model` | (empty) | Path to the GGML whisper model for streaming interim transcription; empty disables transcription |
| `voice.final_model` | (empty) | Path to the whisper model for the pipelined final pass; empty falls back to the interim model |
| `voice.interim_source` | (empty) | URL the interim model can be downloaded from |
| `voice.final_source` | (empty) | URL the final-pass model can be downloaded from |
| `voice.window_seconds` | `5` | Seconds of trailing audio each interim pass transcribes |
| `voice.interval_ms` | `800` | Milliseconds between interim passes while a take is recording |

## Routes

| Route | Description |
| --- | --- |
| `GET /health` | Health probe; answers `{"status":"serving"}` |
| `GET /` | The chat UI (also `/app.js`, `/app.css`, `/style.css`, `/pcm-worklet.js`, served from `ui/dist/`: read from disk in debug builds, embedded in the binary in release builds) |
| `GET /v1/models` | Proxies the gateway's model catalog verbatim |
| `POST /chat` | Buffered chat relay: `{"model", "messages"}` in, gateway response out; `"stream": true` is rejected with 400 - streaming lives on `/ws` |
| `GET /ws` | WebSocket upgrade, one persistent socket for all downstream JSON: `{"type":"chat","id","model","messages"}` frames in (the optional `id` is echoed on the reply), `{"type":"delta","content"}` / `{"type":"done"}` / `{"type":"error","message"}` frames out, plus unsolicited `{"type":"status","label","description","severity","activity","progress"}` observer updates |
| `GET /voice` | WebSocket upgrade: binary f32 PCM at 16 kHz mono in, `start`/`stop` control words, interim and final transcripts out |

## UI development

The chat UI is TypeScript under `ui/src/`, bundled by esbuild into `ui/dist/app.js`. Node.js is required: run `npm install` in `ui/` once per checkout. After that, `cargo build` runs the UI build itself (the crate's `build.rs` prefers `ui/node_modules/.bin/esbuild` and falls back to `npx esbuild`, which may download esbuild on first use). `ui/node_modules/` and `ui/dist/` are gitignored.

Two workflows:

1. **Just cargo:** edit the TypeScript, then `cargo build` (or `cargo run -p promptforge-wb-server`). The build script re-bundles whenever `ui/src/` or the static UI files change, and debug builds read `ui/dist/` from disk on every request.
2. **esbuild watch:** run `npm run watch` in `ui/` in one terminal and `cargo run` in another. Edit, save, refresh the browser - no Rust recompile for UI changes.

`npm run typecheck` runs `tsc --noEmit`; esbuild strips types without checking them, so the typecheck is advisory. `npm test` runs a jsdom smoke test that imports the built `dist/app.js` and asserts the chat UI mounts (run `npm run build` first).

The chat UI itself is [murm-ui](https://github.com/levmv/murm-ui) 0.2.0, vendored in `ui/src/chat/` (MIT, see its `PROVENANCE.md`), driven by a WebSocket provider against `GET /ws` (one socket per generation). Its styles are bundled by esbuild into `dist/app.css`; `ui/style.css` carries the workbench shell (sidebar, picker, voice UI) and overrides.

## Whisper models

Whisper models are not downloaded by the build; fetch them out of band from the whisper.cpp GGML model collection on Hugging Face: <https://huggingface.co/ggerganov/whisper.cpp>. Production configs typically pair `ggml-large-v3-turbo.bin` (interim) with `ggml-large-v3.bin` (final). The test suite uses the tiny English model (`ggml-tiny.en.bin`) plus the `jfk.wav` speech fixture, placed in `tests/fixtures/` (gitignored); a missing fixture fails the test with the download URL in the message.

## Minimum Rust Version

Rust 1.89 or later.

## License

Licensed under the [Boost Software License 1.0](../../LICENSE).
