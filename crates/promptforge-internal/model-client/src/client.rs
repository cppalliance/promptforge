//! The chat-completions protocol vocabulary: what a model round exchanges,
//! with no transport attached.
//!
//! The wire types ([`Message`], [`ToolSchema`], [`ToolCall`],
//! [`Completion`], [`CompletionResult`]) go out of the engine in a `Chat`
//! effect and come back in its answer. Beside them sit the
//! protocol pieces every transport shares, all `#[doc(hidden)]`
//! cross-crate seams: the request body builder, so one JSON shape leaves
//! for the gateway no matter who sends it; the SSE reassembly (scanner,
//! accumulator, and its `finish` into a [`Completion`]), so streamed and
//! buffered turns are judged by one rule set; and the read loop over a
//! transport's [`ChunkSource`], so the byte cap, the sentinel rule, and
//! the timing arithmetic live once.
//!
//! Nothing here opens a connection or reads a clock. The HTTP client that
//! sends the body and yields the chunks is the harness's
//! (`harness-models`); the engine's own suites drive the same protocol
//! through a dev-only client against a mock gateway. The engine itself
//! never performs a round: a model round is a `Chat` effect its host
//! performs and answers.

mod read;
mod request;
mod stream;
mod wire;

// Canonical in `promptforge-types`; re-exported so the
// `promptforge_model_client::client::StreamDelta` path keeps resolving.
pub use promptforge_types::wire::StreamDelta;
#[doc(hidden)]
pub use read::{ChunkSource, read_body_capped, read_completion_stream};
#[doc(hidden)]
pub use request::build_request_body;
#[doc(hidden)]
pub use stream::{Applied, SseScanner, StreamAccumulator, escape_controls};
#[doc(hidden)]
pub use wire::ToolSchemaError;
pub use wire::{Completion, CompletionResult, Message, ToolArguments, ToolCall, ToolSchema};

#[cfg(test)]
mod tests;
