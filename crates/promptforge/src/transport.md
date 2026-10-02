Talk to your own model server: build chat request bodies, read streamed replies, and report failed rounds.

Promptforge never opens a connection, so you need this page whenever your program answers chat effects from a real model server. You can skip it only when other code already answers them for you, or when you answer with canned replies.

# Where this fits

A run asks your program for a model reply by handing you a chat [effect](crate). A round is one chat request your program sends for a chat effect, together with the reply or failure it gets back. The [models page](crate::model) answers that effect with a finished completion or its error. This page produces that completion, or that error, from a real server connection. Your program owns the connection, and these helpers turn a chat effect into a request body, a reply stream into a completion, and a failed response into the error the round carries.

# Read a streamed reply

Your run hands you a [`Chat`](crate::effect::Effect::Chat) effect. You send it to your own server, and you want the reply as a completion while you show its text as it arrives.

Reading a streamed reply feels like draining a `Stream` of byte chunks. Unlike a plain stream, you only supply the bytes, as a response body read one chunk at a time. That is a *chunk source*: any type that implements [`ChunkSource`]. The server streams OpenAI-style chat-completions events, each a `data:` line holding one JSON payload, ending with `data: [DONE]`. The reader, [`read_completion_stream`], decodes the server's events, forwards each piece of text to your callback, and returns the finished [`Completion`](crate::model::Completion).

You build the request body with [`build_request_body`]. You own the connection and the clock that times the round. The reader never reads the time itself, so a test can pass a fake clock and assert exact timings, while production passes [`Instant::now`](std::time::Instant::now).

