# harness-gateway-client

[![License](https://img.shields.io/badge/license-BSL--1.0-blue.svg)](../../LICENSE)

The standard way a Host talks to the PromptForge Gateway. `GatewayBroker` is the inference broker a Gateway Host hands the Harness, and `GatewaySearch` is the search provider it supplies for web search. The Harness streams nothing, so a Host that shows a reply as it forms streams the round itself through `GatewayBroker::chat_streaming`, and every completion either kind of round returns names the model the round was routed to. `GatewayChat` sends a `Chat` effect's round to the Gateway's `/chat/completions` endpoint, reads the SSE stream under the run's byte cap and timeout, and returns the one `Completion` the round produced with the client-side timing it measured; `fetch_model_catalog` reads the Gateway's `GET /v1/models` list. Under the client sits the OpenAI chat-completions wire code: it turns the round into the one request body every round sends, reads the streamed reply into the `Completion` that answers the effect, and turns a failed response into the `CompletionError` the round carries. The crate sits beside `harness` at the `crates/` root, and among the workspace crates it depends only on `promptforge`, `harness`, and `harness-web`. The wire code opens no connection and reads no clock, so another broker can reuse it by supplying its own response as a `ChunkSource` and its own clock.

## Public surface

- `GatewayBroker` - the `harness::InferenceBroker` a Gateway Host passes to `Harness::new`, built from a `GatewayEndpoint` and a `SecretString`. `chat` runs each round on a `GatewayChat` under the Engine's default run limits and returns the whole reply, `chat_streaming` runs a round the same way and hands each `StreamDelta` to the Host's callback as it arrives, and `models` reads the Gateway's model list through `fetch_model_catalog`. Both rounds label the completion with the model the round's options name, not the name the response body gave. Its futures, `models` included, need a tokio runtime with its reactor and timer, so a Host that uses it awaits `Harness::run` inside one, while the Harness itself needs none.
- `GatewayChat` - a chat-completions client bound to one Gateway URL and, usually, the Gateway's shared bearer key. `new` takes a key, `keyless` sends no `Authorization` header, `disabled` fails every round as `Unavailable` without sending, and `from_env` reads `PROMPTFORGE_GATEWAY_URL` and `PROMPTFORGE_GATEWAY_API_KEY`, making the key optional only for a loopback URL. `with_request_limits` applies the run's per-receive timeout and response byte cap, and `complete` runs one streamed round.
- `GatewayEndpoint`, `SecretString`, `GatewayConfigError`, and `SecretError` - the validated Gateway base URL, the redacted bearer key, and the setup errors they report. No setup error echoes the URL, which can embed a credential.
- `GatewaySearch` - the `harness_web::SearchProvider` a Gateway Host supplies, bound to one Gateway URL and its bearer key. Each search POSTs the validated `SearchQuery` to the Gateway's `/tools/web_search` relay under a 30-second deadline and returns the reply's rows as `SearchResults`. A failed search is a `SearchError` of kind `Transport` or `Backend` that keeps the `GatewaySearchError`, with its `GatewaySearchErrorKind` and its own cause, as its source.
- `fetch_model_catalog` - reads the Gateway's model list into a `ModelCatalog`, skipping entries with no context window, under a byte cap on both the success and the error body.
- `CompletionError` and `CompletionErrorKind` - re-exported from `promptforge::model` so a caller can match failures without naming the facade.
- `build_request_body` - the chat-completions JSON body for a `Chat` effect's messages, tools, and options. Every body streams and asks for the final usage chunk, wraps each tool in the OpenAI function shape, and adds `temperature`, `max_tokens`, and `chat_template_kwargs.enable_thinking` only when set.
- `StreamDelta` - one live piece of a streamed reply, `Text` for answer text and `Reasoning` for the reasoning side channel. Tool-call fragments arrive only in the finished `Completion`. It is `#[non_exhaustive]`, so a `match` needs a wildcard arm.
- `ChunkSource` - a response body read one chunk at a time; the only I/O the read helpers touch. A read failure is the caller's own `Timeout` or `Transport` error, returned unchanged.
- `read_completion_stream` - reads the SSE reply to its `[DONE]` sentinel under a byte cap, forwards each text or reasoning delta live, and folds the stream into a `Completion` labeled with the model the request body names, not the name the response gave, with the round's `CallMetrics` (the backend's `usage`, llama.cpp `timings`, vLLM `metrics`, and the client timing measured on the caller's clock) and the `RawExchange` of the request sent and the response rebuilt.
- `read_body_capped` - reads a body the caller decodes whole, such as a non-success status's error body or a model list, refusing it past a byte cap.
- `escape_controls` - bounds a backend error body to a character count and escapes its control characters, so it cannot forge log lines.
- `classify_http_failure` and `classify_stream_error` - turn a non-success status and its escaped body, or an in-stream error envelope, into a `CompletionError` of the right kind, keeping the body as the opt-in `detail` and never in the message.

## Guarantees

- The bearer key never appears in logs, `Debug`, `Display`, or error text.
- A backend error body is bounded and control-escaped before it is kept. A chat round exposes it only through the opt-in `CompletionError::detail`, and a search keeps it in the `GatewaySearchError` message. A success stream or search body is refused once it passes the byte cap, before decoding.
- A keyless client is an explicit choice (`GatewayChat::keyless`, or `from_env` against a loopback URL); nothing here checks the endpoint's address on the caller's behalf.

## Minimum Rust Version

Rust 1.89 or later.

## License

Licensed under the [Boost Software License 1.0](../../LICENSE).
