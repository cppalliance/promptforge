//! An `OpenAI`-compatible chat completions client, pointed at the gateway.
//!
//! The client speaks `/chat/completions` and always streams: every request
//! carries `stream: true` with `stream_options.include_usage`, and
//! [`GatewayClient::complete`] accumulates the SSE deltas into one
//! [`Completion`] - a text reply or the tool calls the model asked for -
//! while invoking the caller's delta callback with each live
//! [`StreamDelta`]. A caller with no use for deltas passes a no-op closure.
//! [`GatewayClient::complete`] sends a `tools` array when the caller
//! supplies one, so the executor's tool-call loop runs over this client.
//! The client holds only the gateway's URL and the shared key; the vendor
//! credential lives in the gateway, so the executor never sees it. Point
//! `PROMPTFORGE_GATEWAY_URL` at a local server or another gateway to
//! retarget it.

mod config;
mod stream;
mod transport;
mod wire;

pub use config::{GatewayEndpoint, SecretError, SecretString};
pub use transport::GatewayClient;
#[doc(hidden)]
pub use wire::ToolSchemaError;
pub use wire::{
    Completion, CompletionResult, Message, StreamDelta, ToolArguments, ToolCall, ToolSchema,
};

#[cfg(test)]
mod tests;
