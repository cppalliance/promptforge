//! harness-gateway-client - the standard way a Host talks to the
//! PromptForge Gateway: the HTTP client that sends a `Chat` effect's round
//! to the Gateway and fetches its model catalog, and the OpenAI
//! chat-completions wire code under it that turns the round into a
//! request body, a streamed reply into a
//! [`Completion`](promptforge::model::Completion), and a failed response
//! into the [`CompletionError`] the round carries.
//!
//! [`GatewayClient`] speaks the always-streaming `/chat/completions` SSE
//! shape to one Gateway URL with, usually, the Gateway's shared bearer
//! key: [`GatewayClient::complete`] sends the request body, reads the
//! stream under the run's byte cap and timeout, invokes the caller's delta
//! callback live, and returns the one completion the round produced.
//! [`fetch_model_catalog`] reads the Gateway's typed model list. The
//! client holds only the Gateway's URL and the shared key; the vendor
//! credential sits in the Gateway.
//!
//! [`GatewaySearch`] runs one web search through the Gateway's
//! `/tools/web_search` relay under a 30-second deadline and parses the
//! reply into a [`GatewaySearchResponse`]; the search vendor's credential
//! stays in the Gateway too.
//!
//! [`build_request_body`] builds the one JSON body every round sends.
//! [`read_completion_stream`] reads the SSE reply over a caller's
//! [`ChunkSource`] to its `[DONE]` sentinel under a byte cap, forwards each
//! live delta, and folds the stream into a completion under one strict turn
//! rule set. [`read_body_capped`] reads a body the caller decodes whole.
//! [`escape_controls`] bounds and escapes a backend error body, and
//! [`classify_http_failure`] and [`classify_stream_error`] turn a status or
//! an in-stream error envelope into a failure kind. The wire code opens no
//! connection and reads no clock: the client, or another broker, supplies
//! the chunks and the clock.
//!
//! ## Invariants
//!
//! - Family: Harness, at the `crates/` root beside `harness`; may depend
//!   on: `promptforge` and third-party crates only. Never on `harness` or
//!   any `crates/harness-internal` crate. `cargo test -p build-xtask`
//!   enforces the product and container boundaries.
//! - Every `Completion` and `ToolCall` is built through the public
//!   validating constructors, so the Engine's neutral reply checks run on
//!   every decoded turn.
//! - A Gateway bearer key is never written to logs, `Debug`, `Display`, or
//!   error text.
//! - A backend error body is bounded and control-escaped before it is
//!   kept: a chat round keeps it only as the opt-in
//!   `CompletionError::detail`, and a search keeps it in the
//!   `GatewaySearchError` message.
//! - A keyless client is an explicit choice; nothing here checks the
//!   endpoint's address on the caller's behalf.
//! - Every file in this crate stays under 500 lines; split first, then
//!   edit.

mod catalog;
mod config;
mod failure;
mod search;
mod transport;
mod wire;

pub use catalog::fetch_model_catalog;
pub use config::GatewayConfigError;
pub use config::GatewayEndpoint;
pub use config::SecretError;
pub use config::SecretString;
pub use promptforge::model::CompletionError;
pub use promptforge::model::CompletionErrorKind;
pub use search::GatewaySearch;
pub use search::GatewaySearchError;
pub use search::GatewaySearchErrorKind;
pub use search::GatewaySearchRequest;
pub use search::GatewaySearchResponse;
pub use search::GatewaySearchResult;
pub use transport::GatewayClient;
pub use wire::classify::classify_http_failure;
pub use wire::classify::classify_stream_error;
pub use wire::read::ChunkSource;
pub use wire::read::read_body_capped;
pub use wire::read::read_completion_stream;
pub use wire::request::build_request_body;
pub use wire::stream::escape_controls;
