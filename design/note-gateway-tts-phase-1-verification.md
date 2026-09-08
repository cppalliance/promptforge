# Gateway TTS phase 1: live-provider verification note

## Status

`tools/gateway-tts-parity.py` ran green against Together AI's Orpheus speech endpoint on 2026-09-08 at HEAD `f92f33ec` (branch `add-tts-phase-1`; first run at `f7e00c81`). Every assertion passed: the gateway contract checks plus the provider-stable subset (wav mapping, emotion-tag 200, unknown-field tolerance) asserted against the live provider. No gateway defect surfaced. Behavior and wire shapes only: no credential material appears in this note, and none was written to disk by the run.

## Run boundary

- Command: `python tools/gateway-tts-parity.py` (exit 0, `PARITY OK`).
- The script built no code; it drove the already-built debug gateway (`cargo build -p gateway`) with a throwaway config in a temp directory: one `[[endpoint]]` for `https://api.together.xyz/v1` with `api_key = "${TOGETHER_API_KEY}"`, one `kind = "speech"` model named `orpheus` upstreaming to `canopylabs/orpheus-3b-0.1-ft` with the eight Orpheus voices configured, and a throwaway `parity` profile selected via `--profile`. The provider key reached the gateway only through its process environment; the config on disk carried the `${TOGETHER_API_KEY}` reference.
- Calls made through the official `openai` Python SDK (3.8.0), once against the gateway (`POST /v1/audio/speech` with default format, with `wav`, and with an emotion-tag input; `GET /v1/audio/voices`; three passthrough-field probes) and once against Together directly with the same call set.
- Probe input: a single English sentence (~90 characters).

## Assertions (all passed)

Gateway-only contract checks:

- Default-format speech call returns `Content-Type: audio/mpeg` with an ID3-tagged mp3 body (magic `49 44 33 04`, ID3v2.4). Together's own default is wav (see below), so the mp3 result proves the gateway's structural `response_format = "mp3"` pin reached the provider.
- The response is streamed: `Transfer-Encoding: chunked` and no `Content-Length`, matching the `relay_audio` contract.
- `GET /v1/audio/voices` returns 200 with `{"voices": [...]}` holding the eight configured voices as sorted `{"id", "name"}` objects with `name` mirroring `id`.

Asserted against both the gateway and the live provider (the provider-stable subset the first live run observed):

- `response_format = "wav"` returns `Content-Type: audio/wav` with a RIFF/WAVE body.
- An input carrying `<laugh>` returns 200 and audio: angle-bracket emotion tags pass through the gateway untouched, and Together accepts them.
- Fields outside the gateway's named wire set (`sample_rate`, and a bogus `promptforge_probe`) ride the `rest` passthrough to the provider and come back 200, confirming verbatim forwarding; Together also tolerates the named `instructions` field with a 200.

## Observed Together AI dialect (direct calls)

- Default `response_format` is wav: the same speech call with no format field returns `Content-Type: audio/wav` with a RIFF body. This is the contrast that proves the gateway's mp3 pin, and it matches the Together docs the design report cited.
- The plain (non-`stream`) speech POST answers with `Transfer-Encoding: chunked` and no `Content-Length`. The design report predicted a non-streaming passthrough from Together; that prediction concerned the response *framing* (no SSE `stream=true` mode for binary audio), and it holds. The transport still arrives chunked, so the gateway's chunked relay mirrors the upstream shape one to one.
- Emotion-tag input (`<laugh>`) is accepted with 200; Together does not reject or error on Orpheus control tags.
- `instructions` (the gpt-4o-mini-tts style field) is tolerated with 200. Whether Orpheus applies it is unverified; it is at minimum not rejected.
- `sample_rate = 44100` is tolerated with 200; whether the rate is applied is unverified from headers alone.
- A wholly unknown field (`promptforge_probe`) is silently ignored with 200. Together's speech endpoint ignores rather than rejects unknown request fields.
- `GET /v1/audio/voices` does not exist on Together: 404 with an HTML page (a Next.js site page), not a JSON error envelope. The gateway's voices union route is a genuine value-add over this provider, not a relay.
- 429/503 envelopes were not provoked (deliberately: provoking a rate limit on a paid provider for observation is not worth the cost). The gateway's distinct `UpstreamRateLimited`/`UpstreamUnavailable` mappings for upstream 429/503 remain covered by the Rust integration suite only, not by live observation.

## Not exercised live

- Voice rejection (a voice outside the catalog's `voices` list earns a 400 naming the valid set) and kind mismatch are covered by the gateway integration suite, not this probe.
- Mid-stream disconnect cancellation is covered by the integration suite; the probe reads every response to completion.
