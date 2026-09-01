//! An `OpenAI`-compatible chat completions client, pointed at the gateway.
//!
//! The client speaks the non-streaming `/chat/completions` shape: a list of
//! messages in, and either one text reply out or the tool calls the model
//! asked for. [`GatewayClient::complete`] sends a `tools` array when the caller
//! supplies one, so the executor's tool-call loop runs over this client.
//! Streaming is not supported. The client holds only the gateway's URL and the
//! shared key; the vendor credential lives in the gateway, so the executor
//! never sees it. Point `PROMPTFORGE_GATEWAY_URL` at a local server or another
//! gateway to retarget it.
//!
//! The implementation lives in the `promptforge-model-client` crate and is
//! re-exported here unchanged, so existing `promptforge_core::client::*` paths
//! keep working.

pub use promptforge_model_client::client::{
    Completion, CompletionResult, GatewayClient, GatewayEndpoint, Message, SecretError,
    SecretString, ToolArguments, ToolCall, ToolSchema,
};

#[cfg(test)]
pub(crate) use promptforge_model_client::client::ToolSchemaError;
