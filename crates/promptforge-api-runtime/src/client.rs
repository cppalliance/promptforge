//! An `OpenAI`-compatible chat completions client, pointed at the gateway.
//!
//! The client speaks `/chat/completions` and always streams SSE internally:
//! [`GatewayClient::complete`] accumulates the deltas into one text reply or
//! the tool calls the model asked for, invoking the caller's delta callback
//! with each live delta. [`GatewayClient::complete`] sends a
//! `tools` array when the caller supplies one, so the executor's tool-call
//! loop runs over this client. The client holds only the gateway's URL and
//! the shared key; the vendor credential lives in the gateway, so the
//! executor never sees it. Point `PROMPTFORGE_GATEWAY_URL` at a local server
//! or another gateway to retarget it. [`fetch_model_catalog`] reads the
//! gateway's typed model list for host-side concerns (the Workshop dropdown
//! and its selection resolution); the list never crosses into the
//! environment an executor run prepares against.
//!
//! The implementation lives in the `promptforge-model-client` crate and is
//! re-exported here. The engine itself never performs a completion: a
//! model round is a `Chat` effect the host performs. This module is the
//! interim door through which Workshop's session machinery (via the
//! harness door's bridge) and the engine's own test driver reach the
//! client until the harness owns a model client of its own; it leaves with
//! that move. Hosts classify a round's failure through
//! [`CompletionError`].

pub use promptforge_model_client::client::{GatewayClient, GatewayEndpoint, SecretString};
pub use promptforge_model_client::model::{
    CompletionError, CompletionErrorKind, fetch_model_catalog,
};

#[cfg(any(test, feature = "test-support"))]
pub(crate) use promptforge_model_client::client::StreamDelta;
pub(crate) use promptforge_model_client::client::{
    Completion, CompletionResult, Message, ToolCall, ToolSchema,
};
