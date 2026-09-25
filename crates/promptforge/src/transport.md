The chat-completions codec that builds a request body and reads the response stream through any HTTP client.

A run hands every model round to the host as an [`Effect::Chat`](crate::effect::Effect::Chat), and this module lets the host perform that round with its own HTTP client. It supplies every part of the round that is not I/O: the request body, the byte caps, the reassembly of the server-sent-events stream, and the timing arithmetic. The host's transport sends the body and passes the response back one chunk at a time. Because every transport shares these functions, two hosts send the same request for one effect and read one response the same way. By the end of this page you can answer a chat effect from your own transport, show a reply as it streams, and fail a round with an error the run can classify.

# Where this fits

[`Run::step`](crate::Run::step) returns [`Step::Pending`](crate::Step::Pending) with effects, each a tuple of an [`EffectId`](crate::effect::EffectId), a [`Provenance`](crate::ids::Provenance), and an [`Effect`](crate::effect::Effect). When the effect is [`Effect::Chat`](crate::effect::Effect::Chat), the host's transport performs one model round.

1. Pass the effect's messages, tool schemas, and options to [`build_request_body`].
2. Read the host's own clock as the start instant, send the body as the JSON body of a chat-completions request, and wrap the response body in the transport's [`ChunkSource`].
3. On a non-success status, read the error body with [`read_body_capped`], bound and escape it with [`escape_controls`], and fail the round with [`ClientError::Backend`].
4. On success, call [`read_completion_stream`] with the same body, a byte cap, a delta callback, the start instant, and the clock. It returns a [`Completion`](crate::model::Completion).
5. Answer through [`Run::resume`](crate::Run::resume) under the same [`EffectId`](crate::effect::EffectId). A served round answers [`EffectAnswer::Chat`](crate::effect::EffectAnswer::Chat) with an [`Ok`] holding the boxed [`Completion`](crate::model::Completion). A failed round answers [`EffectAnswer::Chat`](crate::effect::EffectAnswer::Chat) with an [`Err`] holding a [`CompletionError`](crate::model::CompletionError) converted from a [`ClientError`].

[`EffectAnswer::record`](crate::effect::EffectAnswer::record) turns the answer into an [`AnswerRecord::Chat`](crate::effect::AnswerRecord::Chat) for the run log, and stores a failure as its [`Display`](std::fmt::Display) text. The run then reports the reply as events. [`Event::AssistantReply`](crate::event::Event::AssistantReply) carries call metrics whose [`CallMetrics::client`](crate::metrics::CallMetrics::client) section is the [`ClientTiming`](crate::metrics::ClientTiming) measured by [`read_completion_stream`].

The codec performs no I/O. It never opens a connection, sends a request, spawns a task, or reads a clock. The host's transport does that work and passes its clock in.

# A model round

This transport serves a canned response from memory and drives the codec on the calling thread. A real transport's chunks come from its HTTP client, on the host's own executor.

````
use std::collections::VecDeque;
use std::future::Future;
use std::pin::pin;
use std::task::{Context, Poll, Waker};
use std::time::Instant;

use promptforge::model::{CompletionError, CompletionOptions, CompletionResult, Message};
use promptforge::transport::{ChunkSource, build_request_body, read_completion_stream};

/// A response body that is already in memory.
struct Canned(VecDeque<&'static str>);

impl ChunkSource for Canned {
    type Chunk = &'static str;

    fn next_chunk(&mut self) -> impl Future<Output = Result<Option<Self::Chunk>, CompletionError>> + Send {
        std::future::ready(Ok(self.0.pop_front()))
    }
}

/// Polls a future that never waits to its output.
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut cx) {
            return output;
        }
    }
}

let options = CompletionOptions::new("analyst");
let body = build_request_body(&[Message::user("Say hi.")], None, &options);
assert_eq!(body["stream"], true);

let started = Instant::now();
let mut response = Canned(VecDeque::from([
    "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hi\"}}]}\n\n",
    "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
]));
let completion = block_on(read_completion_stream(
    &mut response,
    body,
    1 << 20,
    |_delta| {},
    started,
    Instant::now,
))?;
assert_eq!(completion.result(), &CompletionResult::Text("Hi".to_owned()));
assert_eq!(completion.finish_reason(), Some("stop"));
# Ok::<(), Box<dyn std::error::Error>>(())
````

Here is what each part does.

1. **Implement the chunk source.** `Canned` holds the response body as a queue of string chunks. Its [`ChunkSource::next_chunk`] returns a ready future with the next chunk, or [`None`] once the queue is empty.
2. **Drive the futures.** [`read_completion_stream`] and [`read_body_capped`] are async functions. `block_on` polls one with a no-op waker from [`Waker::noop`](std::task::Waker::noop), which works here because canned chunks never wait.
3. **Build the body.** [`CompletionOptions::new`](crate::model::CompletionOptions::new) names the model sent on the wire. The conversation is one [`Message::user`](crate::model::Message::user) message, and passing [`None`] offers no tools.
4. **Read the stream.** [`read_completion_stream`] takes the chunk source, the body that was sent, a byte cap of 1 MiB, a callback that ignores live deltas, the start instant, and the clock [`Instant::now`](std::time::Instant::now).
5. **Read the result.** [`Completion::result`](crate::model::Completion::result) is [`CompletionResult::Text`](crate::model::CompletionResult::Text) with `"Hi"`, and [`Completion::finish_reason`](crate::model::Completion::finish_reason) is `Some("stop")`. A host answers the effect with this completion.

