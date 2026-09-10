# Gateway TTS phase 1: live-provider verification note

## Status

Live verification is deferred to the Phase 3 rerun; no run has been recorded yet. The live probe is `tools/gateway-tts-live.mjs`, a dev-only Node script (zero dependencies, built-in `fetch`, never in CI) that builds the gateway fresh, boots it with a throwaway Together-backed profile, and asserts the speech and voices surfaces through the gateway's own responses. This note records the probe's contract now and its findings after the rerun. Behavior and wire shapes only: no credential material appears in this note, and none is written to disk by a run.

## Provenance

Recorded at the Phase 3 live rerun:

- Commit: <recorded at the Phase 3 live rerun>
- Tree hash: <recorded at the Phase 3 live rerun>

## Run boundary

- Command: `node tools/gateway-tts-live.mjs`. Exit 0 with `LIVE OK` when every assertion passes; with no `TOGETHER_API_KEY` in the process environment the script prints a skip and exits 0 without building or touching the network.
- The script always runs `cargo build -p gateway` first, so the probed binary always matches the recorded commit; a stale binary can never be tested against a current hash. It then boots the gateway on an ephemeral loopback port with a throwaway config in a temp directory: one `[[endpoint]]` for `https://api.together.xyz/v1` with `api_key = "${TOGETHER_API_KEY}"`, one `kind = "speech"` model named `orpheus` upstreaming to `canopylabs/orpheus-3b-0.1-ft` with the eight Orpheus voices configured, and a throwaway `tts-live` profile selected via `--profile`.
- Credential invariant A19 (`vibe/archdoc.md`: keep vendor and remote-service credentials inside the gateway): the script takes the vendor key from the process environment only (no dotenv parsing, no `.env` reading) and ferries it only to the gateway subprocess environment, where the config's `${TOGETHER_API_KEY}` interpolation resolves it. The script never calls a vendor directly and never prints or persists the key.
- Calls, all through the gateway: `POST /v1/audio/speech` with default format, with `wav`, and with an emotion-tag input; `GET /v1/audio/voices`; three field-tolerance probes.
- Probe input: a single English sentence (~90 characters).

## Assertions (each fails the run)

- Default-format speech call returns `Content-Type: audio/mpeg` with a byte-nonempty mp3 body (ID3 tag or frame-sync magic). Together's own documented default is wav, so an mp3 answer through the gateway proves the structural `response_format = "mp3"` pin reached the provider.
- The response is streamed: no `Content-Length`, matching the chunked relay contract.
- `response_format = "wav"` returns `Content-Type: audio/wav` with a byte-nonempty RIFF/WAVE body.
- An input carrying `<laugh>` returns 200 with audio: angle-bracket emotion tags pass through the gateway untouched.
- `GET /v1/audio/voices` returns 200 with `{"voices": [...]}` holding the eight configured voices as sorted `{"id", "name"}` objects with `name` mirroring `id`.
- The named optional `instructions` field is forwarded and tolerated with a 2xx, and fields outside the gateway's named wire set (`sample_rate`, a bogus `promptforge_probe`) ride the verbatim passthrough and come back 2xx.

## Observed dialect (filled in at the Phase 3 live rerun)

- Default format, framing, and byte counts: <recorded at the Phase 3 live rerun>
- Emotion-tag handling: <recorded at the Phase 3 live rerun>
- Rejected or ignored fields: <recorded at the Phase 3 live rerun>
- 429/503 envelopes: not provoked unless the rerun finds a cost-free trigger; provoking a rate limit on a paid provider for observation is not worth the cost. The gateway's distinct `UpstreamRateLimited`/`UpstreamUnavailable` mappings stay covered by the Rust integration suite regardless.

## Not exercised live

- Voice rejection (a voice outside the catalog's `voices` list earns a 400 naming the valid set) and kind mismatch are covered by the gateway integration suite, not this probe.
- Mid-stream disconnect cancellation is covered by the integration suite; the probe reads every response to completion.