````
# use std::cell::{Cell, RefCell};
# use std::collections::VecDeque;
# use std::future::Future;
# use std::num::NonZeroU32;
# use std::pin::pin;
# use std::sync::Arc;
# use std::task::{Context, Poll, Waker};
# use std::time::{Duration, Instant};
# use promptforge::effect::{Effect, EffectAnswer};
# use promptforge::model::{CompletionError, CompletionErrorKind, CompletionOptions, CompletionResult, Message};
# use promptforge::model::{ModelDescriptor, ModelId, StreamDelta, ThinkingMode};
# use promptforge::timestamp::Timestamp;
# use promptforge::transport::{ChunkSource, build_request_body, read_completion_stream};
# use promptforge::vfs::perform_vfs_op;
# use promptforge::{Environment, Prompt, Run, RunContext, RunResult, Step};
# let source = concat!(
#     "---\n",
#     "name: greeter\n",
#     "description: Writes a note and asks a model to reply to it.\n",
#     "promptforge: 0\n",
#     "models:\n",
#     "  writer: {}\n",
#     "---\n\n",
#     "# Greeter\n\n",
#     "## Greet\n\n",
#     "```lua\n",
#     "store.write('note.md', 'hello')\n",
#     "models.use('writer')\n",
#     "return models.infer(store.read('note.md'))\n",
#     "```\n",
# );
# let (parsed, _parse_events) = Prompt::parse(source, "greeter");
# let prompt = parsed?;
# let id = ModelId::gateway("canned")?;
# let window = NonZeroU32::new(8_192).ok_or("a context window is never zero")?;
# let model = ModelDescriptor::new(id, "Streams hello world", window, ThinkingMode::Never);
# let ctx = RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH).model(model);
# let (ctx, requirements) = Environment::new().prepare(&prompt, ctx);
# assert!(requirements.refusal().is_none());
# let mut run = Run::new(Arc::new(prompt), "", ctx);
# fn block_on<F: Future>(future: F) -> F::Output {
#     let mut future = pin!(future);
#     let mut cx = Context::from_waker(Waker::noop());
#     loop {
#         if let Poll::Ready(output) = future.as_mut().poll(&mut cx) {
#             return output;
#         }
#     }
# }
// 1. Your connection's response body is a chunk source; `Canned` serves the greeting's events from memory.
struct Canned(VecDeque<&'static str>);
impl ChunkSource for Canned {
    type Chunk = &'static str;
    async fn next_chunk(&mut self) -> Result<Option<Self::Chunk>, CompletionError> {
        Ok(self.0.pop_front())
    }
}
const GREETING: [&str; 3] = [
    "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hello\"}}]}\n\n",
    "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\" \"}}]}\n\n",
    "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"world\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n",
];

// 2. One round reads the stream, prints each text piece, and times itself with a clock that ticks 10 ms.
let shown = RefCell::new(Vec::new());
let stream_round = |body: serde_json::Value, max_bytes: u64| {
    let mut source = Canned(GREETING.into());
    let (started, ticks) = (Instant::now(), Cell::new(1));
    let now = || started + Duration::from_millis(10 * ticks.replace(ticks.get() + 1));
    let on_delta = |delta: StreamDelta| {
        if let StreamDelta::Text(piece) = delta {
            print!("{piece}");
            shown.borrow_mut().push(piece);
        }
    };
    block_on(read_completion_stream(&mut source, body, max_bytes, on_delta, started, now))
};

// 3. The Harness loop's chat arm builds the body from the effect, streams the reply, and checks what came back.
# let result = loop {
#     match run.step() {
#         Step::Pending { effects, .. } => {
#             for (id, _provenance, effect) in effects {
#                 let answer = match effect {
#                     Effect::Vfs { access, op } => EffectAnswer::Vfs(perform_vfs_op(&access, op)),
Effect::Chat { messages, tools, options, .. } => {
    let body = build_request_body(&messages, Some(tools.as_slice()), &options);
    let completion = stream_round(body, 1 << 20)?;
    assert_eq!(completion.result(), &CompletionResult::Text(shown.take().concat()));
    let timing = completion.metrics().and_then(|metrics| metrics.client.as_ref()).ok_or("the reader timed the round")?;
    assert_eq!((timing.ttft_ms, timing.e2e_ms), (Some(10.0), 40.0));
    EffectAnswer::Chat(Ok(Box::new(completion)))
}
#                     _ => EffectAnswer::Dropped,
#                 };
#                 run.resume(id, answer);
#             }
#         }
#         Step::Done { result, .. } => break result,
#     }
# };
assert!(matches!(result, RunResult::Ok(text) if text == "hello world"));

// 4. The same stream under a 64-byte limit fails the round after its first piece was already shown.
let body = build_request_body(&[Message::user("hello")], None, &CompletionOptions::new("canned"));
let error = stream_round(body, 64).err().ok_or("the stream passes 64 bytes")?;
assert_eq!((error.kind(), error.is_retryable()), (CompletionErrorKind::MalformedResponse, true));
assert_eq!(shown.take(), ["hello"]);
# Ok::<(), Box<dyn std::error::Error>>(())
````

1. `Canned` serves the reply from memory, one server-sent event per chunk, then `None`, so this runs offline. The future [`ChunkSource::next_chunk`] returns must be `Send`, so an `async fn` works only when your source and everything it holds across an `.await` are `Send`. A single-threaded client handle cannot implement the trait.
2. `stream_round` hands the reader the source, the body you sent, a byte limit, a callback, `started`, and `now`. Read `started` just before you send. Here `now` ticks 10 ms per call. The callback is `Fn`, so it collects pieces through a `RefCell`, and it never sees tool-call fragments. The hidden `block_on` stands in for your async runtime.
3. Step 3 is the chat arm of the Harness loop from [Answer a model](crate#answer-a-model), with the rest hidden.
   - It builds the body from the effect's messages, tools, and options. Every body asks for a stream, so send it only to a server that streams OpenAI-style chat completions.
   - The reader keeps the body you pass, with the response it rebuilds, as the completion's [`RawExchange`](crate::model::RawExchange), so pass the value you sent, and a run's debug capture shows exactly that.
   - A chunk is whatever one `next_chunk` call yields, and it can hold part of an event or several, as the third chunk here holds a payload and the `[DONE]` line. The reader counts payloads, not chunks.
   - The reader calls `now` once per payload holding text, reasoning, or a tool-call fragment, and once at the end. Three text payloads give a time to first token of 10 ms and an end-to-end time of 40 ms.
   - [`Completion::metrics`](crate::model::Completion::metrics) returns a [`CallMetrics`](crate::metrics::CallMetrics), and its `client` section is a [`ClientTiming`](crate::metrics::ClientTiming) with `ttft_ms`, `mean_itl_ms`, and `e2e_ms`, each rounded to a whole microsecond. `mean_itl_ms` averages the gaps between content payloads, 10 ms here. The other sections, `usage`, `llama`, and `vllm`, come from the stream's own `usage`, `timings`, and `metrics` objects when the backend sent them; a stream with no usage chunk has no `usage`.
   - Time to first token is `None` with no content payloads, and mean inter-token latency with fewer than two.
   - The final `data: [DONE]` line, newline included, lets the stream finish. Answering with the completion in [`EffectAnswer::Chat`](crate::effect::EffectAnswer::Chat) is the whole happy path.
4. Under a 64-byte limit, the 61-byte first chunk fits and `hello` is printed, then the second chunk passes the limit. The round fails with a [`CompletionErrorKind::MalformedResponse`](crate::model::CompletionErrorKind::MalformedResponse) error. The limit counts raw bytes, framing included. The error is *retryable*: [`CompletionError::is_retryable`](crate::model::CompletionError::is_retryable) returns `true`, a hint that resending may succeed. This crate never resends, so your connection decides.

````text
  Effect::Chat
       │
       │ build_request_body
       v
  request body ───── you send it ─────> model server
       │                                      │
       │                                      │ the reply's bytes, through your ChunkSource
       │                                      v
       └────── request_body ─────> read_completion_stream ───> on_delta prints "hello", " ", "world"
                                              │
                                              │ at data: [DONE]
                                              v
                                         Completion ───> EffectAnswer::Chat(Ok(..))
````

You might expect text to reach your callback only once the reply is known good. Instead, each piece is forwarded as it decodes, so your callback may already have shown text from a round that then fails, as step 4 did.

Build the body once, feed the bytes through your chunk source, and let your clock time the round. Next, [Report a failed round](#report-a-failed-round) handles a server that answers with an error.

# Report a failed round

Your server answered a chat request with a non-success status, or the connection broke. You want the run to see that failure as the chat effect's answer.

Reporting a failed round feels like mapping an HTTP client's error into your own enum with `From`. Unlike a plain mapping, you first cap and escape the server's error body, because the error keeps whatever string you give it, and then you hand the status and that text to a classifier that picks the failure's kind.

You call [`classify_http_failure`] with the status and the escaped body, and answer the chat effect with the [`CompletionError`](crate::model::CompletionError) it returns.

````
# use std::collections::VecDeque;
# use std::future::Future;
# use std::num::NonZeroU32;
# use std::pin::pin;
# use std::sync::Arc;
# use std::task::{Context, Poll, Waker};
# use promptforge::effect::{Effect, EffectAnswer};
# use promptforge::model::{CompletionError, CompletionErrorKind, ModelDescriptor, ModelId, ThinkingMode};
# use promptforge::timestamp::Timestamp;
# use promptforge::transport::ChunkSource;
# use promptforge::vfs::perform_vfs_op;
# use promptforge::{Environment, Prompt, Run, RunContext, RunResult, Step};
# let source = concat!(
#     "---\n",
#     "name: greeter\n",
#     "description: Writes a note and asks a model to reply to it.\n",
#     "promptforge: 0\n",
#     "models:\n",
#     "  writer: {}\n",
#     "---\n\n",
#     "# Greeter\n\n",
#     "## Greet\n\n",
#     "```lua\n",
#     "store.write('note.md', 'hello')\n",
#     "models.use('writer')\n",
#     "return models.infer(store.read('note.md'))\n",
#     "```\n",
# );
# let (parsed, _parse_events) = Prompt::parse(source, "greeter");
# let prompt = parsed?;
# let id = ModelId::gateway("canned")?;
# let window = NonZeroU32::new(8_192).ok_or("a context window is never zero")?;
# let model = ModelDescriptor::new(id, "Streams hello world", window, ThinkingMode::Never);
# let ctx = RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH).model(model);
# let (ctx, requirements) = Environment::new().prepare(&prompt, ctx);
# assert!(requirements.refusal().is_none());
# let mut run = Run::new(Arc::new(prompt), "", ctx);
# struct Canned(VecDeque<&'static str>);
# impl ChunkSource for Canned {
#     type Chunk = &'static str;
#     async fn next_chunk(&mut self) -> Result<Option<Self::Chunk>, CompletionError> {
#         Ok(self.0.pop_front())
#     }
# }
# fn block_on<F: Future>(future: F) -> F::Output {
#     let mut future = pin!(future);
#     let mut cx = Context::from_waker(Waker::noop());
#     loop {
#         if let Poll::Ready(output) = future.as_mut().poll(&mut cx) {
#             return output;
#         }
#     }
# }
use promptforge::transport::{classify_http_failure, escape_controls, read_body_capped};

// 1. The server answered 503, so read its whole error body under a byte cap.
fn failed_round(chunks: &[&'static str], cap: u64) -> CompletionError {
    let mut source = Canned(chunks.iter().copied().collect());
    let raw = match block_on(read_body_capped(&mut source, None, cap)) {
        Ok(raw) => raw,
        Err(refused) => return refused,
    };
    // 2. Decode and escape the body, then let the classifier turn the status and that text into the error.
    let body = escape_controls(&String::from_utf8_lossy(&raw), 2000);
    classify_http_failure(503, &body)
}

// 3. Answer the greeter's chat effect with that error, and step the run to its end.
let result = loop {
    match run.step() {
        Step::Pending { effects, .. } => {
            for (id, _provenance, effect) in effects {
                let answer = match effect {
                    Effect::Vfs { access, op } => EffectAnswer::Vfs(perform_vfs_op(&access, op)),
                    Effect::Chat { .. } => {
                        let error = failed_round(&["overloaded\nretry later"], 4096);
                        assert_eq!(error.kind(), CompletionErrorKind::Overloaded);
                        assert!(error.is_retryable());
                        assert_eq!(error.detail(), Some("overloaded\\nretry later"));
                        EffectAnswer::Chat(Err(error))
                    }
                    _ => EffectAnswer::Dropped,
                };
                run.resume(id, answer);
            }
        }
        Step::Done { result, .. } => break result,
    }
};

// 4. The run ends with that failure.
let RunResult::Failure(error) = result else { panic!("a failed round fails the greeter") };
assert_eq!(error.to_string(), "the model backend is overloaded (status 503)");

// 5. An error body over its cap fails the read itself, as a malformed response without the status.
let error = failed_round(&["overloaded, ", "retry later"], 16);
assert_eq!(error.kind(), CompletionErrorKind::MalformedResponse);
assert_eq!(
    error.to_string(),
    "the model backend sent a reply that could not be understood: response body exceeds the 16-byte limit"
);
assert_eq!(error.detail(), None);

// 6. A broken connection and a timeout carry no status. Build each from a kind, its phrase, and your own error.
let broken = CompletionError::new(
    CompletionErrorKind::Transport,
    "the connection to the model backend failed",
)
.with_source(std::io::Error::other("connection reset"));
assert!(broken.is_retryable());
assert_eq!(broken.to_string(), "the connection to the model backend failed");
let slow = CompletionError::new(
    CompletionErrorKind::Timeout,
    "the model backend did not answer in time",
)
.with_source(std::io::Error::other("no chunk for 120 seconds"));
assert_eq!(slow.kind(), CompletionErrorKind::Timeout);
assert!(std::error::Error::source(&slow).is_some());
# Ok::<(), Box<dyn std::error::Error>>(())
````

1. `failed_round` reads the whole 503 error body with [`read_body_capped`] under a 4096-byte cap. It passes `None` for the advertised length, and you pass yours when you have one. An advertised length over the cap fails at once, and the running total stops at the cap either way.
2. It decodes the bytes and runs them through [`escape_controls`] with a `max` of 2000 input characters. Then it passes the status and the escaped text to [`classify_http_failure`]. Escaping turns control characters such as newline, tab, and ESC into their escaped form, so a server's text cannot forge log lines or send terminal control sequences.
3. The chat arm checks that the kind is [`Overloaded`](crate::model::CompletionErrorKind::Overloaded) and that [`detail`](crate::model::CompletionError::detail) shows the newline as a backslash and an `n`. The kind fixes whether resending may help: `RateLimited`, `Overloaded`, `Timeout`, `Transport`, `ServerError`, and `MalformedResponse` are retryable, so a 429 is retryable too. This crate never resends, so your retry loop backs off on those kinds.
4. The run ends in [`RunResult::Failure`](crate::RunResult::Failure), whose message is the kind's fixed phrase with the status, and never the body. A 400 or 413 whose body names a context limit, such as "context length" or "too many tokens" in any case, is classified as `ContextOverflow`. That is not a failed round. The run hands it to the prompt's compactor as a context overflow. So always pass the real escaped body, and never pass a `max` of 0.
5. A 23-byte body under a 16-byte cap fails on its second chunk, as a `MalformedResponse`-kind error with no status. The message names the byte limit after the fixed phrase, because those words are this crate's own, and `detail` stays empty. So log the status before you read. The refusal leaves the source partly read, so drop the connection.
6. A broken connection is a [`Transport`](crate::model::CompletionErrorKind::Transport) kind error and a timeout is a [`Timeout`](crate::model::CompletionErrorKind::Timeout) kind error. Build each with [`CompletionError::new`](crate::model::CompletionError::new), the kind's fixed phrase, and [`with_source`](crate::model::CompletionError::with_source) for your own error. Both are retryable, and neither has a status.

When the connection breaks, build the `Transport` error as in step 6 and return it from `next_chunk`. The reader returns it unchanged, as a retryable transport failure. When the send itself fails, build the error the same way and answer the chat effect with it directly.

For a timeout, build the `Timeout` error as in step 6 instead, so a receive deadline you enforce inside `next_chunk` or around the send reads as a timeout. An error body that fails or times out partway has no status to report, so log the status first, and build the error from the read failure.

When your program lets its user switch model access off, answer every chat effect with an [`Unavailable`](crate::model::CompletionErrorKind::Unavailable) kind error whose message is `model access is turned off or not configured`. It is not retryable. Your own setup problems, such as a missing key or an unusable URL, are not model failures, so report them from your own code before any run starts.

You might expect the classifier to clean up the body you give it. Instead, it keeps the string exactly as given as the error's detail, so you cap and escape it yourself.

Cap the error body, escape it, then build the error the round carries. The [models page](crate::model) shows how a chat effect takes a finished completion or its error.

# Reference

## ChunkSource

Wrap your HTTP client's response body in a [`ChunkSource`], the only I/O the reading helpers touch, to hand it to [`read_body_capped`] or [`read_completion_stream`]. The helpers return a read failure from `next_chunk` unchanged, as the round's error. So build a `Transport` kind [`CompletionError`](crate::model::CompletionError) for a failed read, or a `Timeout` kind one for a timed-out read, with your transport's error as its source. The helpers read no clock, so enforce any receive timeout inside `next_chunk`. [Read a streamed reply](#read-a-streamed-reply) teaches it.

- `Chunk`: one chunk of body bytes in whatever buffer your client yields, such as `Vec<u8>` or [`bytes::Bytes`](https://docs.rs/bytes/latest/bytes/struct.Bytes.html).
- `next_chunk`: returns the next chunk, or `None` once the body is exhausted. Its future must be `Send`, so a single-threaded client handle cannot implement it.

## build_request_body

[`build_request_body`] builds the one chat-completions JSON body every transport sends for a [`Chat`](crate::effect::Effect::Chat) effect. Pass the effect's messages, tools, and options, and send the result as the request body. Later, hand the same value to [`read_completion_stream`]. Every body asks for a stream, with `stream: true` and `stream_options.include_usage: true`, so read every reply with `read_completion_stream`. [Read a streamed reply](#read-a-streamed-reply) teaches it.

- `tools`: `None` and `Some(&[])` give the same body. A non-empty slice adds each schema as a function tool and sets `tool_choice` to `"auto"`.
- `options`: unset options add no `temperature`, `max_tokens`, or `chat_template_kwargs` field, so the server chooses them.
- `options` thinking switch: sent as `chat_template_kwargs.enable_thinking`, a vLLM and llama.cpp field. `Some(false)` still sends it, and only `None` leaves it out.

## classify_http_failure

[`classify_http_failure`] turns a non-success status and its response body into the [`CompletionError`](crate::model::CompletionError) a failed round carries. Pass a body you already capped and escaped with [`escape_controls`]. The message is the kind's fixed phrase with ` (status N)` appended, and the body is kept as the error's [`detail`](crate::model::CompletionError::detail). It never reaches `Display`. [Report a failed round](#report-a-failed-round) teaches it.

The rules run in this order, and the first match wins:

| Status and body | Kind |
|---|---|
| 400 or 413, and the body names a context limit | `ContextOverflow` |
| 429, and the body names `quota`, `billing`, `insufficient_quota`, or `credit` | `QuotaExhausted` |
| any other 429 | `RateLimited` |
| 503 or 529, or any 5xx whose body names `overloaded` | `Overloaded` |
| any other 5xx | `ServerError` |
| 401 or 403 | `Unavailable` |
| 400, and the body names `content_filter`, `content policy`, `safety`, or `refus` | `Refused` |
| any other status | `Rejected` |

- `body`: matched case-insensitively. A context limit is one of `context length`, `context window`, `context size`, `context_length_exceeded`, `maximum context length`, `prompt is too long`, `too many tokens`, `exceeds the available context size`, `exceed_context_size`, `input is too long`, `exceeds the maximum number of tokens`, or `too large for model`. A body that matches no rule is `Rejected` (or `ServerError` for a 5xx), never a success.
- `ContextOverflow` counts: filled in when the text says "maximum context length is N tokens ... M tokens" or "N tokens > M maximum", and left unset otherwise. Read them back through [`CompletionError::overflow`](crate::model::CompletionError::overflow).
- 401 and 403: the message says `the model backend did not accept the credentials`.

## classify_stream_error

[`classify_stream_error`] applies the same body-text rules to an error that arrives inside a 200 stream, where there is no status. [`read_completion_stream`] calls it for you when a payload is an `error` envelope. Text that matches no rule is `Transport`, because the stream died in flight. The message is the kind's fixed phrase with no status, and the text is kept as the error's detail.

## escape_controls

[`escape_controls`] escapes control characters in a server's error body, so the body cannot forge log lines or send terminal control sequences. It also keeps at most `max` input characters, so every connection stores a backend body by one shared rule. Call it on the decoded body before you pass it to [`classify_http_failure`]. Only Unicode `Cc` control characters are escaped, into their `escape_default` form, so escape again for quoted or bidi-sensitive output. [Report a failed round](#report-a-failed-round) teaches it.

- `max`: counts input `char`s before escaping, so the output can be longer than `max`. Truncation adds no marker.
- `body`: an empty body returns the fixed text `(empty body)`, but a non-empty body with `max` of 0 returns an empty string.

## read_body_capped

[`read_body_capped`] reads a whole response body from a [`ChunkSource`], refusing it once it would pass `cap` bytes. Use it when you decode a body whole, such as a non-success status's error body or a model list. A refusal fails with a `MalformedResponse`-kind error that names the byte limit, not a classified HTTP failure, so keep the status yourself. Raise `cap` when a body is legitimately larger. [Report a failed round](#report-a-failed-round) teaches it.

- `content_length`: an advertised length over `cap` fails before any chunk is read. A smaller one is not trusted: the running total still stops at `cap`.
- `cap`: inclusive, so a body of exactly `cap` bytes, or an advertised length equal to it, is accepted.
- `source`: a failed read returns its own error unchanged. A refusal leaves it partly consumed, so do not read it again.

## read_completion_stream

[`read_completion_stream`] reads a streamed chat-completions reply to its `[DONE]` sentinel into a [`Completion`](crate::model::Completion), forwarding live text to your callback. Call it on a success status, then answer the `Chat` effect with the result. Too many bytes, a missing `[DONE]`, invalid JSON, or a cut-off tool-call batch fails as `MalformedResponse`, with what went wrong named after the fixed phrase. An in-stream `error` fails as [`classify_stream_error`] reads it, which is `Transport` unless its text names a known cause, and an empty turn as `EmptyReply`. The completion carries the round's [`CallMetrics`](crate::metrics::CallMetrics), absent when nothing was measured, and a [`RawExchange`](crate::model::RawExchange) of the request you passed and the response it rebuilt. [Read a streamed reply](#read-a-streamed-reply) teaches it.

- `max_bytes`: counts raw received bytes, event framing included, and a stream of exactly `max_bytes` passes.
- `on_delta`: gets [`StreamDelta::Text`](crate::model::StreamDelta::Text) for `content`, and [`StreamDelta::Reasoning`](crate::model::StreamDelta::Reasoning) for `reasoning_content`, `reasoning`, or `thinking`.
- `now`: your clock, and the only one the reader uses. It is called once per content payload and once at the end.
- `source`: must yield a newline-ended `data: [DONE]` line, or the round fails as cut off. The reader then stops, so you may close the connection.
