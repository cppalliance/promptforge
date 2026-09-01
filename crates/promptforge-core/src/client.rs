//! An `OpenAI`-compatible chat completions client, pointed at the gateway.
//!
//! The client speaks `/chat/completions` and always streams SSE internally:
//! [`GatewayClient::complete`] accumulates the deltas into one text reply or
//! the tool calls the model asked for, invoking the caller's delta callback
//! with each live [`StreamDelta`]. [`GatewayClient::complete`] sends a
//! `tools` array when the caller supplies one, so the executor's tool-call
//! loop runs over this client. The client holds only the gateway's URL and
//! the shared key; the vendor credential lives in the gateway, so the
//! executor never sees it. Point `PROMPTFORGE_GATEWAY_URL` at a local server
//! or another gateway to retarget it.
//!
//! The implementation lives in the `promptforge-model-client` crate and is
//! re-exported here unchanged, so existing `promptforge_core::client::*` paths
//! keep working.

pub use promptforge_model_client::client::{
    Completion, CompletionResult, GatewayClient, GatewayEndpoint, Message, SecretError,
    SecretString, StreamDelta, ToolArguments, ToolCall, ToolSchema,
};

#[cfg(test)]
pub(crate) use promptforge_model_client::client::ToolSchemaError;
