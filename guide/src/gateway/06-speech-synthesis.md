# Speech Synthesis

This chapter teaches you the gateway's speech synthesis surface: how to declare a speech model, how to call the synthesis route, and how to enumerate voices. Synthesis builds on remote models, because the gateway routes speech to remote providers only; a `[[local_model]]` with `kind = "speech"` is refused at launch.

## Declare a speech model

A speech synthesis model is an ordinary `[[model]]` entry with `kind = "speech"`, backed by an ordinary `[[endpoint]]`:

````
[[endpoint]]
id = "together"
protocol = "openai"
base_url = "https://api.together.xyz/v1"
api_key = "${TOGETHER_API_KEY}"

[[model]]
name = "orpheus"
kind = "speech"
description = "Orpheus 3B conversational speech synthesis"
upstream = "canopylabs/orpheus-3b-0.1-ft"
endpoints = ["together"]
context = 8192
voices = ["tara", "leah", "jess", "leo", "dan", "mia", "zac", "zoe"]
````

The entry carries the usual remote-model fields, and the kind scopes which of them are meaningful. Chat-only fields such as `thinking`, the effort knobs, `default_max_tokens`, and `tool_dialect` are rejected on a speech model at load time. The speech-only `voices` list declares the voices the model offers: setting it on any other kind fails at load, entries must be non-empty and unique, and an empty or omitted list means the model exposes no fixed voice list, so the route accepts any voice name. The catalog advertises the kind and the voice list verbatim on GET /v1/models, so clients can shape requests before sending them.

## Synthesize speech

The gateway serves OpenAI-shaped speech synthesis at POST /v1/audio/speech:

````
curl -H "Authorization: Bearer $GATEWAY_KEY" http://127.0.0.1:8081/v1/audio/speech \
  -H "Content-Type: application/json" \
  -d '{"model": "orpheus", "input": "The quick brown fox.", "voice": "tara"}' \
  -o speech.mp3
````

The request carries `model` and `input` (both required; the input is non-empty and capped at 4096 characters), `voice` (required; a plain name or the OpenAI object form `{"id": "tara"}`), and four optional fields: `response_format` from the closed set `mp3`, `opus`, `aac`, `flac`, `wav`, `pcm`; `speed` between 0.25 and 4.0; `instructions`, the gpt-4o-mini-tts dialect's style-control string; and `stream_format`, `sse` or `audio`. An omitted `response_format` resolves to `mp3` before the request leaves the gateway: OpenAI defaults to mp3 while Together defaults to wav, so the pin lives in the wire type and every forwarded body carries it. Fields the gateway does not name pass through to the provider verbatim, so provider extras such as Together's `sample_rate` ride the same request, and angle-bracket emotion tags such as `<laugh>` in the input reach the provider untouched.

Authentication runs before the body is parsed, so a bad key earns 401 even for a malformed body. Shape failures earn 400: an empty or over-cap `input` or an out-of-range `speed` as `malformed_request`, a non-speech model as `kind_mismatch`, and a voice outside the model's declared list as `invalid_voice` naming the valid voices, judged before queue admission so the rejection never burns a queue slot. A full queue earns 503 with code `queue_full`. An upstream 429 comes back as 429 with code `upstream_rate_limited` and an upstream 503 as 503 with code `upstream_unavailable`, so an OpenAI client sees a retryable rate-limit or server error rather than a generic failure.

The response is the provider's audio bytes streamed through unread: the gateway forwards the upstream `Content-Type` (falling back to the requested format's MIME type), never sets `Content-Length`, and emits `Transfer-Encoding: chunked`. No JSON error can follow 200 plus audio bytes, so a mid-stream upstream failure surfaces as a truncated body and the client's read fails.

## List voices

GET /v1/audio/voices answers the union of the active profile's speech voices, deduplicated and sorted:

````
{"voices": [{"id": "dan", "name": "dan"}, {"id": "jess", "name": "jess"}, {"id": "leah", "name": "leah"}]}
````

Each entry is an object with `id` first and `name` mirroring it, because the catalog configures voices as bare strings with no separate display name. OpenAI has no voice-list route; the OpenAI-compatible ecosystem converged on this one, and clients such as Open WebUI read the `id` key, so the entry shape is a compatibility surface. Tolerant clients also accept the plain-string form some servers answer with. A profile with no speech models returns an empty list, and non-speech models contribute nothing.

## The stream_format caveat

`stream_format = "sse"` is OpenAI's selector for event-stream framing, and the gateway forwards it verbatim like any other field. Provider dialects differ: Together spells its streaming mode `stream=true` (with `response_format=raw`), and it answers with server-sent events carrying base64-encoded PCM, not chunked binary audio. The gateway does no reframing or decoding: whatever framing the provider answers with passes through untouched, so a client that asks for SSE owns decoding the event stream itself.
