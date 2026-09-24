The sans-I/O model-round codec a host's transport runs.

A host performs an [`Effect::Chat`](crate::effect::Effect::Chat) by sending one chat-completions request and reading the streamed response. The codec here is every part of that round that is not I/O: the request body, the byte caps, the server-sent-events reassembly, and the timing arithmetic. It opens no connection and reads no clock; the host's transport sends the request, supplies the response body one chunk at a time, and hands over its clock. Every transport sharing this one rule set is what keeps two hosts from sending different requests for one effect or reading one response two ways.

# One round in four moves

1. Build the body with [`build_request_body`] from the effect's messages, tool schemas, and options, note the clock, and send the body as the JSON of a chat-completions request. Every request streams and asks for the final usage chunk.
2. Wrap the response body in a [`ChunkSource`]: the one trait a transport implements, returning each chunk of bytes or `None` at the end.
3. On a non-success status, read the error body whole with [`read_body_capped`], bound and escape it with [`escape_controls`] so a hostile body cannot forge log lines, and fail the round with [`ClientError::Backend`] converted into a [`CompletionError`](crate::model::CompletionError).
4. Otherwise hand the source to [`read_completion_stream`] with the body you sent, the byte cap, a delta callback, the clock reading from step 1, and the clock. It reads to the `[DONE]` sentinel, forwards each live [`StreamDelta`](crate::model::StreamDelta), and returns the [`Completion`](crate::model::Completion) that answers the effect as [`EffectAnswer::Chat`](crate::effect::EffectAnswer::Chat).

# Failures

A chunk source reports its own read failure as the round's [`CompletionError`](crate::model::CompletionError): box the transport's error into [`ClientError::Http`] and convert it. Wrap a timeout in [`ClientTimeout`] before boxing it, so [`CompletionError::is_timeout`](crate::model::CompletionError::is_timeout) still holds after the concrete type is erased. The codec itself fails a round as malformed when a body passes its cap, when the stream ends without the sentinel, or when a tool-call batch is cut short, since partial arguments must never run.

# Example

A transport over canned chunks, driven on the calling thread; a real transport's chunks come from its HTTP client, on whatever executor the host uses:

```
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
# Ok::<(), CompletionError>(())
```