# The request body

[`build_request_body`] builds the one chat-completions body that every transport sends for a chat effect. It returns a [`serde_json::Value`](https://docs.rs/serde_json/latest/serde_json/enum.Value.html). The host sends it as the request's JSON body and later passes the same value to [`read_completion_stream`].

Every body streams and asks for the final usage chunk. `"stream": true` and `"stream_options": {"include_usage": true}` are always set, so token accounting works over the stream with no extra setup. There is no non-streaming option. The function takes no stream argument, so the [`Effect::Chat::stream`](crate::effect::Effect#variant.Chat.field.stream) field does not change the body.

The tools argument controls whether the model is offered tools. With a non-empty slice of [`ToolSchema`](crate::model::ToolSchema) values, each schema is wrapped in the OpenAI function shape `{"type": "function", "function": {"name", "description", "parameters"}}` under `tools`, and `"tool_choice": "auto"` is set. With [`None`] or an empty slice, the body is a plain chat request and both keys are left out.

The options set everything else. The model name always becomes `model`. A temperature set with [`CompletionOptions::with_temperature`](crate::model::CompletionOptions::with_temperature) becomes `temperature`, a token cap set with [`CompletionOptions::with_max_tokens`](crate::model::CompletionOptions::with_max_tokens) becomes `max_tokens`, and a thinking switch set with [`CompletionOptions::with_thinking`](crate::model::CompletionOptions::with_thinking) becomes `chat_template_kwargs.enable_thinking`. Each of these keys appears only when its option is set.

````
use std::num::NonZeroU32;

use promptforge::model::{CompletionOptions, Message, ToolSchema};
use promptforge::transport::build_request_body;

let options = CompletionOptions::new("analyst")
    .with_temperature(0.2)?
    .with_max_tokens(NonZeroU32::new(256).ok_or("max tokens is non-zero")?)
    .with_thinking(false);
let no_tools: &[ToolSchema] = &[];
let body = build_request_body(&[Message::user("hi")], Some(no_tools), &options);

assert_eq!(body["model"], "analyst");
assert_eq!(body["stream"], true);
assert_eq!(body["stream_options"]["include_usage"], true);
assert_eq!(body["temperature"], 0.2);
assert_eq!(body["max_tokens"], 256);
assert_eq!(body["chat_template_kwargs"]["enable_thinking"], false);
assert!(body.get("tools").is_none());
assert!(body.get("tool_choice").is_none());
# Ok::<(), Box<dyn std::error::Error>>(())
````

# Chunk sources

A host plugs its HTTP client into the codec by implementing one trait, [`ChunkSource`], over the response body. The trait has one associated type, [`ChunkSource::Chunk`], and one required method, [`ChunkSource::next_chunk`]. Both readers, [`read_body_capped`] and [`read_completion_stream`], take the source by mutable reference.

[`ChunkSource::Chunk`] only has to implement [`AsRef`] of a byte slice, so the source can yield the HTTP client's own buffer type. The example above uses `&'static str`, and a [`Vec`] of bytes works as well. Chunk boundaries may fall anywhere, even in the middle of a line, because the stream reader buffers partial lines.

[`ChunkSource::next_chunk`] returns a [`Send`] future. The future resolves to the next chunk, to [`None`] at the end of the body, or to the transport's own read failure as a [`CompletionError`](crate::model::CompletionError). The readers pass that failure back unchanged and stop reading.

Because the codec performs no I/O, it runs on any executor, or on the calling thread with a no-op waker as the example above does.

# Live replies and timing

The callback argument of [`read_completion_stream`] lets a host show a reply as it arrives. The callback receives each text fragment as a [`StreamDelta::Text`](crate::model::StreamDelta::Text) and each reasoning fragment as a [`StreamDelta::Reasoning`](crate::model::StreamDelta::Reasoning), in stream order. Tool-call fragments are never passed to it. The callback is [`Fn`], not [`FnMut`], and is called synchronously, so collecting deltas needs interior mutability such as [`RefCell`](std::cell::RefCell) or [`Mutex`](std::sync::Mutex). Pass `|_delta| {}` when the host does not show the reply live.

The last two arguments hand the codec the host's clock. `started` is the clock reading from just before the request was sent, and `now` is the clock itself. The codec calls `now` once for each payload that holds generated content, meaning text, reasoning, or a tool-call fragment, and once more at the end. From those readings, the completion's [`ClientTiming`](crate::metrics::ClientTiming) holds three figures in milliseconds.

- [`ClientTiming::ttft_ms`](crate::metrics::ClientTiming::ttft_ms) is the time to first token: the first content payload's reading minus `started`. It is [`None`] if no payload held content.
- [`ClientTiming::mean_itl_ms`](crate::metrics::ClientTiming::mean_itl_ms) is the mean inter-token latency: the last content payload's reading minus the first, divided by one less than the number of content payloads. It is [`None`] with fewer than two content payloads.
- [`ClientTiming::e2e_ms`](crate::metrics::ClientTiming::e2e_ms) is the end-to-end time: the final reading minus `started`.

Every figure is rounded to a whole microsecond, so timings stored in a run log parse back bit-for-bit on replay. For example, `{"ttft_ms":1234.568,"mean_itl_ms":333.333,"e2e_ms":3703.704}` reads back equal. A fake clock makes the figures deterministic in tests.

This stream sends a reasoning fragment and then an answer in two fragments. The fake clock advances 10 ms on every reading.

````
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

use promptforge::model::{CompletionOptions, CompletionResult, Message, StreamDelta};
use promptforge::transport::{build_request_body, read_completion_stream};
# use std::future::Future;
# use std::pin::pin;
# use std::task::{Context, Poll, Waker};
# use promptforge::model::CompletionError;
# use promptforge::transport::ChunkSource;
# struct Canned(VecDeque<&'static str>);
# impl ChunkSource for Canned {
#     type Chunk = &'static str;
#     fn next_chunk(&mut self) -> impl Future<Output = Result<Option<Self::Chunk>, CompletionError>> + Send {
#         std::future::ready(Ok(self.0.pop_front()))
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

let body = build_request_body(&[Message::user("Think, then answer.")], None, &CompletionOptions::new("analyst"));
let mut response = Canned(VecDeque::from([
    "data: {\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"think\"}}]}\n\n",
    "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ans\"}}]}\n\n",
    "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"wer\"}}]}\n\n",
    "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
]));

let deltas = RefCell::new(Vec::new());
let started = Instant::now();
let ticks = Cell::new(0);
let now = || {
    ticks.set(ticks.get() + 1);
    started + Duration::from_millis(10 * ticks.get())
};
let completion = block_on(read_completion_stream(
    &mut response,
    body,
    1 << 20,
    |delta| deltas.borrow_mut().push(delta),
    started,
    now,
))?;

assert_eq!(completion.result(), &CompletionResult::Text("answer".to_owned()));
let deltas = deltas.into_inner();
assert!(matches!(
    deltas.as_slice(),
    [StreamDelta::Reasoning(r), StreamDelta::Text(a), StreamDelta::Text(b)]
        if r == "think" && a == "ans" && b == "wer"
));

let timing = completion.client_timing().ok_or("the codec measured the round")?;
assert_eq!(timing.ttft_ms, Some(10.0));
assert_eq!(timing.mean_itl_ms, Some(10.0));
assert_eq!(timing.e2e_ms, 40.0);
# Ok::<(), Box<dyn std::error::Error>>(())
````

The clock is read at 10, 20, and 30 ms for the three content payloads, and at 40 ms at the end. Reasoning arrives under any of the delta keys `reasoning_content`, `reasoning`, or `thinking`. It reaches the callback and the completion's reasoning text, and it never becomes part of the answer.

# What the stream reader checks

[`read_completion_stream`] applies one rule set for every transport.

- **Byte cap.** The `max_bytes` argument caps the total bytes of the stream, counted across all chunks. A stream of exactly that size is accepted. Past it, the round fails as malformed with "response stream exceeds the {max_bytes}-byte limit".
- **The sentinel.** Reading stops at `data: [DONE]`, and any remaining chunks stay unread. A stream that ends without it fails as malformed with "completion stream ended without the \[DONE\] sentinel", so a cut-off stream never passes for a complete turn.
- **Truncated tool calls.** A tool-call batch finished by `length` or `content_filter` fails with "tool-call batch truncated by finish_reason {reason:?}: partial arguments must not execute", so partial arguments never run. Truncated text with the finish reason `"length"` is still returned as a completion.
- **Interleaved tool calls.** Tool-call fragments are joined by their `index`, so calls whose fragments were streamed interleaved come back whole. Each call's id, name, and arguments grow by string concatenation.
- **Mid-stream errors.** A backend that fails after it has sent its 200 status reports the failure as an `error` envelope inside the stream. The round fails with kind [`CompletionErrorKind::Transport`](crate::model::CompletionErrorKind::Transport), and the error's cause holds the envelope's message passed through [`escape_controls`]. The payload `{"error":{"message":"upstream\ndied"}}`, whose message holds a real newline, gives a cause containing `upstream\ndied` with the newline escaped as a backslash and an `n`.
- **Undecodable chunks.** A payload that is not valid JSON fails as [`ClientError::MalformedResponseSource`] with the message "stream chunk was not valid JSON". The [`serde_json::Error`](https://docs.rs/serde_json/latest/serde_json/struct.Error.html) stays as the error's source, so a host can downcast it through the error chain instead of reading flattened text.
- **Empty turns.** A turn with neither non-empty tool calls nor non-empty text fails as [`ClientError::EmptyModelReply`].
- **Debug capture.** The completion stores the value passed as `request_body` beside the reassembled response body, so a run's debug capture holds exactly what was sent and received. Pass the exact value that [`build_request_body`] returned.

The stream reader skips noise in the server-sent-events format. Lines split on `\n` with a trailing `\r` removed, and invalid UTF-8 in a line is replaced lossily. Blank lines, `:` comments, and fields other than `data:`, such as `event:`, `id:`, and `retry:`, are skipped, and leading whitespace after `data:` is trimmed. Only the first choice, the one with `index` 0, is read, and other choices are ignored. The `usage`, `timings`, and `metrics` sections come from whichever chunk held them last, including the final usage chunk that has no choices. A malformed metadata section never fails the completion.

# Error bodies

On a non-success status, the response body is an error document that the transport reads whole instead of as a stream. [`read_body_capped`] reads a whole body from a chunk source under a hard byte cap. It also serves any JSON document the transport decodes whole, such as a model list. Pass the advertised `Content-Length` as its `content_length` argument, and a body that advertises more than the cap is refused before a single chunk is read. A gateway that omits or lies about its length still cannot force an unbounded read, because chunks are counted as they arrive and the read fails once the total would pass the cap.

A backend error body is untrusted text. [`escape_controls`] bounds it to a number of characters and escapes every control character, so the body cannot forge log lines or smuggle terminal control sequences into a diagnostic. An empty body becomes the fixed marker `(empty body)`. The result goes into [`ClientError::Backend`], whose [`Display`](std::fmt::Display) text leaves the body out. The body stays reachable through [`CompletionError::backend_body`](crate::model::CompletionError::backend_body).

````
use std::collections::VecDeque;

use promptforge::model::CompletionErrorKind;
use promptforge::transport::{escape_controls, read_body_capped};
# use std::future::Future;
# use std::pin::pin;
# use std::task::{Context, Poll, Waker};
# use promptforge::model::CompletionError;
# use promptforge::transport::ChunkSource;
# struct Canned(VecDeque<&'static str>);
# impl ChunkSource for Canned {
#     type Chunk = &'static str;
#     fn next_chunk(&mut self) -> impl Future<Output = Result<Option<Self::Chunk>, CompletionError>> + Send {
#         std::future::ready(Ok(self.0.pop_front()))
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

let mut source = Canned(VecDeque::from(["12345", "678"]));
let body = block_on(read_body_capped(&mut source, Some(8), 8))?;
assert_eq!(body, b"12345678");

let mut source = Canned(VecDeque::from(["never read"]));
let error = block_on(read_body_capped(&mut source, Some(100), 8))
    .err()
    .ok_or("the advertised length is over the cap")?;
assert_eq!(error.kind(), CompletionErrorKind::MalformedResponse);
assert!(error.to_string().contains("100 bytes"));
assert_eq!(source.0.len(), 1);

let mut source = Canned(VecDeque::from(["12345", "6789"]));
let error = block_on(read_body_capped(&mut source, None, 8))
    .err()
    .ok_or("the chunks pass the cap")?;
assert!(error.to_string().contains("8-byte limit"));

assert_eq!(escape_controls("line1\nline2\r\u{7}end", 2000), "line1\\nline2\\r\\u{7}end");
assert_eq!(escape_controls("abcdef", 3), "abc");
assert_eq!(escape_controls("", 2000), "(empty body)");
# Ok::<(), Box<dyn std::error::Error>>(())
````

The first read fits the cap exactly. The second is refused from its advertised length, and its only chunk is still in the queue. The third advertises nothing and fails when the second chunk would bring the total to 9 bytes.

Keep the escaped body in the error. The run recognizes a provider context-window overflow from a [`ClientError::Backend`] error whose status is 400 or 413 and whose body names a context limit, and treats it as an overflow instead of a plain failure. The recognized phrases, matched without regard to case, are `context length`, `context window`, `context size`, `context_length_exceeded`, `too many tokens`, and `prompt is too long`. For a section's chat round, the run refuses a request that is too large for the model's context window before it issues the effect, so this recognition covers a provider that rejects a request anyway.

# Failing a round

A transport never returns a [`ClientError`] directly. It builds one, converts it into a [`CompletionError`](crate::model::CompletionError) through [`From`] or [`Into`], and answers the effect with [`EffectAnswer::Chat`](crate::effect::EffectAnswer::Chat) holding that error in an [`Err`]. [`CompletionError`](crate::model::CompletionError) is `#[non_exhaustive]`, and outside the library a host can build one only from a [`ClientError`]. The inner [`ClientError`] stays reachable through [`source`](std::error::Error::source), and a [`CompletionError`](crate::model::CompletionError) converts back into a [`ClientError`] through [`From`].

Each way a round fails has its own variant.

- A send or read failure from the HTTP client goes into [`ClientError::Http`], boxed.
- A timeout is wrapped in [`ClientTimeout`] before it is boxed into [`ClientError::Http`] or [`ClientError::BackendBodyRead`]. That keeps [`CompletionError::is_timeout`](crate::model::CompletionError::is_timeout) working after the concrete error type is erased.
- A non-success status becomes [`ClientError::Backend`]. An error body that could not be read becomes [`ClientError::BackendBodyRead`], which keeps the status.
- A configuration failure becomes [`ClientError::MissingEnv`], [`ClientError::InvalidEnv`], [`ClientError::InvalidConfig`], or [`ClientError::Config`].
- When the host has turned model access off, it answers with [`ClientError::GatewayDisabled`].

This example builds three failures, reads their classification, and records one the way a run log stores it.

````
use promptforge::effect::{AnswerRecord, EffectAnswer};
use promptforge::model::{CompletionError, CompletionErrorKind};
use promptforge::transport::{ClientError, ClientTimeout, escape_controls};

let body = escape_controls("{\"error\":\"context length exceeded\"}\n", 2000);
let error = CompletionError::from(ClientError::Backend { status: 400, body });
assert_eq!(error.kind(), CompletionErrorKind::Backend);
assert_eq!(error.to_string(), "non-success backend status 400");
assert_eq!(error.status(), Some(400));
assert_eq!(error.backend_body(), Some("{\"error\":\"context length exceeded\"}\\n"));
assert!(!error.is_retryable());

let timeout = ClientTimeout(Box::new(std::io::Error::other("slow")));
let error: CompletionError = ClientError::Http(Box::new(timeout)).into();
assert_eq!(error.kind(), CompletionErrorKind::Transport);
assert!(error.is_timeout());
assert!(error.is_retryable());

let error = CompletionError::from(ClientError::GatewayDisabled);
assert_eq!(error.kind(), CompletionErrorKind::Disabled);
let text = error.to_string();
let answer = EffectAnswer::Chat(Err(error));
assert!(matches!(answer.record(), AnswerRecord::Chat(Err(recorded)) if recorded == text));

let back = ClientError::from(CompletionError::from(ClientError::GatewayDisabled));
assert!(matches!(back, ClientError::GatewayDisabled));
````

A backend status below 500 is not retryable, so [`CompletionError::is_retryable`](crate::model::CompletionError::is_retryable) is `false` for the first error. The recorded text is the error's [`Display`](std::fmt::Display) text, `gateway access is disabled`.

# Reference

This part covers every item in the module. Each codec function comes after the types in its signature.

## ClientTimeout

[`ClientTimeout`] marks a transport's timeout error, so that [`CompletionError::is_timeout`](crate::model::CompletionError::is_timeout) still returns `true` after the concrete error type is erased. The codec never names the transport's HTTP client, and this marker is how a timeout stays detectable without it.

The only way to build one is a tuple-struct literal around the transport's boxed timeout error. The type implements neither [`Default`] nor [`From`].

- [`ClientTimeout::0`] is a [`Box`] holding any [`std::error::Error`] that is [`Send`], [`Sync`], and `'static`. Fill it with the transport's own timeout error, such as the HTTP client's timeout error or an elapsed-deadline error, boxed with [`Box::new`].

Put the [`ClientTimeout`] itself into [`ClientError::Http`] or [`ClientError::BackendBodyRead`]. Detection is a direct downcast of the boxed value in those two variants, so a [`ClientTimeout`] nested deeper inside another error is not detected.

[`ClientTimeout`] implements [`Display`](std::fmt::Display) as `request timed out`. It implements [`std::error::Error`], and its [`source`](std::error::Error::source) is the wrapped error.

## ClientError

[`ClientError`] says why a model round failed, as a transport builds it. A transport constructs a variant directly, then converts it into a [`CompletionError`](crate::model::CompletionError) through [`From`] or [`Into`]. [`CompletionError::kind`](crate::model::CompletionError::kind) then classifies it into a [`CompletionErrorKind`](crate::model::CompletionErrorKind), and [`CompletionError::is_retryable`](crate::model::CompletionError::is_retryable) gives its retry class. [`ClientError`] is not `#[non_exhaustive]`, so a `match` over it can be exhaustive.

The codec functions raise [`ClientError::Http`] for a mid-stream error envelope, and they raise [`ClientError::MalformedResponse`], [`ClientError::MalformedResponseSource`], and [`ClientError::EmptyModelReply`], each already inside the returned [`CompletionError`](crate::model::CompletionError). The library raises [`ClientError::ModelSetLock`]. A transport builds the others, and it also builds the two malformed variants for a body it decodes itself.

**Configuration.** These four variants classify as [`CompletionErrorKind::Config`](crate::model::CompletionErrorKind::Config) and are not retryable. The host fixes its environment or configuration.

- [`ClientError::MissingEnv`] holds a [`String`], the name of a required environment variable that is not set. Its [`Display`](std::fmt::Display) text is `missing environment variable: {0}`. The in-repo gateway client builds it for an unset `PROMPTFORGE_GATEWAY_URL`, and for an unset `PROMPTFORGE_GATEWAY_API_KEY` when the URL is not loopback.
- [`ClientError::InvalidEnv`] holds a [`String`], the name of an environment variable that is set but not valid Unicode. Build it when reading the variable gives [`VarError::NotUnicode`](std::env::VarError::NotUnicode). Its text is `environment variable is set but not valid Unicode: {0}`.
- [`ClientError::InvalidConfig`] holds a [`String`], the full diagnostic for a configuration value that failed validation, and that string is the whole [`Display`](std::fmt::Display) text. The gateway client uses it for a URL with the wrong scheme, as `gateway URL must use the http or https scheme: ...`. Use [`ClientError::Config`] instead when there is an underlying error worth keeping as the cause.
- [`ClientError::Config`] reports invalid configuration input and keeps the concrete cause as its source instead of flattening it into the text. The gateway client builds it for an unparseable URL, with the message `gateway URL is not a valid URL: ...`, and for an unusable bearer key, with the message `gateway bearer key is unusable`.
  - [`ClientError::Config::message`](ClientError#variant.Config.field.message), a [`String`], is the human-readable diagnostic and the whole [`Display`](std::fmt::Display) text. Keep raw dumps of the cause out of it.
  - [`ClientError::Config::source`](ClientError#variant.Config.field.source), a [`Box`] holding any [`std::error::Error`] that is [`Send`] and [`Sync`], is the originating failure, such as a secret or URL validation error. Fill it with [`Box::new`]. It is what [`source`](std::error::Error::source) returns, on the [`ClientError`] and on the [`CompletionError`](crate::model::CompletionError) that wraps it.

**Access.**

- [`ClientError::GatewayDisabled`] means the host has turned model access off. A host answers a chat effect with it before sending anything. It classifies as [`CompletionErrorKind::Disabled`](crate::model::CompletionErrorKind::Disabled) and is not retryable. The round was deliberately not performed, so there is nothing to fix. Its text is `gateway access is disabled`.

**Transport and backend.**

- [`ClientError::Http`] holds the transport's own boxed error: a [`Box`] holding any [`std::error::Error`] that is [`Send`] and [`Sync`]. A transport builds it when a send fails or when [`ChunkSource::next_chunk`] fails, wrapping a timeout in [`ClientTimeout`] first. The codec raises it for a mid-stream error envelope, with a boxed [`std::io::Error`] whose message is `completion stream reported an error: ` followed by the envelope's message passed through [`escape_controls`] with a limit of 2000, or `stream error envelope omitted its message`. It classifies as [`CompletionErrorKind::Transport`](crate::model::CompletionErrorKind::Transport) and is retryable, and [`CompletionError::is_timeout`](crate::model::CompletionError::is_timeout) is `true` when the boxed value is a [`ClientTimeout`]. Its text is `http transport failure`, and the cause is its [`source`](std::error::Error::source).
- [`ClientError::Backend`] reports a non-success status. A transport builds it after reading the error body with [`read_body_capped`] and bounding it with [`escape_controls`]. It classifies as [`CompletionErrorKind::Backend`](crate::model::CompletionErrorKind::Backend), and it is retryable only when the status is 500 or above. Its text is `non-success backend status {status}` and never includes the body.
  - [`ClientError::Backend::status`](ClientError#variant.Backend.field.status), a [`u16`], is the HTTP status code the backend returned. [`CompletionError::status`](crate::model::CompletionError::status) returns it.
  - [`ClientError::Backend::body`](ClientError#variant.Backend.field.body), a [`String`], is the bounded, control-escaped response body. Fill it the way the gateway client does: decode the bytes from [`read_body_capped`] with [`String::from_utf8_lossy`], then pass the text to [`escape_controls`] with a limit of 2000. It is reachable only through [`CompletionError::backend_body`](crate::model::CompletionError::backend_body).
- [`ClientError::BackendBodyRead`] reports that the error body of a non-success status could not be read. It classifies as [`CompletionErrorKind::Transport`](crate::model::CompletionErrorKind::Transport) and is retryable, [`CompletionError::status`](crate::model::CompletionError::status) still returns the status, and [`CompletionError::is_timeout`](crate::model::CompletionError::is_timeout) is `true` when the boxed source is a [`ClientTimeout`]. Its text is `unreadable backend error body (status {status})`.
  - [`ClientError::BackendBodyRead::status`](ClientError#variant.BackendBodyRead.field.status), a [`u16`], is the non-success HTTP status whose body could not be read.
  - [`ClientError::BackendBodyRead::source`](ClientError#variant.BackendBodyRead.field.source), a [`Box`] holding any [`std::error::Error`] that is [`Send`] and [`Sync`], is the transport's read failure. Fill it with [`Box::new`] of the error, or of a [`ClientTimeout`] around it when the read timed out.

**Response.**

- [`ClientError::MalformedResponse`] holds a [`String`] diagnostic for a response that could not be understood. The codec raises it when a body passes its byte cap, when a stream ends without the sentinel, when a tool-call batch is truncated, when a stream chunk has a recognized field of the wrong shape, and when the reassembled turn has no usable choice, as in `no choices in response`. It classifies as [`CompletionErrorKind::MalformedResponse`](crate::model::CompletionErrorKind::MalformedResponse) and is retryable. Its text is `malformed response: {0}`.
- [`ClientError::MalformedResponseSource`] reports a response that could not be decoded and keeps the decoder's error as its cause. It classifies as [`CompletionErrorKind::MalformedResponse`](crate::model::CompletionErrorKind::MalformedResponse) and is retryable. Its text is `malformed response: {message}`.
  - [`ClientError::MalformedResponseSource::message`](ClientError#variant.MalformedResponseSource.field.message), a [`String`], is the human-readable diagnostic, without the raw body. The codec uses `stream chunk was not valid JSON`.
  - [`ClientError::MalformedResponseSource::source`](ClientError#variant.MalformedResponseSource.field.source), a [`Box`] holding any [`std::error::Error`] that is [`Send`] and [`Sync`], is the originating decode failure, such as a [`serde_json::Error`](https://docs.rs/serde_json/latest/serde_json/struct.Error.html). Fill it with [`Box::new`]. It survives as the [`source`](std::error::Error::source) of the wrapping [`CompletionError`](crate::model::CompletionError), where a caller can downcast it.
- [`ClientError::EmptyModelReply`] means the model returned neither non-empty tool calls nor non-empty text. [`read_completion_stream`] raises it, and a transport never builds it. It classifies as [`CompletionErrorKind::EmptyReply`](crate::model::CompletionErrorKind::EmptyReply) and is not retryable. Its text is the detail phrase. The run treats it as a completed round with no reply. An empty turn with the finish reason `"stop"` after successful tool calls is a clean exit, while a missing finish reason or `"length"` stays a failure.
  - [`ClientError::EmptyModelReply::detail`](ClientError#variant.EmptyModelReply.field.detail), a [`&'static str`](str), is either `empty model reply` or `empty model reply: reasoning content was present but ignored`.
  - [`ClientError::EmptyModelReply::finish_reason`](ClientError#variant.EmptyModelReply.field.finish_reason), an [`Option`] of [`String`], is the choice's finish reason when the backend supplied one, such as `"stop"` or `"length"`. [`CompletionError::finish_reason`](crate::model::CompletionError::finish_reason) returns it.

**Library.**

- [`ClientError::ModelSetLock`] holds a [`String`] message for a poisoned lock on the shared model set. The library raises it with the message `model set mutex was poisoned`, and a transport never builds it. It classifies as [`CompletionErrorKind::Config`](crate::model::CompletionErrorKind::Config) and is not retryable. Its text is the bare message. The run reclassifies it as its own scripting-layer error with the same wording.

[`ClientError`] implements [`std::error::Error`]. Its [`source`](std::error::Error::source) is the boxed cause for [`ClientError::Config`], [`ClientError::Http`], [`ClientError::MalformedResponseSource`], and [`ClientError::BackendBodyRead`], and [`None`] for every other variant. It converts into a [`CompletionError`](crate::model::CompletionError) through [`From`], and back the same way. It implements neither serde nor [`Default`].

## ChunkSource

[`ChunkSource`] is a response body read one chunk at a time. It is the one trait a transport implements, and the only I/O the codec touches. The host implements it over its own HTTP client's response body, and the library provides no implementation. The in-repo gateway client wraps its HTTP response together with a per-chunk timeout.

- [`ChunkSource::Chunk`] is the buffer type for one chunk of body bytes. Set it to the HTTP client's own chunk type. Any type that implements [`AsRef`] of a byte slice works, such as `&'static str` or a [`Vec`] of [`u8`].
- [`ChunkSource::next_chunk`] takes `&mut self` and no other arguments. It returns a [`Future`](std::future::Future) that must be [`Send`]. The future resolves to [`Ok`] of [`Some`] chunk for the next chunk of body bytes, or to [`Ok`] of [`None`] once the body is exhausted. On a read failure it resolves to an [`Err`] holding a [`CompletionError`](crate::model::CompletionError): box the transport's error into [`ClientError::Http`] and convert it, wrapping a timeout in [`ClientTimeout`] first. The readers propagate an [`Err`] unchanged and stop reading. [`read_completion_stream`] also stops calling once it sees the sentinel.

## build_request_body

[`build_request_body`] builds the chat-completions request body for one chat effect. It takes three arguments.

- `messages`, a slice of [`Message`](crate::model::Message) values, is the conversation for this round. Pass the chat effect's messages. They are serialized as they are into the body's `messages` array.
- `tools`, an [`Option`] of a slice of [`ToolSchema`](crate::model::ToolSchema) values, is the set of tools offered to the model. Pass the chat effect's tools in [`Some`]. A non-empty slice fills `tools` and sets `tool_choice`. [`None`] or an empty slice leaves both out.
- `options`, a reference to [`CompletionOptions`](crate::model::CompletionOptions), holds the frozen per-call options. Pass the chat effect's options, or build them with [`CompletionOptions::new`](crate::model::CompletionOptions::new) and its setters. Unset options are left out of the body.

It returns a [`serde_json::Value`](https://docs.rs/serde_json/latest/serde_json/enum.Value.html) object with `model`, `messages`, `"stream": true`, `"stream_options": {"include_usage": true}`, and the optional keys described in [The request body](#the-request-body). Send it and pass the same value to [`read_completion_stream`]. The in-repo gateway client posts it to `{base_url}/chat/completions`. The function cannot fail, and it is `#[must_use]`.

## escape_controls

[`escape_controls`] bounds a diagnostic text to a character count and escapes its control characters. It takes two arguments.

- `body`, a [`&str`](str), is the diagnostic text, typically a backend error body. Decode raw bytes with [`String::from_utf8_lossy`] first.
- `max`, a [`usize`], is the most characters of the text to keep, counted as Unicode scalar values, not bytes. The in-repo gateway client and the codec both use 2000.

It returns a [`String`]. Empty text gives `(empty body)`, whatever `max` is. Otherwise the result is the first `max` characters of the text, with each control character replaced by its [`char::escape_default`] form, such as `\n`, `\r`, or `\u{7}`. Truncation counts input characters before escaping, so the output can be longer than `max` characters. Only characters for which [`char::is_control`] is `true` are escaped, and backslashes and quotes pass through unchanged. Store the result in [`ClientError::Backend::body`](ClientError#variant.Backend.field.body). The function cannot fail, and it is `#[must_use]`.

## read_body_capped

[`read_body_capped`] is an async function that reads a whole response body from a [`ChunkSource`] and refuses it once it would exceed a byte cap. Use it for a body the transport decodes whole, such as the error body of a non-success status or a JSON model list. It takes three arguments.

- `source`, a mutable reference to any [`ChunkSource`], is the response body.
- `content_length`, an [`Option`] of [`u64`], is the advertised body length when the transport knows it, such as the HTTP client's content length, and [`None`] otherwise. An advertised length over the cap fails before any chunk is read.
- `cap`, a [`u64`], is the most bytes accepted. A body of exactly `cap` bytes is accepted. The in-repo gateway client passes its configured maximum response size.

It returns a [`Result`] holding a [`Vec`] of [`u8`] with the whole body. For an error body, the host decodes it with [`String::from_utf8_lossy`], passes it to [`escape_controls`], and builds [`ClientError::Backend`]. For a JSON document, the host hands it to its JSON decoder.

It fails with a [`CompletionError`](crate::model::CompletionError) of kind [`CompletionErrorKind::MalformedResponse`](crate::model::CompletionErrorKind::MalformedResponse) in two cases. When `content_length` exceeds `cap`, the message is `response body of {len} bytes exceeds the {cap}-byte limit` and nothing is read. When the chunks received would pass `cap`, the message is `response body exceeds the {cap}-byte limit`, and the chunk that would pass it is not appended. A read failure from the source comes back unchanged. Reading stops at the first failure.

## read_completion_stream

[`read_completion_stream`] is an async function that reads a completion's server-sent-events stream to its sentinel under a byte cap. It forwards each live delta to a callback and finishes the reassembled turn into a [`Completion`](crate::model::Completion). It takes six arguments.

- `source`, a mutable reference to any [`ChunkSource`], is the success response's body.
- `request_body`, a [`serde_json::Value`](https://docs.rs/serde_json/latest/serde_json/enum.Value.html), is the request body as sent. Pass the value that [`build_request_body`] returned. It is stored unchanged on the completion.
- `max_bytes`, a [`u64`], is the most stream bytes accepted, counted across all chunks. A stream of exactly `max_bytes` bytes is accepted. The in-repo gateway client passes its configured maximum response size, as it does for [`read_body_capped`].
- `on_delta`, any [`Fn`] that takes a [`StreamDelta`](crate::model::StreamDelta), is called synchronously for each non-empty text or reasoning fragment of the first choice, in arrival order. Pass `|_delta| {}` to ignore them.
- `started`, an [`Instant`](std::time::Instant), is the transport's clock reading from just before it sent the request.
- `now`, any [`Fn`] that returns an [`Instant`](std::time::Instant), is the transport's clock. Pass [`Instant::now`](std::time::Instant::now), or a fake clock in tests.

It returns a [`Result`] holding the [`Completion`](crate::model::Completion). The completion holds the result, either text or tool calls, the finish reason, the reasoning text, the serving model's name, usage, llama.cpp timings, vLLM metrics, the client timing, metadata diagnostics, the request body, and the reassembled response body. The host answers the chat effect with it through [`EffectAnswer::Chat`](crate::effect::EffectAnswer::Chat). [Live replies and timing](#live-replies-and-timing) explains the client timing.

It fails with a [`CompletionError`](crate::model::CompletionError). [What the stream reader checks](#what-the-stream-reader-checks) gives the messages.

- Kind [`CompletionErrorKind::MalformedResponse`](crate::model::CompletionErrorKind::MalformedResponse): the stream passes `max_bytes`, ends without the sentinel, has a payload that is not valid JSON, or has a recognized field of the wrong shape, such as "stream chunk `choices` was present but not an array". A tool-call batch that finishes with `length` or `content_filter`, and a turn with no usable choice, fail the same way.
- Kind [`CompletionErrorKind::Transport`](crate::model::CompletionErrorKind::Transport): a payload is a mid-stream `error` envelope. The escaped envelope message is the cause.
- Kind [`CompletionErrorKind::EmptyReply`](crate::model::CompletionErrorKind::EmptyReply): the turn has neither non-empty tool calls nor non-empty text.
- A read failure from the source comes back unchanged.

[`ClientTimeout::0`]: ClientTimeout
